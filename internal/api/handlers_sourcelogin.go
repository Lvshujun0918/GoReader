package api

import (
	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/reader-dev/internal/service/crawler"
)

// crawlerClient 创建抓取客户端（带全局质询求解器）。
func (a *API) crawlerClient(ns string) *crawler.Client {
	return crawler.New(nil, ns, a.Solver)
}

// handleLoginBookSource GET/POST /reader3/loginBookSource：书源登录（HTTP 直连表单）。
func (a *API) handleLoginBookSource(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	sourceURL := paramOf(params, "bookSourceUrl")
	username := paramOf(params, "username")
	passwordStr := paramOf(params, "password")
	if sourceURL == "" {
		Fail(c, "参数错误")
		return
	}
	src, err := a.Storage.FindBookSource(ns, sourceURL)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if src == nil {
		Fail(c, "未找到书源")
		return
	}
	loginURL := src.LoginURL
	if loginURL == "" {
		Fail(c, "书源未配置登录地址")
		return
	}
	// HTTP 直连登录（GET/POST 表单）
	client := a.crawlerClient(ns)
	form := map[string]string{}
	if username != "" {
		form["username"] = username
	}
	if passwordStr != "" {
		form["password"] = passwordStr
	}
	// 尝试 POST 表单登录
	headers := map[string]string{"Content-Type": "application/x-www-form-urlencoded"}
	if _, err := client.FetchWithHeaders(loginURL, headers); err != nil {
		Fail(c, "登录失败："+err.Error())
		return
	}
	// 保存书源 cookie（占位：实际应从响应 Set-Cookie 提取）
	cookie := c.GetHeader("Set-Cookie")
	if cookie != "" {
		_ = a.Storage.SetBookSourceCookie(ns, sourceURL, cookie, "")
	}
	OK(c, map[string]any{"success": true})
}

// handleSetBookSourceCookie POST /reader3/setBookSourceCookie。
func (a *API) handleSetBookSourceCookie(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	sourceURL := paramOf(params, "bookSourceUrl")
	cookie := paramOf(params, "cookie")
	ua := paramOf(params, "userAgent")
	if sourceURL == "" || cookie == "" {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.SetBookSourceCookie(ns, sourceURL, cookie, ua); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleGetCaptcha POST /reader3/getCaptcha：获取验证码（占位）。
func (a *API) handleGetCaptcha(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "验证码功能实现中")
}

// handleSubmitCaptcha POST /reader3/submitCaptcha：提交验证码（占位）。
func (a *API) handleSubmitCaptcha(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "验证码功能实现中")
}
