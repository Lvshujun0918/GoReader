package api

import (
	"net"
	"os"
	"strconv"
	"strings"

	"github.com/gin-gonic/gin"
)

// ClientIP 客户端 IP 解析（P1-2）：默认只用直连 IP（不可伪造）；
// X-Forwarded-For 仅当直连 IP 命中 READER_TRUSTED_PROXIES 白名单时取最左项。
func ClientIP(c *gin.Context) string {
	peer := c.ClientIP()
	if peer == "" {
		return ""
	}
	if !trustedProxy(peer) {
		return peer
	}
	xff := c.GetHeader("X-Forwarded-For")
	if xff != "" {
		first := strings.TrimSpace(strings.Split(xff, ",")[0])
		if first != "" {
			return first
		}
	}
	return peer
}

// trustedProxy 直连 IP 是否命中 READER_TRUSTED_PROXIES 白名单。
func trustedProxy(ip string) bool {
	parsed := net.ParseIP(ip)
	if parsed == nil {
		return false
	}
	for _, spec := range trustedProxyNets {
		if spec.matches(parsed) {
			return true
		}
	}
	return false
}

type ipNet struct {
	net    net.IP
	prefix int
	isCIDR bool
}

func (n ipNet) matches(ip net.IP) bool {
	if !n.isCIDR {
		return ip.Equal(n.net)
	}
	// 按前缀长度逐位匹配
	if ip4 := ip.To4(); ip4 != nil && n.net.To4() != nil {
		nb := n.net.To4()
		prefix := n.prefix
		if prefix > 32 {
			prefix = 32
		}
		for i := 0; i < prefix; i++ {
			if bitAt(ip4, i) != bitAt(nb, i) {
				return false
			}
		}
		return true
	}
	if ip16 := ip.To16(); ip16 != nil && n.net.To16() != nil {
		nb := n.net.To16()
		prefix := n.prefix
		if prefix > 128 {
			prefix = 128
		}
		for i := 0; i < prefix; i++ {
			if bitAt(ip16, i) != bitAt(nb, i) {
				return false
			}
		}
		return true
	}
	return false
}

func bitAt(b []byte, i int) bool {
	return b[i/8]&(1<<(7-uint(i%8))) != 0
}

// trustedProxyNets 首次使用时从 env 解析（READER_TRUSTED_PROXIES：逗号分隔 IP/CIDR）。
var trustedProxyNets = parseTrustedProxies(os.Getenv("READER_TRUSTED_PROXIES"))

func parseTrustedProxies(raw string) []ipNet {
	var out []ipNet
	for _, s := range strings.Split(raw, ",") {
		s = strings.TrimSpace(s)
		if s == "" {
			continue
		}
		if i := strings.IndexByte(s, '/'); i >= 0 {
			ip := net.ParseIP(s[:i])
			if ip == nil {
				continue
			}
			prefix, err := strconv.Atoi(s[i+1:])
			if err != nil {
				continue
			}
			out = append(out, ipNet{net: ip, prefix: prefix, isCIDR: true})
		} else {
			ip := net.ParseIP(s)
			if ip != nil {
				out = append(out, ipNet{net: ip})
			}
		}
	}
	return out
}
