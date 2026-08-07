package middleware

import (
	"net/http"
	"strconv"

	"github.com/gin-gonic/gin"
)

// UploadLimit 上传大小上限（对齐 Rust GAP 62）：
// multipart 超限（http.MaxBytesReader 触发 413）→ 替换为明确 JSON 错误。
// 使用 gin 的 MaxMultipartMemory + Content-Length 预检 + http.MaxBytesReader 双兜底。
func UploadLimit(maxBytes int64) gin.HandlerFunc {
	return func(c *gin.Context) {
		// 预检：Content-Length 超限直接 413 JSON
		if cl := c.Request.ContentLength; cl > maxBytes {
			respondTooLarge(c, maxBytes)
			c.Abort()
			return
		}
		// 流式兜底：multipart 读取时超限
		if c.Request.Method == http.MethodPost {
			if c.Request.ContentLength > 0 {
				c.Request.Body = http.MaxBytesReader(c.Writer, c.Request.Body, maxBytes)
			}
		}
		c.Next()
	}
}

func respondTooLarge(c *gin.Context, maxBytes int64) {
	maxMB := maxBytes / (1024 * 1024)
	c.JSON(http.StatusOK, gin.H{
		"isSuccess": false,
		"errorMsg":  "文件过大：超过上传大小上限（" + strconv.FormatInt(maxMB, 10) + " MB）",
		"data":      nil,
	})
}
