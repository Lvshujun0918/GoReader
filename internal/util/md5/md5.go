// Package md5 legacy 双 MD5 密码（md5(md5(pwd+salt)+salt)）兼容。
package md5

import (
	"crypto/md5"
	"encoding/hex"
)

// GenEncryptedPassword legacy 双 MD5：md5(md5(pwd+salt)+salt)（小写 hex）。
func GenEncryptedPassword(password, salt string) string {
	h1 := md5.Sum([]byte(password + salt))
	h2 := md5.Sum(append(h1[:], []byte(salt)...))
	return hex.EncodeToString(h2[:])
}
