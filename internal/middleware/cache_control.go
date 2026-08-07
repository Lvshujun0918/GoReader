// Package middleware HTTP 中间件。
package middleware

import (
	"path"
	"strings"

	"github.com/gin-gonic/gin"
)

// CacheControl 静态资源 Cache-Control（对齐 Rust GAP 60）。
//
//   - /static/**、/fonts/** → public, max-age=2592000, immutable（30 天）
//   - /assets/** → 30 天
//   - /、*.html、无扩展名 → no-cache
//   - 其他带扩展名 → 1 天
//   - /reader3、/opds*、/health → 不修改
//
// 仅 2xx 响应打头，已有 Cache-Control 不覆盖。
func CacheControl() gin.HandlerFunc {
	return func(c *gin.Context) {
		c.Next()
		if c.Writer.Status() < 200 || c.Writer.Status() >= 300 {
			return
		}
		if c.Writer.Header().Get("Cache-Control") != "" {
			return
		}
		p := c.Request.URL.Path
		switch {
		case strings.HasPrefix(p, "/reader3"), strings.HasPrefix(p, "/opds"), p == "/health":
			return
		case strings.HasPrefix(p, "/static/"), strings.HasPrefix(p, "/fonts/"):
			c.Header("Cache-Control", "public, max-age=2592000, immutable")
		case strings.HasPrefix(p, "/assets/"):
			c.Header("Cache-Control", "public, max-age=2592000, immutable")
		case p == "/" || strings.HasSuffix(p, ".html") || path.Ext(p) == "":
			c.Header("Cache-Control", "no-cache")
		default:
			c.Header("Cache-Control", "public, max-age=86400")
		}
	}
}
