#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
书源批量检测审计（只读：不修改/不删除任何书源）
====================================================================
遍历测试库全部书源（target/search-test/storage/reader.db，约 431 个），并发 8，
逐源执行：
  ① 站点可达性探测：searchUrl（无则 bookSourceUrl）域名解析 + TCP 连接（8s 超时）
  ② 搜索链路：经本地服务（8084）bookSourceDebugSSE action=search，
     固定关键词「诡秘之主」，复用生产规则引擎逐步骤输出（规则解析/URL 构造/请求/规则应用）
分类（三类 + 两个旁路桶）：
  - site_down        站点挂了：HTTP 层不可达（DNS/连接失败/超时/TLS/重定向环）
                     或 HTTP 4xx/5xx（搜索端点不可用——站点侧问题，非规则问题）
  - rule_engine_error 规则/引擎问题：HTTP 正常但搜索 0 结果 / 解析异常 / JS 报错
                     （错误类型：css/jsonpath/xpath/regex/js/zero_results/other）
  - normal           正常：有结果，或明确“站内无此书”（响应体含无结果特征标记）
  - no_search_url    旁路：未配置 searchUrl，无法搜索（单列统计）
  - audit_error      旁路：审计链路本身失败（本地服务异常/SSE 超时），单列统计

0 结果判定（区分“站内无此书” vs 规则问题）：
  对 2xx 且 0 结果的源，用最终搜索 URL 在 Python 侧重放 GET/POST（8s 超时），
  检查响应体无结果特征标记（没有找到/未找到/no results…）：
  命中 → normal（站内无此书）；未命中 → rule_engine_error(零结果)。

礼貌限速：全局令牌桶 ≤6 请求/s；同主机最小间隔 1s、同主机并发 ≤2；
          并发线程 8（对应服务侧并发 8）。

用法:
  python scripts/source-audit.py [--resume] [--limit N] [--only url1,url2]
                                 [--db .../reader.db] [--base http://127.0.0.1:8084]
                                 [--workers 8]

环境: 需本地服务运行（8084，READER_APP_WORKDIR=target/search-test 且 READER_APP_SECURE=true）
输出: scripts/source-audit-report.json（明细+摘要）/ source-audit-report.md（可读版）
注意: 本脚本只读审计——不修改、不删除、不禁用任何书源。
"""
import argparse
import json
import re
import socket
import sqlite3
import sys
import threading
import time
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_DB = ROOT / "target" / "search-test" / "storage" / "reader.db"
DEFAULT_BASE = "http://127.0.0.1:8084"
KEYWORD = "诡秘之主"
REPORT_JSON = Path(__file__).resolve().parent / "source-audit-report.json"
REPORT_MD = Path(__file__).resolve().parent / "source-audit-report.md"

PROBE_TIMEOUT = 8        # 站点可达性探测超时（秒）
SSE_TIMEOUT = 75         # debug SSE 总超时（含服务端 CF 求解）
REPLAY_TIMEOUT = 8       # 0 结果重放超时（秒）
WORKERS_DEFAULT = 8

# 站内无此书特征标记（响应体命中任一 → 明确“站内无此书”）
NO_RESULT_MARKERS = [
    "没有找到", "未找到", "找不到", "没有搜索到", "未搜索到", "没有查到", "未查到",
    "没有结果", "无搜索结果", "无结果", "没有相关内容", "无相关内容", "没有相关结果",
    "没有相关", "没有此书", "本站暂无", "暂无此书", "查无此书", "此书不存在", "不存在该书",
    "搜不到", "没有数据", "无数据", "没有您要找的", "没有你要找的", "没有找到与",
    "not found", "no results", "no result", "nothing found", "no books found",
    "0 results", "no matching",
]

# ---------------------------------------------------------------- 限速器
class RateLimiter:
    """全局令牌桶（≤refill/s）+ 同主机最小间隔 + 同主机并发上限"""

    def __init__(self, refill=6.0, capacity=6.0, host_gap=1.0, host_max_concurrent=2):
        self.refill = refill
        self.capacity = capacity
        self.host_gap = host_gap
        self.host_max = host_max_concurrent
        self.tokens = capacity
        self.last = time.monotonic()
        self.lock = threading.Lock()
        self.host_lock = threading.Lock()
        self.host_last = {}      # host -> monotonic ts
        self.host_active = {}    # host -> count

    def _wait_global(self):
        while True:
            now = time.monotonic()
            with self.lock:
                self.tokens = min(self.capacity, self.tokens + (now - self.last) * self.refill)
                self.last = now
                if self.tokens >= 1.0:
                    self.tokens -= 1.0
                    return
                wait = (1.0 - self.tokens) / self.refill
            time.sleep(min(wait, 0.1))

    def acquire(self, host, timeout=45.0):
        """获取发送许可（host 可为 None）。返回 False 表示等待超时（仍放行由调用方决定）"""
        deadline = time.monotonic() + timeout
        # 全局限速
        self._wait_global()
        if not host:
            return True
        # 同主机并发上限
        while True:
            with self.host_lock:
                if self.host_active.get(host, 0) < self.host_max:
                    self.host_active[host] = self.host_active.get(host, 0) + 1
                    break
            if time.monotonic() > deadline:
                return False
            time.sleep(0.1)
        # 同主机最小间隔
        while True:
            with self.host_lock:
                gap = self.host_gap - (time.monotonic() - self.host_last.get(host, 0.0))
            if gap <= 0:
                with self.host_lock:
                    self.host_last[host] = time.monotonic()
                return True
            if time.monotonic() > deadline:
                with self.host_lock:
                    self.host_active[host] = max(0, self.host_active.get(host, 0) - 1)
                return False
            time.sleep(min(gap, 0.1))

    def release(self, host):
        if host:
            with self.host_lock:
                self.host_active[host] = max(0, self.host_active.get(host, 0) - 1)

# ---------------------------------------------------------------- 工具
def host_of_url(url):
    """提取 URL 主机（http/https 才返回）。返回 (host, port, scheme) 或 None"""
    if not url:
        return None
    m = re.match(r"^(https?)://([^/?#]+)", url.strip())
    if not m:
        return None
    scheme, netloc = m.group(1), m.group(2)
    host = netloc.split("@")[-1]
    if ":" in host:
        h, _, p = host.rpartition(":")
        if p.isdigit():
            return h, int(p), scheme
    return host, (443 if scheme == "https" else 80), scheme


def probe_site(url, timeout=PROBE_TIMEOUT):
    """站点可达性：DNS 解析 + TCP 连接（8s）。返回 (ok, ms, err)"""
    hp = host_of_url(url)
    if not hp:
        return None, 0, "无可用 http(s) URL（无法探测）"
    host, port, _ = hp
    t0 = time.time()
    try:
        infos = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
        if not infos:
            return False, 0, "DNS 解析无结果"
    except socket.gaierror as e:
        return False, 0, f"DNS 解析失败: {e}"
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True, int((time.time() - t0) * 1000), None
    except socket.timeout:
        return False, int((time.time() - t0) * 1000), "连接超时"
    except OSError as e:
        return False, int((time.time() - t0) * 1000), f"连接失败: {e.strerror or e}"


def http_request(url, method="GET", body=None, timeout=REPLAY_TIMEOUT, headers=None):
    """简易 HTTP 请求（重放用）。返回 (status, body_text, err)"""
    # 最终 URL 可能含未编码中文（书源 searchUrl 原样），先百分号编码（保留已编码序列）
    url = urllib.parse.quote(url, safe=":/?#[]@!$&'()*+,;=%~-._")
    hdrs = {"User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) source-audit/1.0"}
    if headers:
        hdrs.update(headers)
    data = None
    if method.upper() == "POST" and body is not None:
        data = body.encode("utf-8")
        hdrs["Content-Type"] = "application/x-www-form-urlencoded"
    req = urllib.request.Request(url, data=data, method=method.upper(), headers=hdrs)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read(512 * 1024)
            charset = r.headers.get_content_charset() or "utf-8"
            return r.status, raw.decode(charset, errors="replace"), None
    except urllib.error.HTTPError as e:
        try:
            raw = e.read(256 * 1024)
            charset = e.headers.get_content_charset() or "utf-8"
            return e.code, raw.decode(charset, errors="replace"), None
        except Exception:
            return e.code, "", None
    except Exception as e:
        return None, "", str(e)


def parse_sse(body):
    """解析 SSE 文本 → 事件列表 [{type, message|data}]（兼容 data: json / event: error）"""
    events = []
    cur_event = None
    for line in body.splitlines():
        line = line.strip()
        if line.startswith("event:"):
            cur_event = line[6:].strip()
        elif line.startswith("data:"):
            payload = line[5:].strip()
            try:
                d = json.loads(payload)
            except Exception:
                d = {"type": cur_event or "data", "message": payload}
            if cur_event and "type" not in d:
                d["type"] = cur_event
            events.append(d)
            cur_event = None
    return events


# ---------------------------------------------------------------- 错误分类
NETWORK_KW = [
    ("dns", ["dns", "无法解析", "主机名", "name resolution", "lookup", "getaddrinfo"]),
    ("timeout", ["timed out", "timeout", "超时", "timedout"]),
    ("connect", ["connection refused", "拒绝", "connection reset", "reset by peer",
                 "connect", "closed", "unreachable", "不可达", "积极拒绝"]),
    ("redirect_loop", ["too many redirects", "redirect"]),
    ("tls", ["tls", "certificate", "handshake", "ssl"]),
]

RULE_ERROR_KW = [
    ("js", ["js 执行失败", "referenceerror", "typeerror", "syntaxerror", "error:", "is not defined",
            "javascript", "eval"]),
    ("css", ["css", "selector", "选择器"]),
    ("jsonpath", ["jsonpath", "json path"]),
    ("xpath", ["xpath", "xpath_select"]),
    ("regex", ["regex", "正则", "re2", "regexp"]),
]


def classify_network_error(msg):
    low = (msg or "").lower()
    for kind, kws in NETWORK_KW:
        for kw in kws:
            if kw in low:
                return kind
    return "network"


def classify_rule_error(msg, detail=None):
    low = (msg or "").lower()
    if detail and isinstance(detail, dict):
        if detail.get("jsError"):
            return "js"
    for kind, kws in RULE_ERROR_KW:
        for kw in kws:
            if kw in low:
                return kind
    return "other"


def find_no_result_marker(body):
    low = body.lower()
    for m in NO_RESULT_MARKERS:
        if m.lower() in low:
            return m
    return None


# ---------------------------------------------------------------- 审计单源
class Audit:
    def __init__(self, args):
        self.args = args
        self.base = args.base.rstrip("/")
        self.ratelimit = RateLimiter()
        self.server_ok = True
        self.server_ok_lock = threading.Lock()

    def _token(self, ns):
        db = sqlite3.connect(self.args.db, timeout=30)
        try:
            row = db.execute("SELECT token FROM users WHERE username=?",
                             (ns,)).fetchone()
            return row[0] if row else None
        finally:
            db.close()

    def _sse(self, source_url, ns):
        tok = self._token(ns)
        q = urllib.parse.urlencode({
            "action": "search", "key": KEYWORD, "bookSource": source_url,
            "accessToken": f"{ns}:{tok}" if tok else "",
        })
        url = f"{self.base}/reader3/bookSourceDebugSSE?{q}"
        req = urllib.request.Request(url, headers={"User-Agent": "source-audit"})
        try:
            with urllib.request.urlopen(req, timeout=SSE_TIMEOUT) as r:
                return parse_sse(r.read().decode("utf-8", "replace"))
        except Exception as e:
            return None, str(e)

    def audit_one(self, row):
        """row: dict（db 行）。返回记录 dict"""
        url, name = row["book_source_url"], row["book_source_name"]
        ns = row["user_namespace"] or "default"
        search_url = row["search_url"] or ""
        rec = {
            "url": url, "name": name, "namespace": ns, "enabled": bool(row["enabled"]),
            "searchUrl": search_url[:300],
            "classification": None, "errorType": None, "errorDetail": None,
            "resultCount": None, "bookListKind": None, "status": None,
            "finalUrl": None, "probeOk": None, "probeMs": None, "probeError": None,
            "elapsedMs": None, "note": None, "bodySize": None, "marker": None,
        }
        t0 = time.time()

        # 探测 URL：searchUrl 绝对 URL 优先，否则 bookSourceUrl
        probe_url = None
        if host_of_url(search_url):
            probe_url = search_url.split("##")[0].strip()
        elif host_of_url(url):
            probe_url = url.split("##")[0].strip()
        if probe_url:
            hp = host_of_url(probe_url)
            if hp and not self.ratelimit.acquire(hp[0], timeout=30):
                rec["probeError"] = "限速等待超时，跳过探测"
            else:
                try:
                    ok, ms, err = probe_site(probe_url)
                    rec["probeOk"], rec["probeMs"], rec["probeError"] = ok, ms, err
                finally:
                    if hp:
                        self.ratelimit.release(hp[0])
        else:
            rec["probeError"] = "无 http(s) URL 可探测（searchUrl/bookSourceUrl 均非 URL）"

        # 无 searchUrl → 旁路桶
        if not search_url.strip():
            rec["classification"] = "no_search_url"
            rec["errorType"] = "no_search_url"
            rec["errorDetail"] = "书源未配置 searchUrl，无法执行搜索链路"
            rec["elapsedMs"] = int((time.time() - t0) * 1000)
            return rec

        # 搜索链路：debug SSE（经本地服务；同主机限速）
        hp = host_of_url(probe_url) if probe_url else None
        if hp and not self.ratelimit.acquire(hp[0], timeout=60):
            hp = None  # 超时仍放行（避免死锁）
        try:
            res = self._sse(url, ns)
        finally:
            if hp:
                self.ratelimit.release(hp[0])

        if isinstance(res, tuple):  # SSE 请求本身失败
            rec["classification"] = "audit_error"
            rec["errorType"] = "sse_error"
            rec["errorDetail"] = f"本地服务 SSE 调用失败: {res[1]}"
            rec["elapsedMs"] = int((time.time() - t0) * 1000)
            with self.server_ok_lock:
                if "10061" in str(res[1]) or "拒绝" in str(res[1]):
                    self.server_ok = False
            return rec

        events = res
        steps = {e.get("message", {}).get("ruleName", ""): e.get("message", {})
                 for e in events if e.get("type") == "step"}
        terminal = [e for e in events if e.get("type") in ("result", "error")]
        result_books = None
        term_error = None
        for e in terminal:
            if e.get("type") == "result":
                result_books = e.get("data")
            else:
                term_error = e.get("message")
        if isinstance(result_books, dict) and "data" in result_books:
            result_books = result_books.get("data")

        step_url = steps.get("URL 构造", {})
        step_fetch = steps.get("请求 URL", {})
        step_rule = steps.get("规则应用（bookList 字段）", {})
        step_parse = steps.get("规则解析（ruleSearch）", {})
        detail_parse = step_parse.get("detail") or {}
        if isinstance(detail_parse, dict):
            rec["bookListKind"] = detail_parse.get("bookListKind")
        rec["finalUrl"] = (step_fetch.get("url") or step_url.get("url") or "")[:300]
        fetch_detail = step_fetch.get("detail") or {}
        if isinstance(fetch_detail, dict):
            rec["status"] = fetch_detail.get("status")

        # ① URL 构造失败（JS/URL 规则问题）
        if step_url.get("error"):
            etype = classify_rule_error(step_url["error"], step_url.get("detail"))
            if etype == "other":
                # URL 构造失败多半是 searchUrl JS 或非法 URL
                etype = "js" if (search_url.lstrip().startswith("@js:") or "<js>" in search_url) else "other"
            rec["classification"] = "rule_engine_error"
            rec["errorType"] = etype
            rec["errorDetail"] = f"URL 构造失败: {step_url['error'][:300]}"
            rec["elapsedMs"] = int((time.time() - t0) * 1000)
            return rec

        # ② 请求失败（网络层）→ 站点挂了；但 "builder error" 是 URL 非法（规则问题）
        if step_fetch.get("error"):
            msg = step_fetch["error"]
            if "builder error" in msg.lower():
                rec["classification"] = "rule_engine_error"
                rec["errorType"] = "other"
                rec["errorDetail"] = (f"请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: "
                                      f"{msg[:200]}; 构造 URL={rec['finalUrl'][:150]}")
                rec["elapsedMs"] = int((time.time() - t0) * 1000)
                return rec
            etype = classify_network_error(msg)
            detail = f"请求失败: {msg[:300]}"
            if rec["probeError"]:
                detail += f"；探测: {rec['probeError'][:120]}"
            rec["classification"] = "site_down"
            rec["errorType"] = etype
            rec["errorDetail"] = detail
            rec["elapsedMs"] = int((time.time() - t0) * 1000)
            return rec

        # ③ HTTP 状态 >= 400 → 站点侧问题（搜索端点不可用）
        st = rec["status"]
        if st is not None and st >= 400:
            rec["classification"] = "site_down"
            rec["errorType"] = f"http_{st}"
            rec["errorDetail"] = f"搜索端点 HTTP {st}（站点侧不可用/反爬拦截）"
            rec["elapsedMs"] = int((time.time() - t0) * 1000)
            return rec

        # ④ 规则应用错误 → 规则/引擎问题
        if step_rule.get("error"):
            etype = classify_rule_error(step_rule["error"], step_rule.get("detail"))
            rec["classification"] = "rule_engine_error"
            rec["errorType"] = etype
            rec["errorDetail"] = f"规则应用失败: {step_rule['error'][:300]}"
            rec["elapsedMs"] = int((time.time() - t0) * 1000)
            return rec

        # ⑤ 结果判定
        if isinstance(result_books, list):
            rec["resultCount"] = len(result_books)
        if rec["resultCount"] and rec["resultCount"] > 0:
            rec["classification"] = "normal"
            rec["note"] = f"搜索到 {rec['resultCount']} 条结果"
            rec["elapsedMs"] = int((time.time() - t0) * 1000)
            return rec

        # ⑥ 0 结果：重放最终 URL 检查“站内无此书”特征
        final_url = step_fetch.get("url") or ""
        method = (fetch_detail.get("method") if isinstance(fetch_detail, dict) else None) or "GET"
        post_body = None
        if isinstance(step_url.get("detail"), dict):
            post_body = step_url["detail"].get("body") or None
        if post_body:
            post_body = post_body.replace("{{key}}", KEYWORD).replace("{{page}}", "1") \
                                 .replace("{key}", KEYWORD).replace("{page}", "1")
        replayable = host_of_url(final_url) is not None
        marker = None
        body_size = None
        if replayable:
            hp2 = host_of_url(final_url)
            if hp2 and not self.ratelimit.acquire(hp2[0], timeout=45):
                hp2 = None
            try:
                rst, rbody, rerr = http_request(final_url, method, post_body, REPLAY_TIMEOUT)
                if rerr is None and rst is not None and rst < 400:
                    body_size = len(rbody)
                    marker = find_no_result_marker(rbody)
            finally:
                if hp2:
                    self.ratelimit.release(hp2[0])
        rec["bodySize"] = body_size
        rec["marker"] = marker
        if marker:
            rec["classification"] = "normal"
            rec["errorType"] = None
            rec["note"] = f"明确站内无此书（响应体命中特征「{marker}」）"
        else:
            rec["classification"] = "rule_engine_error"
            rec["errorType"] = "zero_results"
            why = "重放响应体未见无结果特征" if body_size is not None else \
                  ("重放失败" if replayable else "最终 URL 非 http(s)（JS/POST 构造，无法重放）")
            rec["errorDetail"] = (f"HTTP {rec['status']} 但搜索 0 结果；{why}"
                                  f"（bodySize={body_size}）")
        rec["elapsedMs"] = int((time.time() - t0) * 1000)
        return rec


# ---------------------------------------------------------------- 报告
def build_summary(recs):
    by_cls = {}
    for r in recs:
        by_cls[r["classification"]] = by_cls.get(r["classification"], 0) + 1
    engine = [r for r in recs if r["classification"] == "rule_engine_error"]
    down = [r for r in recs if r["classification"] == "site_down"]
    by_etype = {}
    for r in engine:
        t = r["errorType"] or "other"
        by_etype[t] = by_etype.get(t, 0) + 1
    down_by_type = {}
    for r in down:
        t = r["errorType"] or "network"
        down_by_type[t] = down_by_type.get(t, 0) + 1
    examples = {}
    for t in by_etype:
        examples[t] = [f"{r['name']}（{r['url'][:60]}）"
                       for r in engine if (r["errorType"] or "other") == t][:3]
    return {
        "total": len(recs),
        "site_down": by_cls.get("site_down", 0),
        "rule_engine_error": by_cls.get("rule_engine_error", 0),
        "normal": by_cls.get("normal", 0),
        "no_search_url": by_cls.get("no_search_url", 0),
        "audit_error": by_cls.get("audit_error", 0),
        "siteDownByType": dict(sorted(down_by_type.items(), key=lambda x: -x[1])),
        "engineErrorsByType": dict(sorted(by_etype.items(), key=lambda x: -x[1])),
        "engineErrorExamples": examples,
    }


def write_md(meta, summary, recs):
    L = ["# 书源批量检测审计报告", "",
         f"- 生成时间: {meta['generatedAt']}",
         f"- 测试库: {meta['db']}（共 {meta['totalSources']} 个书源）",
         f"- 关键词: 「{meta['keyword']}」 | 并发: {meta['concurrency']} | 探测超时: {meta['probeTimeoutSecs']}s",
         f"- 审计方式: 本地服务 {meta['base']} bookSourceDebugSSE（生产规则引擎逐步执行）+ 可达性探测", "",
         "## 总览", "",
         f"| 分类 | 数量 | 占比 |", "|---|---|---|",
         f"| 正常（有结果/站内无此书） | {summary['normal']} | {summary['normal']/max(summary['total'],1)*100:.1f}% |",
         f"| 站点挂了（网络/HTTP 错误） | {summary['site_down']} | {summary['site_down']/max(summary['total'],1)*100:.1f}% |",
         f"| 规则/引擎问题 | {summary['rule_engine_error']} | {summary['rule_engine_error']/max(summary['total'],1)*100:.1f}% |",
         f"| 未配置 searchUrl（旁路） | {summary['no_search_url']} | - |",
         f"| 审计链路异常（旁路） | {summary['audit_error']} | - |", "",
         "## 站点挂了明细（按错误类型）", "",
         "| 类型 | 数量 |", "|---|---|"]
    for k, v in summary["siteDownByType"].items():
        L.append(f"| {k} | {v} |")
    L += ["", "## 规则/引擎问题明细（按错误类型 + 示例源）", "",
          "| 错误类型 | 数量 | 示例源（前 3） |", "|---|---|---|"]
    for k, v in summary["engineErrorsByType"].items():
        ex = "；".join(summary["engineErrorExamples"].get(k, []))
        L.append(f"| {k} | {v} | {ex} |")
    L += ["", "## 说明", "",
          "- 分类口径：站点挂了 = DNS/连接/超时/TLS/重定向环（HTTP 层不可达）或 HTTP 4xx/5xx（搜索端点不可用）；",
          "  规则/引擎问题 = HTTP 正常但 0 结果（重放未见无结果特征）/解析异常/JS 报错（css/jsonpath/xpath/regex/js/zero_results/other）；",
          "  正常 = 有结果或明确“站内无此书”（响应体命中无结果特征标记）。",
          "- 本审计只读：未修改/未删除/未禁用任何书源。",
          "- 完整逐源明细见 source-audit-report.json。", ""]
    Path(REPORT_MD).write_text("\n".join(L), encoding="utf-8")


# ---------------------------------------------------------------- 主流程
def main():
    global KEYWORD
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", default=str(DEFAULT_DB))
    ap.add_argument("--base", default=os_env("READER_AUDIT_BASE", DEFAULT_BASE))
    ap.add_argument("--workers", type=int, default=WORKERS_DEFAULT)
    ap.add_argument("--limit", type=int, default=0, help="仅审计前 N 个（调试）")
    ap.add_argument("--only", default="", help="仅审计指定 url（逗号分隔）")
    ap.add_argument("--resume", action="store_true", help="续跑：跳过报告 JSON 中已有记录")
    ap.add_argument("--keyword", default=KEYWORD)
    args = ap.parse_args()
    KEYWORD = args.keyword

    db_path = Path(args.db)
    if not db_path.exists():
        sys.exit(f"数据库不存在: {db_path}")
    conn = sqlite3.connect(str(db_path), timeout=30)
    conn.row_factory = sqlite3.Row
    rows = conn.execute(
        "SELECT book_source_url, book_source_name, user_namespace, enabled, search_url "
        "FROM book_sources ORDER BY custom_order, book_source_name").fetchall()
    conn.close()
    print(f"书源总数: {len(rows)}（DB: {db_path}）")

    if args.only:
        wanted = set(x.strip() for x in args.only.split(",") if x.strip())
        rows = [r for r in rows if r["book_source_url"] in wanted]
        print(f"--only 过滤后: {len(rows)}")
    if args.limit and args.limit > 0:
        rows = rows[:args.limit]
        print(f"--limit 截取: {len(rows)}")

    prev = {}
    if args.resume and REPORT_JSON.exists():
        prev = {r["url"]: r for r in json.loads(REPORT_JSON.read_text(encoding="utf-8")).get("sources", [])}
        todo = [r for r in rows if r["book_source_url"] not in prev]
        print(f"--resume: 已有 {len(prev)} 条，待审计 {len(todo)} 条")
        rows = todo
    if not rows:
        print("无可审计书源。")
        return

    audit = Audit(args)
    results = []
    t_start = time.time()
    done = 0
    lock = threading.Lock()
    dead = [False]

    def work(row):
        r = audit.audit_one(dict(row))
        return r

    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = {ex.submit(work, r): r["book_source_url"] for r in rows}
        for fut in as_completed(futs):
            url = futs[fut]
            try:
                rec = fut.result()
            except Exception as e:
                rec = {"url": url, "name": url, "classification": "audit_error",
                       "errorType": "exception", "errorDetail": str(e)[:300]}
            with lock:
                results.append(rec)
                done += 1
                if done % 25 == 0 or done == len(rows):
                    el = time.time() - t_start
                    rate = done / max(el, 0.001)
                    print(f"  进度 {done}/{len(rows)}  用时 {el:.0f}s（{rate:.2f} 源/s）")
            if not audit.server_ok and not dead[0]:
                dead[0] = True
                print("!! 本地服务不可用（连接被拒），停止提交新任务")
                for f in futs:
                    f.cancel()
                break

    if args.resume and prev:
        results = list(prev.values()) + results
    # 排序：按分类分组（正常 / 站点挂了 / 规则引擎 / 旁路），组内按耗时
    order = {"normal": 0, "site_down": 1, "rule_engine_error": 2, "no_search_url": 3, "audit_error": 4}
    results.sort(key=lambda r: (order.get(r.get("classification"), 9), r.get("elapsedMs") or 0))

    summary = build_summary(results)
    meta = {
        "generatedAt": time.strftime("%Y-%m-%d %H:%M:%S"),
        "db": str(db_path), "base": args.base, "keyword": KEYWORD,
        "totalSources": len(results), "concurrency": args.workers,
        "probeTimeoutSecs": PROBE_TIMEOUT, "sseTimeoutSecs": SSE_TIMEOUT,
        "classificationNote": (
            "site_down=站点挂了（DNS/连接/超时/TLS/重定向环，或 HTTP 4xx/5xx 搜索端点不可用）；"
            "rule_engine_error=规则/引擎问题（HTTP 正常但 0 结果或解析异常或 JS 报错；错误类型 css/jsonpath/xpath/regex/js/zero_results/other）；"
            "normal=有结果或明确站内无此书；no_search_url/audit_error=旁路桶"),
    }
    report = {"meta": meta, "summary": summary, "sources": results}
    REPORT_JSON.write_text(json.dumps(report, ensure_ascii=False, indent=1), encoding="utf-8")
    write_md(meta, summary, results)

    print("\n" + "=" * 70)
    print(f"审计完成：{len(results)} 源，总用时 {time.time()-t_start:.0f}s")
    for k in ("normal", "site_down", "rule_engine_error", "no_search_url", "audit_error"):
        print(f"  {k}: {summary[k]}")
    print("  站点挂了明细:", summary["siteDownByType"])
    print("  引擎问题明细:", summary["engineErrorsByType"])
    print("=" * 70)
    print(f"报告: {REPORT_JSON}")
    print(f"报告: {REPORT_MD}")


def os_env(k, d):
    import os
    return os.environ.get(k, d)


if __name__ == "__main__":
    try:
        sys.stdout.reconfigure(encoding="utf-8")
    except Exception:
        pass
    main()
