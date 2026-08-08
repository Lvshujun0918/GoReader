// reader-dev（Go 版）入口
//
// 启动流程：加载 .env → 初始化日志 → 构建配置 → 启动前数据库备份 → serve。
// 与 legacy 行为对齐：workDir 下的 storage/reader.db（WAL），监听 0.0.0.0:port。
package main

import (
	"log"
	"os"

	"github.com/joho/godotenv"

	"github.com/Lvshujun0918/reader-dev/internal/app"
	"github.com/Lvshujun0918/reader-dev/internal/config"
	"github.com/Lvshujun0918/reader-dev/internal/util/dbbackup"
)

// buildVersion 构建版本号（-ldflags "-X main.buildVersion=<git短SHA>" 注入；本地构建为 dev）。
var buildVersion = "dev"

func main() {
	// .env 先加载——日志/配置 env 才能生效
	_ = godotenv.Load()

	initLogging()

	cfg := config.FromEnv()

	// 启动配置摘要（镜像版本/端口/工作目录/鉴权等——docker logs 可直接定位构建）
	log.Printf("===== reader-dev %s 启动 =====", buildVersion)
	log.Printf("配置: 端口=%d workdir=%s secure=%v webRoot=%s", cfg.Port, cfg.StorageDir(), cfg.Secure, cfg.WebRoot)
	log.Printf("配置: obscuraBin=%q obscuraURL=%q proxy=%q", cfg.ObscuraBin, cfg.ObscuraURL, cfg.ObscuraProxy)
	log.Printf("配置: flareSolverr=%q", cfg.FlareSolverrURL)
	log.Printf("配置: tokenTTLDays=%d uploadMaxMB=%d imageCacheMB=%d", cfg.TokenTTLDays, cfg.UploadMaxMB, cfg.ImageCacheMB)
	log.Printf("配置: SSRF 内网放行=%v（READER_ALLOW_PRIVATE_NETWORK=1 可探索/抓取内网或局域网书源）", os.Getenv("READER_ALLOW_PRIVATE_NETWORK") == "1")

	// 启动前数据库备份（reader.db → reader.db.bak-{日期}，保留 5 份；env READER_DB_BACKUP=0 禁用）
	if path, err := dbbackup.BackupReaderDB(cfg.StorageDir()); err != nil {
		log.Printf("启动数据库备份失败（继续启动）: %v", err)
	} else if path != "" {
		log.Printf("启动备份完成: %s", path)
	} else {
		log.Printf("启动备份: 未启用（READER_DB_BACKUP=0 或数据库不存在）")
	}

	if err := app.Serve(cfg); err != nil {
		log.Fatalf("服务启动失败: %v", err)
	}
}

// initLogging 初始化日志：READER_LOG_DIR 设置后启用控制台+文件双写（按大小轮转），否则仅控制台。
func initLogging() {
	logDir := os.Getenv("READER_LOG_DIR")
	if logDir == "" {
		return
	}
	maxSizeMB := envInt("READER_LOG_MAX_SIZE_MB", 10, 1, 1024)
	maxFiles := envInt("READER_LOG_MAX_FILES", 7, 1, 100)
	if err := setupFileLog(logDir, maxSizeMB, maxFiles); err != nil {
		log.Printf("文件日志启用失败（继续控制台输出）: %v", err)
	}
	log.Printf("文件日志已启用: %s（%dMB × %d 个轮转）", logDir, maxSizeMB, maxFiles)
}

func envInt(key string, def, min, max int) int {
	v := def
	if s := os.Getenv(key); s != "" {
		if n, err := parseIntSafe(s); err == nil {
			v = n
		}
	}
	if v < min {
		v = min
	}
	if v > max {
		v = max
	}
	return v
}
