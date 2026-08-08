// Package bookfetch 书源书籍链路：详情 ruleBookInfo / 目录 ruleToc / 正文 ruleContent。
package bookfetch

import (
	"encoding/json"
	"fmt"
	"net/url"
	"strconv"
	"strings"

	"github.com/Lvshujun0918/GoReader/internal/model"
	"github.com/Lvshujun0918/GoReader/internal/parser/rule"
	"github.com/Lvshujun0918/GoReader/internal/service/crawler"
	"github.com/Lvshujun0918/GoReader/internal/service/solver"
	"github.com/Lvshujun0918/GoReader/internal/storage"
)

// Fetcher 书源抓取器。
type Fetcher struct {
	Client *crawler.Client
}

// New 创建抓取器。solverOpt 可选传入质询求解器（转发给 crawler）。
func New(st *storage.Storage, ns string, solverOpt ...*solver.Solver) *Fetcher {
	return &Fetcher{Client: crawler.New(st, ns, solverOpt...)}
}

// BookInfo 书籍详情（ruleBookInfo）。
func (f *Fetcher) BookInfo(source *model.BookSource, bookURL string) (map[string]any, error) {
	body, err := f.Client.Fetch(bookURL, source)
	if err != nil {
		return nil, err
	}
	htmlStr := string(body)
	ruleStr := source.RuleBookInfo
	if ruleStr == "" {
		ruleStr = source.BookInfoRule
	}
	ctx := &rule.Context{BaseURL: bookURL}
	ctx.Set("bookUrl", bookURL)
	info := map[string]any{
		"bookUrl":    bookURL,
		"origin":     source.BookSourceURL,
		"originName": source.BookSourceName,
		"tocUrl":     bookURL,
	}
	rules := parseRuleMap(ruleStr)
	if len(rules) == 0 {
		info["name"] = "未知书籍"
		return info, nil
	}
	// init 上下文（legado：init 规则先把响应转为新上下文，之后所有字段基于其解析）
	initVal := ""
	if v := rules["init"]; v != "" {
		if res := rule.Parse(htmlStr, v, ctx); len(res) > 0 {
			initVal = res[0]
		}
	}
	for key, val := range rules {
		if key == "init" {
			continue
		}
		input := htmlStr
		if initVal != "" {
			input = initVal
		}
		results := rule.Parse(input, val, ctx)
		if len(results) == 0 {
			continue
		}
		if key == "coverUrl" {
			info["coverUrl"] = resolveURL(results[0], bookURL)
		} else if key == "tocUrl" {
			info["tocUrl"] = resolveURL(results[0], bookURL)
		} else {
			info[key] = results[0]
		}
	}
	if info["name"] == nil {
		info["name"] = "未知书籍"
	}
	return info, nil
}

// Chapter 章节。
type Chapter struct {
	Title    string `json:"title"`
	URL      string `json:"url"`
	IsVolume bool   `json:"isVolume"`
	Index    int    `json:"index"`
}

// BookToc 目录（ruleToc）。返回（章节列表, 下一页 URL, 错误）。
// 支持 JSON 分页自动翻页：响应含 paging.{pi,ps,count} 且 pi*ps<count 时，
// 自动请求 pi+1 拼接，直到 count 或最大页数（酷我 paging=1 分页目录等）。
func (f *Fetcher) BookToc(source *model.BookSource, tocURL string) ([]Chapter, string, error) {
	ruleStr := source.RuleToc
	if ruleStr == "" {
		ruleStr = source.TocRule
	}
	rules := parseRuleMap(ruleStr)
	listRule := firstNonEmpty(rules, "chapters", "chapterList", "tocList", "list")
	if listRule == "" {
		return nil, "", nil
	}
	titleRule := firstNonEmpty(rules, "chapterTitle", "chapterName", "title")
	urlRule := firstNonEmpty(rules, "chapterUrl", "url")
	nextRule := firstNonEmpty(rules, "nextTocUrl", "nextUrl")

	var chapters []Chapter
	curURL := tocURL
	lastNext := ""
	for page := 0; page < 100; page++ {
		body, err := f.Client.Fetch(curURL, source)
		if err != nil {
			return nil, "", err
		}
		htmlStr := string(body)
		ctx := &rule.Context{BaseURL: curURL}
		items := rule.Parse(htmlStr, listRule, ctx)
		for _, item := range items {
			ch := Chapter{Index: len(chapters)}
			if titleRule != "" {
				if t := rule.Parse(item, titleRule, ctx); len(t) > 0 {
					ch.Title = t[0]
				}
			}
			if ch.Title == "" {
				ch.Title = item
			}
			if urlRule != "" {
				if u := rule.Parse(item, urlRule, ctx); len(u) > 0 {
					ch.URL = resolveURL(u[0], curURL)
				}
			}
			if ch.URL == "" {
				ch.URL = curURL
			}
			chapters = append(chapters, ch)
		}
		// 下一页：nextTocUrl 规则优先，其次 JSON paging 自动翻页
		next := ""
		if nextRule != "" {
			if res := rule.Parse(htmlStr, nextRule, ctx); len(res) > 0 {
				next = resolveURL(res[0], curURL)
			}
		}
		if next == "" {
			next = nextJSONPage(htmlStr, curURL)
		}
		if next == "" || next == curURL {
			lastNext = next
			break
		}
		curURL = next
	}
	// 卷标题检测（isVolume）
	for i := range chapters {
		if strings.HasSuffix(chapters[i].Title, "卷") || strings.Contains(chapters[i].Title, "·") && len(chapters[i].Title) < 12 {
			chapters[i].IsVolume = true
		}
	}
	return chapters, lastNext, nil
}

// nextJSONPage JSON 分页目录的下一页 URL：响应含 paging.{pi,ps,count} 且未取完时，pi+1。
func nextJSONPage(body, curURL string) string {
	var p struct {
		Paging struct {
			PI    int `json:"pi"`
			PS    int `json:"ps"`
			Count int `json:"count"`
		} `json:"paging"`
	}
	if json.Unmarshal([]byte(body), &p) != nil {
		return ""
	}
	pg := p.Paging
	if pg.PI <= 0 || pg.PS <= 0 || pg.Count <= 0 {
		return ""
	}
	if pg.PI*pg.PS >= pg.Count {
		return ""
	}
	u, err := url.Parse(curURL)
	if err != nil {
		return ""
	}
	q := u.Query()
	q.Set("pi", strconv.Itoa(pg.PI+1))
	u.RawQuery = q.Encode()
	return u.String()
}

// ContentResult 正文结果（含下一页/字数）。
type ContentResult struct {
	Content    string `json:"content"`
	NextURL    string `json:"nextUrl,omitempty"`
	WordCount  int    `json:"wordCount,omitempty"`
}

// BookContent 正文（ruleContent；多页循环拼接）。
func (f *Fetcher) BookContent(source *model.BookSource, chapterURL string, maxPages int) (*ContentResult, error) {
	if maxPages <= 0 {
		maxPages = 10
	}
	ruleStr := source.RuleContent
	if ruleStr == "" {
		ruleStr = source.ContentRule
	}
	rules := parseRuleMap(ruleStr)
	contentRule := firstNonEmpty(rules, "content", "bookContent")
	nextRule := firstNonEmpty(rules, "nextContentUrl", "nextUrl")
	var parts []string
	curURL := chapterURL
	for i := 0; i < maxPages; i++ {
		body, err := f.Client.Fetch(curURL, source)
		if err != nil {
			return nil, err
		}
		htmlStr := string(body)
		ctx := &rule.Context{BaseURL: curURL}
		var contentParts []string
		if contentRule != "" {
			contentParts = append(contentParts, rule.Parse(htmlStr, contentRule, ctx)...)
		}
		if len(contentParts) == 0 {
			// 兜底：正文直接取文本
			contentParts = append(contentParts, stripTags(htmlStr))
		}
		joined := strings.Join(contentParts, "\n")
		if joined != "" {
			parts = append(parts, joined)
		}
		nextURL := ""
		if nextRule != "" {
			if res := rule.Parse(htmlStr, nextRule, ctx); len(res) > 0 {
				nextURL = resolveURL(res[0], curURL)
			}
		}
		if nextURL == "" || nextURL == curURL {
			break
		}
		curURL = nextURL
	}
	content := strings.TrimSpace(strings.Join(parts, "\n\n"))
	// 换行规范化：源站 CRLF → LF（前端按 \n 切段）
	content = strings.ReplaceAll(content, "\r\n", "\n")
	content = strings.ReplaceAll(content, "\r", "\n")
	return &ContentResult{Content: content, WordCount: len([]rune(content))}, nil
}

// stripTags 简易 HTML 去标签。
func stripTags(s string) string {
	var b strings.Builder
	inTag := false
	for _, r := range s {
		switch {
		case r == '<':
			inTag = true
		case r == '>':
			inTag = false
		case !inTag:
			b.WriteRune(r)
		}
	}
	return strings.TrimSpace(b.String())
}

// splitSemicolon 分号分隔规则项（不切括号内分号）。
func splitSemicolon(s string) []string {
	var out []string
	depth := 0
	start := 0
	for i := 0; i < len(s); i++ {
		switch s[i] {
		case '(', '[', '{':
			depth++
		case ')', ']', '}':
			if depth > 0 {
				depth--
			}
		case ';':
			if depth == 0 {
				out = append(out, s[start:i])
				start = i + 1
			}
		}
	}
	out = append(out, s[start:])
	var filtered []string
	for _, v := range out {
		if strings.TrimSpace(v) != "" {
			filtered = append(filtered, v)
		}
	}
	return filtered
}

// splitKeyVal 形如 "key=value" 的拆分。
func splitKeyVal(item string) (string, string) {
	item = strings.TrimSpace(item)
	idx := strings.IndexByte(item, '=')
	if idx < 0 {
		return "", item
	}
	return strings.TrimSpace(item[:idx]), item[idx+1:]
}

// parseRuleMap 解析书源规则为 key→value map：legado JSON 对象
// （{"content":"$.data.content","name":"$.title"}）或 legacy 分号串（content=...;name=...）。
func parseRuleMap(ruleStr string) map[string]string {
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
	for _, item := range splitSemicolon(ruleStr) {
		key, val := splitKeyVal(item)
		if key != "" {
			out[key] = val
		}
	}
	return out
}

// firstNonEmpty 依次返回首个非空的规则值。
func firstNonEmpty(m map[string]string, keys ...string) string {
	for _, k := range keys {
		if v := m[k]; v != "" {
			return v
		}
	}
	return ""
}

// resolveURL 相对链接解析（含 // 协议相对）。base 无路径时视为根目录（https://host → https://host/）。
func resolveURL(raw, base string) string {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return ""
	}
	if strings.HasPrefix(raw, "//") {
		idx := strings.Index(base, "://")
		if idx > 0 {
			return base[:idx+2] + raw
		}
	}
	if strings.HasPrefix(raw, "http://") || strings.HasPrefix(raw, "https://") {
		return raw
	}
	// base 无路径时补根斜杠（https://src.com → https://src.com/）
	dir := base
	if !strings.HasSuffix(dir, "/") {
		if u, err := url.Parse(base); err == nil && u.Path == "" {
			dir += "/"
		}
	}
	bu, err := url.Parse(dir)
	if err != nil {
		return raw
	}
	ru, err := url.Parse(raw)
	if err != nil {
		return raw
	}
	return bu.ResolveReference(ru).String()
}

// Errorf 便捷错误。
func Errorf(format string, args ...any) error {
	return fmt.Errorf(format, args...)
}

// ResolveURL 相对链接解析（导出包装）。
func ResolveURL(raw, base string) string {
	return resolveURL(raw, base)
}

// SplitSemicolon 分号分隔规则项（导出包装）。
func SplitSemicolon(s string) []string {
	return splitSemicolon(s)
}

// URLEncode 关键词 URL 编码：非 ASCII 与保留字符 → %XX（legado URLEncoder UTF-8 行为），
// 空格 → %20。保证请求行合法（原始中文会触发服务器 400）。
func URLEncode(kw string) string {
	if kw == "" {
		return ""
	}
	return strings.ReplaceAll(url.QueryEscape(kw), "+", "%20")
}
