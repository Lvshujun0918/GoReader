#!/usr/bin/env python3
"""滑块验证码 mock 站点（集成测试用）——默认 HTTP 8195

真实滑块站（geetest 等）难以稳定自动化获取，故用 mock 验证拖拽机制（mock 依据：
DETECT_CAPTCHA_JS 的滑块选择器与 solve_captcha 的贝塞尔拖拽路径——与登录流程同一套
拖拽代码）。页面结构对齐 DETECT_CAPTCHA_JS 识别规则：

- 按钮 class="drag-slider"（命中 '.drag-slider' 选择器——DETECT_CAPTCHA_JS 返回其 rect）
- 轨道 class="slider-track"（祖先加宽匹配 /slider/ 正则——返回 trackX/trackW）
- 内嵌 JS：mousedown 起拖 → mousemove 跟随 → mouseup 时位移 ≥ 轨道 40% 即通过
  （mock 阈值放宽——验证拖拽机制而非风控评分；真实滑块需更精准）→ 写
  cf_clearance=mock-slider-<ts> cookie 并原地切换内容页
- GET /status → {"ok": true}（测试探活）

用法：python scripts/mock-slider-site.py [--port 8195] [--host 127.0.0.1]
"""
import argparse
import http.server

SLIDER_HTML = """<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8">
<title>滑块验证（mock）</title>
<style>
#slider-track { width: 320px; height: 44px; border: 1px solid #ccc; background: #f5f5f5;
                position: relative; border-radius: 4px; }
.drag-slider { width: 44px; height: 42px; background: #4a90d9; color: #fff; text-align: center;
               line-height: 42px; position: absolute; left: 0; top: 0; border-radius: 4px;
               cursor: pointer; user-select: none; }
</style>
</head>
<body>
<div id="challenge-wrap">
  <p id="tip">拖动滑块到最右端</p>
  <div id="slider-track" class="slider-track">
    <div class="drag-slider" id="slider-btn">→</div>
  </div>
</div>
<div id="content-area" style="display:none">
  <h1 id="content-marker">真实内容页</h1>
  <p>收到 cookie: <span id="cookie-echo"></span></p>
  <p id="success-echo">SLIDER_OK</p>
</div>
<script>
(function(){
  var track = document.getElementById('slider-track');
  var btn = document.getElementById('slider-btn');
  var dragging = false, startX = 0, startBtn = 0;
  btn.addEventListener('mousedown', function(e){
    dragging = true; startX = e.clientX; startBtn = btn.offsetLeft;
  });
  document.addEventListener('mousemove', function(e){
    if (!dragging) return;
    var max = track.clientWidth - btn.clientWidth;
    var left = Math.min(Math.max(startBtn + (e.clientX - startX), 0), max);
    btn.style.left = left + 'px';
  });
  document.addEventListener('mouseup', function(){
    if (!dragging) return;
    dragging = false;
    var max = track.clientWidth - btn.clientWidth;
    if (btn.offsetLeft >= max * 0.4) {
      var ts = Date.now();
      document.cookie = 'cf_clearance=mock-slider-' + ts + '; path=/; SameSite=Lax';
      document.getElementById('cookie-echo').textContent = document.cookie;
      document.getElementById('challenge-wrap').style.display = 'none';
      document.getElementById('content-area').style.display = 'block';
      document.title = '真实内容';
    } else {
      btn.style.left = '0px';
      document.getElementById('tip').textContent = '距离不足，请重试';
    }
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
            self._send(200, SLIDER_HTML)

    def do_POST(self):
        self.do_GET()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8195)
    ap.add_argument("--host", default="127.0.0.1")
    args = ap.parse_args()
    server = http.server.ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"READER_MOCK_SLIDER listening on {args.host}:{args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
