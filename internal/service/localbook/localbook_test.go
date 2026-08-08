package localbook

import (
	"archive/zip"
	"bytes"
	"testing"
)

func TestParseTxtUTF8(t *testing.T) {
	content := "第一章 初入江湖\n\n张三丰正在打坐。\n\n第二章 奇遇\n\n山崖下传来哭声。\n\n第三章 下山\n\n他决定下山历练。\n"
	book, err := Parse([]byte(content), "书.txt", nil)
	if err != nil {
		t.Fatal(err)
	}
	if book.Format != "txt" || book.Charset != "utf-8" {
		t.Fatalf("format/charset 不符: %s/%s", book.Format, book.Charset)
	}
	if len(book.Chapters) != 3 {
		t.Fatalf("应 3 章，实际 %d", len(book.Chapters))
	}
	if book.Chapters[0].Title != "第一章 初入江湖" {
		t.Errorf("第 1 章标题不符: %q", book.Chapters[0].Title)
	}
	if !bytes.Contains([]byte(book.Chapters[1].Content), []byte("山崖下")) {
		t.Errorf("第 2 章内容不符: %q", book.Chapters[1].Content)
	}
}

func TestParseTxtGBK(t *testing.T) {
	// GBK 编码的 "第一章 测试\n这是正文内容。\n第二章 继续\n更多内容。\n"
	gbk := []byte{
		0xB5, 0xDA, 0xD2, 0xBB, 0xD5, 0xC2, 0x20, 0xB2, 0xE2, 0xCA, 0xD4, 0x0A,
		0xD5, 0xE2, 0xCA, 0xC7, 0xD5, 0xFD, 0xCE, 0xC4, 0xC4, 0xDA, 0xC8, 0xDD, 0xA1, 0xA3, 0x0A,
		0xB5, 0xDA, 0xB6, 0xFE, 0xD5, 0xC2, 0x20, 0xBC, 0xCC, 0xD0, 0xF8, 0x0A,
		0xB8, 0xFC, 0xB6, 0xE0, 0xC4, 0xDA, 0xC8, 0xDD, 0xA1, 0xA3, 0x0A,
	}
	book, err := Parse(gbk, "gbk书.txt", nil)
	if err != nil {
		t.Fatal(err)
	}
	if book.Charset != "gbk" {
		t.Fatalf("应检测为 gbk，实际 %s", book.Charset)
	}
	if len(book.Chapters) != 2 {
		t.Fatalf("应 2 章，实际 %d", len(book.Chapters))
	}
	if book.Chapters[0].Title != "第一章 测试" {
		t.Errorf("标题解码错误: %q", book.Chapters[0].Title)
	}
	if !bytes.Contains([]byte(book.Chapters[1].Content), []byte("更多内容")) {
		t.Errorf("GBK 正文解码错误: %q", book.Chapters[1].Content)
	}
}

func TestParseTxtNoTitleWholeBook(t *testing.T) {
	content := "这是没有任何章节标题的书。\n只有一段文字。\n"
	book, err := Parse([]byte(content), "无目录.txt", nil)
	if err != nil {
		t.Fatal(err)
	}
	if len(book.Chapters) != 1 {
		t.Fatalf("应整书 1 章，实际 %d", len(book.Chapters))
	}
	if book.Chapters[0].Title != "全文" {
		t.Errorf("标题应为「全文」: %q", book.Chapters[0].Title)
	}
	if len(book.Chapters[0].Content) < 10 {
		t.Errorf("整书内容过短: %q", book.Chapters[0].Content)
	}
}

func TestParseEpub(t *testing.T) {
	// 构造最小 epub：container.xml + OPF + 两章 xhtml
	var buf bytes.Buffer
	zw := zip.NewWriter(&buf)
	mustZip := func(name, content string) {
		w, err := zw.Create(name)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := w.Write([]byte(content)); err != nil {
			t.Fatal(err)
		}
	}
	mustZip("mimetype", "application/epub+zip")
	mustZip("META-INF/container.xml", `<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>`)
	mustZip("OEBPS/content.opf", `<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>测试小说</dc:title>
    <dc:creator>测试作者</dc:creator>
  </metadata>
  <manifest>
    <item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="chapter2.xhtml" media-type="application/xhtml+xml"/>
    <item id="img" href="cover.jpg" media-type="image/jpeg"/>
  </manifest>
  <spine><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>`)
	mustZip("OEBPS/chapter1.xhtml", `<html><head><title>第一章 相遇</title></head><body><h1>第一章 相遇</h1><p>雨夜，<b>两人</b>在桥下相遇。</p><p>第二段。</p></body></html>`)
	mustZip("OEBPS/chapter2.xhtml", `<html><head><title>第二章 别离</title></head><body><h1>第二章 别离</h1><p>黎明前，他转身离开。</p></body></html>`)
	if err := zw.Close(); err != nil {
		t.Fatal(err)
	}

	book, err := Parse(buf.Bytes(), "测试.epub", nil)
	if err != nil {
		t.Fatal(err)
	}
	if book.Format != "epub" {
		t.Fatalf("format 不符: %s", book.Format)
	}
	if book.Name != "测试小说" || book.Author != "测试作者" {
		t.Errorf("元数据不符: %q / %q", book.Name, book.Author)
	}
	if len(book.Chapters) != 2 {
		t.Fatalf("应 2 章，实际 %d", len(book.Chapters))
	}
	if book.Chapters[0].Title != "第一章 相遇" {
		t.Errorf("第 1 章标题不符: %q", book.Chapters[0].Title)
	}
	if !bytes.Contains([]byte(book.Chapters[0].Content), []byte("两人")) {
		t.Errorf("HTML 标签未去除干净: %q", book.Chapters[0].Content)
	}
	if !bytes.Contains([]byte(book.Chapters[1].Content), []byte("转身离开")) {
		t.Errorf("第 2 章内容不符: %q", book.Chapters[1].Content)
	}
}

func TestParseUnsupported(t *testing.T) {
	if _, err := Parse([]byte("x"), "a.pdf", nil); err == nil {
		t.Fatal("pdf 应报不支持")
	}
}
