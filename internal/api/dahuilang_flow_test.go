package api

import (
	"encoding/json"
	"net/url"
	"os"
	"strings"
	"testing"
)

func loadDahuilang(t *testing.T) map[string]any {
	t.Helper()
	b, err := os.ReadFile("testdata/dahuilang.json")
	if err != nil {
		t.Skipf("缺 fixture: %v", err)
	}
	var list []map[string]any
	if err := json.Unmarshal(b, &list); err != nil || len(list) == 0 {
		t.Fatalf("解析失败: %v", err)
	}
	return list[0]
}

// TestDiagDahuilang 大灰狼全流程诊断：搜索→详情→目录→正文。
func TestDiagDahuilang(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	src := loadDahuilang(t)
	// 大灰狼需配置 server（真实用户订阅后填写；fixture 未含）
	if v, _ := src["variable"].(string); v == "" {
		src["variable"] = "server=https://api.langge.cf;media=小说;tab=小说;source="
	}
	origin, _ := src["bookSourceUrl"].(string)
	h := newTestAPI(t)
	saveOneSource(t, h, src)

	w := perform(h, "GET", "/reader3/searchBook?key="+url.QueryEscape("剑来")+"&origin="+url.QueryEscape(origin), nil)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		t.Fatalf("搜索失败: %v", rd.ErrorMsg)
	}
	books, _ := rd.Data.([]any)
	t.Logf("搜索到 %d 条", len(books))
	var bookURL string
	var bookName string
	for _, b := range books {
		m, _ := b.(map[string]any)
		u, _ := m["bookUrl"].(string)
		if strings.HasPrefix(u, "data:") {
			bookURL = u
			bookName, _ = m["name"].(string)
			break
		}
	}
	if bookURL == "" {
		t.Fatal("无 data: 书（大灰狼协议 bookUrl 应为 data:;base64,...）")
	}
	t.Logf("book: %s", bookName)
	t.Logf("bookUrl 前缀: %s", bookURL[:60])

	// 详情
	w = perform(h, "GET", "/reader3/getBookInfo?url="+url.QueryEscape(bookURL)+"&bookSource="+url.QueryEscape(origin), nil)
	dRd := parseReturn(t, w)
	if !dRd.IsSuccess {
		t.Fatalf("详情失败: %v", dRd.ErrorMsg)
	}
	dm, _ := dRd.Data.(map[string]any)
	t.Logf("详情: name=%v author=%v intro=%v", dm["name"], dm["author"], truncate2(fmtv(dm["intro"]), 60))

	// 目录
	w = perform(h, "GET", "/reader3/getBookToc?tocUrl="+url.QueryEscape(bookURL)+"&bookSource="+url.QueryEscape(origin), nil)
	tocRd := parseReturn(t, w)
	if !tocRd.IsSuccess {
		t.Fatalf("目录失败: %v", tocRd.ErrorMsg)
	}
	toc, _ := tocRd.Data.([]any)
	t.Logf("目录章节数: %d", len(toc))
	if len(toc) == 0 {
		t.Fatal("目录为空")
	}
	for i := 0; i < 3 && i < len(toc); i++ {
		ch := toc[i].(map[string]any)
		nm, _ := ch["title"].(string)
		cu, _ := ch["url"].(string)
		t.Logf("  章[%d] %q -> %s", i, nm, cu[:min(60, len(cu))])
	}
	chURL, _ := toc[0].(map[string]any)["url"].(string)

	// 正文
	w = perform(h, "GET", "/reader3/getBookContent?chapterUrl="+url.QueryEscape(chURL)+"&bookSource="+url.QueryEscape(origin), nil)
	cRd := parseReturn(t, w)
	if !cRd.IsSuccess {
		t.Fatalf("正文失败: %v", cRd.ErrorMsg)
	}
	cm, _ := cRd.Data.(map[string]any)
	content, _ := cm["content"].(string)
	t.Logf("正文长度: %d", len([]rune(content)))
	t.Logf("正文前200: %s", truncate2(content, 200))
}

func fmtv(v any) string {
	if v == nil {
		return ""
	}
	return stringOr(v)
}

func truncate2(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n])
}

func stringOr(v any) string {
	if s, ok := v.(string); ok {
		return s
	}
	return ""
}
