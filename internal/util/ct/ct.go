// Package ct 常量时间字符串比较（防时序侧信道）。
package ct

import (
	"crypto/subtle"
)

// Equal 常量时间字符串比较。
func Equal(a, b string) bool {
	return subtle.ConstantTimeCompare([]byte(a), []byte(b)) == 1
}
