// Package config 应用配置（env / .env，兼容 READER_APP_* 前缀）。
package config

import (
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// Config 应用配置（兼容 legacy READER_APP_* env）。
type Config struct {
	// WorkDir 工作目录（storage 根，兼容 READER_APP_WORKDIR）
	WorkDir string
	// Port 服务端口
	Port int
	// Secure 是否启用登录鉴权（多用户）
	Secure bool
	// SecureKey 管理密码
	SecureKey string
	// UserLimit 用户上限
	UserLimit int64
	// UserBookLimit 用户书籍上限
	UserBookLimit int64
	// InviteCode 邀请码
	InviteCode string
	// MinUserPasswordLength 最小密码长度
	MinUserPasswordLength int64
	// TokenTTLDays token 有效期（天，<=0 永不过期）
	TokenTTLDays int64
	// WebRoot 前端静态资源根（构建产物 dist 目录）
	WebRoot string
	// SimpleWebRoot Kindle 轻量页目录
	SimpleWebRoot string
	// DefaultUserEnableWebdav 默认新用户权限
	DefaultUserEnableWebdav bool
	DefaultUserEnableLocalStore bool
	DefaultUserEnableBookSource bool
	DefaultUserEnableRssSource bool
	// DefaultUserBookSourceLimit 默认新用户书源上限
	DefaultUserBookSourceLimit int64
	// DefaultUserBookLimit 默认新用户书籍上限
	DefaultUserBookLimit int64
	// UploadMaxMB 上传大小上限（MB）
	UploadMaxMB int64
	// ImageCacheMB 图片代理磁盘缓存容量（MB，0=禁用）
	ImageCacheMB int64
	// MongoURI MongoDB 备份连接串
	MongoURI string
	// TrustedProxies 逗号分隔 IP/CIDR 白名单（仅命中才信 X-Forwarded-For）
	TrustedProxies []string
	// LocalBookDir 本地书双轨同步额外目录
	LocalBookDir string
	// AutoBackupHour 每日自动备份小时
	AutoBackupHour int
	// FlareSolverrURL 外部 FlareSolverr 服务地址（可选，未实现客户端）
	FlareSolverrURL string
	// ObscuraURL / Bin / Proxy 浏览器自动化后端（质询求解，见 internal/service/solver）
	ObscuraURL   string
	ObscuraBin   string
	ObscuraProxy string
	// CDPNoStealth 跳过 stealth JS 注入（测试钩子）
	CDPNoStealth bool
}

// FromEnv 从环境变量构建配置。
func FromEnv() *Config {
	return &Config{
		WorkDir:                  os.Getenv("READER_APP_WORKDIR"),
		Port:                     envInt("READER_SERVER_PORT", 8080),
		Secure:                   envFlag("READER_APP_SECURE"),
		SecureKey:                os.Getenv("READER_APP_SECUREKEY"),
		UserLimit:                envI64("READER_APP_USERLIMIT", 500000),
		UserBookLimit:            envI64("READER_APP_USERBOOKLIMIT", 500000),
		InviteCode:               os.Getenv("READER_APP_INVITECODE"),
		MinUserPasswordLength:    envI64("READER_APP_MINUSERPASSWORDLENGTH", 8),
		TokenTTLDays:             envI64("READER_TOKEN_TTL_DAYS", 30),
		WebRoot:                  envStr("READER_APP_WEB_ROOT", "web-ui/dist"),
		SimpleWebRoot:            envStr("READER_APP_SIMPLE_WEB_ROOT", "web-simple"),
		DefaultUserEnableWebdav:  envFlag("READER_APP_DEFAULTUSERENABLEWEBDAV"),
		DefaultUserEnableLocalStore: envFlag("READER_APP_DEFAULTUSERENABLELOCALSTORE"),
		DefaultUserEnableBookSource: envFlag("READER_APP_DEFAULTUSERENABLEBOOKSOURCE"),
		DefaultUserEnableRssSource: envFlag("READER_APP_DEFAULTUSERENABLERSSSOURCE"),
		DefaultUserBookSourceLimit: envI64("READER_APP_DEFAULTUSERBOOKSOURCELIMIT", 100),
		DefaultUserBookLimit:     envI64("READER_APP_DEFAULTUSERBOOKLIMIT", 200),
		UploadMaxMB:              envI64("READER_UPLOAD_MAX_MB", 100),
		ImageCacheMB:             envI64("READER_IMAGE_CACHE_MB", 512),
		MongoURI:                 os.Getenv("READER_MONGODB_URI"),
		TrustedProxies:           envList("READER_TRUSTED_PROXIES"),
		LocalBookDir:             os.Getenv("READER_LOCAL_BOOK_DIR"),
		AutoBackupHour:           envInt("READER_AUTO_BACKUP_HOUR", 3),
		FlareSolverrURL:          os.Getenv("FLARESOLVERR_URL"),
		ObscuraURL:               os.Getenv("READER_OBSCURA_URL"),
		ObscuraBin:               os.Getenv("READER_OBSCURA_BIN"),
		ObscuraProxy:             os.Getenv("READER_OBSCURA_PROXY"),
		CDPNoStealth:             envFlag("READER_CDP_NO_STEALTH"),
	}
}

// UploadMaxBytes 上传大小上限字节数。
func (c *Config) UploadMaxBytes() int64 {
	if c.UploadMaxMB < 1 {
		return 1 * 1024 * 1024
	}
	return c.UploadMaxMB * 1024 * 1024
}

// StorageDir storage 根目录（workDir 下的 storage）。
func (c *Config) StorageDir() string {
	base := "."
	if c.WorkDir != "" {
		base = c.WorkDir
	}
	return filepath.Join(base, "storage")
}

// AssetsDir 静态资源目录（storage/assets）。
func (c *Config) AssetsDir() string {
	return filepath.Join(c.StorageDir(), "assets")
}

// DBPath SQLite 数据库路径。
func (c *Config) DBPath() string {
	return filepath.Join(c.StorageDir(), "reader.db")
}

func envStr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func envInt(key string, def int) int {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			return n
		}
	}
	return def
}

func envI64(key string, def int64) int64 {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.ParseInt(v, 10, 64); err == nil {
			return n
		}
	}
	return def
}

// envFlag 布尔解析：true/1/yes/on（大小写不敏感）→ true，其余/缺失 → false。
func envFlag(key string) bool {
	return flagFromStr(os.Getenv(key))
}

func flagFromStr(v string) bool {
	switch strings.ToLower(v) {
	case "true", "1", "yes", "on":
		return true
	}
	return false
}

func envList(key string) []string {
	v := os.Getenv(key)
	if v == "" {
		return nil
	}
	parts := strings.Split(v, ",")
	out := make([]string, 0, len(parts))
	for _, p := range parts {
		if p = strings.TrimSpace(p); p != "" {
			out = append(out, p)
		}
	}
	return out
}
