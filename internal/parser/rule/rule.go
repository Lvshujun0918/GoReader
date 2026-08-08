// Package rule 书源多规则引擎（兼容 legado：@css / @xpath / @json / @regex / @js / @get 等）。
package rule

import (
	"bytes"
	"encoding/json"
	"net/url"
	"regexp"
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
	rule = interpolateVars(rule, ctx)
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
	// 纯文本规则（无 @ 前缀）：正则全文匹配或原样返回
	if !strings.HasPrefix(rule, "@") {
		// 常见：直接当正则匹配第一个匹配组
		if out := regexMatchAll(input, rule); len(out) > 0 {
			return out
		}
		if input != "" {
			return []string{input}
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
		return nil
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
	// legado：&& 连接多个 JSONPath 表达式（结果合并，如 $..book_data[0]&&$.data[*]）
	if strings.Contains(expr, "&&") {
		var out []string
		for _, part := range strings.Split(expr, "&&") {
			out = append(out, evalJSON(input, strings.TrimSpace(part), ctx)...)
		}
		return out
	}
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

// interpolateVars {name} 变量插值。
func interpolateVars(rule string, ctx *Context) string {
	if !strings.Contains(rule, "{") || ctx == nil {
		return rule
	}
	re := regexp.MustCompile(`\{([^{}]+)\}`)
	return re.ReplaceAllStringFunc(rule, func(m string) string {
		name := strings.TrimSuffix(strings.TrimPrefix(m, "{"), "}")
		if v := ctx.Get(name); v != "" {
			return v
		}
		return m
	})
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
