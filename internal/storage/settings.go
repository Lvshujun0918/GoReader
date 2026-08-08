package storage

import (
	"errors"

	"gorm.io/gorm"

	"github.com/Lvshujun0918/GoReader/internal/model"
)

func isNotFound(err error) bool {
	return errors.Is(err, gorm.ErrRecordNotFound)
}

// GetSystemSetting 读取键值设置。
func (s *Storage) GetSystemSetting(key string) (string, bool, error) {
	var row model.SystemSetting
	if err := s.DB.Where("key = ?", key).First(&row).Error; err != nil {
		if isNotFound(err) {
			return "", false, nil
		}
		return "", false, err
	}
	return row.Value, true, nil
}

// SetSystemSetting 写入键值设置。
func (s *Storage) SetSystemSetting(key, value string) error {
	row := model.SystemSetting{Key: key, Value: value, UpdatedAt: NowMillis()}
	return s.DB.Save(&row).Error
}
