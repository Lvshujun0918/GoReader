package crawler

import (
	"net/http"
	"strings"
	"testing"

	"github.com/Lvshujun0918/reader-dev/internal/service/solver"
)

func TestIsCloudflareChallenge(t *testing.T) {
	if !isCloudflareChallenge(403, []byte("<html>Just a moment...</html>"), nil) {
		t.Error("403 + Just a moment 应判定为质询")
	}
	if !isCloudflareChallenge(503, []byte(`<script src="challenge-platform"></script>`), nil) {
		t.Error("503 + challenge-platform 应判定为质询")
	}
	if !isCloudflareChallenge(403, []byte(`<input name="cf-turnstile-response">`), nil) {
		t.Error("403 + cf-turnstile 应判定为质询")
	}
	if isCloudflareChallenge(200, []byte("ok"), nil) {
		t.Error("200 不应判定为质询")
	}
	if isCloudflareChallenge(403, []byte("Forbidden"), nil) {
		t.Error("403 无特征不应判定为质询")
	}
	h := http.Header{}
	h.Set("Server", "cloudflare")
	if !isCloudflareChallenge(403, []byte("captcha required"), h) {
		t.Error("cloudflare server + captcha 应判定为质询")
	}
}

func TestParseCookies(t *testing.T) {
	cks := parseCookies("a=1; b=2;  c = 3 ")
	if len(cks) != 3 {
		t.Fatalf("期望 3 个 cookie，实际 %d", len(cks))
	}
	if cks[0].Name != "a" || cks[0].Value != "1" {
		t.Errorf("cookie a 解析错误: %+v", cks[0])
	}
	if cks[2].Name != "c" || cks[2].Value != "3" {
		t.Errorf("cookie c 解析错误: %+v", cks[2])
	}
	if parseCookies("") != nil {
		t.Error("空字符串应返回 nil")
	}
}

func TestMergeCookieString(t *testing.T) {
	existing := "a=1; b=old"
	solved := []solver.Cookie{{Name: "b", Value: "new"}, {Name: "cf_clearance", Value: "xyz"}}
	got := mergeCookieString(existing, solved)
	if got == "" {
		t.Fatal("合并结果为空")
	}
	expect := map[string]string{"a": "1", "b": "new", "cf_clearance": "xyz"}
	for k, v := range expect {
		if !strings.Contains(got, k+"="+v) {
			t.Errorf("合并结果缺少 %s=%s: %s", k, v, got)
		}
	}
	if strings.Contains(got, "b=old") {
		t.Errorf("旧值 b=old 应被覆盖: %s", got)
	}
}
