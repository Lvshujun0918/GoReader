# 安全设计审查（2026-08-03）

审计范围：认证、跨用户隔离、文件系统路径、上传、WebDAV、OPDS、SQL。

## ✅ 已确认安全

### 1. 认证与会话
- `resolve_namespace`：secure 模式下所有业务接口必须携带 `accessToken=username:token`，严格比对 `users.token`（`find_user` + 等值校验），不匹配即 `login_required`。
- **命名空间不可由参数覆盖**：namespace 恒来自 token 解析出的用户名——传其他用户名的 token 无法访问他人书架/数据。
- token 存储于 users 表（单会话；`reset_user_password` 会清空 token 强制下线）。
- WebDAV：Basic 认证 → `gen_encrypted_password` 校验 + `enable_webdav` 门控；home 严格限定 `storage/data/{user}/webdav`。

### 2. 文件系统路径（防穿越）
- `files.rs`：
  - `home` 参数白名单：`__HOME__` / `__WEBDAV__` / `__LOCAL_STORE__` / `__STORAGE__`（后者需 manager）/ 空——其余一律"非法访问"。
  - `resolve_secure_path`：组件级归一化（`..` 逐级弹出，越出 base 即拒绝）+ `starts_with(base)` 最终校验——所有 list/save/mkdir/delete/upload/download 均走此函数。
  - upload 文件名只取 `basename`（`../x`、`a/b` 收敛为安全名）+ 跳过隐藏文件。
- `resolve_storage_path`（本地书解析）：`canonicalize` + `starts_with(storage_dir)` 校验。
- `opds.rs` 下载/获取同样经白名单路径解析。

### 3. SQL 注入
- 全部使用 sqlx 参数化绑定（`?1/?2/...`），无字符串拼接 SQL。

### 4. 上传
- 文件名/路径均收敛（见上）；书源导入仅 JSON 白名单字段；本地书导入只做解析不入壳执行（EPUB zip 解包 + TXT 编码检测）。

### 5. 书源 cookie 按用户隔离
- `book_source_cookies` 表：`user_namespace + source_url` 联合主键——书源登录态（cookie/user_agent）严格按用户命名空间存取，`cookie_for(ns, url)` 只读本命名空间行，跨用户不可见、不可覆盖。
- 抓取入口（`crawler::fetch_book`）按当前请求命名空间注入 cookie；FlareSolverr 返回的 cookie 与用户原 cookie **按 name 合并**后仍存回该用户命名空间。

### 6. FlareSolverr 转发
- 仅当环境变量 `FLARESOLVERR_URL` 配置时才启用（默认禁用，零外部依赖）。
- 仅书源抓取（`fetch_book`）命中 Cloudflare 质询特征（503 + 特征 HTML）时转发；RSS/TTS 等原始抓取（`fetch`/`fetch_get`）不经 FlareSolverr。
- 转发请求携带当前用户的 cookie（保持书源会话连续性），响应 cookie 按 name 合并后按用户存库，UA 一并记录。

## 🔧 本轮已修复

### token 生成（可预测 → 随机）
- 原实现：`token = md5(username + now_millis)`——时间戳可猜测，存在 token 伪造风险。
- 现实现：`uuid::Uuid::new_v4()` 随机 token（32 位十六进制，不可预测）。
- 旧 token 因存于 DB 不受影响；下次登录即换新随机 token。

## 🔒 新增安全项

### 登录限流（GAP 61）
- 失败 5 次 → 锁定 5 分钟（用户名 + IP 双键计数，`x-forwarded-for`/`x-real-ip` 优先；无代理头时退化为按用户名计数）；成功登录清零。
- 覆盖登录/注册入口：锁定中直接拒绝（「尝试过多请稍后」），不泄露用户是否存在。
- 纯内存计数（进程内有效）——单实例部署无需配置；多实例部署需前置外部限流（见已知限制）。

### 多设备 token（GAP 59）
- 每次登录生成新 `uuid v4` token 并追加到 `users.token_map`（每用户上限 5 个并存会话，超出淘汰最旧）；任一 token 均可通过 `resolve_namespace` 认证。
- 设备间互不干扰：登出/改密只清除当前设备 token（`reset_user_password` 清空全部 token 强制全线登出）。
- 配合 **token 过期**（GAP 118）：`users.last_login_at` + `READER_TOKEN_TTL_DAYS`（默认 30 天）——过期 token 拒绝访问，需重新登录；legacy 迁移数据（last_login_at=0）同样按过期处理。

### 上传限制（GAP 62）
- multipart 上传统一上限：`READER_UPLOAD_MAX_MB`（默认 100MB）——覆盖书籍/文件上传、备份恢复、图片等所有 multipart 入口；超限返回 413 + 明确错误提示（含环境变量名），不再依赖代理层限流兜底。
- 与既有防护叠加：文件名 basename 收敛 + 路径白名单 + 本地书解析只读不入壳。
- 注意：正文/封面等非 multipart 接口不受此限制（数据量小）；超大文件仍建议代理层 `client_max_body_size` 与上传上限匹配（见 README 部署节）。

## ⚠️ 已知限制（legacy 兼容保留，建议缓解）

1. **密码哈希为 MD5 双层**（`md5(md5(pw+salt)+salt)`）——legacy 算法兼容，旧密码不重设无法换算法。缓解：HTTPS + 强密码策略；未来可加 bcrypt 并存迁移（登录取新哈希升级）。
2. **accessToken 走 URL query**——可能进入代理/访问日志。缓解：部署 HTTPS（服务端已支持 TLS 配置可查 README）。
3. ~~登录无速率限制~~ **已解决**：登录限流（用户名+IP 失败 5 次锁 5 分钟）已内置（见上）；多实例部署时内存计数不跨进程，仍需前置反向代理限流。
4. ~~multipart 上传无大小上限~~ **已解决**：`READER_UPLOAD_MAX_MB`（默认 100MB）统一限流（见上）。
5. **EPUB zip 解压无条目大小/数量限制**——zip 炸弹防护缺失；已由上传上限（同 4）缓解，公网建议保持代理层限制。

## OPDS 安全（已实现）

### 认证
- 非 secure 模式：恒走 `default` 命名空间。
- secure 模式：Basic（**独立 OPDS 账号优先** → 系统用户账号）或 `accessToken=username:token`（与 /reader3 同套校验）。

### 独立 OPDS 账号（sha256 + salt）
- 存储于 `system_settings` 键值表（`opds_account`），格式 `{salt}${sha256_hex(salt || password)}`（`util::sha256`：16 字节随机盐，非明文）。
- 与系统用户（legacy 双 md5 兼容哈希）分离：配置后仅用于 OPDS Basic 认证，不产生系统用户、不占用户配额。
- 认证顺序：独立账号（sha256 校验）→ 系统用户（`gen_encrypted_password` 双 md5 校验）→ accessToken。

### 路径/下载
- 复用白名单路径解析，不越出用户存储目录。
