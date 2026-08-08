package rule

import (
	"strings"
	"testing"
)

func TestEvalCSS(t *testing.T) {
	html := `<html><body>
		<ul class="book-list">
			<li><a href="/book/1">书名一</a><span class="author">作者一</span></li>
			<li><a href="/book/2">书名二</a><span class="author">作者二</span></li>
		</ul>
	</body></html>`
	ctx := &Context{BaseURL: "https://example.com"}

	// 列表选择器
	items := Parse(html, "@css:ul.book-list li", ctx)
	if len(items) != 2 {
		t.Fatalf("期望 2 个 li，got %d: %v", len(items), items)
	}
	// 属性提取
	hrefs := Parse(html, "@css:a@href", ctx)
	if len(hrefs) != 2 || hrefs[0] != "/book/1" {
		t.Fatalf("href 提取失败: %v", hrefs)
	}
	// 文本
	names := Parse(html, "@css:.author", ctx)
	if len(names) != 2 || !strings.Contains(names[0], "作者一") {
		t.Fatalf("文本提取失败: %v", names)
	}
}

func TestEvalXPath(t *testing.T) {
	html := `<html><body><div class="content"><p>段落一</p><p>段落二</p></div></body></html>`
	out := Parse(html, "@xpath://div[@class='content']/p", nil)
	if len(out) != 2 || !strings.Contains(out[0], "段落一") {
		t.Fatalf("XPath 解析失败: %v", out)
	}
}

func TestEvalJSON(t *testing.T) {
	input := `{"data":{"books":[{"name":"书A","author":"甲"},{"name":"书B","author":"乙"}]}}`
	out := Parse(input, "@json:$.data.books[*].name", nil)
	if len(out) != 2 || out[0] != "书A" {
		t.Fatalf("JSONPath 解析失败: %v", out)
	}
}

func TestEvalRegex(t *testing.T) {
	input := "第1章 开始\n第2章 发展\n第3章 结束"
	out := Parse(input, "@regex:第(\\d+)章", nil)
	if len(out) != 3 || out[0] != "1" {
		t.Fatalf("正则捕获组解析失败: %v", out)
	}
}

func TestPlainRegexFallback(t *testing.T) {
	// 纯文本规则当正则
	out := Parse("abc123def456", `\d+`, nil)
	if len(out) == 0 || out[0] != "123" {
		t.Fatalf("纯文本正则回退失败: %v", out)
	}
}

func TestInterpolateVars(t *testing.T) {
	ctx := &Context{}
	ctx.Set("key", "修真")
	got := interpolateVars("", "@css:{key}.list", ctx)
	if got != "@css:修真.list" {
		t.Fatalf("变量插值失败: %q", got)
	}
	// {{$.path}} 从输入 JSON 提取（legado 双花括号 JSONPath 插值）
	got2 := interpolateVars(`{"book_id":"123","title":"书"}`, "/book/{{$.book_id}}", ctx)
	if got2 != "/book/123" {
		t.Fatalf("JSONPath 插值失败: %q", got2)
	}
}

func TestSplitTopLevel(t *testing.T) {
	parts := splitTopLevel("a||b(||)c||d", "||")
	if len(parts) != 3 {
		t.Fatalf("顶层分隔失败: %v", parts)
	}
}

func TestRegexReplace(t *testing.T) {
	out := processTextOps("<p>hello</p>world", "##<[^>]+>##", nil)
	if len(out) != 1 || out[0] != "helloworld" {
		t.Fatalf("正则替换失败: %v", out)
	}
}
