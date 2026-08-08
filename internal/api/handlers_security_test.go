package api

import (
	"net/http"
	"net/url"
	"strings"
	"testing"
)

// TestAssetsProxySSRF 图片代理：内网/回环 URL 应被拒绝（不再裸 http.Get）。
func TestAssetsProxySSRF(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "0")
	h := newTestAPI(t)
	w := perform(h, "GET", "/assets/proxy?url="+url.QueryEscape("http://127.0.0.1:9999/a.jpg"), nil)
	if w.Code != http.StatusOK {
		t.Fatalf("HTTP %d", w.Code)
	}
	if strings.Contains(w.Body.String(), "isSuccess\":true") {
		t.Fatalf("图片代理应拒绝内网 URL: %s", w.Body.String())
	}
	if !strings.Contains(w.Body.String(), "图片地址非法") {
		t.Errorf("错误信息应含拒绝提示: %s", w.Body.String())
	}
	// 公网 http/https 校验保留
	w2 := perform(h, "GET", "/assets/proxy?url=relative.jpg", nil)
	if !strings.Contains(w2.Body.String(), "图片地址非法") {
		t.Errorf("非 http(s) 应拒绝: %s", w2.Body.String())
	}
}
