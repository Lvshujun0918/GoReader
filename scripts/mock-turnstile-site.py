#!/usr/bin/env python3
"""Turnstile 验证码 mock 站点（集成测试用）——默认 HTTP 8194

模拟 Cloudflare Turnstile widget 页面（与真实 Turnstile 结构对齐，供 crawler 检测与
浏览器求解测试——点击容器 → 轮询 [name=cf-turnstile-response] → token）：

- GET /        → 503 + Turnstile 特征页（.cf-turnstile 容器 + data-sitekey + 隐藏
                 input[name=cf-turnstile-response] + challenges.cloudflare.com/turnstile
                 脚本标签 + title "Verifying..."）；内嵌 JS：点击容器 1.5 秒后填
                 token=mock-turnstile-<ts> 并写 cookie（cf_clearance=mock-<token>），
                 原地切换为内容页（input 保留在 DOM 中——轮询可读 token）
- GET /status  → {"ok": true}（测试探活）
- POST 与 GET 同路径同处理（http_post 链路测试兼容）

用法：python scripts/mock-turnstile-site.py [--port 8194] [--host 127.0.0.1]
"""
import argparse
import http.server
import time

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
<div id="content-area" style="display:none">
  <h1 id="content-marker">真实内容页</h1>
  <p>收到 cookie: <span id="cookie-echo"></span></p>
  <p id="token-echo"></p>
</div>
<script>
(function(){
  // 模拟 Turnstile 回调：点击容器 1.5 秒后填 token 并写 cookie（与真实 widget 回调语义一致）
  document.querySelector('.cf-turnstile').addEventListener('click', function(){
    setTimeout(function(){
      var token = 'mock-turnstile-' + Date.now();
      document.querySelector('[name="cf-turnstile-response"]').value = token;
      document.cookie = 'cf_clearance=mock-' + token + '; path=/; SameSite=Lax';
      document.getElementById('cookie-echo').textContent = document.cookie;
      document.getElementById('token-echo').textContent = token;
      document.getElementById('challenge-wrap').style.display = 'none';
      document.getElementById('content-area').style.display = 'block';
      document.title = '真实内容';
    }, 1500);
  });
})();
</script>
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
        else:
            # Turnstile 页：503 + 特征（is_cloudflare_challenge 与浏览器检测均命中）
            self._send(503, TURNSTILE_HTML.replace("{TS}", str(int(time.time()))))

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
