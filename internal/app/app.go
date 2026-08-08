// Package app 应用启动（配置 → 存储初始化 → 路由 → 监听）。
package app

import (
	"fmt"
	"log"
	"net/http"
	"os"
	"time"

	"github.com/Lvshujun0918/reader-dev/internal/api"
	"github.com/Lvshujun0918/reader-dev/internal/config"
	"github.com/Lvshujun0918/reader-dev/internal/middleware"
	"github.com/Lvshujun0918/reader-dev/internal/storage"
)

// Serve 启动服务（对齐 Rust config.serve()）：
// 存储初始化 → 定时任务 → 路由 → 中间件 → 监听 0.0.0.0:port。
func Serve(cfg *config.Config) error {
	st, err := storage.Init(cfg)
	if err != nil {
		return err
	}
	log.Printf("数据库就绪: %s", cfg.DBPath())

	// 本地书双轨同步 + 定时任务（书架更新/订阅刷新/每日自动备份）
	StartBackgroundJobs(st)

	stats := middleware.NewRequestStats()
	apiHandler := api.New(st, cfg, stats)

	router := apiHandler.Engine()
	// 请求日志（默认开启，READER_REQUEST_LOG=0 关闭——docker logs 逐请求追踪）
	if os.Getenv("READER_REQUEST_LOG") != "0" {
		router.Use(middleware.RequestLog(true))
	}
	log.Printf("路由注册完成: %d 条", len(router.Routes()))

	addr := fmt.Sprintf("0.0.0.0:%d", cfg.Port)
	srv := &http.Server{
		Addr:              addr,
		Handler:           router,
		ReadHeaderTimeout: 30 * time.Second,
	}
	log.Printf("reader-dev (Go) listening on %s", addr)
	return srv.ListenAndServe()
}
