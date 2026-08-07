package password

import (
	"strings"
	"testing"

	"github.com/Lvshujun0918/reader-dev/internal/model"
	"github.com/Lvshujun0918/reader-dev/internal/util/md5"
)

func TestHashPasswordPHCFormat(t *testing.T) {
	phc := HashPassword("pass1234")
	if !strings.HasPrefix(phc, "$argon2id$v=19$m=65536,t=3,p=4$") {
		t.Fatalf("PHC 前缀应含约定参数: %s", phc)
	}
	parts := strings.Split(phc, "$")
	if len(parts) != 6 {
		t.Fatalf("PHC 应为 5 段: %s", phc)
	}
	if !IsArgon2id(phc) {
		t.Fatal("应识别为 argon2id")
	}
	// 随机盐 → 两次哈希不同
	if phc == HashPassword("pass1234") {
		t.Fatal("随机盐应产生不同哈希")
	}
}

func TestHashVerifyRoundtrip(t *testing.T) {
	phc := HashPassword("correct-horse")
	if !VerifyArgon2id("correct-horse", phc) {
		t.Fatal("正确密码应通过")
	}
	if VerifyArgon2id("wrong", phc) {
		t.Fatal("错误密码不应通过")
	}
	if VerifyArgon2id("correct-horse", "not-a-phc") {
		t.Fatal("非法 PHC 不应通过")
	}
	if VerifyArgon2id("correct-horse", "$argon2id$v=19$m=8,t=1,p=1$badsalt$badhash") {
		t.Fatal("损坏 PHC 不应通过")
	}
}

func TestLegacyMD5Compat(t *testing.T) {
	salt := "abcdef12"
	legacy := md5.GenEncryptedPassword("legacy-pass", salt)
	user := &model.User{Password: legacy, Salt: salt}
	ok, needUpgrade := CheckPassword(user, "legacy-pass")
	if !ok {
		t.Fatal("legacy 双 MD5 密码应通过校验")
	}
	if !needUpgrade {
		t.Fatal("legacy 密码应标记需升级")
	}
	// 错误密码
	ok, _ = CheckPassword(user, "wrong")
	if ok {
		t.Fatal("错误密码不应通过")
	}
}
