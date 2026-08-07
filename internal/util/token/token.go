// Package token 随机 token 生成。
package token

import (
	"crypto/rand"
	"encoding/hex"
)

// New 生成 32 字节随机 token（hex，64 字符）。
func New() string {
	b := make([]byte, 32)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)
}
