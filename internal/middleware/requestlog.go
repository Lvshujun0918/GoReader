package middleware

import (
	"log"
	"time"

	"github.com/gin-gonic/gin"
)

// RequestLog 请求日志中间件：method / path / status / 耗时 / 客户端 IP / UA。
// 默认开启（docker logs 可逐请求追踪）；READER_REQUEST_LOG=0 关闭（生产压测可关）。
func RequestLog(enabled bool) gin.HandlerFunc {
	return func(c *gin.Context) {
		start := time.Now()
		c.Next()
		if enabled {
			log.Printf("[http] %s %s → %d (%s) ip=%s ua=%s",
				c.Request.Method,
				c.Request.URL.RequestURI(),
				c.Writer.Status(),
				time.Since(start).Round(time.Millisecond),
				c.ClientIP(),
				shortUA(c.Request.UserAgent()),
			)
		}
	}
}

// shortUA 截断 UA 防止日志刷屏过长。
func shortUA(ua string) string {
	if len(ua) > 60 {
		return ua[:60] + "..."
	}
	return ua
}
