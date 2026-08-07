package main

import (
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strconv"
	"sync"
)

// parseIntSafe 宽松整数解析（非法输入回退 0+err）。
func parseIntSafe(s string) (int, error) {
	return strconv.Atoi(s)
}

// rotatingFileWriter 按大小轮转的日志 writer（对齐 Rust 版 RotatingFileWriter）。
// 文件命名：{dir}/{prefix}.log（当前）、{dir}/{prefix}.log.1 …（历史，编号越大越旧）。
type rotatingFileWriter struct {
	mu        sync.Mutex
	dir       string
	prefix    string
	maxBytes  int64
	maxFiles  int
	current   *os.File
	curBytes  int64
}

// setupFileLog 初始化文件日志：log 包双写（stdout + 文件）。
func setupFileLog(dir string, maxSizeMB, maxFiles int) error {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	w := &rotatingFileWriter{
		dir:      dir,
		prefix:   "reader-dev",
		maxBytes: int64(maxSizeMB) * 1024 * 1024,
		maxFiles: maxFiles,
	}
	if err := w.openCurrent(); err != nil {
		return err
	}
	// log.SetOutput 支持多 writer 需 MultiWriter；为最小改动，直接替换为自定义 writer。
	logWriter := &multiWriter{w: w}
	log.SetOutput(logWriter)
	return nil
}

func (r *rotatingFileWriter) openCurrent() error {
	path := filepath.Join(r.dir, r.prefix+".log")
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if err != nil {
		return err
	}
	info, err := f.Stat()
	if err != nil {
		f.Close()
		return err
	}
	r.current = f
	r.curBytes = info.Size()
	return nil
}

func (r *rotatingFileWriter) Write(p []byte) (int, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.current == nil {
		return len(p), nil
	}
	n, err := r.current.Write(p)
	r.curBytes += int64(n)
	if r.curBytes >= r.maxBytes {
		_ = r.rotate()
	}
	return n, err
}

// rotate 轮转：.{i} → .{i+1} 平移，超出 maxFiles 删除最旧，重开当前文件。
func (r *rotatingFileWriter) rotate() error {
	if r.current != nil {
		_ = r.current.Close()
		r.current = nil
	}
	cur := filepath.Join(r.dir, r.prefix+".log")
	// 删除最旧
	oldest := filepath.Join(r.dir, fmt.Sprintf("%s.log.%d", r.prefix, r.maxFiles))
	_ = os.Remove(oldest)
	// 平移
	for i := r.maxFiles - 1; i >= 1; i-- {
		from := filepath.Join(r.dir, fmt.Sprintf("%s.log.%d", r.prefix, i))
		to := filepath.Join(r.dir, fmt.Sprintf("%s.log.%d", r.prefix, i+1))
		if _, err := os.Stat(from); err == nil {
			_ = os.Rename(from, to)
		}
	}
	if _, err := os.Stat(cur); err == nil {
		_ = os.Rename(cur, filepath.Join(r.dir, r.prefix+".log.1"))
	}
	return r.openCurrent()
}

// multiWriter 控制台 + 文件双写。
type multiWriter struct {
	w *rotatingFileWriter
}

func (m *multiWriter) Write(p []byte) (int, error) {
	n, err := os.Stdout.Write(p)
	_, _ = m.w.Write(p)
	return n, err
}
