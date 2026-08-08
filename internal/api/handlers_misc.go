package api

import (
	"crypto/rand"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"time"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/GoReader/internal/model"
	"github.com/Lvshujun0918/GoReader/internal/storage"
)

var crand = rand.Reader

// ---------------- 书签 ----------------

func (a *API) handleSaveBookmark(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	var bm model.Bookmark
	if err := c.ShouldBindJSON(&bm); err != nil {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.SaveBookmark(ns, &bm); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleSaveBookmarks(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	var list []*model.Bookmark
	if err := c.ShouldBindJSON(&list); err != nil {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.SaveBookmarks(ns, list); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleGetBookmarks(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	list, err := a.Storage.ListBookmarks(ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, list)
}

func (a *API) handleDeleteBookmark(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	if err := a.Storage.DeleteBookmark(ns, paramOf(params, "bookUrl"), paramOf(params, "title")); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleDeleteBookmarks(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	bookURL := paramOf(params, "bookUrl")
	titles := stringArrayParam(params, "titles")
	if len(titles) == 0 {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.DeleteBookmarks(ns, bookURL, titles); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// ---------------- 书架分组 ----------------

func (a *API) handleGetBookGroups(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	groups, err := a.Storage.ListBookGroups(ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	// 附 bookCount
	books, err := a.Storage.ListBooks(ns)
	if err == nil {
		counts := map[int64]int{}
		for _, b := range books {
			counts[b.GroupName]++
		}
		type groupOut struct {
			model.BookGroup
			BookCount int `json:"bookCount"`
		}
		out := make([]groupOut, 0, len(groups))
		for _, g := range groups {
			out = append(out, groupOut{BookGroup: g, BookCount: counts[g.ID]})
		}
		OK(c, out)
		return
	}
	OK(c, groups)
}

func (a *API) handleSaveBookGroup(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	g := &model.BookGroup{Name: paramOf(params, "name")}
	if id, ok := intParam(params, "id"); ok {
		g.ID = id
	}
	if err := a.Storage.SaveBookGroup(ns, g); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, g)
}

func (a *API) handleUpdateBookGroupID(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	groupID, _ := intParam(params, "groupId")
	urls := stringArrayParam(params, "bookUrls")
	if len(urls) == 0 {
		if v := paramOf(params, "bookUrl"); v != "" {
			urls = []string{v}
		}
	}
	if len(urls) == 0 {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.MoveBookToGroup(ns, groupID, urls); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleDeleteBookGroup(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	id, _ := intParam(a.params(c), "id")
	if err := a.Storage.DeleteBookGroup(ns, id); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleAddBookGroupMulti(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	groupID, _ := intParam(params, "groupId")
	urls := stringArrayParam(params, "bookUrls")
	if err := a.Storage.MoveBookToGroup(ns, groupID, urls); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleRemoveBookGroupMulti(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	urls := stringArrayParam(params, "bookUrls")
	if err := a.Storage.MoveBookToGroup(ns, 0, urls); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleSaveBookGroupOrder(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	var ids []int64
	if arr, ok := params["groupIds"].([]any); ok {
		for _, v := range arr {
			switch t := v.(type) {
			case float64:
				ids = append(ids, int64(t))
			}
		}
	}
	if err := a.Storage.SaveBookGroupOrder(ns, ids); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// ---------------- 替换规则 ----------------

func (a *API) handleGetReplaceRules(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	list, err := a.Storage.ListReplaceRules(ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, list)
}

func (a *API) handleSaveReplaceRule(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	var r model.ReplaceRule
	if err := c.ShouldBindJSON(&r); err != nil {
		Fail(c, "参数错误")
		return
	}
	if r.ID == "" {
		r.ID = randomUUID()
	}
	if err := a.Storage.SaveReplaceRule(ns, &r); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleSaveReplaceRules(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	var list []*model.ReplaceRule
	if err := c.ShouldBindJSON(&list); err != nil {
		Fail(c, "参数错误")
		return
	}
	for _, r := range list {
		if r.ID == "" {
			r.ID = randomUUID()
		}
	}
	if err := a.Storage.SaveReplaceRules(ns, list); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleDeleteReplaceRule(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	id := paramOf(a.params(c), "id")
	if err := a.Storage.DeleteReplaceRule(ns, id); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleDeleteReplaceRules(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	ids := stringArrayParam(a.params(c), "ids")
	if len(ids) == 0 {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.DeleteReplaceRules(ns, ids); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// ---------------- TXT 目录规则 ----------------

func (a *API) handleGetTxtTocRules(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	list, err := a.Storage.ListTxtTocRules(ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, list)
}

func (a *API) handleSaveTxtTocRule(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	var r model.TxtTocRule
	if err := c.ShouldBindJSON(&r); err != nil {
		Fail(c, "参数错误")
		return
	}
	if r.ID == "" {
		r.ID = randomUUID()
	}
	if err := a.Storage.SaveTxtTocRule(ns, &r); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleDeleteTxtTocRule(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	id := paramOf(a.params(c), "id")
	if err := a.Storage.DeleteTxtTocRule(ns, id); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleImportDefaultTxtTocRules(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	// 内置默认规则
	defaults := []*model.TxtTocRule{
		{ID: randomUUID(), Name: "默认正则目录", Rule: "^\\s*(第[0-9一二三四五六七八九十百千]+[章节卷部集篇]).*", Enabled: 1, SerialNumber: 0, UserNamespace: ns},
	}
	if err := a.Storage.DB.CreateInBatches(defaults, len(defaults)).Error; err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// ---------------- HttpTTS ----------------

func (a *API) handleGetHttpTTSList(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	list, err := a.Storage.ListHttpTTS(ns)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, list)
}

func (a *API) handleSaveHttpTTS(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	var t model.HttpTTS
	if err := c.ShouldBindJSON(&t); err != nil {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.SaveHttpTTS(ns, &t); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleSaveHttpTTSMulti(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	var list []*model.HttpTTS
	if err := c.ShouldBindJSON(&list); err != nil {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.SaveHttpTTSMulti(ns, list); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleDeleteHttpTTS(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	url := paramOf(a.params(c), "url")
	if err := a.Storage.DeleteHttpTTS(ns, url); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// ---------------- TTS / 导出 ----------------

func (a *API) handleGetTTSVoices(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	// 内置 Edge TTS 中文语音
	OK(c, []map[string]any{
		{"name": "晓晓", "shortName": "zh-CN-XiaoxiaoNeural", "locale": "zh-CN"},
		{"name": "云希", "shortName": "zh-CN-YunxiNeural", "locale": "zh-CN"},
		{"name": "云健", "shortName": "zh-CN-YunjianNeural", "locale": "zh-CN"},
		{"name": "晓伊", "shortName": "zh-CN-XiaoyiNeural", "locale": "zh-CN"},
		{"name": "云夏", "shortName": "zh-CN-YunxiaNeural", "locale": "zh-CN"},
	})
}

func (a *API) handleTTS(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	params := a.params(c)
	_ = params
	Fail(c, "TTS 合成功能实现中")
}

func (a *API) handleExportBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	params := a.params(c)
	bookURL := paramOf(params, "bookUrl")
	if bookURL == "" {
		Fail(c, "参数错误")
		return
	}
	chapters, err := a.Storage.ListChapters(bookURL)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	var b strings.Builder
	for _, ch := range chapters {
		b.WriteString(ch.Title)
		b.WriteString("\n\n")
		b.WriteString(ch.Content)
		b.WriteString("\n\n")
	}
	c.Header("Content-Disposition", `attachment; filename="export.txt"`)
	c.Data(200, "text/plain; charset=utf-8", []byte(b.String()))
}

// ---------------- 备份恢复 ----------------

func (a *API) handleBackupToWebdav(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "WebDAV 备份功能实现中")
}

func (a *API) handleRestoreFromZip(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "ZIP 恢复功能实现中")
}

func (a *API) handleRestoreFromWebdav(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "WebDAV 恢复功能实现中")
}

func (a *API) handleBackupToMongodb(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "MongoDB 备份功能实现中")
}

func (a *API) handleRestoreFromMongodb(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "MongoDB 恢复功能实现中")
}

// ---------------- 缓存 ----------------

func (a *API) handleGetCacheInfo(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	info, err := a.Storage.CacheInfo()
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, info)
}

func (a *API) handleClearCache(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	if err := a.Storage.ClearCache(); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleCacheBookOnServer(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	Fail(c, "整书缓存功能实现中")
}

func (a *API) handleCacheBookSSE(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	c.Header("Content-Type", "text/event-stream")
	fmt.Fprint(c.Writer, "data: [DONE]\n\n")
}

func (a *API) handleCancelCacheBook(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	OK(c, nil)
}

func (a *API) handleDeleteBookCache(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	bookURL := paramOf(a.params(c), "bookUrl")
	if bookURL == "" {
		Fail(c, "参数错误")
		return
	}
	if err := a.Storage.DeleteBookCache(bookURL); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func (a *API) handleGetShelfBookWithCacheInfo(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	bookURL := paramOf(a.params(c), "bookUrl")
	if bookURL == "" {
		Fail(c, "参数错误")
		return
	}
	book, err := a.Storage.FindBook(ns, bookURL)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	if book == nil {
		Fail(c, "书籍不存在")
		return
	}
	chapters, _ := a.Storage.ListChapters(bookURL)
	OK(c, map[string]any{
		"book":         book,
		"cacheChapterCount": len(chapters),
	})
}

// ---------------- 用户配置 ----------------

func (a *API) handleGetUserConfig(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	key := paramOf(a.params(c), "key")
	if key == "" {
		key = "default"
	}
	cfg, err := a.Storage.GetUserConfig(ns, key)
	if err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, cfg)
}

func (a *API) handleSaveUserConfig(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	params := a.params(c)
	key := paramOf(params, "key")
	if key == "" {
		key = "default"
	}
	var configVal any = params
	// 兼容 {config:{...}} 形态
	if v, exists := params["config"]; exists {
		configVal = v
	}
	if err := a.Storage.SaveUserConfig(ns, key, configVal); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

// ---------------- 统计 / 系统 ----------------

func (a *API) handleGetReadingStats(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	now := time.Now()
	today := now.Format("2006-01-02")
	weekStart := now.AddDate(0, 0, -7).Format("2006-01-02")

	todayS, _, _ := a.Storage.ReadingStatsByDateRange(ns, today)
	weekS, _, _ := a.Storage.ReadingStatsByDateRange(ns, weekStart)
	totalS, _, _ := a.Storage.ReadingStatsByDateRange(ns, "0000-00-00")
	perBook, _ := a.Storage.ReadingStatsPerBook(ns)

	type statBook struct {
		BookURL string `json:"bookUrl"`
		Name    string `json:"name"`
		Seconds int64  `json:"seconds"`
		Chars   int64  `json:"chars"`
	}
	books := make([]statBook, 0, len(perBook))
	for _, s := range perBook {
		name := s.BookURL
		if b, err := a.Storage.FindBook(ns, s.BookURL); err == nil && b != nil && b.Name != "" {
			name = b.Name
		}
		books = append(books, statBook{BookURL: s.BookURL, Name: name, Seconds: s.Seconds, Chars: s.Chars})
	}
	OK(c, map[string]any{
		"today": todayS,
		"week":  weekS,
		"total": totalS,
		"books": books,
	})
}

func (a *API) handleGetSystemInfo(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	userCount, _ := a.Storage.CountUsers()
	bookCount, _ := a.Storage.CountBooks()
	sourceCount, _ := a.Storage.CountBookSources("default")
	var mem runtime.MemStats
	runtime.ReadMemStats(&mem)
	OK(c, map[string]any{
		"version":         "go-5.0.0",
		"port":            a.Config.Port,
		"userCount":       userCount,
		"bookCount":       bookCount,
		"bookSourceCount": sourceCount,
		"freeMemory":      fmt.Sprintf("%dMB", mem.Alloc/1024/1024),
		"totalMemory":     fmt.Sprintf("%dMB", mem.Sys/1024/1024),
	})
}

func (a *API) handleGetServerStats(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	top := a.Stats.Top(10)
	paths := make([]map[string]any, 0, len(top))
	for _, p := range top {
		paths = append(paths, map[string]any{"path": p.Path, "count": p.Count})
	}
	// 内存：系统（/proc/meminfo）+ 进程（runtime.MemStats）
	sysTotal, sysAvail := systemMemoryMB()
	var ms runtime.MemStats
	runtime.ReadMemStats(&ms)
	used := sysTotal - sysAvail
	if used < 0 {
		used = 0
	}
	memPct := 0.0
	if sysTotal > 0 {
		memPct = used / sysTotal * 100
	}
	// CPU（/proc/stat 两次采样，300ms 间隔）
	cpuPct := cpuUsagePercent()
	// 书源：总数（成功率统计未实现，占位）
	totalSources, _ := a.Storage.CountBookSources(ns)
	OK(c, map[string]any{
		"version":       "GoReader",
		"port":          a.Config.Port,
		"timestamp":     time.Now().UnixMilli(),
		"uptimeSeconds": processUptimeSeconds(),
		"memory": map[string]any{
			"totalMb":     sysTotal,
			"availableMb": sysAvail,
			"usedMb":      used,
			"processMb":   float64(ms.Sys) / 1024 / 1024,
			"percent":     memPct,
		},
		"cpu": map[string]any{
			"percent": cpuPct,
			"cores":   runtime.NumCPU(),
		},
		"requests": map[string]any{
			"total":        a.Stats.Total(),
			"today":        a.Stats.Today(),
			"topEndpoints": paths,
		},
		"online": map[string]any{"sessions": 0},
		"bookSource": map[string]any{
			"ok": 0, "total": totalSources, "failed": 0,
			"successRate": nil, "checkedAt": nil, "note": "暂无统计",
		},
	})
}

// systemMemoryMB 系统内存（/proc/meminfo，Linux）。返回 (总 MB, 可用 MB)。
func systemMemoryMB() (total, avail float64) {
	b, err := os.ReadFile("/proc/meminfo")
	if err != nil {
		return 0, 0
	}
	for _, line := range strings.Split(string(b), "\n") {
		switch {
		case strings.HasPrefix(line, "MemTotal:"):
			total = meminfoKbToMb(line)
		case strings.HasPrefix(line, "MemAvailable:"):
			avail = meminfoKbToMb(line)
		}
	}
	return
}

// meminfoKbToMb 解析 "/proc/meminfo" 行中的 KB 值并转 MB。
func meminfoKbToMb(line string) float64 {
	fields := strings.Fields(line)
	if len(fields) < 2 {
		return 0
	}
	v, err := strconv.ParseFloat(fields[1], 64)
	if err != nil {
		return 0
	}
	return v / 1024
}

// cpuUsagePercent 系统 CPU 使用率（/proc/stat 两次采样，300ms 间隔）。
func cpuUsagePercent() float64 {
	idle1, total1 := readCPUStat()
	if total1 == 0 {
		return 0
	}
	time.Sleep(300 * time.Millisecond)
	idle2, total2 := readCPUStat()
	if total2 <= total1 {
		return 0
	}
	idleDelta := idle2 - idle1
	totalDelta := total2 - total1
	if totalDelta == 0 {
		return 0
	}
	return (1 - float64(idleDelta)/float64(totalDelta)) * 100
}

// readCPUStat 读取 /proc/stat 首行 cpu 汇总（idle, total）。
func readCPUStat() (idle, total uint64) {
	b, err := os.ReadFile("/proc/stat")
	if err != nil {
		return 0, 0
	}
	for _, line := range strings.Split(string(b), "\n") {
		if !strings.HasPrefix(line, "cpu ") {
			continue
		}
		fields := strings.Fields(line)
		for i, f := range fields {
			if i == 0 {
				continue
			}
			v, _ := strconv.ParseUint(f, 10, 64)
			total += v
			if i == 4 { // idle
				idle = v
			}
		}
		return idle, total
	}
	return 0, 0
}

// processUptimeSeconds 进程已运行秒数（/proc/self/stat starttime + /proc/uptime）。
func processUptimeSeconds() int64 {
	b, err := os.ReadFile("/proc/self/stat")
	if err != nil {
		return 0
	}
	// comm 字段可能含空格/括号——从最后一个 ')' 之后取字段
	s := string(b)
	idx := strings.LastIndexByte(s, ')')
	if idx < 0 {
		return 0
	}
	rest := strings.Fields(s[idx+1:])
	// 字段 22 = starttime（rest[19]，因前两个字段 pid/comm 已跳过）
	if len(rest) < 20 {
		return 0
	}
	start, err := strconv.ParseInt(rest[19], 10, 64)
	if err != nil {
		return 0
	}
	hz := int64(100) // CLK_TCK 通常 100
	startSec := start / hz
	// 系统已运行秒数（/proc/uptime）− 进程启动距 boot 秒数 = 进程已运行秒
	ub, err := os.ReadFile("/proc/uptime")
	if err != nil {
		return startSec
	}
	fields := strings.Fields(string(ub))
	if len(fields) == 0 {
		return startSec
	}
	uptime, _ := strconv.ParseFloat(fields[0], 64)
	if s := int64(uptime) - startSec; s >= 0 {
		return s
	}
	return 0
}

// handleBookSourceDebugSSE 书源调试（SSE 占位）。
func (a *API) handleBookSourceDebugSSE(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	c.Header("Content-Type", "text/event-stream")
	fmt.Fprint(c.Writer, "data: [DONE]\n\n")
}

// handleReadSourceFile POST /reader3/readSourceFile。
func (a *API) handleReadSourceFile(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	params := a.params(c)
	path := paramOf(params, "path")
	if path == "" {
		Fail(c, "参数错误")
		return
	}
	// 仅允许读取 storage 目录内文件（防穿越）
	storageDir := a.Config.StorageDir()
	full := filepath.Join(storageDir, path)
	rel, err := filepath.Rel(storageDir, full)
	if err != nil || strings.HasPrefix(rel, "..") {
		Fail(c, "路径非法")
		return
	}
	data, err := os.ReadFile(full)
	if err != nil {
		Fail(c, "文件读取失败")
		return
	}
	OK(c, string(data))
}

// randomUUID 简单 uuid v4（crypto/rand）。
func randomUUID() string {
	b := make([]byte, 16)
	_, _ = crand.Read(b)
	b[6] = (b[6] & 0x0f) | 0x40
	b[8] = (b[8] & 0x3f) | 0x80
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}

// jsonRoundTrip 任意值 JSON 往返（保 camelCase）。
func jsonRoundTrip(v any) map[string]any {
	b, _ := json.Marshal(v)
	var m map[string]any
	_ = json.Unmarshal(b, &m)
	return m
}

// storageNow 兼容引用（避免未使用）。
var _ = storage.NowMillis
