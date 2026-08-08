package bookfetch

import (
	"testing"
)

func TestHtmlToText(t *testing.T) {
	cases := []struct {
		name, in, want string
	}{
		{"段落", `<div class="con"><p>　　玄天大陆，南州域</p><p>　　雾峰书院是最好的学院。</p></div>`,
			"玄天大陆，南州域\n雾峰书院是最好的学院。"},
		{"br 换行", `<div>第一行<br>第二行</div>`, "第一行\n第二行"},
		{"无标签", "剑来", "剑来"},
	}
	for _, c := range cases {
		if got := htmlToText(c.in); got != c.want {
			t.Errorf("%s: htmlToText(%q)=%q want %q", c.name, c.in, got, c.want)
		}
	}
}
