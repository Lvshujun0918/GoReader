#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""69shuba search.php 门禁机制探测（临时）：
1) 页面导航式 POST（表单提交）是否仍触发 Turnstile 挑战页
2) navigator.userAgentData polyfill 是否能避免挑战
3) 挑战页出现时：自动点击 Turnstile 勾选 → verify.php → reload → 结果"""
import asyncio
import json
import re
import sys
import time
import urllib.parse

from camoufox.async_api import AsyncCamoufox, AsyncNewContext
from camoufox.fingerprints import generate_context_fingerprint

CHROME_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36"
KEY = "宿命之环"
FIELDS = [("searchkey", KEY), ("searchtype", "all"), ("page", "1")]

CH_HEADERS = {
    "Sec-CH-UA": '"Chromium";v="125", "Google Chrome";v="125", "Not.A/Brand";v="24"',
    "Sec-CH-UA-Mobile": "?0",
    "Sec-CH-UA-Platform": '"Windows"',
}

UADATA_JS = """
(() => {
  try {
    if (!navigator.userAgentData) {
      const ua = %r;
      const m = ua.match(/Chrome\\/(\\d+)/);
      const v = m ? m[1] : '125';
      const brands = [
        {brand: 'Chromium', version: v},
        {brand: 'Google Chrome', version: v},
        {brand: 'Not.A/Brand', version: '24'}
      ];
      Object.defineProperty(navigator, 'userAgentData', {
        get: () => ({
          brands, mobile: false, platform: 'Windows',
          getHighEntropyValues: async (hints) => ({architecture: 'x86', bitness: '64', platformVersion: '10.0.0', uaFullVersion: '125.0.6422.165', fullVersionList: brands, model: '', mobile: false, platform: 'Windows', wow64: false}),
          toJSON: () => ({brands, mobile: false, platform: 'Windows'})
        }),
        configurable: true
      });
    }
  } catch (e) {}
})()
""" % CHROME_UA

FORM_SUBMIT_JS = """
(() => {
  const fields = %s;
  const f = document.createElement('form');
  f.method = 'POST';
  f.action = %s;
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

CHALLENGE_STATE_JS = """
(function(){
  try {
    var features = document.querySelector('#challenge-form, [id^="cf-chl-"], [class*="cf-chl"], iframe[src*="challenges.cloudflare.com"], #cfts');
    var t = (document.title || '').toLowerCase();
    var input = document.querySelector('[name="cf-turnstile-response"]');
    return {
      challenge: !!(features || t.indexOf('just a moment') >= 0 || t.indexOf('turnstile') >= 0 || t.indexOf('verifying') >= 0),
      hasInput: !!input,
      inputValue: input && input.value ? input.value : '',
      title: document.title || '',
      url: location.href
    };
  } catch (e) { return { challenge: true, hasInput: false, inputValue: '', title: '', url: '' }; }
})()
"""


async def make_ctx(browser, uadata):
    fp = await asyncio.get_event_loop().run_in_executor(
        None, lambda: generate_context_fingerprint(os="windows", config_overrides={"navigator.userAgent": CHROME_UA})
    )
    opts = dict(fp["context_options"])
    opts["extra_http_headers"] = CH_HEADERS
    ctx = await browser.new_context(**opts)
    await ctx.add_init_script(fp["init_script"])
    if uadata:
        await ctx.add_init_script(UADATA_JS)
    return ctx


async def click_turnstile(page):
    """点击 Turnstile widget（iframe body 坐标点击）"""
    clicks = 0
    for _ in range(10):
        try:
            frame = next((f for f in page.frames if "challenges.cloudflare.com" in (f.url or "")), None)
            if frame is not None:
                await frame.click("body", timeout=3000)
                clicks += 1
            else:
                loc = page.locator('iframe[src*="challenges.cloudflare.com"]')
                if await loc.count() > 0:
                    await loc.first.click(timeout=3000)
                    clicks += 1
        except Exception:
            pass
        await asyncio.sleep(1)
        state = await page.evaluate(CHALLENGE_STATE_JS)
        if not state["challenge"] or state["inputValue"]:
            break
        # 若 widget 已消失且页面还在挑战页（token 回调 ajax 中）——继续等
        has_widget = await page.evaluate("!!document.querySelector('#cfts iframe') || !!document.querySelector('iframe[src*=challenges]')")
        if not has_widget and not state["challenge"]:
            break
    return clicks


async def run_variant(browser, label, uadata, max_wait=70):
    print(f"\n===== {label} =====", flush=True)
    ctx = await make_ctx(browser, uadata)
    page = await ctx.new_page()
    t0 = time.monotonic()
    await page.goto("https://www.69shuba.com/", wait_until="domcontentloaded", timeout=60000)
    print(f"首页: {await page.title():.30} url={page.url[:50]} ({time.monotonic()-t0:.1f}s)", flush=True)
    # 表单导航式 POST（页面为 GBK——表单提交自动按 GBK 编码 searchkey）
    try:
        await page.evaluate(FORM_SUBMIT_JS % (json.dumps(FIELDS, ensure_ascii=True), json.dumps("https://www.69shuba.com/modules/article/search.php")))
    except Exception as e:
        print("表单提交失败:", e, flush=True)
        await ctx.close()
        return
    # 等待导航（挑战页或结果页）
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        await asyncio.sleep(0.4)
        if page.url != "https://www.69shuba.com/":
            break
    await asyncio.sleep(2)
    state = await page.evaluate(CHALLENGE_STATE_JS)
    print(f"提交后: title={state['title']!r} challenge={state['challenge']} url={page.url[:70]}", flush=True)
    if state["challenge"]:
        print("→ 命中挑战页，尝试自动点击 Turnstile...", flush=True)
        clicks = await click_turnstile(page)
        print(f"点击次数: {clicks}", flush=True)
        # 等待 verify.php → reload → 结果页
        deadline = time.monotonic() + 40
        while time.monotonic() < deadline:
            await asyncio.sleep(1.5)
            state = await page.evaluate(CHALLENGE_STATE_JS)
            if not state["challenge"]:
                break
        print(f"最终: title={state['title']!r} challenge={state['challenge']} url={page.url[:70]}", flush=True)
    await asyncio.sleep(2)
    html = await page.evaluate("document.documentElement.outerHTML")
    has_gate = "请使用新版" in html
    has_results = "bookname" in html or "article_list" in html or 'class="newbox"' in html or "最新章节" in html
    print(f"门禁文案: {has_gate} | 结果特征: {has_results} | html {len(html)}B", flush=True)
    if has_results:
        names = [(m.group(1), m.group(2).strip()) for m in re.finditer(r'<h3[^>]*>\s*<a[^>]*href="([^"]+)"[^>]*>([^<]+)</a>', html)]
        if not names:
            names = [(m.group(1), m.group(2).strip()) for m in re.finditer(r'<a[^>]*href="(/book/\d+\.htm)"[^>]*>([^<]{2,40})</a>', html)]
        print("结果:", [(n, u) for u, n in names[:6]], flush=True)
    open(f"scripts/_probe_{label.replace(' ', '_').replace(':', '').replace('+', 'p')}.html", "w", encoding="utf-8").write(html)
    await ctx.close()


async def main():
    browser = await AsyncCamoufox(headless=True, humanize=True).__aenter__()
    await run_variant(browser, "V1 nav-POST 无polyfill", False)
    await run_variant(browser, "V2 nav-POST +uaData polyfill", True)
    await browser.close()


if __name__ == "__main__":
    asyncio.run(main())
