package rule

import (
	"testing"
)

func TestJSONPathJSConcat(t *testing.T) {
	ctx := &Context{}
	item := `{"author":"風","name":"劍來","novel_id":"jianlai-fenghuoxizhuhou","topic_img":"x.jpg","description":"d"}`
	if v := Parse(item, "$.novel_id@js:'https://cn.ttkan.co/novel/chapters/'+result", ctx); len(v) == 0 || v[0] != "https://cn.ttkan.co/novel/chapters/jianlai-fenghuoxizhuhou" {
		t.Fatalf("novel_id@js 失败: %v", v)
	} else {
		t.Logf("ok: %v", v)
	}
}
