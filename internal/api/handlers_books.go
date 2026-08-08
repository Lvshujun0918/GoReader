package api

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/reader-dev/internal/model"
	"github.com/Lvshujun0918/reader-dev/internal/service/bookfetch"
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
		Fail(c, "参数错误")
		return
	}
	title := paramOf(params, "durChapterTitle")
	index, _ := intParam(params, "durChapterIndex")
	pos, _ := intParam(params, "durChapterPos")
	ts, _ := intParam(params, "durChapterTime")
	if ts == 0 {
		ts = int64(0)
	}
	if err := a.Storage.UpdateBookProgress(ns, url, title, index, pos, ts); err != nil {
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

// handleGetBookInfo GET/POST /reader3/getBookInfo：书源书籍详情。
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
	origin := paramOf(params, "origin")
	if origin == "" {
		origin = paramOf(params, "bookSource")
	}
	if url == "" {
		Fail(c, "书源链接不能为空")
		return
	}
	// 书架已有则直接返回（合并书架字段；tocUrl 实时用 ruleBookInfo 重算——
	// 书架里可能是旧代码存错的详情 URL，直接返回会致目录只抓 1 章）
	if book, err := a.Storage.FindBook(ns, url); err == nil && book != nil {
		info := bookInfoFromBook(book)
		if source, serr := a.Storage.FindBookSource(ns, book.Origin); serr == nil && source != nil {
			if real, rerr := (bookfetch.New(a.Storage, ns, a.Solver)).BookInfo(source, url); rerr == nil {
				if toc, ok := real["tocUrl"].(string); ok && toc != "" {
					info["tocUrl"] = toc
				}
			}
		}
		OK(c, info)
		return
	}
	source, err := a.Storage.FindBookSource(ns, origin)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if source == nil {
		Fail(c, "未找到书源")
		return
	}
	fetcher := bookfetch.New(a.Storage, ns, a.Solver)
	info, err := fetcher.BookInfo(source, url)
	if err != nil {
		Fail(c, "获取书籍信息失败："+err.Error())
		return
	}
	OK(c, info)
}

// handleGetBookToc GET/POST /reader3/getBookToc：书源目录。
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
	origin := paramOf(params, "origin")
	if origin == "" {
		origin = paramOf(params, "bookSource")
	}
	if url == "" && tocURL == "" {
		Fail(c, "书源链接不能为空")
		return
	}
	// 目录缓存（5 分钟 TTL；key 用 tocURL）
	if cache, err := a.Storage.GetTocCache(tocURL); err == nil && cache != nil && cache.ChaptersJSON != "" {
		c.Data(200, "application/json; charset=utf-8", []byte(cache.ChaptersJSON))
		return
	}
	source, err := a.Storage.FindBookSource(ns, origin)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if source == nil {
		Fail(c, "未找到书源")
		return
	}
	fetcher := bookfetch.New(a.Storage, ns, a.Solver)
	chapters, _, err := fetcher.BookToc(source, tocURL)
	if err != nil {
		Fail(c, "获取目录失败："+err.Error())
		return
	}
	// 构造兼容输出
	type tocItem struct {
		Title    string `json:"title"`
		URL      string `json:"url"`
		IsVolume bool   `json:"isVolume"`
		Index    int    `json:"index"`
	}
	var items []tocItem
	for _, ch := range chapters {
		items = append(items, tocItem{Title: ch.Title, URL: ch.URL, IsVolume: ch.IsVolume, Index: ch.Index})
	}
	_ = a.Storage.SetTocCache(url, toAnySlice(items))
	OK(c, items)
}

// handleGetBookContent GET/POST /reader3/getBookContent：书源正文。
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
	origin := paramOf(params, "origin")
	if origin == "" {
		origin = paramOf(params, "bookSource")
	}
	if url == "" {
		Fail(c, "章节链接不能为空")
		return
	}
	// 章节缓存优先
	if ch, err := a.Storage.GetChapter(url, index); err == nil && ch != nil && ch.Content != "" {
		OK(c, map[string]any{"content": ch.Content, "chapterIndex": index, "title": ch.Title})
		return
	}
	source, err := a.Storage.FindBookSource(ns, origin)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if source == nil {
		Fail(c, "未找到书源")
		return
	}
	fetcher := bookfetch.New(a.Storage, ns, a.Solver)
	res, err := fetcher.BookContent(source, url, 10)
	if err != nil {
		Fail(c, "获取正文失败："+err.Error())
		return
	}
	// 写入缓存
	_ = a.Storage.SaveChapter(url, index, "", res.Content)
	OK(c, map[string]any{"content": res.Content, "chapterIndex": index, "wordCount": res.WordCount})
}

// handleGetChapterListByRule GET/POST /reader3/getChapterListByRule。
func (a *API) handleGetChapterListByRule(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	url := paramOf(params, "url")
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	source, err := a.Storage.FindBookSource(ns, paramOf(params, "origin"))
	if err != nil || source == nil {
		Fail(c, "未找到书源")
		return
	}
	fetcher := bookfetch.New(a.Storage, ns, a.Solver)
	chapters, _, err := fetcher.BookToc(source, url)
	if err != nil {
		Fail(c, "获取目录失败："+err.Error())
		return
	}
	OK(c, chapters)
}

// handleMigrateLocBook / refreshLocalBook / importBookPreview / uploadLocalBook：本地书占位（迭代实现）。
func (a *API) handleMigrateLocBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "功能实现中")
}

func (a *API) handleRefreshLocalBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "功能实现中")
}

func (a *API) handleImportBookPreview(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "功能实现中")
}

func (a *API) handleUploadLocalBook(c *gin.Context) {
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
