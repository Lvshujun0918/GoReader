package api

import (
	"os"
	"path/filepath"

	"github.com/gin-gonic/gin"
)

// handleFileMkdir POST /reader3/file/mkdir：创建目录（书封/背景图上传用）。
func (a *API) handleFileMkdir(c *gin.Context) {
	ns, ok := a.ResolveNamespace(c)
	if !ok {
		NeedLogin(c)
		return
	}
	_ = ns
	params := a.params(c)
	path := paramOf(params, "path")
	if name := paramOf(params, "name"); name != "" {
		path = filepath.Join(path, name)
	}
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

// handleFileDownload GET /reader3/file/download：下载文件（备份 zip 下载用）。
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

// handleFileUpload POST /reader3/file/upload：multipart 上传（书封/背景图用）。
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
