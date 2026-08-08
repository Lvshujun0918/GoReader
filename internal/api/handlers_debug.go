package api

import (
	"encoding/json"
	"fmt"
	"time"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/GoReader/internal/model"
	"github.com/Lvshujun0918/GoReader/internal/parser/rule"
	"github.com/Lvshujun0918/GoReader/internal/service/bookfetch"
	"github.com/Lvshujun0918/GoReader/internal/service/crawler"
)

// handleBookSourceDebugSSE GET /reader3/bookSourceDebugSSE：书源逐步调试（SSE）。
// 事件契约（web-ui/src/api/sourceDebug.ts）：
//   event: step   + data {"message": "..."}             逐步日志
//   event: result + data {"data": <任意 JSON>}          最终结果
//   event: error  + data {"message": "..."}             失败
func (a *API) handleBookSourceDebugSSE(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	bookSource := paramOf(params, "bookSource")
	action := paramOf(params, "action")
	if bookSource == "" || action == "" {
		a.debugEmit(c, "error", map[string]any{"message": "参数错误：需要 bookSource 与 action"})
		return
	}
	src, err := a.Storage.FindBookSource(ns, bookSource)
	if err != nil || src == nil {
		a.debugEmit(c, "error", map[string]any{"message": "未找到书源"})
		return
	}
	c.Header("Content-Type", "text/event-stream")
	c.Header("Cache-Control", "no-cache")
	c.Header("Connection", "keep-alive")

	a.debugEmit(c, "step", map[string]any{
		"message": fmt.Sprintf("书源：%s · 动作：%s", src.BookSourceName, action),
	})
	switch action {
	case "search":
		a.debugSearch(c, src, paramOf(params, "key"))
	case "toc":
		a.debugToc(c, ns, src, paramOf(params, "chapterUrl"))
	case "content":
		a.debugContent(c, ns, src, paramOf(params, "chapterUrl"))
	default:
		a.debugEmit(c, "error", map[string]any{"message": "未知动作：" + action})
	}
}

func (a *API) debugEmit(c *gin.Context, event string, data any) {
	b, _ := json.Marshal(data)
	fmt.Fprintf(c.Writer, "event: %s\ndata: %s\n\n", event, b)
	c.Writer.Flush()
}

// debugSearch 搜索逐步调试：搜索 URL 模板 → 构造 → 抓取 → 列表规则 → 字段规则 → 结果。
func (a *API) debugSearch(c *gin.Context, src *model.BookSource, key string) {
	if key == "" {
		a.debugEmit(c, "error", map[string]any{"message": "请输入搜索关键词"})
		return
	}
	ruleStr := src.RuleSearch
	if ruleStr == "" {
		ruleStr = src.SearchRule
	}
	if ruleStr == "" {
		a.debugEmit(c, "error", map[string]any{"message": "书源无搜索规则（ruleSearch）"})
		return
	}
	a.debugEmit(c, "step", map[string]any{"message": "搜索规则：" + truncateStr(ruleStr, 300)})

	vars := map[string]string{
		"baseUrl":        src.BookSourceURL,
		"key":            key,
		"page":           "1",
		"sourceVariable": src.Variable,
		"sourceName":     src.BookSourceName,
		"sourceUrl":      src.BookSourceURL,
		"cookie":         "",
	}
	ctx := &rule.Context{BaseURL: src.BookSourceURL, Variables: vars}
	req := buildSearchRequest(src.SearchURL, key, 1, ctx, src.BookSourceURL)
	a.debugEmit(c, "step", map[string]any{
		"message": fmt.Sprintf("构造请求：%s（Method=%s）", req.URL, req.Method),
	})

	client := crawler.New(nil, "", a.Solver)
	start := time.Now()
	body, err := client.Fetch(req.URL, src)
	if err != nil {
		a.debugEmit(c, "error", map[string]any{"message": "抓取失败：" + err.Error()})
		return
	}
	a.debugEmit(c, "step", map[string]any{
		"message": fmt.Sprintf("抓取成功：%d 字节（%dms）", len(body), time.Since(start).Milliseconds()),
	})
	htmlStr := string(body)
	rules := parseSearchRules(ruleStr)
	bookList := rules["bookList"]
	if bookList == "" {
		bookList = rules["books"]
	}
	parseCtx := &rule.Context{BaseURL: req.URL, Variables: vars}

	var items []string
	if bookList != "" {
		a.debugEmit(c, "step", map[string]any{"message": "列表规则（bookList）：" + truncateStr(bookList, 200)})
		items = rule.Parse(htmlStr, ensureListHTML(bookList), parseCtx)
		a.debugEmit(c, "step", map[string]any{"message": fmt.Sprintf("列表规则命中 %d 项", len(items))})
	} else {
		items = []string{htmlStr}
		a.debugEmit(c, "step", map[string]any{"message": "无列表规则（bookList），按单书处理"})
	}
	if len(items) == 0 {
		a.debugEmit(c, "step", map[string]any{"message": "列表为空：规则未命中，或页面无搜索结果"})
		a.debugEmit(c, "result", map[string]any{"data": map[string]any{"count": 0, "items": []any{}}})
		return
	}

	results := make([]*SearchResult, 0, len(items))
	for range items {
		results = append(results, &SearchResult{})
	}
	fillSearchResults(rules, items, results, parseCtx, src, req.URL)

	// 字段命中统计（帮助定位"搜得到列表但字段为空"）
	fieldKeys := []string{"name", "author", "bookUrl", "coverUrl", "intro", "wordCount", "latestChapterTitle"}
	for _, f := range fieldKeys {
		n := 0
		for _, r := range results {
			switch f {
			case "name":
				if r.Name != "" {
					n++
				}
			case "author":
				if r.Author != "" {
					n++
				}
			case "bookUrl":
				if r.BookURL != "" {
					n++
				}
			case "coverUrl":
				if r.CoverURL != "" {
					n++
				}
			case "intro":
				if r.Intro != "" {
					n++
				}
			case "wordCount":
				if r.WordCount != "" {
					n++
				}
			case "latestChapterTitle":
				if r.LatestChapterTitle != "" {
					n++
				}
			}
		}
		a.debugEmit(c, "step", map[string]any{"message": fmt.Sprintf("字段 %s：%d/%d 条命中", f, n, len(results))})
	}

	sample := results
	if len(sample) > 5 {
		sample = results[:5]
	}
	a.debugEmit(c, "result", map[string]any{"data": map[string]any{"count": len(results), "items": sample}})
}

// debugToc 目录逐步调试。
func (a *API) debugToc(c *gin.Context, ns string, src *model.BookSource, url string) {
	if url == "" {
		a.debugEmit(c, "error", map[string]any{"message": "请输入书籍 URL"})
		return
	}
	tocRule := src.RuleToc
	if tocRule == "" {
		tocRule = src.TocRule
	}
	a.debugEmit(c, "step", map[string]any{"message": "目录规则（ruleToc）：" + truncateStr(tocRule, 300)})
	fetcher := bookfetch.New(a.Storage, ns, a.Solver)
	chapters, _, err := fetcher.BookToc(src, url)
	if err != nil {
		a.debugEmit(c, "error", map[string]any{"message": "目录获取失败：" + err.Error()})
		return
	}
	a.debugEmit(c, "step", map[string]any{"message": fmt.Sprintf("解析到 %d 个章节", len(chapters))})
	sample := make([]map[string]any, 0, len(chapters))
	for i, ch := range chapters {
		if i >= 5 {
			break
		}
		sample = append(sample, map[string]any{"title": ch.Title, "url": ch.URL})
	}
	a.debugEmit(c, "result", map[string]any{"data": map[string]any{"count": len(chapters), "sample": sample}})
}

// debugContent 正文逐步调试。
func (a *API) debugContent(c *gin.Context, ns string, src *model.BookSource, url string) {
	if url == "" {
		a.debugEmit(c, "error", map[string]any{"message": "请输入章节 URL"})
		return
	}
	contentRule := src.RuleContent
	if contentRule == "" {
		contentRule = src.ContentRule
	}
	a.debugEmit(c, "step", map[string]any{"message": "正文规则（ruleContent）：" + truncateStr(contentRule, 300)})
	fetcher := bookfetch.New(a.Storage, ns, a.Solver)
	res, err := fetcher.BookContent(src, url, 3)
	if err != nil {
		a.debugEmit(c, "error", map[string]any{"message": "正文获取失败：" + err.Error()})
		return
	}
	n := len([]rune(res.Content))
	a.debugEmit(c, "step", map[string]any{"message": fmt.Sprintf("正文长度：%d 字", n)})
	preview := res.Content
	if r := []rune(preview); len(r) > 300 {
		preview = string(r[:300]) + "…"
	}
	a.debugEmit(c, "result", map[string]any{"data": map[string]any{"length": n, "preview": preview}})
}

// truncateStr 截断长文本（调试日志展示用）。
func truncateStr(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n]) + "…"
}
