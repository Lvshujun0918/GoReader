import asyncio, json, sys, time
from camoufox.async_api import AsyncCamoufox, AsyncNewContext

FIELDS = [("searchkey", "宿命之环"), ("searchtype", "all"), ("page", "1")]
FORM_SUBMIT_JS = """
(() => {
  const fields = %s;
  const f = document.createElement('form');
  f.method = 'POST'; f.action = %s; f.style.display = 'none';
  for (const [n, v] of fields) {
    const inp = document.createElement('input');
    inp.type = 'hidden'; inp.name = n; inp.value = v;
    f.appendChild(inp);
  }
  document.body.appendChild(f); f.submit(); return true;
})()
"""

async def variant(label, headless, ua_override):
    print(f"\n===== {label} (headless={headless}, ua_override={ua_override}) =====", flush=True)
    browser = await AsyncCamoufox(headless=headless, humanize=True).__aenter__()
    kw = {}
    if ua_override:
        kw["user_agent"] = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36"
        kw["extra_http_headers"] = {"Sec-CH-UA": '"Chromium";v="125", "Google Chrome";v="125", "Not.A/Brand";v="24"', "Sec-CH-UA-Mobile": "?0", "Sec-CH-UA-Platform": '"Windows"'}
    ctx = await AsyncNewContext(browser, os="windows", **kw)
    page = await ctx.new_page()
    errs = []
    page.on("pageerror", lambda e: errs.append(str(e)[:160]))
    await page.goto("https://www.69shuba.com/", wait_until="domcontentloaded", timeout=60000)
    await page.evaluate(FORM_SUBMIT_JS % (json.dumps(FIELDS, ensure_ascii=True), json.dumps("https://www.69shuba.com/modules/article/search.php")))
    dl = time.monotonic() + 20
    while time.monotonic() < dl:
        await asyncio.sleep(0.4)
        if "search.php" in page.url: break
    await asyncio.sleep(8)
    frames = [f.url[:90] for f in page.frames]
    print("frames:", frames, flush=True)
    widget = await page.evaluate("""(() => { const c = document.querySelector('#cfts'); const inp = document.querySelector('[name="cf-turnstile-response"]'); return {cfts: c ? c.innerHTML.slice(0,200) : null, inputVal: inp ? inp.value : null}; })()""")
    print("widget:", json.dumps(widget, ensure_ascii=False), flush=True)
    print("pageerrors:", errs[-5:], flush=True)
    if any("challenges.cloudflare.com" in f for f in frames):
        frame = next(f for f in page.frames if "challenges.cloudflare.com" in f.url)
        await asyncio.sleep(4)
        try:
            t = await frame.evaluate("document.body ? document.body.innerText.slice(0,200) : ''")
            print("iframe text:", json.dumps(t, ensure_ascii=False), flush=True)
        except Exception as e:
            print("iframe eval fail:", e, flush=True)
    # 尝试点击
    try:
        loc = page.locator('iframe[src*="challenges.cloudflare.com"]')
        if await loc.count() > 0:
            await loc.first.click(timeout=4000)
            print("clicked", flush=True)
    except Exception as e:
        print("click fail:", e, flush=True)
    await asyncio.sleep(8)
    inp_val = await page.evaluate("(document.querySelector('[name=\"cf-turnstile-response\"]')||{}).value || ''")
    print("token 值:", repr(inp_val[:40]), flush=True)
    await browser.close()

async def main():
    await variant("V3 Firefox UA headless", True, False)
    await variant("V4 Chrome UA headed", False, True)

asyncio.run(main())
