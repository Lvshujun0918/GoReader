import asyncio, json, sys, time
sys.path.insert(0, "scripts")
from camoufox.async_api import AsyncCamoufox, AsyncNewContext
from camoufox.fingerprints import generate_context_fingerprint

CHROME_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36"
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
    browser = await AsyncCamoufox(headless=True, humanize=True).__aenter__()
    fp = await asyncio.get_event_loop().run_in_executor(
        None, lambda: generate_context_fingerprint(os="windows", config_overrides={"navigator.userAgent": CHROME_UA})
    )
    opts = dict(fp["context_options"])
    opts["extra_http_headers"] = {"Sec-CH-UA": '"Chromium";v="125", "Google Chrome";v="125", "Not.A/Brand";v="24"', "Sec-CH-UA-Mobile": "?0", "Sec-CH-UA-Platform": '"Windows"'}
    ctx = await browser.new_context(**opts)
    await ctx.add_init_script(fp["init_script"])
    page = await ctx.new_page()
    # 主页 WebGL 状态
    await page.goto("https://www.69shuba.com/", wait_until="domcontentloaded", timeout=60000)
    gl = await page.evaluate("""(() => { try { const c = document.createElement('canvas'); const g = c.getContext('webgl') || c.getContext('experimental-webgl'); if (!g) return 'no-webgl'; const d = g.getParameter(g.RENDERER); return String(d); } catch (e) { return 'err:' + e; } })()""")
    print("主页 WebGL renderer:", gl, flush=True)
    await page.evaluate(FORM_SUBMIT_JS % (json.dumps(FIELDS, ensure_ascii=True), json.dumps("https://www.69shuba.com/modules/article/search.php")))
    dl = time.monotonic() + 20
    while time.monotonic() < dl:
        await asyncio.sleep(0.4)
        if "search.php" in page.url: break
    await asyncio.sleep(8)
    frame = next((f for f in page.frames if "challenges.cloudflare.com" in (f.url or "")), None)
    if frame:
        body = await frame.evaluate("""(() => { const b = document.body; return {text: b.innerText.slice(0,300), html: b.innerHTML.slice(0, 800)}; })()""")
        print("iframe body:", json.dumps(body, ensure_ascii=False), flush=True)
        try:
            frame_gl = await frame.evaluate("""(() => { try { const c = document.createElement('canvas'); const g = c.getContext('webgl') || c.getContext('experimental-webgl'); return g ? String(g.getParameter(g.RENDERER)) : 'no-webgl'; } catch (e) { return 'err:' + e; } })()""")
            print("iframe WebGL renderer:", frame_gl, flush=True)
        except Exception as e:
            print("iframe gl eval fail:", e, flush=True)
        # 等更久再观察一次
        await asyncio.sleep(10)
        body2 = await frame.evaluate("document.body ? document.body.innerText.slice(0,300) : ''")
        print("iframe body(10s后):", json.dumps(body2, ensure_ascii=False), flush=True)
    else:
        print("no turnstile frame", flush=True)
    await browser.close()

asyncio.run(main())
