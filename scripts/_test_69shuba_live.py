#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""69shuba 全链路实测（走真实服务协议）：camoufox 导航 → CF 过 → UA 门禁过 → 页内搜索 POST → 真实结果"""
import json
import sys
import time
import urllib.parse
import urllib.request

BASE = "http://127.0.0.1:8196"
CHROME_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36"
SEARCH_URL = "https://www.69shuba.com/modules/article/search.php"
KEY = "宿命之环"


def post(path, payload):
    req = urllib.request.Request(
        BASE + path,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=180) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


def main():
    step = sys.argv[1] if len(sys.argv) > 1 else "all"
    if step in ("all", "health"):
        h = json.loads(urllib.request.urlopen(BASE + "/health", timeout=10).read())
        print("HEALTH:", json.dumps(h, ensure_ascii=False))
        if step == "health":
            return
    body = "searchkey=" + urllib.parse.quote_from_bytes(KEY.encode("gbk")) + "&searchtype=all&page=1"
    print("搜索 POST body:", body)
    t0 = time.monotonic()
    payload = {
        "url": "https://www.69shuba.com/",
        "cookies": [],
        "maxWaitMs": 90000,
        "userAgent": CHROME_UA,
        "post": {"mode": "navigate",
            "action": SEARCH_URL,
            "body": body,
            "contentType": "application/x-www-form-urlencoded; charset=gbk",
            "charset": "gbk",
        },
    }
    r = post("/solve", payload)
    dt = time.monotonic() - t0
    print(f"耗时 {dt:.1f}s")
    if "error" in r:
        print("SOLVE ERROR:", r["error"])
        print("DIAG:", json.dumps(r.get("diagnostics", {}), ensure_ascii=False))
        sys.exit(1)
    diag = r.get("diagnostics", {})
    print("DIAG:", json.dumps(diag, ensure_ascii=False))
    ua = r.get("userAgent", "")
    print("返回 UA:", ua[:80])
    print("cookies:", [(c["name"], (c["value"] or "")[:20]) for c in r.get("cookies", [])])
    pr = r.get("postResult") or {}
    if pr.get("error"):
        print("POST ERROR:", pr["error"])
        sys.exit(1)
    print("POST status:", pr.get("status"), "url:", pr.get("url"))
    html = pr.get("html", "")
    print("POST html 长度:", len(html))
    low = html.lower()
    if "请使用" in html or "google chrome" in low:
        import re
        m = re.search(r"请使用[^<\"']{0,80}", html)
        print("!!! UA 门禁命中:", m.group(0) if m else "请使用...")
        sys.exit(2)
    if any(x in low for x in ("just a moment", "cf-chl", "challenge-form", "cf-browser-verification")):
        print("!!! 仍是 CF 质询页")
        sys.exit(3)
    # 结果解析（书源 ruleSearch 同特征）
    names = []
    for pat in ('class="bookname"', "bookname", "article_list", "class=\"newbox\""):
        if pat in html:
            print("命中结果容器特征:", pat)
            break
    import re as _re
    for m in _re.finditer(r"<h3[^>]*>\s*<a[^>]*href=\"([^\"]+)\"[^>]*>([^<]+)</a>", html):
        names.append((m.group(1), m.group(2).strip()))
    if not names:
        # 兜底特征：li > a
        for m in _re.finditer(r"<a[^>]*href=\"(/book/\d+\.htm)\"[^>]*>([^<]{2,40})</a>", html):
            if m.group(2).strip() not in [n[1] for n in names]:
                names.append((m.group(1), m.group(2).strip()))
    print(f"解析到 {len(names)} 条结果:")
    for u, n in names[:10]:
        print("  ", n, "→", u)
    # 校验：结果里是否含搜索词相关（宿命之环 / 环）
    hit = any(("环" in n) or ("宿命" in n) for _, n in names)
    print("结果含搜索词命中:", hit)
    if names and hit:
        print("RESULT: PASS")
        sys.exit(0)
    print("RESULT: 结果异常（无解析结果或无关）——打印 html 片段")
    print(html[:1200])
    sys.exit(4)


if __name__ == "__main__":
    main()
