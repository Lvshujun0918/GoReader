package api

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/GoReader/internal/config"
	"github.com/Lvshujun0918/GoReader/internal/middleware"
	"github.com/Lvshujun0918/GoReader/internal/storage"
)

// newTestAPI 创建隔离的测试 API（临时 sqlite）。
func newTestAPI(t *testing.T) http.Handler {
	t.Helper()
	gin.SetMode(gin.TestMode)
	dir := t.TempDir()
	cfg := &config.Config{WorkDir: dir, Secure: false}
	st, err := storage.Init(cfg)
	if err != nil {
		t.Fatalf("storage.Init 失败: %v", err)
	}
	t.Cleanup(func() {
		if sqlDB, err := st.DB.DB(); err == nil {
			_ = sqlDB.Close()
		}
	})
	return New(st, cfg, middleware.NewRequestStats()).Engine()
}

// perform 执行 HTTP 请求（body 为对象时 JSON 序列化并设 Content-Type）。
func perform(r http.Handler, method, path string, body any) *httptest.ResponseRecorder {
	var reader io.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		reader = bytes.NewReader(b)
	}
	req := httptest.NewRequest(method, path, reader)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	w := httptest.NewRecorder()
	r.ServeHTTP(w, req)
	return w
}

// parseReturn 解析统一响应结构。
func parseReturn(t *testing.T, w *httptest.ResponseRecorder) ReturnData {
	t.Helper()
	var rd ReturnData
	if err := json.Unmarshal(w.Body.Bytes(), &rd); err != nil {
		t.Fatalf("响应解析失败: %v（body=%s）", err, w.Body.String())
	}
	return rd
}

// dataCount 取 data.count。
func dataCount(t *testing.T, rd ReturnData) int {
	t.Helper()
	m, ok := rd.Data.(map[string]any)
	if !ok {
		t.Fatalf("data 非对象: %v", rd.Data)
	}
	c, _ := m["count"].(float64)
	return int(c)
}
