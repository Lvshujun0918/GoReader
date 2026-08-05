#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""69shuba 全链路实测（navigate 模式）：导航 → CF → UA 门禁 → 表单搜索 POST → 结果"""
import json, re, sys, time, urllib.parse, urllib.request

BASE = "http://127.0.0.1:8196"
CHROME_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36"

def post(path, payload):
    req = urllib.request.Request(BASE + path, data=json.dumps(payload).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=240) as r:
        return json.loads(r.read().decode("utf-8", "replace"))

body = "searchkey=" + urllib.parse.quote_from_bytes("宿命之环".encode("gbk")) + "&searchtype=all&page=1"
t0 = time.monotonic()
r = post("/solve", {"url": "https://www.69shuba.com/", "cookies": [], "maxWaitMs": 40000,
    "userAgent": CHROME_UA,
    "post": {"mode": "navigate", "action": "https://www.69shuba.com/modules/article/search.php",
             "body": body, "contentType": "application/x-www-form-urlencoded; charset=gbk", "charset": "gbk"}})
print(f"耗时 {time.monotonic()-t0:.1f}s")
if "error" in r:
    print("SOLVE ERROR:", r["error"]); print("DIAG:", json.dumps(r.get("diagnostics", {}), ensure_ascii=False)); sys.exit(1)
print("DIAG:", json.dumps(r.get("diagnostics", {}), ensure_ascii=False))
pr = r.get("postResult") or {}
print("postResult url:", pr.get("url"))
print("postResult error:", pr.get("error"))
html = pr.get("html", "")
print("最终页 html 长度:", len(html))
low = html.lower()
if "400030" in html or "event: 'fail'" in html or "errCode" in html:
    print("!!! Turnstile 平台拒绝（400030 fail event）")
    m = re.search(r"errCode = (\d+)", html)
    print("errCode:", m.group(1) if m else "?")
    sys.exit(2)
if "请使用" in html:
    m = re.search(r"请使用[^<\"']{0,80}", html)
    print("!!! 门禁/挑战页:", m.group(0) if m else "请使用...")
    sys.exit(3)
names = [(m.group(1), m.group(2).strip()) for m in re.finditer(r'<h3[^>]*>\s*<a[^>]*href="([^"]+)"[^>]*>([^<]+)</a>', html)]
if not names:
    names = [(m.group(1), m.group(2).strip()) for m in re.finditer(r'<a[^>]*href="(/book/\d+\.htm)"[^>]*>([^<]{2,40})</a>', html)]
print(f"解析到 {len(names)} 条结果:")
for u, n in names[:8]:
    print("  ", n, "→", u)
hit = any(("环" in n) or ("宿命" in n) for _, n in names)
if names and hit:
    print("RESULT: PASS"); sys.exit(0)
print("RESULT: 无结果——html 头 800 字:")
print(html[:800]); sys.exit(4)
