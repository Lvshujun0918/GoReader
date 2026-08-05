#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""camoufox 验证码求解 HTTP 服务（GAP 175）——默认端口 8196

camoufox（Playwright 封装，Firefox 内核 + 真实指纹预设：navigator/screen/WebGL/
字体/canvas 噪声等）替代手搓 stealth：求解 Cloudflare 质询/Turnstile managed
challenge（如 69shuba 的强质询页）。

协议（reader-dev 后端 browser.rs camoufox 后端调用）：
- GET  /health            → {"ok": true, "camoufoxVersion": "...", "browserReady": bool}
- POST /solve             → 请求 {"url": str, "cookies": [{"name","value"}],
                             "maxWaitMs": int（默认 60000）,
                             "userAgent": str（可选——Chrome Windows UA 覆盖，自动补
                               sec-ch-ua 头；69shuba UA 门禁必需）,
                             "post": {"action", "body", "contentType"?, "charset"?}
                               （可选——质询通过后页内 fetch POST，同源 cookie/referer
                                 自动携带；69shuba 搜索 search.php 用，charset=gbk）}
                            成功 {"html": str, "cookies": [{"name","value"}],
                                  "userAgent": str, "turnstileToken": str,
                                  "postResult": {"status", "url", "html"}?（post 时）,
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

69shuba 实测（2026-08-05 续）：UA 覆盖已跑通——Playwright user_agent 选项只改线上
（wire）UA，camoufox 指纹注入脚本会另把 navigator.userAgent 改回 Firefox；必须用
config_overrides={'navigator.userAgent': ...} 让 wire 与 JS 两侧一致为 Chrome。
69shuba 首页在 Chrome wire UA 下直过 CF（无质询、无 UA 门禁——门禁文案
"请使用新版本的Google Chrome" 实际是 search.php 的 Turnstile 挑战页横幅）；搜索
search.php 是站点级 Turnstile managed challenge（sitekey 0x4AAAAAAAarpkvdua7P4myE，
token 走 /verify.php 设 cookie 后 reload）——用 post.mode="navigate" 表单提交触发
widget 并自动点击。
2026-08-06 结论：本机（DMIT AS906 美西数据中心 IP）Turnstile 挑战平台直接拒绝
（iframe 文档 event:fail code:400030——环境风控，与 UA/头/指纹无关——真 Chrome
无自动化 flag 同样失败）；需住宅 IP 代理（camoufox 支持 proxy）才能全自动。
"""
import argparse
import asyncio
import json
import os
import re
import sys
import time

from camoufox.async_api import AsyncCamoufox, AsyncNewContext

PORT = int(os.environ.get("CAMOUFOX_SOLVER_PORT", "8196"))
DEFAULT_MAX_WAIT_MS = 60000

# UA 覆盖（69shuba 等站点 UA 门禁）：Playwright user_agent 选项只改线上（wire）UA——
# camoufox 指纹注入脚本（setNavigatorUserAgent）会把 JS 可见 navigator.userAgent 改回
# Firefox，两侧不一致会触发站点门禁/指纹检测。正解：generate_context_fingerprint 的
# config_overrides={'navigator.userAgent': ...}——wire UA 与 JS UA 同时为覆盖值。
# 回退：AsyncNewContext(user_agent=...) + 追加 init script 二次补丁 navigator.userAgent。
UA_PATCH_INIT_JS = """
(() => {
  const ua = %r;
  try { Object.defineProperty(Navigator.prototype, 'userAgent', { get: () => ua, configurable: true }); } catch (e) {}
  try { Object.defineProperty(navigator, 'userAgent', { get: () => ua, configurable: true }); } catch (e) {}
})()
"""


async def new_context_with_ua(browser, user_agent=None):
    """新建 camoufox 指纹 context；user_agent 覆盖时保证 wire 与 JS 两侧一致。

    优先 config_overrides（camoufox 0.5.4 generate_context_fingerprint 私有 API，
    try/except 回退到二次 init script 补丁——两路均已实测：JS/WIRE UA 均为覆盖值）。
    """
    if not user_agent:
        return await AsyncNewContext(browser, os="windows")
    try:
        from camoufox.fingerprints import generate_context_fingerprint

        fp = await asyncio.get_event_loop().run_in_executor(
            None,
            lambda: generate_context_fingerprint(
                os="windows", config_overrides={"navigator.userAgent": user_agent}
            ),
        )
        opts = dict(fp.get("context_options") or {})
        opts["extra_http_headers"] = chrome_hint_headers(user_agent)
        ctx = await browser.new_context(**opts)
        await ctx.add_init_script(fp.get("init_script") or "")
        return ctx
    except Exception:
        ctx = await AsyncNewContext(
            browser,
            os="windows",
            user_agent=user_agent,
            extra_http_headers=chrome_hint_headers(user_agent),
        )
        await ctx.add_init_script(UA_PATCH_INIT_JS % user_agent)
        return ctx


def chrome_hint_headers(user_agent):
    """Chrome UA 时补 sec-ch-ua 客户端提示头（Chromium 系默认携带；Firefox 不发送）。
    69shuba 等站点会用 Sec-CH-UA 交叉验证 UA——缺失即非 Chrome 判定。"""
    m = re.search(r"Chrome/(\d+)", user_agent or "")
    if not m:
        return {}
    v = m.group(1)
    return {
        "Sec-CH-UA": f'"Chromium";v="{v}", "Google Chrome";v="{v}", "Not.A/Brand";v="24"',
        "Sec-CH-UA-Mobile": "?0",
        "Sec-CH-UA-Platform": '"Windows"',
    }


# 页内 fetch POST（搜索等表单链路——同源 cookie/referer 自动携带，CF 视为真实请求）。
# 响应按 charset 解码（69shuba search.php 为 GBK——res.text() 会按 UTF-8 乱码）。
# 注意：Playwright evaluate 字符串里 arguments 不可用——payload 直接 json.dumps 嵌入。
POST_FETCH_JS = """
(async () => {
  const p = %s;
  try {
    const r = await fetch(p.action, {
      method: 'POST',
      headers: { 'Content-Type': p.contentType || 'application/x-www-form-urlencoded' },
      body: p.body || '',
      credentials: 'include'
    });
    let text = '';
    try {
      const buf = await r.arrayBuffer();
      text = new TextDecoder(p.charset || 'utf-8').decode(buf);
    } catch (e) { text = await r.text(); }
    return { status: r.status, url: r.url, html: text };
  } catch (e) {
    return { error: String((e && e.message) || e) };
  }
})()
"""

# 质询状态求值 JS（与 browser.rs CF_CHALLENGE_STATE_JS / TURNSTILE_DETECT_JS 同特征）：
# challenge = 仍在质询页；hasInput = Turnstile 隐藏 input 已渲染（managed challenge
# 勾选成功的标志）；inputValue = cf-turnstile-response 值（非空即通过）
CHALLENGE_STATE_JS = """
(function(){
  try {
    var features = document.querySelector('#challenge-form, [id^="cf-chl-"], [class*="cf-chl"], iframe[src*="challenges.cloudflare.com"], #cfts, [name="cf-turnstile-response"]');
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


# 页内表单导航式 POST（post.mode="navigate"）：隐藏表单 submit——同源 cookie/referer
# 自动携带，且页面级导航会渲染 Turnstile widget（fetch 模式拿不到 widget 交互能力）。
POST_NAVIGATE_JS = """
(() => {
  const fields = %s;
  const action = %s;
  const f = document.createElement('form');
  f.method = 'POST';
  f.action = action;
  f.style.display = 'none';
  for (const [n, v] of fields) {
    const inp = document.createElement('input');
    inp.type = 'hidden';
    inp.name = n;
    inp.value = v;
    f.appendChild(inp);
  }
  document.body.appendChild(f);
  f.submit();
  return true;
})()
"""


def form_fields_from_body(body):
    """URL 编码表单体 → [name, value] 对（百分号解码：先 UTF-8，失败回退 GBK——
    69shuba searchkey 为 GBK 字节；提交时浏览器按页面 charset 重新编码）"""
    from urllib.parse import unquote_to_bytes

    fields = []
    for kv in str(body or "").split("&"):
        if "=" not in kv:
            continue
        k, v = kv.split("=", 1)
        k = unquote_to_bytes(k).decode("utf-8", "replace")
        raw = unquote_to_bytes(v)
        try:
            v = raw.decode("utf-8")
        except UnicodeDecodeError:
            v = raw.decode("gbk", "replace")
        fields.append([k, v])
    return fields


async def wait_challenge_clear(page, deadline, diag, max_wait_ms, prefix=""):
    """质询等待循环：每 500ms 求值——input 值非空（Turnstile 通过）或 challenge 特征消失
    （经典 CF JS 质询自动解）→ 退出。返回 (ok, error_or_None)。
    注意：部分站点（如 69shuba）有 hidden input 但永远不写值（token 走自定义 callback）——
    点击条件按 inputValue 判，不按 hasInput（否则永不点击）。"""
    start = time.monotonic()
    while True:
        try:
            state = await page.evaluate(CHALLENGE_STATE_JS)
        except Exception:
            state = {"challenge": True, "hasInput": False, "inputValue": "", "title": "", "url": "", "bodyChildren": 0}
        diag["title"] = state.get("title", "")
        diag["hasInput"] = bool(state.get("hasInput"))
        diag["waitMs"] = int((time.monotonic() - start) * 1000)
        if state.get("inputValue"):
            return True, None  # Turnstile token 已生成 → 通过
        if not state.get("challenge"):
            return True, None  # 质询特征消失 → 通过
        if time.monotonic() >= deadline:
            return False, (
                f"质询求解超时（{max_wait_ms / 1000:.0f}s）——页面仍停留在质询页"
                f"（title={diag['title']!r} hasInput={diag['hasInput']} clicks={diag['clicks']}）"
            )
        # Turnstile widget iframe → 坐标点击勾选（比 CDP 坐标数学更稳）
        if not state.get("inputValue"):
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


async def post_navigate(page, post, max_wait_ms, diag):
    """post.mode="navigate"：页内表单提交（页面导航式 POST）→ 二次质询等待循环
    （渲染出的 Turnstile widget 自动点击）→ 最终页 HTML。返回 (postResult, err)"""
    action = str(post.get("action") or "")
    fields = form_fields_from_body(post.get("body") or "")
    try:
        await page.evaluate(POST_NAVIGATE_JS % (json.dumps(fields, ensure_ascii=True), json.dumps(action)))
    except Exception as e:
        return {"error": f"表单提交失败: {e}"[:300]}, str(e)[:200]
    # 等导航离开起始页且文档加载完成（避免在加载中的文档上误判"无质询"）
    start_url = page.url
    dl = time.monotonic() + 20
    while time.monotonic() < dl:
        try:
            if page.url != start_url:
                try:
                    rs = await page.evaluate("document.readyState")
                    if rs == "complete":
                        break
                except Exception:
                    break
        except Exception:
            break
        await asyncio.sleep(0.4)
    # 二次质询等待（Turnstile widget 等）
    deadline = time.monotonic() + max_wait_ms / 1000.0
    ok, err = await wait_challenge_clear(page, deadline, diag, max_wait_ms, prefix="post")
    await asyncio.sleep(1.0)
    try:
        html = await page.evaluate("document.documentElement.outerHTML")
    except Exception:
        html = await page.content()
    res = {"status": 200, "url": page.url, "html": html}
    if not ok:
        res["error"] = err
    return res, (err or "")


async def post_fetch(page, post, diag):
    """post.mode="fetch"（默认）：页内 fetch POST——响应按 charset 解码（GBK 支持）"""
    try:
        post_js = POST_FETCH_JS % json.dumps(
            {
                "action": str(post.get("action")),
                "body": str(post.get("body") or ""),
                "contentType": str(post.get("contentType") or "") or None,
                "charset": str(post.get("charset") or "") or None,
            },
            ensure_ascii=True,
        )
        result = await page.evaluate(post_js)
        if isinstance(result, dict):
            diag["postStatus"] = result.get("status")
            diag["postError"] = result.get("error")
        return result, (result.get("error") if isinstance(result, dict) else None)
    except Exception as e:
        return {"error": str(e)[:300]}, str(e)[:200]


async def solve_once(browser, url, cookies, max_wait_ms, user_agent=None, post=None):
    """单次求解：新建指纹 context → 导航 → 质询等待循环 →（可选页内 POST）→ 结果/诊断"""
    host = None
    try:
        from urllib.parse import urlparse

        host = urlparse(url).hostname or ""
    except Exception:
        host = ""
    diag = {"title": "", "hasInput": False, "url": url, "waitMs": 0, "clicks": 0}
    if user_agent:
        diag["userAgent"] = user_agent
    context = await new_context_with_ua(browser, user_agent)
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
        # 质询等待循环（详见 wait_challenge_clear）
        deadline = time.monotonic() + max_wait_ms / 1000.0
        ok, err = await wait_challenge_clear(page, deadline, diag, max_wait_ms)
        if not ok:
            return {"error": err}, diag
        # 稳定等待（质询跳转后的重绘）
        await asyncio.sleep(1.0)
        # 可选页内 POST（搜索等表单链路——同源 cookie/referer 自动携带）
        post_result = None
        if post and isinstance(post, dict) and post.get("action"):
            if str(post.get("mode") or "") == "navigate":
                post_result, _ = await post_navigate(page, post, max_wait_ms, diag)
            else:
                post_result, _ = await post_fetch(page, post, diag)
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
        result = {
            "html": html,
            "cookies": site_cookies,
            "userAgent": ua,
            "turnstileToken": token,
            "diagnostics": diag,
        }
        if post_result is not None:
            result["postResult"] = post_result
        return result, diag
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
    post = payload.get("post")
    if post is not None and not isinstance(post, dict):
        return 400, {"error": "post 必须是对象 {action, body, contentType?, charset?}"}
    try:
        browser = await get_browser()
    except Exception as e:
        return 502, {"error": f"camoufox 浏览器启动失败: {e}（请先执行 camoufox fetch 下载浏览器）"}
    result, diag = await solve_once(browser, url, cookies, max_wait_ms, user_agent, post)
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
