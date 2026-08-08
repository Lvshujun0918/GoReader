package api

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/Lvshujun0918/reader-dev/internal/model"
)

// TestSearchSource 单书源搜索：httptest 书源服务器 + legado CSS 规则解析。
// 覆盖：bookList 列表规则、name/author/bookUrl 字段填充、相对链接解析、Origin 回填。
func TestSearchSource(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	html := `<div class="book"><a class="name" href="/book/1">测试书籍</a><span class="author">作者甲</span></div>
	<div class="book"><a class="name" href="/book/2">第二本书</a><span class="author">作者乙</span></div>`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, html)
	}))
	defer srv.Close()

	src := &model.BookSource{
		BookSourceURL:  srv.URL,
		BookSourceName: "测试书源",
		BookSourceType: 0,
		SearchURL:      srv.URL + "/search?key={key}",
		RuleSearch:     "bookList=@css:.book;name=@css:.name@text;author=@css:.author@text;bookUrl=@css:.name@href",
	}
	results, err := SearchSource(src, "测试")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if len(results) != 2 {
		t.Fatalf("期望 2 条结果，实际 %d", len(results))
	}
	if results[0].Name != "测试书籍" {
		t.Errorf("第一条书名=%q", results[0].Name)
	}
	if results[0].Author != "作者甲" {
		t.Errorf("第一条作者=%q", results[0].Author)
	}
	if want := srv.URL + "/book/1"; results[0].BookURL != want {
		t.Errorf("第一条链接=%q 期望 %q", results[0].BookURL, want)
	}
	if results[0].TocURL != results[0].BookURL {
		t.Error("TocURL 应等于 BookURL")
	}
	if results[1].Name != "第二本书" {
		t.Errorf("第二条书名=%q", results[1].Name)
	}
	if results[0].Origin != srv.URL {
		t.Errorf("origin=%q 期望 %q", results[0].Origin, srv.URL)
	}
	if results[0].OriginName != "测试书源" {
		t.Errorf("originName=%q", results[0].OriginName)
	}
}

// TestSearchSourceJSONRule 搜索规则用 JSONPath（书源返回 JSON 的场景）。
func TestSearchSourceJSONRule(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	body := `{"data":{"books":[{"name":"玄幻一","author":"张三","url":"/b/1"},{"name":"玄幻二","author":"李四","url":"/b/2"}]}}`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, body)
	}))
	defer srv.Close()
	src := &model.BookSource{
		BookSourceURL:  srv.URL,
		BookSourceName: "JSON书源",
		SearchURL:      srv.URL + "/api?key={key}",
		RuleSearch:     "bookList=@json:$.data.books;name=@json:$.name;author=@json:$.author;bookUrl=@json:$.url",
	}
	results, err := SearchSource(src, "玄幻")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if len(results) != 2 {
		t.Fatalf("期望 2 条，实际 %d", len(results))
	}
	if results[0].Name != "玄幻一" || results[0].Author != "张三" {
		t.Errorf("第一条=%+v", results[0])
	}
}

// TestSearchSourceNoRule 无搜索规则应报错。
func TestSearchSourceNoRule(t *testing.T) {
	src := &model.BookSource{BookSourceURL: "https://x.com", SearchURL: "https://x.com/s?key={key}"}
	if _, err := SearchSource(src, "kw"); err == nil {
		t.Fatal("无规则应报错")
	}
}

// TestSearchSourceServerError 书源服务器错误（502）应透传报错。
func TestSearchSourceServerError(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "bad gateway", 502)
	}))
	defer srv.Close()
	src := &model.BookSource{
		BookSourceURL: srv.URL, BookSourceName: "挂掉的书源",
		SearchURL: srv.URL, RuleSearch: "name=@css:.name@text",
	}
	if _, err := SearchSource(src, "x"); err == nil {
		t.Fatal("502 应报错")
	}
}

// TestSearchSourceSingleBook 无 bookList 的单书规则（name 直达）。
func TestSearchSourceSingleBook(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	html := `<h1 class="title">独本书</h1>`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, html)
	}))
	defer srv.Close()
	src := &model.BookSource{
		BookSourceURL: srv.URL, BookSourceName: "单书源",
		SearchURL: srv.URL + "/?key={key}",
		RuleSearch: "name=@css:.title@text;bookUrl=@css:a@href",
	}
	results, err := SearchSource(src, "独本")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if len(results) != 1 || results[0].Name != "独本书" {
		t.Fatalf("单书结果不符: %+v", results)
	}
}

// TestSearchSourceKeywordURL 关键词应被 URL 编码替换 {key}。
func TestSearchSourceKeywordURL(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	var gotQuery string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotQuery = r.URL.RawQuery
		_, _ = fmt.Fprint(w, `<div class="book"><a class="name" href="/b">书</a></div>`)
	}))
	defer srv.Close()
	src := &model.BookSource{
		BookSourceURL: srv.URL, BookSourceName: "编码源",
		SearchURL:      srv.URL + "/search?key={key}",
		RuleSearch:     "bookList=@css:.book;name=@css:.name@text;bookUrl=@css:.name@href",
	}
	if _, err := SearchSource(src, "修仙 小说"); err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if gotQuery == "" {
		t.Fatal("请求未携带查询参数")
	}
	if !containsPercentEncoded(gotQuery) {
		t.Errorf("关键词应 URL 编码，实际 query=%q", gotQuery)
	}
}

// containsPercentEncoded 判断 query 含百分号编码。
func containsPercentEncoded(s string) bool {
	for i := 0; i+2 < len(s); i++ {
		if s[i] == '%' && s[i+1] != 0 && s[i+2] != 0 {
			return true
		}
	}
	return false
}

// TestSearchSourceRealJSONObject 真实书源（XIU2 番茄/酷我格式）：JSON 对象规则 + 裸 JSONPath。
func TestSearchSourceRealJSONObject(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	body := `{"data":[{"title":"斗破苍穹","author_name":"天蚕土豆","url":"/book/1","intro":"经典玄幻"},{"title":"凡人修仙传","author_name":"忘语","url":"/book/2"}]}`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, body)
	}))
	defer srv.Close()

	// legado JSON 对象规则（导入时对象 → JSON 字符串存库）
	ruleStr := `{"bookList":"$.data","name":"$.title","author":"$.author_name","bookUrl":"/api/book/{{$.book_id}}","intro":"$.intro"}`
	// 简化：bookUrl 用固定模板（{{}} 插值由规则引擎处理，此处测试列表+字段）
	ruleStr = `{"bookList":"$.data","name":"$.title","author":"$.author_name","intro":"$.intro"}`
	src := &model.BookSource{
		BookSourceURL:  srv.URL,
		BookSourceName: "JSON对象源",
		SearchURL:      srv.URL + "/api?key={key}",
		RuleSearch:     ruleStr,
	}
	results, err := SearchSource(src, "斗破")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if len(results) != 2 {
		t.Fatalf("期望 2 条，实际 %d", len(results))
	}
	if results[0].Name != "斗破苍穹" {
		t.Errorf("书名=%q", results[0].Name)
	}
	if results[0].Author != "天蚕土豆" {
		t.Errorf("作者=%q", results[0].Author)
	}
	if results[0].Intro != "经典玄幻" {
		t.Errorf("简介=%q", results[0].Intro)
	}
	if results[1].Name != "凡人修仙传" {
		t.Errorf("第二本书名=%q", results[1].Name)
	}
	if results[0].OriginName != "JSON对象源" {
		t.Errorf("originName=%q", results[0].OriginName)
	}
}

// TestSearchSourceLegacyBareJSONPath legacy 字符串规则 + 裸 JSONPath 值。
func TestSearchSourceLegacyBareJSONPath(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	body := `{"data":[{"title":"书一","author":"作者一"},{"title":"书二","author":"作者二"}]}`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, body)
	}))
	defer srv.Close()
	src := &model.BookSource{
		BookSourceURL: srv.URL, BookSourceName: "裸JSONPath",
		SearchURL: srv.URL + "/?key={key}",
		RuleSearch: "bookList=$.data;name=$.title;author=$.author",
	}
	results, err := SearchSource(src, "书")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if len(results) != 2 || results[0].Name != "书一" || results[1].Author != "作者二" {
		t.Fatalf("裸 JSONPath 结果不符: %+v", results)
	}
}

// TestSearchSourceJSFormat legado <js> 规则格式（起点书源）。
func TestSearchSourceJSFormat(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	// <js> 返回书籍列表 JSON 字符串数组
	body := `["{\"name\":\"js书一\"}","{\"name\":\"js书二\"}"]`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, body)
	}))
	defer srv.Close()
	src := &model.BookSource{
		BookSourceURL: srv.URL, BookSourceName: "JS格式源",
		SearchURL:     srv.URL + "/?key={key}",
		RuleSearch:    `{"bookList":"<js>JSON.parse(result)</js>","name":"$.name"}`,
	}
	results, err := SearchSource(src, "js")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	// <js> 返回数组 → evalJS 展开成字符串项
	if len(results) == 0 {
		t.Fatal("js 规则应有结果")
	}
	if results[0].Name == "" {
		t.Errorf("js 列表项 name 未填充: %+v", results[0])
	}
}
