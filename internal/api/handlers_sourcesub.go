package api

import (
	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/GoReader/internal/model"
)

// ---------------- 订阅（书源订阅，独立于探索） ----------------

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
