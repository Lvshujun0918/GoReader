"""对照组：真实 Chrome 走同一流程——Turnstile 是否能跑（排除 IP/站点因素）"""
import asyncio, json, sys, time
from playwright.async_api import async_playwright

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

async def main():
    async with async_playwright() as pw:
        browser = await pw.chromium.launch(headless=False, channel="chrome", args=["--disable-blink-features=AutomationControlled"])
        ctx = await browser.new_context(locale="zh-CN", timezone_id="Asia/Shanghai")
        page = await ctx.new_page()
        msgs = []
        page.on("console", lambda m: msgs.append(f"{m.type}: {m.text[:120]}"))
        await page.goto("https://www.69shuba.com/", wait_until="domcontentloaded", timeout=60000)
        print("homepage:", await page.title(), flush=True)
        await page.evaluate(FORM_SUBMIT_JS % (json.dumps(FIELDS, ensure_ascii=True), json.dumps("https://www.69shuba.com/modules/article/search.php")))
        dl = time.monotonic() + 20
        while time.monotonic() < dl:
            await asyncio.sleep(0.4)
            if "search.php" in page.url: break
        await asyncio.sleep(8)
        frames = [(f.url[:90]) for f in page.frames]
        print("frames:", frames, flush=True)
        w = await page.evaluate("""(() => { const c = document.querySelector('#cfts'); const inp = document.querySelector('[name="cf-turnstile-response"]'); return {cfts: c ? c.innerHTML.slice(0,300) : null, inputVal: inp ? inp.value : null}; })()""")
        print("widget:", json.dumps(w, ensure_ascii=False), flush=True)
        if any("challenges.cloudflare.com" in f for f in frames):
            frame = next(f for f in page.frames if "challenges.cloudflare.com" in f.url)
            try:
                t = await frame.evaluate("document.body ? document.body.innerText.slice(0,300) : ''")
                print("iframe text:", json.dumps(t, ensure_ascii=False), flush=True)
            except Exception as e:
                print("iframe eval fail:", e, flush=True)
            await page.screenshot(path="scripts/_diag_realchrome.png")
        # 点击
        try:
            loc = page.locator('iframe[src*="challenges.cloudflare.com"]')
            if await loc.count() > 0:
                await loc.first.click(timeout=4000)
                print("clicked", flush=True)
        except Exception as e:
            print("click fail:", e, flush=True)
        await asyncio.sleep(10)
        inp_val = await page.evaluate("(document.querySelector('[name=\"cf-turnstile-response\"]')||{}).value || ''")
        print("token 值:", repr(inp_val[:60]), flush=True)
        print("console tail:", flush=True)
        for m in msgs[-12:]:
            print("  ", m, flush=True)
        await browser.close()

asyncio.run(main())
