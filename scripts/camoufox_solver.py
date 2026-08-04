#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""camoufox 验证码求解 HTTP 服务（GAP 175）——默认端口 8196

camoufox（Playwright 封装，Firefox 内核 + 真实指纹预设：navigator/screen/WebGL/
字体/canvas 噪声等）替代手搓 stealth：求解 Cloudflare 质询/Turnstile managed
challenge（如 69shuba 的强质询页）。

协议（reader-dev 后端 browser.rs camoufox 后端调用）：
- GET  /health            → {"ok": true, "camoufoxVersion": "...", "browserReady": bool}
- POST /solve             → 请求 {"url": str, "cookies": [{"name","value"}],
                                   "maxWaitMs": int（默认 60000）}
                            成功 {"html": str, "cookies": [{"name","value"}],
                                  "userAgent": str, "turnstileToken": str,
                                  "diagnostics": {...}}
                            失败 HTTP 200 + {"error": str, "diagnostics": {...}}
                            （200 承载错误——Rust 侧统一走 JSON 解析）

求解流程：常驻 camoufox 浏览器（惰性启动）→ 每请求新建指纹 context（os=windows
预设，随机指纹）→ 注入书源 cookie → 导航 → 质询等待循环（每 500ms 求值：
cf-turnstile-response input 值非空 / challenge 特征消失 / 标题离开 Just a moment
→ 通过；Turnstile iframe 存在 → Playwright 坐标点击勾选）→ 60s 超时 → 提取
最终 HTML + 站点 cookie + UA。

依赖：pip install camoufox && camoufox fetch（浏览器二进制）
用法：python scripts/camoufox_solver.py [--port 8196] [--host 127.0.0.1]
测试：python -m py_compile scripts/camoufox_solver.py
      GET /health；POST /solve 指向 scripts/mock-cf-site.py（8193）验证质询自动过

69shuba 实测（2026-08-04）：camoufox 过 Cloudflare（首页直过、无质询）；但
search.php 命中站点级 UA 门禁（"请使用新版本的Google Chrome"——camoufox 为
Firefox 指纹，context user_agent 会被指纹注入脚本覆盖）——搜索链路仍不可全自动，
需手动 Cookie 兜底（书源自带引导）。
"""
import argparse
import asyncio
import json
import os
import sys
import time

from camoufox.async_api import AsyncCamoufox, AsyncNewContext

PORT = int(os.environ.get("CAMOUFOX_SOLVER_PORT", "8196"))
DEFAULT_MAX_WAIT_MS = 60000

# 质询状态求值 JS（与 browser.rs CF_CHALLENGE_STATE_JS / TURNSTILE_DETECT_JS 同特征）：
# challenge = 仍在质询页；hasInput = Turnstile 隐藏 input 已渲染（managed challenge
# 勾选成功的标志）；inputValue = cf-turnstile-response 值（非空即通过）
CHALLENGE_STATE_JS = """
(function(){
  try {
    var features = document.querySelector('#challenge-form, [id^="cf-chl-"], [class*="cf-chl"], iframe[src*="challenges.cloudflare.com"]');
    var t = (document.title || '').toLowerCase();
    var input = document.querySelector('[name="cf-turnstile-response"]');
    return {
      challenge: !!(features || t.indexOf('just a moment') >= 0 || t.indexOf('turnstile') >= 0 || t.indexOf('verifying') >= 0),
      hasInput: !!input,
      inputValue: input && input.value ? input.value : '',
      title: document.title || '',
      url: location.href,
      bodyChildren: document.body ? document.body.children.length : 0
    };
  } catch (e) { return { challenge: true, hasInput: false, inputValue: '', title: '', url: '', bodyChildren: 0 }; }
})()
"""

_browser = None
_browser_ready = False


async def get_browser():
    """惰性启动常驻 camoufox 浏览器（进程生命周期内复用；并发请求经锁排队）"""
    global _browser, _browser_ready
    if _browser is None:
        _browser = await AsyncCamoufox(headless=True, humanize=True).__aenter__()
        _browser_ready = True
    return _browser


def cookies_for_host(cookies, host):
    """筛选目标主机（含父域）的 cookie——与 browser.rs cookie_domain_matches 同语义"""
    out = []
    for c in cookies:
        dom = (c.get("domain") or "").lstrip(".")
        if not dom:
            continue
        if host == dom or (dom.count(".") >= 1 and host.endswith("." + dom)):
            out.append({"name": c.get("name", ""), "value": c.get("value", "")})
    return out


async def solve_once(browser, url, cookies, max_wait_ms, user_agent=None):
    """单次求解：新建指纹 context → 导航 → 质询等待循环 → 结果/诊断"""
    host = None
    try:
        from urllib.parse import urlparse

        host = urlparse(url).hostname or ""
    except Exception:
        host = ""
    diag = {"title": "", "hasInput": False, "url": url, "waitMs": 0, "clicks": 0}
    ctx_kwargs = {}
    # 可选 UA 覆盖（部分站点 UA 门禁——如 69shuba 要求 Chrome；camoufox 指纹默认 Firefox UA）
    if user_agent:
        ctx_kwargs["user_agent"] = user_agent
        diag["userAgent"] = user_agent
    context = await AsyncNewContext(browser, os="windows", **ctx_kwargs)
    try:
        page = await context.new_page()
        # 书源既有 cookie 注入（domain 由目标主机推导）
        if cookies and host:
            try:
                await context.add_cookies(
                    [
                        {
                            "name": c["name"],
                            "value": c["value"],
                            "domain": host,
                            "path": "/",
                            "sameSite": "Lax",
                        }
                        for c in cookies
                        if c.get("name") and c.get("value") is not None
                    ]
                )
            except Exception as e:
                diag["cookieError"] = str(e)[:200]
        # 导航（domcontentloaded 即可进入等待循环；导航失败 → 明确错误）
        try:
            await page.goto(url, wait_until="domcontentloaded", timeout=min(max_wait_ms, 60000))
        except Exception as e:
            return {"error": f"导航失败: {e}"}, diag
        # 质询等待循环：每 500ms 求值——input 值非空（Turnstile 通过）或
        # challenge 特征消失（经典 CF JS 质询自动解）→ 退出
        deadline = time.monotonic() + max_wait_ms / 1000.0
        while True:
            try:
                state = await page.evaluate(CHALLENGE_STATE_JS)
            except Exception:
                state = {"challenge": True, "hasInput": False, "inputValue": "", "title": "", "url": "", "bodyChildren": 0}
            diag["title"] = state.get("title", "")
            diag["hasInput"] = bool(state.get("hasInput"))
            diag["waitMs"] = int((time.monotonic() - (deadline - max_wait_ms / 1000.0)) * 1000)
            if state.get("inputValue"):
                break  # Turnstile token 已生成 → 通过
            if not state.get("challenge"):
                break  # 质询特征消失 → 通过
            if time.monotonic() >= deadline:
                return {"error": f"质询求解超时（{max_wait_ms / 1000:.0f}s）——页面仍停留在质询页（title={diag['title']!r} hasInput={diag['hasInput']}）"}, diag
            # Turnstile widget iframe → Playwright 坐标点击勾选（比 CDP 坐标数学更稳）
            if not state.get("hasInput"):
                try:
                    frame = next(
                        (f for f in page.frames if "challenges.cloudflare.com" in (f.url or "")),
                        None,
                    )
                    if frame is not None:
                        await frame.click("body", timeout=3000)
                        diag["clicks"] += 1
                    else:
                        loc = page.locator('iframe[src*="challenges.cloudflare.com"]')
                        if await loc.count() > 0:
                            await loc.first.click(timeout=3000)
                            diag["clicks"] += 1
                except Exception:
                    pass
            await asyncio.sleep(0.5)
        # 稳定等待（质询跳转后的重绘）+ 提取最终 HTML / cookie / UA / Turnstile token
        await asyncio.sleep(1.0)
        try:
            html = await page.evaluate("document.documentElement.outerHTML")
        except Exception:
            html = await page.content()
        ua = ""
        try:
            ua = await page.evaluate("navigator.userAgent") or ""
        except Exception:
            pass
        token = ""
        try:
            token = await page.evaluate(
                "(function(){var el=document.querySelector('[name=\"cf-turnstile-response\"]');"
                "return el&&el.value?el.value:'';})()"
            ) or ""
        except Exception:
            pass
        all_cookies = await context.cookies()
        site_cookies = cookies_for_host(all_cookies, host)
        return {
            "html": html,
            "cookies": site_cookies,
            "userAgent": ua,
            "turnstileToken": token,
            "diagnostics": diag,
        }, diag
    finally:
        try:
            await context.close()
        except Exception:
            pass


async def handle_solve(reader):
    """POST /solve：读 Content-Length body → 求解 → JSON 响应"""
    try:
        length = 0
        while True:
            line = await asyncio.wait_for(reader.readline(), timeout=10)
            if not line or line in (b"\r\n", b"\n"):
                break
            low = line.strip().lower()
            if low.startswith(b"content-length:") and b":" in line:
                length = int(line.split(b":", 1)[1].strip() or 0)
        body = (
            await asyncio.wait_for(reader.readexactly(length), timeout=10) if length else b""
        )
        payload = json.loads(body.decode("utf-8", "replace")) if body else {}
    except Exception as e:
        return 400, {"error": f"请求解析失败: {e}"}
    url = str(payload.get("url") or "")
    if not url:
        return 400, {"error": "url 不能为空"}
    cookies = payload.get("cookies") or []
    max_wait_ms = int(payload.get("maxWaitMs") or DEFAULT_MAX_WAIT_MS)
    user_agent = str(payload.get("userAgent") or "") or None
    try:
        browser = await get_browser()
    except Exception as e:
        return 502, {"error": f"camoufox 浏览器启动失败: {e}（请先执行 camoufox fetch 下载浏览器）"}
    result, diag = await solve_once(browser, url, cookies, max_wait_ms, user_agent)
    result["diagnostics"] = diag
    return 200, result


async def handle_client(reader, writer):
    """极简 HTTP/1.1（单请求/连接——reqwest 客户端兼容）"""
    try:
        request_line = await asyncio.wait_for(reader.readline(), timeout=10)
        if not request_line:
            return
        parts = request_line.decode("latin-1", "replace").split()
        if len(parts) < 2:
            return
        method, path = parts[0].upper(), parts[1]
        if method == "GET" and path in ("/health", "/health/"):
            status, payload = 200, {
                "ok": True,
                "camoufoxVersion": "0.5.4",
                "browserReady": _browser_ready,
                "port": PORT,
            }
        elif method == "POST" and path in ("/solve", "/solve/"):
            status, payload = await handle_solve(reader)
        else:
            status, payload = 404, {"error": "not found（GET /health | POST /solve）"}
        data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        writer.write(
            (
                f"HTTP/1.1 {status} OK\r\n"
                "Content-Type: application/json; charset=utf-8\r\n"
                f"Content-Length: {len(data)}\r\n"
                "Connection: close\r\n"
                "\r\n"
            ).encode("latin-1")
            + data
        )
    except Exception:
        pass
    finally:
        try:
            await writer.drain()
        except Exception:
            pass
        writer.close()


async def amain(host, port):
    server = await asyncio.start_server(handle_client, host, port)
    print(f"CAMOUFOX_SOLVER listening on {host}:{port}（camoufox 常驻浏览器惰性启动）", flush=True)
    async with server:
        await server.serve_forever()


def main():
    ap = argparse.ArgumentParser(description="camoufox 验证码求解 HTTP 服务（GAP 175）")
    ap.add_argument("--port", type=int, default=PORT)
    ap.add_argument("--host", default="127.0.0.1")
    args = ap.parse_args()
    try:
        asyncio.run(amain(args.host, args.port))
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
