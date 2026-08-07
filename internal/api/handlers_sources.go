package api

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/reader-dev/internal/model"
)

// handleGetBookSources GET/POST /reader3/getBookSources。
func (a *API) handleGetBookSources(c *gin.Context) {
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
	OK(c, sources)
}

// handleGetBookSource GET/POST /reader3/getBookSource。
func (a *API) handleGetBookSource(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	url := paramOf(a.params(c), "bookSourceUrl")
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	src, err := a.Storage.FindBookSource(ns, url)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if src == nil {
		Fail(c, "书源不存在")
		return
	}
	OK(c, src)
}

// handleSaveBookSource POST /reader3/saveBookSource。
func (a *API) handleSaveBookSource(c *gin.Context) {
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
	src, err := normalizeSourceMap(raw)
	if err != nil {
		Fail(c, "参数错误")
		return
	}
	if src.BookSourceURL == "" {
		Fail(c, "书源地址不能为空")
		return
	}
	// 书源数上限校验
	if limit := a.sourceLimit(ns); limit > 0 {
		if count, err := a.Storage.CountBookSources(ns); err == nil && count >= limit {
			Fail(c, "超过书源数上限")
			return
		}
	}
	if err := a.Storage.SaveBookSource(ns, src); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleSaveBookSources POST /reader3/saveBookSources（legacy 数组形态）。
func (a *API) handleSaveBookSources(c *gin.Context) {
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
	sources := make([]*model.BookSource, 0, len(raw))
	for _, m := range raw {
		src, err := normalizeSourceMap(m)
		if err != nil || src.BookSourceURL == "" {
			continue // 跳过无法归一化/缺地址的项（legado 合集常含空壳）
		}
		sources = append(sources, src)
	}
	if len(sources) == 0 {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.SaveBookSources(ns, sources); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, map[string]any{"count": len(sources)})
}

// handleDeleteBookSource POST /reader3/deleteBookSource。
func (a *API) handleDeleteBookSource(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	url := paramOf(a.params(c), "bookSourceUrl")
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.DeleteBookSource(ns, url); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleDeleteBookSources POST /reader3/deleteBookSources。
func (a *API) handleDeleteBookSources(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	var urls []string
	if arr, ok := params["bookSourceUrls"].([]any); ok {
		for _, v := range arr {
			if s, ok := v.(string); ok {
				urls = append(urls, s)
			}
		}
	}
	if len(urls) == 0 {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.DeleteBookSources(ns, urls); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleDeleteAllBookSources POST /reader3/deleteAllBookSources。
func (a *API) handleDeleteAllBookSources(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	if err := a.Storage.DeleteAllBookSources(ns); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleSaveFromRemoteSource POST /reader3/saveFromRemoteSource：远程书源订阅导入。
func (a *API) handleSaveFromRemoteSource(c *gin.Context) {
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
	// 远程源通常返回书源 JSON 数组；先按 JSON 尝试
	if sources, err := fetchRemoteSources(url); err == nil && len(sources) > 0 {
		if err := a.Storage.SaveBookSources(ns, sources); err != nil {
			Fail(c, "系统错误")
			return
		}
		OK(c, map[string]any{"saved": len(sources)})
		return
	}
	Fail(c, "远程源获取失败")
}

// fetchRemoteSources 抓取远程书源（JSON 数组形态，宽松归一化——legado 布尔 enabled 兼容）。
func fetchRemoteSources(url string) ([]*model.BookSource, error) {
	client := &http.Client{Timeout: 15 * time.Second}
	resp, err := client.Get(url)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 400 {
		return nil, fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, 8<<20))
	if err != nil {
		return nil, err
	}
	var raw []map[string]any
	if err := json.Unmarshal(body, &raw); err != nil {
		return nil, err
	}
	out := make([]*model.BookSource, 0, len(raw))
	for _, m := range raw {
		if src, err := normalizeSourceMap(m); err == nil && src.BookSourceURL != "" {
			out = append(out, src)
		}
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("未识别到书源")
	}
	return out, nil
}

// normalizeSourceMap 将宽松 JSON 书源对象归一化为 model.BookSource：
//   - legado 布尔字段（enabled/enabledExplore/enabledCookieJar）→ 0/1（后端 int 列）
//   - 对象型字段（header 等）→ JSON 字符串（兼容嵌套对象）
//
// 未知字段由 encoding/json 忽略；返回 err 表示字段类型无法归一化。
func normalizeSourceMap(m map[string]any) (*model.BookSource, error) {
	for _, k := range []string{"enabled", "enabledExplore", "enabledCookieJar"} {
		if v, ok := m[k].(bool); ok {
			if v {
				m[k] = 1
			} else {
				m[k] = 0
			}
		}
	}
	for _, k := range []string{"header"} {
		if v, ok := m[k].(map[string]any); ok {
			b, err := json.Marshal(v)
			if err != nil {
				return nil, err
			}
			m[k] = string(b)
		}
	}
	b, err := json.Marshal(m)
	if err != nil {
		return nil, err
	}
	var src model.BookSource
	if err := json.Unmarshal(b, &src); err != nil {
		return nil, err
	}
	return &src, nil
}

// handleGetAvailableBookSource GET/POST /reader3/getAvailableBookSource。
func (a *API) handleGetAvailableBookSource(c *gin.Context) {
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
	var available []model.BookSource
	for _, s := range sources {
		if s.Enabled == 1 {
			available = append(available, s)
		}
	}
	OK(c, available)
}

// handleGetInvalidBookSources GET/POST /reader3/getInvalidBookSources。
func (a *API) handleGetInvalidBookSources(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	// 简化：无健康检测后端时返回空（完整实现走 health 服务）
	_ = ns
	OK(c, []any{})
}

// handleDisableInvalidBookSources POST /reader3/disableInvalidBookSources。
func (a *API) handleDisableInvalidBookSources(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	var urls []string
	if arr, ok := params["bookSourceUrls"].([]any); ok {
		for _, v := range arr {
			if s, ok := v.(string); ok {
				urls = append(urls, s)
			}
		}
	}
	if len(urls) == 0 {
		OK(c, nil)
		return
	}
	// 禁用指定书源
	err := a.Storage.DB.Model(&model.BookSource{}).
		Where("user_namespace = ? AND book_source_url IN ?", ns, urls).
		Update("enabled", 0).Error
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleSetAsDefaultBookSources POST /reader3/setAsDefaultBookSources。
func (a *API) handleSetAsDefaultBookSources(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	var urls []string
	if arr, ok := params["bookSourceUrls"].([]any); ok {
		for _, v := range arr {
			if s, ok := v.(string); ok {
				urls = append(urls, s)
			}
		}
	}
	if len(urls) == 0 {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.SetAsDefaultBookSources(ns, urls); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleDeleteUserBookSource POST /reader3/deleteUserBookSource。
func (a *API) handleDeleteUserBookSource(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	url := paramOf(a.params(c), "bookSourceUrl")
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.DeleteBookSource(ns, url); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleExportBookSources GET /reader3/exportBookSources：导出书源 JSON。
func (a *API) handleExportBookSources(c *gin.Context) {
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
	OK(c, sources)
}

// sourceLimit 用户书源上限（secure 模式）。
func (a *API) sourceLimit(ns string) int64 {
	if !a.Config.Secure {
		return 0
	}
	u, err := a.Storage.FindUser(ns)
	if err != nil || u == nil {
		return 0
	}
	return u.BookSourceLimit
}
