// Package storage 存储层：SQLite（WAL），兼容迁移自 legacy 的 JSON storage。
package storage

import (
	"fmt"
	"os"
	"time"

	"github.com/glebarez/sqlite"
	"gorm.io/gorm"
	glogger "gorm.io/gorm/logger"

	"github.com/Lvshujun0918/reader-dev/internal/config"
	"github.com/Lvshujun0918/reader-dev/internal/model"
)

// Storage 存储句柄。
type Storage struct {
	DB     *gorm.DB
	Config *config.Config
}

// Init 初始化：建目录 + 打开/建库（WAL）+ 建表 + 幂等迁移 + JSON 迁移。
func Init(cfg *config.Config) (*Storage, error) {
	if err := os.MkdirAll(cfg.StorageDir(), 0o755); err != nil {
		return nil, err
	}

	// 纯 Go SQLite 驱动（无 cgo，静态构建友好）：
	// WAL 模式（并发读写不互斥）+ busy_timeout(5s) + 外键关闭（SQLite 默认）
	dsn := fmt.Sprintf("file:%s?_pragma=journal_mode(WAL)&_pragma=busy_timeout(5000)&_pragma=synchronous(NORMAL)", cfg.DBPath())
	db, err := gorm.Open(sqlite.Open(dsn), &gorm.Config{
		Logger: glogger.Default.LogMode(glogger.Silent),
	})
	if err != nil {
		return nil, fmt.Errorf("打开数据库失败: %w", err)
	}

	sqlDB, err := db.DB()
	if err != nil {
		return nil, err
	}
	// 连接池：8 连接上限（对齐 Rust max_connections(8)）
	sqlDB.SetMaxOpenConns(8)
	sqlDB.SetMaxIdleConns(8)
	sqlDB.SetConnMaxLifetime(time.Hour)

	s := &Storage{DB: db, Config: cfg}

	// 建表（幂等：CREATE TABLE IF NOT EXISTS）
	if err := s.AutoMigrate(); err != nil {
		return nil, err
	}

	// 旧库复合主键重建（幂等）
	if err := s.MigrateNSPrimaryKeys(); err != nil {
		return nil, err
	}

	// JSON storage → SQLite 一次性迁移
	if err := s.MigrateFromJSON(); err != nil {
		return nil, err
	}

	// WAL 快照刷新（对齐 Rust：BEGIN;COMMIT; 每连接）——确保 checkpoint 到主库
	if err := db.Exec("BEGIN; COMMIT;").Error; err != nil {
		return nil, err
	}

	return s, nil
}

// AutoMigrate 全量建表（幂等）。
func (s *Storage) AutoMigrate() error {
	return s.DB.AutoMigrate(
		&model.User{},
		&model.Book{},
		&model.BookSource{},
		&model.RssSource{},
		&model.RssArticle{},
		&model.BookChapter{},
		&model.TocCache{},
		&model.Bookmark{},
		&model.BookGroup{},
		&model.ReplaceRule{},
		&model.TxtTocRule{},
		&model.HttpTTS{},
		&model.SourceSub{},
		&model.BookSourceCookie{},
		&model.SystemSetting{},
		&model.UserConfig{},
		&model.ReadingStat{},
	)
}

// Now Unix 秒时间戳。
func Now() int64 { return time.Now().Unix() }
