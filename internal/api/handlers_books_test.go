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

// kuwoFullServer 模拟酷我完整 API（真实格式 code:200+data）：搜索/详情/目录/正文。
func kuwoFullServer(t *testing.T) *httptest.Server {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		p := r.URL.Path
		var body string
		switch {
		case strings.Contains(p, "/search"):
			body = `{"code":200,"data":[{"book_id":"2237","title":"庆余年","author_name":"猫腻","all_words":"100万","cover_url":"/c.jpg"}]}`
		case strings.Contains(p, "/chapters/"):
			body = `{"code":200,"data":{"content":"第一章正文内容。\r\n第二段。"}}`
		case strings.Contains(p, "/chapters"):
			// 按 pi 分页返回（验证 BookToc JSON 自动翻页）
			pi := r.URL.Query().Get("pi")
			if pi == "" || pi == "1" {
				body = `{"code":200,"data":[{"book_id":"2237","chapter_id":"c1","chapter_title":"第一章 风起"},{"book_id":"2237","chapter_id":"c2","chapter_title":"第二章 云涌"}],"paging":{"count":4,"ps":2,"pi":1}}`
			} else if pi == "2" {
				body = `{"code":200,"data":[{"book_id":"2237","chapter_id":"c3","chapter_title":"第三章 暗涌"},{"book_id":"2237","chapter_id":"c4","chapter_title":"第四章 裂变"}],"paging":{"count":4,"ps":2,"pi":2}}`
			} else {
				body = `{"code":200,"data":[],"paging":{"count":4,"ps":2,"pi":2}}`
			}
		default:
			body = `{"code":200,"data":{"book_id":"2237","title":"庆余年","author_name":"猫腻","intro":"积善之家","status":50,"all_words":"100万","cover_url":"/c.jpg"}}`
		}
		_, _ = fmt.Fprint(w, body)
	}))
	t.Cleanup(srv.Close)
	return srv
}

// saveKuwoSource 保存真实酷我格式书源（bookSourceUrl 指向 mock）。
func saveKuwoSource(t *testing.T, h http.Handler, srvURL string) {
	t.Helper()
	src := map[string]any{
		"bookSourceUrl":  srvURL,
		"bookSourceName": "酷我小说",
		"bookSourceType": 0,
		"searchUrl":      "/novels/api/book/search?keyword={{key}}&pi={{page}}&ps=30",
		"ruleSearch": map[string]any{
			"author": "$.author_name", "bookList": "$.data", "bookUrl": "/novels/api/book/{{$.book_id}}",
			"coverUrl": "$.cover_url", "intro": "$.intro", "name": "$.title", "wordCount": "$.all_words",
		},
		"ruleBookInfo": map[string]any{
			"author": "$.author_name", "coverUrl": "$.cover_url", "init": "$.data",
			"intro": "$.intro", "kind": "{{$.category_name}},{{$.status}}@js:result.replace(/30/,\"连载\").replace(/50/,\"完结\")",
			"name": "$.title", "tocUrl": "/novels/api/book/{{$.book_id}}/chapters?paging=0", "wordCount": "$.all_words",
		},
		"ruleToc": map[string]any{
			"chapterList": "$.data", "chapterName": "$.chapter_title", "chapterUrl": "/novels/api/book/{{$.book_id}}/chapters/{{$.chapter_id}}",
		},
		"ruleContent": map[string]any{"content": "$.data.content"},
	}
	saveOneSource(t, h, src)
}

// TestFullChainSearchShelfToc 全链路：搜索"庆余年"→ 加入书架（tocUrl 用搜索结果=详情 URL）→
// getBookInfo 书架分支实时重算 tocUrl → getBookToc 返回全部章节（非 1 章）。
func TestFullChainSearchShelfToc(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	h := newTestAPI(t)
	srv := kuwoFullServer(t)
	saveKuwoSource(t, h, srv.URL)

	// 1. 搜索（与前端 searchBookMulti 同参数）
	w := perform(h, "POST", "/reader3/searchBookMulti", map[string]any{"key": "庆余年", "maxSources": 1, "page": 1})
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("搜索失败: %v", rd.ErrorMsg)
	}
	arr, _ := rd.Data.([]any)
	if len(arr) == 0 {
		t.Fatal("搜索无结果")
	}
	book, _ := arr[0].(map[string]any)
	bookURL, _ := book["bookUrl"].(string)
	bookToc, _ := book["tocUrl"].(string)
	if bookURL != srv.URL+"/novels/api/book/2237" {
		t.Fatalf("搜索结果 bookUrl=%q", bookURL)
	}
	// 搜索结果 tocUrl 与 bookUrl 相同（详情 URL）——正是用户"新添加的书只有一章"的入口
	if bookToc != bookURL {
		t.Errorf("搜索 tocUrl 应为 bookUrl（详情）: %q", bookToc)
	}

	// 2. 加入书架（前端用搜索结果的 tocUrl）
	w = perform(h, "POST", "/reader3/saveBook", map[string]any{
		"bookUrl": bookURL, "name": "庆余年", "author": "猫腻",
		"origin": srv.URL, "originName": "酷我小说", "tocUrl": bookToc, "group": 0,
	})
	rd = parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("加入书架失败: %v", rd.ErrorMsg)
	}

	// 3. getBookInfo（书架分支：应实时重算 tocUrl，而非返回书架里的详情 URL）
	w = perform(h, "GET", "/reader3/getBookInfo?url="+url.QueryEscape(bookURL)+"&bookSource="+srv.URL, nil)
	rd = parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("getBookInfo 失败: %v", rd.ErrorMsg)
	}
	info, _ := rd.Data.(map[string]any)
	toc, _ := info["tocUrl"].(string)
	wantToc := srv.URL + "/novels/api/book/2237/chapters?paging=0"
	if toc != wantToc {
		t.Fatalf("getBookInfo tocUrl=%q 期望 %q（书架分支应实时重算）", toc, wantToc)
	}

	// 4. getBookToc：全部章节（非 1 章）
	w = perform(h, "GET", "/reader3/getBookToc?tocUrl="+url.QueryEscape(toc)+"&bookSource="+srv.URL, nil)
	rd = parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("getBookToc 失败: %v", rd.ErrorMsg)
	}
	chapters, _ := rd.Data.([]any)
	// JSON 分页自动翻页：pi=1（2 章）+ pi=2（2 章）= 4 章，非 1 章
	if len(chapters) != 4 {
		t.Fatalf("章节数=%d 期望 4（分页自动翻页，非 1 章）", len(chapters))
	}
	ch0, _ := chapters[0].(map[string]any)
	if ch0["title"] != "第一章 风起" {
		t.Errorf("章节 0=%v", ch0)
	}
	if ch0["url"] != srv.URL+"/novels/api/book/2237/chapters/c1" {
		t.Errorf("章节 URL=%v", ch0["url"])
	}
	// 末章来自第二页（验证翻页拼接）
	ch3, _ := chapters[3].(map[string]any)
	if ch3["title"] != "第四章 裂变" {
		t.Errorf("末章=%v（应来自第二页）", ch3)
	}
}

var _ = json.Marshal
