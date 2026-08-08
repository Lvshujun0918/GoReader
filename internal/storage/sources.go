package storage

import (
	"gorm.io/gorm"
	"gorm.io/gorm/clause"

	"github.com/Lvshujun0918/GoReader/internal/model"
)

// ListBookSources 按命名空间返回书源（legacy 语义：用户无书源回退 default）。
func (s *Storage) ListBookSources(ns string) ([]model.BookSource, error) {
	var sources []model.BookSource
	if err := s.DB.Where("user_namespace = ?", ns).Find(&sources).Error; err != nil {
		return nil, err
	}
	if len(sources) == 0 && ns != "default" {
		var def []model.BookSource
		if err := s.DB.Where("user_namespace = ?", "default").Find(&def).Error; err != nil {
			return nil, err
		}
		return def, nil
	}
	return sources, nil
}

// FindBookSource 查找书源。
func (s *Storage) FindBookSource(ns, url string) (*model.BookSource, error) {
	var src model.BookSource
	err := s.DB.Where("user_namespace = ? AND book_source_url = ?", ns, url).First(&src).Error
	if err == nil {
		return &src, nil
	}
	if err != gorm.ErrRecordNotFound {
		return nil, err
	}
	// 用户书源缺失时回退 default
	if ns != "default" {
		if err := s.DB.Where("user_namespace = ? AND book_source_url = ?", "default", url).First(&src).Error; err == nil {
			return &src, nil
		}
	}
	return nil, nil
}

// SaveBookSource 保存书源。
func (s *Storage) SaveBookSource(ns string, src *model.BookSource) error {
	src.UserNamespace = ns
	if src.LastUpdateTime == 0 {
		src.LastUpdateTime = NowMillis()
	}
	return s.DB.Clauses(clause.OnConflict{UpdateAll: true}).Create(src).Error
}

// SaveBookSources 批量保存。
func (s *Storage) SaveBookSources(ns string, sources []*model.BookSource) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		for _, src := range sources {
			src.UserNamespace = ns
			if src.LastUpdateTime == 0 {
				src.LastUpdateTime = NowMillis()
			}
			if err := tx.Clauses(clause.OnConflict{UpdateAll: true}).Create(src).Error; err != nil {
				return err
			}
		}
		return nil
	})
}

// DeleteBookSource 删除书源。
func (s *Storage) DeleteBookSource(ns, url string) error {
	return s.DB.Where("user_namespace = ? AND book_source_url = ?", ns, url).Delete(&model.BookSource{}).Error
}

// DeleteBookSources 批量删除。
func (s *Storage) DeleteBookSources(ns string, urls []string) error {
	return s.DB.Where("user_namespace = ? AND book_source_url IN ?", ns, urls).Delete(&model.BookSource{}).Error
}

// DeleteAllBookSources 清空用户书源。
func (s *Storage) DeleteAllBookSources(ns string) error {
	return s.DB.Where("user_namespace = ?", ns).Delete(&model.BookSource{}).Error
}

// CountBookSources 书源数。
func (s *Storage) CountBookSources(ns string) (int64, error) {
	var n int64
	err := s.DB.Model(&model.BookSource{}).Where("user_namespace = ?", ns).Count(&n).Error
	return n, err
}

// SetAsDefaultBookSources 设置默认书源（enabled + custom_order）。
func (s *Storage) SetAsDefaultBookSources(ns string, urls []string) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		// 先清除原默认
		if err := tx.Model(&model.BookSource{}).Where("user_namespace = ?", ns).
			Update("custom_order", 0).Error; err != nil {
			return err
		}
		for i, u := range urls {
			if err := tx.Model(&model.BookSource{}).Where("user_namespace = ? AND book_source_url = ?", ns, u).
				Updates(map[string]any{"custom_order": i + 1, "enabled": 1}).Error; err != nil {
				return err
			}
		}
		return nil
	})
}

// GetBookSourceCookie 读取书源 cookie。
func (s *Storage) GetBookSourceCookie(ns, sourceURL string) (*model.BookSourceCookie, error) {
	var c model.BookSourceCookie
	if err := s.DB.Where("user_namespace = ? AND source_url = ?", ns, sourceURL).First(&c).Error; err != nil {
		if err == gorm.ErrRecordNotFound {
			return nil, nil
		}
		return nil, err
	}
	return &c, nil
}

// SetBookSourceCookie 保存书源 cookie。
func (s *Storage) SetBookSourceCookie(ns, sourceURL, cookie, ua string) error {
	c := model.BookSourceCookie{
		UserNamespace: ns,
		SourceURL:     sourceURL,
		Cookie:        cookie,
		UserAgent:     ua,
		UpdatedAt:     NowMillis(),
	}
	return s.DB.Clauses(clause.OnConflict{UpdateAll: true}).Create(&c).Error
}
