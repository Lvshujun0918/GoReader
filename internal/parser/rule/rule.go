// Package rule 书源多规则引擎（兼容 legado：@css / @xpath / @json / @regex / @js / @get 等）。
package rule

import (
	"bytes"
	"encoding/json"
	"net/url"
	"regexp"
	"strconv"
	"strings"

	"github.com/PaesslerAG/jsonpath"
	"github.com/andybalholm/cascadia"
	"github.com/antchfx/htmlquery"
	"github.com/dlclark/regexp2"
	"github.com/dop251/goja"
	"golang.org/x/net/html"
)

// Context 规则执行上下文。
type Context struct {
	// Variables 变量表（{name} 插值）
	Variables map[string]string
	// BaseURL 相对链接解析基准
	BaseURL string
}

// Set 设置变量。
func (c *Context) Set(name, value string) {
	if c.Variables == nil {
		c.Variables = map[string]string{}
	}
	c.Variables[name] = value
}

// Get 取变量（空表时安全）。
func (c *Context) Get(name string) string {
	if c.Variables == nil {
		return ""
	}
	return c.Variables[name]
}

// Parse 执行规则返回结果字符串列表。
// 规则可含 || 分隔的多个子规则（依次尝试，取首个非空集合）。
func Parse(input, rule string, ctx *Context) []string {
	if ctx == nil {
		ctx = &Context{}
	}
	rule = interpolateVars(input, rule, ctx)
	if rule == "" {
		return nil
	}
	// 多规则 ||（注意区分正则里的 ||，legado 以非转义 || 分隔）
	parts := splitTopLevel(rule, "||")
	var results []string
	for _, part := range parts {
		part = strings.TrimSpace(part)
		if part == "" {
			continue
		}
		results = parseSingle(input, part, ctx)
		if len(results) > 0 {
			return results
		}
	}
	return nil
}

// parseSingle 执行单条规则。
func parseSingle(input, rule string, ctx *Context) []string {
	// legado <js>code</js> 格式（如起点书源：<js>path='...';...</js>）
	if strings.HasPrefix(rule, "<js>") {
		end := strings.Index(rule, "</js>")
		if end > 0 {
			return evalJS(input, rule[len("<js>"):end], ctx)
		}
	}
	// 裸 JSONPath（$ 开头，如 $.data / $..book_data[0]）——legado 常用，无 @json: 前缀
	if strings.HasPrefix(rule, "$") {
		return evalJSON(input, rule, ctx)
	}
	// 文本后处理：value@js:code（先解析前缀，再对结果执行 JS，如 {{$.status}}@js:result.replace(...)）
	if idx := strings.LastIndex(rule, "@js:"); idx > 0 {
		prefix, code := rule[:idx], rule[idx+len("@js:"):]
		base := parseSingle(input, prefix, ctx)
		if len(base) > 0 {
			if vs := evalJS(base[0], code, ctx); len(vs) > 0 {
				return vs
			}
			return base
		}
	}
	// legado 链式选择器（class.x.0@tag.ul / tag.h3.0@tag.a.0@href）——无 @css: 前缀
	if isChainSelector(rule) {
		if vs := evalCSSChainFromInput(input, rule, ctx); len(vs) > 0 {
			return vs
		}
	}
	// 纯文本规则（无 @ 前缀）：正则全文匹配；无匹配时返回插值后的规则本身
	// （URL 模板等，如 /novels/api/book/{{$.book_id}} —— 插值后原样输出）
	if !strings.HasPrefix(rule, "@") {
		if out := regexMatchAll(input, rule); len(out) > 0 {
			return out
		}
		if rule != "" {
			return []string{rule}
		}
		return nil
	}

	// 定位类型规则：@css / @xpath / @json / @regex / @js / @get
	idx := strings.IndexByte(rule, ':')
	if idx < 0 {
		// 无冒号：可能是替换规则 ## 或纯 @ 前缀
		return processTextOps(input, rule, ctx)
	}
	kind := rule[1:idx]
	param := rule[idx+1:]

	switch kind {
	case "css":
		return evalCSS(input, param, ctx)
	case "xpath":
		return evalXPath(input, param, ctx)
	case "json":
		return evalJSON(input, param, ctx)
	case "regex":
		return evalRegex(input, param, ctx)
	case "js":
		return evalJS(input, param, ctx)
	case "get":
		return evalGet(input, param, ctx)
	default:
		return processTextOps(input, rule, ctx)
	}
}

// evalCSS CSS 选择器：@css:selector|:attr（attr 缺省 @text；支持 selector@attr 与 :text）。
func evalCSS(input, param string, ctx *Context) []string {
	doc, err := htmlquery.Parse(strings.NewReader(input))
	if err != nil {
		return nil
	}
	selector, attr := splitPipe(param)
	if attr == "" && strings.Contains(selector, "@") {
		// legado 语法：selector@attr（如 a@href）
		idx := strings.LastIndex(selector, "@")
		if idx > 0 {
			attr = "@" + selector[idx+1:]
			selector = selector[:idx]
		}
	}
	if selector == "" {
		return nil
	}
	sel, err := cascadia.Compile(selector)
	if err != nil {
		// legado 链式选择器（class.x.0@tag.ul.0@tag.li）——标准 CSS 编译失败时降级链式解析
		return evalCSSChain(doc, param, ctx)
	}
	var out []string
	for _, n := range sel.MatchAll(doc) {
		out = append(out, cssValue(n, attr, ctx))
	}
	return out
}

func cssValue(n *html.Node, attr string, ctx *Context) string {
	switch {
	case attr == "" || attr == "@text" || attr == ":text":
		return extractText(n)
	case attr == "@html":
		// 返回元素 OuterHTML（bookList 等需要子规则再解析的场景）
		return htmlquery.OutputHTML(n, true)
	case strings.HasPrefix(attr, "@"):
		key := strings.TrimPrefix(attr, "@")
		return attrValue(n, key)
	case strings.HasPrefix(attr, ":"):
		return extractText(n)
	default:
		return attrValue(n, attr)
	}
}

func attrValue(n *html.Node, key string) string {
	for _, a := range n.Attr {
		if a.Key == key {
			return strings.TrimSpace(a.Val)
		}
	}
	return ""
}

func extractText(n *html.Node) string {
	var buf bytes.Buffer
	var walk func(*html.Node)
	walk = func(node *html.Node) {
		if node.Type == html.TextNode {
			buf.WriteString(node.Data)
		}
		for ch := node.FirstChild; ch != nil; ch = ch.NextSibling {
			walk(ch)
		}
	}
	walk(n)
	return strings.TrimSpace(buf.String())
}

// ---------- legado 链式选择器 ----------

// isChainSelector 是否为 legado 链式选择器（class./id./tag./text. 开头，无 @css: 前缀）。
func isChainSelector(rule string) bool {
	for _, p := range []string{"class.", "id.", "tag.", "text."} {
		if strings.HasPrefix(rule, p) {
			return true
		}
	}
	return false
}

// IsChainSelector 导出：legado 链式选择器判断。
func IsChainSelector(rule string) bool {
	return isChainSelector(rule)
}

// ChainNeedsHTML 链式选择器作为 bookList 时是否需要补 @html 输出（末段为推进操作）。
func ChainNeedsHTML(val string) bool {
	if !isChainSelector(val) {
		return false
	}
	segs := chainSegs(val)
	if len(segs) == 0 {
		return false
	}
	return !chainIsOutput(segs[len(segs)-1])
}

// evalCSSChainFromInput 从 HTML 文本执行链式选择器。
func evalCSSChainFromInput(input, selector string, ctx *Context) []string {
	doc, err := htmlquery.Parse(strings.NewReader(input))
	if err != nil {
		return nil
	}
	return evalCSSChain(doc, selector, ctx)
}

// evalCSSChain legado 链式选择器：class.x.0@tag.ul.0@tag.li / tag.h3.0@tag.a.0@href / text.x。
// 基础选择器（class./id./tag./text.）匹配子孙节点 → .N 索引 → @操作
// （tag.x / css: / xpath: 推进；text / html / 属性 / js: / json: / regex: 输出）。
func evalCSSChain(doc *html.Node, selector string, ctx *Context) []string {
	nodes := []*html.Node{doc}
	segs := chainSegs(selector)
	if len(segs) == 0 {
		return nil
	}
	nodes = chainApplySel(nodes, segs[0])
	if len(nodes) == 0 {
		return nil
	}
	for i := 1; i < len(segs); i++ {
		seg := segs[i]
		if chainIsOutput(seg) {
			return chainOutput(nodes, seg, ctx)
		}
		nodes = chainAdvance(nodes, seg)
		if len(nodes) == 0 {
			return nil
		}
	}
	// 默认输出：元素文本
	var out []string
	for _, n := range nodes {
		out = append(out, extractText(n))
	}
	return out
}

// chainSegs 拆分选择器段：["class.x.0","tag.ul.0","tag.li"]（第一段无 @）。
func chainSegs(selector string) []string {
	selector = strings.TrimSpace(selector)
	if selector == "" {
		return nil
	}
	var segs []string
	rest := selector
	for rest != "" {
		if strings.HasPrefix(rest, "@") {
			rest = rest[1:]
		}
		i := strings.Index(rest, "@")
		seg := rest
		if i >= 0 {
			seg = rest[:i]
			rest = rest[i:]
		} else {
			rest = ""
		}
		seg = strings.TrimSpace(seg)
		if seg != "" {
			segs = append(segs, seg)
		}
	}
	return segs
}

// chainSplitIdx 分离末尾 .N 索引（返回 base 与索引；无索引时 idx=-1）。
func chainSplitIdx(seg string) (string, int) {
	m := regexp.MustCompile(`^(.*)\.(\d+)$`).FindStringSubmatch(seg)
	if m == nil {
		return seg, -1
	}
	n, _ := strconv.Atoi(m[2])
	return m[1], n
}

// chainApplySel 基础选择器（class./id./tag./text. 或标准 CSS）匹配子孙节点 + .N。
func chainApplySel(nodes []*html.Node, seg string) []*html.Node {
	base, idx := chainSplitIdx(seg)
	base = strings.TrimSpace(base)
	if base == "" {
		return nodes
	}
	var out []*html.Node
	if m := regexp.MustCompile(`^(class|id|tag|text)\.(.+)$`).FindStringSubmatch(base); m != nil {
		kind, name := m[1], m[2]
		if kind == "text" {
			for _, n := range nodes {
				out = append(out, chainFindText(n, name)...)
			}
		} else {
			selStr := name
			if kind == "class" {
				selStr = "." + name
			} else if kind == "id" {
				selStr = "#" + name
			}
			sel, err := cascadia.Compile(selStr)
			if err != nil {
				return nil
			}
			for _, n := range nodes {
				out = append(out, sel.MatchAll(n)...)
			}
		}
	} else {
		sel, err := cascadia.Compile(base)
		if err != nil {
			return nil
		}
		for _, n := range nodes {
			out = append(out, sel.MatchAll(n)...)
		}
	}
	if idx >= 0 && idx < len(out) {
		return []*html.Node{out[idx]}
	}
	return out
}

// chainFindText 找文本包含 name 的后代元素。
func chainFindText(n *html.Node, name string) []*html.Node {
	var out []*html.Node
	var walk func(*html.Node)
	walk = func(node *html.Node) {
		if node.Type == html.ElementNode && strings.Contains(extractText(node), name) {
			out = append(out, node)
		}
		for ch := node.FirstChild; ch != nil; ch = ch.NextSibling {
			walk(ch)
		}
	}
	walk(n)
	return out
}

// chainIsOutput 段是否为输出操作（text/html/属性/js:/json:/regex:；tag./css:/xpath: 为推进）。
func chainIsOutput(seg string) bool {
	base, _ := chainSplitIdx(seg)
	base = strings.TrimSpace(base)
	if strings.HasPrefix(base, "tag.") || strings.HasPrefix(base, "css:") || strings.HasPrefix(base, "xpath:") {
		return false
	}
	return true
}

// chainAdvance 推进操作：tag.x / css:... / xpath:... 匹配子孙节点 + .N。
func chainAdvance(nodes []*html.Node, seg string) []*html.Node {
	base, idx := chainSplitIdx(seg)
	base = strings.TrimSpace(base)
	var out []*html.Node
	switch {
	case strings.HasPrefix(base, "tag."):
		name := strings.TrimSpace(base[4:])
		if name == "" {
			return nil
		}
		sel, err := cascadia.Compile(name)
		if err != nil {
			return nil
		}
		for _, n := range nodes {
			out = append(out, sel.MatchAll(n)...)
		}
	case strings.HasPrefix(base, "css:"):
		param := strings.TrimSpace(base[4:])
		sel, err := cascadia.Compile(param)
		if err != nil {
			return nil
		}
		for _, n := range nodes {
			out = append(out, sel.MatchAll(n)...)
		}
	case strings.HasPrefix(base, "xpath:"):
		expr := strings.TrimSpace(base[6:])
		for _, n := range nodes {
			if ns, err := htmlquery.QueryAll(n, expr); err == nil {
				out = append(out, ns...)
			}
		}
	default:
		return nil
	}
	if idx >= 0 && idx < len(out) {
		return []*html.Node{out[idx]}
	}
	return out
}

// chainOutput 输出操作：text / html / 属性 / js: / json: / regex:（对节点文本），+ .N。
func chainOutput(nodes []*html.Node, seg string, ctx *Context) []string {
	base, idx := chainSplitIdx(seg)
	base = strings.TrimSpace(base)
	ns := nodes
	if idx >= 0 && idx < len(ns) {
		ns = []*html.Node{ns[idx]}
	}
	var out []string
	for _, n := range ns {
		switch {
		case base == "text":
			out = append(out, extractText(n))
		case base == "html":
			out = append(out, htmlquery.OutputHTML(n, true))
		case strings.HasPrefix(base, "js:"):
			out = append(out, evalJS(extractText(n), strings.TrimSpace(base[3:]), ctx)...)
		case strings.HasPrefix(base, "json:"):
			out = append(out, evalJSON(extractText(n), strings.TrimSpace(base[5:]), ctx)...)
		case strings.HasPrefix(base, "regex:"):
			out = append(out, regexMatchAll(extractText(n), strings.TrimSpace(base[6:]))...)
		default:
			// 属性（href/src/data-x...）
			if v := attrValue(n, base); v != "" {
				out = append(out, v)
			}
		}
	}
	return out
}

// evalXPath XPath：@xpath:expr。
func evalXPath(input, expr string, ctx *Context) []string {
	doc, err := htmlquery.Parse(strings.NewReader(input))
	if err != nil {
		return nil
	}
	nodes, err := htmlquery.QueryAll(doc, expr)
	if err != nil {
		return nil
	}
	var out []string
	for _, n := range nodes {
		out = append(out, strings.TrimSpace(htmlquery.InnerText(n)))
	}
	return out
}

// evalJSON JSONPath：@json:$.a.b[0]。
func evalJSON(input, expr string, ctx *Context) []string {
	// legado：$.path##pattern##repl（JSONPath 提取后正则替换，如
	// $.chapter_title##正文卷.|正文.## / $.intro##(^|[。！？]+[”」）】]?)##$1<br>）
	replSpec := ""
	if idx := strings.Index(expr, "##"); idx >= 0 {
		replSpec = expr[idx+2:]
		expr = expr[:idx]
	}
	var out []string
	// legado：&& 连接多个 JSONPath 表达式（结果合并，如 $..book_data[0]&&$.data[*]）
	if strings.Contains(expr, "&&") {
		for _, part := range strings.Split(expr, "&&") {
			out = append(out, evalJSONPath(input, strings.TrimSpace(part), ctx)...)
		}
	} else {
		out = evalJSONPath(input, expr, ctx)
	}
	return applyJSONReplace(out, replSpec)
}

// evalJSONPath 单条 JSONPath 提取（无 ## 替换）。
func evalJSONPath(input, expr string, ctx *Context) []string {
	var v any
	if err := json.Unmarshal([]byte(input), &v); err != nil {
		return nil
	}
	if !strings.HasPrefix(expr, "$") {
		expr = "$." + expr
	}
	res, err := jsonpath.Get(expr, v)
	if err != nil {
		return nil
	}
	switch t := res.(type) {
	case []any:
		var out []string
		for _, item := range t {
			out = append(out, jsonScalarToString(item))
		}
		return out
	case map[string]any:
		b, _ := json.Marshal(t)
		return []string{string(b)}
	default:
		return []string{jsonScalarToString(res)}
	}
}

// applyJSONReplace 对 JSONPath 结果应用 ##pattern##repl 正则替换（repl 缺省为删除匹配）。
func applyJSONReplace(items []string, replSpec string) []string {
	if replSpec == "" {
		return items
	}
	parts := strings.SplitN(replSpec, "##", 2)
	pattern := parts[0]
	replacement := ""
	if len(parts) >= 2 {
		replacement = parts[1]
	}
	re, err := regexp2.Compile(pattern, regexp2.None)
	if err != nil {
		return items
	}
	var out []string
	for _, s := range items {
		if m, err := re.FindStringMatch(s); err == nil && m != nil {
			if r, err := re.Replace(s, replacement, -1, -1); err == nil {
				out = append(out, r)
				continue
			}
		}
		out = append(out, s)
	}
	return out
}

func jsonScalarToString(v any) string {
	switch t := v.(type) {
	case string:
		return t
	case float64:
		return floatToStr(t)
	case bool:
		return boolToStr(t)
	case nil:
		return ""
	default:
		b, _ := json.Marshal(v)
		return string(b)
	}
}

// evalRegex 正则：@regex:pattern。
func evalRegex(input, pattern string, ctx *Context) []string {
	return regexMatchAll(input, pattern)
}

// regexMatchAll 正则匹配（支持捕获组：有组返回组 1，否则返回整体匹配）。
func regexMatchAll(input, pattern string) []string {
	re, err := regexp2.Compile(pattern, regexp2.None)
	if err != nil {
		// 降级标准正则
		re2, err2 := regexp.Compile(pattern)
		if err2 != nil {
			return nil
		}
		matches := re2.FindAllString(input, -1)
		if len(matches) == 0 {
			return nil
		}
		return matches
	}
	m, err := re.FindStringMatch(input)
	if err != nil || m == nil {
		return nil
	}
	var out []string
	for {
		if m.GroupCount() > 1 {
			g := m.GroupByNumber(1)
			if g != nil {
				out = append(out, g.Capture.String())
			}
		} else {
			out = append(out, m.String())
		}
		m, err = re.FindNextMatch(m)
		if err != nil || m == nil {
			break
		}
	}
	return out
}

// evalJS JS 求值：@js:code（输入绑定为 result）。
func evalJS(input, code string, ctx *Context) []string {
	vm := goja.New()
	_ = vm.Set("result", input)
	for k, v := range ctx.Variables {
		_ = vm.Set(k, v)
	}
	// java 兼容 shim（最小子集）
	_ = vm.Set("java", map[string]any{
		"url":            map[string]any{"encode": func(s string) string { return strings.ReplaceAll(url.QueryEscape(s), "+", "%20") }},
		"base64Decode":   func(s string) string { return s },
		"base64Encode":   func(s string) string { return s },
	})
	val, err := vm.RunString(code)
	if err != nil {
		return nil
	}
	switch v := val.Export().(type) {
	case string:
		return []string{v}
	case []any:
		var out []string
		for _, item := range v {
			if s, ok := item.(string); ok {
				out = append(out, s)
			}
		}
		return out
	default:
		if v == nil {
			return nil
		}
		return []string{toStr(v)}
	}
}

// evalGet 从 URL/输入中提取：@get:regex|queryKey。
func evalGet(input, param string, ctx *Context) []string {
	pattern, key := splitPipe(param)
	if pattern != "" {
		if out := regexMatchAll(input, pattern); len(out) > 0 {
			return out
		}
	}
	if key != "" {
		// query 参数提取
		for _, part := range strings.Split(input, "&") {
			kv := strings.SplitN(part, "=", 2)
			if len(kv) == 2 && kv[0] == key {
				return []string{kv[1]}
			}
		}
	}
	return nil
}

// processTextOps 文本处理操作：## 正则替换、@@ 连接、! 取反、纯文本。
func processTextOps(input, rule string, ctx *Context) []string {
	// 替换 ##pattern##replacement##flags
	if strings.Contains(rule, "##") {
		parts := strings.Split(rule, "##")
		if len(parts) >= 3 {
			re, err := regexp2.Compile(parts[1], regexp2.None)
			if err != nil {
				return nil
			}
			repl := parts[2]
			m, err := re.FindStringMatch(input)
			if err != nil || m == nil {
				return nil
			}
			out, err := re.Replace(input, repl, -1, -1)
			if err != nil {
				return nil
			}
			return []string{out}
		}
	}
	// 连接 @@
	if strings.HasPrefix(rule, "@@") {
		return []string{strings.ReplaceAll(input, "\n", "")}
	}
	// 纯文本：原样返回（已过滤空）
	if input != "" {
		return []string{input}
	}
	return nil
}

// ---------- 工具 ----------

// interpolateVars {name}/{{name}} 变量插值（legado：{{$.path}} 从输入 JSON 提取）。
func interpolateVars(input, rule string, ctx *Context) string {
	if !strings.Contains(rule, "{") || ctx == nil {
		return rule
	}
	// 优先 {{name}}（legado 双花括号），再 {name}
	re := regexp.MustCompile(`\{\{([^{}]+)\}\}`)
	rule = re.ReplaceAllStringFunc(rule, func(m string) string {
		return interpolateOne(input, strings.TrimSuffix(strings.TrimPrefix(m, "{{"), "}}"), ctx, m)
	})
	re2 := regexp.MustCompile(`\{([^{}]+)\}`)
	return re2.ReplaceAllStringFunc(rule, func(m string) string {
		return interpolateOne(input, strings.TrimSuffix(strings.TrimPrefix(m, "{"), "}"), ctx, m)
	})
}

// interpolateOne 单变量插值：$. 前缀从输入 JSONPath 提取，否则查 ctx 变量。
func interpolateOne(input, name string, ctx *Context, fallback string) string {
	if strings.HasPrefix(name, "$") {
		if vs := evalJSON(input, name, ctx); len(vs) > 0 {
			return vs[0]
		}
		return ""
	}
	if v := ctx.Get(name); v != "" {
		return v
	}
	return fallback
}

// splitTopLevel 顶层分隔（不进入括号/引号）。
func splitTopLevel(s, sep string) []string {
	var parts []string
	start := 0
	depth := 0
	for i := 0; i < len(s); i++ {
		switch s[i] {
		case '(', '[', '{':
			depth++
		case ')', ']', '}':
			if depth > 0 {
				depth--
			}
		}
		if depth == 0 && strings.HasPrefix(s[i:], sep) {
			parts = append(parts, s[start:i])
			i += len(sep) - 1
			start = i + 1
		}
	}
	parts = append(parts, s[start:])
	return parts
}

func splitPipe(s string) (string, string) {
	idx := strings.IndexByte(s, '|')
	if idx < 0 {
		return s, ""
	}
	return s[:idx], s[idx+1:]
}

func boolToStr(b bool) string {
	if b {
		return "true"
	}
	return "false"
}

func floatToStr(f float64) string {
	return strings.TrimRight(strings.TrimRight(json.Number(jsonFmt(f)).String(), "0"), ".")
}

func jsonFmt(f float64) string {
	b, _ := json.Marshal(f)
	return string(b)
}

func toStr(v any) string {
	switch t := v.(type) {
	case string:
		return t
	case float64:
		return floatToStr(t)
	default:
		b, _ := json.Marshal(v)
		return string(b)
	}
}
