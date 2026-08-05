"""本地页 + CF 公开 demo sitekey——Turnstile 在本机能否渲染/出 token"""
import asyncio, json, sys, time
from playwright.async_api import async_playwright

DEMO_HTML = """<!DOCTYPE html><html><head><meta charset="utf-8"><title>ts-demo</title>
<script src="https://challenges.cloudflare.com/turnstile/v0/api.js" async defer></script></head>
<body><div id="w"></div><script>
window.onloadTurnstileCallback = function () {
  turnstile.render("#w", {sitekey: "0x4AAAAAAABGpllqO9XmdphoA",
    callback: function (t) { document.title = "TOKEN:" + t.slice(0, 20); }});
};
</script></body></html>"""

async def main():
    import pathlib
    p = pathlib.Path("scripts/_ts_demo_page.html")
    p.write_text(DEMO_HTML, encoding="utf-8")
    async with async_playwright() as pw:
        browser = await pw.chromium.launch(headless=False, channel="chrome")
        page = await browser.new_page()
        msgs = []
        page.on("console", lambda m: msgs.append(f"{m.type}: {m.text[:130]}"))
        await page.goto("file:///" + str(p.resolve()).replace("\\", "/"), wait_until="load", timeout=30000)
        await asyncio.sleep(8)
        w = await page.evaluate("""(() => { const el = document.querySelector('#w'); const inp = document.querySelector('[name="cf-turnstile-response"]'); return {html: el ? el.innerHTML.slice(0, 400) : null, inputVal: inp ? inp.value : null}; })()""")
        print("widget:", json.dumps(w, ensure_ascii=False), flush=True)
        print("frames:", [f.url[:90] for f in page.frames], flush=True)
        try:
            loc = page.locator('iframe[src*="challenges.cloudflare.com"]')
            if await loc.count() > 0:
                await loc.first.click(timeout=5000)
                print("clicked", flush=True)
        except Exception as e:
            print("click fail:", e, flush=True)
        await asyncio.sleep(12)
        w2 = await page.evaluate("""(() => { const inp = document.querySelector('[name="cf-turnstile-response"]'); return {inputVal: inp ? inp.value : null, title: document.title}; })()""")
        print("after click:", json.dumps(w2, ensure_ascii=False), flush=True)
        print("console:", flush=True)
        for m in msgs[-12:]:
            print("  ", m, flush=True)
        await browser.close()

asyncio.run(main())
