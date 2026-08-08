package api

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"strings"
	"testing"

	"github.com/Lvshujun0918/GoReader/internal/model"
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
	// <js> 返回书籍列表 JSON 字符串数组（result 已解析为对象/数组——legado 语义，
	// 直接返回数组即可由 evalJS 展开为字符串项）
	body := `["{\"name\":\"js书一\"}","{\"name\":\"js书二\"}"]`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, body)
	}))
	defer srv.Close()
	src := &model.BookSource{
		BookSourceURL: srv.URL, BookSourceName: "JS格式源",
		SearchURL:     srv.URL + "/?key={key}",
		RuleSearch:    `{"bookList":"<js>result</js>","name":"$.name"}`,
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

// TestSearchSourceDoubleBraces 真实书源 searchUrl 用 {{key}}（legado 双花括号），
// 关键词应被完整 URL 编码（请求行合法，标准 HTTP 服务器可接受）。
func TestSearchSourceDoubleBraces(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	var rawQuery string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		rawQuery = r.URL.RawQuery
		_, _ = fmt.Fprint(w, `{"data":[{"title":"斗破苍穹","author_name":"天蚕土豆"}]}`)
	}))
	defer srv.Close()
	src := &model.BookSource{
		BookSourceURL:  srv.URL,
		BookSourceName: "双花括号源",
		SearchURL:      srv.URL + "/api/search?keyword={{key}}&page={{page}}",
		RuleSearch:     `{"bookList":"$.data","name":"$.title","author":"$.author_name"}`,
	}
	results, err := SearchSource(src, "斗破 玄幻")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if len(results) != 1 || results[0].Name != "斗破苍穹" {
		t.Fatalf("结果不符: %+v", results)
	}
	// 占位符应被替换且完整编码（含中文 + 空格 %20）
	expected := "keyword=%E6%96%97%E7%A0%B4%20%E7%8E%84%E5%B9%BB&page=1"
	if rawQuery != expected {
		t.Errorf("query=%q 期望 %q", rawQuery, expected)
	}
}

// TestSearchSourceJSURLPrefix @js: 前缀构造搜索 URL（起点/大文学等）。
func TestSearchSourceJSURLPrefix(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	var path string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		path = r.URL.EscapedPath()
		_, _ = fmt.Fprint(w, `<div class="book"><a class="nm" href="/b/1">JS书</a></div>`)
	}))
	defer srv.Close()
	// JS：baseUrl 来自 ctx 变量，拼路径 + {{key}} 占位符（JS 字符串里，解析后替换）
	ruleURL := `@js:url=baseUrl+"/so/{{key}}.html"`
	src := &model.BookSource{
		BookSourceURL:  srv.URL,
		BookSourceName: "JS构造源",
		SearchURL:      ruleURL,
		RuleSearch:     "bookList=@css:.book;name=@css:.nm@text",
	}
	results, err := SearchSource(src, "斗破")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if len(results) != 1 || results[0].Name != "JS书" {
		t.Fatalf("结果不符: %+v", results)
	}
	if path != "/so/%E6%96%97%E7%A0%B4.html" {
		t.Errorf("JS 构造路径=%q", path)
	}
}

// TestSearchSourcePOSTForm searchUrl 带 ,{'method':'POST','body':...} 表单描述（69书吧等）。
func TestSearchSourcePOSTForm(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	var gotMethod, gotCT, gotBody string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotMethod = r.Method
		gotCT = r.Header.Get("Content-Type")
		b, _ := io.ReadAll(r.Body)
		gotBody = string(b)
		_, _ = fmt.Fprint(w, `<div class="item"><a class="t" href="/x">POST书</a></div>`)
	}))
	defer srv.Close()
	src := &model.BookSource{
		BookSourceURL:  srv.URL,
		BookSourceName: "POST源",
		SearchURL:      srv.URL + "/search.php,{'method':'POST','body':'searchkey={{key}}&searchtype=all'}",
		RuleSearch:     "bookList=@css:.item;name=@css:.t@text",
	}
	results, err := SearchSource(src, "修仙")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if len(results) != 1 || results[0].Name != "POST书" {
		t.Fatalf("结果不符: %+v", results)
	}
	if gotMethod != http.MethodPost {
		t.Errorf("method=%s 期望 POST", gotMethod)
	}
	if !strings.HasPrefix(gotCT, "application/x-www-form-urlencoded") {
		t.Errorf("Content-Type=%q", gotCT)
	}
	if gotBody != "searchkey=%E4%BF%AE%E4%BB%99&searchtype=all" {
		t.Errorf("POST body=%q", gotBody)
	}
}

// TestGBKPercentEncode 关键词 GBK 编码（charset=gbk 书源）。
func TestGBKPercentEncode(t *testing.T) {
	// "修仙" UTF-8: E4 BF AE E4 BB 99；GBK: D0 DE CF C9
	got := gbkPercentEncode("修仙")
	want := "%D0%DE%CF%C9"
	if got != want {
		t.Errorf("gbkPercentEncode=%q 期望 %q", got, want)
	}
	if gbkPercentEncode("abc123") != "abc123" {
		t.Errorf("ASCII 不应被编码")
	}
}

// TestBuildSearchRequestPlaceholderOrder {{key}} 不被 {key} 部分命中；相对路径 resolve。
func TestBuildSearchRequestPlaceholderOrder(t *testing.T) {
	req := buildSearchRequest("https://x.com/s?k={{key}}&q={key}", "斗破", 1, nil, "https://x.com")
	if !strings.Contains(req.URL, "k=%E6%96%97%E7%A0%B4&q=%E6%96%97%E7%A0%B4") {
		t.Errorf("双/单花括号均应替换: %q", req.URL)
	}
	if strings.Contains(req.URL, "{{") || strings.Contains(req.URL, "{key}") {
		t.Errorf("残留占位符: %q", req.URL)
	}
	// 相对 searchUrl resolve 到书源根
	req2 := buildSearchRequest("/api/search?keyword={{key}}", "斗破", 1, nil, "https://src.com")
	if req2.URL != "https://src.com/api/search?keyword=%E6%96%97%E7%A0%B4" {
		t.Errorf("相对 resolve 失败: %q", req2.URL)
	}
}

// TestParseRequestDesc 单引号 JSON 描述解析。
func TestParseRequestDesc(t *testing.T) {
	r := &searchRequest{}
	parseRequestDesc(`{'method':'POST','body':'a={{key}}','charset':'gbk'}`, r)
	if r.Method != "POST" || r.Body != "a={{key}}" || r.Charset != "gbk" {
		t.Errorf("解析失败: %+v", r)
	}
}

// TestSearchSourceSingleBookJSON 单书规则（无 bookList）JSONPath 直达。
func TestSearchSourceSingleBookJSON(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, `{"title":"单书","author":"作者"}`)
	}))
	defer srv.Close()
	src := &model.BookSource{
		BookSourceURL:  srv.URL,
		BookSourceName: "单书JSON",
		SearchURL:      srv.URL + "/?k={key}",
		RuleSearch:     `{"name":"$.title","author":"$.author"}`,
	}
	results, err := SearchSource(src, "单书")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if len(results) != 1 || results[0].Name != "单书" || results[0].Author != "作者" {
		t.Fatalf("结果不符: %+v", results)
	}
}

// TestSearchSourceRealKuwo 真实 XIU2 酷我书源格式：相对 searchUrl + {{key}}/{{page}}、
// {{$.book_id}} 插值 bookUrl、kind 的 @js: 后缀后处理。
func TestSearchSourceRealKuwo(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	var rawQuery string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		rawQuery = r.URL.RawQuery
		_, _ = fmt.Fprint(w, `{"data":[{"book_id":"123","title":"斗破苍穹","author_name":"天蚕土豆",
			"intro":"三十年河东","category_name":"玄幻","status":"30","all_words":"100万","cover_url":"/c/1.jpg"}]}`)
	}))
	defer srv.Close()
	// 真实酷我配置：searchUrl 相对路径，bookUrl 用 {{$.book_id}}，kind 用 @js: 后缀
	src := &model.BookSource{
		BookSourceURL:  srv.URL,
		BookSourceName: "酷我小说",
		SearchURL:      "/novels/api/book/search?keyword={{key}}&pi={{page}}&ps=30",
		RuleSearch: `{"author":"$.author_name","bookList":"$.data",
			"bookUrl":"/novels/api/book/{{$.book_id}}","coverUrl":"$.cover_url","intro":"$.intro",
			"kind":"{{$.category_name}},{{$.status}}@js:result.replace(/30/,\"连载\").replace(/50/,\"完结\")",
			"name":"$.title","wordCount":"$.all_words"}`,
	}
	results, err := SearchSource(src, "斗破")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if len(results) != 1 {
		t.Fatalf("期望 1 条，实际 %d", len(results))
	}
	r := results[0]
	if r.Name != "斗破苍穹" || r.Author != "天蚕土豆" || r.Intro != "三十年河东" || r.WordCount != "100万" {
		t.Errorf("基础字段不符: %+v", r)
	}
	// 相对 searchUrl resolve + {{key}} 编码 + {{page}}
	exp := "keyword=%E6%96%97%E7%A0%B4&pi=1&ps=30"
	if rawQuery != exp {
		t.Errorf("query=%q 期望 %q", rawQuery, exp)
	}
	// {{$.book_id}} 插值 + resolve
	expURL := srv.URL + "/novels/api/book/123"
	if r.BookURL != expURL || r.TocURL != expURL {
		t.Errorf("bookUrl=%q tocUrl=%q 期望 %q", r.BookURL, r.TocURL, expURL)
	}
	// @js: 后缀后处理（30 → 连载）
	if r.Kind != "玄幻,连载" {
		t.Errorf("kind=%q 期望 玄幻,连载", r.Kind)
	}
	// coverUrl resolve
	if r.CoverURL != srv.URL+"/c/1.jpg" {
		t.Errorf("coverUrl=%q", r.CoverURL)
	}
}

// TestSearchDaHuiLangSourceReal 真实网络单步测试：大灰狼书源 + 真实聚合 API。
// 需要外网可达 api.langge.cf；失败/超时自动跳过（CI 无网场景不阻塞）。
func TestSearchDaHuiLangSourceReal(t *testing.T) {
	const server = "https://api.langge.cf"
	if resp, err := http.Get(server + "/"); err != nil || resp.StatusCode != 200 {
		t.Skipf("真实 API 不可达：%v", err)
	}
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	src := &model.BookSource{
		BookSourceURL: server, BookSourceName: "大灰狼真实",
		Variable:      "server=" + server,
		SearchURL:     daHuiLangSearchURL(),
		RuleSearch:    daHuiLangRuleSearch(),
	}
	results, err := SearchSource(src, "斗破苍穹")
	if err != nil {
		t.Skipf("真实搜索失败（站点可能变动）：%v", err)
	}
	if len(results) == 0 {
		t.Fatal("真实搜索无结果（应至少返回站点提示项）")
	}
	if results[0].Name == "" {
		t.Errorf("首条结果 name 为空: %+v", results[0])
	}
}

// daHuiLangSearchURL 大灰狼 searchUrl 模板（与 fixture 同构，server 走书源变量）。
func daHuiLangSearchURL() string {
	return "<js>\nlet base_url = getArguments(source.getVariable(), 'server');\n" +
		"let media = '';\nlet sources = '0';\n" +
		"let disabled_sources = getArguments(source.getVariable(), 'disabled_sources') || '0';\n" +
		"let qtcookie = cookie.getCookie(base_url);\n" +
		"let op = JSON.stringify({ method: 'GET', headers: { cookie: qtcookie } });\n" +
		"`${base_url}/search?title=${key}&tab=${media}&source=${sources}&page=1&disabled_sources=${disabled_sources},${op}`\n</js>"
}

// daHuiLangRuleSearch 大灰狼 ruleSearch（与 fixture 同构）。
func daHuiLangRuleSearch() string {
	return `{"author":"$.author","bookList":"$.data","bookUrl":"<js>let book_id = result.book_id; let url = result.toc_url || ''; ` + "`" + `data:;base64,${java.base64Encode(JSON.stringify({book_id:book_id,url:url}))}` + "`" + `</js>","coverUrl":"$.thumb_url","intro":"$.abstract","name":"$.book_name","wordCount":"$.word_number"}`
}

// TestSearchChainFieldRules 搜索结果字段链式规则（用户报告场景）：
// bookList=.odd（class 缩写）+ 裸 tag 推进（td.0）+ ## 文本替换（##《|》）。
func TestSearchChainFieldRules(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		fmt.Fprint(w, `<table>
			<tr class="odd"><td>《书名一》</td><td>作者一</td></tr>
			<tr class="odd"><td>《书名二》</td><td>作者二</td></tr>
		</table>`)
	}))
	defer srv.Close()

	src := &model.BookSource{
		BookSourceURL: srv.URL, BookSourceName: "链式规则源",
		SearchURL:     srv.URL + "/search?key={key}",
		RuleSearch:    `{"bookList":".odd","name":"td.0@text##《|》","author":"td.1@text"}`,
	}
	results, err := SearchSource(src, "测试")
	if err != nil {
		t.Fatalf("SearchSource 失败: %v", err)
	}
	if len(results) != 2 {
		t.Fatalf("应 2 条结果，实际 %d: %+v", len(results), results)
	}
	if results[0].Name != "书名一" {
		t.Errorf("书名应解析为「书名一」（不含规则串），实际: %q", results[0].Name)
	}
	if results[0].Author != "作者一" {
		t.Errorf("作者应解析为「作者一」，实际: %q", results[0].Author)
	}
	if results[1].Name != "书名二" {
		t.Errorf("第 2 本书名: %q", results[1].Name)
	}
}

/* ================= 大灰狼融合书源（goja JS 搜索 + SSE 格式 + 调试） ================= */

// daHuiLangSource 加载大灰狼书源 fixture（bookSourceUrl 重定向到 mock server）。
func daHuiLangSource(t *testing.T, serverURL string) map[string]any {
	t.Helper()
	b, err := os.ReadFile("testdata/dahuilang.json")
	if err != nil {
		t.Fatalf("缺少 dahuilang.json fixture: %v", err)
	}
	var list []map[string]any
	if err := json.Unmarshal(b, &list); err != nil || len(list) == 0 {
		t.Fatalf("fixture 解析失败: %v", err)
	}
	src := list[0]
	src["bookSourceUrl"] = serverURL
	// 书源变量注入 server（JS getArguments(source.getVariable(), 'server') 用）
	src["variable"] = "server=" + serverURL
	return src
}

// TestSearchDaHuiLangSource 大灰狼融合书源搜索：goja 执行 <js> searchUrl
// （getArguments/source/key/cookie）+ ruleSearch JSON 对象（$.data bookList + <js> bookUrl）。
func TestSearchDaHuiLangSource(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1") // 测试 mock server 为内网地址
	mux := http.NewServeMux()
	mux.HandleFunc("/search", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json; charset=utf-8")
		fmt.Fprint(w, `{"data":[
			{"book_id":"123","source":"s1","tab":"小说","toc_url":"/toc/1",
			 "thumb_url":"/cover/1.jpg","abstract":"这是一段简介",
			 "book_name":"测试书名","word_number":"10000",
			 "status":"连载","score":"9.5","tags":"玄幻",
			 "last_chapter_title":"第1章","last_chapter_update_time":"2024-01-01"}
		]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	h := newTestAPI(t)
	saveOneSource(t, h, daHuiLangSource(t, srv.URL))

	w := perform(h, "GET", "/reader3/searchBook?key="+url.QueryEscape("测试")+"&origin="+srv.URL, nil)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("searchBook 失败: %s", rd.ErrorMsg)
	}
	list, _ := rd.Data.([]any)
	if len(list) == 0 {
		t.Fatal("搜索结果为空（大灰狼书源 JS 搜索未出结果）")
	}
	book, _ := list[0].(map[string]any)
	if book["name"] != "测试书名" {
		t.Errorf("书名不符: %v", book["name"])
	}
	if book["intro"] != "这是一段简介" {
		t.Errorf("简介不符: %v", book["intro"])
	}
	if book["bookUrl"] == "" {
		t.Error("bookUrl 为空（bookUrl 的 <js> 未执行成功）")
	}
}

// TestSearchBookMultiSSEFormat SSE 流式搜索事件格式（前端契约：event: book + {lastIndex,data} + event: end）。
func TestSearchBookMultiSSEFormat(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	mux := http.NewServeMux()
	mux.HandleFunc("/search", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json; charset=utf-8")
		fmt.Fprint(w, `{"data":[{"book_id":"1","book_name":"SSE书","abstract":"简介","source":"s"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	h := newTestAPI(t)
	saveOneSource(t, h, daHuiLangSource(t, srv.URL))

	w := perform(h, "GET", "/reader3/searchBookMultiSSE?key="+url.QueryEscape("测试"), nil)
	body := w.Body.String()
	if !strings.Contains(body, "event: start") || !strings.Contains(body, `"total":1`) {
		t.Fatalf("SSE 缺 start 帧（total）:\n%s", body)
	}
	if !strings.Contains(body, "event: book") {
		t.Fatalf("SSE 缺 event: book 帧:\n%s", body)
	}
	if !strings.Contains(body, "event: end") {
		t.Fatalf("SSE 缺 event: end 帧:\n%s", body)
	}
	if !strings.Contains(body, `"lastIndex":0`) {
		t.Errorf("book 帧缺 lastIndex=0:\n%s", body)
	}
	if !strings.Contains(body, "SSE书") {
		t.Errorf("book 帧缺搜索结果:\n%s", body)
	}
}

// TestDebugSSESearch 调试功能：search 动作应返回 step + result 事件。
func TestDebugSSESearch(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	mux := http.NewServeMux()
	mux.HandleFunc("/search", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json; charset=utf-8")
		fmt.Fprint(w, `{"data":[{"book_id":"1","book_name":"调试书","abstract":"简介","source":"s"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	h := newTestAPI(t)
	saveOneSource(t, h, daHuiLangSource(t, srv.URL))

	u := "/reader3/bookSourceDebugSSE?bookSource=" + srv.URL + "&action=search&key=" + url.QueryEscape("测试")
	w := perform(h, "GET", u, nil)
	body := w.Body.String()
	if !strings.Contains(body, "event: step") {
		t.Fatalf("调试缺 step 帧:\n%s", body)
	}
	if !strings.Contains(body, "event: result") {
		t.Fatalf("调试缺 result 帧:\n%s", body)
	}
	if !strings.Contains(body, "调试书") {
		t.Errorf("调试 result 缺结果:\n%s", body)
	}
}
