package api

import (
	"testing"
)

// TestShelfBookProgress 书架书保存 → 阅读进度更新 → 读取验证。
func TestShelfBookProgress(t *testing.T) {
	handler := newTestAPI(t)
	const url = "https://example.com/book/progress"

	// 保存书
	w := perform(handler, "POST", "/reader3/saveBook", map[string]any{
		"bookUrl": url, "name": "进度书", "author": "作者",
	})
	if !parseReturn(t, w).IsSuccess {
		t.Fatalf("saveBook 失败: %s", parseReturn(t, w).ErrorMsg)
	}
	// 更新进度
	w = perform(handler, "POST", "/reader3/saveBookProgress", map[string]any{
		"bookUrl": url, "durChapterTitle": "第5章", "durChapterIndex": 4, "durChapterPos": 120,
	})
	if !parseReturn(t, w).IsSuccess {
		t.Fatal("saveBookProgress 失败")
	}
	// 读取验证
	w = perform(handler, "GET", "/reader3/getShelfBook?url="+url, nil)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("getShelfBook 失败: %s", rd.ErrorMsg)
	}
	book, _ := rd.Data.(map[string]any)
	if book["durChapterTitle"] != "第5章" {
		t.Errorf("章节标题=%v", book["durChapterTitle"])
	}
	if book["durChapterIndex"] != float64(4) {
		t.Errorf("章节索引=%v", book["durChapterIndex"])
	}
	if book["durChapterPos"] != float64(120) {
		t.Errorf("阅读位置=%v", book["durChapterPos"])
	}
}

// TestSaveBookProgressEmptyURL 空 bookUrl 应静默成功（阅读页卸载时前端偶发空 url，不得报参数错误打扰）。
func TestSaveBookProgressEmptyURL(t *testing.T) {
	handler := newTestAPI(t)
	w := perform(handler, "POST", "/reader3/saveBookProgress", map[string]any{
		"bookUrl": "", "durChapterTitle": "第1章", "durChapterIndex": 0, "durChapterPos": 0,
	})
	if !parseReturn(t, w).IsSuccess {
		t.Fatalf("空 bookUrl 应静默成功，实际: %s", parseReturn(t, w).ErrorMsg)
	}
}

// TestGetShelfBookMissing 缺 url 应报错。
func TestGetShelfBookMissing(t *testing.T) {
	handler := newTestAPI(t)
	w := perform(handler, "GET", "/reader3/getShelfBook", nil)
	rd := parseReturn(t, w)
	if rd.IsSuccess {
		t.Fatal("缺 url 应失败")
	}
}

// TestBookmarkCRUD 书签 增/查/删。
func TestBookmarkCRUD(t *testing.T) {
	handler := newTestAPI(t)
	const url = "https://example.com/book/bm"

	// 增
	w := perform(handler, "POST", "/reader3/saveBookmark", map[string]any{
		"bookUrl": url, "title": "第3章", "paragraphIndex": 42, "chapterIndex": 2,
	})
	if !parseReturn(t, w).IsSuccess {
		t.Fatal("saveBookmark 失败")
	}
	// 查
	w = perform(handler, "GET", "/reader3/getBookmarks", nil)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("getBookmarks 失败: %s", rd.ErrorMsg)
	}
	list, _ := rd.Data.([]any)
	if len(list) != 1 {
		t.Fatalf("书签应 1 个，实际 %d", len(list))
	}
	bm, _ := list[0].(map[string]any)
	if bm["bookUrl"] != url || bm["title"] != "第3章" {
		t.Errorf("书签内容=%v", bm)
	}
	if bm["paragraphIndex"] != float64(42) {
		t.Errorf("段落索引=%v", bm["paragraphIndex"])
	}
	// 删
	w = perform(handler, "POST", "/reader3/deleteBookmark", map[string]any{"bookUrl": url, "title": "第3章"})
	if !parseReturn(t, w).IsSuccess {
		t.Fatal("deleteBookmark 失败")
	}
	w = perform(handler, "GET", "/reader3/getBookmarks", nil)
	rd = parseReturn(t, w)
	list, _ = rd.Data.([]any)
	if len(list) != 0 {
		t.Fatalf("删除后应 0 个，实际 %d", len(list))
	}
}

// TestBookGroupCRUD 分组 增/查/移书/删 全链路。
func TestBookGroupCRUD(t *testing.T) {
	handler := newTestAPI(t)
	const bookURL = "https://example.com/book/g"

	// 准备书架书
	if !parseReturn(t, perform(handler, "POST", "/reader3/saveBook", map[string]any{"bookUrl": bookURL, "name": "组内书"})).IsSuccess {
		t.Fatal("saveBook 失败")
	}
	// 建组
	w := perform(handler, "POST", "/reader3/saveBookGroup", map[string]any{"name": "玄幻"})
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("saveBookGroup 失败: %s", rd.ErrorMsg)
	}
	group, _ := rd.Data.(map[string]any)
	gid := int64(group["id"].(float64))
	if gid == 0 {
		t.Fatal("分组 id 应为正数")
	}
	// 查组
	w = perform(handler, "GET", "/reader3/getBookGroups", nil)
	rd = parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("getBookGroups 失败: %s", rd.ErrorMsg)
	}
	list, _ := rd.Data.([]any)
	if len(list) != 1 {
		t.Fatalf("分组应 1 个，实际 %d", len(list))
	}
	// 移书入组
	w = perform(handler, "POST", "/reader3/updateBookGroupId", map[string]any{"groupId": gid, "bookUrls": []string{bookURL}})
	if !parseReturn(t, w).IsSuccess {
		t.Fatal("updateBookGroupId 失败")
	}
	// 书架书分组应更新
	w = perform(handler, "GET", "/reader3/getShelfBook?url="+bookURL, nil)
	rd = parseReturn(t, w)
	book, _ := rd.Data.(map[string]any)
	if book["group"] != float64(gid) {
		t.Errorf("书分组=%v 期望 %d", book["group"], gid)
	}
	// 删组
	w = perform(handler, "POST", "/reader3/deleteBookGroup", map[string]any{"id": gid})
	if !parseReturn(t, w).IsSuccess {
		t.Fatal("deleteBookGroup 失败")
	}
}

// TestSaveBookmarksBatch 批量书签保存 + 读取（空 title 主键也能存则跳过）。
func TestSaveBookmarksBatch(t *testing.T) {
	handler := newTestAPI(t)
	list := []map[string]any{
		{"bookUrl": "https://example.com/book/1", "title": "第1章", "paragraphIndex": 10},
		{"bookUrl": "https://example.com/book/1", "title": "第2章", "paragraphIndex": 20},
	}
	w := perform(handler, "POST", "/reader3/saveBookmarks", list)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("saveBookmarks 失败: %s", rd.ErrorMsg)
	}
	w = perform(handler, "GET", "/reader3/getBookmarks", nil)
	rd = parseReturn(t, w)
	back, _ := rd.Data.([]any)
	if len(back) != 2 {
		t.Fatalf("批量书签应 2 个，实际 %d", len(back))
	}
}
