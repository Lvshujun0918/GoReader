package api

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

// TestFetchRssRejectsPrivate RSS 抓取经 crawler：内网/回环 URL 应被 SSRF 拒绝。
func TestFetchRssRejectsPrivate(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "0")
	// 未设 env（默认拒绝内网）
	if _, err := fetchRss("http://127.0.0.1:9999/feed.xml"); err == nil {
		t.Fatal("RSS 抓取应拒绝回环地址（SSRF 防护）")
	}
	// 相对 URL（无 hostname）同样拒绝
	if _, err := fetchRss("/feed.xml"); err == nil {
		t.Fatal("RSS 抓取应拒绝相对 URL")
	}
}

// TestAssetsProxySSRF 图片代理经 crawler：内网 URL 应被拒绝（不再裸 http.Get）。
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
	if !strings.Contains(w.Body.String(), "禁止访问内网") {
		t.Errorf("错误信息应含 SSRF 提示: %s", w.Body.String())
	}
	// 公网 http/https 校验保留
	w2 := perform(h, "GET", "/assets/proxy?url=relative.jpg", nil)
	if !strings.Contains(w2.Body.String(), "图片地址非法") {
		t.Errorf("非 http(s) 应拒绝: %s", w2.Body.String())
	}
}

// TestLoginBookSourceRelativeURL 书源登录相对 URL 应 resolve 到书源根（不报 SSRF 误判）。
func TestLoginBookSourceRelativeURL(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	h := newTestAPI(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/login.php" {
			t.Errorf("登录请求路径=%q", r.URL.Path)
		}
		w.WriteHeader(http.StatusOK)
	}))
	defer srv.Close()
	src := map[string]any{
		"bookSourceUrl": srv.URL, "bookSourceName": "登录源",
		"loginUrl": "/login.php",
	}
	saveOneSource(t, h, src)
	w := perform(h, "POST", "/reader3/loginBookSource", map[string]any{
		"bookSourceUrl": srv.URL, "username": "u", "password": "p",
	})
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("登录应成功（相对 URL resolve 到书源根）: %v", rd.ErrorMsg)
	}
}

// TestImportBookSourcesLooseJSON 书源合集导入支持尾随逗号（宽松 JSON）。
func TestImportBookSourcesLooseJSON(t *testing.T) {
	h := newTestAPI(t)
	// 书源数组带尾随逗号（[a,b,]，标准 json.Unmarshal 会失败）
	loose := `[
		{"bookSourceUrl":"https://src1.com","bookSourceName":"源一","bookSourceType":0,"enabled":true},
		{"bookSourceUrl":"https://src2.com","bookSourceName":"源二","bookSourceType":0,"enabled":true},
	]`
	w := perform(h, "POST", "/reader3/saveBookSources", []byte(loose))
	// perform 的 body 是 []byte → 直接传（不 JSON 序列化）
	_ = w
	// 手动构造请求（[]byte body 用 httptest 直接发）
	req := httptest.NewRequest("POST", "/reader3/saveBookSources", strings.NewReader(loose))
	req.Header.Set("Content-Type", "application/json")
	w2 := httptest.NewRecorder()
	h.ServeHTTP(w2, req)
	rd := parseReturn(t, w2)
	if !rd.IsSuccess {
		t.Fatalf("宽松 JSON 导入应成功: %v (%s)", rd.ErrorMsg, w2.Body.String())
	}
	if n := dataCount(t, rd); n != 2 {
		t.Fatalf("应导入 2 个书源，实际 %d", n)
	}
}

var _ = json.Marshal
var _ = fmt.Sprint
