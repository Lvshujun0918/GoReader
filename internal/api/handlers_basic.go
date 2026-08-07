package api

import (
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/gin-gonic/gin"
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
	client := &http.Client{Timeout: 20 * time.Second}
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		Fail(c, "图片加载失败："+err.Error())
		return
	}
	if referer := c.Query("referer"); referer != "" {
		req.Header.Set("Referer", referer)
	}
	req.Header.Set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/131.0.0.0")
	resp, err := client.Do(req)
	if err != nil {
		Fail(c, "图片加载失败："+err.Error())
		return
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 400 {
		Fail(c, fmt.Sprintf("图片加载失败：HTTP %d", resp.StatusCode))
		return
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, 32<<20))
	if err != nil {
		Fail(c, "图片加载失败："+err.Error())
		return
	}
	contentType := resp.Header.Get("Content-Type")
	if contentType == "" {
		contentType = http.DetectContentType(body)
	}
	c.Data(http.StatusOK, contentType, body)
}
