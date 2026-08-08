package storage

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"gorm.io/gorm"

	"github.com/Lvshujun0918/GoReader/internal/model"
)

// nsCompositeTables 五类 (url 单列主键) 表 → (url, user_namespace) 复合主键。
// 键为表名，值为 URL 列名——与 CREATE TABLE 的复合主键列序一致。
var nsCompositeTables = map[string]string{
	"http_tts_list": "url",
}

// MigrateNSPrimaryKeys 旧库迁移：复合主键重建（幂等）。
//
// 背景：原以 URL 为主键，secure 多用户下用户 B 保存同 URL 会覆盖用户 A 的行。
// 重建为复合主键后同 URL 按用户分行。已含复合主键即跳过（新库 CREATE 直接复合键）。
func (s *Storage) MigrateNSPrimaryKeys() error {
	for table, urlCol := range nsCompositeTables {
		var sqlText string
		err := s.DB.Raw("SELECT sql FROM sqlite_master WHERE type='table' AND name=?", table).Scan(&sqlText).Error
		if err != nil {
			return err
		}
		if sqlText == "" {
			continue // 表不存在（新库未建或未迁移）——AutoMigrate 已建
		}
		flat := strings.ReplaceAll(sqlText, `"`, "")
		composite := fmt.Sprintf("PRIMARY KEY (%s, user_namespace)", urlCol)
		if strings.Contains(flat, composite) {
			continue // 已是复合主键
		}

		// 实况列元数据（pragma 序）：name/type/notnull/dflt_value
		type colInfo struct {
			Name      string
			Type      string
			NotNull   int64
			DfltValue *string
		}
		var cols []colInfo
		if err := s.DB.Raw("SELECT name, type, \"notnull\", dflt_value FROM pragma_table_info(?)", table).Scan(&cols).Error; err != nil {
			return err
		}
		if len(cols) == 0 {
			continue
		}
		var defs []string
		var colList []string
		for _, c := range cols {
			colList = append(colList, fmt.Sprintf(`"%s"`, c.Name))
			d := fmt.Sprintf(`"%s" %s`, c.Name, c.Type)
			if c.NotNull != 0 {
				d += " NOT NULL"
			}
			if c.DfltValue != nil {
				d += " DEFAULT " + *c.DfltValue
			}
			defs = append(defs, d)
		}
		tmp := table + "_ns_pk"
		err = s.DB.Transaction(func(tx *gorm.DB) error {
			create := fmt.Sprintf("CREATE TABLE \"%s\" (%s, PRIMARY KEY (\"%s\", \"user_namespace\"))",
				tmp, strings.Join(defs, ", "), urlCol)
			if err := tx.Exec(create).Error; err != nil {
				return err
			}
			copySQL := fmt.Sprintf("INSERT INTO \"%s\" (%s) SELECT %s FROM \"%s\"",
				tmp, strings.Join(colList, ", "), strings.Join(colList, ", "), table)
			if err := tx.Exec(copySQL).Error; err != nil {
				return err
			}
			if err := tx.Exec(fmt.Sprintf("DROP TABLE \"%s\"", table)).Error; err != nil {
				return err
			}
			if err := tx.Exec(fmt.Sprintf("ALTER TABLE \"%s\" RENAME TO \"%s\"", tmp, table)).Error; err != nil {
				return err
			}
			return nil
		})
		if err != nil {
			return err
		}
	}
	return nil
}

// ---------------------------------------------------------------
// JSON storage → SQLite 一次性迁移
// ---------------------------------------------------------------

// MigrateFromJSON 触发条件：storage/data/users.json 存在 且 users 表为空。
// 迁移前自动备份 storage/data/ → storage/backup-before-migrate-{ts}/。
// 每类幂等（表空才迁），raw_json 全量保底。
func (s *Storage) MigrateFromJSON() error {
	dataDir := filepath.Join(s.Config.StorageDir(), "data")
	usersJSON := filepath.Join(dataDir, "users.json")
	if _, err := os.Stat(usersJSON); err != nil {
		return nil // 无 JSON 数据
	}

	var count int64
	if err := s.DB.Model(&model.User{}).Count(&count).Error; err != nil {
		return err
	}
	if count > 0 {
		// 补迁逻辑：users 表非空时，仍按各命名空间扫描 bookSource.json 等补迁空表
		return s.migrateJSONNamespaceFiles(dataDir)
	}

	// 迁移前备份
	backupDir := filepath.Join(s.Config.StorageDir(),
		fmt.Sprintf("backup-before-migrate-%d", time.Now().Unix()))
	if err := copyDir(dataDir, backupDir); err != nil {
		return fmt.Errorf("JSON 迁移前备份失败: %w", err)
	}

	// 1. users.json → users
	if err := s.migrateUsers(usersJSON); err != nil {
		return err
	}

	// 2. 各命名空间文件
	return s.migrateJSONNamespaceFiles(dataDir)
}

// migrateUsers users.json → users。
func (s *Storage) migrateUsers(path string) error {
	data, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	var users []model.User
	if err := json.Unmarshal(data, &users); err != nil {
		// 兼容旧格式：可能是 map[username]user
		var m map[string]model.User
		if err2 := json.Unmarshal(data, &m); err2 != nil {
			return fmt.Errorf("users.json 解析失败: %w", err)
		}
		for _, u := range m {
			users = append(users, u)
		}
	}
	for i := range users {
		u := &users[i]
		raw, _ := json.Marshal(u)
		u.RawJSON = string(raw)
		if err := s.DB.Create(u).Error; err != nil {
			return err
		}
	}
	return nil
}

// migrateJSONNamespaceFiles 按命名空间扫描补迁（幂等：对应表空才迁）。
func (s *Storage) migrateJSONNamespaceFiles(dataDir string) error {
	entries, err := os.ReadDir(dataDir)
	if err != nil {
		return nil
	}
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		ns := e.Name()
		dir := filepath.Join(dataDir, ns)

		if err := migrateNSFile(s, dir, "bookmark.json", &model.Bookmark{}, func(v *model.Bookmark) { v.UserNamespace = ns }); err != nil {
			return err
		}
		if err := migrateNSFile(s, dir, "replaceRule.json", &model.ReplaceRule{}, func(v *model.ReplaceRule) { v.UserNamespace = ns }); err != nil {
			return err
		}
		if err := migrateNSFile(s, dir, "txtTocRule.json", &model.TxtTocRule{}, func(v *model.TxtTocRule) { v.UserNamespace = ns }); err != nil {
			return err
		}
		if err := migrateNSFile(s, dir, "httpTTS.json", &model.HttpTTS{}, func(v *model.HttpTTS) { v.UserNamespace = ns }); err != nil {
			return err
		}
		if err := migrateNSFile(s, dir, "bookGroup.json", &model.BookGroup{}, func(v *model.BookGroup) { v.UserNamespace = ns }); err != nil {
			return err
		}
		if err := migrateNSFile(s, dir, "userConfig.json", &model.UserConfig{}, func(v *model.UserConfig) { v.UserNamespace = ns }); err != nil {
			return err
		}
		// 书架
		if err := migrateNSFile(s, dir, "bookshelf.json", &model.Book{}, func(v *model.Book) { v.UserNamespace = ns }); err != nil {
			return err
		}
	}
	return nil
}

// migrateNSFile 迁移单个 JSON 文件到表（表空才迁，raw_json 保底）。
func migrateNSFile[T any](s *Storage, dir, file string, dst *T, applyNS func(*T)) error {
	path := filepath.Join(dir, file)
	data, err := os.ReadFile(path)
	if err != nil {
		return nil // 无此文件
	}
	var count int64
	if err := s.DB.Model(dst).Count(&count).Error; err != nil {
		return err
	}
	if count > 0 {
		return nil // 表非空，跳过
	}
	var rows []*T
	if err := json.Unmarshal(data, &rows); err != nil {
		return nil // 解析失败跳过（保持幂等）
	}
	for _, r := range rows {
		applyNS(r)
		if err := s.DB.Create(r).Error; err != nil {
			return err
		}
	}
	return nil
}

func copyDir(src, dst string) error {
	return filepath.Walk(src, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		rel, err := filepath.Rel(src, path)
		if err != nil {
			return err
		}
		target := filepath.Join(dst, rel)
		if info.IsDir() {
			return os.MkdirAll(target, 0o755)
		}
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			return err
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		return os.WriteFile(target, data, 0o644)
	})
}
