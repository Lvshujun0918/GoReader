// Package loginlimit 登录限流：5 次失败锁 5 分钟（用户名+客户端 IP）。
package loginlimit

import (
	"fmt"
	"sync"
	"time"
)

const (
	maxFailures = 5
	lockWindow  = 5 * time.Minute
)

type entry struct {
	failures   int
	lockedUntil time.Time
}

var (
	mu      sync.Mutex
	entries = make(map[string]*entry)
)

func key(username, ip string) string { return username + "|" + ip }

// CheckAllowed 检查是否允许登录（锁定中返回错误消息）。
func CheckAllowed(username, ip string) error {
	mu.Lock()
	defer mu.Unlock()
	e := entries[key(username, ip)]
	if e == nil {
		return nil
	}
	if time.Now().Before(e.lockedUntil) {
		return fmt.Errorf("登录失败次数过多，请%d分钟后再试", int(time.Until(e.lockedUntil).Minutes())+1)
	}
	return nil
}

// RecordFailure 记录一次失败。
func RecordFailure(username, ip string) {
	mu.Lock()
	defer mu.Unlock()
	k := key(username, ip)
	e := entries[k]
	if e == nil {
		e = &entry{}
		entries[k] = e
	}
	e.failures++
	if e.failures >= maxFailures {
		e.lockedUntil = time.Now().Add(lockWindow)
		e.failures = 0
	}
}

// Reset 登录成功重置。
func Reset(username, ip string) {
	mu.Lock()
	defer mu.Unlock()
	delete(entries, key(username, ip))
}
