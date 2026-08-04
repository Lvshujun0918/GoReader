#!/usr/bin/env python3
"""API 全面扫描：遍历 /reader3 路由逐接口测试（正常/空参/错参/边界）→ 报告 FAIL"""
import sqlite3, json, subprocess, urllib.parse, sys, time, re

BASE = "http://localhost:8084"
db = sqlite3.connect('target/search-test/storage/reader.db')
tok = db.execute("SELECT token FROM users WHERE username='transwarp'").fetchone()[0]
db.close()
A = f"accessToken=transwarp:{tok}"

def call(path, params="", method="GET", body=None, timeout=25):
    url = f"{BASE}{path}" + ("?" + params + "&" + A if params else "?" + A)
    if method == "GET":
        r = subprocess.run(["curl","-s","-m",str(timeout),url], capture_output=True, timeout=timeout+10)
    else:
        args = ["curl","-s","-m",str(timeout),"-X",method]
        if body is not None: args += ["-H","Content-Type: application/json","-d",json.dumps(body)]
        args.append(url)
        r = subprocess.run(args, capture_output=True, timeout=timeout+10)
    try:
        return json.loads(r.stdout.decode('utf-8', errors='replace'))
    except Exception:
        return {"_raw": r.stdout.decode('utf-8', errors='replace')[:100], "_http": r.returncode}

def check(name, d, expect_ok=True):
    if "_raw" in d:
        print(f"  ❌ {name}: 响应异常 {d['_raw']}")
        return False
    ok = bool(d.get("isSuccess"))
    if expect_ok and not ok:
        print(f"  ❌ {name}: {str(d.get('errorMsg'))[:70]}")
        return False
    if not expect_ok and ok:
        print(f"  ⚠️ {name}: 期望失败但成功")
        return False
    return True

results = []
def t(name, d, expect_ok=True):
    results.append(check(name, d, expect_ok))

# 先拿书架书/本地书/书源
d = call("/reader3/getBookshelf")
books = d.get("data") or []
loc = next((b for b in books if b.get("origin") == "loc_book"), None)
web = next((b for b in books if b.get("origin") not in ("loc_book", "local") and b.get("origin")), None)
src = (call("/reader3/getBookSources").get("data") or [])
src_url = src[0].get("bookSourceUrl") if src else ""

print("=== 基础 ===")
t("getSystemInfo", call("/reader3/getSystemInfo"))
t("getBookshelf", call("/reader3/getBookshelf"))
t("getBookSources", call("/reader3/getBookSources"))
t("getBookGroups", call("/reader3/getBookGroups"))
t("getCacheInfo", call("/reader3/getCacheInfo"))
t("getTxtTocRules", call("/reader3/getTxtTocRules"))
t("getReplaceRules", call("/reader3/getReplaceRules"))
t("getRssSources", call("/reader3/getRssSources"))
t("getHttpTTSList", call("/reader3/getHttpTTSList"))
t("getTTSVoices", call("/reader3/getTTSVoices"))
t("getExploreSources", call("/reader3/getExploreSources"))
t("getSourceSubs", call("/reader3/getSourceSubs"))
t("getUsers", call("/reader3/getUsers"))
t("getUserConfig", call("/reader3/getUserConfig"))
t("getReadingStats", call("/reader3/getReadingStats"))
t("getOpdsSettings", call("/reader3/getOpdsSettings"))

print("=== 边界/异常 ===")
t("空 key 搜索", call("/reader3/searchBook", "key="), expect_ok=False)
t("非法 token", call("/reader3/getBookshelf", "accessToken=bad:token"), expect_ok=False)
t("空 url 详情", call("/reader3/getBookInfo"), expect_ok=False)
t("空 tocUrl", call("/reader3/getBookToc"), expect_ok=False)
t("非法 id 书签", call("/reader3/getBookmarks", "bookUrl=__nonexist__"), expect_ok=False)
t("非法书源", call("/reader3/getBookSource", "bookSource=__nope__"), expect_ok=False)
t("超长 key 搜索", call("/reader3/searchBook", "key=" + "x"*5000, timeout=15), expect_ok=False)
t("非法 home", call("/reader3/file/list", "home=__EVIL__&path=/"), expect_ok=False)
t("穿越路径", call("/reader3/file/list", "home=__HOME__&path=../../.."), expect_ok=False)
t("隐藏文件", call("/reader3/file/list", "home=__HOME__&path=/.users.key"), expect_ok=False)
t("非法格式导出", call("/reader3/exportBook", "url=x&format=evil"), expect_ok=False)
t("空批量删除", call("/reader3/deleteBooks", "", "POST", {"bookUrls": []}), expect_ok=False)
t("不存在书删除", call("/reader3/deleteBook", "url=__nope__"), expect_ok=False)
t("空统计", call("/reader3/getReadingStats"))

print("=== 本地书 ===")
if loc:
    bu = urllib.parse.quote(loc["bookUrl"], safe='')
    t("本地书详情", call("/reader3/getBookInfo", f"url={bu}&bookSource=loc_book"))
    t("本地书目录", call("/reader3/getBookToc", f"url={bu}"))
    d = call("/reader3/getBookToc", f"url={bu}")
    chaps = d.get("data") or []
    if chaps:
        cu = urllib.parse.quote(chaps[1]["url"], safe='')
        t("本地书正文", call("/reader3/getBookContent", f"url={cu}"))
    t("本地书全书搜索", call("/reader3/searchBookContent", f"key={urllib.parse.quote('的')}&bookUrl={bu}"))
    t("本地书导出", call("/reader3/exportBook", f"url={bu}&format=epub", timeout=40))
else:
    print("  - 无本地书")

print("=== 书源书 ===")
if web:
    bu = urllib.parse.quote(web["bookUrl"], safe='')
    toc = urllib.parse.quote(web.get("tocUrl") or web["bookUrl"], safe='')
    o = urllib.parse.quote(web.get("origin") or "", safe='')
    t("书源书详情", call("/reader3/getBookInfo", f"url={bu}&bookSource={o}"))
    t("书源书目录", call("/reader3/getBookToc", f"tocUrl={toc}&bookSource={o}", timeout=40))
    d = call("/reader3/getBookToc", f"tocUrl={toc}&bookSource={o}", timeout=40)
    chaps = d.get("data") or []
    if chaps:
        cu = urllib.parse.quote(chaps[0]["url"], safe='')
        t("书源书正文", call("/reader3/getBookContent", f"url={cu}&bookSource={o}", timeout=40))
    t("换源搜索", call("/reader3/searchBookSource", f"url={bu}&bookSource={o}", timeout=60))
else:
    print("  - 无书源书")

print("=== 写入类 ===")
t("存进度", call("/reader3/saveBookProgress", "", "POST", {"bookUrl": "x", "durChapterIndex": 0, "durChapterTime": 1786000000000}))
t("存配置", call("/reader3/saveUserConfig", "", "POST", {"config": {"a": 1}}))
t("替换规则 CRUD", call("/reader3/saveReplaceRule", "", "POST", {"replaceSummary": "t", "rule": "a|b", "serialNumber": 1}))
t("TXT规则 CRUD", call("/reader3/saveTxtTocRule", "", "POST", {"name": "t", "rule": "第.*章", "enable": 1}))
t("HTTP TTS CRUD", call("/reader3/saveHttpTTS", "", "POST", {"name": "t", "url": "http://x"}))
t("书源保存(非法)", call("/reader3/saveBookSource", "", "POST", {"bookSource": "{bad json"}), expect_ok=False)
t("书签保存(空)", call("/reader3/saveBookmark", "", "POST", {}), expect_ok=False)
t("分组保存(空)", call("/reader3/saveBookGroup", "", "POST", {}), expect_ok=False)
t("删除分组(不存在)", call("/reader3/deleteBookGroup", "", "POST", {"id": 99999}), expect_ok=False)
t("清缓存", call("/reader3/clearCache", "", "POST", {}))

print("=== 书源订阅 ===")
if src_url:
    su = urllib.parse.quote(src_url, safe='')
    t("订阅书源", call("/reader3/saveSourceSub", "", "POST", {"bookSourceUrl": src_url}))
    t("订阅列表", call("/reader3/getSourceSubs"))
    t("刷新订阅", call("/reader3/refreshSourceSub", f"url={su}", timeout=40))
    t("删订阅", call("/reader3/deleteSourceSub", "", "POST", {"bookSourceUrl": src_url}))

print("=== SSE 接口（流完整性） ===")
for name, path in [("全书搜索SSE", "/reader3/searchBookMultiSSE"), ("调试SSE", "/reader3/bookSourceDebugSSE")]:
    url = f"{BASE}{path}?{A}" + (f"&key={urllib.parse.quote('诡秘')}" if "search" in path else "")
    try:
        r = subprocess.run(["curl","-s","-m","20","-N",url], capture_output=True, timeout=30)
        out = r.stdout.decode('utf-8', errors='replace')
        ok = "data:" in out or "event:" in out or len(out) > 10
        print(f"  {'✅' if ok else '❌'} {name}: {len(out)}B")
    except Exception as e:
        print(f"  ❌ {name}: {e}")

print("=== OPDS ===")
for name, path in [("1.2 根", "/opds"), ("shelf", "/opds/shelf"), ("recent", "/opds/recent"), ("local", "/opds/local"),
                   ("groups", "/opds/groups"), ("search", "/opds/search?q=诡秘"), ("2.0", "/opds/catalog"),
                   ("2.0 shelf", "/opds/catalog/shelf"), ("2.0 search", "/opds/catalog/search?q=诡秘"), ("opensearch", "/opds/opensearch.xml")]:
    r = subprocess.run(["curl","-s","-m","15",f"{BASE}{path}?{A}"], capture_output=True, timeout=25)
    out = r.stdout.decode('utf-8', errors='replace')
    ok = len(out) > 50 and "error" not in out.lower()[:50]
    print(f"  {'✅' if ok else '❌'} OPDS {name}: {len(out)}B")

print("=== WebDAV ===")
import base64 as b64
auth = "Basic " + b64.b64encode(b"transwarp:readwarp123").decode()
for name, args in [("OPTIONS", ["-X","OPTIONS"]), ("PROPFIND", ["-X","PROPFIND","-H","Depth: 1"])]:
    r = subprocess.run(["curl","-s","-m","15","-H",f"Authorization: {auth}"] + args + [f"{BASE}/reader3/webdav/"], capture_output=True, timeout=25)
    print(f"  {'✅' if r.returncode == 0 and len(r.stdout) > 0 or name == 'OPTIONS' else '❌'} WebDAV {name}: {r.stdout[:40].decode('utf-8', errors='replace')!r}")

passed = sum(1 for x in results if x)
print(f"\n=== 扫描完成: {passed}/{len(results)} 通过 ===")
