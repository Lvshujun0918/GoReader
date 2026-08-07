package api

import (
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/gin-gonic/gin"
)

// fileRoot 文件管理根目录（storage 目录）。
func (a *API) fileRoot() string {
	return a.Config.StorageDir()
}

// safeFilePath 安全拼接（防路径穿越）。
func (a *API) safeFilePath(rel string) (string, bool) {
	if rel == "" {
		rel = "."
	}
	root := a.fileRoot()
	full := filepath.Join(root, rel)
	absRoot, err1 := filepath.Abs(root)
	absFull, err2 := filepath.Abs(full)
	if err1 != nil || err2 != nil {
		return "", false
	}
	if absFull != absRoot && !strings.HasPrefix(absFull, absRoot+string(filepath.Separator)) {
		return "", false
	}
	return full, true
}

// FileItem 文件项（兼容 legacy camelCase）。
type FileItem struct {
	Name         string `json:"name"`
	Size         int64  `json:"size"`
	Path         string `json:"path"`
	LastModified any    `json:"lastModified"`
	IsDirectory  bool   `json:"isDirectory"`
}

// handleFileList GET /reader3/file/list。
func (a *API) handleFileList(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	params := a.params(c)
	path := paramOf(params, "path")
	full, safe := a.safeFilePath(path)
	if !safe {
		Fail(c, "路径非法")
		return
	}
	entries, err := os.ReadDir(full)
	if err != nil {
		Fail(c, "目录不存在")
		return
	}
	sort.Slice(entries, func(i, j int) bool {
		ei, ej := entries[i], entries[j]
		if ei.IsDir() != ej.IsDir() {
			return ei.IsDir()
		}
		return ei.Name() < ej.Name()
	})
	var items []FileItem
	for _, e := range entries {
		info, err := e.Info()
		if err != nil {
			continue
		}
		rel := filepath.Join(path, e.Name())
		items = append(items, FileItem{
			Name:         e.Name(),
			Size:         info.Size(),
			Path:         rel,
			LastModified: info.ModTime().UnixMilli(),
			IsDirectory:  e.IsDir(),
		})
	}
	OK(c, items)
}

// handleFileGet GET /reader3/file/get：读取文件内容。
func (a *API) handleFileGet(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	path := paramOf(a.params(c), "path")
	full, safe := a.safeFilePath(path)
	if !safe {
		Fail(c, "路径非法")
		return
	}
	data, err := os.ReadFile(full)
	if err != nil {
		Fail(c, "文件不存在")
		return
	}
	OK(c, string(data))
}

// handleFileSave POST /reader3/file/save。
func (a *API) handleFileSave(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	params := a.params(c)
	path := paramOf(params, "path")
	content := paramOf(params, "content")
	full, safe := a.safeFilePath(path)
	if !safe {
		Fail(c, "路径非法")
		return
	}
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		Fail(c, "系统错误")
		return
	}
	if err := os.WriteFile(full, []byte(content), 0o644); err != nil {
		Fail(c, "保存失败")
		return
	}
	OK(c, nil)
}

// handleFileMkdir POST /reader3/file/mkdir。
func (a *API) handleFileMkdir(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	path := paramOf(a.params(c), "path")
	full, safe := a.safeFilePath(path)
	if !safe {
		Fail(c, "路径非法")
		return
	}
	if err := os.MkdirAll(full, 0o755); err != nil {
		Fail(c, "创建失败")
		return
	}
	OK(c, nil)
}

// handleFileDownload GET /reader3/file/download。
func (a *API) handleFileDownload(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	path := paramOf(a.params(c), "path")
	full, safe := a.safeFilePath(path)
	if !safe {
		Fail(c, "路径非法")
		return
	}
	c.FileAttachment(full, filepath.Base(full))
}

// handleFileUpload POST /reader3/file/upload。
func (a *API) handleFileUpload(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	path := paramOf(a.params(c), "path")
	dir, safe := a.safeFilePath(path)
	if !safe {
		Fail(c, "路径非法")
		return
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		Fail(c, "系统错误")
		return
	}
	file, err := c.FormFile("file")
	if err != nil {
		Fail(c, "未收到文件")
		return
	}
	// 上传大小上限
	if file.Size > a.Config.UploadMaxBytes() {
		Fail(c, "文件过大：超过上传大小上限")
		return
	}
	dst := filepath.Join(dir, filepath.Base(file.Filename))
	if err := c.SaveUploadedFile(file, dst); err != nil {
		Fail(c, "保存失败")
		return
	}
	OK(c, nil)
}

// handleFileDelete POST /reader3/file/delete。
func (a *API) handleFileDelete(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	path := paramOf(a.params(c), "path")
	full, safe := a.safeFilePath(path)
	if !safe {
		Fail(c, "路径非法")
		return
	}
	if err := os.RemoveAll(full); err != nil {
		Fail(c, "删除失败")
		return
	}
	OK(c, nil)
}

// handleFileDeleteMulti POST /reader3/file/deleteMulti。
func (a *API) handleFileDeleteMulti(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	paths := stringArrayParam(a.params(c), "paths")
	for _, p := range paths {
		full, safe := a.safeFilePath(p)
		if !safe {
			continue
		}
		_ = os.RemoveAll(full)
	}
	OK(c, nil)
}
