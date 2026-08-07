// Package solver 通过 obscura CDP 浏览器求解 Cloudflare/Turnstile 质询。
//
// Go 原生实现（替代 Python camoufox_solver）——驱动项目已内置的 obscura
// 反检测浏览器（BoringSSL TLS 指纹模拟/反检测/追踪器拦截 + STEALTH_JS）：
//   - 直连既有 CDP 服务（READER_OBSCURA_URL，不接管进程）
//   - 或 spawn `obscura serve --port <随机> --stealth`（READER_OBSCURA_BIN）
//
// 求解流程：每请求新建页面 → 注入书源 cookie → 导航 → 质询等待循环
// （cf-turnstile-response 非空 / 质询特征消失 → 通过；Turnstile iframe →
// 点击勾选）→ 超时 → 提取最终 HTML + cookie + UA。
package solver

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/url"
	"os/exec"
	"sync"
	"time"

	"github.com/go-rod/rod"
	"github.com/go-rod/rod/lib/proto"
)

// DefaultUA 默认 Chrome Windows UA（69shuba 等站点有 UA 门禁）。
const DefaultUA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"

// DefaultMaxWait 质询默认最大等待时长。
const DefaultMaxWait = 60 * time.Second

// ErrUnavailable 浏览器后端不可用（未配置 URL 且未配置二进制）。
var ErrUnavailable = errors.New("浏览器后端不可用：需配置 READER_OBSCURA_URL 或 READER_OBSCURA_BIN")

// Cookie 浏览器 cookie（求解结果 / 注入用）。
type Cookie struct {
	Name   string
	Value  string
	Domain string
	Path   string
}

// Options 求解选项。
type Options struct {
	// UserAgent 覆盖浏览器 UA（空则用 DefaultUA）。
	UserAgent string
	// MaxWaitMs 质询最大等待毫秒（默认 60000）。
	MaxWaitMs int
}

// Result 求解结果。
type Result struct {
	HTML      string
	Cookies   []Cookie
	UserAgent string
	// Solved 质询是否在超时前通过（false=超时兜底返回）。
	Solved bool
}

// Solver obscura CDP 质询求解器（进程内单例，线程安全）。
type Solver struct {
	cdpURL  string // 直连既有 CDP 服务（不接管进程）
	bin     string // obscura 二进制路径（spawn 接管）
	proxy   string // HTTP/SOCKS5 代理（spawn 时传给 obscura）
	proc    *exec.Cmd
	browser *rod.Browser
	mu      sync.Mutex
}

// New 创建求解器。url/bin/proxy 对应 READER_OBSCURA_URL/BIN/PROXY。
func New(url, bin, proxy string) *Solver {
	return &Solver{cdpURL: url, bin: bin, proxy: proxy}
}

// Available 是否有可用后端配置。
func (s *Solver) Available() bool {
	return s != nil && (s.cdpURL != "" || s.bin != "")
}

// ensureBrowser 确保 obscura 可用：优先直连既有 CDP 服务，否则 spawn 二进制。
func (s *Solver) ensureBrowser(ctx context.Context) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.browser != nil {
		return nil
	}
	target := s.cdpURL
	if target == "" {
		if s.bin == "" {
			return ErrUnavailable
		}
		port, err := freePort()
		if err != nil {
			return fmt.Errorf("分配端口失败: %w", err)
		}
		args := []string{"serve", "--port", port, "--stealth"}
		if s.proxy != "" {
			args = append(args, "--proxy", s.proxy)
		}
		proc := exec.CommandContext(ctx, s.bin, args...)
		if err := proc.Start(); err != nil {
			return fmt.Errorf("启动 obscura 失败: %w", err)
		}
		if err := waitForPort(ctx, "127.0.0.1:"+port, 10*time.Second); err != nil {
			_ = proc.Process.Kill()
			return fmt.Errorf("等待 obscura 就绪失败: %w", err)
		}
		s.proc = proc
		target = "http://127.0.0.1:" + port
	}
	b := rod.New().ControlURL(target)
	if err := b.Connect(); err != nil {
		return fmt.Errorf("连接浏览器后端失败: %w", err)
	}
	s.browser = b
	return nil
}

// Solve 打开页面，等待质询通过，返回最终 HTML + cookie + UA。
// 超时或质询未通过时仍返回页面 HTML（Solved=false），由调用方决定兜底。
func (s *Solver) Solve(ctx context.Context, targetURL string, cookies []Cookie, opts Options) (*Result, error) {
	if err := s.ensureBrowser(ctx); err != nil {
		return nil, err
	}
	maxWait := time.Duration(opts.MaxWaitMs) * time.Millisecond
	if maxWait <= 0 {
		maxWait = DefaultMaxWait
	}
	ua := opts.UserAgent
	if ua == "" {
		ua = DefaultUA
	}
	page, err := s.browser.Page(proto.TargetCreateTarget{URL: "about:blank"})
	if err != nil {
		return nil, err
	}
	defer page.Close()

	// UA 覆盖（wire 与 JS 两侧一致——69shuba UA 门禁必需）
	if err := page.SetUserAgent(&proto.NetworkSetUserAgentOverride{UserAgent: ua}); err != nil {
		return nil, fmt.Errorf("设置 UA 失败: %w", err)
	}
	// 注入书源 cookie（导航前）
	if err := setCookies(page, targetURL, cookies); err != nil {
		return nil, fmt.Errorf("注入 cookie 失败: %w", err)
	}
	if err := page.Navigate(targetURL); err != nil {
		return nil, fmt.Errorf("导航失败: %w", err)
	}

	// 质询等待循环（每 500ms 求值，直到通过或超时）
	deadline := time.Now().Add(maxWait)
	solved := false
	for time.Now().Before(deadline) {
		if err := ctx.Err(); err != nil {
			return nil, ctx.Err()
		}
		ok, err := s.challengeSolved(page)
		if err != nil {
			// 求值失败（页面未就绪等）——继续等待
		} else if ok {
			solved = true
			break
		} else {
			s.tryClickTurnstile(page)
		}
		time.Sleep(500 * time.Millisecond)
	}

	html, err := page.HTML()
	if err != nil {
		return nil, fmt.Errorf("提取 HTML 失败: %w", err)
	}
	cks, err := page.Cookies(nil)
	if err != nil {
		return nil, fmt.Errorf("提取 cookie 失败: %w", err)
	}
	if ua == "" {
		if v, err := page.Eval(`navigator.userAgent`); err == nil {
			ua = v.Value.String()
		}
	}
	res := &Result{HTML: html, UserAgent: ua, Solved: solved}
	for _, c := range cks {
		res.Cookies = append(res.Cookies, Cookie{Name: c.Name, Value: c.Value, Domain: c.Domain, Path: c.Path})
	}
	return res, nil
}

// challengeSolved 判断质询是否已通过。
// 通过：cf-turnstile-response 非空（managed challenge 自动完成）/ 页面不再呈现质询特征。
func (s *Solver) challengeSolved(page *rod.Page) (bool, error) {
	// 1. Turnstile token 已生成 → 通过
	if v, err := page.Eval(`(()=>{try{const el=document.querySelector('input[name="cf-turnstile-response"],input[name^="cf-turnstile"]');return el?el.value:""}catch(e){return ""}})()`); err == nil {
		if val := v.Value.String(); val != "" && val != "undefined" && len(val) > 8 {
			return true, nil
		}
	}
	// 2. 仍在质询页（标题 / 挑战容器特征）→ 未通过
	if v, err := page.Eval(`(()=>{try{const t=document.title.toLowerCase();return /just a moment|attention required|cf-challenge|checking your browser/.test(t)||!!document.querySelector('#challenge-form,#challenge-running,.cf-chl,[id^="challenge-"]')}catch(e){return true}})()`); err == nil && v.Value.Bool() {
		return false, nil
	}
	// 3. 无质询特征 → 已通过
	return true, nil
}

// tryClickTurnstile 检测 Turnstile iframe 内 checkbox 并点击勾选。
func (s *Solver) tryClickTurnstile(page *rod.Page) {
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	p := page.Context(ctx)
	// 查找 Cloudflare Turnstile iframe
	iframe, err := p.Element(`iframe[src*="challenges.cloudflare.com"], iframe[src*="challenges.cloudflare"]`)
	if err != nil {
		return
	}
	frame, err := iframe.Frame()
	if err != nil {
		return
	}
	checkbox, err := frame.Element(`input[type="checkbox"], .cb-holder, #challenge-stage`)
	if err != nil {
		return
	}
	_ = checkbox.Click(proto.InputMouseButtonLeft, 1)
}

// Close 释放浏览器与子进程（应用退出时调用）。
func (s *Solver) Close() {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.browser != nil {
		_ = s.browser.Close()
		s.browser = nil
	}
	if s.proc != nil && s.proc.Process != nil {
		_ = s.proc.Process.Kill()
		s.proc = nil
	}
}

// setCookies 注入 cookie（按 host 归一到 Domain/Path）。
func setCookies(page *rod.Page, targetURL string, cookies []Cookie) error {
	if len(cookies) == 0 {
		return nil
	}
	host := hostOf(targetURL)
	params := make([]*proto.NetworkCookieParam, 0, len(cookies))
	for _, c := range cookies {
		p := &proto.NetworkCookieParam{
			Name:  c.Name,
			Value: c.Value,
			Path:  c.Path,
		}
		if p.Path == "" {
			p.Path = "/"
		}
		if c.Domain != "" {
			p.Domain = c.Domain
		} else {
			p.Domain = host
		}
		params = append(params, p)
	}
	return page.SetCookies(params)
}

// hostOf 提取 URL 的 host（IPv6 归一为无括号形式）。
func hostOf(rawURL string) string {
	u, err := url.Parse(rawURL)
	if err != nil || u.Hostname() == "" {
		return rawURL
	}
	return u.Hostname()
}

// freePort 分配一个空闲端口。
func freePort() (string, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return "", err
	}
	defer l.Close()
	_, port, err := net.SplitHostPort(l.Addr().String())
	if err != nil {
		return "", err
	}
	return port, nil
}

// waitForPort 等待 TCP 端口就绪。
func waitForPort(ctx context.Context, addr string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		conn, err := net.DialTimeout("tcp", addr, 200*time.Millisecond)
		if err == nil {
			conn.Close()
			return nil
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(200 * time.Millisecond):
		}
	}
	return fmt.Errorf("端口 %s 未就绪", addr)
}
