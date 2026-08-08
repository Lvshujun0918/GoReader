package api

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/GoReader/internal/model"
)

// handleGetBookshelf GET /reader3/getBookshelf。
func (a *API) handleGetBookshelf(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	books, err := a.Storage.ListBooks(ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, books)
}

// handleGetShelfBook GET/POST /reader3/getShelfBook：书架单书。
func (a *API) handleGetShelfBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	url := paramOf(a.params(c), "url")
	if url == "" {
		Fail(c, "书源链接不能为空")
		return
	}
	book, err := a.Storage.FindBook(ns, url)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if book == nil {
		Fail(c, "书籍不存在")
		return
	}
	OK(c, book)
}

// handleSaveBook POST /reader3/saveBook：保存/更新书架书。
func (a *API) handleSaveBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	var book model.Book
	if err := c.ShouldBindJSON(&book); err != nil {
		Fail(c, "参数错误")
		return
	}
	if book.BookURL == "" {
		Fail(c, "书源链接不能为空")
		return
	}
	if err := a.Storage.SaveBook(ns, &book); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleDeleteBook POST /reader3/deleteBook。
func (a *API) handleDeleteBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	url := paramOf(a.params(c), "bookUrl")
	if url == "" {
		url = paramOf(a.params(c), "url")
	}
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.DeleteBook(ns, url); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleDeleteBooks POST /reader3/deleteBooks。
func (a *API) handleDeleteBooks(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	var urls []string
	if arr, ok := params["bookUrls"].([]any); ok {
		for _, v := range arr {
			if s, ok := v.(string); ok {
				urls = append(urls, s)
			}
		}
	}
	if len(urls) == 0 {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.DeleteBooks(ns, urls); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleSaveBookProgress POST /reader3/saveBookProgress。
func (a *API) handleSaveBookProgress(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	url := paramOf(params, "bookUrl")
	if url == "" {
		url = paramOf(params, "url")
	}
	if url == "" {
		// 进度同步属可忽略调用（阅读页卸载时前端偶发空 url）——静默成功，不打扰用户
		OK(c, nil)
		return
	}
	title := paramOf(params, "durChapterTitle")
	index, _ := intParam(params, "durChapterIndex")
	pos, _ := intParam(params, "durChapterPos")
	ts, _ := intParam(params, "durChapterTime")
	totalNum, _ := intParam(params, "totalChapterNum")
	if ts == 0 {
		ts = int64(0)
	}
	if err := a.Storage.UpdateBookProgress(ns, url, title, index, pos, ts, totalNum); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleSaveBookContent POST /reader3/saveBookContent：缓存章节正文。
func (a *API) handleSaveBookContent(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	params := a.params(c)
	url := paramOf(params, "bookUrl")
	index, _ := intParam(params, "chapterIndex")
	title := paramOf(params, "title")
	content := paramOf(params, "content")
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.SaveChapter(url, index, title, content); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleGetBookInfo GET/POST /reader3/getBookInfo：书籍详情（书架书）。
func (a *API) handleGetBookInfo(c *gin.Context) {
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
		Fail(c, "书源链接不能为空")
		return
	}
	book, err := a.Storage.FindBook(ns, url)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if book == nil {
		Fail(c, "书籍不存在")
		return
	}
	OK(c, bookInfoFromBook(book))
}

// handleGetBookToc GET/POST /reader3/getBookToc：目录（本地书）。
func (a *API) handleGetBookToc(c *gin.Context) {
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
	tocURL := paramOf(params, "tocUrl")
	if tocURL == "" {
		tocURL = url
	}
	if url == "" && tocURL == "" {
		Fail(c, "书源链接不能为空")
		return
	}
	// 本地书（loc://bookID）目录：缓存优先 → 章节表 → 文件重解析
	if strings.HasPrefix(tocURL, locBookPrefix) {
		b, err := a.Storage.FindBook(ns, tocURL)
		if err == nil && b != nil {
			items, err := a.resolveLocToc(ns, b)
			if err != nil {
				Fail(c, "获取目录失败："+err.Error())
				return
			}
			OK(c, items)
			return
		}
	}
	// 目录缓存（5 分钟 TTL；key 用 tocURL）
	if cache, err := a.Storage.GetTocCache(tocURL); err == nil && cache != nil && cache.ChaptersJSON != "" {
		c.Data(200, "application/json; charset=utf-8", []byte(cache.ChaptersJSON))
		return
	}
	Fail(c, "未找到本地书")
}

// handleGetBookContent GET/POST /reader3/getBookContent：正文（本地书）。
func (a *API) handleGetBookContent(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	url := paramOf(params, "url")
	if url == "" {
		url = paramOf(params, "chapterUrl")
	}
	index, _ := intParam(params, "chapterIndex")
	if url == "" {
		Fail(c, "章节链接不能为空")
		return
	}
	// 本地书章节（loc://bookID@index）：读章节缓存；缓存被清则从原文件重解析恢复
	if bookURL, chIndex, isLoc := parseLocChapterURL(url); isLoc {
		ch, err := a.Storage.GetChapter(bookURL, chIndex)
		if err == nil && ch != nil && ch.Content != "" {
			OK(c, map[string]any{
				"content": ch.Content, "chapterIndex": chIndex, "title": ch.Title,
				"chapterWordCount": len([]rune(ch.Content)),
			})
			return
		}
		b, err := a.Storage.FindBook(ns, bookURL)
		if err == nil && b != nil {
			chapters, rerr := a.rebuildLocChapters(ns, b)
			if rerr == nil && chIndex >= 0 && int(chIndex) < len(chapters) {
				ch = &chapters[chIndex]
				OK(c, map[string]any{
					"content": ch.Content, "chapterIndex": chIndex, "title": ch.Title,
					"chapterWordCount": len([]rune(ch.Content)),
				})
				return
			}
			Fail(c, "获取正文失败：章节不存在")
			return
		}
		Fail(c, "未找到本地书")
		return
	}
	// 章节缓存优先（本地书非 loc:// 章节 URL）
	if ch, err := a.Storage.GetChapter(url, index); err == nil && ch != nil && ch.Content != "" {
		OK(c, map[string]any{"content": ch.Content, "chapterIndex": index, "title": ch.Title})
		return
	}
	Fail(c, "未找到本地书")
}

// handleSearchBookContent GET/POST /reader3/searchBookContent：全书搜索（缓存章节）。
func (a *API) handleSearchBookContent(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	bookURL := paramOf(params, "bookUrl")
	key := paramOf(params, "key")
	if bookURL == "" || key == "" {
		Fail(c, "参数错误")
		return
	}
	_ = ns
	chapters, err := a.Storage.ListChapters(bookURL)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	type hit struct {
		ChapterIndex int64  `json:"chapterIndex"`
		Title        string `json:"title"`
		Snippet      string `json:"snippet"`
	}
	var hits []hit
	keyLower := strings.ToLower(key)
	for _, ch := range chapters {
		lower := strings.ToLower(ch.Content)
		idx := strings.Index(lower, keyLower)
		if idx < 0 {
			continue
		}
		start := idx - 30
		if start < 0 {
			start = 0
		}
		end := idx + len(key) + 30
		if end > len(ch.Content) {
			end = len(ch.Content)
		}
		snippet := ch.Content[start:end]
		snippet = strings.ReplaceAll(snippet, "\n", " ")
		hits = append(hits, hit{ChapterIndex: ch.ChapterIndex, Title: ch.Title, Snippet: snippet})
	}
	OK(c, hits)
}

// handleMigrateLocBook：legacy 本地书迁移（迭代实现）。
func (a *API) handleMigrateLocBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "功能实现中")
}

// bookInfoFromBook 书架书 → 详情输出。
func bookInfoFromBook(b *model.Book) map[string]any {
	return map[string]any{
		"bookUrl":            b.BookURL,
		"name":               b.Name,
		"author":             b.Author,
		"kind":               b.Kind,
		"intro":              b.Intro,
		"coverUrl":           b.CoverURL,
		"tocUrl":             b.TocURL,
		"wordCount":          b.WordCount,
		"latestChapterTitle": b.LatestChapterTitle,
		"origin":             b.Origin,
		"originName":         b.OriginName,
		"type":               b.Type,
	}
}

// toAnySlice 转换任意切片为 []any（供目录缓存 JSON 序列化）。
func toAnySlice[T any](in []T) []any {
	out := make([]any, len(in))
	for i, v := range in {
		out[i] = v
	}
	return out
}

// helpers 供文件管理/搜索等复用
func parseURLHost(urlStr string) string {
	idx := strings.Index(urlStr, "://")
	if idx < 0 {
		return urlStr
	}
	rest := urlStr[idx+3:]
	if i := strings.IndexAny(rest, "/?"); i >= 0 {
		return rest[:i]
	}
	return rest
}

func parseInt64Default(s string, def int64) int64 {
	if s == "" {
		return def
	}
	n, err := strconv.ParseInt(s, 10, 64)
	if err != nil {
		return def
	}
	return n
}

func fmtString(v any) string {
	if v == nil {
		return ""
	}
	return fmt.Sprintf("%v", v)
}
