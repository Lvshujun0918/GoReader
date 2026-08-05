"""CF 官方 Turnstile demo 页对照——区分 站点级 vs 环境级"""
import asyncio, json, sys, time
from playwright.async_api import async_playwright

async def main():
    async with async_playwright() as pw:
        browser = await pw.chromium.launch(headless=False, channel="chrome")
        ctx = await browser.new_context(locale="zh-CN")
        page = await ctx.new_page()
        msgs = []
        page.on("console", lambda m: msgs.append(f"{m.type}: {m.text[:150]}"))
        await page.goto("https://challenges.cloudflare.com/turnstile/v0/demo", wait_until="domcontentloaded", timeout=60000)
        await asyncio.sleep(8)
        print("demo title:", await page.title(), flush=True)
        frames = [f.url[:100] for f in page.frames]
        print("frames:", frames, flush=True)
        w = await page.evaluate("""(() => { const inp = document.querySelector('[name="cf-turnstile-response"]'); return {inputVal: inp ? inp.value : null, iframes: document.querySelectorAll('iframe').length}; })()""")
        print("widget:", json.dumps(w), flush=True)
        try:
            loc = page.locator('iframe[src*="challenges.cloudflare.com"]')
            n = await loc.count()
            print("cf iframe count:", n, flush=True)
            if n > 0:
                await loc.first.click(timeout=5000)
                print("clicked demo widget", flush=True)
        except Exception as e:
            print("click fail:", e, flush=True)
        await asyncio.sleep(12)
        w2 = await page.evaluate("""(() => { const inp = document.querySelector('[name="cf-turnstile-response"]'); return {inputVal: inp ? inp.value : null}; })()""")
        print("after click:", json.dumps(w2), flush=True)
        print("console tail:", flush=True)
        for m in msgs[-15:]:
            print("  ", m, flush=True)
        await browser.close()

asyncio.run(main())
