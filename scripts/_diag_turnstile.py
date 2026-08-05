#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Turnstile widget 状态诊断：挑战页渲染后 iframe/frames/截图 + 点击前后对比"""
import asyncio
import json
import sys
import time

from camoufox.async_api import AsyncCamoufox, AsyncNewContext
from camoufox.fingerprints import generate_context_fingerprint

CHROME_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36"
FIELDS = [("searchkey", "宿命之环"), ("searchtype", "all"), ("page", "1")]
FORM_SUBMIT_JS = """
(() => {
  const fields = %s;
  const f = document.createElement('form');
  f.method = 'POST';
  f.action = %s;
  f.style.display = 'none';
  for (const [n, v] of fields) {
    const inp = document.createElement('input');
    inp.type = 'hidden'; inp.name = n; inp.value = v;
    f.appendChild(inp);
  }
  document.body.appendChild(f);
  f.submit();
  return true;
})()
"""


async def dump(page, tag):
    print(f"--- {tag} ---", flush=True)
    print("url:", page.url, flush=True)
    print("frames:", [(f.url[:80]) for f in page.frames], flush=True)
    try:
        info = await page.evaluate("""
        (() => {
          const c = document.querySelector('#cfts');
          const ifr = c ? c.querySelectorAll('iframe').length : -1;
          const inp = document.querySelector('[name="cf-turnstile-response"]');
          const hasTs = typeof window.turnstile !== 'undefined';
          return {cftsHtml: c ? c.innerHTML.slice(0, 300) : null, iframes: ifr,
                  inputValue: inp ? inp.value : null, hasTurnstile: hasTs};
        })()
        """)
        print("widget:", json.dumps(info, ensure_ascii=False), flush=True)
    except Exception as e:
        print("widget eval fail:", e, flush=True)
    try:
        await page.screenshot(path=f"scripts/_diag_{tag}.png")
    except Exception as e:
        print("screenshot fail:", e, flush=True)


async def main():
    browser = await AsyncCamoufox(headless=True, humanize=True).__aenter__()
    fp = await asyncio.get_event_loop().run_in_executor(
        None, lambda: generate_context_fingerprint(os="windows", config_overrides={"navigator.userAgent": CHROME_UA})
    )
    opts = dict(fp["context_options"])
    opts["extra_http_headers"] = {
        "Sec-CH-UA": '"Chromium";v="125", "Google Chrome";v="125", "Not.A/Brand";v="24"',
        "Sec-CH-UA-Mobile": "?0", "Sec-CH-UA-Platform": '"Windows"',
    }
    ctx = await browser.new_context(**opts)
    await ctx.add_init_script(fp["init_script"])
    page = await ctx.new_page()
    console_msgs = []
    page.on("console", lambda m: console_msgs.append(f"{m.type}: {m.text[:150]}"))
    page.on("pageerror", lambda e: console_msgs.append(f"PAGEERROR: {str(e)[:200]}"))
    page.on("requestfailed", lambda r: console_msgs.append(f"REQFAIL: {r.url[:100]} {r.failure}"))
    await page.goto("https://www.69shuba.com/", wait_until="domcontentloaded", timeout=60000)
    print("homepage ok", flush=True)
    await page.evaluate(FORM_SUBMIT_JS % (json.dumps(FIELDS, ensure_ascii=True), json.dumps("https://www.69shuba.com/modules/article/search.php")))
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        await asyncio.sleep(0.4)
        if "search.php" in page.url:
            break
    await asyncio.sleep(6)  # 等 widget 渲染
    await dump(page, "before_click")
    # 尝试点击 checkbox（iframe 内）
    try:
        frame = next((f for f in page.frames if "challenges.cloudflare.com" in (f.url or "")), None)
        if frame:
            print("iframe url:", frame.url[:120], flush=True)
            box = await frame.locator("body").bounding_box()
            print("iframe body box:", box, flush=True)
            await frame.click("body", timeout=5000)
            print("clicked iframe body", flush=True)
        else:
            loc = page.locator('iframe[src*="challenges.cloudflare.com"]')
            print("iframe locator count:", await loc.count(), flush=True)
            if await loc.count() > 0:
                await loc.first.click(timeout=5000)
                print("clicked locator", flush=True)
    except Exception as e:
        print("click fail:", e, flush=True)
    await asyncio.sleep(6)
    await dump(page, "after_click")
    print("console:", flush=True)
    for m in console_msgs[-25:]:
        print("  ", m, flush=True)
    await browser.close()


if __name__ == "__main__":
    asyncio.run(main())
