package api

import (
	"encoding/json"
	"strings"
	"testing"
)

// TestNormalizeSourceMapLegado 验证 legado 布尔书源可正确归一化（GAP：boolean enabled → int）。
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
