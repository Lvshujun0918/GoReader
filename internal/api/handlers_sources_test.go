package api

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/reader-dev/internal/config"
	"github.com/Lvshujun0918/reader-dev/internal/middleware"
	"github.com/Lvshujun0918/reader-dev/internal/model"
	"github.com/Lvshujun0918/reader-dev/internal/storage"
)

// ---------- 测试辅助 ----------

// newTestAPI 构建 API + gin 路由（临时目录 sqlite，Secure=false → 默认命名空间）。
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

// loadRealShuyuan 加载真实 XIU2/Yuedu 书源合集 fixture。
// 获取方式：curl -o internal/api/testdata/xiu2_shuyuan.json \
//
//	"https://ghfast.top/https://raw.githubusercontent.com/XIU2/Yuedu/master/shuyuan"
func loadRealShuyuan(t *testing.T) []map[string]any {
	t.Helper()
	b, err := os.ReadFile("testdata/xiu2_shuyuan.json")
	if err != nil {
		t.Skipf("缺少真实 fixture（联网下载后重跑）: %v", err)
	}
	var list []map[string]any
	if err := json.Unmarshal(b, &list); err != nil {
		t.Fatalf("fixture 解析失败: %v", err)
	}
	if len(list) == 0 {
		t.Fatal("fixture 为空")
	}
	return list
}

// sourceStrField 取 BookSource 指定 json 字段字符串值（测试断言用）。
func sourceStrField(s *model.BookSource, jsonKey string) string {
	switch jsonKey {
	case "ruleSearch":
		return s.RuleSearch
	case "ruleBookInfo":
		return s.RuleBookInfo
	case "ruleToc":
		return s.RuleToc
	case "ruleContent":
		return s.RuleContent
	case "ruleExplore":
		return s.RuleExplore
	case "header":
		return s.Header
	case "searchUrl":
		return s.SearchURL
	case "exploreUrl":
		return s.ExploreURL
	}
	return ""
}

// ---------- normalizeSourceMap 单测 ----------

// TestNormalizeSourceMapLegado 验证 legado 布尔书源可正确归一化（boolean enabled → int）。
func TestNormalizeSourceMapLegado(t *testing.T) {
	raw := `{
		"bookSourceUrl": "https://example.com/book",
		"bookSourceName": "测试书源",
		"enabled": true,
		"enabledExplore": false,
		"enabledCookieJar": true,
		"header": {"User-Agent": "Mozilla/5.0"},
		"customOrder": 3,
		"ruleSearch": "@js:search()",
		"weight": 100
	}`
	var m map[string]any
	if err := json.Unmarshal([]byte(raw), &m); err != nil {
		t.Fatal(err)
	}
	src, err := normalizeSourceMap(m)
	if err != nil {
		t.Fatalf("normalizeSourceMap 失败: %v", err)
	}
	if src.BookSourceURL != "https://example.com/book" {
		t.Errorf("bookSourceUrl = %q", src.BookSourceURL)
	}
	if src.Enabled != 1 {
		t.Errorf("enabled 应归一化为 1，实际 %d", src.Enabled)
	}
	if src.EnabledExplore != 0 {
		t.Errorf("enabledExplore 应归一化为 0，实际 %d", src.EnabledExplore)
	}
	if src.EnabledCookieJar != 1 {
		t.Errorf("enabledCookieJar 应归一化为 1，实际 %d", src.EnabledCookieJar)
	}
	if !strings.Contains(src.Header, "Mozilla") {
		t.Errorf("header 对象应转为 JSON 字符串，实际 %q", src.Header)
	}
	if src.CustomOrder != 3 {
		t.Errorf("customOrder = %d", src.CustomOrder)
	}
	if src.RuleSearch != "@js:search()" {
		t.Errorf("ruleSearch = %q", src.RuleSearch)
	}
	if src.Weight != 100 {
		t.Errorf("weight = %d", src.Weight)
	}
}

// TestNormalizeSourceMapMissingURL 缺 bookSourceUrl 不应 panic，URL 为空由调用方过滤。
func TestNormalizeSourceMapMissingURL(t *testing.T) {
	var m map[string]any
	if err := json.Unmarshal([]byte(`{"bookSourceName":"无地址"}`), &m); err != nil {
		t.Fatal(err)
	}
	src, err := normalizeSourceMap(m)
	if err != nil {
		t.Fatalf("不应报错: %v", err)
	}
	if src.BookSourceURL != "" {
		t.Errorf("应保留空 URL")
	}
}

// TestNormalizeSourceMapNumericEnabled 数字 enabled 也应通过（legacy 数值形态）。
func TestNormalizeSourceMapNumericEnabled(t *testing.T) {
	var m map[string]any
	if err := json.Unmarshal([]byte(`{"bookSourceUrl":"u","enabled":0}`), &m); err != nil {
		t.Fatal(err)
	}
	src, err := normalizeSourceMap(m)
	if err != nil {
		t.Fatalf("不应报错: %v", err)
	}
	if src.Enabled != 0 {
		t.Errorf("enabled 应保持 0，实际 %d", src.Enabled)
	}
}

// TestNormalizeSourceMapObjectRules 对象规则字段（legado 新格式）应转 JSON 字符串。
func TestNormalizeSourceMapObjectRules(t *testing.T) {
	var m map[string]any
	raw := `{"bookSourceUrl":"u","enabled":true,
		"ruleSearch":{"bookList":"@css:.list a","name":"@text"},
		"ruleBookInfo":{"name":"@css:h1@text"},
		"header":{"User-Agent":"Mozilla/5.0"}}`
	if err := json.Unmarshal([]byte(raw), &m); err != nil {
		t.Fatal(err)
	}
	src, err := normalizeSourceMap(m)
	if err != nil {
		t.Fatalf("normalizeSourceMap 失败: %v", err)
	}
	if !strings.Contains(src.RuleSearch, "@css:.list a") {
		t.Errorf("ruleSearch 对象未转字符串: %q", src.RuleSearch)
	}
	if !strings.Contains(src.RuleBookInfo, "@css:h1") {
		t.Errorf("ruleBookInfo 对象未转字符串: %q", src.RuleBookInfo)
	}
	if !strings.Contains(src.Header, "Mozilla") {
		t.Errorf("header 对象未转字符串: %q", src.Header)
	}
}

// ---------- 真实 XIU2/Yuedu 书源合集（ghfast 加速 raw.githubusercontent） ----------

// TestNormalizeSourceMapRealXiu2 真实 26 个书源全部可归一化（布尔 enabled + 对象 rule 字段）。
func TestNormalizeSourceMapRealXiu2(t *testing.T) {
	list := loadRealShuyuan(t)
	for i, raw := range list {
		src, err := normalizeSourceMap(raw)
		if err != nil {
			t.Fatalf("第 %d 个书源 %v 归一化失败: %v", i, raw["bookSourceName"], err)
		}
		if src.BookSourceURL == "" {
			t.Errorf("第 %d 个书源缺 bookSourceUrl", i)
		}
		if src.BookSourceName == "" {
			t.Errorf("第 %d 个书源缺 bookSourceName", i)
		}
		// 对象规则字段应转为 JSON 字符串
		for _, k := range []string{"ruleSearch", "ruleBookInfo", "ruleToc", "ruleContent", "ruleExplore"} {
			if _, ok := raw[k].(map[string]any); ok {
				if got := sourceStrField(src, k); got == "" {
					t.Errorf("第 %d 个书源 %v：对象字段 %s 未转为字符串", i, raw["bookSourceName"], k)
				}
			}
		}
	}
}

// TestSaveBookSourcesRealXiu2 接口级：真实 26 书源批量保存（端到端验证订阅导入主路径）。
func TestSaveBookSourcesRealXiu2(t *testing.T) {
	list := loadRealShuyuan(t)
	handler := newTestAPI(t)
	w := perform(handler, "POST", "/reader3/saveBookSources", list)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("saveBookSources 失败: %s", rd.ErrorMsg)
	}
	if got := dataCount(t, rd); got != len(list) {
		t.Fatalf("保存数=%d 期望 %d", got, len(list))
	}
	// 存库后可读回
	w = perform(handler, "GET", "/reader3/getBookSources", nil)
	rd = parseReturn(t, w)
	back, _ := rd.Data.([]any)
	if len(back) != len(list) {
		t.Fatalf("回读 %d 个，期望 %d", len(back), len(list))
	}
}

// TestFetchRemoteSourcesRealData 远程拉取：httptest server 返回真实数据，fetchRemoteSources 解析全部。
func TestFetchRemoteSourcesRealData(t *testing.T) {
	list := loadRealShuyuan(t)
	b, err := json.Marshal(list)
	if err != nil {
		t.Fatal(err)
	}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write(b)
	}))
	defer srv.Close()
	sources, err := fetchRemoteSources(srv.URL)
	if err != nil {
		t.Fatalf("fetchRemoteSources 失败: %v", err)
	}
	if len(sources) != len(list) {
		t.Fatalf("解析 %d 个，期望 %d", len(sources), len(list))
	}
	if sources[0].Enabled != 1 {
		t.Errorf("首个书源 enabled 应为 1，实际 %d", sources[0].Enabled)
	}
}

// ---------- 书源 CRUD 接口级 ----------

// TestSaveBookSourceLegadoObject 单个书源含对象规则/布尔开关：保存成功且存库可读。
func TestSaveBookSourceLegadoObject(t *testing.T) {
	handler := newTestAPI(t)
	body := map[string]any{
		"bookSourceUrl":  "https://example.com",
		"bookSourceName": "对象规则源",
		"enabled":        true,
		"enabledExplore": false,
		"header":         map[string]any{"User-Agent": "Mozilla/5.0"},
		"ruleSearch":     map[string]any{"bookList": "@css:.list a", "name": "@text"},
	}
	w := perform(handler, "POST", "/reader3/saveBookSource", body)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("saveBookSource 失败: %s", rd.ErrorMsg)
	}
	w = perform(handler, "GET", "/reader3/getBookSource?bookSourceUrl=https://example.com", nil)
	rd = parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("getBookSource 失败: %s", rd.ErrorMsg)
	}
	src, _ := rd.Data.(map[string]any)
	if src["bookSourceName"] != "对象规则源" {
		t.Errorf("名称不符: %v", src["bookSourceName"])
	}
	if src["ruleSearch"] == "" {
		t.Error("ruleSearch 对象未存为字符串")
	}
	if src["enabled"] != float64(1) {
		t.Errorf("enabled 应归一化为 1: %v", src["enabled"])
	}
}

// TestBookSourceCRUD 书源 增/查/删 全链路。
func TestBookSourceCRUD(t *testing.T) {
	handler := newTestAPI(t)
	// 增
	w := perform(handler, "POST", "/reader3/saveBookSource", map[string]any{
		"bookSourceUrl": "https://crud.com", "bookSourceName": "CRUD源", "enabled": true,
	})
	if !parseReturn(t, w).IsSuccess {
		t.Fatal("保存书源失败")
	}
	// 查
	w = perform(handler, "GET", "/reader3/getBookSources", nil)
	rd := parseReturn(t, w)
	list, _ := rd.Data.([]any)
	if len(list) != 1 {
		t.Fatalf("列表应有 1 个，实际 %d", len(list))
	}
	// 删
	w = perform(handler, "POST", "/reader3/deleteBookSource", map[string]any{"bookSourceUrl": "https://crud.com"})
	if !parseReturn(t, w).IsSuccess {
		t.Fatal("删除书源失败")
	}
	w = perform(handler, "GET", "/reader3/getBookSources", nil)
	rd = parseReturn(t, w)
	list, _ = rd.Data.([]any)
	if len(list) != 0 {
		t.Fatalf("删除后应有 0 个，实际 %d", len(list))
	}
}

// TestSaveBookSourceMissingURL 缺地址应返回"书源地址不能为空"。
func TestSaveBookSourceMissingURL(t *testing.T) {
	handler := newTestAPI(t)
	w := perform(handler, "POST", "/reader3/saveBookSource", map[string]any{"bookSourceName": "无地址"})
	rd := parseReturn(t, w)
	if rd.IsSuccess {
		t.Fatal("缺地址应失败")
	}
	if !strings.Contains(rd.ErrorMsg, "书源地址不能为空") {
		t.Errorf("错误信息不符: %s", rd.ErrorMsg)
	}
}
