// Package api HTTP API（/reader3/* 兼容 legacy）。
package api

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

// ReturnData 统一返回结构（兼容 legacy ReturnData：isSuccess/errorMsg/data——camelCase）。
type ReturnData struct {
	IsSuccess bool   `json:"isSuccess"`
	ErrorMsg  string `json:"errorMsg"`
	Data      any    `json:"data"`
}

// OK 成功返回。
func OK(c *gin.Context, data any) {
	c.JSON(http.StatusOK, ReturnData{IsSuccess: true, Data: data})
}

// Fail 失败返回。
func Fail(c *gin.Context, msg string) {
	c.JSON(http.StatusOK, ReturnData{IsSuccess: false, ErrorMsg: msg, Data: nil})
}

// FailData 失败返回（带 data）。
func FailData(c *gin.Context, msg string, data any) {
	c.JSON(http.StatusOK, ReturnData{IsSuccess: false, ErrorMsg: msg, Data: data})
}

// NeedLogin 未登录返回（兼容 legacy checkAuth 失败：errorMsg=请登录后使用，data=NEED_LOGIN）。
func NeedLogin(c *gin.Context) {
	c.JSON(http.StatusOK, ReturnData{IsSuccess: false, ErrorMsg: "请登录后使用", Data: "NEED_LOGIN"})
}

// NeedSecureKey 需要管理密码（data=NEED_SECURE_KEY）。
func NeedSecureKey(c *gin.Context) {
	c.JSON(http.StatusOK, ReturnData{IsSuccess: false, ErrorMsg: "需要管理密码", Data: "NEED_SECURE_KEY"})
}
