package api

import (
	"bytes"
	"encoding/json"
	"io"
	"strconv"
	"strings"

	"github.com/gin-gonic/gin"
)

// params 提取请求参数：query + form + JSON body 合并（JSON body 优先）。
func (a *API) params(c *gin.Context) map[string]any {
	out := make(map[string]any)
	// query
	for k, vs := range c.Request.URL.Query() {
		if len(vs) > 0 {
			out[k] = vs[len(vs)-1]
		}
	}
	// form（multipart urlencoded 已解析时）
	if err := c.Request.ParseMultipartForm(32 << 20); err == nil {
		for k, vs := range c.Request.PostForm {
			if len(vs) > 0 {
				out[k] = vs[len(vs)-1]
			}
		}
	}
	// JSON body（优先，覆盖 query/form 同名）
	if b, err := io.ReadAll(c.Request.Body); err == nil && len(b) > 0 {
		if obj, ok := parseJSONObject(b); ok {
			for k, v := range obj {
				out[k] = v
			}
		}
	}
	return out
}

// parseJSONObject 尝试将字节解析为 JSON 对象。
func parseJSONObject(b []byte) (map[string]any, bool) {
	b = bytes.TrimSpace(b)
	if len(b) == 0 || b[0] != '{' {
		return nil, false
	}
	var m map[string]any
	if err := json.Unmarshal(b, &m); err != nil {
		return nil, false
	}
	return m, true
}

// paramOf 从 query/body 提取字符串参数（兼容 legacy：query → body）。
func paramOf(params map[string]any, key string) string {
	v, ok := params[key]
	if !ok {
		return ""
	}
	if s, ok := v.(string); ok {
		return s
	}
	return ""
}

// boolParam 布尔参数：body 布尔值或 query "true"/"1"。
func boolParam(params map[string]any, key string) (bool, bool) {
	v, ok := params[key]
	if !ok {
		return false, false
	}
	switch t := v.(type) {
	case bool:
		return t, true
	case string:
		return t == "true" || t == "1", true
	}
	return false, false
}

// intParam 整数参数。
func intParam(params map[string]any, key string) (int64, bool) {
	v, ok := params[key]
	if !ok {
		return 0, false
	}
	switch t := v.(type) {
	case float64:
		return int64(t), true
	case int64:
		return t, true
	case int:
		return int64(t), true
	case string:
		if n, err := strconv.ParseInt(t, 10, 64); err == nil {
			return n, true
		}
	}
	return 0, false
}

// stringArrayParam 字符串数组参数（兼容 body 数组或 {"key":[...]}）。
func stringArrayParam(params map[string]any, key string) []string {
	if arr, ok := params[key].([]any); ok {
		out := make([]string, 0, len(arr))
		for _, v := range arr {
			if s, ok := v.(string); ok {
				out = append(out, s)
			}
		}
		return out
	}
	return nil
}

// secureKeyOf 提取 secureKey。
func secureKeyOf(params map[string]any) string {
	return paramOf(params, "secureKey")
}

// splitKeyVal 形如 "key=value" 的拆分。
func splitKeyVal(item string) (string, string) {
	item = strings.TrimSpace(item)
	idx := strings.IndexByte(item, '=')
	if idx < 0 {
		return "", item
	}
	return strings.TrimSpace(item[:idx]), item[idx+1:]
}
