#!/usr/bin/env python3
"""CF 质询 mock 站点（集成测试用）——默认 HTTP 8193

模拟 Cloudflare 质询流程（与真实 CF 特征对齐，供 crawler 检测与浏览器求解测试）：

- GET /        → 503 + CF 特征质询页（title="Just a moment"、#challenge-form、
                 #challenge-running、challenge-platform 脚本标记）；
                 内嵌 JS：2 秒后 document.cookie 写入 cf_clearance=mock-<ts>（path=/）
                 并 location.href='/content' 跳转真实内容
- GET /content → 真实内容页（title="真实内容"），回显收到的 Cookie 头（证明
                 cf_clearance 已随跳转携带），含 CF_OK / CF_MISSING 标记
- GET/POST /search → 搜索接口（POST 重试链路测试）：Cookie 含 cf_clearance=mock- →
                 200 搜索结果页（SEARCH_OK + 关键词回显）；否则 → 503 质询页
- GET /status  → {"ok": true}（测试探活）
- POST 与 GET 同路径同处理（http_post 链路测试兼容）

用法：python scripts/mock-cf-site.py [--port 8193] [--host 127.0.0.1]
"""
import argparse
import http.server
import time

CHALLENGE_HTML = """<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8">
<title>Just a moment...</title>
<script src="/cdn-cgi/challenge-platform/h/g/orchestrate/jsch/v1"></script>
<script>
(function(){
  // 模拟 CF 质询求解：2 秒后写入 cf_clearance 并跳转真实内容
  setTimeout(function(){
    document.cookie = "cf_clearance=mock-{TS}; path=/; SameSite=Lax";
    location.href = "/content";
  }, 2000);
})();
</script>
</head>
<body>
<form id="challenge-form" class="challenge-form">
  <input type="hidden" name="md" value="mock">
  <div id="challenge-running">Just a moment...</div>
</form>
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

    def _send(self, status, body, ctype="text/html; charset=utf-8"):
        data = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/status":
            self._send(200, '{"ok": true}', "application/json")
        elif self.path == "/content":
            cookies = self.headers.get("Cookie", "")
            cf_echo = "CF_OK" if "cf_clearance=mock-" in cookies else "CF_MISSING"
            self._send(200, CONTENT_HTML.format(cookies=cookies, cf_echo=cf_echo))
        elif self.path == "/search":
            # 搜索接口（POST 重试链路测试）：带 cf_clearance → 真实搜索结果；否则质询
            cookies = self.headers.get("Cookie", "")
            if "cf_clearance=mock-" in cookies:
                key = self.req_body if self.command == "POST" else ""
                self._send(200, SEARCH_HTML.format(key=key, cookies=cookies))
            else:
                self._send(503, CHALLENGE_HTML.replace("{TS}", str(int(time.time()))))
        else:
            # 质询页：503 + CF 特征（is_cloudflare_challenge 与浏览器检测均命中）
            self._send(503, CHALLENGE_HTML.replace("{TS}", str(int(time.time()))))

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
