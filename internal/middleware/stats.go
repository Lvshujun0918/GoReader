package middleware

import (
	"sync"
	"sync/atomic"
	"time"

	"github.com/gin-gonic/gin"
)

// RequestStats 请求计数（对齐 Rust service::monitor::REQUESTS）。
type RequestStats struct {
	mu     sync.Mutex
	total  atomic.Int64
	today  atomic.Int64
	day    string
	byPath map[string]int64
}

// NewRequestStats 创建统计器。
func NewRequestStats() *RequestStats {
	return &RequestStats{
		byPath: make(map[string]int64),
		day:    time.Now().Format("2006-01-02"),
	}
}

// Stats 请求计数中间件（最外层——413/404/静态资源同样计入）。
func (s *RequestStats) Handler() gin.HandlerFunc {
	return func(c *gin.Context) {
		// 跨天重置今日计数
		if d := time.Now().Format("2006-01-02"); d != s.day {
			s.mu.Lock()
			if d != s.day {
				s.day = d
				s.today.Store(0)
			}
			s.mu.Unlock()
		}
		c.Next()
		s.total.Add(1)
		s.today.Add(1)
		path := c.Request.URL.Path
		s.mu.Lock()
		s.byPath[path]++
		s.mu.Unlock()
	}
}

// Total 总请求数。
func (s *RequestStats) Total() int64 { return s.total.Load() }

// Today 今日请求数。
func (s *RequestStats) Today() int64 { return s.today.Load() }

// Top 按路径计数 TopN（降序）。
func (s *RequestStats) Top(n int) []PathCount {
	s.mu.Lock()
	defer s.mu.Unlock()
	type kv struct {
		path  string
		count int64
	}
	items := make([]kv, 0, len(s.byPath))
	for p, c := range s.byPath {
		items = append(items, kv{p, c})
	}
	// 简单选择排序 TopN
	for i := 0; i < len(items) && i < n; i++ {
		for j := i + 1; j < len(items); j++ {
			if items[j].count > items[i].count {
				items[i], items[j] = items[j], items[i]
			}
		}
	}
	out := make([]PathCount, 0, min(n, len(items)))
	for _, it := range items[:min(n, len(items))] {
		out = append(out, PathCount{Path: it.path, Count: it.count})
	}
	return out
}

// PathCount 单路径计数。
type PathCount struct {
	Path  string `json:"path"`
	Count int64  `json:"count"`
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
