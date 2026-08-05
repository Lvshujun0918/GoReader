"""对照组：纯 Chrome（无自动化 flag，navigator.webdriver=false）attach CDP 驱动"""
import asyncio, json, subprocess, sys, time
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
    import pathlib, shutil
    prof = pathlib.Path("scripts/_chrome_prof")
    shutil.rmtree(prof, ignore_errors=True)
    chrome = r"C:\Program Files\Google\Chrome\Application\chrome.exe"
    proc = subprocess.Popen([chrome, f"--user-data-dir={prof.resolve()}", "--remote-debugging-port=19222",
                             "--no-first-run", "--no-default-browser-check", "about:blank"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(4)
    async with async_playwright() as pw:
        browser = await pw.chromium.connect_over_cdp("http://127.0.0.1:19222")
        ctx = browser.contexts[0]
        page = await ctx.new_page()
        print("webdriver flag:", await page.evaluate("navigator.webdriver"), flush=True)
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
                info = await frame.evaluate("document.body ? document.body.innerHTML.slice(0, 800) : 'no-body'")
                print("iframe html:", json.dumps(info, ensure_ascii=False)[:1000], flush=True)
            except Exception as e:
                print("iframe eval fail:", e, flush=True)
            try:
                await frame.click("body", timeout=5000)
                print("clicked", flush=True)
            except Exception as e:
                print("click fail:", e, flush=True)
        else:
            print("no iframe", flush=True)
        await asyncio.sleep(12)
        print("url now:", page.url, flush=True)
        body = await page.evaluate("document.body ? document.body.innerText.slice(0,150) : ''")
        print("body now:", json.dumps(body, ensure_ascii=False), flush=True)
        await browser.close()
    proc.terminate()

asyncio.run(main())
