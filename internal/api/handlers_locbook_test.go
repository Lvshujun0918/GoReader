package api

import (
	"bytes"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"
)

// writeTestFile 写文件（测试辅助）。
func writeTestFile(path string, data []byte) error {
	return os.WriteFile(path, data, 0o644)
}

// performUpload 执行 multipart 文件上传（字段名 file）。
func performUpload(r http.Handler, path, filename string, data []byte) *httptest.ResponseRecorder {
	var buf bytes.Buffer
	mw := multipart.NewWriter(&buf)
	fw, _ := mw.CreateFormFile("file", filename)
	_, _ = fw.Write(data)
	_ = mw.Close()
	req := httptest.NewRequest(http.MethodPost, path, &buf)
	req.Header.Set("Content-Type", mw.FormDataContentType())
	w := httptest.NewRecorder()
	r.ServeHTTP(w, req)
	return w
}

const sampleTxt = "第一章 初入江湖\n\n张三丰正在打坐修炼。\n\n第二章 奇遇\n\n山崖下传来阵阵哭声。\n"

// TestUploadLocalBookTxt 上传 UTF-8 txt → 书架 → 目录 → 正文全链路。
func TestUploadLocalBookTxt(t *testing.T) {
	h := newTestAPI(t)

	// 上传
	w := performUpload(h, "/reader3/uploadLocalBook", "武侠小说.txt", []byte(sampleTxt))
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("上传失败: %s", rd.ErrorMsg)
	}
	m, _ := rd.Data.(map[string]any)
	bookURL, _ := m["bookUrl"].(string)
	if bookURL == "" || !bytes.HasPrefix([]byte(bookURL), []byte("loc://")) {
		t.Fatalf("bookUrl 不符: %v", m["bookUrl"])
	}
	if m["name"] != "武侠小说" {
		t.Errorf("书名不符: %v", m["name"])
	}
	if m["chapterCount"] != float64(2) {
		t.Errorf("章节数不符: %v", m["chapterCount"])
	}

	// 书架
	w = perform(h, "GET", "/reader3/getBookshelf", nil)
	rd = parseReturn(t, w)
	list, _ := rd.Data.([]any)
	found := false
	for _, it := range list {
		b, _ := it.(map[string]any)
		if b["bookUrl"] == bookURL && b["origin"] == "loc_book" {
			found = true
		}
	}
	if !found {
		t.Fatal("书架中未找到导入的本地书")
	}

	// 目录
	w = perform(h, "GET", "/reader3/getBookToc?tocUrl="+bookURL+"&origin=loc_book", nil)
	rd = parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("getBookToc 失败: %s", rd.ErrorMsg)
	}
	toc, _ := rd.Data.([]any)
	if len(toc) != 2 {
		t.Fatalf("目录应 2 章，实际 %d", len(toc))
	}
	first, _ := toc[0].(map[string]any)
	if first["title"] != "第一章 初入江湖" {
		t.Errorf("第 1 章标题不符: %v", first["title"])
	}
	chURL, _ := first["url"].(string)
	if chURL == "" {
		t.Fatal("章节 url 为空")
	}

	// 正文
	w = perform(h, "GET", "/reader3/getBookContent?chapterUrl="+chURL+"&bookSource=loc_book", nil)
	rd = parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("getBookContent 失败: %s", rd.ErrorMsg)
	}
	cm, _ := rd.Data.(map[string]any)
	content, _ := cm["content"].(string)
	if !bytes.Contains([]byte(content), []byte("张三丰")) {
		t.Errorf("正文不符: %q", content)
	}
}

// TestUploadLocalBookGBK 上传 GBK 编码 txt：编码检测 + 章节切分。
func TestUploadLocalBookGBK(t *testing.T) {
	h := newTestAPI(t)
	gbk := []byte{
		0xB5, 0xDA, 0xD2, 0xBB, 0xD5, 0xC2, 0x20, 0xB2, 0xE2, 0xCA, 0xD4, 0x0A,
		0xD5, 0xE2, 0xCA, 0xC7, 0xD5, 0xFD, 0xCE, 0xC4, 0xC4, 0xDA, 0xC8, 0xDD, 0xA1, 0xA3, 0x0A,
		0xB5, 0xDA, 0xB6, 0xFE, 0xD5, 0xC2, 0x20, 0xBC, 0xCC, 0xD0, 0xF8, 0x0A,
		0xB8, 0xFC, 0xB6, 0xE0, 0xC4, 0xDA, 0xC8, 0xDD, 0xA1, 0xA3, 0x0A,
	}
	w := performUpload(h, "/reader3/uploadLocalBook", "古文.txt", gbk)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("GBK 上传失败: %s", rd.ErrorMsg)
	}
	m, _ := rd.Data.(map[string]any)
	if m["charset"] != "gbk" {
		t.Errorf("应检测为 gbk: %v", m["charset"])
	}
	if m["chapterCount"] != float64(2) {
		t.Errorf("GBK 章节数不符: %v", m["chapterCount"])
	}

	// 目录第 1 章标题应正确解码
	bookURL, _ := m["bookUrl"].(string)
	w = perform(h, "GET", "/reader3/getBookToc?tocUrl="+bookURL+"&origin=loc_book", nil)
	rd = parseReturn(t, w)
	toc, _ := rd.Data.([]any)
	first, _ := toc[0].(map[string]any)
	if first["title"] != "第一章 测试" {
		t.Errorf("GBK 标题解码不符: %v", first["title"])
	}
}

// TestImportBookPreview 预览：不保存，返回元数据与章节。
func TestImportBookPreview(t *testing.T) {
	h := newTestAPI(t)
	w := performUpload(h, "/reader3/importBookPreview", "预览.txt", []byte(sampleTxt))
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("预览失败: %s", rd.ErrorMsg)
	}
	m, _ := rd.Data.(map[string]any)
	if m["name"] != "预览" || m["format"] != "txt" {
		t.Errorf("预览元数据不符: %v", m)
	}
	if m["chapterCount"] != float64(2) {
		t.Errorf("预览章节数不符: %v", m["chapterCount"])
	}
	chapters, _ := m["chapters"].([]any)
	if len(chapters) != 2 {
		t.Errorf("预览章节列表不符: %v", chapters)
	}

	// 预览不应产生书架书
	w = perform(h, "GET", "/reader3/getBookshelf", nil)
	rd = parseReturn(t, w)
	list, _ := rd.Data.([]any)
	if len(list) != 0 {
		t.Errorf("预览不应入书架，实际 %d 本", len(list))
	}
}

// TestUploadLocalBookUnsupported 不支持格式应报错。
func TestUploadLocalBookUnsupported(t *testing.T) {
	h := newTestAPI(t)
	w := performUpload(h, "/reader3/uploadLocalBook", "文档.pdf", []byte("%PDF-1.4"))
	rd := parseReturn(t, w)
	if rd.IsSuccess {
		t.Fatal("pdf 应拒绝")
	}
	if !bytes.Contains([]byte(rd.ErrorMsg), []byte("仅支持")) {
		t.Errorf("错误信息不符: %s", rd.ErrorMsg)
	}
}

// TestRefreshLocalBook 重新扫描：改文件后刷新章节。
func TestRefreshLocalBook(t *testing.T) {
	h := newTestAPI(t)
	w := performUpload(h, "/reader3/uploadLocalBook", "刷新.txt", []byte(sampleTxt))
	rd := parseReturn(t, w)
	m, _ := rd.Data.(map[string]any)
	bookURL, _ := m["bookUrl"].(string)

	// 从书架拿 localFile 路径，覆盖为新内容（3 章）
	w = perform(h, "GET", "/reader3/getBookshelf", nil)
	rd = parseReturn(t, w)
	list, _ := rd.Data.([]any)
	var localFile string
	for _, it := range list {
		b, _ := it.(map[string]any)
		if b["bookUrl"] == bookURL {
			localFile, _ = b["localFile"].(string)
		}
	}
	if localFile == "" {
		t.Fatal("书架未返回 localFile")
	}
	newContent := "第一章 甲\n内容甲\n第二章 乙\n内容乙\n第三章 丙\n内容丙\n"
	if err := writeTestFile(localFile, []byte(newContent)); err != nil {
		t.Fatal(err)
	}

	w = perform(h, "POST", "/reader3/refreshLocalBook", map[string]any{"url": bookURL})
	rd = parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("refreshLocalBook 失败: %s", rd.ErrorMsg)
	}
	if rd.Data.(map[string]any)["chapterCount"] != float64(3) {
		t.Errorf("刷新后章节数应为 3: %v", rd.Data)
	}

	// 目录应更新为 3 章
	w = perform(h, "GET", "/reader3/getBookToc?tocUrl="+bookURL+"&origin=loc_book", nil)
	rd = parseReturn(t, w)
	toc, _ := rd.Data.([]any)
	if len(toc) != 3 {
		t.Errorf("刷新后目录应 3 章，实际 %d", len(toc))
	}
}
