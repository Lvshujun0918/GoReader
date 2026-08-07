// Package dbbackup 启动前数据库备份：reader.db → reader.db.bak-{日期}（保留最近 5 份）。
package dbbackup

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

const (
	keepCount = 5
	prefix    = "reader.db.bak-"
)

// BackupReaderDB 备份 storage/reader.db。READER_DB_BACKUP=0 时禁用。
// 返回备份文件路径（未执行时为空串）。仅备份 .db（不含 -wal/-shm）。
func BackupReaderDB(storageDir string) (string, error) {
	if os.Getenv("READER_DB_BACKUP") == "0" {
		return "", nil
	}
	dbPath := filepath.Join(storageDir, "reader.db")
	info, err := os.Stat(dbPath)
	if err != nil {
		if os.IsNotExist(err) {
			return "", nil // 无库无需备份
		}
		return "", err
	}
	if info.IsDir() {
		return "", nil
	}

	// WAL 一致性：备份前做一次 checkpoint（BEGIN;COMMIT;）由 storage 层负责；
	// 此处直接拷贝主库文件。
	backupPath := filepath.Join(storageDir, prefix+time.Now().Format("2006-01-02"))
	if _, err := os.Stat(backupPath); err == nil {
		return backupPath, nil // 当天已备份
	}
	data, err := os.ReadFile(dbPath)
	if err != nil {
		return "", err
	}
	if err := os.WriteFile(backupPath, data, 0o644); err != nil {
		return "", err
	}
	prune(storageDir)
	return backupPath, nil
}

// prune 保留最近 keepCount 份，删除更旧备份。
func prune(storageDir string) {
	entries, err := os.ReadDir(storageDir)
	if err != nil {
		return
	}
	var baks []string
	for _, e := range entries {
		if e.IsDir() || !strings.HasPrefix(e.Name(), prefix) {
			continue
		}
		baks = append(baks, e.Name())
	}
	if len(baks) <= keepCount {
		return
	}
	sort.Strings(baks)
	for _, name := range baks[:len(baks)-keepCount] {
		_ = os.Remove(filepath.Join(storageDir, name))
	}
}
