// Package password 用户密码哈希（argon2id PHC + legacy 双 MD5 兼容校验/自动升级）。
//
// 存储格式：users.password 列存 PHC 字符串
// `$argon2id$v=19$m=65536,t=3,p=4$<salt>$<hash>`（盐为随机 16 字节，base64 无 padding）。
// 旧数据为 legacy 双 MD5（md5(md5(pwd+salt)+salt)，盐存 users.salt 列）——校验兼容，
// 通过后登录成功路径自动升级为 argon2id。
package password

import (
	"crypto/rand"
	"encoding/base64"
	"fmt"
	"strconv"
	"strings"

	"golang.org/x/crypto/argon2"

	"github.com/Lvshujun0918/GoReader/internal/model"
	"github.com/Lvshujun0918/GoReader/internal/util/ct"
	"github.com/Lvshujun0918/GoReader/internal/util/md5"
)

const (
	// Argon2M 内存成本（KiB）：64 MiB
	Argon2M uint32 = 65536
	// Argon2T 迭代次数
	Argon2T uint32 = 3
	// Argon2P 并行度
	Argon2P uint8 = 4
	// Argon2KeyLen 输出长度（字节）
	Argon2KeyLen uint32 = 32
)

// HashPassword 生成 argon2id PHC 哈希（随机 16 字节盐）。
func HashPassword(password string) string {
	salt := make([]byte, 16)
	_, _ = rand.Read(salt)
	hash := argon2.IDKey([]byte(password), salt, Argon2T, Argon2M, Argon2P, Argon2KeyLen)
	return fmt.Sprintf("$argon2id$v=19$m=%d,t=%d,p=%d$%s$%s",
		Argon2M, Argon2T, Argon2P,
		base64.RawStdEncoding.EncodeToString(salt),
		base64.RawStdEncoding.EncodeToString(hash))
}

// VerifyArgon2id 校验 argon2id PHC 字符串（成本参数以存储串内嵌值为准）。
func VerifyArgon2id(password, phc string) bool {
	parts := strings.Split(phc, "$")
	// ["", "argon2id", "v=19", "m=...,t=...,p=...", salt, hash]
	if len(parts) != 6 {
		return false
	}
	if parts[1] != "argon2id" {
		return false
	}
	var m, t uint32
	var p uint8
	for _, kv := range strings.Split(parts[3], ",") {
		keyVal := strings.SplitN(kv, "=", 2)
		if len(keyVal) != 2 {
			return false
		}
		switch keyVal[0] {
		case "m":
			v, err := strconv.ParseUint(keyVal[1], 10, 32)
			if err != nil {
				return false
			}
			m = uint32(v)
		case "t":
			v, err := strconv.ParseUint(keyVal[1], 10, 32)
			if err != nil {
				return false
			}
			t = uint32(v)
		case "p":
			v, err := strconv.ParseUint(keyVal[1], 10, 8)
			if err != nil {
				return false
			}
			p = uint8(v)
		}
	}
	salt, err := base64.RawStdEncoding.DecodeString(parts[4])
	if err != nil {
		return false
	}
	expect, err := base64.RawStdEncoding.DecodeString(parts[5])
	if err != nil {
		return false
	}
	got := argon2.IDKey([]byte(password), salt, t, m, p, uint32(len(expect)))
	return ct.Equal(string(got), string(expect))
}

// IsArgon2id 是否为 argon2id PHC 存储。
func IsArgon2id(stored string) bool {
	return strings.HasPrefix(stored, "$argon2id$")
}

// CheckPassword 统一密码校验（纯函数）：argon2id 优先；否则 legacy 双 MD5。
// 返回（是否通过, 通过时是否需要升级为 argon2id）。
func CheckPassword(user *model.User, password string) (ok bool, needUpgrade bool) {
	if IsArgon2id(user.Password) {
		return VerifyArgon2id(password, user.Password), false
	}
	ok = ct.Equal(md5.GenEncryptedPassword(password, user.Salt), user.Password)
	return ok, ok
}
