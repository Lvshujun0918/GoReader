package api

import (
	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/reader-dev/internal/config"
	"github.com/Lvshujun0918/reader-dev/internal/middleware"
	"github.com/Lvshujun0918/reader-dev/internal/service/solver"
	"github.com/Lvshujun0918/reader-dev/internal/storage"
)

// API 处理器集合（共享存储与配置）。
type API struct {
	Storage *storage.Storage
	Config  *config.Config
	Stats   *middleware.RequestStats
	Solver  *solver.Solver // 全局 obscura 质询求解器（READER_OBSCURA_URL/BIN 配置）
}

// New 创建 API 处理器。
func New(st *storage.Storage, cfg *config.Config, stats *middleware.RequestStats) *API {
	return &API{
		Storage: st,
		Config:  cfg,
		Stats:   stats,
		Solver:  solver.New(cfg.ObscuraURL, cfg.ObscuraBin, cfg.ObscuraProxy),
	}
}

// Engine 构建 gin 引擎。
func (a *API) Engine() *gin.Engine {
	gin.SetMode(gin.ReleaseMode)
	r := gin.New()
	r.Use(gin.Recovery())

	// 全局中间件（对齐 Rust serve() 挂载顺序——Gin 中间件先注册先执行）
	r.Use(middleware.CacheControl())
	r.Use(middleware.UploadLimit(a.Config.UploadMaxBytes()))
	r.Use(a.Stats.Handler())

	// 基础路由
	r.GET("/health", a.handleHealth)
	r.GET("/assets/proxy", a.handleAssetsProxy)

	// OPDS
	r.Any("/opds", a.handleOpds)
	r.Any("/opds/*rest", a.handleOpds)
	r.Any("/opds-save", a.handleOpdsSave)

	// WebDAV（任意方法）
	r.Any("/reader3/webdav/*rest", a.handleWebDAV)

	// /reader3 API 路由
	a.registerReader3(r)

	return r
}
