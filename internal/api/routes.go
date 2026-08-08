package api

import (
	"net/http"
	"os"
	"path"
	"path/filepath"
	"strings"

	"github.com/gin-gonic/gin"
)

// registerReader3 注册 /reader3/* 全部 API 路由（兼容 legacy，约 125 个接口）。
func (a *API) registerReader3(r *gin.Engine) {
	g := r.Group("/reader3")

	// ---- 账号/登录/用户管理 ----
	g.POST("/login", a.handleLogin)
	g.POST("/logout", a.handleLogout)
	g.Any("/getUsers", a.handleGetUsers)
	g.Any("/getUserList", a.handleGetUsers)
	g.POST("/updateUser", a.handleUpdateUser)
	g.POST("/deleteUser", a.handleDeleteUser)
	g.POST("/deleteUsers", a.handleDeleteUsers)
	g.POST("/resetUserPassword", a.handleResetUserPassword)
	g.POST("/resetPassword", a.handleResetUserPassword)
	g.POST("/clearInactiveUsers", a.handleClearInactiveUsers)

	// ---- 书架/书籍 ----
	g.GET("/getBookshelf", a.handleGetBookshelf)
	g.Any("/getShelfBook", a.handleGetShelfBook)
	g.POST("/saveBook", a.handleSaveBook)
	g.POST("/deleteBook", a.handleDeleteBook)
	g.POST("/deleteBooks", a.handleDeleteBooks)
	g.POST("/saveBookProgress", a.handleSaveBookProgress)
	g.POST("/saveBookContent", a.handleSaveBookContent)
	g.Any("/getBookInfo", a.handleGetBookInfo)
	g.Any("/getBookToc", a.handleGetBookToc)
	g.Any("/getChapterList", a.handleGetBookToc)
	g.Any("/getBookContent", a.handleGetBookContent)
	g.Any("/getChapterListByRule", a.handleGetChapterListByRule)
	g.POST("/migrateLocBook", a.handleMigrateLocBook)
	g.POST("/refreshLocalBook", a.handleRefreshLocalBook)
	g.POST("/importBookPreview", a.handleImportBookPreview)
	g.POST("/uploadLocalBook", a.handleUploadLocalBook)

	// ---- 书源 ----
	g.Any("/getBookSources", a.handleGetBookSources)
	g.Any("/getBookSource", a.handleGetBookSource)
	g.POST("/saveBookSource", a.handleSaveBookSource)
	g.POST("/saveBookSources", a.handleSaveBookSources)
	g.POST("/deleteBookSource", a.handleDeleteBookSource)
	g.POST("/deleteBookSources", a.handleDeleteBookSources)
	g.POST("/deleteAllBookSources", a.handleDeleteAllBookSources)
	g.POST("/saveFromRemoteSource", a.handleSaveFromRemoteSource)
	g.Any("/getAvailableBookSource", a.handleGetAvailableBookSource)
	g.Any("/getInvalidBookSources", a.handleGetInvalidBookSources)
	g.POST("/disableInvalidBookSources", a.handleDisableInvalidBookSources)
	g.POST("/setAsDefaultBookSources", a.handleSetAsDefaultBookSources)
	g.POST("/deleteUserBookSource", a.handleDeleteUserBookSource)
	g.GET("/exportBookSources", a.handleExportBookSources)

	// ---- 搜索 ----
	g.Any("/searchBook", a.handleSearchBook)
	g.Any("/searchBookMulti", a.handleSearchBookMulti)
	g.Any("/searchBookMultiSSE", a.handleSearchBookMultiSSE)
	g.Any("/searchBookSource", a.handleSearchBookSource)
	g.Any("/searchBookSourceSSE", a.handleSearchBookSourceSSE)
	g.Any("/searchBookContent", a.handleSearchBookContent)

	// ---- 书源登录/验证码 ----
	g.Any("/loginBookSource", a.handleLoginBookSource)
	g.POST("/setBookSourceCookie", a.handleSetBookSourceCookie)
	g.POST("/getCaptcha", a.handleGetCaptcha)
	g.POST("/submitCaptcha", a.handleSubmitCaptcha)

	// ---- 探索/订阅 ----
	g.Any("/getSourceSubs", a.handleGetSourceSubs)
	g.POST("/saveSourceSub", a.handleSaveSourceSub)
	g.POST("/deleteSourceSub", a.handleDeleteSourceSub)
	g.POST("/refreshSourceSub", a.handleRefreshSourceSub)

	// ---- 书签/分组 ----
	g.POST("/saveBookmark", a.handleSaveBookmark)
	g.POST("/saveBookmarks", a.handleSaveBookmarks)
	g.Any("/getBookmarks", a.handleGetBookmarks)
	g.POST("/deleteBookmark", a.handleDeleteBookmark)
	g.POST("/deleteBookmarks", a.handleDeleteBookmarks)
	g.Any("/getBookGroups", a.handleGetBookGroups)
	g.Any("/getBookGroupList", a.handleGetBookGroups)
	g.POST("/saveBookGroup", a.handleSaveBookGroup)
	g.POST("/saveBookGroupName", a.handleSaveBookGroup)
	g.POST("/updateBookGroup", a.handleSaveBookGroup)
	g.POST("/updateBookGroupId", a.handleUpdateBookGroupID)
	g.POST("/saveBookGroupId", a.handleUpdateBookGroupID)
	g.POST("/deleteBookGroup", a.handleDeleteBookGroup)
	g.POST("/addBookGroupMulti", a.handleAddBookGroupMulti)
	g.POST("/removeBookGroupMulti", a.handleRemoveBookGroupMulti)
	g.POST("/saveBookGroupOrder", a.handleSaveBookGroupOrder)

	// ---- 替换规则 / TXT 目录规则 / HttpTTS ----
	g.Any("/getReplaceRules", a.handleGetReplaceRules)
	g.POST("/saveReplaceRule", a.handleSaveReplaceRule)
	g.POST("/saveReplaceRules", a.handleSaveReplaceRules)
	g.POST("/replaceRule/saveMulti", a.handleSaveReplaceRules)
	g.POST("/deleteReplaceRule", a.handleDeleteReplaceRule)
	g.POST("/deleteReplaceRules", a.handleDeleteReplaceRules)
	g.Any("/getTxtTocRules", a.handleGetTxtTocRules)
	g.POST("/saveTxtTocRule", a.handleSaveTxtTocRule)
	g.POST("/deleteTxtTocRule", a.handleDeleteTxtTocRule)
	g.POST("/importDefaultTxtTocRules", a.handleImportDefaultTxtTocRules)
	g.Any("/getHttpTTSList", a.handleGetHttpTTSList)
	g.POST("/saveHttpTTS", a.handleSaveHttpTTS)
	g.POST("/httpTTS/saveMulti", a.handleSaveHttpTTSMulti)
	g.POST("/deleteHttpTTS", a.handleDeleteHttpTTS)

	// ---- TTS / 导出 / 备份恢复 ----
	g.Any("/getTTSVoices", a.handleGetTTSVoices)
	g.Any("/tts", a.handleTTS)
	g.Any("/httpTTS", a.handleTTS)
	g.GET("/exportBook", a.handleExportBook)
	g.POST("/backupToWebdav", a.handleBackupToWebdav)
	g.POST("/restoreFromZip", a.handleRestoreFromZip)
	g.POST("/restoreFromWebdav", a.handleRestoreFromWebdav)
	g.POST("/backupToMongodb", a.handleBackupToMongodb)
	g.POST("/restoreFromMongodb", a.handleRestoreFromMongodb)

	// ---- 缓存/用户配置/系统 ----
	g.Any("/getCacheInfo", a.handleGetCacheInfo)
	g.POST("/clearCache", a.handleClearCache)
	g.POST("/cacheBookOnServer", a.handleCacheBookOnServer)
	g.Any("/cacheBookSSE", a.handleCacheBookSSE)
	g.Any("/cancelCacheBook", a.handleCancelCacheBook)
	g.Any("/deleteBookCache", a.handleDeleteBookCache)
	g.Any("/getShelfBookWithCacheInfo", a.handleGetShelfBookWithCacheInfo)
	g.Any("/getUserConfig", a.handleGetUserConfig)
	g.Any("/saveUserConfig", a.handleSaveUserConfig)
	g.Any("/bookSourceDebugSSE", a.handleBookSourceDebugSSE)
	g.Any("/getReadingStats", a.handleGetReadingStats)
	g.GET("/getSystemInfo", a.handleGetSystemInfo)
	g.GET("/getServerStats", a.handleGetServerStats)
	g.POST("/readSourceFile", a.handleReadSourceFile)

	// ---- 书封/背景图上传、备份下载（保留 file/mkdir + file/upload + file/download） ----
	g.POST("/file/mkdir", a.handleFileMkdir)
	g.POST("/file/upload", a.handleFileUpload)
	g.GET("/file/download", a.handleFileDownload)

	// 未匹配路由：/reader3 → JSON 404；静态资源（assets/simple/web-ui SPA）
	r.NoRoute(func(c *gin.Context) {
		p := c.Request.URL.Path
		switch {
		case strings.HasPrefix(p, "/reader3"):
			if p == "/reader3/webdav" || p == "/reader3/webdav/" {
				a.handleWebDAV(c)
				return
			}
			Fail(c, "接口不存在")
		case strings.HasPrefix(p, "/assets/"):
			a.serveDirFile(c, a.Config.AssetsDir(), strings.TrimPrefix(p, "/assets/"), false)
		case strings.HasPrefix(p, "/simple"):
			a.serveDirFile(c, a.Config.SimpleWebRoot, strings.TrimPrefix(p, "/simple/"), false)
		default:
			// 前端 SPA（web-ui/dist，fallback index.html）
			a.serveSPA(c, p)
		}
	})
}

// serveDirFile 目录静态文件（路径穿越防护 + 目录请求 index.html）。
func (a *API) serveDirFile(c *gin.Context, root, rel string, dirIndex bool) {
	clean := path.Clean("/" + rel)
	clean = strings.TrimPrefix(clean, "/")
	full := filepath.Join(root, clean)
	absRoot, _ := filepath.Abs(root)
	absFull, _ := filepath.Abs(full)
	if absFull != absRoot && !strings.HasPrefix(absFull, absRoot+string(filepath.Separator)) {
		c.String(http.StatusForbidden, "forbidden")
		return
	}
	if info, err := os.Stat(full); err == nil {
		if !info.IsDir() {
			c.File(full)
			return
		}
		if dirIndex {
			index := filepath.Join(full, "index.html")
			if _, err := os.Stat(index); err == nil {
				c.File(index)
				return
			}
		}
	}
	c.String(http.StatusNotFound, "not found")
}

// serveSPA 前端 SPA 静态服务（fallback index.html）。
func (a *API) serveSPA(c *gin.Context, p string) {
	webRoot := a.Config.WebRoot
	if a.serveDirFileOK(c, webRoot, p) {
		return
	}
	index := filepath.Join(webRoot, "index.html")
	if _, err := os.Stat(index); err == nil {
		c.File(index)
		return
	}
	c.String(http.StatusNotFound, "not found")
}

// serveDirFileOK 尝试服务文件（成功返回 true）。
func (a *API) serveDirFileOK(c *gin.Context, root, rel string) bool {
	clean := path.Clean("/" + rel)
	clean = strings.TrimPrefix(clean, "/")
	full := filepath.Join(root, clean)
	absRoot, _ := filepath.Abs(root)
	absFull, _ := filepath.Abs(full)
	if absFull != absRoot && !strings.HasPrefix(absFull, absRoot+string(filepath.Separator)) {
		return false
	}
	if info, err := os.Stat(full); err == nil && !info.IsDir() {
		c.File(full)
		return true
	}
	return false
}
