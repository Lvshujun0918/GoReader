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

## ⚠️ 已知限制（legacy 兼容保留，建议缓解）

1. **密码哈希为 MD5 双层**（`md5(md5(pw+salt)+salt)`）——legacy 算法兼容，旧密码不重设无法换算法。缓解：HTTPS + 强密码策略；未来可加 bcrypt 并存迁移（登录取新哈希升级）。
2. **accessToken 走 URL query**——可能进入代理/访问日志。缓解：部署 HTTPS（服务端已支持 TLS 配置可查 README）。
3. **登录无速率限制**——本地单机规模可接受；公网部署建议前置反向代理限流。
4. **multipart 上传无大小上限**——本地服务可接受；公网建议代理层限制（如 nginx client_max_body_size）。
5. **EPUB zip 解压无条目大小/数量限制**——zip 炸弹防护缺失；本地导入可接受，公网建议限制上传大小（同 4）。

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
