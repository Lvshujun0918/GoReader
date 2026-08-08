package config

import "testing"

// TestFlagFromStr 布尔解析——true/1/yes/on（大小写不敏感）→ true，其余/缺失 → false。
func TestFlagFromStr(t *testing.T) {
	for _, v := range []string{"true", "TRUE", "True", "1", "yes", "on", "On"} {
		if !flagFromStr(v) {
			t.Errorf("%q 应为 true", v)
		}
	}
	for _, v := range []string{"false", "0", "no", "off", "", "2", "tru", "true "} {
		if flagFromStr(v) {
			t.Errorf("%q 应为 false", v)
		}
	}
}

// TestFromEnvDefaultUserFlags 默认用户权限 env 正确读取。
func TestFromEnvDefaultUserFlags(t *testing.T) {
	t.Setenv("READER_APP_DEFAULTUSERENABLEBOOKSOURCE", "true")
	cfg := FromEnv()
	if !cfg.DefaultUserEnableBookSource {
		t.Error("BOOKSOURCE 应为 true")
	}
}

// TestFromEnvDefaults 缺省值正确。
func TestFromEnvDefaults(t *testing.T) {
	cfg := FromEnv()
	if cfg.Port != 8080 {
		t.Errorf("Port 默认应为 8080，got %d", cfg.Port)
	}
	if cfg.UploadMaxMB != 100 {
		t.Errorf("UploadMaxMB 默认应为 100，got %d", cfg.UploadMaxMB)
	}
	if cfg.WebRoot != "web-ui/dist" {
		t.Errorf("WebRoot 默认应为 web-ui/dist，got %q", cfg.WebRoot)
	}
	if cfg.UploadMaxBytes() != 100*1024*1024 {
		t.Errorf("UploadMaxBytes 应为 100MB，got %d", cfg.UploadMaxBytes())
	}
}

// TestStorageDir workDir 为空时指向 ./storage。
func TestStorageDir(t *testing.T) {
	t.Setenv("READER_APP_WORKDIR", "")
	cfg := FromEnv()
	if got := cfg.StorageDir(); got != "storage" {
		t.Errorf("StorageDir 应为 storage，got %q", got)
	}
	t.Setenv("READER_APP_WORKDIR", "/tmp/rd")
	cfg = FromEnv()
	if got := cfg.StorageDir(); got != "/tmp/rd/storage" {
		t.Errorf("StorageDir 应为 /tmp/rd/storage，got %q", got)
	}
}
