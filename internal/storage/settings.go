package storage

import (
	"encoding/json"
	"errors"

	"gorm.io/gorm"

	"github.com/Lvshujun0918/reader-dev/internal/model"
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

// GetOPDSSettings 读取 OPDS 独立账号设置（system_settings 中 OPDS_* 键）。
func (s *Storage) GetOPDSSettings() (map[string]any, error) {
	keys := []string{"OPDS_ACCOUNT", "OPDS_PASSWORD", "OPDS_WEBDAV_PATH", "OPDS_AUTO_LOGIN"}
	out := map[string]any{}
	for _, k := range keys {
		v, ok, err := s.GetSystemSetting(k)
		if err != nil {
			return nil, err
		}
		if ok {
			out[opdsKeyToJSON(k)] = v
		}
	}
	return out, nil
}

// SaveOPDSSettings 保存 OPDS 设置（params 中驼峰键 → OPDS_* 存储键）。
func (s *Storage) SaveOPDSSettings(params map[string]any) error {
	keys := map[string]string{
		"opdsAccount":  "OPDS_ACCOUNT",
		"opdsPassword": "OPDS_PASSWORD",
		"opdsWebdavPath": "OPDS_WEBDAV_PATH",
		"opdsAutoLogin": "OPDS_AUTO_LOGIN",
	}
	for jsonKey, storeKey := range keys {
		if v, ok := params[jsonKey]; ok {
			var sv string
			switch t := v.(type) {
			case string:
				sv = t
			case bool:
				sv = "true"
			default:
				b, _ := json.Marshal(t)
				sv = string(b)
			}
			if err := s.SetSystemSetting(storeKey, sv); err != nil {
				return err
			}
		}
	}
	return nil
}

func opdsKeyToJSON(storeKey string) string {
	switch storeKey {
	case "OPDS_ACCOUNT":
		return "opdsAccount"
	case "OPDS_PASSWORD":
		return "opdsPassword"
	case "OPDS_WEBDAV_PATH":
		return "opdsWebdavPath"
	case "OPDS_AUTO_LOGIN":
		return "opdsAutoLogin"
	}
	return storeKey
}
