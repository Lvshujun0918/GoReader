package storage

import (
	"encoding/json"

	"gorm.io/gorm"
	"gorm.io/gorm/clause"

	"github.com/Lvshujun0918/reader-dev/internal/model"
)

// ---------------- 书签 ----------------

func (s *Storage) ListBookmarks(ns string) ([]model.Bookmark, error) {
	var out []model.Bookmark
	err := s.DB.Where("user_namespace = ?", ns).Find(&out).Error
	return out, err
}

func (s *Storage) SaveBookmark(ns string, bm *model.Bookmark) error {
	bm.UserNamespace = ns
	if bm.CreatedAt == 0 {
		bm.CreatedAt = NowMillis()
	}
	return s.DB.Clauses(clause.OnConflict{UpdateAll: true}).Create(bm).Error
}

func (s *Storage) SaveBookmarks(ns string, list []*model.Bookmark) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		for _, bm := range list {
			bm.UserNamespace = ns
			if bm.CreatedAt == 0 {
				bm.CreatedAt = NowMillis()
			}
			if err := tx.Clauses(clause.OnConflict{UpdateAll: true}).Create(bm).Error; err != nil {
				return err
			}
		}
		return nil
	})
}

func (s *Storage) DeleteBookmark(ns, bookURL, title string) error {
	return s.DB.Where("user_namespace = ? AND book_url = ? AND title = ?", ns, bookURL, title).Delete(&model.Bookmark{}).Error
}

func (s *Storage) DeleteBookmarks(ns, bookURL string, titles []string) error {
	return s.DB.Where("user_namespace = ? AND book_url = ? AND title IN ?", ns, bookURL, titles).Delete(&model.Bookmark{}).Error
}

// ---------------- 书架分组 ----------------

func (s *Storage) ListBookGroups(ns string) ([]model.BookGroup, error) {
	var out []model.BookGroup
	err := s.DB.Where("user_namespace = ?", ns).Order("order_num ASC, id ASC").Find(&out).Error
	return out, err
}

func (s *Storage) SaveBookGroup(ns string, g *model.BookGroup) error {
	g.UserNamespace = ns
	if g.ID > 0 {
		return s.DB.Model(&model.BookGroup{}).Where("id = ? AND user_namespace = ?", g.ID, ns).Updates(map[string]any{
			"name": g.Name, "order_num": g.OrderNum,
		}).Error
	}
	return s.DB.Create(g).Error
}

func (s *Storage) DeleteBookGroup(ns string, id int64) error {
	return s.DB.Where("id = ? AND user_namespace = ?", id, ns).Delete(&model.BookGroup{}).Error
}

// MoveBookToGroup 批量移动书籍到分组（books.group_name 存分组 id）。
func (s *Storage) MoveBookToGroup(ns string, groupID int64, urls []string) error {
	return s.DB.Model(&model.Book{}).
		Where("user_namespace = ? AND book_url IN ?", ns, urls).
		Update("group_name", groupID).Error
}

func (s *Storage) SaveBookGroupOrder(ns string, ids []int64) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		for i, id := range ids {
			if err := tx.Model(&model.BookGroup{}).Where("id = ? AND user_namespace = ?", id, ns).
				Update("order_num", int64(i+1)).Error; err != nil {
				return err
			}
		}
		return nil
	})
}

// ---------------- 替换规则 ----------------

func (s *Storage) ListReplaceRules(ns string) ([]model.ReplaceRule, error) {
	var out []model.ReplaceRule
	err := s.DB.Where("user_namespace = ?", ns).Order("order_num ASC").Find(&out).Error
	return out, err
}

func (s *Storage) SaveReplaceRule(ns string, r *model.ReplaceRule) error {
	r.UserNamespace = ns
	return s.DB.Clauses(clause.OnConflict{UpdateAll: true}).Create(r).Error
}

func (s *Storage) SaveReplaceRules(ns string, list []*model.ReplaceRule) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		for _, r := range list {
			r.UserNamespace = ns
			if err := tx.Clauses(clause.OnConflict{UpdateAll: true}).Create(r).Error; err != nil {
				return err
			}
		}
		return nil
	})
}

func (s *Storage) DeleteReplaceRule(ns, id string) error {
	return s.DB.Where("user_namespace = ? AND id = ?", ns, id).Delete(&model.ReplaceRule{}).Error
}

func (s *Storage) DeleteReplaceRules(ns string, ids []string) error {
	return s.DB.Where("user_namespace = ? AND id IN ?", ns, ids).Delete(&model.ReplaceRule{}).Error
}

// ---------------- TXT 目录规则 ----------------

func (s *Storage) ListTxtTocRules(ns string) ([]model.TxtTocRule, error) {
	var out []model.TxtTocRule
	err := s.DB.Where("user_namespace = ?", ns).Order("serial_number ASC").Find(&out).Error
	return out, err
}

func (s *Storage) SaveTxtTocRule(ns string, r *model.TxtTocRule) error {
	r.UserNamespace = ns
	return s.DB.Clauses(clause.OnConflict{UpdateAll: true}).Create(r).Error
}

func (s *Storage) DeleteTxtTocRule(ns, id string) error {
	return s.DB.Where("user_namespace = ? AND id = ?", ns, id).Delete(&model.TxtTocRule{}).Error
}

// ---------------- HttpTTS ----------------

func (s *Storage) ListHttpTTS(ns string) ([]model.HttpTTS, error) {
	var out []model.HttpTTS
	err := s.DB.Where("user_namespace = ?", ns).Find(&out).Error
	return out, err
}

func (s *Storage) SaveHttpTTS(ns string, t *model.HttpTTS) error {
	t.UserNamespace = ns
	return s.DB.Clauses(clause.OnConflict{UpdateAll: true}).Create(t).Error
}

func (s *Storage) SaveHttpTTSMulti(ns string, list []*model.HttpTTS) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		for _, t := range list {
			t.UserNamespace = ns
			if err := tx.Clauses(clause.OnConflict{UpdateAll: true}).Create(t).Error; err != nil {
				return err
			}
		}
		return nil
	})
}

func (s *Storage) DeleteHttpTTS(ns, url string) error {
	return s.DB.Where("user_namespace = ? AND url = ?", ns, url).Delete(&model.HttpTTS{}).Error
}

// ---------------- 订阅 ----------------

func (s *Storage) ListSourceSubs(ns string) ([]model.SourceSub, error) {
	var out []model.SourceSub
	err := s.DB.Where("user_namespace = ?", ns).Find(&out).Error
	return out, err
}

func (s *Storage) SaveSourceSub(ns string, sub *model.SourceSub) error {
	sub.UserNamespace = ns
	return s.DB.Clauses(clause.OnConflict{UpdateAll: true}).Create(sub).Error
}

func (s *Storage) DeleteSourceSub(ns, url string) error {
	return s.DB.Where("user_namespace = ? AND url = ?", ns, url).Delete(&model.SourceSub{}).Error
}

// ---------------- RSS ----------------

func (s *Storage) ListRssSources(ns string) ([]model.RssSource, error) {
	var out []model.RssSource
	err := s.DB.Where("user_namespace = ?", ns).Find(&out).Error
	return out, err
}

func (s *Storage) SaveRssSource(ns string, src *model.RssSource) error {
	src.UserNamespace = ns
	return s.DB.Clauses(clause.OnConflict{UpdateAll: true}).Create(src).Error
}

func (s *Storage) SaveRssSources(ns string, list []*model.RssSource) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		for _, src := range list {
			src.UserNamespace = ns
			if err := tx.Clauses(clause.OnConflict{UpdateAll: true}).Create(src).Error; err != nil {
				return err
			}
		}
		return nil
	})
}

func (s *Storage) DeleteRssSource(ns, url string) error {
	return s.DB.Where("user_namespace = ? AND rss_source_url = ?", ns, url).Delete(&model.RssSource{}).Error
}

// FindRssSource 查找 RSS 源。
func (s *Storage) FindRssSource(ns, url string) (*model.RssSource, error) {
	var src model.RssSource
	if err := s.DB.Where("user_namespace = ? AND rss_source_url = ?", ns, url).First(&src).Error; err != nil {
		if err == gorm.ErrRecordNotFound {
			return nil, nil
		}
		return nil, err
	}
	return &src, nil
}

func (s *Storage) ListRssArticles(ns, sourceURL string) ([]model.RssArticle, error) {
	var out []model.RssArticle
	q := s.DB.Where("user_namespace = ?", ns)
	if sourceURL != "" {
		q = q.Where("source_url = ?", sourceURL)
	}
	err := q.Order("time DESC").Find(&out).Error
	return out, err
}

func (s *Storage) SaveRssArticle(ns string, a *model.RssArticle) error {
	a.UserNamespace = ns
	return s.DB.Clauses(clause.OnConflict{UpdateAll: true}).Create(a).Error
}

func (s *Storage) SaveRssArticles(ns string, list []*model.RssArticle) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		for _, a := range list {
			a.UserNamespace = ns
			if err := tx.Clauses(clause.OnConflict{UpdateAll: true}).Create(a).Error; err != nil {
				return err
			}
		}
		return nil
	})
}

func (s *Storage) FindRssArticle(ns, url string) (*model.RssArticle, error) {
	var a model.RssArticle
	if err := s.DB.Where("user_namespace = ? AND url = ?", ns, url).First(&a).Error; err != nil {
		if err == gorm.ErrRecordNotFound {
			return nil, nil
		}
		return nil, err
	}
	return &a, nil
}

func (s *Storage) MarkRssArticleRead(ns, url string, read bool) error {
	return s.DB.Model(&model.RssArticle{}).Where("user_namespace = ? AND url = ?", ns, url).
		Update("read", read).Error
}

// ---------------- 用户配置 ----------------

func (s *Storage) GetUserConfig(ns, key string) (map[string]any, error) {
	var c model.UserConfig
	if err := s.DB.Where("user_namespace = ? AND ns = ?", ns, key).First(&c).Error; err != nil {
		if err == gorm.ErrRecordNotFound {
			return map[string]any{}, nil
		}
		return nil, err
	}
	var m map[string]any
	if err := json.Unmarshal([]byte(c.Config), &m); err != nil {
		return map[string]any{}, nil
	}
	return m, nil
}

func (s *Storage) SaveUserConfig(ns, key string, config any) error {
	b, err := json.Marshal(config)
	if err != nil {
		return err
	}
	c := model.UserConfig{UserNamespace: ns, NS: key, Config: string(b), UpdatedAt: NowMillis()}
	return s.DB.Clauses(clause.OnConflict{UpdateAll: true}).Create(&c).Error
}

// ---------------- 阅读统计 ----------------

func (s *Storage) SaveReadingStat(ns, bookURL, date string, seconds, chars int64) error {
	stat := model.ReadingStat{UserNamespace: ns, BookURL: bookURL, Date: date}
	var existing model.ReadingStat
	if err := s.DB.Where("user_namespace = ? AND book_url = ? AND date = ?", ns, bookURL, date).First(&existing).Error; err == nil {
		return s.DB.Model(&model.ReadingStat{}).Where("user_namespace = ? AND book_url = ? AND date = ?", ns, bookURL, date).
			Updates(map[string]any{
				"seconds": existing.Seconds + seconds,
				"chars":   existing.Chars + chars,
			}).Error
	}
	stat.Seconds = seconds
	stat.Chars = chars
	return s.DB.Create(&stat).Error
}

// ReadingStatsByDateRange 按日期范围统计（date >= start）。
func (s *Storage) ReadingStatsByDateRange(ns, start string) (int64, int64, error) {
	var sum struct {
		Seconds int64
		Chars   int64
	}
	err := s.DB.Model(&model.ReadingStat{}).
		Select("COALESCE(SUM(seconds),0) AS seconds, COALESCE(SUM(chars),0) AS chars").
		Where("user_namespace = ? AND date >= ?", ns, start).Scan(&sum).Error
	return sum.Seconds, sum.Chars, err
}

// ReadingStatsPerBook 单书汇总（按秒数降序）。
func (s *Storage) ReadingStatsPerBook(ns string) ([]model.ReadingStat, error) {
	var out []model.ReadingStat
	err := s.DB.Model(&model.ReadingStat{}).
		Select("book_url, SUM(seconds) AS seconds, SUM(chars) AS chars").
		Where("user_namespace = ?", ns).Group("book_url").Order("seconds DESC").Scan(&out).Error
	return out, err
}
