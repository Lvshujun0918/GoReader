package api

import (
	"net/http"
	"strings"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/reader-dev/internal/service/crawler"
)

// handleHealth GET /health。
func (a *API) handleHealth(c *gin.Context) {
	c.String(http.StatusOK, "ok!")
}

// handleAssetsProxy GET /assets/proxy：图片防盗链代理（?url=&referer=&fmt=webp&q=）。
func (a *API) handleAssetsProxy(c *gin.Context) {
	url := c.Query("url")
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	// 仅允许 http/https
	if !strings.HasPrefix(url, "http://") && !strings.HasPrefix(url, "https://") {
		Fail(c, "图片地址非法")
		return
	}
	// 经 crawler 客户端：SSRF 防护（内网/回环拒绝）+ 统一 UA + 响应限制
	client := crawler.New(nil, "")
	headers := map[string]string{}
	if referer := c.Query("referer"); referer != "" {
		headers["Referer"] = referer
	}
	body, err := client.FetchWithHeaders(url, headers)
	if err != nil {
		Fail(c, "图片加载失败："+err.Error())
		return
	}
	contentType := http.DetectContentType(body)
	c.Data(http.StatusOK, contentType, body)
}
