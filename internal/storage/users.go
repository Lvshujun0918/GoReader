package storage

import (
	"encoding/json"
	"time"

	"gorm.io/gorm"

	"github.com/Lvshujun0918/GoReader/internal/model"
)

// FindUser 按用户名查询用户。
func (s *Storage) FindUser(username string) (*model.User, error) {
	var u model.User
	if err := s.DB.Where("username = ?", username).First(&u).Error; err != nil {
		if err == gorm.ErrRecordNotFound {
			return nil, nil
		}
		return nil, err
	}
	return &u, nil
}

// InsertUser 创建用户。
func (s *Storage) InsertUser(u *model.User) error {
	return s.DB.Create(u).Error
}

// CountUsers 用户数。
func (s *Storage) CountUsers() (int64, error) {
	var n int64
	err := s.DB.Model(&model.User{}).Count(&n).Error
	return n, err
}

// ListUsers 全部用户（按创建时间升序）。
func (s *Storage) ListUsers() ([]model.User, error) {
	var users []model.User
	err := s.DB.Order("created_at ASC").Find(&users).Error
	return users, err
}

// AddUserToken 追加 token 到 token_map（多设备会话，上限 5）；同时更新主 token。
func (s *Storage) AddUserToken(username, tok string, now int64) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		var u model.User
		if err := tx.Where("username = ?", username).First(&u).Error; err != nil {
			return err
		}
		// token_map：JSON 数组形态 ["a","b"]
		tokens := tokenMapToSlice(u.TokenMap)
		// 去重 + 追加
		seen := map[string]bool{u.Token: true}
		filtered := make([]string, 0, len(tokens)+1)
		for _, t := range tokens {
			if t != "" && !seen[t] {
				filtered = append(filtered, t)
				seen[t] = true
			}
		}
		filtered = append(filtered, tok)
		// 上限 5：保留最近 5 个
		if len(filtered) > 5 {
			filtered = filtered[len(filtered)-5:]
		}
		b, _ := json.Marshal(filtered)
		return tx.Model(&model.User{}).Where("username = ?", username).
			Updates(map[string]any{
				"token":      tok,
				"token_map":  string(b),
				"last_login_at": now,
			}).Error
	})
}

// RemoveUserToken 移除指定 token（退出登录单设备）。
func (s *Storage) RemoveUserToken(username, tok string) error {
	return s.DB.Transaction(func(tx *gorm.DB) error {
		var u model.User
		if err := tx.Where("username = ?", username).First(&u).Error; err != nil {
			return err
		}
		tokens := tokenMapToSlice(u.TokenMap)
		filtered := tokens[:0]
		for _, t := range tokens {
			if t != tok {
				filtered = append(filtered, t)
			}
		}
		b, _ := json.Marshal(filtered)
		updates := map[string]any{"token_map": string(b)}
		// 若移除的是主 token，主 token 清空
		if u.Token == tok {
			updates["token"] = ""
		}
		return tx.Model(&model.User{}).Where("username = ?", username).Updates(updates).Error
	})
}

// LogoutUser 全清 token（legacy 兼容）。
func (s *Storage) LogoutUser(username string) error {
	return s.DB.Model(&model.User{}).Where("username = ?", username).
		Updates(map[string]any{"token": "", "token_map": ""}).Error
}

// UpdateUserPermissions 更新用户权限（nil 字段不修改）。返回受影响行数。
func (s *Storage) UpdateUserPermissions(username string, enableWebdav, enableLocalStore *bool, bookLimit *int64) (int64, error) {
	updates := map[string]any{}
	if enableWebdav != nil {
		updates["enable_webdav"] = *enableWebdav
	}
	if enableLocalStore != nil {
		updates["enable_local_store"] = *enableLocalStore
	}
	if bookLimit != nil {
		updates["book_limit"] = *bookLimit
	}
	if len(updates) == 0 {
		return 0, nil
	}
	res := s.DB.Model(&model.User{}).Where("username = ?", username).Updates(updates)
	return res.RowsAffected, res.Error
}

// DeleteUser 删除用户。返回受影响行数。
func (s *Storage) DeleteUser(username string) (int64, error) {
	res := s.DB.Where("username = ?", username).Delete(&model.User{})
	return res.RowsAffected, res.Error
}

// DeleteUsers 批量删除用户（含命名空间数据清理）。
func (s *Storage) DeleteUsers(usernames []string) (int64, error) {
	var n int64
	err := s.DB.Transaction(func(tx *gorm.DB) error {
		res := tx.Where("username IN ?", usernames).Delete(&model.User{})
		if res.Error != nil {
			return res.Error
		}
		n = res.RowsAffected
		// 清理各命名空间数据
		cleanup := []any{
			&model.Book{}, &model.Bookmark{}, &model.BookGroup{},
			&model.ReplaceRule{}, &model.TxtTocRule{}, &model.HttpTTS{},
			&model.UserConfig{},
		}
		for _, m := range cleanup {
			if err := tx.Where("user_namespace IN ?", usernames).Delete(m).Error; err != nil {
				return err
			}
		}
		return nil
	})
	return n, err
}

// ResetUserPassword 重置密码。
func (s *Storage) ResetUserPassword(username, salt, encrypted string) (int64, error) {
	res := s.DB.Model(&model.User{}).Where("username = ?", username).
		Updates(map[string]any{"password": encrypted, "salt": salt})
	return res.RowsAffected, res.Error
}

// ClearInactiveUsers 清理 last_login_at < before 且非当前用户的用户。返回被删用户名。
func (s *Storage) ClearInactiveUsers(before int64, except *string) ([]string, error) {
	var users []model.User
	q := s.DB.Where("last_login_at < ?", before)
	if except != nil {
		q = q.Where("username != ?", *except)
	}
	if err := q.Find(&users).Error; err != nil {
		return nil, err
	}
	var names []string
	for _, u := range users {
		names = append(names, u.Username)
	}
	if len(names) > 0 {
		if _, err := s.DeleteUsers(names); err != nil {
			return nil, err
		}
	}
	return names, nil
}

// UpgradeUserPasswordHash 密码自动升级为 argon2id。
func (s *Storage) UpgradeUserPasswordHash(username, phc string) error {
	return s.DB.Model(&model.User{}).Where("username = ?", username).
		Update("password", phc).Error
}

// tokenMapToSlice token_map JSON 数组 → slice；兼容旧对象形态 {"token":ts}。
func tokenMapToSlice(tm string) []string {
	tm = trimSpace(tm)
	if tm == "" {
		return nil
	}
	if tm[0] == '[' {
		var arr []string
		if err := json.Unmarshal([]byte(tm), &arr); err == nil {
			return arr
		}
	}
	if tm[0] == '{' {
		var m map[string]int64
		if err := json.Unmarshal([]byte(tm), &m); err == nil {
			out := make([]string, 0, len(m))
			for k := range m {
				out = append(out, k)
			}
			return out
		}
	}
	return splitComma(tm)
}

func trimSpace(s string) string {
	start, end := 0, len(s)
	for start < end && (s[start] == ' ' || s[start] == '\t' || s[start] == '\n') {
		start++
	}
	for end > start && (s[end-1] == ' ' || s[end-1] == '\t' || s[end-1] == '\n') {
		end--
	}
	return s[start:end]
}

func splitComma(s string) []string {
	var out []string
	cur := ""
	for _, r := range s {
		if r == ',' {
			if cur != "" {
				out = append(out, cur)
				cur = ""
			}
			continue
		}
		cur += string(r)
	}
	if cur != "" {
		out = append(out, cur)
	}
	return out
}

// NowMillis Unix 毫秒时间戳。
func NowMillis() int64 { return time.Now().UnixMilli() }
