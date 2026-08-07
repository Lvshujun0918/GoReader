package api

import (
	"encoding/xml"
	"net/http"
	"strings"
	"time"

	"github.com/gin-gonic/gin"
)

// OPDS 命名空间（Atom 1.0 + OPDS 1.2）。
const (
	opdsNS   = "http://www.w3.org/2005/Atom"
	opdsNS2  = "http://opds-spec.org/2010/catalog"
	opdsNS3  = "http://opds-spec.org/2010/acquisition"
)

type opdsEntry struct {
	XMLName  xml.Name   `xml:"entry"`
	ID       string     `xml:"id"`
	Title    string     `xml:"title"`
	Updated  string     `xml:"updated"`
	Content  string     `xml:"content"`
	Author   *opdsName  `xml:"author,omitempty"`
	Link     []opdsLink `xml:"link"`
}

type opdsName struct {
	Name string `xml:"name"`
}

type opdsLink struct {
	Href  string `xml:"href,attr"`
	Type  string `xml:"type,attr"`
	Rel   string `xml:"rel,attr,omitempty"`
	Title string `xml:"title,attr,omitempty"`
}

type opdsFeed struct {
	XMLName xml.Name    `xml:"feed"`
	NS      string      `xml:"xmlns,attr"`
	NS2     string      `xml:"xmlns:opds,attr"`
	ID      string      `xml:"id"`
	Title   string      `xml:"title"`
	Updated string      `xml:"updated"`
	Link    []opdsLink  `xml:"link"`
	Entry   []opdsEntry `xml:"entry"`
}

// opdsUser 从请求解析 OPDS 用户（Basic 认证或 query）。
func (a *API) opdsUser(c *gin.Context) string {
	// Basic 认证
	if user, _, ok := c.Request.BasicAuth(); ok && user != "" {
		return user
	}
	// query ns
	if ns := c.Query("ns"); ns != "" {
		return ns
	}
	return "default"
}

// handleOpds GET /opds*：OPDS 1.2/2.0 分派。
func (a *API) handleOpds(c *gin.Context) {
	ns := a.opdsUser(c)
	path := c.Param("rest")
	// /opds/{namespace} → 用户书架 feed
	trimmed := strings.Trim(path, "/")
	if trimmed != "" && !strings.Contains(trimmed, "/") {
		// 可能是命名空间
		if trimmed != "catalog" && trimmed != "search" {
			ns = trimmed
		}
	}

	books, err := a.Storage.ListBooks(ns)
	if err != nil {
		c.String(http.StatusInternalServerError, "error")
		return
	}

	feed := opdsFeed{
		NS:      opdsNS,
		NS2:     opdsNS2,
		ID:      "urn:uuid:reader3",
		Title:   "阅读书架 - " + ns,
		Updated: nowRFC3339(),
		Link: []opdsLink{
			{Href: "/opds", Type: opdsNS, Rel: "self"},
			{Href: "/opds?ns=" + ns, Type: opdsNS, Rel: "start"},
		},
	}
	for _, b := range books {
		entry := opdsEntry{
			ID:      "urn:reader:" + b.BookURL,
			Title:   b.Name,
			Updated: nowRFC3339(),
			Content: b.Intro,
			Link: []opdsLink{
				{Href: "/opds-save?url=" + b.BookURL + "&ns=" + ns, Type: "text/html", Rel: "http://opds-spec.org/acquisition"},
			},
		}
		if b.Author != "" {
			entry.Author = &opdsName{Name: b.Author}
		}
		feed.Entry = append(feed.Entry, entry)
	}
	c.Header("Content-Type", "application/atom+xml; charset=utf-8")
	c.XML(http.StatusOK, feed)
}

// handleOpdsSave GET+POST /opds-save：OPDS-PSE 进度保存（占位实现）。
func (a *API) handleOpdsSave(c *gin.Context) {
	ns := a.opdsUser(c)
	url := c.Query("url")
	chapter := c.Query("chapter")
	position := c.Query("position")
	if url == "" {
		Fail(c, "参数错误")
		return
	}
	_ = chapter
	_ = position
	if err := a.Storage.UpdateBookProgress(ns, url, chapter, 0, 0, 0); err != nil {
		Fail(c, "系统错误")
		return
	}
	OK(c, nil)
}

func nowRFC3339() string {
	return time.Now().UTC().Format("2006-01-02T15:04:05Z")
}
