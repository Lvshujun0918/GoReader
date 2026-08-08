package rule

import (
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"strings"
	"testing"
)

// TestDahuilangCatalog 直接执行大灰狼 chapterList JS（java.ajax 走真实请求）。
func TestDahuilangCatalog(t *testing.T) {
	b, err := os.ReadFile("../../../internal/api/testdata/dahuilang.json")
	if err != nil {
		t.Skip("缺 fixture")
	}
	var list []map[string]any
	if err := json.Unmarshal(b, &list); err != nil {
		t.Fatal(err)
	}
	cl := list[0]["ruleToc"].(map[string]any)["chapterList"].(string)

	client := &http.Client{Timeout: 20e9}
	ctx := &Context{
		BaseURL: "data:;base64,x",
		Variables: map[string]string{
			"server": "https://api.langge.cf", "sourceVariable": "server=https://api.langge.cf",
		},
		Fetch: func(method, u, body string, headers map[string]string) (string, error) {
			t.Logf("ajax %s %s body=%s", method, u, truncate(body, 80))
			var r io.Reader
			if body != "" {
				r = strings.NewReader(body)
			}
			req, err := http.NewRequest(method, u, r)
			if err != nil {
				return "", err
			}
			if body != "" {
				req.Header.Set("Content-Type", "application/json")
			}
			resp, err := client.Do(req)
			if err != nil {
				return "", err
			}
			defer resp.Body.Close()
			b2, _ := io.ReadAll(resp.Body)
			return string(b2), nil
		},
	}
	// 构造 qingtian data URL（剑仙 book_id）
	q := map[string]any{"book_id": "NjUxMTk2MzU2OTkwMTI3NjE2Mw", "sources": "番茄", "tab": "小说", "url": ""}
	qb, _ := json.Marshal(q)
	dataURL := "data:;base64," + base64.StdEncoding.EncodeToString(qb) + `,{"type":"qingtian2"}`

	res := Parse(dataURL, cl, ctx)
	t.Logf("chapterList 结果数: %d", len(res))
	for i, r := range res {
		t.Logf("  [%d] %s", i, truncate(r, 150))
		if i >= 2 {
			break
		}
	}
}

// TestDahuilangContent 直接执行大灰狼 content JS。
func TestDahuilangContent(t *testing.T) {
	b, err := os.ReadFile("../../../internal/api/testdata/dahuilang.json")
	if err != nil {
		t.Skip("缺 fixture")
	}
	var list []map[string]any
	if err := json.Unmarshal(b, &list); err != nil {
		t.Fatal(err)
	}
	contentRule := list[0]["ruleContent"].(map[string]any)["content"].(string)

	client := &http.Client{Timeout: 20e9}
	ctx := &Context{
		BaseURL: "data:;base64,x",
		Variables: map[string]string{
			"server": "https://api.langge.cf", "sourceVariable": "server=https://api.langge.cf",
		},
		Fetch: func(method, u, body string, headers map[string]string) (string, error) {
			t.Logf("ajax %s %s body=%s", method, u, truncate(body, 100))
			var r io.Reader
			if body != "" {
				r = strings.NewReader(body)
			}
			req, err := http.NewRequest(method, u, r)
			if err != nil {
				return "", err
			}
			for k, v := range headers {
				req.Header.Set(k, v)
			}
			if body != "" && req.Header.Get("Content-Type") == "" {
				req.Header.Set("Content-Type", "application/json")
			}
			resp, err := client.Do(req)
			if err != nil {
				return "", err
			}
			defer resp.Body.Close()
			b2, _ := io.ReadAll(resp.Body)
			return string(b2), nil
		},
	}
	q := map[string]any{"book_id": "NjUxMTk2MzU2OTkwMTI3NjE2Mw", "item_id": "6511978580325433864", "title": "楔子", "sources": "番茄", "tab": "小说", "url": ""}
	qb, _ := json.Marshal(q)
	dataURL := "data:;base64," + base64.StdEncoding.EncodeToString(qb) + `,{"type":"qingtian3"}`

	res := Parse(dataURL, contentRule, ctx)
	t.Logf("content 结果数: %d", len(res))
	for i, r := range res {
		t.Logf("  [%d] len=%d head=%s", i, len(r), truncate(r, 120))
	}
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n]
}
