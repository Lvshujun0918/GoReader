#!/usr/bin/env python3
"""CF 质询 mock 站点（集成测试用）——默认 HTTP 8193

浏览器后端无关设计（Chrome/Edge 与 obscura 均可用）：
- 质询页 503 + CF 特征（#challenge-form / title "Just a moment" / jsch 脚本）
- 页面内联脚本**同步 form.submit()** → /solve（不用 JS 定时器/fetch——obscura
  只同步执行 CDP 驱动的 JS，定时器与异步 fetch 不触发；form 提交导航是其支持的
  同步机制）→ 服务端 **Set-Cookie 头** 写 cf_clearance=mock-<ts>（真实 CF 语义；
  obscura 的 JS document.cookie 写入不落 jar）→ 302 → /content
- /content 按请求 Cookie 头回显 CF_OK/CF_MISSING（证明 cookie 已随请求携带——
  真实 CF 的 cf_clearance 随后续请求生效）
- GET /status → {"ok": true}（测试探活）
- GET/POST /search → 带 cf_clearance → 搜索结果页；否则 503 质询页
  （POST 重试链路测试）
- 默认分支：带 cf_clearance → 200 真实内容页（重试链路）；否则 503 质询页
"""
import argparse
import http.server
import time
import urllib.parse

CHALLENGE_HTML = """<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8">
<title>Just a moment...</title>
<script src="/cdn-cgi/challenge-platform/h/g/orchestrate/jsch/v1"></script>
<script>
(function(){
  // 模拟 CF 质询求解：同步 form 提交 → /solve（服务端 Set-Cookie + 302 → /content）。
  // 不用 JS 定时器/fetch——obscura 不触发页面加载期异步执行
  var f = document.createElement('form');
  f.method = 'GET';
  f.action = '/solve?ts=' + Date.now();
  document.body.appendChild(f);
  f.submit();
})();
</script>
</head>
<body>
<div id="challenge-wrap">
  <form id="challenge-form" class="challenge-form">
    <input type="hidden" name="md" value="mock">
    <div id="challenge-running">Just a moment...</div>
  </form>
</div>
</body>
</html>
"""

CONTENT_HTML = """<!DOCTYPE html>
<html lang="zh">
<head><meta charset="utf-8"><title>真实内容</title></head>
<body>
<h1 id="content-marker">真实内容页</h1>
<p>收到 cookie: {cookies}</p>
<p id="cf-echo">{cf_echo}</p>
</body>
</html>
"""

SEARCH_HTML = """<!DOCTYPE html>
<html lang="zh">
<head><meta charset="utf-8"><title>搜索结果</title></head>
<body>
<h1 id="search-marker">SEARCH_OK</h1>
<p>关键词: {key}</p>
<p>收到 cookie: {cookies}</p>
<ul><li><a href="/book/1">《{key}》第一卷 第一章</a></li></ul>
</body>
</html>
"""


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):  # 静默日志
        pass

    def _send(self, status, body, ctype="text/html; charset=utf-8", extra_headers=None):
        data = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        for k, v in (extra_headers or []):
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path = urllib.parse.urlparse(self.path).path
        cookies = self.headers.get("Cookie", "")
        if path == "/status":
            self._send(200, '{"ok": true}', "application/json")
        elif path == "/solve":
            # Set-Cookie 头写 cf_clearance + 302 → /content（真实 CF 语义）
            ts = urllib.parse.parse_qs(
                urllib.parse.urlparse(self.path).query
            ).get("ts", ["0"])[0]
            self._send(
                302,
                "",
                "text/plain",
                [
                    ("Set-Cookie", f"cf_clearance=mock-{ts}; Path=/; SameSite=Lax"),
                    ("Location", "/content"),
                ],
            )
        elif path == "/content":
            cf_echo = "CF_OK" if "cf_clearance=mock-" in cookies else "CF_MISSING"
            self._send(200, CONTENT_HTML.format(cookies=cookies, cf_echo=cf_echo))
        elif path == "/search":
            # 搜索接口（POST 重试链路测试）：带 cf_clearance → 真实搜索结果；否则质询
            if "cf_clearance=mock-" in cookies:
                key = self.req_body if self.command == "POST" else ""
                self._send(200, SEARCH_HTML.format(key=key, cookies=cookies))
            else:
                self._send(503, CHALLENGE_HTML)
        elif path.startswith("/cdn-cgi/"):
            # jsch 脚本 200（避免 404 干扰页面脚本阶段）
            self._send(200, "/* mock jsch */", "application/javascript")
        else:
            # 质询页：503 + CF 特征（is_cloudflare_challenge 与浏览器检测均命中）；
            # 重试（带 cf_clearance）→ 真实内容（重试链路测试）
            if "cf_clearance=mock-" in cookies:
                cf_echo = "CF_OK"
                self._send(200, CONTENT_HTML.format(cookies=cookies, cf_echo=cf_echo))
            else:
                self._send(503, CHALLENGE_HTML)

    def do_POST(self):
        # 读取请求体（搜索/表单场景——重试链路需回显 searchkey）
        length = int(self.headers.get("Content-Length", 0) or 0)
        self.req_body = (
            self.rfile.read(length).decode("utf-8", "replace") if length else ""
        )
        self.do_GET()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8193)
    ap.add_argument("--host", default="127.0.0.1")
    args = ap.parse_args()
    server = http.server.ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"READER_MOCK_CF listening on {args.host}:{args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
