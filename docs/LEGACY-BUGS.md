# Legacy 已知 Bug（不修旧分支，Go 重写时修复）

> 决策：legacy（Kotlin）分支不再修复以下 bug，Go 重写（master）实现对应功能时修复。

## 1. 导入订阅源报错

- **现象**：legacy 远程书源/订阅源导入（saveFromRemoteSource / readSourceFile）报错
- **Go 版修复要求**：重写书源导入链路时实现并验证：
  - 远程 JSON 抓取（订阅源 URL）→ 校验格式 → 批量入库（save_book_sources）
  - 前端订阅源 UI 对接（输入 URL → 导入 → 列表刷新）

## 2. WebDAV 文件管理默认目录

- **现象**：legacy 文件管理打开 WebDAV 定位到根目录，应为 `/webdav/legado`（legado 客户端默认备份目录）
- **Go 版修复要求**：
  - 文件管理前端打开 WebDAV 时默认定位 `storage/data/{user}/webdav/legado`
  - WebDAV 服务（未来切片）默认目录语义对齐：legado 客户端备份写入 `{user}/webdav/legado/`

## 3. 其他已知（补充记录）

（新增 bug 在此追加）
