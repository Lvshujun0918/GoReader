package api

import (
	"io"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"time"

	"github.com/gin-gonic/gin"
)

// maxProxyBytes 图片代理响应上限。
const maxProxyBytes = 32 << 20

// handleHealth GET /health。
func (a *API) handleHealth(c *gin.Context) {
	c.String(http.StatusOK, "ok!")
}

// handleAssetsProxy GET /assets/proxy：图片防盗链代理（?url=&referer=&fmt=webp&q=）。
func (a *API) handleAssetsProxy(c *gin.Context) {
	raw := c.Query("url")
	if raw == "" {
		Fail(c, "参数错误")
		return
	}
	u, err := url.Parse(raw)
	if err != nil || (u.Scheme != "http" && u.Scheme != "https") {
		Fail(c, "图片地址非法")
		return
	}
	// 本地图片代理不经过书源抓取：仅基础 SSRF 防护（回环/内网拒绝）
	if !ssrfHostAllowed(u.Hostname()) {
		Fail(c, "图片地址非法")
		return
	}
	req, err := http.NewRequest(http.MethodGet, raw, nil)
	if err != nil {
		Fail(c, "图片加载失败")
		return
	}
	if referer := c.Query("referer"); referer != "" {
		req.Header.Set("Referer", referer)
	}
	req.Header.Set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/131.0.0.0")
	client := &http.Client{Timeout: 15 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		Fail(c, "图片加载失败："+err.Error())
		return
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 400 {
		Fail(c, "图片加载失败：HTTP "+strconv.Itoa(resp.StatusCode))
		return
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, maxProxyBytes))
	if err != nil {
		Fail(c, "图片加载失败："+err.Error())
		return
	}
	c.Data(http.StatusOK, http.DetectContentType(body), body)
}

// ssrfHostAllowed 基础 SSRF 防护：回环/内网/链路本地地址拒绝。
func ssrfHostAllowed(host string) bool {
	ip := net.ParseIP(host)
	if ip == nil {
		addrs, err := net.LookupIP(host)
		if err != nil || len(addrs) == 0 {
			return false
		}
		for _, a := range addrs {
			if !privateIP(a) {
				return true
			}
		}
		return false
	}
	return !privateIP(ip)
}

// privateIP 判断是否为回环/内网/链路本地/多播地址。
func privateIP(ip net.IP) bool {
	if ip.IsLoopback() || ip.IsPrivate() || ip.IsLinkLocalUnicast() || ip.IsLinkLocalMulticast() || ip.IsMulticast() || ip.IsUnspecified() {
		return true
	}
	return false
}
