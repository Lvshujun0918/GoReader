package app

import (
	"github.com/Lvshujun0918/GoReader/internal/storage"
)

// StartBackgroundJobs 后台任务（对齐 Rust service::schedule + local_sync）：
// 书架更新检查（10 分钟）+ 订阅/RSS 自动刷新 + 每日自动备份。
func StartBackgroundJobs(st *storage.Storage) {
	go scheduleLoop(st)
}

func scheduleLoop(st *storage.Storage) {
	// TODO(迭代): 实现 10 分钟书架更新、订阅刷新、每日自动备份的定时任务
	_ = st
}
