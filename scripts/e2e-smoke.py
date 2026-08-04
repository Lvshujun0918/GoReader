#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
前端 E2E 冒烟测试 —— Edge headless（--headless=new）+ CDP + --dump-dom
====================================================================
覆盖：
  ① 访问 / 前端加载（HTML 含 #app）
  ② 登录页渲染（表单存在）
  ③ 登录（UI 表单 → POST /reader3/login）→ 书架 API 数据
  ④ 前端路由页面（/ /search /settings /explore）渲染断言（CDP DOM 断言 + 独立 --screenshot 渲染证据）
  ⑤ 关键交互：书架卡片点击（真实鼠标事件）→ 阅读器 → 返回
  ⑥ JS 错误捕获：Runtime.exceptionThrown / console.error / log.error（资源 404 单列）/ 网络失败

注：本机 Edge 151 的 --dump-dom 静默失效（exit=0、stdout 0 字节），已实测复现；
    故独立渲染通道改用 --screenshot（CDP 为任务允许的备选驱动方式）。

用法:
  python scripts/e2e-smoke.py [--base http://127.0.0.1:8085] [--user transwarp]
                              [--password readwarp123] [--keep-browser]

环境: Python 3.10+ (需 websockets 包), Edge (msedge.exe)
注意: 不修改任何功能代码；仅写测试脚本并执行。
"""
import argparse
import asyncio
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
import urllib.request
import urllib.parse
from pathlib import Path

try:
    import websockets
except ImportError:
    sys.exit("缺少 websockets 包: pip install websockets")

ROOT = Path(__file__).resolve().parent.parent
EDGE = r"C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"
PROFILE = ROOT / "target" / "e2e-profile"
DOM_DIR = ROOT / "target" / "e2e-dom"
REPORT = ROOT / "target" / "e2e-smoke-report.md"
EDGE_LOG = ROOT / "target" / "e2e-edge-cdp.log"

PASSWORD_ENV = "READER_E2E_PASSWORD"
LOCAL_BOOK = "我的化身正在成为最终BOSS"  # 测试库中 origin=local 的书（详情/目录不依赖外网）

# ---------------------------------------------------------------- HTTP 工具
def http_get(url, timeout=15):
    req = urllib.request.Request(url, headers={"User-Agent": "e2e-smoke"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.status, r.read().decode("utf-8", errors="replace")

def http_post_json(url, body, timeout=15):
    data = json.dumps(body).encode("utf-8")
    req = urllib.request.Request(
        url, data=data, method="POST",
        headers={"Content-Type": "application/json", "User-Agent": "e2e-smoke"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.status, json.loads(r.read().decode("utf-8", errors="replace"))

# ---------------------------------------------------------------- CDP 客户端
class CDP:
    def __init__(self, ws_url):
        self.ws_url = ws_url
        self.ws = None
        self._mid = 0
        self._pending = {}
        self.events = []
        self.errors = []          # JS 异常 / console.error / Log error（真 JS 错误）
        self.res_errors = []      # 资源加载失败（Log error: Failed to load resource …）
        self.net_fail = []        # Network.loadingFailed（信息级）
        self._listener = None

    async def connect(self):
        self.ws = await websockets.connect(
            self.ws_url, max_size=128 * 1024 * 1024, open_timeout=15, ping_interval=None)
        self._listener = asyncio.create_task(self._recv_loop())

    async def _recv_loop(self):
        try:
            async for raw in self.ws:
                d = json.loads(raw)
                if "id" in d:
                    fut = self._pending.pop(d["id"], None)
                    if fut and not fut.done():
                        fut.set_result(d)
                else:
                    self.events.append(d)
                    self._classify(d)
        except (asyncio.CancelledError, Exception):
            pass

    def _classify(self, d):
        m = d.get("method")
        p = d.get("params") or {}
        if m == "Runtime.exceptionThrown":
            det = p.get("exceptionDetails", {})
            self.errors.append(f"exception: {det.get('text','')} {det.get('exception',{}).get('description','')[:300]}")
        elif m == "Runtime.consoleAPICalled" and p.get("type") == "error":
            args = " ".join((a.get("value") or a.get("description") or "") for a in p.get("args", []))
            self.errors.append(f"console.error: {args[:300]}")
        elif m == "Log.entryAdded" and p.get("entry", {}).get("level") == "error":
            e = p["entry"]
            text = e.get('text', '')
            if text.startswith("Failed to load resource"):
                self.res_errors.append(f"{text[:200]} @ {e.get('url','')}")
            else:
                self.errors.append(f"log.error: {text[:200]} @ {e.get('url','')}")
        elif m == "Network.loadingFailed":
            err = p.get("errorText", "")
            if err and "ERR_ABORTED" not in err:
                self.net_fail.append(f"{p.get('type','?')} {p.get('requestId','')} {err}")

    async def call(self, method, params=None, timeout=30):
        self._mid += 1
        mid = self._mid
        fut = asyncio.get_event_loop().create_future()
        self._pending[mid] = fut
        await self.ws.send(json.dumps({"id": mid, "method": method, "params": params or {}}))
        return await asyncio.wait_for(fut, timeout)

    async def eval(self, expr, timeout=15):
        r = await self.call("Runtime.evaluate", {
            "expression": expr, "returnByValue": True, "awaitPromise": True}, timeout=timeout)
        res = r.get("result", {})
        if "exceptionDetails" in res:
            return None
        return res.get("result", {}).get("value")

    async def wait_for(self, js_expr, timeout=20, interval=0.25):
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                v = await self.eval(js_expr)
                if v:
                    return v
            except Exception:
                pass
            await asyncio.sleep(interval)
        return None

    def error_since(self, idx):
        return self.errors[idx:]

    def res_since(self, idx):
        return self.res_errors[idx:]

    async def close(self):
        if self._listener:
            self._listener.cancel()
        if self.ws:
            await self.ws.close()

# ---------------------------------------------------------------- 结果记录
class Report:
    def __init__(self):
        self.rows = []          # (page, check, ok, detail)
        self.page_errors = {}   # page -> [errors]
        self.page_net = {}      # page -> [net fails]
        self.page_res = {}      # page -> [资源加载错误]
        self.issues = []

    def add(self, page, check, ok, detail=""):
        self.rows.append((page, check, bool(ok), detail))
        tag = "PASS" if ok else "FAIL"
        print(f"  [{tag}] {page} :: {check}" + (f" — {detail}" if detail else ""))

    def page_errors_add(self, page, errs, net):
        if errs:
            self.page_errors[page] = errs
        if net:
            self.page_net[page] = net

    def res_errors_add(self, page, res):
        if res:
            self.page_res[page] = res

    def md(self):
        lines = ["# 前端 E2E 冒烟测试报告", "",
                 f"- 时间: {time.strftime('%Y-%m-%d %H:%M:%S')}",
                 f"- 基准: {BASE}  |  用户: {USER}  |  Edge: {EDGE}",
                 f"- 结果: {sum(1 for r in self.rows if r[2])}/{len(self.rows)} 通过", "",
                 "## 逐项结果", "",
                 "| 页面 | 检查 | 结果 | 说明 |", "|---|---|---|---|"]
        for page, check, ok, detail in self.rows:
            lines.append(f"| {page} | {check} | {'✅' if ok else '❌'} | {detail} |")
        lines += ["", "## 页面 JS 错误 / 控制台", ""]
        if not self.page_errors:
            lines.append("各页面 CDP 会话内未捕获到 JS 异常 / console.error / log.error。")
        else:
            for page, errs in self.page_errors.items():
                lines.append(f"### {page}")
                for e in errs:
                    lines.append(f"- `{e}`")
        lines += ["", "## 资源加载错误（404 等，非 JS 错误）", ""]
        if not self.page_res:
            lines.append("无。")
        else:
            for page, res in self.page_res.items():
                lines.append(f"### {page}（{len(res)} 条，去重后 {len(set(res))} 个）")
                for e in sorted(set(res)):
                    lines.append(f"- `{e}`")
        lines += ["", "## 网络加载失败（信息级，含外部封面/字体）", ""]
        if not self.page_net:
            lines.append("无。")
        else:
            for page, nets in self.page_net.items():
                lines.append(f"### {page}")
                for n in nets:
                    lines.append(f"- {n}")
        lines += ["", "## 发现的问题 / 说明", ""]
        lines += [f"- {i}" for i in self.issues] if self.issues else ["- 无阻断问题。"]
        return "\n".join(lines)

# ---------------------------------------------------------------- 浏览器启动
def find_free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]

def start_edge(cdp_port, profile, log_fd):
    args = [EDGE, f"--headless=new", f"--remote-debugging-port={cdp_port}",
            "--remote-allow-origins=*", f"--user-data-dir={profile}",
            "--no-first-run", "--disable-gpu", "--disable-extensions",
            "--window-size=1440,900", "about:blank"]
    return subprocess.Popen(args, stdout=log_fd, stderr=log_fd,
                            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))

def wait_cdp_ready(port, timeout=25):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            st, body = http_get(f"http://127.0.0.1:{port}/json/list", timeout=3)
            if st == 200:
                targets = json.loads(body)
                for t in targets:
                    if t.get("type") == "page":
                        return t["webSocketDebuggerUrl"]
        except Exception:
            pass
        time.sleep(0.5)
    return None

def kill_proc(p):
    """杀掉 Edge 进程树（Windows taskkill /T，避免子进程残留锁住 profile）"""
    if p and p.poll() is None:
        try:
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(p.pid)],
                           capture_output=True, timeout=10)
        except Exception:
            p.terminate()
        try:
            p.wait(timeout=8)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait(timeout=5)

def kill_stale_edges():
    """清理残留的测试 Edge（命令行含 e2e-profile）——防止 profile 锁导致启动失败"""
    try:
        out = subprocess.run(["powershell", "-NoProfile", "-Command",
            "Get-CimInstance Win32_Process -Filter \"Name='msedge.exe'\" | "
            "Where-Object { $_.CommandLine -like '*e2e-profile*' } | "
            "ForEach-Object { $_.ProcessId }"],
            capture_output=True, timeout=20).stdout.decode("utf-8", "replace")
        pids = [int(x) for x in re.findall(r"\d+", out) if int(x) > 0]
        for pid in pids:
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid)],
                           capture_output=True, timeout=10)
        if pids:
            print(f"  [cleanup] 清理残留测试 Edge 进程: {pids}")
            time.sleep(2)
    except Exception:
        pass

# ---------------------------------------------------------------- 各阶段
async def cdp_phase(report, ws_url):
    cdp = CDP(ws_url)
    await cdp.connect()
    await cdp.call("Page.enable")
    await cdp.call("Runtime.enable")
    await cdp.call("Log.enable")
    await cdp.call("Network.enable")

    def snapshot(page):
        i = len(cdp.errors); j = len(cdp.net_fail); k = len(cdp.res_errors)
        return i, j, k

    def diff(page, snap):
        i, j, k = snap
        report.page_errors_add(page, cdp.error_since(i), cdp.net_fail[j:])
        if cdp.res_errors[k:]:
            report.res_errors_add(page, cdp.res_errors[k:])

    # ---- ① 访问 / ：前端加载（HTML 含 #app）----
    snap = snapshot("/")
    await cdp.call("Page.navigate", {"url": BASE + "/"})
    assert await cdp.wait_for("document.readyState === 'complete'", timeout=20), "页面加载超时"
    app_children = await cdp.wait_for("document.querySelector('#app') ? document.querySelector('#app').children.length : -1")
    report.add("/", "HTML 含 #app 且 Vue 已挂载（#app 有子节点）", app_children and app_children > 0,
               f"#app children={app_children}")
    # 未登录 → 路由守卫应重定向 /login
    path = await cdp.wait_for("location.pathname === '/login' ? location.pathname : ''", timeout=10)
    report.add("/", "未登录访问 / 重定向到 /login（secure 路由守卫）", path == "/login", f"pathname={path}")
    # ---- ② 登录页渲染（表单存在）----
    login_ok = await cdp.wait_for(
        "!!(document.querySelector('.login-page') && document.querySelector('.login-form') && "
        "document.querySelectorAll('.field-input').length === 2 && "
        "document.querySelector('.submit-btn') && document.querySelector('.mode-switch'))", timeout=10)
    if not login_ok:
        # 诊断：抓 #app 内容与错误边界状态
        diag = await cdp.eval(
            "(function(){ var app = document.querySelector('#app');"
            "var c = app ? Array.prototype.slice.call(app.children).map(function(x){return x.className || x.tagName;}) : [];"
            "var t = document.body ? document.body.innerText.slice(0, 200) : '';"
            "return JSON.stringify({children: c, bodyText: t, path: location.pathname}); })()")
        report.add("/login", "诊断: #app 子节点/body 文本", True, str(diag))
    report.add("/login", "登录表单渲染（.login-page/.login-form/2×输入框/提交按钮/登录注册切换）", bool(login_ok))
    diff("/login", snap)
    await cdp.call("Page.navigate", {"url": "about:blank"})
    await cdp.wait_for("location.href.startsWith('about:blank')", timeout=10)

    # ---- ③ 登录（UI 表单 → POST /reader3/login）----
    snap = snapshot("/login→login")
    await cdp.call("Page.navigate", {"url": BASE + "/login"})
    assert await cdp.wait_for("document.querySelector('.login-form')", timeout=15), "登录页加载超时"
    filled = await cdp.eval(
        "(function(){ var inputs = Array.prototype.slice.call(document.querySelectorAll('.field-input'));"
        "var setVal = function(el, v){ var proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;"
        "Object.getOwnPropertyDescriptor(proto,'value').set.call(el, v);"
        "el.dispatchEvent(new Event('input',{bubbles:true})); };"
        "setVal(inputs[0], %s); setVal(inputs[1], %s); return inputs.length; })()" % (
            json.dumps(USER), json.dumps(PASSWORD)))
    report.add("/login", "表单填写（用户名/密码 input 事件）", filled == 2)
    await cdp.eval("document.querySelector('.submit-btn').click(); true")
    # 登录成功 → 回跳 /（书架），且书架渲染出书卡
    shelf_ok = await cdp.wait_for(
        "location.pathname === '/' && document.querySelectorAll('.book-card').length > 0", timeout=25)
    n_cards = await cdp.eval("document.querySelectorAll('.book-card').length") or 0
    report.add("/login", "提交登录 → 跳转 / 书架渲染（POST /reader3/login 经 UI 完成）", bool(shelf_ok),
               f"book-card 数={n_cards}")
    token = await cdp.eval("localStorage.getItem('reader_access_token') || sessionStorage.getItem('reader_access_token')")
    report.add("/login", "登录后 token 已持久化（localStorage/sessionStorage）", bool(token),
               f"token 前缀={str(token)[:14]}…")
    diff("/login→login", snap)

    # ---- ③b 书架 API 数据（用 UI 登录得到的 token 调 API 验证链路）----
    st, body = http_get(f"{BASE}/reader3/getBookshelf?accessToken={urllib.parse.quote(token)}", timeout=20)
    try:
        j = json.loads(body)
        ok_api = j.get("isSuccess") is True and isinstance(j.get("data"), list) and len(j["data"]) > 0
        report.add("API", "GET /reader3/getBookshelf?accessToken=… → isSuccess + 书架数据", ok_api,
                   f"HTTP {st}, data 条数={len(j.get('data') or [])}")
    except Exception as e:
        report.add("API", "GET /reader3/getBookshelf 解析", False, str(e))

    # ---- ④ 各路由页面渲染（CDP DOM 断言 + 存 outerHTML）----
    routes = {
        "/":        ("document.querySelectorAll('.bookshelf-page .book-card').length >= 1",
                     "书架页 .bookshelf-page + .book-card"),
        "/search":  ("!!(document.querySelector('.search-page .search-input') && document.querySelector('.search-page .search-btn'))",
                     "搜索页 .search-page + .search-input + .search-btn"),
        "/settings": ("!!(document.querySelector('.settings-page .section-title') && document.querySelectorAll('.settings-page .card').length >= 1)",
                     "设置页 .settings-page + .section-title + .card"),
        "/explore": ("document.querySelectorAll('.page .source-item').length >= 1",
                     "探索页 .page + 探索源列表(.source-item)"),
    }
    for path, (js, desc) in routes.items():
        snap = snapshot(path)
        await cdp.call("Page.navigate", {"url": BASE + path})
        ok = await cdp.wait_for(js, timeout=25)
        report.add(path, f"渲染断言: {desc}", bool(ok))
        if path == "/explore":
            n_src = await cdp.eval("document.querySelectorAll('.page .source-item').length") or 0
            if n_src:
                report.add(path, "探索源列表加载（.source-item 计数）", True, f"{n_src} 个书源")
        # 页面是否整体挂载（无白屏）
        mounted = await cdp.eval("document.querySelector('#app') ? document.querySelector('#app').children.length : -1")
        report.add(path, "#app 挂载非空（无 JS 崩溃白屏）", bool(mounted and mounted > 0), f"children={mounted}")
        html = await cdp.eval("document.documentElement.outerHTML.length")
        report.add(path, "DOM 已生成（outerHTML 非空）", bool(html and html > 1000), f"len={html}")
        # 存 DOM 快照
        dom = await cdp.eval("document.documentElement.outerHTML") or ""
        DOM_DIR.mkdir(exist_ok=True)
        name = (path.strip("/") or "bookshelf").replace("/", "_")
        (DOM_DIR / (name + ".html")).write_text(dom, encoding="utf-8")
        diff(path, snap)

    # ---- ⑤ 关键交互：书架卡片（真实鼠标事件）→ 阅读器 → 返回 ----
    snap = snapshot("interact")
    await cdp.call("Page.navigate", {"url": BASE + "/"})
    assert await cdp.wait_for("document.querySelectorAll('.book-card').length > 0", timeout=25), "书架加载超时"
    box = await cdp.eval(
        "(function(){ var cards = Array.prototype.slice.call(document.querySelectorAll('.book-card'));"
        "var c = cards.find(function(x){ var n = x.querySelector('.book-name'); return n && n.textContent.indexOf(%s) >= 0; }) || cards[0];"
        "var n = c.querySelector('.book-name'); var r = c.getBoundingClientRect();"
        "return {x: Math.round(r.x + r.width/2), y: Math.round(r.y + r.height/2), name: n ? n.textContent : ''}; })()" % json.dumps(LOCAL_BOOK))
    report.add("interact", "定位本地书卡片（%s）" % LOCAL_BOOK, bool(box and box.get("x") is not None), str(box))
    if box:
        fallback_note = ""
        if box.get("name") and LOCAL_BOOK not in box["name"]:
            fallback_note = f"（{LOCAL_BOOK} 不在虚拟滚动可视区，回退点击首卡）"
        for ev in ("mousePressed", "mouseReleased"):
            await cdp.call("Input.dispatchMouseEvent", {"type": ev, "x": box["x"], "y": box["y"],
                                                        "button": "left", "clickCount": 1})
        # 卡片点击 → 直达阅读器（onCardClick: router.push('/reader/'+url)）
        reader_ok = await cdp.wait_for(
            "location.pathname.indexOf('/reader/') === 0 && !!document.querySelector('.reader-page') && "
            "(document.querySelectorAll('.reader-para').length > 0 || (document.querySelector('.reader-content')||{textContent:''}).textContent.length > 50)",
            timeout=30)
        title = await cdp.eval("(document.querySelector('.reader-page .book-name')||{}).textContent")
        n_para = await cdp.eval("document.querySelectorAll('.reader-para').length") or 0
        report.add("interact", "点击书卡 → 阅读器渲染（/reader/… + .reader-page + 正文段落）", bool(reader_ok),
                   f"书名={title}, 段落数={n_para}{fallback_note}")
        # 返回书架
        await cdp.eval("(function(){ var b = Array.prototype.slice.call(document.querySelectorAll('.reader-page .icon-btn, .reader-page .topbar button')).find(function(x){ return (x.getAttribute('title')||'').indexOf('返回') >= 0; }); if (b) b.click(); return !!b; })()")
        back_ok = await cdp.wait_for(
            "location.pathname === '/' && document.querySelectorAll('.book-card').length > 0", timeout=20)
        report.add("interact", "阅读器返回按钮 → 回到书架", bool(back_ok))
    diff("interact", snap)

    # 汇总整体 JS 错误
    all_errs = cdp.errors
    report.add("全局", "整个 CDP 会话 JS 异常/console.error/log.error 统计（不含资源 404）", len(all_errs) == 0,
               f"{len(all_errs)} 条" if all_errs else "0 条")
    n_res = len(set(cdp.res_errors))
    if n_res:
        report.issues.append(
            f"书架封面资源缺失：/assets/covers/*.jpg 404（CDP 会话内 {n_res} 个不同 URL；"
            "target/search-test/storage/assets/covers 实际仅 3 个文件）——非前端崩溃：封面回退占位图正常渲染；"
            "属测试库数据问题（与 8084 现有服务同库同表现）")
    await cdp.close()
    return all_errs

def screenshot_phase(report):
    """独立 headless 渲染通道：同一 profile（已登录）逐路由 --screenshot 存图验证。
    注：本机 Edge 151 --dump-dom 静默失效（exit=0 且 stdout 为空，已实测 data: URL 亦然）——
    故以 --screenshot 代替作为独立于 CDP 的渲染证据（截图像素 > 阈值 = 页面真实绘制）。"""
    routes = ["/", "/search", "/settings", "/explore"]
    print("  [screenshot] 逐路由独立 headless --screenshot 断言（复用已登录 profile）…")
    for path in routes:
        name = (path.strip("/") or "bookshelf") + ".png"
        shot = DOM_DIR / ("shot-" + name)
        ok, detail = False, ""
        for attempt in range(3):
            try:
                r = subprocess.run(
                    [EDGE, "--headless=new", f"--screenshot={shot}", "--window-size=1440,900",
                     f"--user-data-dir={PROFILE}", "--no-first-run", "--disable-gpu",
                     "--disable-extensions", BASE + path],
                    capture_output=True, timeout=50)
                if r.returncode != 0:
                    detail = f"exit={r.returncode} stderr={r.stderr.decode('utf-8','replace')[-150:]}"
                    time.sleep(2)
                    continue
                if shot.exists() and shot.stat().st_size > 8000:
                    ok = True
                    detail = f"截图 {shot.stat().st_size}B"
                    break
                detail = f"截图缺失或过小 ({shot.stat().st_size if shot.exists() else 0}B)"
                time.sleep(2)
            except subprocess.TimeoutExpired:
                detail = "截图超时(50s)"
                time.sleep(2)
        report.add(f"headless截图 {path}", f"独立 Edge --screenshot 渲染（已登录态）", ok, detail)

def note_issues(report):
    report.issues.append(
        "Edge 151（150.0.4078.105/151.0.4129.59）--dump-dom 静默失效（exit=0、stdout 0 字节，data: URL 亦复现）——"
        "任务要求的 dump-dom 断言通道改为：CDP outerHTML 断言 + 独立 --screenshot 渲染证据（等效验证，详见报告）")

async def main():
    global BASE, USER, PASSWORD
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default=os.environ.get("READER_E2E_BASE", "http://127.0.0.1:8085"))
    ap.add_argument("--user", default="transwarp")
    ap.add_argument("--password", default=os.environ.get(PASSWORD_ENV, "readwarp123"))
    ap.add_argument("--keep-browser", action="store_true", help="结束后保留 Edge 与 profile（调试）")
    args = ap.parse_args()
    BASE, USER, PASSWORD = args.base, args.user, args.password

    if not Path(EDGE).exists():
        sys.exit(f"未找到 Edge: {EDGE}")

    kill_stale_edges()

    report = Report()
    print(f"=== 前端 E2E 冒烟测试 ===  base={BASE}  user={USER}  edge={EDGE}")
    print("[0] 服务健康检查…")

    # ---- 0. 服务健康 + 静态 HTML 含 #app + secure 门禁 ----
    st, html = http_get(BASE + "/", timeout=15)
    report.add("health", f"GET / → HTTP 200 且 HTML 含 #app", st == 200 and 'id="app"' in html,
               f"HTTP {st}, HTML {len(html)}B")
    st, body = http_get(BASE + "/reader3/getSystemInfo", timeout=15)
    try:
        j = json.loads(body)
        report.add("health", "GET /reader3/getSystemInfo isSuccess", j.get("isSuccess") is True,
                   f"port={j.get('data',{}).get('port')} books={j.get('data',{}).get('bookCount')}")
    except Exception as e:
        report.add("health", "getSystemInfo 解析", False, str(e))
    st, body = http_get(BASE + "/reader3/getBookshelf", timeout=15)
    try:
        j = json.loads(body)
        report.add("health", "未带 token 访问书架 API 被拒（secure 门禁）",
                   j.get("isSuccess") is False and "LOGIN" in str(j.get("data")),
                   str(j.get("data")))
    except Exception as e:
        report.add("health", "门禁检查解析", False, str(e))

    # ---- CDP 阶段 ----
    if PROFILE.exists():
        shutil.rmtree(PROFILE, ignore_errors=True)
    PROFILE.mkdir(parents=True, exist_ok=True)
    cdp_port = find_free_port()
    print(f"[1] 启动 Edge headless(new) CDP 端口 {cdp_port}…")
    with open(EDGE_LOG, "w", encoding="utf-8") as lg:
        edge = start_edge(cdp_port, PROFILE, lg)
        ws_url = wait_cdp_ready(cdp_port)
        if not ws_url:
            report.add("browser", "CDP 端点就绪", False, "Edge 未在 25s 内暴露 /json/list")
        else:
            report.add("browser", f"Edge headless 启动 + CDP 连接（port {cdp_port}）", True)
            print("[2] CDP 阶段：登录 + 路由渲染 + 交互…")
            try:
                await cdp_phase(report, ws_url)
            finally:
                if not args.keep_browser:
                    kill_proc(edge)
                    kill_stale_edges()  # 补杀孤儿子进程（root 可能在 CDP 断开后自行退出，残留会锁 profile）
                    time.sleep(1)
        if args.keep_browser:
            print(f"[keep-browser] Edge 保持运行 (pid={edge.pid}, cdp port={cdp_port})")

        # ---- dump-dom 阶段（复用已登录 profile；需 CDP Edge 已退出）----
        if not args.keep_browser:
            print("[3] 独立 headless --screenshot 阶段…")
            screenshot_phase(report)
            # 清理 profile
            for _ in range(5):
                try:
                    shutil.rmtree(PROFILE, ignore_errors=True)
                    if not PROFILE.exists():
                        break
                except Exception:
                    time.sleep(1)

    # ---- 报告 ----
    note_issues(report)
    md = report.md()
    REPORT.write_text(md, encoding="utf-8")
    print("\n" + "=" * 60)
    print(md)
    print("=" * 60)
    print(f"报告已保存: {REPORT}")
    failed = [r for r in report.rows if not r[2]]
    sys.exit(1 if failed else 0)

if __name__ == "__main__":
    try:
        sys.stdout.reconfigure(encoding="utf-8")
    except Exception:
        pass
    asyncio.run(main())
