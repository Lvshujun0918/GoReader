package api

import (
	"strings"
	"testing"
)

// ---------- 书源订阅（远程订阅流程） ----------

// TestSourceSubCRUD 订阅 注册/列表/删除 全链路（真实订阅 URL 示例）。
func TestSourceSubCRUD(t *testing.T) {
	handler := newTestAPI(t)
	const url = "https://ghfast.top/https://raw.githubusercontent.com/XIU2/Yuedu/master/shuyuan"

	// 注册
	w := perform(handler, "POST", "/reader3/saveSourceSub", map[string]any{"url": url, "name": "XIU2合集"})
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("saveSourceSub 失败: %s", rd.ErrorMsg)
	}
	// 列表
	w = perform(handler, "GET", "/reader3/getSourceSubs", nil)
	rd = parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("getSourceSubs 失败: %s", rd.ErrorMsg)
	}
	list, _ := rd.Data.([]any)
	if len(list) != 1 {
		t.Fatalf("订阅列表应 1 个，实际 %d", len(list))
	}
	item, _ := list[0].(map[string]any)
	if item["url"] != url {
		t.Errorf("订阅 url 不符: %v", item["url"])
	}
	if item["name"] != "XIU2合集" {
		t.Errorf("订阅 name 不符: %v", item["name"])
	}
	// 重复注册（复合主键 upsert，仍 1 个）
	w = perform(handler, "POST", "/reader3/saveSourceSub", map[string]any{"url": url, "name": "XIU2合集"})
	if !parseReturn(t, w).IsSuccess {
		t.Fatal("重复注册应成功（upsert）")
	}
	// 删除
	w = perform(handler, "POST", "/reader3/deleteSourceSub", map[string]any{"url": url})
	if !parseReturn(t, w).IsSuccess {
		t.Fatal("deleteSourceSub 失败")
	}
	w = perform(handler, "GET", "/reader3/getSourceSubs", nil)
	rd = parseReturn(t, w)
	list, _ = rd.Data.([]any)
	if len(list) != 0 {
		t.Fatalf("删除后应 0 个，实际 %d", len(list))
	}
}

// TestDeleteSourceSubMissingURL 缺 url 应返回参数错误。
func TestDeleteSourceSubMissingURL(t *testing.T) {
	handler := newTestAPI(t)
	w := perform(handler, "POST", "/reader3/deleteSourceSub", map[string]any{})
	rd := parseReturn(t, w)
	if rd.IsSuccess {
		t.Fatal("缺 url 应失败")
	}
	if !strings.Contains(rd.ErrorMsg, "参数错误") {
		t.Errorf("错误信息不符: %s", rd.ErrorMsg)
	}
}

// ---------- RSS 订阅源 ----------

// TestRssSourceCRUD RSS 源 增/查/删（含 legado 布尔 enabled——验证宽松解析修复）。
func TestRssSourceCRUD(t *testing.T) {
	handler := newTestAPI(t)
	const feed = "https://rss.example.com/feed.xml"

	// 增（enabled 为布尔——legado 前端格式）
	w := perform(handler, "POST", "/reader3/saveRssSource", map[string]any{
		"rssSourceUrl": feed, "rssSourceName": "示例RSS", "rssSourceGroup": "新闻", "enabled": true,
	})
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("saveRssSource 失败: %s", rd.ErrorMsg)
	}
	// 查
	w = perform(handler, "GET", "/reader3/getRssSources", nil)
	rd = parseReturn(t, w)
	list, _ := rd.Data.([]any)
	if len(list) != 1 {
		t.Fatalf("RSS 列表应 1 个，实际 %d", len(list))
	}
	item, _ := list[0].(map[string]any)
	if item["rssSourceName"] != "示例RSS" {
		t.Errorf("名称不符: %v", item["rssSourceName"])
	}
	if item["enabled"] != float64(1) {
		t.Errorf("enabled 应归一化为 1: %v", item["enabled"])
	}
	// 删
	w = perform(handler, "POST", "/reader3/deleteRssSource", map[string]any{"rssSourceUrl": feed})
	if !parseReturn(t, w).IsSuccess {
		t.Fatal("deleteRssSource 失败")
	}
	w = perform(handler, "GET", "/reader3/getRssSources", nil)
	rd = parseReturn(t, w)
	list, _ = rd.Data.([]any)
	if len(list) != 0 {
		t.Fatalf("删除后应 0 个，实际 %d", len(list))
	}
}

// TestRssSourceBooleanEnabled 布尔 enabled 保存必须成功（回归：后端 int 列曾报参数错误）。
func TestRssSourceBooleanEnabled(t *testing.T) {
	handler := newTestAPI(t)
	w := perform(handler, "POST", "/reader3/saveRssSource", map[string]any{
		"rssSourceUrl": "https://bool.example.com/feed", "rssSourceName": "布尔开关", "enabled": false,
	})
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("布尔 enabled 保存失败: %s", rd.ErrorMsg)
	}
	w = perform(handler, "GET", "/reader3/getRssSources", nil)
	rd = parseReturn(t, w)
	list, _ := rd.Data.([]any)
	item, _ := list[0].(map[string]any)
	if item["enabled"] != float64(0) {
		t.Errorf("enabled=false 应归一化为 0: %v", item["enabled"])
	}
}

// ---------- 用户 ----------

// TestUserRegisterLogin 注册（未登录态自动注册）+ 登录 + 密码错误。
func TestUserRegisterLogin(t *testing.T) {
	handler := newTestAPI(t)
	// 注册（isLogin=false 且用户不存在 → 自动注册）
	w := perform(handler, "POST", "/reader3/login", map[string]any{
		"username": "alice", "password": "password123", "isLogin": false,
	})
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("注册失败: %s", rd.ErrorMsg)
	}
	// 登录
	w = perform(handler, "POST", "/reader3/login", map[string]any{
		"username": "alice", "password": "password123", "isLogin": true,
	})
	rd = parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("登录失败: %s", rd.ErrorMsg)
	}
	// 密码错误
	w = perform(handler, "POST", "/reader3/login", map[string]any{
		"username": "alice", "password": "wrong-pass", "isLogin": true,
	})
	rd = parseReturn(t, w)
	if rd.IsSuccess {
		t.Fatal("错误密码应登录失败")
	}
}

// TestUserRegisterDuplicate 已存在用户注册应失败。
func TestUserRegisterDuplicate(t *testing.T) {
	handler := newTestAPI(t)
	body := map[string]any{"username": "bob123", "password": "password123", "isLogin": false}
	if !parseReturn(t, perform(handler, "POST", "/reader3/login", body)).IsSuccess {
		t.Fatal("首次注册失败")
	}
	w := perform(handler, "POST", "/reader3/login", body)
	rd := parseReturn(t, w)
	if rd.IsSuccess {
		t.Fatal("重复注册应失败")
	}
	if !strings.Contains(rd.ErrorMsg, "用户名已被占用") {
		t.Errorf("错误信息不符: %s", rd.ErrorMsg)
	}
}

// ---------- 书架 ----------

// TestSaveBookAndGetBookshelf 书架 保存/读取/删除。
func TestSaveBookAndGetBookshelf(t *testing.T) {
	handler := newTestAPI(t)
	// 保存
	w := perform(handler, "POST", "/reader3/saveBook", map[string]any{
		"bookUrl": "https://example.com/book/1", "name": "测试书", "author": "作者",
	})
	if !parseReturn(t, w).IsSuccess {
		t.Fatalf("saveBook 失败: %s", parseReturn(t, w).ErrorMsg)
	}
	// 读取
	w = perform(handler, "GET", "/reader3/getBookshelf", nil)
	rd := parseReturn(t, w)
	list, _ := rd.Data.([]any)
	if len(list) != 1 {
		t.Fatalf("书架应 1 本，实际 %d", len(list))
	}
	book, _ := list[0].(map[string]any)
	if book["name"] != "测试书" {
		t.Errorf("书名不符: %v", book["name"])
	}
	// 删除
	w = perform(handler, "POST", "/reader3/deleteBook", map[string]any{"bookUrl": "https://example.com/book/1"})
	if !parseReturn(t, w).IsSuccess {
		t.Fatal("deleteBook 失败")
	}
	w = perform(handler, "GET", "/reader3/getBookshelf", nil)
	rd = parseReturn(t, w)
	list, _ = rd.Data.([]any)
	if len(list) != 0 {
		t.Fatalf("删除后应 0 本，实际 %d", len(list))
	}
}

// TestSaveBookMissingURL 缺 bookUrl 应失败。
func TestSaveBookMissingURL(t *testing.T) {
	handler := newTestAPI(t)
	w := perform(handler, "POST", "/reader3/saveBook", map[string]any{"name": "缺链接"})
	rd := parseReturn(t, w)
	if rd.IsSuccess {
		t.Fatal("缺 bookUrl 应失败")
	}
}
