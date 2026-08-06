#!/usr/bin/env python3
"""Turnstile 验证码 mock 站点（集成测试用）——默认 HTTP 8194

浏览器后端无关设计（Chrome/Edge 与 obscura 均可用）：
- Turnstile 页 503 + 特征（.cf-turnstile 容器 / 隐藏 input[name=cf-turnstile-response]
  / title "Verifying..." / challenges.cloudflare.com 脚本标签）
- 内联脚本**同步移除真实 api.js 脚本标签**——避免真实 widget 异步重建 .cf-turnstile
  容器（容器监听器失效）；mock 只验证我们的「点击容器 → 轮询 token」机制，
  真实 widget 链路由 turnstile_real_widget_local_page（always-passes sitekey）覆盖
- 点击容器（同步监听器）→ form.submit() → /solve?token=（不用 JS 定时器/fetch——
  obscura 只同步执行 CDP 驱动的 JS）→ 服务端 **Set-Cookie 头** 写
  cf_clearance=mock-<token>（真实 CF 语义；obscura 的 JS document.cookie 写入不落
  jar）→ 302 → /content?token= → 内容页回显 token + cookie（服务端按请求头判定）
  并内嵌 `<input name="cf-turnstile-response" value="<token>">`（求解循环轮询命中）
- GET /status → {"ok": true}（测试探活）
"""
import argparse
import http.server
import urllib.parse

TURNSTILE_HTML = """<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8">
<title>Verifying...</title>
<script src="https://challenges.cloudflare.com/turnstile/v0/api.js" async defer></script>
</head>
<body>
<div id="challenge-wrap">
  <div class="cf-turnstile" data-sitekey="0x4AAAAAAA-mockkey" data-callback="onTurnstileSuccess"></div>
  <input type="hidden" name="cf-turnstile-response" id="cf-turnstile-response" value="">
</div>
<script>
(function(){
  // 同步移除真实 api.js（async defer 尚未加载）——mock 确定性：
  // 容器不被真实 widget 重建，点击监听器始终有效
  var s = document.querySelector('script[src*="challenges.cloudflare.com/turnstile"]');
  if (s && s.parentNode) s.parentNode.removeChild(s);
  var el = document.querySelector('.cf-turnstile');
  if (el) {
    el.addEventListener('click', function(){
      // 同步 form 提交 → /solve（服务端 Set-Cookie + 302 → 内容页）。
      // 不用 JS 定时器/fetch——obscura 不触发页面异步执行
      var token = 'mock-turnstile-' + Date.now();
      var f = document.createElement('form');
      f.method = 'GET';
      f.action = '/solve?token=' + encodeURIComponent(token);
      document.body.appendChild(f);
      f.submit();
    });
  }
})();
</script>
</body>
</html>
"""

CONTENT_HTML = """<!DOCTYPE html>
<html lang="zh">
<head><meta charset="utf-8"><title>真实内容</title></head>
<body>
<h1 id="content-marker">真实内容页</h1>
<p>收到 cookie: {cookies}</p>
<p id="token-echo">{token}</p>
<input type="hidden" name="cf-turnstile-response" id="cf-turnstile-response" value="{token}">
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
        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path
        if path == "/status":
            self._send(200, '{"ok": true}', "application/json")
        elif path == "/solve":
            # Set-Cookie 头写 cf_clearance + 302 → 内容页（真实 CF 语义）
            token = urllib.parse.parse_qs(parsed.query).get(
                "token", ["mock-turnstile-0"]
            )[0]
            self._send(
                302,
                "",
                "text/plain",
                [
                    ("Set-Cookie", f"cf_clearance=mock-{token}; Path=/; SameSite=Lax"),
                    ("Location", f"/content?token={urllib.parse.quote(token)}"),
                ],
            )
        elif path == "/content":
            token = urllib.parse.parse_qs(parsed.query).get("token", [""])[0]
            cookies = self.headers.get("Cookie", "")
            self._send(
                200,
                CONTENT_HTML.format(cookies=cookies or "(none)", token=token),
            )
        else:
            # Turnstile 页：503 + 特征（is_cloudflare_challenge 与浏览器检测均命中）
            self._send(503, TURNSTILE_HTML)

    def do_POST(self):
        self.do_GET()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8194)
    ap.add_argument("--host", default="127.0.0.1")
    args = ap.parse_args()
    server = http.server.ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"READER_MOCK_TURNSTILE listening on {args.host}:{args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
