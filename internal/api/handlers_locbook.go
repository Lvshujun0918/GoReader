package api

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/GoReader/internal/model"
	"github.com/Lvshujun0918/GoReader/internal/service/localbook"
	"github.com/Lvshujun0918/GoReader/internal/storage"
)

// readAllLimited 读取不超过 limit 字节的全部内容。
func readAllLimited(r io.Reader, limit int64) ([]byte, error) {
	data, err := io.ReadAll(io.LimitReader(r, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(data)) > limit {
		return nil, fmt.Errorf("文件过大：超过上传大小上限")
	}
	return data, nil
}

/* ------------------------- 本地书 URL 约定 -------------------------
 * bookURL     = loc://{bookID}            （Book 主键 / toc 缓存 key）
 * 章节 url    = loc://{bookID}@{index}    （getBookToc 输出 / getBookContent 入参）
 * 原始文件    = <StorageDir>/data/{ns}/localBooks/{bookID}/{原文件名}
 */

const locBookPrefix = "loc://"

// localBookDir 返回某本地书的文件目录。
func (a *API) localBookDir(ns, bookID string) string {
	return filepath.Join(a.Config.StorageDir(), "data", ns, "localBooks", bookID)
}

// locChapterURL 章节 url。
func locChapterURL(bookURL string, index int64) string {
	return bookURL + "@" + strconv.FormatInt(index, 10)
}

// parseLocChapterURL 解析章节 url → (bookURL, index)。
func parseLocChapterURL(url string) (string, int64, bool) {
	if !strings.HasPrefix(url, locBookPrefix) {
		return "", 0, false
	}
	at := strings.LastIndex(url, "@")
	if at < 0 {
		return url, 0, true
	}
	idx, err := strconv.ParseInt(url[at+1:], 10, 64)
	if err != nil {
		return "", 0, false
	}
	return url[:at], idx, true
}

// tocItem 本地书目录项（legado 兼容字段）。
type locTocItem struct {
	Title    string `json:"title"`
	URL      string `json:"url"`
	IsVolume bool   `json:"isVolume"`
	Index    int    `json:"index"`
}

// rebuildLocChapters 缓存被清后从原文件重解析整书，恢复章节（返回章节列表）。
func (a *API) rebuildLocChapters(ns string, b *model.Book) ([]model.BookChapter, error) {
	if b.LocalFile == "" {
		return nil, fmt.Errorf("本地书文件路径缺失")
	}
	data, err := os.ReadFile(b.LocalFile)
	if err != nil {
		return nil, err
	}
	rules, err := a.Storage.ListTxtTocRules(ns)
	if err != nil {
		rules = nil
	}
	parsed, err := localbook.Parse(data, b.LocalFile, localbook.TocRulesFromModel(rules))
	if err != nil {
		return nil, err
	}
	chapters := make([]model.BookChapter, 0, len(parsed.Chapters))
	for i, ch := range parsed.Chapters {
		chapters = append(chapters, model.BookChapter{
			BookURL:      b.BookURL,
			ChapterIndex: int64(i),
			Title:        ch.Title,
			Content:      ch.Content,
		})
	}
	for _, ch := range chapters {
		_ = a.Storage.SaveChapter(ch.BookURL, ch.ChapterIndex, ch.Title, ch.Content)
	}
	return chapters, nil
}

// locTocOf 由章节列表构造目录输出。
func locTocOf(bookURL string, chapters []model.BookChapter) []locTocItem {
	items := make([]locTocItem, 0, len(chapters))
	for i, ch := range chapters {
		items = append(items, locTocItem{
			Title: ch.Title,
			URL:   locChapterURL(bookURL, int64(i)),
			Index: i,
		})
	}
	return items
}

// resolveLocToc 返回本地书目录（缓存优先 → 章节表 → 文件重解析）。
func (a *API) resolveLocToc(ns string, b *model.Book) ([]locTocItem, error) {
	if cache, err := a.Storage.GetTocCache(b.BookURL); err == nil && cache != nil && cache.ChaptersJSON != "" {
		// 缓存内容直接透传
		var items []locTocItem
		if err := json.Unmarshal([]byte(cache.ChaptersJSON), &items); err == nil {
			return items, nil
		}
	}
	chapters, err := a.Storage.ListChapters(b.BookURL)
	if err != nil || len(chapters) == 0 {
		chapters, err = a.rebuildLocChapters(ns, b)
		if err != nil {
			return nil, err
		}
	}
	items := locTocOf(b.BookURL, chapters)
	_ = a.Storage.SetTocCache(b.BookURL, items)
	return items, nil
}

// handleImportBookPreview POST /reader3/importBookPreview：解析预览（不保存）。
func (a *API) handleImportBookPreview(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	file, err := c.FormFile("file")
	if err != nil {
		Fail(c, "未收到文件")
		return
	}
	if file.Size > a.Config.UploadMaxBytes() {
		Fail(c, "文件过大：超过上传大小上限")
		return
	}
	rc, err := file.Open()
	if err != nil {
		Fail(c, "读取文件失败")
		return
	}
	defer rc.Close()
	data, err := readAllLimited(rc, a.Config.UploadMaxBytes())
	if err != nil {
		Fail(c, "读取文件失败")
		return
	}
	rules, _ := a.Storage.ListTxtTocRules(ns)
	parsed, err := localbook.Parse(data, file.Filename, localbook.TocRulesFromModel(rules))
	if err != nil {
		Fail(c, err.Error())
		return
	}
	// 预览章节（前 5 章标题）
	type previewChapter struct {
		Title string `json:"title"`
	}
	preview := make([]previewChapter, 0, len(parsed.Chapters))
	for i := 0; i < len(parsed.Chapters) && i < 5; i++ {
		preview = append(preview, previewChapter{Title: parsed.Chapters[i].Title})
	}
	OK(c, map[string]any{
		"name":         parsed.Name,
		"author":       parsed.Author,
		"format":       parsed.Format,
		"charset":      parsed.Charset,
		"chapterCount": len(parsed.Chapters),
		"wordCount":    totalWords(parsed),
		"chapters":     preview,
	})
}

// handleUploadLocalBook POST /reader3/uploadLocalBook：导入本地书（保存文件 + 书架 + 章节）。
func (a *API) handleUploadLocalBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	file, err := c.FormFile("file")
	if err != nil {
		Fail(c, "未收到文件")
		return
	}
	if file.Size > a.Config.UploadMaxBytes() {
		Fail(c, "文件过大：超过上传大小上限")
		return
	}
	rc, err := file.Open()
	if err != nil {
		Fail(c, "读取文件失败")
		return
	}
	defer rc.Close()
	data, err := readAllLimited(rc, a.Config.UploadMaxBytes())
	if err != nil {
		Fail(c, "读取文件失败")
		return
	}
	rules, _ := a.Storage.ListTxtTocRules(ns)
	parsed, err := localbook.Parse(data, file.Filename, localbook.TocRulesFromModel(rules))
	if err != nil {
		Fail(c, err.Error())
		return
	}

	bookID := randomUUID()
	bookURL := locBookPrefix + bookID
	dir := a.localBookDir(ns, bookID)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		Fail(c, "系统错误")
		return
	}
	filePath := filepath.Join(dir, filepath.Base(file.Filename))
	if err := os.WriteFile(filePath, data, 0o644); err != nil {
		Fail(c, "保存文件失败")
		return
	}

	latest := ""
	if n := len(parsed.Chapters); n > 0 {
		latest = parsed.Chapters[n-1].Title
	}
	b := &model.Book{
		BookURL:            bookURL,
		Name:               parsed.Name,
		Author:             parsed.Author,
		Origin:             "loc_book",
		OriginName:         "本地书",
		TocURL:             bookURL,
		Charset:            parsed.Charset,
		Type:               0,
		LatestChapterTitle: latest,
		TotalChapterNum:    int64(len(parsed.Chapters)),
		WordCount:          strconv.FormatInt(totalWords(parsed), 10),
		LocalFile:          filePath,
		LocalFileSize:      file.Size,
		LocalFileMtime:     storage.NowMillis(),
	}
	if err := a.Storage.SaveBook(ns, b); err != nil {
		Fail(c, "系统错误")
		return
	}
	for i, ch := range parsed.Chapters {
		if err := a.Storage.SaveChapter(bookURL, int64(i), ch.Title, ch.Content); err != nil {
			Fail(c, "系统错误")
			return
		}
	}
	_ = a.Storage.SetTocCache(bookURL, locTocOf(bookURL, chaptersOf(bookURL, parsed)))

	OK(c, map[string]any{
		"bookUrl":      bookURL,
		"name":         parsed.Name,
		"author":       parsed.Author,
		"format":       parsed.Format,
		"charset":      parsed.Charset,
		"chapterCount": len(parsed.Chapters),
		"wordCount":    totalWords(parsed),
	})
}

// chaptersOf 解析结果 → BookChapter 列表。
func chaptersOf(bookURL string, parsed *localbook.Book) []model.BookChapter {
	out := make([]model.BookChapter, 0, len(parsed.Chapters))
	for i, ch := range parsed.Chapters {
		out = append(out, model.BookChapter{BookURL: bookURL, ChapterIndex: int64(i), Title: ch.Title, Content: ch.Content})
	}
	return out
}

// totalWords 全书总字数。
func totalWords(parsed *localbook.Book) int64 {
	var n int64
	for _, ch := range parsed.Chapters {
		n += int64(len([]rune(ch.Content)))
	}
	return n
}

// handleRefreshLocalBook POST /reader3/refreshLocalBook：重解析本地书原文件刷新章节。
func (a *API) handleRefreshLocalBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	url := paramOf(params, "url")
	if url == "" {
		url = paramOf(params, "bookUrl")
	}
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	b, err := a.Storage.FindBook(ns, url)
	if err != nil || b == nil {
		Fail(c, "未找到本地书")
		return
	}
	if b.Origin != "loc_book" && !strings.HasPrefix(b.BookURL, locBookPrefix) {
		Fail(c, "非本地书")
		return
	}
	// 删除旧章节后重解析
	_ = a.Storage.DeleteBookCache(b.BookURL)
	chapters, err := a.rebuildLocChapters(ns, b)
	if err != nil {
		Fail(c, "重新解析失败："+err.Error())
		return
	}
	_ = a.Storage.SetTocCache(b.BookURL, locTocOf(b.BookURL, chapters))
	OK(c, map[string]any{
		"bookUrl":      b.BookURL,
		"chapterCount": len(chapters),
	})
}
