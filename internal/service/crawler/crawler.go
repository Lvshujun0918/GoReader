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
	"strconv"
	"strings"
	"time"

	"github.com/Lvshujun0918/reader-dev/internal/model"
	"github.com/Lvshujun0918/reader-dev/internal/storage"
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
}

// New 创建抓取客户端。
func New(st *storage.Storage, ns string) *Client {
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
	return &Client{
		HTTP:    &http.Client{Transport: transport, Timeout: 30 * time.Second},
		Storage: st,
		NS:      ns,
	}
}

// Fetch 抓取 URL（书源 cookie 自动附加）。返回响应体字节。
func (c *Client) Fetch(rawURL string, source *model.BookSource) ([]byte, error) {
	u, err := url.Parse(rawURL)
	if err != nil {
		return nil, err
	}
	if !ssrfAllowed(u.Hostname()) {
		return nil, fmt.Errorf("%w: %s", ErrSSRF, u.Hostname())
	}
	req, err := http.NewRequestWithContext(context.Background(), http.MethodGet, rawURL, nil)
	if err != nil {
		return nil, err
	}
	// UA + 书源 header
	ua := "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
	if source != nil && source.BookSourceName != "" {
		req.Header.Set("User-Agent", ua)
		applySourceHeaders(req, source.Header)
	} else {
		req.Header.Set("User-Agent", ua)
	}
	// 书源 cookie 附加
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
	if resp.StatusCode >= 400 {
		return nil, fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, MaxResponseBytes))
	if err != nil {
		return nil, err
	}
	return body, nil
}

// FetchWithHeaders 自定义 header 抓取。
func (c *Client) FetchWithHeaders(rawURL string, headers map[string]string) ([]byte, error) {
	u, err := url.Parse(rawURL)
	if err != nil {
		return nil, err
	}
	if !ssrfAllowed(u.Hostname()) {
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
func ssrfAllowed(host string) bool {
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
