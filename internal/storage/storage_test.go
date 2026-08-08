package storage

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/Lvshujun0918/GoReader/internal/config"
	"github.com/Lvshujun0918/GoReader/internal/model"
)

func testStorage(t *testing.T) *Storage {
	t.Helper()
	dir := t.TempDir()
	cfg := &config.Config{WorkDir: dir, Secure: false}
	s, err := Init(cfg)
	if err != nil {
		t.Fatalf("Init 失败: %v", err)
	}
	t.Cleanup(func() {
		if sqlDB, err := s.DB.DB(); err == nil {
			_ = sqlDB.Close()
		}
		_ = os.RemoveAll(dir)
	})
	return s
}

func TestUserCRUD(t *testing.T) {
	s := testStorage(t)
	u := &model.User{
		Username: "alice", Password: "hash", Salt: "salt1",
		Token: "tok1", UserNamespace: "alice",
	}
	if err := s.InsertUser(u); err != nil {
		t.Fatalf("InsertUser 失败: %v", err)
	}
	found, err := s.FindUser("alice")
	if err != nil || found == nil {
		t.Fatalf("FindUser 失败: %v / %v", found, err)
	}
	// 重复插入（复合主键冲突）
	if err := s.InsertUser(&model.User{Username: "alice", Password: "h2"}); err == nil {
		t.Fatal("重复 username 应失败")
	}
	count, _ := s.CountUsers()
	if count != 1 {
		t.Fatalf("CountUsers 应为 1，got %d", count)
	}
	// token 追加
	if err := s.AddUserToken("alice", "tok2", NowMillis()); err != nil {
		t.Fatalf("AddUserToken 失败: %v", err)
	}
	found, _ = s.FindUser("alice")
	if found.Token != "tok2" {
		t.Fatalf("主 token 应更新为 tok2，got %q", found.Token)
	}
	// 移除 token
	if err := s.RemoveUserToken("alice", "tok2"); err != nil {
		t.Fatalf("RemoveUserToken 失败: %v", err)
	}
	found, _ = s.FindUser("alice")
	if found.Token != "" {
		t.Fatalf("主 token 应清空，got %q", found.Token)
	}
}

func TestBookCRUDAndNamespaceIsolation(t *testing.T) {
	s := testStorage(t)
	nsA := "userA"
	nsB := "userB"
	b1 := &model.Book{BookURL: "https://a/book/1", Name: "书A"}
	b2 := &model.Book{BookURL: "https://a/book/1", Name: "书B"}
	if err := s.SaveBook(nsA, b1); err != nil {
		t.Fatalf("SaveBook(A) 失败: %v", err)
	}
	if err := s.SaveBook(nsB, b2); err != nil {
		t.Fatalf("SaveBook(B) 失败: %v", err)
	}
	// 同 URL 不同命名空间互不影响
	listA, _ := s.ListBooks(nsA)
	if len(listA) != 1 || listA[0].Name != "书A" {
		t.Fatalf("命名空间 A 隔离失败: %+v", listA)
	}
	// 章节缓存（无 ns 列，按 book_url）
	if err := s.SaveChapter("https://a/book/1", 0, "第一章", "正文内容"); err != nil {
		t.Fatalf("SaveChapter 失败: %v", err)
	}
	ch, err := s.GetChapter("https://a/book/1", 0)
	if err != nil || ch == nil || ch.Content != "正文内容" {
		t.Fatalf("GetChapter 失败: %v / %v", ch, err)
	}
}

func TestCacheInfo(t *testing.T) {
	s := testStorage(t)
	_ = s.SaveChapter("https://b/1", 0, "t", "content123")
	info, err := s.CacheInfo()
	if err != nil {
		t.Fatalf("CacheInfo 失败: %v", err)
	}
	if info["chapterCount"].(int64) != 1 {
		t.Fatalf("chapterCount 应为 1: %v", info)
	}
}

func TestMigrateNSPrimaryKeysIdempotent(t *testing.T) {
	s := testStorage(t)
	// 新库已是复合主键 → 迁移应幂等跳过
	if err := s.MigrateNSPrimaryKeys(); err != nil {
		t.Fatalf("复合主键迁移失败: %v", err)
	}
}

var _ = filepath.Join
