"""抓取 turnstile iframe 的 HTTP 响应体/状态——判断挑战平台是否拒发"""
import asyncio, json, subprocess, sys, time, pathlib, shutil
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
    prof = pathlib.Path("scripts/_chrome_prof2")
    shutil.rmtree(prof, ignore_errors=True)
    chrome = r"C:\Program Files\Google\Chrome\Application\chrome.exe"
    proc = subprocess.Popen([chrome, f"--user-data-dir={prof.resolve()}", "--remote-debugging-port=19223",
                             "--no-first-run", "--no-default-browser-check", "about:blank"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(4)
    async with async_playwright() as pw:
        browser = await pw.chromium.connect_over_cdp("http://127.0.0.1:19223")
        ctx = browser.contexts[0]
        page = await ctx.new_page()
        resp_bodies = {}
        async def on_resp(r):
            if "challenges.cloudflare.com" in r.url and ("turnstile" in r.url or "challenge" in r.url):
                try:
                    body = await r.body()
                    resp_bodies[r.url[:130]] = {"status": r.status, "len": len(body), "head": body[:120].decode("utf-8", "replace")}
                except Exception as e:
                    resp_bodies[r.url[:130]] = {"err": str(e)[:100]}
        page.on("response", lambda r: asyncio.ensure_future(on_resp(r)))
        await page.goto("https://www.69shuba.com/", wait_until="domcontentloaded", timeout=60000)
        print("home:", await page.title(), flush=True)
        await page.evaluate(FORM_SUBMIT_JS % (json.dumps(FIELDS, ensure_ascii=True), json.dumps("https://www.69shuba.com/modules/article/search.php")))
        dl = time.monotonic() + 20
        while time.monotonic() < dl:
            await asyncio.sleep(0.4)
            if "search.php" in page.url: break
        await asyncio.sleep(10)
        print("== iframe 响应 ==", flush=True)
        for u, info in resp_bodies.items():
            print(json.dumps({"url": u, **info}, ensure_ascii=False), flush=True)
        await browser.close()
    proc.terminate()

asyncio.run(main())
