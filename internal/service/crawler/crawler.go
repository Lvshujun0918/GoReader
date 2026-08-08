// Package crawler HTTP 抓取客户端：书源 cookie 自动附加、SSRF 防护、响应限制。
package crawler

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/Lvshujun0918/GoReader/internal/model"
	"github.com/Lvshujun0918/GoReader/internal/service/solver"
	"github.com/Lvshujun0918/GoReader/internal/storage"
)

// MaxResponseBytes 响应体上限（防止内存爆炸）。
const MaxResponseBytes = 32 << 20 // 32MB

// ErrSSRF 内网/回环地址拒绝。
var ErrSSRF = errors.New("禁止访问内网地址")

// Client 抓取客户端。
type Client struct {
	HTTP    *http.Client
	Storage *storage.Storage
	NS      string
	Solver  *solver.Solver // 可选：obscura 质询求解器（配置了 READER_OBSCURA_URL/BIN 时启用）
	// AllowPrivate 允许访问内网/回环地址（READER_ALLOW_PRIVATE_NETWORK=1，测试与本地代理场景）
	AllowPrivate bool
}

// New 创建抓取客户端。solverOpt 可选传入质询求解器。
func New(st *storage.Storage, ns string, solverOpt ...*solver.Solver) *Client {
	transport := &http.Transport{
		DialContext: (&net.Dialer{
			Timeout:   10 * time.Second,
			KeepAlive: 30 * time.Second,
		}).DialContext,
		MaxIdleConns:          100,
		MaxIdleConnsPerHost:   10,
		IdleConnTimeout:       90 * time.Second,
		TLSHandshakeTimeout:   10 * time.Second,
		ResponseHeaderTimeout: 15 * time.Second,
	}
	c := &Client{
		HTTP:         &http.Client{Transport: transport, Timeout: 30 * time.Second},
		Storage:      st,
		NS:           ns,
		AllowPrivate: os.Getenv("READER_ALLOW_PRIVATE_NETWORK") == "1",
	}
	if len(solverOpt) > 0 && solverOpt[0] != nil {
		c.Solver = solverOpt[0]
	}
	return c
}

// Fetch 抓取 URL（书源 cookie 自动附加 + Cloudflare/Turnstile 质询求解）。返回响应体字节。
func (c *Client) Fetch(rawURL string, source *model.BookSource) ([]byte, error) {
	body, status, header, ua, err := c.doFetch(rawURL, source)
	if err != nil {
		return nil, err
	}
	// 质询命中且求解器可用 → 浏览器求解后重试
	if isCloudflareChallenge(status, body, header) && c.Solver != nil && c.Solver.Available() {
		return c.solveAndRetry(rawURL, source, ua, body)
	}
	if status >= 400 {
		return nil, fmt.Errorf("HTTP %d", status)
	}
	return body, nil
}

// doFetch 执行 GET 抓取（SSRF 防护 + cookie/header 附加）。返回 body/状态码/响应头/实际 UA。
func (c *Client) doFetch(rawURL string, source *model.BookSource) ([]byte, int, http.Header, string, error) {
	u, err := url.Parse(rawURL)
	if err != nil {
		return nil, 0, nil, "", err
	}
	if !ssrfAllowed(u.Hostname(), c.AllowPrivate) {
		return nil, 0, nil, "", fmt.Errorf("%w: %s", ErrSSRF, u.Hostname())
	}
	req, err := http.NewRequestWithContext(context.Background(), http.MethodGet, rawURL, nil)
	if err != nil {
		return nil, 0, nil, "", err
	}
	// UA：书源自定义 header 优先，否则默认 Chrome Windows（与求解器一致）
	ua := solver.DefaultUA
	if source != nil && source.BookSourceName != "" {
		applySourceHeaders(req, source.Header)
		if h := req.Header.Get("User-Agent"); h != "" {
			ua = h
		}
	}
	req.Header.Set("User-Agent", ua)
	// 书源 cookie 附加
	if source != nil {
		if cookie, _, _ := c.sourceCookie(source.BookSourceURL); cookie != "" {
			req.Header.Set("Cookie", cookie)
		}
	}
	resp, err := c.HTTP.Do(req)
	if err != nil {
		return nil, 0, nil, "", err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, MaxResponseBytes))
	if err != nil {
		return nil, 0, nil, "", err
	}
	return body, resp.StatusCode, resp.Header, ua, nil
}

// solveAndRetry 浏览器求解质询 → cookie 合并存库 → 用新 cookie 重试原请求。
// 求解超时或重试仍质询时兜底返回求解 HTML。
func (c *Client) solveAndRetry(rawURL string, source *model.BookSource, ua string, _ []byte) ([]byte, error) {
	existing := c.existingCookieString(source)
	res, err := c.Solver.Solve(context.Background(), rawURL, parseCookies(existing), solver.Options{UserAgent: ua})
	if err != nil {
		return nil, fmt.Errorf("质询求解失败: %w", err)
	}
	// 求解 cookie 按 name 合并存库（同源后续请求自动携带，避免重复求解；UA 一并记录）
	if c.Storage != nil && source != nil {
		merged := mergeCookieString(existing, res.Cookies)
		_ = c.Storage.SetBookSourceCookie(c.NS, source.BookSourceURL, merged, res.UserAgent)
	}
	// 求解成功 → 用新 cookie 重试原请求（POST 场景关键——浏览器只 GET 首页，重试才能拿到真实结果）
	if res.Solved {
		body, status, header, _, err := c.doFetch(rawURL, source)
		if err == nil && status < 400 && !isCloudflareChallenge(status, body, header) {
			return body, nil
		}
	}
	// 超时/重试仍质询 → 兜底返回求解 HTML
	return []byte(res.HTML), nil
}

// existingCookieString 读取书源已存 cookie 字符串。
func (c *Client) existingCookieString(source *model.BookSource) string {
	if source == nil {
		return ""
	}
	cookie, _, _ := c.sourceCookie(source.BookSourceURL)
	return cookie
}

// isCloudflareChallenge 检测 Cloudflare 质询响应（403/503 + 特征）。
func isCloudflareChallenge(status int, body []byte, header http.Header) bool {
	if status != 403 && status != 503 {
		return false
	}
	low := strings.ToLower(string(body))
	if strings.Contains(low, "cf-browser-gesture") ||
		strings.Contains(low, "challenge-platform") ||
		strings.Contains(low, "__cf_chl") ||
		strings.Contains(low, "cf-turnstile") ||
		strings.Contains(low, "just a moment") ||
		strings.Contains(low, "challenge-form") ||
		strings.Contains(low, "attention required") {
		return true
	}
	if header != nil && strings.Contains(strings.ToLower(header.Get("Server")), "cloudflare") {
		if strings.Contains(low, "captcha") || strings.Contains(low, "challenge") {
			return true
		}
	}
	return false
}

// parseCookies 解析 "a=b; c=d" cookie 字符串为 cookie 列表。
func parseCookies(s string) []solver.Cookie {
	s = strings.TrimSpace(s)
	if s == "" {
		return nil
	}
	var out []solver.Cookie
	for _, part := range strings.Split(s, ";") {
		part = strings.TrimSpace(part)
		idx := strings.IndexByte(part, '=')
		if idx <= 0 {
			continue
		}
		out = append(out, solver.Cookie{Name: strings.TrimSpace(part[:idx]), Value: strings.TrimSpace(part[idx+1:])})
	}
	return out
}

// mergeCookieString 求解结果 cookie 按 name 合并原 cookie 字符串（求解结果覆盖）。
func mergeCookieString(existing string, solved []solver.Cookie) string {
	m := map[string]string{}
	for _, c := range parseCookies(existing) {
		m[c.Name] = c.Value
	}
	for _, c := range solved {
		m[c.Name] = c.Value
	}
	parts := make([]string, 0, len(m))
	for name, val := range m {
		parts = append(parts, name+"="+val)
	}
	sort.Strings(parts)
	return strings.Join(parts, "; ")
}

// FetchWithHeaders 自定义 header 抓取。
func (c *Client) FetchWithHeaders(rawURL string, headers map[string]string) ([]byte, error) {
	u, err := url.Parse(rawURL)
	if err != nil {
		return nil, err
	}
	if !ssrfAllowed(u.Hostname(), c.AllowPrivate) {
		return nil, fmt.Errorf("%w: %s", ErrSSRF, u.Hostname())
	}
	req, err := http.NewRequestWithContext(context.Background(), http.MethodGet, rawURL, nil)
	if err != nil {
		return nil, err
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	if req.Header.Get("User-Agent") == "" {
		req.Header.Set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/131.0.0.0")
	}
	resp, err := c.HTTP.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 400 {
		return nil, fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	return io.ReadAll(io.LimitReader(resp.Body, MaxResponseBytes))
}

// FetchPost POST 表单请求（legado searchUrl 的 ,{'method':'POST','body':...} 描述）。
// formBody 为已替换占位符的表单串（keyword=%E6%96%97...&page=1），UTF-8 百分号编码。
func (c *Client) FetchPost(rawURL, formBody string, source *model.BookSource) ([]byte, error) {
	u, err := url.Parse(rawURL)
	if err != nil {
		return nil, err
	}
	if !ssrfAllowed(u.Hostname(), c.AllowPrivate) {
		return nil, fmt.Errorf("%w: %s", ErrSSRF, u.Hostname())
	}
	req, err := http.NewRequestWithContext(context.Background(), http.MethodPost, rawURL, strings.NewReader(formBody))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	// UA：书源自定义 header 优先，否则默认 Chrome Windows（与 GET 一致）
	ua := solver.DefaultUA
	if source != nil && source.BookSourceName != "" {
		applySourceHeaders(req, source.Header)
		if h := req.Header.Get("User-Agent"); h != "" {
			ua = h
		}
	}
	req.Header.Set("User-Agent", ua)
	if source != nil {
		if cookie, _, _ := c.sourceCookie(source.BookSourceURL); cookie != "" {
			req.Header.Set("Cookie", cookie)
		}
	}
	resp, err := c.HTTP.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, MaxResponseBytes))
	if err != nil {
		return nil, err
	}
	if resp.StatusCode >= 400 {
		return nil, fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	return body, nil
}

func (c *Client) sourceCookie(sourceURL string) (string, string, error) {
	if c.Storage == nil {
		return "", "", nil
	}
	ck, err := c.Storage.GetBookSourceCookie(c.NS, sourceURL)
	if err != nil || ck == nil {
		return "", "", err
	}
	return ck.Cookie, ck.UserAgent, nil
}

// applySourceHeaders 应用书源自定义 header（JSON 对象或换行 key:value）。
func applySourceHeaders(req *http.Request, raw string) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return
	}
	// JSON 形态 {"User-Agent":"...","Referer":"..."}
	if strings.HasPrefix(raw, "{") {
		var m map[string]string
		if err := jsonUnmarshal(raw, &m); err == nil {
			for k, v := range m {
				req.Header.Set(k, v)
			}
			return
		}
	}
	// 换行 key:value
	for _, line := range strings.Split(raw, "\n") {
		line = strings.TrimSpace(line)
		idx := strings.IndexByte(line, ':')
		if idx > 0 {
			req.Header.Set(strings.TrimSpace(line[:idx]), strings.TrimSpace(line[idx+1:]))
		}
	}
}

// ssrfAllowed SSRF 防护：回环/内网/链路本地/多播地址拒绝。
// allowPrivate=true（READER_ALLOW_PRIVATE_NETWORK）时放行（测试/本地代理）。
func ssrfAllowed(host string, allowPrivate bool) bool {
	if allowPrivate {
		return true
	}
	ip := net.ParseIP(host)
	if ip == nil {
		// 域名：解析后检查
		addrs, err := net.LookupIP(host)
		if err != nil || len(addrs) == 0 {
			return false
		}
		for _, a := range addrs {
			if !ipAllowed(a) {
				return false
			}
		}
		return true
	}
	return ipAllowed(ip)
}

func ipAllowed(ip net.IP) bool {
	if ip.IsLoopback() || ip.IsPrivate() || ip.IsLinkLocalUnicast() ||
		ip.IsLinkLocalMulticast() || ip.IsMulticast() || ip.IsUnspecified() {
		return false
	}
	return true
}

func jsonUnmarshal(s string, v any) error {
	return json.Unmarshal([]byte(s), v)
}

// ParseIntSafe 安全整数解析（供其他包复用）。
func ParseIntSafe(s string) int64 {
	n, _ := strconv.ParseInt(s, 10, 64)
	return n
}
