"""真实 Chrome headed：69shuba 挑战页 iframe 完整 HTML + 点击 + 网络监听 verify.php"""
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
        browser = await pw.chromium.launch(headless=False, channel="chrome")
        ctx = await browser.new_context(locale="zh-CN", timezone_id="Asia/Shanghai")
        page = await ctx.new_page()
        hits = []
        page.on("request", lambda r: hits.append(("REQ", r.method, r.url[:110])) if ("verify.php" in r.url or "turnstile" in r.url) else None)
        page.on("response", lambda r: hits.append(("RESP", r.status, r.url[:110])) if ("verify.php" in r.url or "turnstile" in r.url) else None)
        page.on("console", lambda m: hits.append(("CONS", m.type, m.text[:130])))
        await page.goto("https://www.69shuba.com/", wait_until="domcontentloaded", timeout=60000)
        print("home:", await page.title(), flush=True)
        await page.evaluate(FORM_SUBMIT_JS % (json.dumps(FIELDS, ensure_ascii=True), json.dumps("https://www.69shuba.com/modules/article/search.php")))
        dl = time.monotonic() + 20
        while time.monotonic() < dl:
            await asyncio.sleep(0.4)
            if "search.php" in page.url: break
        await asyncio.sleep(10)
        frames = [(f.url[:95]) for f in page.frames]
        print("frames:", frames, flush=True)
        frame = next((f for f in page.frames if "challenges.cloudflare.com" in f.url), None)
        if frame:
            try:
                info = await frame.evaluate("""(() => { const b = document.body; return {html: b ? b.innerHTML.slice(0, 1500) : 'no-body', cls: b ? b.className : ''}; })()""")
                print("iframe html:", json.dumps(info, ensure_ascii=False)[:1800], flush=True)
            except Exception as e:
                print("iframe eval fail:", e, flush=True)
            await page.screenshot(path="scripts/_diag_real3_before.png")
            try:
                await frame.click("body", timeout=5000)
                print("clicked iframe", flush=True)
            except Exception as e:
                print("click fail:", e, flush=True)
        else:
            print("no iframe", flush=True)
        await asyncio.sleep(12)
        print("url now:", page.url, flush=True)
        print("title now:", await page.title(), flush=True)
        body = await page.evaluate("document.body ? document.body.innerText.slice(0,200) : ''")
        print("body now:", json.dumps(body, ensure_ascii=False), flush=True)
        print("== net/console ==", flush=True)
        for h in hits[-30:]:
            print("  ", h, flush=True)
        await browser.close()

asyncio.run(main())
