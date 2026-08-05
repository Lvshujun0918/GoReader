#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""UA 覆盖策略对比（临时脚本）：
A) AsyncNewContext + user_agent kwarg
B) A + 追加 init script 再补丁 navigator.userAgent
C) generate_context_fingerprint(config_overrides={'navigator.userAgent': ...})
每种策略验证：JS navigator.userAgent + wire UA（本地 echo 服务器）+ sec-ch-ua。
可选第 4 步：导航 69shuba 首页（CF + UA 门禁）。
用法: python scripts/_ua_probe.py [--69]"""
import asyncio
import json
import subprocess
import sys
import time

from camoufox.async_api import AsyncCamoufox, AsyncNewContext
from camoufox.fingerprints import generate_context_fingerprint

CHROME_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36"
ECHO_PORT = 18991

PATCH_JS = """
(() => {
  const ua = %r;
  try { Object.defineProperty(Navigator.prototype, 'userAgent', { get: () => ua, configurable: true }); } catch (e) {}
  try { Object.defineProperty(navigator, 'userAgent', { get: () => ua, configurable: true }); } catch (e) {}
})()
""" % CHROME_UA

CH_HEADERS = {
    "Sec-CH-UA": '"Chromium";v="125", "Google Chrome";v="125", "Not.A/Brand";v="24"',
    "Sec-CH-UA-Mobile": "?0",
    "Sec-CH-UA-Platform": '"Windows"',
}


async def probe(browser, label, make_ctx):
    print(f"\n===== {label} =====", flush=True)
    try:
        ctx = await make_ctx(browser)
    except Exception as e:
        print(f"context 创建失败: {e}", flush=True)
        return None
    try:
        page = await ctx.new_page()
        js_ua = await page.evaluate("navigator.userAgent")
        js_plat = await page.evaluate("navigator.platform")
        js_ch = await page.evaluate("navigator.userAgentData ? JSON.stringify(navigator.userAgentData.brands) : 'no-uaData'")
        print(f"JS UA: {js_ua}", flush=True)
        print(f"JS platform: {js_plat} | userAgentData: {js_ch}", flush=True)
        try:
            await page.goto(f"http://127.0.0.1:{ECHO_PORT}/", wait_until="domcontentloaded", timeout=20000)
            wire = json.loads(await page.evaluate("document.body.innerText"))
            print(f"WIRE UA: {wire.get('User-Agent')}", flush=True)
            print(f"WIRE sec-ch-ua: {wire.get('Sec-CH-UA')} | platform: {wire.get('Sec-CH-UA-Platform')}", flush=True)
        except Exception as e:
            print(f"echo 探测失败（不影响后续）: {e}", flush=True)
        return ctx, page, js_ua
    except Exception as e:
        print(f"probe 失败: {e}", flush=True)
        return None
    finally:
        pass


async def main():
    test69 = "--69" in sys.argv
    srv = subprocess.Popen([sys.executable, "-c", """
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
class Echo(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps(dict(self.headers)).encode()
        self.send_response(200); self.send_header("Content-Type","application/json"); self.send_header("Content-Length",str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self,*a): pass
HTTPServer(("127.0.0.1", %d), Echo).serve_forever()
""" % ECHO_PORT])
    time.sleep(2)
    browser = await AsyncCamoufox(headless=True, humanize=True).__aenter__()

    def ctx_a(b):
        return AsyncNewContext(b, os="windows", user_agent=CHROME_UA, extra_http_headers=CH_HEADERS)

    async def ctx_b(b):
        ctx = await AsyncNewContext(b, os="windows", user_agent=CHROME_UA, extra_http_headers=CH_HEADERS)
        await ctx.add_init_script(PATCH_JS)
        return ctx

    async def ctx_c(b):
        fp = await asyncio.get_event_loop().run_in_executor(
            None, lambda: generate_context_fingerprint(os="windows", config_overrides={"navigator.userAgent": CHROME_UA})
        )
        opts = dict(fp["context_options"])
        opts["extra_http_headers"] = CH_HEADERS
        ctx = await b.new_context(**opts)
        await ctx.add_init_script(fp["init_script"])
        return ctx

    best = None
    for label, make in [("A: user_agent kwarg", ctx_a), ("B: +patch init script", ctx_b), ("C: config_overrides", ctx_c)]:
        r = await probe(browser, label, make)
        if r:
            if best is None:
                best = (label, r)
            else:
                # 只保留 best 的 context 供后续 69shuba 使用
                try:
                    await r[0].close()
                except Exception:
                    pass

    if test69 and best:
        label, (ctx, page, js_ua) = best
        print(f"\n===== 69shuba（用 {label}，JS UA={js_ua[:60]}...）=====", flush=True)
        try:
            resp = await page.goto("https://www.69shuba.com/", wait_until="domcontentloaded", timeout=60000)
            print("status:", resp.status if resp else "?", flush=True)
            deadline = time.monotonic() + 60
            title = ""
            while time.monotonic() < deadline:
                await asyncio.sleep(2)
                title = await page.evaluate("document.title")
                low = title.lower()
                if "just a moment" not in low and "verifying" not in low:
                    break
            print("title:", title, flush=True)
            txt = await page.evaluate("document.body ? document.body.innerText.slice(0, 400) : ''")
            print("body head:", repr(txt), flush=True)
            html = await page.evaluate("document.documentElement.outerHTML")
            has_gate = "请使用" in html or "Google Chrome" in html
            print("UA 门禁命中:", has_gate, flush=True)
            if has_gate:
                import re
                m = re.search(r'请使用[^<"]{0,80}', html)
                print("门禁文案:", m.group(0) if m else "?", flush=True)
        except Exception as e:
            print("69shuba 导航失败:", e, flush=True)
        await ctx.close()
    await browser.close()
    srv.terminate()


if __name__ == "__main__":
    asyncio.run(main())
