"""本地 http 服务 + demo sitekey + error callback 捕获"""
import asyncio, json, threading, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from playwright.async_api import async_playwright

DEMO_HTML = """<!DOCTYPE html><html><head><meta charset="utf-8"><title>ts-demo2</title>
<script src="https://challenges.cloudflare.com/turnstile/v0/api.js" async defer></script></head>
<body><div id="w"></div><div id="log"></div><script>
function log(m){var d=document.createElement('div');d.textContent=m;document.getElementById('log').appendChild(d);}
window.onloadTurnstileCallback = function () {
  log('callback fired; turnstile=' + typeof window.turnstile);
  try {
    turnstile.render("#w", {sitekey: "0x4AAAAAAABGpllqO9XmdphoA",
      callback: function (t) { log('TOKEN:' + t.slice(0, 30)); },
      'error-callback': function (e) { log('ERR:' + e); },
      'expired-callback': function () { log('EXPIRED'); }});
  } catch (e) { log('render threw: ' + e); }
};
setTimeout(function(){ log('t+8s turnstile=' + typeof window.turnstile + ' w-html=' + (document.getElementById('w').innerHTML.length)); }, 8000);
</script></body></html>"""

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body = DEMO_HTML.encode("utf-8")
        self.send_response(200); self.send_header("Content-Type", "text/html; charset=utf-8"); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass

async def main():
    srv = HTTPServer(("127.0.0.1", 18992), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    async with async_playwright() as pw:
        browser = await pw.chromium.launch(headless=False, channel="chrome")
        page = await browser.new_page()
        await page.goto("http://127.0.0.1:18992/", wait_until="load", timeout=30000)
        await asyncio.sleep(12)
        txt = await page.evaluate("document.getElementById('log').innerText")
        print("LOG:", txt, flush=True)
        w = await page.evaluate("document.getElementById('w').innerHTML.slice(0,400)")
        print("W:", json.dumps(w, ensure_ascii=False), flush=True)
        print("frames:", [f.url[:90] for f in page.frames], flush=True)
        try:
            loc = page.locator('iframe[src*="challenges.cloudflare.com"]')
            if await loc.count() > 0:
                await loc.first.click(timeout=5000)
                print("clicked", flush=True)
        except Exception as e:
            print("click fail:", e, flush=True)
        await asyncio.sleep(12)
        txt = await page.evaluate("document.getElementById('log').innerText")
        print("LOG2:", txt, flush=True)
        await browser.close()
    srv.shutdown()

asyncio.run(main())
