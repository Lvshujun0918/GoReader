package bookfetch

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/Lvshujun0918/reader-dev/internal/model"
)

// kuwoServer 模拟酷我小说 API（JSON）：/novels/api/book/{id} 详情、chapters 目录、chapters/{cid} 正文。
func kuwoServer(t *testing.T) *httptest.Server {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		p := r.URL.Path
		var body string
		switch {
		case strings.Contains(p, "/chapters/"):
			body = `{"data":{"content":"第一章正文内容，测试用。\n\n第二段。"}}`
		case strings.HasSuffix(p, "/chapters"):
			body = `{"data":[{"book_id":"2237","chapter_id":"c1","chapter_title":"第一章 风起","volume_name":"第一卷","original_words":"2300"},{"book_id":"2237","chapter_id":"c2","chapter_title":"第二章 云涌","volume_name":"第一卷","original_words":"1800"}]}`
		default:
			body = `{"data":{"book_id":"2237","title":"斗破苍穹","author_name":"天蚕土豆","intro":"三十年河东","category_name":"玄幻","status":"30","all_words":"100万","cover_url":"/cover/1.jpg","new_chapter_name":"第二章 云涌","update_time":"2026-08-01 12:00:00"}}`
		}
		_, _ = fmt.Fprint(w, body)
	}))
	t.Cleanup(srv.Close)
	return srv
}

// kuwoSource 真实酷我书源（fixture 原样规则），bookSourceUrl 指向 mock。
func kuwoSource(srvURL string) *model.BookSource {
	return &model.BookSource{
		BookSourceURL:  srvURL,
		BookSourceName: "酷我小说",
		RuleBookInfo: `{"author":"$.author_name","coverUrl":"$.cover_url","init":"$.data",
			"intro":"$.intro##(^|[。！？]+[”」）】]?)##$1<br>",
			"kind":"{{$.category_name}},{{$.status}},{{$.update_time}}@js:result.replace(/30/,\"连载\").replace(/50/,\"完结\").replace(/\\s..:.*/,\"\")",
			"lastChapter":"$.new_chapter_name","name":"$.title","tocUrl":"/novels/api/book/{{$.book_id}}/chapters?paging=0","wordCount":"$.all_words"}`,
		RuleToc: `{"chapterList":"$.data","chapterName":"$.chapter_title##正文卷.|正文.|VIP卷.|默认卷.|卷_|VIP章节.|免费章节.|章节目录.|最新章节.|[\\(（【].*?[求更票谢乐发订合补加架字修Kk].*?[】）\\)]",
			"chapterUrl":"/novels/api/book/{{$.book_id}}/chapters/{{$.chapter_id}}",
			"updateTime":"{{$.volume_name}}•{{$.original_words}}字"}`,
		RuleContent: `{"content":"$.data.content"}`,
	}
}

// TestBookInfoJSONRule 真实酷我 ruleBookInfo：init + JSON 对象 + {{$.book_id}} tocUrl + ## 替换 + @js: 后缀。
func TestBookInfoJSONRule(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	srv := kuwoServer(t)
	src := kuwoSource(srv.URL)
	f := New(nil, "")
	info, err := f.BookInfo(src, srv.URL+"/novels/api/book/2237")
	if err != nil {
		t.Fatalf("BookInfo 失败: %v", err)
	}
	if info["name"] != "斗破苍穹" || info["author"] != "天蚕土豆" {
		t.Errorf("name/author 不符: %v", info)
	}
	if info["tocUrl"] != srv.URL+"/novels/api/book/2237/chapters?paging=0" {
		t.Errorf("tocUrl=%v（{{$.book_id}} 插值 + resolve）", info["tocUrl"])
	}
	if info["wordCount"] != "100万" || info["lastChapter"] != "第二章 云涌" {
		t.Errorf("wordCount/lastChapter 不符: %v", info)
	}
	// ## 替换：intro 句首加 <br>
	if !strings.Contains(fmt.Sprint(info["intro"]), "<br>三十年河东") {
		t.Errorf("intro ## 替换失败: %v", info["intro"])
	}
	// @js: 后缀：status 30→连载，update_time 去时间
	if info["kind"] != "玄幻,连载,2026-08-01" {
		t.Errorf("kind @js: 后缀失败: %v", info["kind"])
	}
	// coverUrl resolve
	if info["coverUrl"] != srv.URL+"/cover/1.jpg" {
		t.Errorf("coverUrl=%v", info["coverUrl"])
	}
}

// TestBookTocJSONRule 真实酷我 ruleToc：chapterList + chapterName ## 替换 + chapterUrl {{$.xxx}} 插值。
func TestBookTocJSONRule(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	srv := kuwoServer(t)
	src := kuwoSource(srv.URL)
	f := New(nil, "")
	chapters, next, err := f.BookToc(src, srv.URL+"/novels/api/book/2237/chapters?paging=0")
	if err != nil {
		t.Fatalf("BookToc 失败: %v", err)
	}
	if next != "" {
		t.Errorf("next 应为空: %q", next)
	}
	if len(chapters) != 2 {
		t.Fatalf("期望 2 章，实际 %d", len(chapters))
	}
	if chapters[0].Title != "第一章 风起" {
		t.Errorf("章节 0 标题=%q（chapterName ## 替换）", chapters[0].Title)
	}
	want := srv.URL + "/novels/api/book/2237/chapters/c1"
	if chapters[0].URL != want {
		t.Errorf("章节 0 url=%q 期望 %q（{{$.book_id}}/{{$.chapter_id}} 插值 + resolve）", chapters[0].URL, want)
	}
	if chapters[1].URL != srv.URL+"/novels/api/book/2237/chapters/c2" {
		t.Errorf("章节 1 url=%q", chapters[1].URL)
	}
}

// TestBookContentJSONRule 真实酷我 ruleContent：{"content":"$.data.content"}。
func TestBookContentJSONRule(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	srv := kuwoServer(t)
	src := kuwoSource(srv.URL)
	f := New(nil, "")
	res, err := f.BookContent(src, srv.URL+"/novels/api/book/2237/chapters/c1", 3)
	if err != nil {
		t.Fatalf("BookContent 失败: %v", err)
	}
	if !strings.Contains(res.Content, "第一章正文内容") {
		t.Errorf("content=%q（$.data.content 提取）", res.Content)
	}
	if res.WordCount <= 0 {
		t.Errorf("wordCount=%d", res.WordCount)
	}
}

// TestParseRuleMap JSON 对象与 legacy 分号格式解析。
func TestParseRuleMap(t *testing.T) {
	// JSON 对象
	m := parseRuleMap(`{"content":"$.data.content","nextUrl":"$.next"}`)
	if m["content"] != "$.data.content" || m["nextUrl"] != "$.next" {
		t.Errorf("JSON 对象解析失败: %v", m)
	}
	// legacy 分号
	m2 := parseRuleMap("content=@css:.content;nextUrl=@css:.next@href")
	if m2["content"] != "@css:.content" || m2["nextUrl"] != "@css:.next@href" {
		t.Errorf("分号解析失败: %v", m2)
	}
	// 空
	if len(parseRuleMap("")) != 0 {
		t.Errorf("空规则应返回空 map")
	}
}
