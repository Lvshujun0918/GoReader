package api

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/reader-dev/internal/model"
	"github.com/Lvshujun0918/reader-dev/internal/parser/rule"
	"github.com/Lvshujun0918/reader-dev/internal/service/bookfetch"
	"github.com/Lvshujun0918/reader-dev/internal/service/crawler"
)

// SearchResult 搜索结果（兼容 legacy SearchBook camelCase）。
type SearchResult struct {
	BookURL            string `json:"bookUrl"`
	Origin             string `json:"origin"`
	OriginName         string `json:"originName"`
	Type               int    `json:"type"`
	Name               string `json:"name"`
	Author             string `json:"author"`
	Kind               string `json:"kind"`
	CoverURL           string `json:"coverUrl"`
	Intro              string `json:"intro"`
	WordCount          string `json:"wordCount"`
	LatestChapterTitle string `json:"latestChapterTitle"`
	TocURL             string `json:"tocUrl"`
	Time               int64  `json:"time"`
}

// SearchSource 单书源搜索。
func SearchSource(src *model.BookSource, keyword string) ([]*SearchResult, error) {
	ruleStr := src.RuleSearch
	if ruleStr == "" {
		ruleStr = src.SearchRule
	}
	if ruleStr == "" {
		return nil, fmt.Errorf("书源无搜索规则")
	}
	// 构造搜索 URL（{key} 变量替换）
	searchURL := src.SearchURL
	searchURL = strings.ReplaceAll(searchURL, "{key}", urlEncodeKeyword(keyword))
	searchURL = strings.ReplaceAll(searchURL, "{searchKey}", urlEncodeKeyword(keyword))

	client := crawler.New(nil, "")
	body, err := client.FetchWithHeaders(searchURL, nil)
	if err != nil {
		return nil, err
	}
	htmlStr := string(body)
	ctx := &rule.Context{BaseURL: searchURL}

	var results []*SearchResult
	// 尝试列表+字段成对规则
	for _, item := range splitSemicolonRule(ruleStr) {
		key, val := splitKeyVal(item)
		switch key {
		case "bookList", "books":
			items := rule.Parse(htmlStr, val, ctx)
			results = make([]*SearchResult, 0, len(items))
			for range items {
				results = append(results, &SearchResult{})
			}
			// 标题/作者/链接成对规则（第二次遍历填充）
			fillSearchFields(htmlStr, ruleStr, results, ctx, src, searchURL)
			return results, nil
		case "name":
			// 无列表规则：单书
			res := &SearchResult{}
			fillSearchFields(htmlStr, ruleStr, []*SearchResult{res}, ctx, src, searchURL)
			if res.Name != "" {
				return []*SearchResult{res}, nil
			}
		}
	}
	// 无 ruleSearch 时的兜底
	return nil, nil
}

// fillSearchFields 填充搜索结果字段（列表规则或单书规则）。
func fillSearchFields(htmlStr, ruleStr string, results []*SearchResult, ctx *rule.Context, src *model.BookSource, baseURL string) {
	items := []string{}
	if len(results) > 1 {
		for _, item := range splitSemicolonRule(ruleStr) {
			key, val := splitKeyVal(item)
			if key == "bookList" || key == "books" {
				items = rule.Parse(htmlStr, val, ctx)
				break
			}
		}
	} else {
		items = []string{htmlStr}
	}
	for i, item := range items {
		if i >= len(results) {
			break
		}
		res := results[i]
		for _, sub := range splitSemicolonRule(ruleStr) {
			key, val := splitKeyVal(sub)
			switch key {
			case "name", "bookName":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					res.Name = v[0]
				}
			case "author", "bookAuthor":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					res.Author = v[0]
				}
			case "bookUrl", "detailUrl":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					res.BookURL = resolveURL(v[0], baseURL)
					res.TocURL = res.BookURL
				}
			case "coverUrl":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					res.CoverURL = resolveURL(v[0], baseURL)
				}
			case "intro", "desc":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					res.Intro = v[0]
				}
			case "kind":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					res.Kind = v[0]
				}
			case "wordCount":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					res.WordCount = v[0]
				}
			case "latestChapterTitle":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					res.LatestChapterTitle = v[0]
				}
			}
		}
		res.Origin = src.BookSourceURL
		res.OriginName = src.BookSourceName
		res.Type = src.BookSourceType
	}
}

// handleSearchBook GET/POST /reader3/searchBook：单书源搜索。
func (a *API) handleSearchBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	keyword := paramOf(params, "key")
	origin := paramOf(params, "origin")
	if keyword == "" {
		Fail(c, "搜索关键词不能为空")
		return
	}
	src, err := a.Storage.FindBookSource(ns, origin)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if src == nil {
		Fail(c, "未找到书源")
		return
	}
	results, err := SearchSource(src, keyword)
	if err != nil {
		Fail(c, "搜索失败："+err.Error())
		return
	}
	OK(c, results)
}

// handleSearchBookMulti GET/POST /reader3/searchBookMulti：多书源搜索。
func (a *API) handleSearchBookMulti(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	keyword := paramOf(params, "key")
	if keyword == "" {
		Fail(c, "搜索关键词不能为空")
		return
	}
	sources, err := a.Storage.ListBookSources(ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	limit := 5
	if v, ok := intParam(params, "limit"); ok && v > 0 {
		limit = int(v)
	}
	var all []*SearchResult
	count := 0
	for _, src := range sources {
		if count >= limit {
			break
		}
		if src.Enabled != 1 || src.SearchURL == "" {
			continue
		}
		if results, err := SearchSource(&src, keyword); err == nil {
			all = append(all, results...)
		}
		count++
	}
	OK(c, all)
}

// handleSearchBookMultiSSE GET/POST /reader3/searchBookMultiSSE：SSE 流式搜索。
func (a *API) handleSearchBookMultiSSE(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	keyword := paramOf(params, "key")
	if keyword == "" {
		Fail(c, "搜索关键词不能为空")
		return
	}
	sources, err := a.Storage.ListBookSources(ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	c.Header("Content-Type", "text/event-stream")
	c.Header("Cache-Control", "no-cache")
	c.Header("Connection", "keep-alive")
	flusher, _ := c.Writer.(gin.ResponseWriter)
	_ = flusher

	for _, src := range sources {
		if src.Enabled != 1 || src.SearchURL == "" {
			continue
		}
		results, err := SearchSource(&src, keyword)
		if err != nil {
			continue
		}
		b, _ := json.Marshal(results)
		fmt.Fprintf(c.Writer, "data: %s\n\n", b)
		c.Writer.Flush()
	}
	fmt.Fprint(c.Writer, "data: [DONE]\n\n")
}

// handleSearchBookSource GET/POST /reader3/searchBookSource：书源内搜索（别名）。
func (a *API) handleSearchBookSource(c *gin.Context) {
	a.handleSearchBook(c)
}

// handleSearchBookSourceSSE GET/POST /reader3/searchBookSourceSSE。
func (a *API) handleSearchBookSourceSSE(c *gin.Context) {
	a.handleSearchBookMultiSSE(c)
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

// handleGetChapterListByRule 已在 handlers_books.go 定义。

// helpers
func urlEncodeKeyword(kw string) string {
	return bookfetch.URLEncode(kw)
}

func splitSemicolonRule(s string) []string {
	return bookfetch.SplitSemicolon(s)
}

func resolveURL(raw, base string) string {
	return bookfetch.ResolveURL(raw, base)
}
