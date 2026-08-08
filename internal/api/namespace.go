package api

import (
	"strings"
	"time"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/GoReader/internal/model"
)

// AccessTokenOf 从 query/header 提取 accessToken（query → accessToken 头 → Authorization: Bearer）。
func AccessTokenOf(c *gin.Context) string {
	if v := c.Query("accessToken"); v != "" {
		return v
	}
	if v := c.GetHeader("accessToken"); v != "" {
		return v
	}
	if v := c.GetHeader("Authorization"); v != "" {
		return strings.TrimPrefix(v, "Bearer ")
	}
	return ""
}

// ResolveNamespace 解析命名空间：
//   - 非 secure → "default"
//   - secure → 从 query/header 解析 accessToken（username:token）并校验 token，合法则返回用户名
//
// 主 token 或 users.token_map（多设备）中任一 token 均可通过；token 过期 → 需重新登录。
func (a *API) ResolveNamespace(c *gin.Context) (string, bool) {
	if !a.Config.Secure {
		return "default", true
	}
	accessToken := AccessTokenOf(c)
	if accessToken == "" {
		return "", false
	}
	idx := strings.IndexByte(accessToken, ':')
	if idx <= 0 || idx == len(accessToken)-1 {
		return "", false
	}
	username, tok := accessToken[:idx], accessToken[idx+1:]

	var user model.User
	if err := a.Storage.DB.Where("username = ?", username).First(&user).Error; err != nil {
		return "", false
	}
	tokenOK := (user.Token != "" && user.Token == tok) || tokenMapContains(user.TokenMap, tok)
	if !tokenOK {
		return "", false
	}
	// token 过期：基于 users.last_login_at + READER_TOKEN_TTL_DAYS（默认 30 天）；
	// 过期（或 legacy 用户 last_login_at=0 从未登录）→ NEED_LOGIN；ttl<=0 永不过期
	if ttl := a.Config.TokenTTLDays; ttl > 0 {
		if user.LastLoginAt == 0 || time.Now().UnixMilli()-user.LastLoginAt > ttl*86400*1000 {
			return "", false
		}
	}
	return user.Username, true
}

// tokenMapContains token_map 兼容旧对象形态 {token:ts} 与数组 [token,...]。
func tokenMapContains(tokenMap, tok string) bool {
	tm := strings.TrimSpace(tokenMap)
	if tm == "" {
		return false
	}
	// 数组形态 ["a","b"] 或 旧对象形态 {"token":ts}
	if strings.Contains(tm, "\"") {
		return strings.Contains(tm, "\""+tok+"\"")
	}
	// 纯逗号分隔
	for _, t := range strings.Split(tm, ",") {
		if t == tok {
			return true
		}
	}
	return false
}

// FormatUser 登录/注册返回结构（camelCase，兼容 legacy BaseController.formatUser）。
func FormatUser(u *model.User) map[string]any {
	return map[string]any{
		"username":         u.Username,
		"lastLoginAt":      u.LastLoginAt,
		"accessToken":      u.Username + ":" + u.Token,
		"enableWebdav":     u.EnableWebdav,
		"enableLocalStore": u.EnableLocalStore,
		"bookLimit":        u.BookLimit,
		"createdAt":        u.CreatedAt,
	}
}
