package solver

import "testing"

func TestHostOf(t *testing.T) {
	cases := map[string]string{
		"https://www.69shuba.com/book/1": "www.69shuba.com",
		"http://example.com:8080/path":   "example.com",
		"https://a.b.c/x?y=1":            "a.b.c",
		"example.com":                    "example.com",
		"https://[::1]:8080/":            "::1",
	}
	for in, want := range cases {
		if got := hostOf(in); got != want {
			t.Errorf("hostOf(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestNewAvailable(t *testing.T) {
	if s := New("http://127.0.0.1:8196", "", ""); !s.Available() {
		t.Error("URL 配置应可用")
	}
	if s := New("", "/opt/obscura/obscura", ""); !s.Available() {
		t.Error("BIN 配置应可用")
	}
	if s := New("", "", ""); s.Available() {
		t.Error("空配置应不可用")
	}
}
