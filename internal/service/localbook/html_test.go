package localbook

import (
	"testing"
)

func TestParseHTML(t *testing.T) {
	doc := `<html><head><title>剑来</title></head><body>
<h1>第一章 惊蛰</h1><p>二月二，龙抬头。</p><p>暮色里。</p>
<h2>第二章 开门</h2><p>清晨。</p>
</body></html>`
	b, err := Parse([]byte(doc), "book.html", nil)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	if b.Name != "剑来" {
		t.Errorf("书名=%q", b.Name)
	}
	if b.Format != "html" {
		t.Errorf("format=%q", b.Format)
	}
	if len(b.Chapters) != 2 {
		t.Fatalf("章节数=%d", len(b.Chapters))
	}
	if b.Chapters[0].Title != "第一章 惊蛰" {
		t.Errorf("章0标题=%q", b.Chapters[0].Title)
	}
	if got := b.Chapters[0].Content; got != "二月二，龙抬头。\n暮色里。" {
		t.Errorf("章0内容=%q", got)
	}
	if b.Chapters[1].Title != "第二章 开门" {
		t.Errorf("章1标题=%q", b.Chapters[1].Title)
	}
}

func TestParseHTMLNoHeadings(t *testing.T) {
	doc := `<html><body><p>全文内容一段。</p><p>第二段。</p></body></html>`
	b, err := Parse([]byte(doc), "novel.htm", nil)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	if len(b.Chapters) != 1 || b.Chapters[0].Title != "全文" {
		t.Fatalf("应整书一章: %+v", b.Chapters)
	}
	if b.Chapters[0].Content != "全文内容一段。\n第二段。" {
		t.Errorf("内容=%q", b.Chapters[0].Content)
	}
}
