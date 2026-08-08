package api

import (
	"encoding/json"
	"fmt"
	"strconv"
	"strings"

	"github.com/gin-gonic/gin"
	"golang.org/x/text/encoding/simplifiedchinese"

	"github.com/Lvshujun0918/GoReader/internal/model"
	"github.com/Lvshujun0918/GoReader/internal/parser/rule"
	"github.com/Lvshujun0918/GoReader/internal/service/bookfetch"
	"github.com/Lvshujun0918/GoReader/internal/service/crawler"
	"github.com/Lvshujun0918/GoReader/internal/service/solver"
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

// SearchSource 单书源搜索。solverOpt 可选传入质询求解器（书源 cookie 自动附加 + 质询求解）。
// 支持两种规则格式：legacy 分号串（bookList=...;name=...）与 legado JSON 对象（{"bookList":"...","name":"..."}）。
func SearchSource(src *model.BookSource, keyword string, solverOpt ...*solver.Solver) ([]*SearchResult, error) {
	ruleStr := src.RuleSearch
	if ruleStr == "" {
		ruleStr = src.SearchRule
	}
	if ruleStr == "" {
		return nil, fmt.Errorf("书源无搜索规则")
	}
	rules := parseSearchRules(ruleStr)
	bookList := rules["bookList"]
	if bookList == "" {
		bookList = rules["books"]
	}
	// 构造搜索请求（legado searchUrl：{{key}}/{{searchKey}}/{key} 占位符、{{page}} 分页、
	// @js:/<js> 前缀 JS 构造、URL 后 ,{'method':'POST','body':...} 表单描述）
	// 变量集贯穿 JS 环境（source.getVariable / getArguments / cookie.getCookie / key）
	vars := map[string]string{
		"baseUrl":        src.BookSourceURL,
		"key":            keyword,
		"page":           "1",
		"sourceVariable": src.Variable,
		"sourceName":     src.BookSourceName,
		"sourceUrl":      src.BookSourceURL,
		"cookie":         "",
	}
	ctx := &rule.Context{
		BaseURL:   src.BookSourceURL,
		Variables: vars,
	}
	req := buildSearchRequest(src.SearchURL, keyword, 1, ctx, src.BookSourceURL)

	client := crawler.New(nil, "", solverOpt...)
	var body []byte
	var err error
	if req.Method == "POST" {
		body, err = client.FetchPost(req.URL, req.Body, src)
	} else {
		body, err = client.Fetch(req.URL, src)
	}
	if err != nil {
		return nil, err
	}
	htmlStr := string(body)
	// 解析上下文：注入书源变量集（bookUrl 等子规则的 <js> 依赖 source/key/result）
	parseCtx := &rule.Context{BaseURL: req.URL, Variables: vars}

	if bookList != "" {
		// 列表规则：bookList 项（CSS 补 @html 供子规则解析；JSONPath/js 直接取项）
		items := rule.Parse(htmlStr, ensureListHTML(bookList), parseCtx)
		results := make([]*SearchResult, 0, len(items))
		for range items {
			results = append(results, &SearchResult{})
		}
		fillSearchResults(rules, items, results, parseCtx, src, req.URL)
		return results, nil
	}
	// 无列表规则：单书直达
	res := &SearchResult{}
	fillSearchResults(rules, []string{htmlStr}, []*SearchResult{res}, parseCtx, src, req.URL)
	if res.Name != "" {
		return []*SearchResult{res}, nil
	}
	return nil, nil
}

// parseSearchRules 解析搜索规则为 key→规则值：legacy 分号串（bookList=...;name=...）
// 或 legado JSON 对象（{"bookList":"...","name":"..."}）。
func parseSearchRules(ruleStr string) map[string]string {
	out := map[string]string{}
	var m map[string]any
	if json.Unmarshal([]byte(ruleStr), &m) == nil {
		for k, v := range m {
			if s, ok := v.(string); ok && s != "" {
				out[k] = s
			}
		}
		return out
	}
	for _, item := range splitSemicolonRule(ruleStr) {
		key, val := splitKeyVal(item)
		if key != "" {
			out[key] = val
		}
	}
	return out
}

// fillSearchResults 按规则 map 填充搜索结果字段。
func fillSearchResults(rules map[string]string, items []string, results []*SearchResult, ctx *rule.Context, src *model.BookSource, baseURL string) {
	for i, item := range items {
		if i >= len(results) {
			break
		}
		res := results[i]
		if v := firstRule(rules, item, ctx, "name", "bookName"); v != "" {
			res.Name = v
		}
		if v := firstRule(rules, item, ctx, "author", "bookAuthor"); v != "" {
			res.Author = v
		}
		if v := firstRule(rules, item, ctx, "bookUrl", "detailUrl"); v != "" {
			res.BookURL = resolveURL(v, baseURL)
			res.TocURL = res.BookURL
		}
		if v := firstRule(rules, item, ctx, "coverUrl"); v != "" {
			res.CoverURL = resolveURL(v, baseURL)
		}
		if v := firstRule(rules, item, ctx, "intro", "desc"); v != "" {
			res.Intro = v
		}
		if v := firstRule(rules, item, ctx, "kind"); v != "" {
			res.Kind = v
		}
		if v := firstRule(rules, item, ctx, "wordCount"); v != "" {
			res.WordCount = v
		}
		if v := firstRule(rules, item, ctx, "latestChapterTitle"); v != "" {
			res.LatestChapterTitle = v
		}
		res.Origin = src.BookSourceURL
		res.OriginName = src.BookSourceName
		res.Type = src.BookSourceType
	}
}

// firstRule 依次尝试多个规则键，返回第一个非空解析结果。
func firstRule(rules map[string]string, item string, ctx *rule.Context, keys ...string) string {
	for _, k := range keys {
		val := rules[k]
		if val == "" {
			continue
		}
		if v := rule.Parse(item, val, ctx); len(v) > 0 {
			return v[0]
		}
	}
	return ""
}

// ensureListHTML bookList 的 CSS 规则需返回元素 HTML 供子规则再解析：
// 无属性后缀时补 @html（legado 默认返回元素文本，无法作为子规则输入）。
// 链式选择器（class.x.0@tag.ul）同理补 @html。
func ensureListHTML(val string) string {
	if strings.HasPrefix(val, "@css:") && !strings.Contains(val[5:], "@") {
		return val + "@html"
	}
	if rule.ChainNeedsHTML(val) {
		return val + "@html"
	}
	return val
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
	results, err := SearchSource(src, keyword, a.Solver)
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
	if v, ok := intParam(params, "maxSources"); ok && v > 0 {
		limit = int(v)
	} else if v, ok := intParam(params, "limit"); ok && v > 0 {
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
		if results, err := SearchSource(&src, keyword, a.Solver); err == nil {
			all = append(all, results...)
		}
		count++
	}
	OK(c, all)
}

// handleSearchBookMultiSSE GET/POST /reader3/searchBookMultiSSE：SSE 流式搜索。
// 事件契约（与 web-ui/src/api/sse.ts 对齐）：
//   event: book + data {"lastIndex": n, "data": [SearchBook]}  每个参与搜索的书源一帧
//   event: end  + data {"lastIndex": n, "isEnd": true}         流结束
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

	idx := 0
	for _, src := range sources {
		if src.Enabled != 1 || src.SearchURL == "" {
			continue
		}
		results, serr := SearchSource(&src, keyword, a.Solver)
		payload := map[string]any{"lastIndex": idx, "data": results}
		if serr != nil {
			// 失败也推一帧（空结果 + 错误信息），让前端"已搜索 N 个书源"计数准确
			payload["data"] = []*SearchResult{}
			payload["error"] = serr.Error()
		}
		b, _ := json.Marshal(payload)
		fmt.Fprintf(c.Writer, "event: book\ndata: %s\n\n", b)
		c.Writer.Flush()
		idx++
	}
	fmt.Fprintf(c.Writer, "event: end\ndata: %s\n\n",
		mustJSON(map[string]any{"lastIndex": idx - 1, "isEnd": true}))
}

// mustJSON 序列化（失败返回空对象，SSE 结束帧用）。
func mustJSON(v any) string {
	b, err := json.Marshal(v)
	if err != nil {
		return "{}"
	}
	return string(b)
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

// searchRequest 一次搜索请求描述（legado searchUrl 解析结果）。
type searchRequest struct {
	URL     string // 最终请求 URL（占位符已替换）
	Method  string // GET / POST
	Body    string // POST 表单串（占位符已替换，UTF-8 百分号编码）
	Charset string // 源站字符集（gbk/gb2312/utf-8）
}

// buildSearchRequest 解析书源 searchUrl 为具体请求。支持：
//   - {{key}}/{{searchKey}}/{key} 关键词占位符、{{page}}/{page} 分页（legado 双花括号）
//   - @js:/<js> 前缀：JS 构造 URL（result 变量）——先替换占位符再执行（{{key}} 在 JS 里否则语法错误）
//   - URL 后 ,{'method':'POST','body':'...','charset':'gbk'} 表单描述
//   - 相对路径 resolve 到书源根（baseURL）
func buildSearchRequest(raw, keyword string, page int, ctx *rule.Context, baseURL string) *searchRequest {
	r := &searchRequest{URL: raw, Method: "GET"}
	encKey := urlEncodeKeyword(keyword)
	// @js:/<js> 前缀：先替换双花括号占位符（{{key}}/{{page}}，JS 里 key 走裸变量注入，
	// 不能替换单花括号 {key}——会破坏 JS 模板字符串 ${key}），再 evalJS 得 URL（或 URL + POST 描述）
	if strings.HasPrefix(raw, "@js:") || strings.HasPrefix(raw, "<js>") {
		raw = replaceDoubleBraces(raw, encKey, page)
		if strings.HasPrefix(raw, "@js:") {
			// 传完整 @js: 规则（parseSingle 识别前缀后 evalJS）
			if vs := rule.Parse("", raw, ctx); len(vs) > 0 && vs[0] != "" {
				r.URL = vs[0]
			}
		} else if strings.HasPrefix(raw, "<js>") && strings.HasSuffix(raw, "</js>") {
			if vs := rule.Parse("", raw, ctx); len(vs) > 0 && vs[0] != "" {
				r.URL = vs[0]
			}
		}
	}
	// URL 后 ,{...} 请求描述（POST 表单等）
	if idx := strings.Index(r.URL, ",{"); idx >= 0 {
		desc := r.URL[idx+1:]
		r.URL = r.URL[:idx]
		parseRequestDesc(desc, r)
	}
	// 相对路径 resolve 到书源根（真实书源 searchUrl 常为 /path?key=...）
	if r.URL != "" && !strings.HasPrefix(r.URL, "http://") && !strings.HasPrefix(r.URL, "https://") {
		r.URL = resolveURL(r.URL, baseURL)
	}
	// 占位符替换（关键词编码随源站字符集）
	if r.Charset == "gbk" || r.Charset == "gb2312" {
		encKey = gbkPercentEncode(keyword)
	}
	r.URL = replacePlaceholders(r.URL, encKey, page)
	if r.Body != "" {
		r.Body = replacePlaceholders(r.Body, encKey, page)
	}
	return r
}

// parseRequestDesc 解析 ,{...} 请求描述（legado 单引号 JSON：method/body/charset）。
func parseRequestDesc(desc string, r *searchRequest) {
	// 单引号 → 双引号（legado 描述用单引号；值内单引号罕见，直接替换）
	js := strings.ReplaceAll(desc, "'", `"`)
	var m map[string]any
	if json.Unmarshal([]byte(js), &m) != nil {
		return
	}
	if v, ok := m["method"].(string); ok {
		r.Method = strings.ToUpper(v)
	}
	if v, ok := m["body"].(string); ok {
		r.Body = v
	}
	if v, ok := m["charset"].(string); ok {
		r.Charset = strings.ToLower(v)
	}
}

// replacePlaceholders 替换搜索占位符（先双花括号再单花括号，避免 {{key}} 被 {key} 部分命中）。
func replacePlaceholders(s, encKey string, page int) string {
	pageStr := strconv.Itoa(page)
	s = strings.ReplaceAll(s, "{{searchKey}}", encKey)
	s = strings.ReplaceAll(s, "{{key}}", encKey)
	s = strings.ReplaceAll(s, "{searchKey}", encKey)
	s = strings.ReplaceAll(s, "{key}", encKey)
	s = strings.ReplaceAll(s, "{{page}}", pageStr)
	s = strings.ReplaceAll(s, "{page}", pageStr)
	return s
}

// replaceDoubleBraces 仅替换双花括号占位符（JS searchUrl 专用）：
// 不替换 {key}/{page} 单花括号——避免破坏 JS 模板字符串 ${key} 等。
func replaceDoubleBraces(s, encKey string, page int) string {
	pageStr := strconv.Itoa(page)
	s = strings.ReplaceAll(s, "{{searchKey}}", encKey)
	s = strings.ReplaceAll(s, "{{key}}", encKey)
	s = strings.ReplaceAll(s, "{{page}}", pageStr)
	return s
}

// gbkPercentEncode 关键词按 GBK 编码后百分号编码（legado charset=gbk 的 POST 表单）。
func gbkPercentEncode(s string) string {
	enc := simplifiedchinese.GBK.NewEncoder()
	var b strings.Builder
	for _, r := range s {
		if r < 0x80 {
			b.WriteRune(r)
			continue
		}
		out, err := enc.String(string(r))
		if err != nil {
			continue
		}
		for i := 0; i < len(out); i++ {
			fmt.Fprintf(&b, "%%%02X", out[i])
		}
	}
	return b.String()
}

func splitSemicolonRule(s string) []string {
	return bookfetch.SplitSemicolon(s)
}

func resolveURL(raw, base string) string {
	return bookfetch.ResolveURL(raw, base)
}
