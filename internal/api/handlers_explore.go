package api

import (
	"encoding/json"
	"net/url"
	"strconv"
	"strings"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/reader-dev/internal/model"
	"github.com/Lvshujun0918/reader-dev/internal/parser/rule"
	"github.com/Lvshujun0918/reader-dev/internal/service/bookfetch"
)

// ExploreCategory 探索分类（前端 ExploreCategory：{title, url, type?}）。
type ExploreCategory struct {
	Title string `json:"title"`
	URL   string `json:"url"`
	Type  string `json:"type,omitempty"` // link=外部链接（前端 ↗ 打开）
}

// handleGetExploreSources GET/POST /reader3/getExploreSources：启用探索的书源。
func (a *API) handleGetExploreSources(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	sources, err := a.Storage.ListBookSources(ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	var out []model.BookSource
	for _, s := range sources {
		if s.EnabledExplore == 1 && s.ExploreURL != "" {
			out = append(out, s)
		}
	}
	OK(c, out)
}

// handleGetExploreUrls GET/POST /reader3/getExploreUrls：探索分类 URL 列表（[{title,url,type}]）。
// 参数兼容：origin（legacy）与 bookSource（前端实际传参）。
func (a *API) handleGetExploreUrls(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	origin := paramOf(params, "origin")
	if origin == "" {
		origin = paramOf(params, "bookSource")
	}
	if origin == "" {
		Fail(c, "参数错误")
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
	ctx := &rule.Context{
		BaseURL:   src.BookSourceURL,
		Variables: map[string]string{"baseUrl": src.BookSourceURL},
	}
	OK(c, parseExploreURLs(src.ExploreURL, ctx))
}

// parseExploreURLs 解析 exploreUrl 为分类列表 [{title,url,type}]。
// 支持真实书源格式：
//   - JSON 数组（起点/番茄/熊猫：{"title":..,"url":..,"style":..}）
//   - 标题::URL 换行/分号/| 分隔（69书吧等）
//   - @js:/<js> 前缀 JS 构造（笔阅读器等）
//   - 纯 URL（分类名从 URL 尾部路径提取，缺省"默认"）
func parseExploreURLs(raw string, ctx *rule.Context) []ExploreCategory {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return nil
	}
	// @js:/<js> 前缀：evalJS 得结果（JSON 数组或分隔文本）——执行失败则返回空
	// （避免把 JS 源码当分类 URL；笔阅读器等依赖模板字符串/java.ajax 的书源本实现无法解析）
	if strings.HasPrefix(raw, "@js:") || strings.HasPrefix(raw, "<js>") {
		if vs := rule.Parse("", raw, ctx); len(vs) > 0 && vs[0] != "" {
			raw = vs[0]
		} else {
			return nil
		}
	}
	// JSON 数组格式（legado 允许尾随逗号，如 [{"title":..},]——标准 Unmarshal 失败后宽松修复）
	var arr []map[string]any
	if json.Unmarshal([]byte(raw), &arr) != nil {
		if fixed := stripTrailingCommas(raw); fixed != raw {
			_ = json.Unmarshal([]byte(fixed), &arr)
		}
	}
	if len(arr) > 0 {
		var out []ExploreCategory
		for _, m := range arr {
			title, _ := m["title"].(string)
			u, _ := m["url"].(string)
			if strings.TrimSpace(title) == "" {
				title = "默认"
			}
			c := ExploreCategory{Title: title, URL: u}
			if u == "" {
				c.Type = "link"
			}
			out = append(out, c)
		}
		return out
	}
	// 分隔文本：每项 标题::URL 或纯 URL
	var out []ExploreCategory
	for _, seg := range strings.FieldsFunc(raw, func(r rune) bool {
		return r == '\n' || r == ';' || r == '|'
	}) {
		seg = strings.TrimSpace(seg)
		if seg == "" {
			continue
		}
		if idx := strings.Index(seg, "::"); idx >= 0 {
			title := strings.TrimSpace(seg[:idx])
			u := strings.TrimSpace(seg[idx+2:])
			if title == "" {
				title = "默认"
			}
			out = append(out, ExploreCategory{Title: title, URL: u})
		} else {
			out = append(out, ExploreCategory{Title: categoryTitleFromURL(seg), URL: seg})
		}
	}
	return out
}

// categoryTitleFromURL 纯 URL 分类名：取 URL 路径最后一段（域名段/占位符/纯数字时用"默认"）。
func categoryTitleFromURL(u string) string {	p := u
	if i := strings.IndexAny(p, "?#"); i >= 0 {
		p = p[:i]
	}
	if parsed, err := url.Parse(p); err == nil && parsed.Path != "" {
		path := strings.TrimRight(parsed.Path, "/")
		if i := strings.LastIndex(path, "/"); i >= 0 {
			seg := path[i+1:]
			if seg != "" && !strings.ContainsAny(seg, "{}0123456789") {
				return seg
			}
		}
	}
	return "默认"
}

// stripTrailingCommas 移除 JSON 尾随逗号（legado exploreUrl 数组常用 [a,b,] 形式，
// 标准 json.Unmarshal 会失败）。逐字符扫描并跳过字符串/转义。
func stripTrailingCommas(s string) string {
	if !strings.Contains(s, ",") {
		return s
	}
	var b strings.Builder
	b.Grow(len(s))
	inStr := false
	esc := false
	for i := 0; i < len(s); i++ {
		c := s[i]
		if inStr {
			b.WriteByte(c)
			if esc {
				esc = false
			} else if c == '\\' {
				esc = true
			} else if c == '"' {
				inStr = false
			}
			continue
		}
		switch c {
		case '"':
			inStr = true
			b.WriteByte(c)
		case ',':
			// 逗号后（跳过空白）紧跟 ] 或 } → 尾随逗号，跳过
			j := i + 1
			for j < len(s) && (s[j] == ' ' || s[j] == '\t' || s[j] == '\n' || s[j] == '\r') {
				j++
			}
			if j < len(s) && (s[j] == ']' || s[j] == '}') {
				continue
			}
			b.WriteByte(c)
		default:
			b.WriteByte(c)
		}
	}
	return b.String()
}

// handleExploreBook GET/POST /reader3/exploreBook：探索书单。
// 参数兼容：origin（legacy）与 bookSource（前端实际传参）；{{page}}/{page} 占位符替换。
func (a *API) handleExploreBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	url := paramOf(params, "url")
	origin := paramOf(params, "origin")
	if origin == "" {
		origin = paramOf(params, "bookSource")
	}
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	page := 1
	if v, ok := intParam(params, "page"); ok && v > 0 {
		page = int(v)
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
	// {{page}}/{page} 占位符（分类 URL 常含分页）
	url = replaceExplorePage(url, page)
	// 相对路径 resolve 到书源根（getExploreUrls 返回的相对 exploreUrl，如 /blist/class/0/1.htm——
	// 不 resolve 则 hostname 为空，crawler SSRF 会误判"禁止访问内网地址"）
	if !strings.HasPrefix(url, "http://") && !strings.HasPrefix(url, "https://") {
		url = bookfetch.ResolveURL(url, src.BookSourceURL)
	}
	client := a.crawlerClient(ns)
	body, err := client.Fetch(url, src)
	if err != nil {
		Fail(c, "探索失败："+err.Error())
		return
	}
	htmlStr := string(body)
	ctx := &rule.Context{
		BaseURL:   url,
		Variables: map[string]string{"baseUrl": src.BookSourceURL, "page": strconv.Itoa(page)},
	}
	ruleStr := src.RuleExplore
	if ruleStr == "" {
		ruleStr = src.ExploreRule
	}
	// 规则解析：legacy 分号串 / legado JSON 对象（复用搜索规则引擎）
	rules := parseSearchRules(ruleStr)
	bookList := rules["bookList"]
	if bookList == "" {
		bookList = rules["books"]
	}
	var items []string
	if bookList != "" {
		items = rule.Parse(htmlStr, ensureListHTML(bookList), ctx)
	}
	if len(items) == 0 {
		items = []string{htmlStr}
	}
	var results []map[string]any
	for _, item := range items {
		book := map[string]any{}
		if v := firstRule(rules, item, ctx, "name"); v != "" {
			book["name"] = v
		}
		if v := firstRule(rules, item, ctx, "author"); v != "" {
			book["author"] = v
		}
		if v := firstRule(rules, item, ctx, "bookUrl", "detailUrl"); v != "" {
			book["bookUrl"] = bookfetch.ResolveURL(v, url)
			book["tocUrl"] = book["bookUrl"]
		}
		if v := firstRule(rules, item, ctx, "coverUrl"); v != "" {
			book["coverUrl"] = bookfetch.ResolveURL(v, url)
		}
		if v := firstRule(rules, item, ctx, "intro", "desc"); v != "" {
			book["intro"] = v
		}
		if v := firstRule(rules, item, ctx, "latestChapterTitle"); v != "" {
			book["latestChapterTitle"] = v
		}
		if book["name"] != nil {
			book["origin"] = src.BookSourceURL
			book["originName"] = src.BookSourceName
			results = append(results, book)
		}
	}
	OK(c, results)
}

// replaceExplorePage 替换探索分类 URL 的分页占位符 {{page}}/{page}。
func replaceExplorePage(u string, page int) string {
	p := strconv.Itoa(page)
	u = strings.ReplaceAll(u, "{{page}}", p)
	u = strings.ReplaceAll(u, "{page}", p)
	return u
}

// ---------------- 订阅 ----------------

func (a *API) handleGetSourceSubs(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	list, err := a.Storage.ListSourceSubs(ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, list)
}

func (a *API) handleSaveSourceSub(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	var sub model.SourceSub
	if err := c.ShouldBindJSON(&sub); err != nil {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.SaveSourceSub(ns, &sub); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleDeleteSourceSub(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	url := paramOf(a.params(c), "url")
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.DeleteSourceSub(ns, url); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleRefreshSourceSub(c *gin.Context) {
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
	// 刷新订阅：重新抓取并更新文章
	src, err := a.Storage.FindBookSource(ns, url)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if src == nil {
		Fail(c, "未找到书源")
		return
	}
	// 订阅刷新逻辑（迭代）
	OK(c, nil)
}
