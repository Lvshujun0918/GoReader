# reader-dev (Rust) — 架构设计

> 状态：**v5.0.0 已成型**——Rust 重构主功能齐备（书源/阅读/本地书 9 格式/OPDS/WebDAV/反爬/多用户/双轨书仓/备份），
> 剩余待办见 `docs/ROADMAP.md`。版本号以 `Cargo.toml` 为准（当前 `5.0.0`）。
> 本文各节均按当前代码状态核对（2026-08-06）——**未实现的能力不写为已实现**。

---

## 1. 目标（已完成态）

Rust 重构，**API 兼容 + 数据兼容迁移**，远期计划已全部吸收落地：
- ✅ SQLite 兼容迁移（远期 2：JSON→SQLite 自动迁移，raw_json 保底）
- ✅ legado 多规则解析（远期 3：CSS/JSONPath/XPath/Regex/JS 五类规则）
- ✅ 书籍格式扩展（远期 4：9 种本地格式）

## 2. 模块结构

```
src/
├── main.rs        启动/配置（READER_APP_* env）+ 启动 DB 快照（READER_DB_BACKUP）
├── lib.rs         AppConfig（env 解析）/ serve()（axum 组装 + 压缩/缓存中间件挂载）
├── api/           axum 路由（/reader3/*、/opds、/opds-save、/assets/proxy）+ ReturnData
├── middleware/    upload_limit（multipart 上限 413）/ cache_control（静态资源缓存）
├── model/         实体（兼容 legacy：User/Book/BookChapter/BookSource/RssSource...）
├── parser/        legado 多规则引擎（css_chain / js(boa 沙箱) / rule / xpath）
├── service/       业务（search / crawler(SSRF+质询) / explore / local_book(9 格式) /
│                  browser(obscura CDP——唯一浏览器后端) / camoufox(强质询兜底) /
│                  image_cache(跨用户隔离图片缓存) / login(书源登录) / export_book /
│                  debug / cache_job / health / tts / local_sync(双轨书仓) /
│                  schedule(定时任务) / imaging(webp 代理) / opds / epub / rss）
├── storage/       SQLite（迁移/CRUD/书源 cookie/正文缓存/阅读统计）
└── util/          md5 / sha256 / login_limit(登录限流) / db_backup / 编码检测

web-ui/            Vue3 + Vite + Element Plus 全新前端（非 legacy 产物）
scripts/           api-scan.py / mock-*-site.py（质询 mock）/ camoufox_solver.py /
                   69shuba.json（真实书源验证）/ e2e-smoke.py / source-audit.py
tests/             集成测试（cf_solve / turnstile_solve / captcha_matrix /
                   obscura_browser / 69shuba_probe）
```

## 3. API 兼容约定（硬性）

- 路由路径：`/reader3/*` 与 legacy 完全一致
- 返回结构：`{"isSuccess": bool, "errorMsg": string, "data": ...}`（camelCase）
- 认证：`accessToken`（`username:token` query/header），`secure` 模式行为一致
- legacy 已知 bug 清单（重写时修复）：见 `docs/LEGACY-BUGS.md`
- 参数名/语义与 legacy 一致（书源规则格式、搜索参数、分页等）
- **前端**：全新 Vue 3（`web-ui/`），不再复用 legacy 构建产物；构建产物由 `READER_APP_WEB_ROOT` 指向（默认 `web-ui/dist`——目录形态，**未用 rust-embed 内嵌**）

## 4. 数据兼容迁移（已实现）

1. **启动时自动检测**：`storage/reader.db` 不存在且 `storage/data/` 存在 JSON → 触发一次性迁移
2. **迁移前自动备份**：`storage/backup-before-migrate-{ts}/`（原 JSON 完整拷贝）
3. **逐表迁移**：users / books / book_sources / rssSource / book_groups / 用户配置等，按 `user_namespace` 归入；raw_json 保底
4. **迁移校验**：行数核对 + 抽样比对；失败自动回滚（备份恢复）
5. **文件数据不动**：书籍正文/封面/缓存文件路径不变（SQLite 只存引用）
6. **双向兼容**：迁移后 JSON 保留（只读归档）；SQLite 为唯一数据源
7. **回滚路径**：`READER_APP_MIGRATE_SKIP=1` 跳过迁移

## 5. 规则解析引擎（legado 多规则——全量落地）

| 规则类型 | Rust 实现 | 状态 |
|---|---|---|
| CSS Selector | `scraper` | ✅（含 @CSS:/@@ 前缀、a@href 属性、链式选择器） |
| JSONPath | 自实现遍历 | ✅（@Json:/$./$[ 前缀、[*] 通配、{{$.x}} 内嵌） |
| Regex | `regex` + `fancy-regex` | ✅（$N 引用、##替换；**lookbehind 经 fancy-regex 兼容层**） |
| XPath | `sxd-xpath`/`sxd-document` | ✅（`//` 前缀、`{{//xpath}}` 内嵌） |
| JavaScript | `boa_engine`（纯 Rust 沙箱） | ✅（含 `{{}}` 内嵌 JS、`java.*`/`source.*` shim、AES 解密、`java.startBrowserAwait` 浏览器桥） |

对齐 `warpdotsys/legado`（阅读Sigma）的 analyzeRule 语义（参考 docs/legado-ref/）：
- 规则标志：@@ / @CSS: / @XPath: / @Json: / $. / $[ / // / @js:
- 三段/两段式 `##` 拆分 + 替换规则
- URL 附加参数（,{"js":..}/{"bodyJs":..}）与并发率（concurrent_rate）

## 6. 本地书籍格式（9 格式全量）

| 格式 | Rust 实现 | 状态 |
|---|---|---|
| TXT | 内置（编码自动检测 encoding_rs） | ✅ |
| EPUB | `zip` + XML | ✅ |
| PDF | `lopdf`（8MB 解压上限防炸弹） | ✅ |
| MOBI / AZW3 | `mobi` | ✅ |
| FB2 | quick-xml（body/section 分章） | ✅ |
| DOCX | zip + XML（标题样式分章） | ✅ |
| CBZ | zip 图片列表 → 图片页章节 | ✅ |
| UMD | 手写解析（对齐 me.ag2s.umdlib 状态机） | ✅ |

## 6.5 OPDS（已实现）

- **OPDS 1.2**：Atom 导航/分组/分页/OpenSearch/获取/下载/封面
- **OPDS 2.0**：JSON catalog（facets/groups/publications）
- **OPDS-PSE**：进度保存/读取
- 认证：独立 OPDS 账号（sha256+salt）/ 系统用户 Basic / token 三路（详见 SECURITY.md）

## 6.8 网络协议层现状（v5.0.0——如实）

### 出站（书源抓取 / 图片回源）
- **HTTP/1.1 是唯一可用协议**：`reqwest` 依赖**未启用 `http2` feature**（`Cargo.toml`：`rustls-tls, http3, json, gzip, brotli, deflate, cookies, stream`）——书源直连固定 HTTP/1.1（含 HTTPS）。
- `http3` feature 虽在 `Cargo.toml` 启用，但**客户端代码未调用 QUIC 接口**（未使用 `http3_prior_knowledge()` 等，未启用 `reqwest_unstable` cfg）——**实际不会发起 HTTP/3 连接**。
- 计划（未实现）：启用客户端 QUIC 需 `reqwest_unstable` cfg + 传输参数指纹细化（ROADMAP 待办 5）。

### 服务端（对外服务）
- `axum` 监听 **TCP 纯 HTTP**——**无 TLS、无 QUIC**，HTTP/2 / HTTP/3 **暂不提供**；公网 HTTPS 由反向代理终止（nginx/Caddy），H2/H3 如需同样由代理层提供。
- 计划（未实现）：服务端 TLS（设计为 `READER_APP_TLS_CERT`/`READER_APP_TLS_KEY`）+ quinn+h3 双栈监听（ROADMAP 待办 4）。
- **响应压缩已落地**：tower-http `CompressionLayer`（router 内层 + lib.rs 外层兜底，SSE/已编码响应自动跳过）。
- 弱网项：reqwest 连接复用（pool/keep-alive）、crawler 超时分级 + 5xx 重试退避、SSE 流式已实现。

## 6.9 验证码/反爬 bypass 架构（已实现）

### 总览（一页图式）

```
                    ┌────────────────────────────────────────────┐
                    │       书源抓取统一入口 http_fetch          │
                    │   （GET/POST——搜索/目录/正文/探索全走此路）  │
                    └────────────────────────────────────────────┘
                                      │
               ① 会话注入：书源 cookie + 记录 UA（按用户命名空间）
                                      │
                                      ▼
               ② 直连（reqwest **HTTP/1.1**——唯一协议，浏览器头保序）
                                      │
                                      ▼
               ③ 质询检测 is_cloudflare_challenge
                  （503/403 + cf-browser-gesture / challenge-platform /
                    __cf_chl / just a moment / Turnstile 特征）
                                      │
                          命中？──否──→ 正常返回（零开销直连）
                                      │ 是
                                      ▼
        ┌─────────────── 解质询降级链（按序） ──────────────────┐
        │                                                      │
        │  A. 外部 FlareSolverr（仅当配置 FLARESOLVERR_URL）    │
        │     POST /v1 request.get/post（带书源 cookie 数组，  │
        │     保持会话连续性）                                  │
        │                                                      │
        │  B. 进程内 obscura 浏览器（默认——零外部依赖）        │
        │     · 唯一浏览器后端（Chrome/Edge 已移除，无回退）    │
        │     · 发现：READER_OBSCURA_URL（连接既有 CDP 服务，  │
        │       不接管进程）→ READER_OBSCURA_BIN → 同目录 →    │
        │       PATH；不可用则明确报错                         │
        │     · spawn obscura serve --port 随机 --stealth      │
        │       stealth 构建：BoringSSL TLS 指纹模拟 / 反检测  │
        │       / 追踪器拦截 + STEALTH_JS 注入                 │
        │     · 质询分型：经典 CF 质询 → 执行 JS 质询 → 等     │
        │       cf_clearance；Turnstile → widget 检测 → 点击   │
        │       → 读取 cf-turnstile-response                   │
        │                                                      │
        │  C. camoufox 强质询兜底（READER_CAMOUFOX_URL 默认    │
        │     127.0.0.1:8196——Docker 内置 python 后端）        │
        │     Firefox 内核 + 真实指纹预设（navigator/screen/   │
        │     WebGL/字体/canvas 噪声）——69shuba managed        │
        │     challenge 级强质询；READER_CAMOUFOX_FIRST=1 可   │
        │     提前到 CDP 之前；UA 默认 Chrome/131 Win（69shuba │
        │     有 UA 门禁）                                     │
        │                                                      │
        │  全部失败 → 明确报错（不静默吞掉）                   │
        └───────────────────────┬──────────────────────────────┘
                                │
               ④ cookie 按 name 合并（求解结果 ∪ 用户原 cookie）
                  → 按用户命名空间存 book_source_cookies + UA 记录
                                │
                                ▼
               ⑤ 质询重试：原 method/body/headers + 新 cookie 重发
                  （POST 场景关键——浏览器求解只会 GET 首页，重试才能
                   让 POST（如 69shuba search.php 搜索）拿到真实结果）
                                │
                  重试成功？──是──→ 返回真实内容
                  否（仍质询/失败）→ 兜底返回求解 HTML
```

### 关键设计点

1. **零开销直连**：未命中质询特征时完全不经过浏览器——检测先行，日常抓取无浏览器开销。
2. **浏览器后端唯一化**：`service/browser.rs` = **obscura（唯一浏览器后端）**——替代并移除 Chrome/Edge；CDP 兼容；`READER_OBSCURA_URL` 直连既有服务时不 spawn 进程；每用户命名空间独立浏览器实例（防跨用户 cookie 泄漏）。
3. **cookie 复用**：`cf_clearance`/`cf-turnstile-response` 按 name 与用户原 cookie 合并后存库（`book_source_cookies`，按用户命名空间隔离），同源后续请求自动携带——避免重复求解；UA 一并记录（部分站点校验 UA 与 cookie 绑定）。
4. **POST 保真**：求解阶段浏览器只会 GET 首页；必须以**原 method/body/headers + 新 cookie** 重试原请求，才能让 POST 场景（69shuba search.php 等）拿到真实结果；重试仍质询才兜底返回求解 HTML。
5. **真实站点验证**：`scripts/69shuba.json`（真实书源）驱动集成测试（tests/cf_solve.rs / turnstile_solve.rs / captcha_matrix.rs / obscura_browser.rs / 69shuba_probe.rs），另配 mock 站点（mock-cf-site.py / mock-slider-site.py / mock-turnstile-site.py）。
6. **JS 规则共享同一浏览器**：`java.startBrowserAwait(url, title, isForeground)` shim 走与验证码求解相同的浏览器流（含 stealth/UA 覆盖），书源 JS 规则可直接打开页面取 DOM。
7. **降级次序固定**：直连 → FlareSolverr（配置了才启用）→ obscura CDP → camoufox（可 FIRST 提前）→ 明确报错；任一环节解出即返回，不重复消耗。

## 7. 产物策略（当前）

- **Docker 镜像**（多阶段）：
  - 构建阶段：Rust 编译（`rust:1.97-slim`）+ 前端构建（`node:20-slim`）+ obscura release 下载（stealth 构建，amd64/arm64 自动选资产）+ camoufox（`python:3.12-slim` pip 安装 + 浏览器二进制）
  - 运行镜像：**`debian:trixie-slim`（GLIBC 运行时）**——内置 CA 证书、时区（`TZ=Asia/Shanghai`）、python3 + `camoufox_solver.py`、obscura（`/opt/obscura/obscura`）
  - 入口：**tini**（`ENTRYPOINT ["/usr/bin/tini", "--"]` + `CMD ["reader-dev"]`——PID 1 信号转发/僵尸回收，1Panel 等面板兼容）
  - 数据目录 `/data`（`VOLUME`），`READER_APP_WORKDIR=/data`
- **GitHub Release 资产**：`reader-dev-linux-x64-musl`（musl 静态链接——无 glibc 依赖，任意发行版直跑）+ `reader-dev-windows-x64.exe`（`build-windows-signed` job——Authenticode 签名，PFX 来自 secrets，secrets 未配置时拒绝发布未签名 exe）
- 前端**不内嵌**（`web-ui/dist` 目录由 `READER_APP_WEB_ROOT` 指定）
- CI：`rust-ci.yml`（fmt/clippy/test）、`frontend-ci.yml`（vue-tsc 严格类型检查 + vite 构建）、`docker-publish-rust.yml`（`v1.*` 标签触发 + `origin/master` 祖先发版 guard + 多架构镜像 + Windows 签名）

## 8. 迭代路线（现状核对）

- [x] 0. 骨架：axum + SQLite 初始化 + /health + /reader3/getBookshelf
- [x] 1. 数据迁移（JSON→SQLite 零丢失：raw_json 保底，真实 169 本/429 源验证）+ login/token
- [x] 2. 规则引擎全量：CSS/JSONPath/Regex（含 lookbehind）/XPath/JS（boa 沙箱 + shim + 浏览器桥）
- [x] 3. 详情/目录/正文 + 阅读页 API + 新前端（Vue3+Vite+TS，15 视图）
- [x] 4. 本地书 9 格式（TXT/EPUB/PDF/MOBI/AZW3/FB2/DOCX/CBZ/UMD）
- [x] 5. RSS/TTS/WebDAV/文件管理/OPDS 1.2+2.0+PSE
- [x] 6. 多用户管理 + 管理 API 全量对齐（多设备 token/过期/限流/上传上限）
- [x] 7. 反爬（obscura 唯一浏览器后端 + camoufox 兜底 + FlareSolverr 可选 + 真实站点 69shuba 验证）
- [x] 8. 安全审计 6 major 修复（SSRF/跨用户缓存/限流绕过/封面墙/PWA SW/JS 阻塞）
- [x] 9. 双轨书仓 / 备份恢复闭环 / 启动快照 + 每日自动备份 / Docker（trixie + tini + 内置 obscura/camoufox）
- [ ] 剩余项（未实现）：见 `docs/ROADMAP.md`（Windows 签名发布验证 / 69shuba 住宅代理 / argon2 / 服务端 TLS+H2/H3 / zip 炸弹强化 / 多实例 / macOS 资产）
