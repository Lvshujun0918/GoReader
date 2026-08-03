# reader-dev (Rust) — 架构设计

> 状态：骨架阶段（垂直切片迭代中）。稳定版（Kotlin）见 `legacy` 分支。
> 开发策略：master 开发中，不发版不 release（可 pre-release/tag），docker-publish 有发版 guard。

---

## 1. 目标

Rust 重构，**API 兼容 + 数据兼容迁移**，并吸收计划功能：
- SQLite 兼容迁移（远期 2）
- legado 多规则解析（远期 3）
- 书籍格式扩展（远期 4）

## 2. 模块结构

```
src/
├── main.rs        启动/配置（READER_APP_* env，兼容 legacy）
├── api/           HTTP 路由（/reader3/* 兼容）+ ReturnData（isSuccess/errorMsg/data）
├── model/         实体（兼容 legacy：User/Book/BookChapter/BookSource/RssSource...）
├── storage/       SQLite + JSON 迁移
├── parser/        legado 多规则引擎（CSS/JSONPath/XPath/Regex/JS）
├── service/       业务（书源/书架/书籍/RSS/TTS/本地书）
└── util/          工具（md5/编码检测/加密...）
```

## 3. API 兼容约定（硬性）

- 路由路径：`/reader3/*` 与 legacy 完全一致
- 返回结构：`{"isSuccess": bool, "errorMsg": string, "data": ...}`（camelCase）
- 认证：`accessToken`（`username:token` query/header），`secure` 模式行为一致
- legacy 已知 bug 清单（重写时修复）：见 `docs/LEGACY-BUGS.md`（导入订阅源报错 / WebDAV 默认目录）
- 参数名/语义与 legacy 一致（书源规则格式、搜索参数、分页等）
- 前端：兼容阶段复用 legacy 构建产物（rust-embed 内嵌 dist），后续再演进

## 4. 数据兼容迁移（设计）

### 现状（legacy）
- `storage/data/`：users.json / bookshelf.json / bookSource.json / rssSource.json / bookGroup.json / 各用户子目录 / 本地书文件
- 用户隔离：secure 模式下 `storage/data/{username}/`，非 secure 为 `storage/data/default/`
- 书籍正文/缓存：文件（`storage/data/{user}/books/...`、cache/）

### 迁移策略（JSON → SQLite）
1. **启动时自动检测**：`storage/reader.db` 不存在且 `storage/data/` 存在 JSON → 触发一次性迁移
2. **迁移前自动备份**：`storage/backup-before-migrate-{ts}/`（原 JSON 完整拷贝）
3. **逐表迁移**：
   - users.json → `users` 表（含密码盐/token/权限字段）
   - bookshelf.json → `books` 表（bookUrl 主键 + 全部字段）
   - bookSource.json → `book_sources` 表（含规则 JSON 原样）
   - rssSource.json / rssArticle 等 → 对应表
   - bookGroup.json → `book_groups`
   - 用户子目录 JSON → 按 `user_namespace` 列归入
4. **迁移校验**：行数核对 + 抽样比对；失败自动回滚（备份恢复），保留 JSON
5. **文件数据不动**：书籍正文/封面/缓存文件路径不变（SQLite 只存引用）
6. **双向兼容**：迁移后 JSON 保留（只读归档）；SQLite 为唯一数据源
7. **回滚路径**：`READER_APP_MIGRATE_SKIP=1` 跳过迁移（legacy 容器继续用 JSON）

### 表结构（v1）
```sql
users(username PK, password, salt, token, enable_webdav, enable_local_store,
      enable_book_source, enable_rss_source, book_source_limit, book_limit,
      last_login_at, created_at, user_namespace)

books(book_url PK, name, author, origin, origin_name, kind, cover_url, intro,
      toc_url, charset, custom_cover_url, can_update, dur_chapter_index,
      dur_chapter_pos, dur_chapter_time, dur_chapter_title, group, type,
      last_check_error, user_namespace, created_at)

book_sources(book_source_url PK, book_source_name, book_source_group,
      book_source_type, rule_*, enabled, user_namespace, ...)

book_groups(id, group_name, order_num, user_namespace)
```

## 5. 规则解析引擎（legado 多规则，逐项移植）

| 规则类型 | Rust 实现 | 状态 |
|---|---|---|
| CSS Selector | `scraper` | ✅（含 @CSS:/@@ 前缀、a@href 属性） |
| JSONPath | 自实现遍历 | ✅（@Json:/$./$[ 前缀、[*] 通配、{{$.x}} 内嵌） |
| Regex | `regex` | ✅（$N 引用、##替换） |
| XPath | `sxd-xpath`/`sxd-document` | 🔄 实现中 |
| JavaScript | `boa_engine`（纯 Rust） | 🔄 实现中（含 {{}} 内嵌 JS） |

对齐 `warpdotsys/legado`（阅读Sigma）的 analyzeRule 语义（参考 docs/legado-ref/ 源码与文档）：
- 规则标志：@@ / @CSS: / @XPath: / @Json: / $. / $[ / // / @js:
- 三段/两段式 `##` 拆分 + 替换规则
- URL 附加参数（,{"js":..}/{"bodyJs":..}）与并发率（concurrent_rate）
- 参考文档：docs/legado-ref/ruleHelp.md + AnalyzeRule.kt 源码

## 6. 书籍格式（远期 4）

| 格式 | Rust 实现 | 状态 |
|---|---|---|
| TXT | 内置（编码检测 encoding_rs） | 计划 |
| EPUB | `zip` + XML | 计划 |
| PDF | `lopdf` | 计划 |
| CBZ | `zip`（图片） | 计划 |
| **MOBI/AZW3** | `mobi` | 计划（新增） |
| FB2 | XML | 计划（新增） |

## 6.5 OPDS 支持（计划）

**目标**：对外提供 OPDS（Open Publication Distribution System）目录——外部阅读器可直接浏览书架、搜索、下载书籍，无需 Web 前端。

| 端点 | 说明 |
|---|---|
| `GET /opds` | OPDS 根目录（书架 → 分类/书籍条目，Atom+XML） |
| `GET /opds/search?q={key}` | OPDS OpenSearch（搜索书架） |
| `GET /opds/books/{id}/download` | acquisition link（正文导出 / 本地书文件下载） |
| `GET /opds/covers/{id}` | 封面（复用 /assets） |

**协议**：OPDS 1.2（Atom + OPDS 扩展命名空间），兼容主流客户端（Legado / KyBook / Apple Books / Calibre）
**认证**：secure 模式支持 Basic（复用 WebDAV 认证逻辑）或 token
**数据源**：books 表（书架）→ 目录条目；正文（ruleContent）→ 下载导出；本地书（TXT/EPUB/MOBI）→ 文件下载
**内容类型**：`application/atom+xml;profile=opds-catalog`、`application/epub+zip` 等

**实现切片**：与 WebDAV 同批（协议服务层），前端无关。

## 7. 产物策略

- **scratch 镜像**（musl 静态编译，系统层 CVE=0）+ **裸静态二进制**（Release 附件 + systemd 示例）
- 前端 rust-embed 内嵌（兼容阶段复用 legacy dist）
- CA 证书/tzdata 内置；数据目录 `storage/` 与 legacy 一致

## 8. 迭代路线（垂直切片）

- [x] 0. 骨架：axum + SQLite 初始化 + /health + /reader3/getBookshelf 占位
- [x] 1. 数据迁移（JSON→SQLite 零丢失：raw_json 保底，真实 169 本/429 源验证）+ login/token
- [x] 2a. getBookSources 真实数据（429 源全量迁移）
- [x] 2b. 规则引擎 v1（CSS/JSONPath/Regex + legado 标志对齐 + 搜索链路：真实搜索 15 条验证）
- [ ] 2c. 规则引擎完整：XPath/JS/内嵌 JS/URL 附加参数/并发率（并行 subagent 推进中）
- [ ] 3. 详情/目录/正文 + 阅读页 API
- [ ] 4. 本地书（TXT/EPUB/PDF/CBZ/MOBI）
- [ ] 5. RSS/TTS/WebDAV/文件管理
- [ ] 6. 多用户管理 + 管理 API 全量对齐
- [ ] 7. 新前端（Vue3+Vite+TS，不复用 legacy 产物——见 docs/FRONTEND.md）
- [ ] 7.5 **OPDS 支持**（新增）：书架/书籍以 OPDS 目录协议暴露——外部阅读器（Legado/KyBook/Calibre 等）直接浏览/搜索/下载
- [ ] 8. musl 交叉编译 + scratch 镜像 + 双形态发布
