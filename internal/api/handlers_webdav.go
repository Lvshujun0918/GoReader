package api

import (
	"encoding/xml"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/gin-gonic/gin"

	"github.com/Lvshujun0918/GoReader/internal/model"
)

// fileRoot WebDAV/本地文件根目录（storage 目录）。
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

const (
	davNS  = "DAV:"
)

// handleWebDAV /reader3/webdav*：WebDAV 服务（基于 storage 目录）。
// 认证：Basic 认证（username = 用户命名空间）；非 secure 模式默认 default。
func (a *API) handleWebDAV(c *gin.Context) {
	// 认证
	ns, ok := a.webdavAuth(c)
	if !ok {
		c.Header("WWW-Authenticate", `Basic realm="reader3"`)
		c.Status(http.StatusUnauthorized)
		return
	}

	// 路径
	relPath := strings.TrimPrefix(c.Request.URL.Path, "/reader3/webdav")
	relPath = strings.TrimPrefix(relPath, "/")
	full, safe := a.safeFilePath(relPath)
	if !safe {
		c.Status(http.StatusForbidden)
		return
	}

	switch c.Request.Method {
	case "OPTIONS":
		a.webdavOptions(c)
	case "PROPFIND":
		a.webdavPropfind(c, ns, relPath, full)
	case "GET", "HEAD":
		a.webdavGet(c, full)
	case "PUT":
		a.webdavPut(c, full)
	case "MKCOL":
		a.webdavMkcol(c, full)
	case "DELETE":
		a.webdavDelete(c, full)
	case "MOVE":
		a.webdavMove(c, relPath)
	case "COPY":
		a.webdavCopy(c, relPath)
	case "LOCK", "UNLOCK":
		// 简化：不支持锁，返回 200（客户端兼容）
		c.Status(http.StatusOK)
	default:
		c.Status(http.StatusMethodNotAllowed)
	}
}

// webdavAuth WebDAV 认证。
func (a *API) webdavAuth(c *gin.Context) (string, bool) {
	if !a.Config.Secure {
		return "default", true
	}
	user, pass, ok := c.Request.BasicAuth()
	if !ok || user == "" || pass == "" {
		return "", false
	}
	u, err := a.Storage.FindUser(user)
	if err != nil || u == nil {
		return "", false
	}
	if !u.EnableWebdav {
		return "", false
	}
	if !a.verifyPassword(u, pass) {
		return "", false
	}
	return user, true
}

func (a *API) webdavOptions(c *gin.Context) {
	c.Header("Allow", "OPTIONS, GET, HEAD, PUT, MKCOL, DELETE, MOVE, COPY, PROPFIND")
	c.Header("DAV", "1, 2")
	c.Header("Access-Control-Allow-Origin", "*")
	c.Status(http.StatusOK)
}

type davPropfind struct {
	XMLName xml.Name `xml:"D:propfind"`
	NS      string   `xml:"xmlns:D,attr"`
	Prop    struct {
		XMLName xml.Name `xml:"D:prop"`
	} `xml:"D:prop"`
}

type davResponse struct {
	XMLName xml.Name  `xml:"D:response"`
	Href    string    `xml:"D:href"`
	Propstat davPropstat `xml:"D:propstat"`
}

type davPropstat struct {
	Prop davProp `xml:"D:prop"`
	Status string `xml:"D:status"`
}

type davProp struct {
	Resourcetype davResourcetype `xml:"D:resourcetype"`
	Getcontentlength string       `xml:"D:getcontentlength"`
	Getlastmodified  string       `xml:"D:getlastmodified"`
	Getcontenttype   string       `xml:"D:getcontenttype"`
}

type davResourcetype struct {
	Collection *struct{} `xml:"D:collection"`
}

type davMultistatus struct {
	XMLName xml.Name      `xml:"D:multistatus"`
	NS      string        `xml:"xmlns:D,attr"`
	Responses []davResponse `xml:"D:response"`
}

// webdavPropfind 属性查询。
func (a *API) webdavPropfind(c *gin.Context, ns, relPath, full string) {
	depth := c.Request.Header.Get("Depth")
	if depth == "" {
		depth = "infinity"
	}
	// 读取请求体（兼容部分客户端发送 propfind body）
	_, _ = io.Copy(io.Discard, c.Request.Body)

	var paths []string
	info, err := os.Stat(full)
	if err != nil {
		c.Status(http.StatusNotFound)
		return
	}
	paths = append(paths, relPath)
	if info.IsDir() && depth != "0" {
		entries, err := os.ReadDir(full)
		if err == nil {
			for _, e := range entries {
				paths = append(paths, filepath.Join(relPath, e.Name()))
			}
		}
	}

	ms := davMultistatus{NS: davNS}
	for _, p := range paths {
		fp, _ := a.safeFilePath(p)
		fi, err := os.Stat(fp)
		if err != nil {
			continue
		}
		resp := davResponse{
			Href: "/reader3/webdav/" + p,
			Propstat: davPropstat{
				Status: "HTTP/1.1 200 OK",
				Prop: davProp{
					Getcontentlength: fmt.Sprintf("%d", fi.Size()),
					Getlastmodified:  fi.ModTime().UTC().Format(http.TimeFormat),
				},
			},
		}
		if fi.IsDir() {
			resp.Propstat.Prop.Resourcetype.Collection = &struct{}{}
			resp.Propstat.Prop.Getcontenttype = "httpd/unix-directory"
		} else {
			resp.Propstat.Prop.Getcontenttype = mimeType(p)
		}
		ms.Responses = append(ms.Responses, resp)
	}
	c.Header("Content-Type", "application/xml; charset=utf-8")
	c.XML(http.StatusMultiStatus, ms)
}

func (a *API) webdavGet(c *gin.Context, full string) {
	info, err := os.Stat(full)
	if err != nil || info.IsDir() {
		c.Status(http.StatusNotFound)
		return
	}
	c.Header("Content-Type", mimeType(full))
	http.ServeFile(c.Writer, c.Request, full)
}

func (a *API) webdavPut(c *gin.Context, full string) {
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		c.Status(http.StatusInternalServerError)
		return
	}
	body, err := io.ReadAll(io.LimitReader(c.Request.Body, a.Config.UploadMaxBytes()+1))
	if err != nil {
		c.Status(http.StatusBadRequest)
		return
	}
	if int64(len(body)) > a.Config.UploadMaxBytes() {
		c.Status(http.StatusRequestEntityTooLarge)
		return
	}
	if err := os.WriteFile(full, body, 0o644); err != nil {
		c.Status(http.StatusInternalServerError)
		return
	}
	c.Status(http.StatusCreated)
}

func (a *API) webdavMkcol(c *gin.Context, full string) {
	if err := os.MkdirAll(full, 0o755); err != nil {
		c.Status(http.StatusConflict)
		return
	}
	c.Status(http.StatusCreated)
}

func (a *API) webdavDelete(c *gin.Context, full string) {
	if err := os.RemoveAll(full); err != nil {
		c.Status(http.StatusConflict)
		return
	}
	c.Status(http.StatusNoContent)
}

// webdavMove/COPY 目标路径来自 Destination header。
func (a *API) webdavMove(c *gin.Context, relPath string) {
	a.webdavCopyInternal(c, relPath, true)
}

func (a *API) webdavCopy(c *gin.Context, relPath string) {
	a.webdavCopyInternal(c, relPath, false)
}

func (a *API) webdavCopyInternal(c *gin.Context, relPath string, move bool) {
	dest := c.GetHeader("Destination")
	if dest == "" {
		c.Status(http.StatusBadRequest)
		return
	}
	// 提取目标相对路径
	idx := strings.Index(dest, "/reader3/webdav/")
	destRel := ""
	if idx >= 0 {
		destRel = dest[idx+len("/reader3/webdav/"):]
	} else {
		if u := strings.Index(dest, "://"); u >= 0 {
			rest := dest[u+3:]
			if slash := strings.Index(rest, "/"); slash >= 0 {
				destRel = rest[slash+1:]
			}
		}
	}
	srcFull, ok1 := a.safeFilePath(relPath)
	dstFull, ok2 := a.safeFilePath(destRel)
	if !ok1 || !ok2 {
		c.Status(http.StatusForbidden)
		return
	}
	var err error
	if move {
		err = os.Rename(srcFull, dstFull)
		if err != nil {
			// 跨目录 rename 失败则复制+删除
			err = copyTree(srcFull, dstFull)
			if err == nil {
				err = os.RemoveAll(srcFull)
			}
		}
		c.Status(http.StatusCreated)
	} else {
		err = copyTree(srcFull, dstFull)
		c.Status(http.StatusCreated)
	}
	if err != nil {
		c.Status(http.StatusConflict)
	}
}

// copyTree 递归复制。
func copyTree(src, dst string) error {
	info, err := os.Stat(src)
	if err != nil {
		return err
	}
	if !info.IsDir() {
		data, err := os.ReadFile(src)
		if err != nil {
			return err
		}
		if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
			return err
		}
		return os.WriteFile(dst, data, info.Mode())
	}
	if err := os.MkdirAll(dst, 0o755); err != nil {
		return err
	}
	entries, err := os.ReadDir(src)
	if err != nil {
		return err
	}
	for _, e := range entries {
		if err := copyTree(filepath.Join(src, e.Name()), filepath.Join(dst, e.Name())); err != nil {
			return err
		}
	}
	return nil
}

func mimeType(name string) string {
	switch strings.ToLower(filepath.Ext(name)) {
	case ".txt", ".md":
		return "text/plain"
	case ".html", ".htm":
		return "text/html"
	case ".json":
		return "application/json"
	case ".xml":
		return "application/xml"
	case ".jpg", ".jpeg":
		return "image/jpeg"
	case ".png":
		return "image/png"
	case ".gif":
		return "image/gif"
	case ".webp":
		return "image/webp"
	case ".epub":
		return "application/epub+zip"
	case ".zip":
		return "application/zip"
	case ".pdf":
		return "application/pdf"
	default:
		return "application/octet-stream"
	}
}

// 兼容引用（避免未使用 model 包告警由 gofmt 处理）。
var _ = model.User{}
var _ = time.Now
