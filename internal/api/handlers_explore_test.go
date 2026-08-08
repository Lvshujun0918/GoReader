package api

import (
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/Lvshujun0918/GoReader/internal/model"
	"github.com/Lvshujun0918/GoReader/internal/parser/rule"
	"github.com/Lvshujun0918/GoReader/internal/service/bookfetch"
)

// ---------- 辅助 ----------

// saveOneSource 通过 HTTP 保存单个书源。
func saveOneSource(t *testing.T, h http.Handler, src map[string]any) {
	t.Helper()
	w := perform(h, "POST", "/reader3/saveBookSources", []map[string]any{src})
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("保存书源失败: %v (%s)", rd.ErrorMsg, w.Body.String())
	}
}

// sourceBase 构造一个基础探索书源（exploreUrl/ruleExplore 由调用方补充）。
func sourceBase(srvURL, name string) map[string]any {
	return map[string]any{
		"bookSourceUrl":  srvURL,
		"bookSourceName": name,
		"bookSourceType": 0,
		"enabledExplore": true,
	}
}

// fetchExploreUrls 调 getExploreUrls 并返回分类数组。
func fetchExploreUrls(t *testing.T, h http.Handler, paramName, bookSource string) []map[string]any {
	t.Helper()
	w := perform(h, "GET", "/reader3/getExploreUrls?"+paramName+"="+bookSource, nil)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("getExploreUrls 失败: %v", rd.ErrorMsg)
	}
	arr, _ := rd.Data.([]any)
	var out []map[string]any
	for _, it := range arr {
		m, _ := it.(map[string]any)
		out = append(out, m)
	}
	return out
}

// ---------- getExploreUrls ----------

// TestGetExploreUrlsBookSourceParam 前端实际传 bookSource 参数名（不是 origin）。
func TestGetExploreUrlsBookSourceParam(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	h := newTestAPI(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, "ok")
	}))
	defer srv.Close()
	src := sourceBase(srv.URL, "探索源")
	src["exploreUrl"] = "玄幻::/xh.html\n都市::/ds.html"
	saveOneSource(t, h, src)
	cats := fetchExploreUrls(t, h, "bookSource", srv.URL)
	if len(cats) != 2 || cats[0]["title"] != "玄幻" {
		t.Fatalf("bookSource 参数应返回分类: %+v", cats)
	}
}

// TestGetExploreUrlsTitleURL 69书吧格式：标题::URL 换行分隔。
func TestGetExploreUrlsTitleURL(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	h := newTestAPI(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, "ok")
	}))
	defer srv.Close()
	src := sourceBase(srv.URL, "69书吧")
	src["exploreUrl"] = "全部分类::/blist/class/0/{{page}}.htm\n玄幻魔法::/blist/class/1/{{page}}.htm\n修真武侠::/blist/class/2/{{page}}.htm"
	saveOneSource(t, h, src)

	cats := fetchExploreUrls(t, h, "bookSource", srv.URL)
	if len(cats) != 3 {
		t.Fatalf("期望 3 个分类，实际 %d: %v", len(cats), cats)
	}
	if cats[0]["title"] != "全部分类" || cats[0]["url"] != "/blist/class/0/{{page}}.htm" {
		t.Errorf("分类 0 不符: %+v", cats[0])
	}
	if cats[1]["title"] != "玄幻魔法" {
		t.Errorf("分类 1 标题=%v", cats[1]["title"])
	}
	// legacy origin 参数也兼容
	cats2 := fetchExploreUrls(t, h, "origin", srv.URL)
	if len(cats2) != 3 {
		t.Errorf("origin 参数应同样返回 3 个分类")
	}
}

// TestGetExploreUrlsJSONArray 起点/番茄格式：JSON 数组（含空 url 分组 → type=link）。
func TestGetExploreUrlsJSONArray(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	h := newTestAPI(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, "ok")
	}))
	defer srv.Close()
	src := sourceBase(srv.URL, "起点中文")
	src["exploreUrl"] = `[{"title":"男生榜单","url":"","style":{"x":1}},{"title":"月票榜","url":"/rank/month.html"}]`
	saveOneSource(t, h, src)

	cats := fetchExploreUrls(t, h, "bookSource", srv.URL)
	if len(cats) != 2 {
		t.Fatalf("期望 2 个分类，实际 %d: %v", len(cats), cats)
	}
	if cats[0]["title"] != "男生榜单" || cats[0]["type"] != "link" {
		t.Errorf("空 url 分类应为 link 类型: %+v", cats[0])
	}
	if cats[1]["url"] != "/rank/month.html" {
		t.Errorf("分类 1 url=%v", cats[1]["url"])
	}
	if cats[1]["type"] != nil {
		t.Errorf("有 url 分类不应标 link: %+v", cats[1])
	}
}

// TestGetExploreUrlsJSPrefix @js: 前缀构造分类数组（笔阅读器格式）。
func TestGetExploreUrlsJSPrefix(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	h := newTestAPI(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, "ok")
	}))
	defer srv.Close()
	src := sourceBase(srv.URL, "笔阅读器")
	src["exploreUrl"] = `@js:
sort=[];
push=(title,url)=>sort.push({title:title,url:url});
push("分类A","/a.html");push("分类B","/b.html");
JSON.stringify(sort)`
	saveOneSource(t, h, src)

	cats := fetchExploreUrls(t, h, "bookSource", srv.URL)
	if len(cats) != 2 {
		t.Fatalf("期望 2 个分类，实际 %d: %v", len(cats), cats)
	}
	if cats[0]["title"] != "分类A" || cats[0]["url"] != "/a.html" {
		t.Errorf("js 分类 0 不符: %+v", cats[0])
	}
}

// ---------- exploreBook ----------

// TestExploreBookBookSourceParam 前端传 bookSource 参数 → 抓取分类书籍。
func TestExploreBookBookSourceParam(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	h := newTestAPI(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// 断言 {{page}} 已替换
		if r.URL.RawQuery != "pageid=1&size=20" {
			t.Errorf("探索请求 query=%q，期望 pageid=1&size=20", r.URL.RawQuery)
		}
		_, _ = fmt.Fprint(w, `[{"title":"熊猫书","authorname":"作者甲","book_id":"9"}]`)
	}))
	defer srv.Close()
	src := sourceBase(srv.URL, "熊猫看书")
	src["exploreUrl"] = `/category/list?pageid={{page}}&size=20`
	src["ruleExplore"] = map[string]any{
		"author":   "$.authorname",
		"bookList": "$[*]",
		"bookUrl":  "/book/{{$.book_id}}",
		"name":     "$.title",
	}
	saveOneSource(t, h, src)

	// 分类 URL 用 mock 全路径（含 {{page}}）；axios 会 encode url 参数，测试同样编码
	catURL := url.QueryEscape(srv.URL + "/category/list?pageid={{page}}&size=20")
	w := perform(h, "GET", "/reader3/exploreBook?url="+catURL+"&bookSource="+srv.URL+"&page=1", nil)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("exploreBook 失败: %v", rd.ErrorMsg)
	}
	arr, _ := rd.Data.([]any)
	if len(arr) != 1 {
		t.Fatalf("期望 1 本书，实际 %d", len(arr))
	}
	book, _ := arr[0].(map[string]any)
	if book["name"] != "熊猫书" || book["author"] != "作者甲" {
		t.Errorf("字段不符: %+v", book)
	}
	if book["bookUrl"] != srv.URL+"/book/9" {
		t.Errorf("bookUrl=%v（{{$.book_id}} 插值 + resolve）", book["bookUrl"])
	}
	if book["originName"] != "熊猫看书" {
		t.Errorf("originName=%v", book["originName"])
	}
}

// TestExploreBookCSSRule 69书吧 CSS 规则（bookList 列表 + 相对 bookUrl resolve）。
func TestExploreBookCSSRule(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	h := newTestAPI(t)
	html := `<ul class="newlistbox">
  <li><h3><a href="/book/1/">书一</a></h3><label>作者一</label></li>
  <li><h3><a href="/book/2/">书二</a></h3><label>作者二</label></li>
</ul>`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, html)
	}))
	defer srv.Close()
	src := sourceBase(srv.URL, "69书吧")
	src["exploreUrl"] = "全部分类::/blist/class/0/{{page}}.htm"
	src["ruleExplore"] = map[string]any{
		"author":   "tag.label.0@text",
		"bookList": "class.newlistbox.0@tag.ul.0@tag.li",
		"bookUrl":  "tag.h3.0@tag.a.0@href",
		"name":     "tag.h3.0@tag.a.0@text",
	}
	saveOneSource(t, h, src)

	catURL := url.QueryEscape(srv.URL + "/blist/class/0/1.htm")
	w := perform(h, "GET", "/reader3/exploreBook?url="+catURL+"&bookSource="+srv.URL, nil)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("exploreBook 失败: %v", rd.ErrorMsg)
	}
	arr, _ := rd.Data.([]any)
	if len(arr) != 2 {
		t.Fatalf("期望 2 本书，实际 %d: %v", len(arr), arr)
	}
	b0, _ := arr[0].(map[string]any)
	if b0["name"] != "书一" || b0["author"] != "作者一" {
		t.Errorf("书一字段不符: %+v", b0)
	}
	if b0["bookUrl"] != srv.URL+"/book/1/" {
		t.Errorf("bookUrl=%v", b0["bookUrl"])
	}
}

// TestExploreBookLegacyRule legacy 分号规则（bookList=...;name=...）。
func TestExploreBookLegacyRule(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	h := newTestAPI(t)
	html := `<div class="item"><a class="nm" href="/b/1">旧书</a><span class="au">老作者</span></div>`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprint(w, html)
	}))
	defer srv.Close()
	src := sourceBase(srv.URL, "旧格式源")
	src["exploreUrl"] = "分类::/cat/1.html"
	src["ruleExplore"] = "bookList=@css:.item;name=@css:.nm@text;author=@css:.au@text;bookUrl=@css:.nm@href"
	saveOneSource(t, h, src)

	w := perform(h, "GET", "/reader3/exploreBook?url="+srv.URL+"/cat/1.html&origin="+srv.URL, nil)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("exploreBook 失败: %v", rd.ErrorMsg)
	}
	arr, _ := rd.Data.([]any)
	if len(arr) != 1 {
		t.Fatalf("期望 1 本书，实际 %d", len(arr))
	}
	b, _ := arr[0].(map[string]any)
	if b["name"] != "旧书" || b["bookUrl"] != srv.URL+"/b/1" {
		t.Errorf("legacy 规则字段不符: %+v", b)
	}
}

// TestExploreBookPagePlaceholderReplace 分页占位符 {{page}} 替换。
func TestExploreBookPagePlaceholderReplace(t *testing.T) {
	got := replaceExplorePage("/cat/{{page}}.htm?p={page}", 3)
	if got != "/cat/3.htm?p=3" {
		t.Errorf("replaceExplorePage=%q", got)
	}
}

// TestExploreCategoryTitleFromURL 纯 URL 分类名提取。
func TestExploreCategoryTitleFromURL(t *testing.T) {
	if got := categoryTitleFromURL("https://x.com/category/xuanhuan.html"); got != "xuanhuan.html" {
		t.Errorf("提取=%q", got)
	}
	if got := categoryTitleFromURL("/blist/class/0/{{page}}.htm"); got != "默认" {
		t.Errorf("含占位符应默认: %q", got)
	}
	if got := categoryTitleFromURL("https://x.com/"); got != "默认" {
		t.Errorf("根路径应默认: %q", got)
	}
}

// isPrivateURL 判断 URL 是否内网/回环/空 hostname（与 crawler SSRF 防护同规则——
// 相对 URL 未 resolve 时 hostname 为空即触发"禁止访问内网地址"）。
func isPrivateURL(raw string) bool {
	u, err := url.Parse(raw)
	if err != nil || u.Hostname() == "" {
		return true
	}
	ip := net.ParseIP(u.Hostname())
	if ip != nil {
		return ip.IsLoopback() || ip.IsPrivate() || ip.IsLinkLocalUnicast() ||
			ip.IsLinkLocalMulticast() || ip.IsMulticast() || ip.IsUnspecified()
	}
	return false
}

// TestExploreXIU2URLsNoPrivate 用 XIU2/Yuedu 官方书源合集（fixture 原样）：
// 解析所有书源的 exploreUrl 为分类，resolve 相对路径到书源根后，
// 不应含内网地址或空 hostname（即不应触发"禁止访问内网地址"）。
func TestExploreXIU2URLsNoPrivate(t *testing.T) {
	list := loadRealShuyuan(t)
	checked := 0
	for _, m := range list {
		eu, _ := m["exploreUrl"].(string)
		if strings.TrimSpace(eu) == "" {
			continue
		}
		bookSourceURL, _ := m["bookSourceUrl"].(string)
		name, _ := m["bookSourceName"].(string)
		ctx := &rule.Context{BaseURL: bookSourceURL, Variables: map[string]string{"baseUrl": bookSourceURL}}
		cats := parseExploreURLs(eu, ctx)
		if len(cats) == 0 {
			// @js: 依赖外部环境的跳过（不视为失败）
			continue
		}
		for _, c := range cats {
			if c.URL == "" || c.Type == "link" {
				continue
			}
			checked++
			u := c.URL
			if !strings.HasPrefix(u, "http://") && !strings.HasPrefix(u, "https://") {
				u = bookfetch.ResolveURL(u, bookSourceURL)
			}
			if isPrivateURL(u) {
				t.Errorf("书源 %s 探索 URL 触发 SSRF（内网/空 hostname）: %q", name, u)
			}
		}
	}
	if checked == 0 {
		t.Fatal("无任何可检查的探索 URL")
	}
	t.Logf("已校验 %d 个探索分类 URL（XIU2 合集原样，resolve 后均非内网）", checked)
}

// TestExploreBookRelativeURL 前端传相对分类 URL（getExploreUrls 返回的相对 exploreUrl，
// 如 /blist/class/0/1.htm）→ 后端应 resolve 到书源根后抓取，不报"禁止访问内网地址"。
func TestExploreBookRelativeURL(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	h := newTestAPI(t)
	html := `<ul class="newlistbox"><li><h3><a href="/book/1/">书一</a></h3><label>作者一</label></li></ul>`
	var gotPath string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotPath = r.URL.Path + "?" + r.URL.RawQuery
		_, _ = fmt.Fprint(w, html)
	}))
	defer srv.Close()
	src := sourceBase(srv.URL, "69书吧")
	src["exploreUrl"] = "全部分类::/blist/class/0/{{page}}.htm"
	src["ruleExplore"] = map[string]any{
		"author": "tag.label.0@text", "bookList": "class.newlistbox.0@tag.ul.0@tag.li",
		"bookUrl": "tag.h3.0@tag.a.0@href", "name": "tag.h3.0@tag.a.0@text",
	}
	saveOneSource(t, h, src)

	// 分类 URL：相对路径 + {{page}} 占位符（前端 getExploreUrls 返回原样）
	relURL := "/blist/class/0/{{page}}.htm"
	w := perform(h, "GET", "/reader3/exploreBook?url="+url.QueryEscape(relURL)+"&bookSource="+srv.URL+"&page=1", nil)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("相对 URL 探索应成功（resolve 到书源根）: %v", rd.ErrorMsg)
	}
	// mock 应收到 resolve + {{page}} 替换后的完整路径
	if gotPath != "/blist/class/0/1.htm?" {
		t.Errorf("mock 收到路径=%q 期望 /blist/class/0/1.htm", gotPath)
	}
	arr, _ := rd.Data.([]any)
	if len(arr) != 1 {
		t.Fatalf("期望 1 本书，实际 %d", len(arr))
	}
	book, _ := arr[0].(map[string]any)
	if book["name"] != "书一" || book["bookUrl"] != srv.URL+"/book/1/" {
		t.Errorf("探索结果不符: %+v", book)
	}
}

var _ = model.BookSource{}

var _ = json.Marshal
