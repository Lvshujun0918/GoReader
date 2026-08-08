package api

import (
	"crypto/rand"
	"encoding/hex"
	"regexp"
	"strings"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/GoReader/internal/model"
	"github.com/Lvshujun0918/GoReader/internal/storage"
	"github.com/Lvshujun0918/GoReader/internal/util/ct"
	"github.com/Lvshujun0918/GoReader/internal/util/loginlimit"
	"github.com/Lvshujun0918/GoReader/internal/util/password"
)

var usernameRe = regexp.MustCompile(`^[a-zA-Z0-9]+$`)

// handleLogin POST /reader3/login：登录或自动注册。
func (a *API) handleLogin(c *gin.Context) {
	params := a.params(c)
	username := paramOf(params, "username")
	passwordStr := paramOf(params, "password")
	isLogin, _ := boolParam(params, "isLogin")

	if username == "" {
		Fail(c, "请输入用户名")
		return
	}
	if passwordStr == "" {
		Fail(c, "请输入密码")
		return
	}

	// 登录限流（用户名+客户端 IP）
	ip := ClientIP(c)
	if msg := loginlimit.CheckAllowed(username, ip); msg != nil {
		Fail(c, msg.Error())
		return
	}

	user, err := a.Storage.FindUser(username)
	if err != nil {
		Fail(c, "系统错误")
		return
	}

	if user == nil {
		// 用户不存在
		if isLogin {
			loginlimit.RecordFailure(username, ip)
			Fail(c, "用户不存在")
			return
		}
		a.register(c, username, passwordStr, paramOf(params, "code"))
		return
	}

	// 用户已存在
	if !isLogin {
		Fail(c, "用户名已被占用")
		return
	}
	// 统一密码校验：argon2id 优先，legacy 双 MD5 兼容；MD5 通过时自动升级
	if !a.verifyPassword(user, passwordStr) {
		loginlimit.RecordFailure(username, ip)
		Fail(c, "密码错误")
		return
	}
	loginlimit.Reset(username, ip)

	now := storage.NowMillis()
	token := randomToken()
	if err := a.Storage.AddUserToken(username, token, now); err != nil {
		Fail(c, "系统错误")
		return
	}
	user.Token = token
	user.LastLoginAt = now
	OK(c, FormatUser(user))
}

// register 自动注册（校验顺序与错误消息兼容 legacy）。
func (a *API) register(c *gin.Context, username, passwordStr, code string) {
	cfg := a.Config
	if len(username) < 5 {
		Fail(c, "用户名不能低于5位")
		return
	}
	if int64(len(passwordStr)) < cfg.MinUserPasswordLength {
		Fail(c, "密码不能低于"+itoa(cfg.MinUserPasswordLength)+"位")
		return
	}
	if username == "default" {
		Fail(c, "用户名不能为非法字符")
		return
	}
	if !usernameRe.MatchString(username) {
		Fail(c, "用户名只能由字母和数字组成")
		return
	}
	// 邀请码校验（配置了才要求）
	if cfg.InviteCode != "" {
		if code == "" {
			Fail(c, "请输入邀请码")
			return
		}
		if code != cfg.InviteCode {
			Fail(c, "邀请码错误")
			return
		}
	}
	// 用户数上限
	count, err := a.Storage.CountUsers()
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	userLimit := cfg.UserLimit
	if userLimit < 1 {
		userLimit = 1
	}
	if count >= userLimit {
		Fail(c, "超过用户数上限")
		return
	}

	now := storage.NowMillis()
	user := &model.User{
		Username:         username,
		Password:         password.HashPassword(passwordStr),
		Salt:             randomSalt(),
		Token:            randomToken(),
		EnableWebdav:     cfg.DefaultUserEnableWebdav,
		EnableLocalStore: cfg.DefaultUserEnableLocalStore,
		BookLimit:        cfg.DefaultUserBookLimit,
		LastLoginAt:      now,
		CreatedAt:        now,
		UserNamespace:    username,
	}
	if err := a.Storage.InsertUser(user); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, FormatUser(user))
}

// verifyPassword 统一密码校验 + 自动升级。
func (a *API) verifyPassword(user *model.User, passwordStr string) bool {
	ok, needUpgrade := password.CheckPassword(user, passwordStr)
	if ok && needUpgrade {
		phc := password.HashPassword(passwordStr)
		if err := a.Storage.UpgradeUserPasswordHash(user.Username, phc); err != nil {
			// 升级失败仅告警、不影响登录
		}
	}
	return ok
}

// handleLogout POST /reader3/logout：退出登录（清 token，token 立即失效）。
func (a *API) handleLogout(c *gin.Context) {
	if !a.Config.Secure {
		Fail(c, "不支持的操作")
		return
	}
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	token := AccessTokenOf(c)
	if idx := strings.IndexByte(token, ':'); idx >= 0 {
		token = token[idx+1:]
	}
	var err error
	if token == "" {
		err = a.Storage.LogoutUser(ns)
	} else {
		err = a.Storage.RemoveUserToken(ns, token)
	}
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// handleGetUsers GET/POST /reader3/getUsers：用户列表（secure + secureKey 管理校验）。
func (a *API) handleGetUsers(c *gin.Context) {
	if _, ok := a.ResolveNamespace(c); !ok {
		NeedLogin(c)
		return
	}
	if !a.checkManagerAuth(c) {
		return
	}
	users, err := a.Storage.ListUsers()
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	arr := make([]map[string]any, 0, len(users))
	for i := range users {
		arr = append(arr, userAdminJSON(&users[i]))
	}
	OK(c, arr)
}

// handleUpdateUser POST /reader3/updateUser：更新用户权限/限额。
func (a *API) handleUpdateUser(c *gin.Context) {
	if _, ok := a.ResolveNamespace(c); !ok {
		NeedLogin(c)
		return
	}
	if !a.checkManagerAuth(c) {
		return
	}
	params := a.params(c)
	username := paramOf(params, "username")
	if username == "" {
		Fail(c, "参数错误")
		return
	}
	var eb, el *bool
	if v, ok := boolParam(params, "enableWebdav"); ok {
		eb = &v
	}
	if v, ok := boolParam(params, "enableLocalStore"); ok {
		el = &v
	}
	var il *int64
	if v, ok := intParam(params, "bookLimit"); ok {
		il = &v
	}
	n, err := a.Storage.UpdateUserPermissions(username, eb, el, il)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if n == 0 {
		Fail(c, "用户不存在")
		return
	}
	OK(c, nil)
}

// handleDeleteUser POST /reader3/deleteUser：删除用户（secureKey；不能删除自己）。
func (a *API) handleDeleteUser(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	if !a.checkManagerAuth(c) {
		return
	}
	username := paramOf(a.params(c), "username")
	if username == "" {
		Fail(c, "参数错误")
		return
	}
	if username == ns {
		Fail(c, "不能删除自己")
		return
	}
	n, err := a.Storage.DeleteUser(username)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if n == 0 {
		Fail(c, "用户不存在")
		return
	}
	OK(c, nil)
}

// handleDeleteUsers POST /reader3/deleteUsers：批量删除用户。返回剩余用户列表。
func (a *API) handleDeleteUsers(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	if !a.checkManagerAuth(c) {
		return
	}
	params := a.params(c)
	var usernames []string
	// 兼容 legacy 原始字符串数组 body 或 {"usernames":[...]}
	if arr, ok := params["usernames"].([]any); ok {
		for _, v := range arr {
			if s, ok := v.(string); ok {
				usernames = append(usernames, s)
			}
		}
	} else if arr, ok := params["usernames"].([]string); ok {
		usernames = arr
	}
	if len(usernames) == 0 {
		Fail(c, "参数错误")
		return
	}
	targets := usernames[:0]
	for _, u := range usernames {
		if u != ns {
			targets = append(targets, u)
		}
	}
	if _, err := a.Storage.DeleteUsers(targets); err != nil {
		Fail(c, "系统错误")
		return
	}
	users, err := a.Storage.ListUsers()
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	arr := make([]map[string]any, 0, len(users))
	for i := range users {
		arr = append(arr, userAdminJSON(&users[i]))
	}
	OK(c, arr)
}

// handleResetUserPassword POST /reader3/resetUserPassword：重置用户密码。
func (a *API) handleResetUserPassword(c *gin.Context) {
	if _, ok := a.ResolveNamespace(c); !ok {
		NeedLogin(c)
		return
	}
	if !a.checkManagerAuth(c) {
		return
	}
	params := a.params(c)
	username := paramOf(params, "username")
	pwd := paramOf(params, "password")
	if pwd == "" {
		pwd = paramOf(params, "newPassword")
	}
	if username == "" || pwd == "" {
		Fail(c, "参数错误")
		return
	}
	encrypted := password.HashPassword(pwd)
	n, err := a.Storage.ResetUserPassword(username, randomSalt(), encrypted)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if n == 0 {
		Fail(c, "用户不存在")
		return
	}
	OK(c, nil)
}

// handleClearInactiveUsers POST /reader3/clearInactiveUsers：清理不活跃用户。
func (a *API) handleClearInactiveUsers(c *gin.Context) {
	cfg := a.Config
	if !cfg.Secure || cfg.SecureKey == "" {
		Fail(c, "不支持的操作")
		return
	}
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	if !a.checkManagerAuth(c) {
		return
	}
	params := a.params(c)
	inactiveDay, _ := intParam(params, "inactiveDay")
	before := storage.NowMillis() - inactiveDay*86400*1000
	deleted, err := a.Storage.ClearInactiveUsers(before, &ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, map[string]any{"deleted": deleted, "count": len(deleted)})
}

// checkManagerAuth 管理校验（legacy checkManagerAuth）：secure 模式 + secureKey 匹配。
func (a *API) checkManagerAuth(c *gin.Context) bool {
	cfg := a.Config
	if !cfg.Secure || cfg.SecureKey == "" {
		Fail(c, "不支持的操作")
		return false
	}
	key := secureKeyOf(a.params(c))
	if !ct.Equal(key, cfg.SecureKey) {
		FailData(c, "请输入管理密码", "NEED_SECURE_KEY")
		return false
	}
	return true
}

// userAdminJSON 用户管理输出 JSON（不含密码/salt/token；camelCase 兼容 legacy）。
func userAdminJSON(u *model.User) map[string]any {
	return map[string]any{
		"username":         u.Username,
		"enableWebdav":     u.EnableWebdav,
		"enableLocalStore": u.EnableLocalStore,
		"bookLimit":        u.BookLimit,
		"lastLoginAt":      u.LastLoginAt,
		"createdAt":        u.CreatedAt,
	}
}

// randomToken uuid v4 随机（hex，32 字符）。
func randomToken() string {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	b[6] = (b[6] & 0x0f) | 0x40
	b[8] = (b[8] & 0x3f) | 0x80
	return hex.EncodeToString(b)
}

// randomSalt 8 位随机字母数字。
func randomSalt() string {
	const chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
	b := make([]byte, 8)
	_, _ = rand.Read(b)
	for i := range b {
		b[i] = chars[int(b[i])%len(chars)]
	}
	return string(b)
}

func itoa(v int64) string {
	if v == 0 {
		return "0"
	}
	neg := v < 0
	if neg {
		v = -v
	}
	var buf [20]byte
	i := len(buf)
	for v > 0 {
		i--
		buf[i] = byte('0' + v%10)
		v /= 10
	}
	if neg {
		i--
		buf[i] = '-'
	}
	return string(buf[i:])
}
