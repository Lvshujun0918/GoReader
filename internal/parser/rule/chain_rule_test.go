package rule

import (
	"testing"
)

func TestChainTagHref(t *testing.T) {
	ctx := &Context{}
	html := `<table><tbody><tr><td><a href="/book/123/">《剑来》</a></td></tr></tbody></table>`

	// bookList（搜索路径：ChainNeedsHTML 补 @html）
	listRule := "tag.tbody.0@tag.tr"
	if ChainNeedsHTML(listRule) {
		listRule += "@html"
	}
	t.Logf("listRule: %s", listRule)
	items := Parse(html, listRule, ctx)
	if len(items) == 0 {
		t.Fatalf("bookList 为空")
	}
	t.Logf("item: %s", items[0])

	// 子规则在 item 上解析
	if v := Parse(items[0], "tag.a.0@href", ctx); len(v) == 0 || v[0] != "/book/123/" {
		t.Fatalf("href 解析错误: %v", v)
	}
	if v := Parse(items[0], "tag.a.0@text##《|》", ctx); len(v) == 0 {
		t.Fatalf("name 解析为空")
	} else {
		t.Logf("name: %q", v[0])
	}
}

// TestAttrOnclickJs 阅友小说 @onclick@js: 规则。
func TestAttrOnclickJs(t *testing.T) {
	ctx := &Context{}
	item := `<div class="v-list-item flex" onclick="newWebView(&#39;/b/235755.html&#39;, &#39;&#39;, &#39;&#39;)"><a href="/b/235755.html">书名</a></div>`
	v := Parse(item, `@onclick@js:result.match(/\('(.*?)', '', ''\)/)[1]`, ctx)
	if len(v) == 0 || v[0] != "/b/235755.html" {
		t.Fatalf("onclick@js 解析错误: %v", v)
	}
	t.Logf("onclick@js: %v", v)
	// 纯 @attr
	if v := Parse(item, "@onclick", ctx); len(v) == 0 || v[0] != "newWebView('/b/235755.html', '', '')" {
		t.Fatalf("@onclick 解析错误: %v", v)
	}
}
