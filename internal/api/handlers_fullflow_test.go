package api

import (
	"net/url"
	"strconv"
	"strings"
	"testing"
)

// TestFullFlowJianLai 真实全流程：《剑来》搜索 → 详情 → 目录 → 正文 → 上书架 → 书架验证。
// 使用 xiu2 订阅合集里的真实书源（独步小说网/天天看小说/阅友小说/就爱文学/69书吧），
// 取第一个全流程可跑通的书源；外站不可达/规则不兼容时逐源回退，全部失败则 Skip。
func TestFullFlowJianLai(t *testing.T) {
	t.Setenv("READER_ALLOW_PRIVATE_NETWORK", "1")
	candidates := []string{"独步小说网", "天天看小说", "阅友小说", "就爱文学", "69书吧"}
	sources := loadRealShuyuan(t)

	for _, name := range candidates {
		var src map[string]any
		for _, s := range sources {
			if n, _ := s["bookSourceName"].(string); n == name {
				src = s
				break
			}
		}
		if src == nil {
			t.Logf("跳过（合集无此书源）: %s", name)
			continue
		}
		ok, detail := runFullFlowJianLai(t, src)
		if ok {
			t.Logf("书源 %s 全流程通过：%s", name, detail)
			return
		}
		t.Logf("书源 %s 未跑通：%s", name, detail)
	}
	t.Skip("所有候选书源全流程未跑通（外站不可达或规则不兼容）")
}

// runFullFlowJianLai 单书源全流程；返回 (成功, 详情)。
func runFullFlowJianLai(t *testing.T, src map[string]any) (bool, string) {
	origin, _ := src["bookSourceUrl"].(string)
	h := newTestAPI(t)
	saveOneSource(t, h, src)

	// 1) 搜索《剑来》
	w := perform(h, "GET", "/reader3/searchBook?key="+url.QueryEscape("剑来")+"&origin="+url.QueryEscape(origin), nil)
	rd := parseReturn(t, w)
	if !rd.IsSuccess {
		return false, "搜索失败: " + rd.ErrorMsg
	}
	books, _ := rd.Data.([]any)
	var bookURL string
	var bookName string
	for _, b := range books {
		m, _ := b.(map[string]any)
		u, _ := m["bookUrl"].(string)
		if strings.HasPrefix(u, "http") {
			bookURL = u
			bookName, _ = m["name"].(string)
			break
		}
	}
	if bookURL == "" {
		return false, "搜索无真实 http 书（仅提示项）"
	}
	t.Logf("  搜索到: %s -> %s", bookName, bookURL)

	// 2) 详情
	w = perform(h, "GET", "/reader3/getBookInfo?url="+url.QueryEscape(bookURL)+"&bookSource="+url.QueryEscape(origin), nil)
	dRd := parseReturn(t, w)
	if !dRd.IsSuccess {
		return false, "详情失败: " + dRd.ErrorMsg
	}

	// 3) 目录
	w = perform(h, "GET", "/reader3/getBookToc?tocUrl="+url.QueryEscape(bookURL)+"&bookSource="+url.QueryEscape(origin), nil)
	tocRd := parseReturn(t, w)
	if !tocRd.IsSuccess {
		return false, "目录失败: " + tocRd.ErrorMsg
	}
	toc, _ := tocRd.Data.([]any)
	if len(toc) == 0 {
		return false, "目录为空"
	}
	ch0, _ := toc[0].(map[string]any)
	chURL, _ := ch0["url"].(string)
	if chURL == "" {
		return false, "首章 URL 为空"
	}

	// 4) 正文
	w = perform(h, "GET", "/reader3/getBookContent?chapterUrl="+url.QueryEscape(chURL)+"&bookSource="+url.QueryEscape(origin), nil)
	cRd := parseReturn(t, w)
	if !cRd.IsSuccess {
		return false, "正文失败: " + cRd.ErrorMsg
	}
	cm, _ := cRd.Data.(map[string]any)
	content, _ := cm["content"].(string)
	if len([]rune(content)) < 100 {
		return false, "正文过短(" + strconv.Itoa(len([]rune(content))) + "字): " + chURL
	}

	// 5) 上书架
	w = perform(h, "POST", "/reader3/saveBook", map[string]any{
		"bookUrl": bookURL, "name": bookName, "origin": origin, "originName": src["bookSourceName"],
		"tocUrl": bookURL,
	})
	if !parseReturn(t, w).IsSuccess {
		return false, "上书架失败"
	}

	// 6) 书架验证
	w = perform(h, "GET", "/reader3/getBookshelf", nil)
	shRd := parseReturn(t, w)
	shelf, _ := shRd.Data.([]any)
	found := false
	for _, it := range shelf {
		if m, _ := it.(map[string]any); m["bookUrl"] == bookURL {
			found = true
			break
		}
	}
	if !found {
		return false, "书架未找到该书"
	}
	return true, bookName + " · 正文 " + strconv.Itoa(len([]rune(content))) + " 字"
}
