package api

import (
	"strings"
	"testing"
)

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
