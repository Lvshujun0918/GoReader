package api

import (
	"strings"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/reader-dev/internal/model"
	"github.com/Lvshujun0918/reader-dev/internal/parser/rule"
	"github.com/Lvshujun0918/reader-dev/internal/service/bookfetch"
)

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

// handleGetExploreUrls GET/POST /reader3/getExploreUrls：探索分类 URL 列表。
func (a *API) handleGetExploreUrls(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	origin := paramOf(params, "origin")
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
	// exploreUrl 多为多个 URL（分号/换行分隔）
	var urls []string
	for _, u := range strings.FieldsFunc(src.ExploreURL, func(r rune) bool {
		return r == ';' || r == '\n' || r == '|'
	}) {
		if u = strings.TrimSpace(u); u != "" {
			urls = append(urls, u)
		}
	}
	if len(urls) == 0 && src.ExploreURL != "" {
		urls = []string{src.ExploreURL}
	}
	OK(c, urls)
}

// handleExploreBook GET/POST /reader3/exploreBook：探索书单。
func (a *API) handleExploreBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	url := paramOf(params, "url")
	origin := paramOf(params, "origin")
	if url == "" {
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
	ruleStr := src.RuleExplore
	if ruleStr == "" {
		ruleStr = src.ExploreRule
	}
	client := crawlerClient(ns)
	body, err := client.FetchWithHeaders(url, nil)
	if err != nil {
		Fail(c, "探索失败："+err.Error())
		return
	}
	htmlStr := string(body)
	ctx := &rule.Context{BaseURL: url}
	var results []map[string]any
	items := rule.Parse(htmlStr, ruleStr, ctx)
	if len(items) == 0 {
		// 尝试成对规则
		for _, item := range bookfetch.SplitSemicolon(ruleStr) {
			key, val := splitKeyVal(item)
			if key == "bookList" || key == "books" {
				items = rule.Parse(htmlStr, val, ctx)
				break
			}
		}
	}
	for _, item := range items {
		book := map[string]any{}
		for _, sub := range bookfetch.SplitSemicolon(ruleStr) {
			key, val := splitKeyVal(sub)
			switch key {
			case "name":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					book["name"] = v[0]
				}
			case "author":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					book["author"] = v[0]
				}
			case "bookUrl":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					book["bookUrl"] = bookfetch.ResolveURL(v[0], url)
					book["tocUrl"] = book["bookUrl"]
				}
			case "coverUrl":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					book["coverUrl"] = bookfetch.ResolveURL(v[0], url)
				}
			case "intro":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					book["intro"] = v[0]
				}
			case "latestChapterTitle":
				if v := rule.Parse(item, val, ctx); len(v) > 0 {
					book["latestChapterTitle"] = v[0]
				}
			}
		}
		if book["name"] != nil {
			book["origin"] = src.BookSourceURL
			book["originName"] = src.BookSourceName
			results = append(results, book)
		}
	}
	OK(c, results)
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
