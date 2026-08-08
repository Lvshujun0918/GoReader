// Package model 数据模型（gorm ↔ SQLite，camelCase JSON + snake_case 列，兼容 legacy）。
package model

import "time"

// TableName 统一返回表名（gorm 默认复数表名不适用，全部显式指定）。

// User 用户（主键 username）。
type User struct {
	Username             string `gorm:"column:username;primaryKey" json:"username"`
	Password             string `gorm:"column:password;not null" json:"-"`
	Salt                 string `gorm:"column:salt;not null" json:"-"`
	Token                string `gorm:"column:token;default:''" json:"token"`
	TokenMap             string `gorm:"column:token_map" json:"-"`
	EnableWebdav     bool   `gorm:"column:enable_webdav;default:0" json:"enableWebdav"`
	EnableLocalStore bool   `gorm:"column:enable_local_store;default:0" json:"enableLocalStore"`
	BookLimit        int64  `gorm:"column:book_limit;default:0" json:"bookLimit"`
	LastLoginAt          int64  `gorm:"column:last_login_at;default:0" json:"lastLoginAt"`
	CreatedAt            int64  `gorm:"column:created_at;default:0" json:"createdAt"`
	UserNamespace        string `gorm:"column:user_namespace;default:''" json:"-"`
	RawJSON              string `gorm:"column:raw_json" json:"-"`
}

func (User) TableName() string { return "users" }

// Book 书架书籍（复合主键 book_url + user_namespace）。
type Book struct {
	BookURL            string `gorm:"column:book_url;primaryKey" json:"bookUrl"`
	Name               string `gorm:"column:name;default:''" json:"name"`
	Author             string `gorm:"column:author;default:''" json:"author"`
	Origin             string `gorm:"column:origin;default:''" json:"origin"`
	OriginName         string `gorm:"column:origin_name;default:''" json:"originName"`
	TocURL             string `gorm:"column:toc_url;default:''" json:"tocUrl"`
	Kind               string `gorm:"column:kind" json:"kind"`
	CustomTag          string `gorm:"column:custom_tag" json:"customTag"`
	CoverURL           string `gorm:"column:cover_url" json:"coverUrl"`
	CustomCoverURL     string `gorm:"column:custom_cover_url" json:"customCoverUrl"`
	Intro              string `gorm:"column:intro" json:"intro"`
	CustomIntro        string `gorm:"column:custom_intro" json:"customIntro"`
	Charset            string `gorm:"column:charset" json:"charset"`
	Type               int    `gorm:"column:type;default:0" json:"type"`
	GroupName          int64  `gorm:"column:group_name;default:0" json:"group"`
	LatestChapterTitle string `gorm:"column:latest_chapter_title" json:"latestChapterTitle"`
	LatestChapterTime  int64  `gorm:"column:latest_chapter_time;default:0" json:"latestChapterTime"`
	LastCheckTime      int64  `gorm:"column:last_check_time;default:0" json:"lastCheckTime"`
	LastCheckCount     int64  `gorm:"column:last_check_count;default:0" json:"lastCheckCount"`
	TotalChapterNum    int64  `gorm:"column:total_chapter_num;default:0" json:"totalChapterNum"`
	DurChapterTitle    string `gorm:"column:dur_chapter_title" json:"durChapterTitle"`
	DurChapterIndex    int64  `gorm:"column:dur_chapter_index;default:0" json:"durChapterIndex"`
	DurChapterPos      int64  `gorm:"column:dur_chapter_pos;default:0" json:"durChapterPos"`
	DurChapterTime     int64  `gorm:"column:dur_chapter_time;default:0" json:"durChapterTime"`
	WordCount          string `gorm:"column:word_count" json:"wordCount"`
	CanUpdate          int    `gorm:"column:can_update;default:1" json:"canUpdate"`
	OrderNum           int64  `gorm:"column:order_num;default:0" json:"orderNum"`
	OriginOrder        int64  `gorm:"column:origin_order;default:0" json:"originOrder"`
	UseReplaceRule     int    `gorm:"column:use_replace_rule;default:1" json:"useReplaceRule"`
	Variable           string `gorm:"column:variable" json:"variable"`
	ReadConfig         string `gorm:"column:read_config" json:"readConfig"`
	IsInShelf          int    `gorm:"column:is_in_shelf;default:1" json:"isInShelf"`
	Cbz                int    `gorm:"column:cbz;default:0" json:"cbz"`
	DisplayCover       string `gorm:"column:display_cover" json:"displayCover"`
	DisplayIntro       string `gorm:"column:display_intro" json:"displayIntro"`
	LocalEpub          int    `gorm:"column:local_epub;default:0" json:"localEpub"`
	LocalPDF           int    `gorm:"column:local_pdf;default:0" json:"localPdf"`
	PDF                int    `gorm:"column:pdf;default:0" json:"pdf"`
	SplitLongChapter   int    `gorm:"column:split_long_chapter;default:0" json:"splitLongChapter"`
	LastCheckError     string `gorm:"column:last_check_error" json:"lastCheckError"`
	InfoHTML           string `gorm:"column:info_html" json:"infoHtml"`
	TocHTML            string `gorm:"column:toc_html" json:"tocHtml"`
	Language           string `gorm:"column:language" json:"language"`
	Publisher          string `gorm:"column:publisher" json:"publisher"`
	PublishedAt        string `gorm:"column:published_at" json:"publishedAt"`
	UserNamespace      string `gorm:"column:user_namespace;primaryKey;default:''" json:"-"`
	CreatedAt          int64  `gorm:"column:created_at;default:0" json:"createdAt"`
	RawJSON            string `gorm:"column:raw_json" json:"-"`
	// 本地书双轨同步（GAP 170）
	LocalFile       string `gorm:"column:local_file" json:"localFile"`
	LocalFileMtime  int64  `gorm:"column:local_file_mtime;default:0" json:"localFileMtime"`
	LocalFileSize   int64  `gorm:"column:local_file_size;default:0" json:"localFileSize"`
	LocalFileDeleted int   `gorm:"column:local_file_deleted;default:0" json:"localFileDeleted"`
}

func (Book) TableName() string { return "books" }

// BookChapter 章节缓存（复合主键 book_url + chapter_index）。
type BookChapter struct {
	BookURL      string `gorm:"column:book_url;primaryKey;not null" json:"-"`
	ChapterIndex int64  `gorm:"column:chapter_index;primaryKey;not null" json:"-"`
	Title        string `gorm:"column:title;default:''" json:"title"`
	Content      string `gorm:"column:content" json:"content"`
}

func (BookChapter) TableName() string { return "book_chapters" }

// TocCache 目录缓存（主键 book_url，TTL 5 分钟）。
type TocCache struct {
	BookURL     string `gorm:"column:book_url;primaryKey" json:"-"`
	TocURL      string `gorm:"column:toc_url;default:''" json:"tocUrl"`
	ChaptersJSON string `gorm:"column:chapters_json" json:"-"`
	UpdatedAt   int64  `gorm:"column:updated_at;default:0" json:"-"`
}

func (TocCache) TableName() string { return "toc_cache" }

// Bookmark 书签（复合主键 book_url + title）。
type Bookmark struct {
	BookURL        string `gorm:"column:book_url;primaryKey;not null" json:"bookUrl"`
	Title          string `gorm:"column:title;primaryKey;not null;default:''" json:"title"`
	ParagraphIndex int64  `gorm:"column:paragraph_index;default:0" json:"paragraphIndex"`
	ChapterIndex   int64  `gorm:"column:chapter_index;default:0" json:"chapterIndex"`
	CreatedAt      int64  `gorm:"column:created_at;default:0" json:"createdAt"`
	UserNamespace  string `gorm:"column:user_namespace;default:''" json:"-"`
	RawJSON        string `gorm:"column:raw_json" json:"-"`
}

func (Bookmark) TableName() string { return "bookmarks" }

// BookGroup 书架分组（主键 id AUTOINCREMENT）。
type BookGroup struct {
	ID            int64  `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	Name          string `gorm:"column:name;not null;default:''" json:"name"`
	OrderNum      int64  `gorm:"column:order_num;default:0" json:"orderNum"`
	UserNamespace string `gorm:"column:user_namespace;default:''" json:"-"`
}

func (BookGroup) TableName() string { return "book_groups" }

// ReplaceRule 替换规则（主键 id 前端生成字符串）。
type ReplaceRule struct {
	ID            string `gorm:"column:id;primaryKey" json:"id"`
	Name          string `gorm:"column:name" json:"name"`
	Find          string `gorm:"column:find" json:"find"`
	Replace       string `gorm:"column:replace" json:"replace"`
	Enabled       int    `gorm:"column:enable;default:1" json:"enabled"`
	OrderNum      int64  `gorm:"column:order_num;default:0" json:"order"`
	UserNamespace string `gorm:"column:user_namespace;default:''" json:"-"`
}

func (ReplaceRule) TableName() string { return "replace_rules" }

// TxtTocRule TXT 目录规则（主键 id）。
type TxtTocRule struct {
	ID            string `gorm:"column:id;primaryKey" json:"id"`
	Name          string `gorm:"column:name" json:"name"`
	Rule          string `gorm:"column:rule" json:"rule"`
	Enabled       int    `gorm:"column:enable;default:1" json:"enable"`
	SerialNumber  int64  `gorm:"column:serial_number;default:0" json:"serialNumber"`
	UserNamespace string `gorm:"column:user_namespace;default:''" json:"-"`
}

func (TxtTocRule) TableName() string { return "txt_toc_rules" }

// HttpTTS HttpTTS 听书源（复合主键 url + user_namespace；type 0=在线合成/1=本地引擎）。
type HttpTTS struct {
	URL           string `gorm:"column:url;primaryKey" json:"url"`
	Name          string `gorm:"column:name;not null;default:''" json:"name"`
	Type          int    `gorm:"column:type;default:0" json:"type"`
	UserNamespace string `gorm:"column:user_namespace;primaryKey;default:''" json:"-"`
}

func (HttpTTS) TableName() string { return "http_tts_list" }

// SystemSetting 系统设置键值表（主键 key；OPDS 独立账号等）。
type SystemSetting struct {
	Key       string `gorm:"column:key;primaryKey" json:"key"`
	Value     string `gorm:"column:value" json:"value"`
	UpdatedAt int64  `gorm:"column:updated_at;default:0" json:"updatedAt"`
}

func (SystemSetting) TableName() string { return "system_settings" }

// UserConfig 用户配置（复合主键 user_namespace + ns）。
type UserConfig struct {
	UserNamespace string `gorm:"column:user_namespace;primaryKey" json:"-"`
	NS            string `gorm:"column:ns;primaryKey" json:"ns"`
	Config        string `gorm:"column:config" json:"config"`
	UpdatedAt     int64  `gorm:"column:updated_at;default:0" json:"updatedAt"`
}

func (UserConfig) TableName() string { return "user_config" }

// ReadingStat 阅读统计（复合主键 user_namespace + book_url + date）。
type ReadingStat struct {
	UserNamespace string `gorm:"column:user_namespace;primaryKey" json:"-"`
	BookURL       string `gorm:"column:book_url;primaryKey" json:"bookUrl"`
	Date          string `gorm:"column:date;primaryKey" json:"date"`
	Seconds       int64  `gorm:"column:seconds;default:0" json:"seconds"`
	Chars         int64  `gorm:"column:chars;default:0" json:"chars"`
}

func (ReadingStat) TableName() string { return "reading_stats" }

// TimestampNow 统一时间戳（Unix 秒）。
func TimestampNow() int64 { return time.Now().Unix() }
