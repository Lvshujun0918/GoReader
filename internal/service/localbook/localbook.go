// Package localbook 本地书解析：txt（GBK/UTF-8/GB18030 编码检测 + 目录规则切章）、
// epub（OPF/NCX 章节结构）与 html（标题切章）。
package localbook

import (
	"archive/zip"
	"bytes"
	"encoding/xml"
	"html"
	"io"
	"path"
	"regexp"
	"sort"
	"strings"
	"unicode/utf8"

	xhtml "golang.org/x/net/html"
	"golang.org/x/text/encoding/simplifiedchinese"
	"golang.org/x/text/encoding/unicode"
	"golang.org/x/text/transform"

	"github.com/Lvshujun0918/GoReader/internal/model"
)

// Chapter 解析出的章节。
type Chapter struct {
	Title   string `json:"title"`
	Content string `json:"content"`
}

// Book 解析结果。
type Book struct {
	Name     string
	Author   string
	Format   string // "txt" / "epub"
	Charset  string // txt 检测出的编码（utf-8/gbk/gb18030/utf-16）
	Chapters []Chapter
}

// DefaultTocRules 无启用目录规则时的内置默认正则（与 importDefaultTxtTocRules 一致）。
var DefaultTocRules = []string{
	`^\s*(第[0-9一二三四五六七八九十百千万零〇两]+[章节卷部集篇回]).*`,
}

// Parse 按扩展名解析本地书文件。
// tocRules：启用的 TXT 目录规则正则（txt 用；为空则用 DefaultTocRules）。
func Parse(data []byte, filename string, tocRules []string) (*Book, error) {
	ext := strings.ToLower(path.Ext(filename))
	switch ext {
	case ".txt":
		return parseTxt(data, filename, tocRules)
	case ".epub":
		return parseEpub(data)
	case ".html", ".htm":
		return parseHTML(data, filename)
	default:
		return nil, &UnsupportedError{Ext: ext}
	}
}

// UnsupportedError 不支持的格式。
type UnsupportedError struct{ Ext string }

func (e *UnsupportedError) Error() string {
	return "不支持的格式：" + e.Ext + "（仅支持 .txt / .epub / .html）"
}

/* ------------------------------ HTML ------------------------------ */

var (
	htmlHeadingTags = map[string]bool{"h1": true, "h2": true, "h3": true, "h4": true, "h5": true, "h6": true}
	htmlBlockTags   = map[string]bool{
		"p": true, "div": true, "br": true, "li": true, "tr": true, "td": true,
		"section": true, "article": true, "blockquote": true, "pre": true,
	}
)

// parseHTML 解析 html 文件：<title> 作书名，h1-h6 作章节标题切章；无标题则整书一章。
func parseHTML(data []byte, filename string) (*Book, error) {
	doc, err := xhtml.Parse(bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	// 书名：<title> 优先，否则文件名
	title := ""
	var findTitle func(*xhtml.Node)
	findTitle = func(n *xhtml.Node) {
		if title != "" {
			return
		}
		if n.Type == xhtml.ElementNode && strings.EqualFold(n.Data, "title") {
			title = strings.TrimSpace(htmlText(n))
			return
		}
		for ch := n.FirstChild; ch != nil; ch = ch.NextSibling {
			findTitle(ch)
		}
	}
	findTitle(doc)
	if title == "" {
		title = strings.TrimSuffix(path.Base(filename), path.Ext(filename))
	}

	var body *xhtml.Node
	var findBody func(*xhtml.Node)
	findBody = func(n *xhtml.Node) {
		if body != nil {
			return
		}
		if n.Type == xhtml.ElementNode && strings.EqualFold(n.Data, "body") {
			body = n
			return
		}
		for ch := n.FirstChild; ch != nil; ch = ch.NextSibling {
			findBody(ch)
		}
	}
	findBody(doc)

	var chapters []Chapter
	cur := &Chapter{Title: "全文"}
	var walk func(*xhtml.Node)
	walk = func(n *xhtml.Node) {
		if n.Type == xhtml.TextNode {
			cur.Content += n.Data
			return
		}
		if n.Type != xhtml.ElementNode {
			return
		}
		tag := strings.ToLower(n.Data)
		if htmlHeadingTags[tag] {
			// 保存当前章，标题开新章（首个标题前的正文并入第一章）
			if strings.TrimSpace(cur.Content) != "" || len(chapters) > 0 {
				chapters = append(chapters, *cur)
			}
			cur = &Chapter{Title: strings.TrimSpace(htmlText(n))}
			return
		}
		if htmlBlockTags[tag] {
			cur.Content += "\n"
		}
		for ch := n.FirstChild; ch != nil; ch = ch.NextSibling {
			walk(ch)
		}
		if htmlBlockTags[tag] && tag != "br" {
			cur.Content += "\n"
		}
	}
	if body != nil {
		walk(body)
	}
	if strings.TrimSpace(cur.Content) != "" {
		chapters = append(chapters, *cur)
	}
	if len(chapters) == 0 {
		chapters = append(chapters, Chapter{Title: "全文", Content: ""})
	}
	for i := range chapters {
		chapters[i].Content = normalizeHTMLText(chapters[i].Content)
	}
	return &Book{Name: title, Author: "", Format: "html", Chapters: chapters}, nil
}

// htmlText 提取节点内全部文本（title / 标题用）。
func htmlText(n *xhtml.Node) string {
	var b strings.Builder
	var walk func(*xhtml.Node)
	walk = func(x *xhtml.Node) {
		if x.Type == xhtml.TextNode {
			b.WriteString(x.Data)
		}
		for ch := x.FirstChild; ch != nil; ch = ch.NextSibling {
			walk(ch)
		}
	}
	walk(n)
	return b.String()
}

// normalizeHTMLText 压缩空白/全角空格/空行，保留 \n 段落。
func normalizeHTMLText(s string) string {
	out := regexp.MustCompile(`[ \t]+`).ReplaceAllString(s, " ")
	out = regexp.MustCompile(`\x{3000}+`).ReplaceAllString(out, "")
	out = regexp.MustCompile(`\n\s*\n+`).ReplaceAllString(out, "\n")
	return strings.TrimSpace(out)
}

/* ------------------------------ 编码检测 ------------------------------ */

// DecodeText 检测并解码 txt 文本字节为 UTF-8 字符串。
func DecodeText(data []byte) (string, string) {
	// BOM 检测
	if len(data) >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF {
		return string(data[3:]), "utf-8"
	}
	if len(data) >= 4 && data[0] == 0x84 && data[1] == 0x31 && data[2] == 0x95 && data[3] == 0x33 {
		// GB18030 BOM
		if s, err := decodeGB(data[4:], true); err == nil {
			return s, "gb18030"
		}
	}
	if len(data) >= 2 {
		if data[0] == 0xFF && data[1] == 0xFE {
			return decodeUTF16(data[2:], true), "utf-16le"
		}
		if data[0] == 0xFE && data[1] == 0xFF {
			return decodeUTF16(data[2:], false), "utf-16be"
		}
	}
	// 无 BOM：UTF-8 合法 → UTF-8；否则 GBK（回退 GB18030）
	if utf8.Valid(data) {
		return string(data), "utf-8"
	}
	if s, err := decodeGB(data, false); err == nil {
		return s, "gbk"
	}
	if s, err := decodeGB(data, true); err == nil {
		return s, "gb18030"
	}
	// 全部失败：按 UTF-8 原样返回（保留数据，避免导入失败）
	return string(data), "utf-8"
}

func decodeGB(data []byte, gb18030 bool) (string, error) {
	var dec *transform.Reader
	if gb18030 {
		dec = transform.NewReader(bytes.NewReader(data), simplifiedchinese.GB18030.NewDecoder())
	} else {
		dec = transform.NewReader(bytes.NewReader(data), simplifiedchinese.GBK.NewDecoder())
	}
	s, err := io.ReadAll(dec)
	if err != nil {
		return "", err
	}
	return string(s), nil
}

func decodeUTF16(data []byte, little bool) string {
	e := unicode.BigEndian
	if little {
		e = unicode.LittleEndian
	}
	enc := unicode.UTF16(e, unicode.IgnoreBOM)
	s, _, err := transform.Bytes(enc.NewDecoder(), data)
	if err != nil {
		return string(data)
	}
	return string(s)
}

/* ------------------------------ TXT ------------------------------ */

func parseTxt(data []byte, filename string, tocRules []string) (*Book, error) {
	text, charset := DecodeText(data)
	// 规范化换行
	text = strings.ReplaceAll(text, "\r\n", "\n")
	text = strings.ReplaceAll(text, "\r", "\n")
	lines := strings.Split(text, "\n")

	// 收集标题正则：启用规则 + 内置默认
	patterns := make([]*regexp.Regexp, 0, len(tocRules)+len(DefaultTocRules))
	for _, r := range tocRules {
		if re, err := regexp.Compile(r); err == nil {
			patterns = append(patterns, re)
		}
	}
	for _, r := range DefaultTocRules {
		if re, err := regexp.Compile(r); err == nil {
			patterns = append(patterns, re)
		}
	}

	isTitle := func(line string) bool {
		for _, re := range patterns {
			if re.MatchString(line) {
				return true
			}
		}
		return false
	}

	var chapters []Chapter
	var cur *Chapter
	var pending []string // 首个标题前的非标题行（并入第一章）
	for _, line := range lines {
		trimmed := strings.TrimSpace(line)
		if trimmed == "" {
			if cur != nil {
				cur.Content += "\n"
			}
			continue
		}
		if isTitle(trimmed) {
			if cur != nil {
				chapters = append(chapters, *cur)
			}
			cur = &Chapter{Title: cleanTitle(trimmed)}
			// 前一章结束后，pending 已清空；若这是第一章标题，pending 并入
			if len(chapters) == 0 && len(pending) > 0 {
				cur.Content = strings.Join(pending, "\n") + "\n"
				pending = nil
			}
			continue
		}
		if cur == nil {
			pending = append(pending, trimmed)
			continue
		}
		cur.Content += trimmed + "\n"
	}
	if cur != nil {
		chapters = append(chapters, *cur)
	}
	// 无任何标题匹配 → 整书一章（含 pending 与全部行）
	if len(chapters) == 0 {
		if len(pending) > 0 {
			chapters = append(chapters, Chapter{Title: "全文", Content: strings.Join(pending, "\n")})
		} else {
			chapters = append(chapters, Chapter{Title: "全文", Content: text})
		}
	}

	// 书名：优先用文件名（去扩展名）；异常时回退正文猜测
	name := guessName(lines, isTitle)
	if n := strings.TrimSpace(strings.TrimSuffix(path.Base(filename), path.Ext(filename))); n != "" && n != "未命名" {
		name = n
	}
	return &Book{Name: name, Author: "", Format: "txt", Charset: charset, Chapters: chapters}, nil
}

// cleanTitle 清理章节标题：去掉首尾空白、行号前缀等。
func cleanTitle(s string) string {
	s = strings.TrimSpace(s)
	// 去掉可能的前缀编号（如 "001 第一章 xxx" 保留后半）
	return s
}

// guessName 从 txt 前若干行猜测书名：跳过空行与章节标题行，取首个非空行。
func guessName(lines []string, isTitle func(string) bool) string {
	for _, l := range lines[:min(len(lines), 20)] {
		t := strings.TrimSpace(l)
		if t == "" {
			continue
		}
		// 跳过明显的目录标题行与章节标题行
		if t == "目录" {
			continue
		}
		if isTitle(t) {
			continue
		}
		return strings.TrimSpace(strings.TrimRight(t, "　 "))
	}
	return "未命名"
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

/* ------------------------------ EPUB ------------------------------ */

type opfManifestItem struct {
	ID        string `xml:"id,attr"`
	Href      string `xml:"href,attr"`
	MediaType string `xml:"media-type,attr"`
}

type opfSpineRef struct {
	IDRef string `xml:"idref,attr"`
}

type opfMetadata struct {
	Title   string `xml:"title"`
	Creator string `xml:"creator"`
	Lang    string `xml:"lang,attr"`
}

type opfManifest struct {
	Items []opfManifestItem `xml:"item"`
}

type opfSpine struct {
	Refs []opfSpineRef `xml:"itemref"`
}

type opfPackage struct {
	XMLName  xml.Name     `xml:"package"`
	Metadata opfMetadata  `xml:"metadata"`
	Manifest opfManifest  `xml:"manifest"`
	Spine    opfSpine     `xml:"spine"`
}

type containerRootfile struct {
	FullPath string `xml:"full-path,attr"`
}

type container struct {
	Rootfiles []containerRootfile `xml:"rootfiles>rootfile"`
}

func parseEpub(data []byte) (*Book, error) {
	zr, err := zip.NewReader(bytes.NewReader(data), int64(len(data)))
	if err != nil {
		return nil, err
	}
	// 文件名 → 内容
	files := map[string][]byte{}
	for _, f := range zr.File {
		rc, err := f.Open()
		if err != nil {
			continue
		}
		b, _ := io.ReadAll(rc)
		rc.Close()
		files[strings.TrimPrefix(f.Name, "/")] = b
	}

	// container.xml → OPF 路径
	opfPath := "OEBPS/content.opf"
	if raw, ok := files["META-INF/container.xml"]; ok {
		var c container
		if xml.Unmarshal(raw, &c) == nil && len(c.Rootfiles) > 0 && c.Rootfiles[0].FullPath != "" {
			opfPath = c.Rootfiles[0].FullPath
		}
	}
	opfRaw, ok := files[opfPath]
	if !ok {
		// 尝试大小写不敏感查找
		opfPath = findCI(files, "content.opf")
		if opfPath == "" {
			return nil, &FormatError{Msg: "epub 缺少 OPF 目录文件"}
		}
		opfRaw = files[opfPath]
	}

	var pkg opfPackage
	if err := xml.Unmarshal(opfRaw, &pkg); err != nil {
		return nil, &FormatError{Msg: "epub OPF 解析失败"}
	}

	// manifest: id → item
	itemsByID := map[string]opfManifestItem{}
	for _, it := range pkg.Manifest.Items {
		itemsByID[it.ID] = it
	}

	opfDir := path.Dir(opfPath)
	resolveHref := func(href string) string {
		// href 可能是相对 OPF 目录，也可能含查询/锚点
		href = strings.SplitN(href, "#", 2)[0]
		if href == "" {
			return ""
		}
		if strings.HasPrefix(href, "/") {
			return strings.TrimPrefix(href, "/")
		}
		return path.Join(opfDir, href)
	}

	// 标题提取：h1/h2/h3/title 取首个
	titlePatterns := []*regexp.Regexp{
		regexp.MustCompile(`(?is)<h1\b[^>]*>(.*?)</h1\s*>`),
		regexp.MustCompile(`(?is)<h2\b[^>]*>(.*?)</h2\s*>`),
		regexp.MustCompile(`(?is)<h3\b[^>]*>(.*?)</h3\s*>`),
		regexp.MustCompile(`(?is)<title\b[^>]*>(.*?)</title\s*>`),
	}
	extractTitle := func(text string) string {
		for _, re := range titlePatterns {
			if m := re.FindStringSubmatch(text); len(m) > 1 {
				if t := cleanHTMLText(m[1]); t != "" {
					return t
				}
			}
		}
		return ""
	}

	var chapters []Chapter
	seen := map[string]bool{}
	for _, ref := range pkg.Spine.Refs {
		it, ok := itemsByID[ref.IDRef]
		if !ok {
			continue
		}
		if !strings.Contains(strings.ToLower(it.MediaType), "html") &&
			!strings.Contains(strings.ToLower(it.Href), ".htm") &&
			!strings.Contains(strings.ToLower(it.Href), ".xhtml") {
			continue
		}
		filePath := resolveHref(it.Href)
		if filePath == "" {
			continue
		}
		// 处理大小写（zip 内大小写敏感）
		raw, ok := files[filePath]
		if !ok {
			if alt := findCI(files, path.Base(filePath)); alt != "" {
				raw = files[alt]
			}
		}
		if len(raw) == 0 {
			continue
		}
		// 内容解码（xhtml 可能是 UTF-8 或 GBK；多数为 UTF-8）
		text, _ := DecodeText(raw)
		title := extractTitle(text)
		if title == "" {
			title = strings.TrimSuffix(path.Base(filePath), path.Ext(filePath))
		}
		content := htmlToText(text)
		if strings.TrimSpace(content) == "" {
			continue
		}
		key := filePath + "|" + title
		if seen[key] {
			continue
		}
		seen[key] = true
		chapters = append(chapters, Chapter{Title: title, Content: content})
	}

	if len(chapters) == 0 {
		return nil, &FormatError{Msg: "epub 未解析到任何章节"}
	}

	return &Book{
		Name:     cleanHTMLText(pkg.Metadata.Title),
		Author:   cleanHTMLText(pkg.Metadata.Creator),
		Format:   "epub",
		Charset:  "utf-8",
		Chapters: chapters,
	}, nil
}

// findCI 在 map 中大小写不敏感查找含关键字的文件名（zip 内大小写可能不一致）。
func findCI(files map[string][]byte, keyword string) string {
	kw := strings.ToLower(keyword)
	for name := range files {
		if strings.Contains(strings.ToLower(name), kw) {
			return name
		}
	}
	return ""
}

// cleanHTMLText 提取标签内纯文本。
func cleanHTMLText(s string) string {
	s = stripTags(s)
	return strings.TrimSpace(s)
}

var tagRe = regexp.MustCompile(`(?s)<[^>]+>`)

func stripTags(s string) string {
	return html.UnescapeString(tagRe.ReplaceAllString(s, ""))
}

// htmlToText HTML → 纯文本：块级/换行标签转换行，其余标签去除，实体解码。
func htmlToText(s string) string {
	// 去掉 script/style/head/svg 块
	for _, tag := range []string{"script", "style", "head", "svg"} {
		s = regexp.MustCompile(`(?is)<` + tag + `\b[^>]*>.*?</` + tag + `\s*>`).ReplaceAllString(s, "")
	}
	// 块级标签 → 换行
	s = regexp.MustCompile(`(?i)<\s*(br|/p|/div|/h[1-6]|/li|/tr|/section|/article|/blockquote|/table|/ul|/ol|hr)\b[^>]*>`).ReplaceAllString(s, "\n")
	// 其余标签删除
	s = tagRe.ReplaceAllString(s, "")
	// 实体解码
	s = html.UnescapeString(s)
	// 清理空行
	lines := strings.Split(s, "\n")
	out := make([]string, 0, len(lines))
	for _, l := range lines {
		t := strings.TrimSpace(l)
		if t != "" {
			out = append(out, t)
		}
	}
	return strings.Join(out, "\n")
}

// FormatError 文件格式错误。
type FormatError struct{ Msg string }

func (e *FormatError) Error() string { return e.Msg }

// TocRulesFromModel 从模型规则取启用正则（按 serialNumber 升序）。
func TocRulesFromModel(rules []model.TxtTocRule) []string {
	enabled := make([]model.TxtTocRule, 0, len(rules))
	for _, r := range rules {
		if r.Enabled != 0 && strings.TrimSpace(r.Rule) != "" {
			enabled = append(enabled, r)
		}
	}
	sort.Slice(enabled, func(i, j int) bool { return enabled[i].SerialNumber < enabled[j].SerialNumber })
	out := make([]string, 0, len(enabled))
	for _, r := range enabled {
		out = append(out, r.Rule)
	}
	return out
}
