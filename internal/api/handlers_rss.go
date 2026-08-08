package api

import (
	"encoding/json"
	"encoding/xml"
	"fmt"
	"strings"
	"time"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/GoReader/internal/model"
	"github.com/Lvshujun0918/GoReader/internal/service/crawler"
)

// ---------------- RSS 解析（RSS 2.0 / Atom） ----------------

type rssDoc struct {
	XMLName xml.Name   `xml:"rss"`
	Channel rssChannel `xml:"channel"`
}

type rssChannel struct {
	Title       string    `xml:"title"`
	Description string    `xml:"description"`
	Items       []rssItem `xml:"item"`
}

type rssItem struct {
	Title       string `xml:"title"`
	Link        string `xml:"link"`
	Description string `xml:"description"`
	Content     string `xml:"encoded"`
	PubDate     string `xml:"pubDate"`
	Author      string `xml:"author"`
}

type atomDoc struct {
	XMLName xml.Name   `xml:"feed"`
	Title   string     `xml:"title"`
	Entries []atomEntry `xml:"entry"`
}

type atomEntry struct {
	Title   string `xml:"title"`
	Link    struct {
		Href string `xml:"href,attr"`
	} `xml:"link"`
	Content struct {
		Body string `xml:",chardata"`
	} `xml:"content"`
	Summary string `xml:"summary"`
	Updated string `xml:"updated"`
	Author  struct {
		Name string `xml:"name"`
	} `xml:"author"`
}

// parseFeed 解析 RSS/Atom 内容为文章列表。
func parseFeed(body []byte, sourceURL string) ([]*model.RssArticle, error) {
	// 尝试 RSS 2.0
	var rss rssDoc
	if err := xml.Unmarshal(body, &rss); err == nil && rss.Channel.Title != "" {
		var out []*model.RssArticle
		for _, item := range rss.Channel.Items {
			content := item.Content
			if content == "" {
				content = item.Description
			}
			out = append(out, &model.RssArticle{
				URL:       item.Link,
				SourceURL: sourceURL,
				Title:     item.Title,
				Author:    item.Author,
				Time:      parseRSSDate(item.PubDate),
				Content:   content,
			})
		}
		return out, nil
	}
	// Atom
	var atom atomDoc
	if err := xml.Unmarshal(body, &atom); err == nil && atom.Title != "" {
		var out []*model.RssArticle
		for _, e := range atom.Entries {
			content := e.Content.Body
			if content == "" {
				content = e.Summary
			}
			out = append(out, &model.RssArticle{
				URL:       e.Link.Href,
				SourceURL: sourceURL,
				Title:     e.Title,
				Author:    e.Author.Name,
				Time:      parseRSSDate(e.Updated),
				Content:   content,
			})
		}
		return out, nil
	}
	return nil, fmt.Errorf("无法解析 RSS 源")
}

func parseRSSDate(s string) int64 {
	for _, layout := range []string{
		time.RFC1123Z, time.RFC1123, time.RFC3339,
		"Mon, 02 Jan 2006 15:04:05 -0700", "2006-01-02T15:04:05Z",
	} {
		if t, err := time.Parse(layout, strings.TrimSpace(s)); err == nil {
			return t.UnixMilli()
		}
	}
	return 0
}

// fetchRss 抓取 RSS 源（经 crawler 客户端：SSRF 防护 + 统一 UA + 响应限制）。
func fetchRss(url string) ([]byte, error) {
	return crawler.New(nil, "").Fetch(url, nil)
}

// ---------------- 处理器 ----------------

func (a *API) handleGetRssSources(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	list, err := a.Storage.ListRssSources(ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, list)
}

func (a *API) handleSaveRssSource(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	var raw map[string]any
	if err := c.ShouldBindJSON(&raw); err != nil {
		Fail(c, "参数错误")
		return
	}
	src, err := normalizeRssSourceMap(raw)
	if err != nil {
		Fail(c, "参数错误")
		return
	}
	if src.RssSourceURL == "" {
		Fail(c, "RSS 源地址不能为空")
		return
	}
	if err := a.Storage.SaveRssSource(ns, src); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleSaveRssSources(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	var raw []map[string]any
	if err := c.ShouldBindJSON(&raw); err != nil {
		Fail(c, "参数错误")
		return
	}
	list := make([]*model.RssSource, 0, len(raw))
	for _, m := range raw {
		src, err := normalizeRssSourceMap(m)
		if err != nil || src.RssSourceURL == "" {
			continue
		}
		list = append(list, src)
	}
	if len(list) == 0 {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.SaveRssSources(ns, list); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// normalizeRssSourceMap 宽松归一化 RSS 源（legado 布尔 enabled → 0/1，后端 int 列）。
func normalizeRssSourceMap(m map[string]any) (*model.RssSource, error) {
	switch v := m["enabled"].(type) {
	case bool:
		if v {
			m["enabled"] = 1
		} else {
			m["enabled"] = 0
		}
	case nil:
		m["enabled"] = 1 // 缺省默认启用
	}
	b, err := json.Marshal(m)
	if err != nil {
		return nil, err
	}
	var src model.RssSource
	if err := json.Unmarshal(b, &src); err != nil {
		return nil, err
	}
	return &src, nil
}

func (a *API) handleDeleteRssSource(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	url := paramOf(a.params(c), "rssSourceUrl")
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.DeleteRssSource(ns, url); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleGetRssArticles(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	sourceURL := paramOf(params, "rssSourceUrl")
	// 无本地文章时尝试抓取
	articles, err := a.Storage.ListRssArticles(ns, sourceURL)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if len(articles) == 0 && sourceURL != "" {
		src, err := a.Storage.FindRssSource(ns, sourceURL)
		if err == nil && src != nil {
			if body, err := fetchRss(src.RssSourceURL); err == nil {
				if fetched, err := parseFeed(body, src.RssSourceURL); err == nil {
					_ = a.Storage.SaveRssArticles(ns, fetched)
					articles = make([]model.RssArticle, 0, len(fetched))
					for _, f := range fetched {
						articles = append(articles, *f)
					}
				}
			}
		}
	}
	// 输出（hasRead 字段）
	type articleOut struct {
		model.RssArticle
		HasRead bool `json:"hasRead"`
	}
	out := make([]articleOut, 0, len(articles))
	for _, a := range articles {
		out = append(out, articleOut{RssArticle: a, HasRead: a.Read == 1})
	}
	OK(c, out)
}

func (a *API) handleMarkRssArticleRead(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	url := paramOf(params, "url")
	read, _ := boolParam(params, "read")
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.MarkRssArticleRead(ns, url, read); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleGetRssArticle(c *gin.Context) {
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
	article, err := a.Storage.FindRssArticle(ns, url)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if article == nil {
		Fail(c, "文章不存在")
		return
	}
	OK(c, article)
}
