package storage

import (
	"encoding/json"
	"time"

	"gorm.io/gorm"
	"gorm.io/gorm/clause"

	"github.com/Lvshujun0918/GoReader/internal/model"
)

// ListBooks 按命名空间列出书架书籍。
func (s *Storage) ListBooks(ns string) ([]model.Book, error) {
	var books []model.Book
	err := s.DB.Where("user_namespace = ?", ns).Order("order_num ASC, created_at ASC").Find(&books).Error
	return books, err
}

// FindBook 按命名空间+URL 查书架书。
func (s *Storage) FindBook(ns, bookURL string) (*model.Book, error) {
	var b model.Book
	if err := s.DB.Where("user_namespace = ? AND book_url = ?", ns, bookURL).First(&b).Error; err != nil {
		if err == gorm.ErrRecordNotFound {
			return nil, nil
		}
		return nil, err
	}
	return &b, nil
}

// SaveBook 保存/更新书架书（复合主键 upsert）。
func (s *Storage) SaveBook(ns string, b *model.Book) error {
	b.UserNamespace = ns
	if b.CreatedAt == 0 {
		b.CreatedAt = NowMillis()
	}
	// gorm 复合主键 upsert（sqlite ON CONFLICT）
	return s.DB.Clauses(upsertClause()).Create(b).Error
}

// SaveBooks 批量保存。
func (s *Storage) SaveBooks(ns string, books []*model.Book) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		for _, b := range books {
			b.UserNamespace = ns
			if b.CreatedAt == 0 {
				b.CreatedAt = NowMillis()
			}
			if err := tx.Clauses(upsertClause()).Create(b).Error; err != nil {
				return err
			}
		}
		return nil
	})
}

// DeleteBook 删除书架书（含章节缓存）。
func (s *Storage) DeleteBook(ns, bookURL string) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		if err := tx.Where("user_namespace = ? AND book_url = ?", ns, bookURL).Delete(&model.Book{}).Error; err != nil {
			return err
		}
		// 章节缓存按 book_url 清理（不区分命名空间——章节表无 ns 列）
		return tx.Where("book_url = ?", bookURL).Delete(&model.BookChapter{}).Error
	})
}

// DeleteBooks 批量删除书架书。
func (s *Storage) DeleteBooks(ns string, urls []string) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		if err := tx.Where("user_namespace = ? AND book_url IN ?", ns, urls).Delete(&model.Book{}).Error; err != nil {
			return err
		}
		return tx.Where("book_url IN ?", urls).Delete(&model.BookChapter{}).Error
	})
}

// CountBooks 用户书籍数。
func (s *Storage) CountBooks() (int64, error) {
	var n int64
	err := s.DB.Model(&model.Book{}).Count(&n).Error
	return n, err
}

// UpdateBookProgress 保存阅读进度（durChapter* 字段；totalNum>0 时一并更新总章数）。
func (s *Storage) UpdateBookProgress(ns, bookURL string, title string, index, pos, ts, totalNum int64) error {
	updates := map[string]any{
		"dur_chapter_title": title,
		"dur_chapter_index": index,
		"dur_chapter_pos":   pos,
		"dur_chapter_time":  ts,
	}
	if totalNum > 0 {
		updates["total_chapter_num"] = totalNum
	}
	return s.DB.Model(&model.Book{}).
		Where("user_namespace = ? AND book_url = ?", ns, bookURL).
		Updates(updates).Error
}

// SaveChapter 缓存章节正文（复合主键 upsert）。
func (s *Storage) SaveChapter(bookURL string, index int64, title, content string) error {
	ch := model.BookChapter{BookURL: bookURL, ChapterIndex: index, Title: title, Content: content}
	return s.DB.Clauses(upsertClause()).Create(&ch).Error
}

// GetChapter 读取章节正文缓存。
func (s *Storage) GetChapter(bookURL string, index int64) (*model.BookChapter, error) {
	var ch model.BookChapter
	if err := s.DB.Where("book_url = ? AND chapter_index = ?", bookURL, index).First(&ch).Error; err != nil {
		if err == gorm.ErrRecordNotFound {
			return nil, nil
		}
		return nil, err
	}
	return &ch, nil
}

// ListChapters 列出某书全部章节缓存。
func (s *Storage) ListChapters(bookURL string) ([]model.BookChapter, error) {
	var chapters []model.BookChapter
	err := s.DB.Where("book_url = ?", bookURL).Order("chapter_index ASC").Find(&chapters).Error
	return chapters, err
}

// DeleteBookCache 删除某书缓存（章节 + toc_cache）。
func (s *Storage) DeleteBookCache(bookURL string) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		if err := tx.Where("book_url = ?", bookURL).Delete(&model.BookChapter{}).Error; err != nil {
			return err
		}
		return tx.Where("book_url = ?", bookURL).Delete(&model.TocCache{}).Error
	})
}

// ClearCache 清空全部缓存（章节 + toc_cache）。
func (s *Storage) ClearCache() error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		if err := tx.Exec("DELETE FROM book_chapters").Error; err != nil {
			return err
		}
		return tx.Exec("DELETE FROM toc_cache").Error
	})
}

// TocCache TTL（5 分钟）。
const TocCacheTTL = 5 * time.Minute

// GetTocCache 读取目录缓存（未过期才返回）。
func (s *Storage) GetTocCache(bookURL string) (*model.TocCache, error) {
	var c model.TocCache
	if err := s.DB.Where("book_url = ?", bookURL).First(&c).Error; err != nil {
		if err == gorm.ErrRecordNotFound {
			return nil, nil
		}
		return nil, err
	}
	if NowMillis()-c.UpdatedAt > int64(TocCacheTTL/time.Millisecond) {
		return nil, nil
	}
	return &c, nil
}

// SetTocCache 写入目录缓存。
func (s *Storage) SetTocCache(bookURL string, chapters any) error {
	b, err := json.Marshal(chapters)
	if err != nil {
		return err
	}
	c := model.TocCache{BookURL: bookURL, ChaptersJSON: string(b), UpdatedAt: NowMillis()}
	return s.DB.Clauses(upsertClause()).Create(&c).Error
}

// CacheInfo 缓存统计（getCacheInfo）。
func (s *Storage) CacheInfo() (map[string]any, error) {
	var tocCount, tocSize, chCount, chSize int64
	s.DB.Model(&model.TocCache{}).Count(&tocCount)
	s.DB.Model(&model.TocCache{}).Select("COALESCE(SUM(length(chapters_json)),0)").Scan(&tocSize)
	s.DB.Model(&model.BookChapter{}).Count(&chCount)
	s.DB.Model(&model.BookChapter{}).Select("COALESCE(SUM(length(content)),0)").Scan(&chSize)
	return map[string]any{
		"tocCacheCount": tocCount,
		"tocCacheSize":  tocSize,
		"chapterCount":  chCount,
		"chapterSize":   chSize,
		"totalSize":     tocSize + chSize,
	}, nil
}

// upsertClause SQLite 复合主键 upsert（INSERT OR REPLACE 语义：冲突时全列更新）。
func upsertClause() clause.OnConflict {
	return clause.OnConflict{UpdateAll: true}
}
