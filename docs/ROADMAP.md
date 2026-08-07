# Reader-dev 路线图（Roadmap）

> 状态：**Go 重构迁移中（v5.0.0）**——后端已迁移 Go（gin+gorm），核心 API/存储/规则引擎完成；
> 本文档已完成区为 Rust 版成果记录，Go 端完成度见 `docs/ARCHITECTURE.md`。
> 更新日期：2026-08-07。版本号以 `go.mod` 为准（当前 `5.0.0`）。
> 原则：**只列已实现/已确认的事实**；未实现项一律标注「计划/未实现」。

---

## ✅ 已完成（v5.0.0）

### Go 重构主体
- [x] gin + SQLite 服务端（`/reader3/*` API 与 legacy 兼容，ReturnData 结构一致，gorm 17 张表）
- [x] JSON→SQLite 自动迁移（检测/备份/逐表/校验回滚/JSON 保留只读归档/raw_json 保底）
- [x] 多用户：注册/登录、权限开关（WebDAV/本地书仓/书源/RSS）、用户管理
- [x] 多设备 token（uuid v4，每用户 5 个并存）+ token 过期（`READER_TOKEN_TTL_DAYS` 默认 30 天）

### 书源与阅读
- [x] legado 多规则引擎全量：CSS / JSONPath / XPath / Regex（fancy-regex lookbehind）/ JS（boa 沙箱 + `java.*`/`source.*` shim + AES）
- [x] 书源管理（增删改/启停/分组/失效检测/导入导出/订阅/header+loginUrl+cookie 编辑）、书源调试（SSE 逐步日志）
- [x] 换源（并发多源 + 书名去重 + 弹层书源名过滤/手动刷新，SSE 流式）
- [x] 阅读器全套（翻页模式/主题/纸纹/简繁/预加载/TTS/快捷键/进度同步/划词朗读/复制本章…）
- [x] 整书缓存、全书内容搜索、阅读统计、书架分组拖拽/置顶/封面墙（三态）
- [x] 主页搜索框 = 全网搜书入口（回车跳搜索页）

### 本地书 / 协议 / 数据
- [x] 本地书 **9 格式**：EPUB/TXT/MOBI/AZW3/PDF/FB2/DOCX/CBZ/UMD
- [x] 双轨书仓（文件监听 300ms 去抖 + DB 对账；`READER_LOCAL_BOOK_DIR`）+ `migrateLocBook` 迁移工具
- [x] OPDS 1.2 / 2.0 / PSE + 独立 OPDS 账号（sha256+salt）
- [x] WebDAV 服务器（OPTIONS 预检/PROPFIND/GET/PUT/MKCOL/DELETE/MOVE/COPY/LOCK/UNLOCK）
- [x] 备份恢复闭环（backupToWebdav / restoreFromZip / restoreFromWebdav）+ 启动快照（保留 5 份）+ 每日自动备份（`READER_AUTO_BACKUP_HOUR` 默认 03:00，保留 7 份）

### 反爬 / 安全
- [x] **obscura 反检测浏览器集成**（唯一浏览器后端——替代 Chrome/Edge，无回退；stealth 构建：BoringSSL TLS 指纹/反检测/追踪器拦截；`READER_OBSCURA_URL` 直连或 spawn）
- [x] camoufox 强质询兜底（Firefox 内核真实指纹，HTTP 后端；`READER_CAMOUFOX_URL/DISABLE/FIRST/UA`）+ FlareSolverr 可选
- [x] CF 质询/Turnstile 求解 + POST 保真重试 + cookie 按 name 合并复用 + 真实书源 69shuba 实测
- [x] 安全审计 6 个 major 全修（2026-08-06，提交 `e5f12b4`）：SSRF 逐跳校验 / 图片缓存跨用户隔离 / 登录限流直连 IP（XFF 忽略）/ 封面墙 / PWA SW v2 / JS 桥超时 10s
- [x] 上传上限（`READER_UPLOAD_MAX_MB` 默认 100MB，413 明确错误）

### 工程
- [x] 新前端（Vue3 + Vite + shadcn-vue，15 视图，vue-tsc 严格类型检查 CI）
- [x] CI：go-ci（vet/test-race/静态校验/交叉编译）、frontend-ci、docker-publish-go（`v5.*` 标签触发 + master 祖先 guard + 多架构镜像）
- [x] Docker 镜像：`debian:trixie-slim`（GLIBC）+ **tini 入口**（1Panel 兼容）+ 内置 obscura/camoufox/python + CA/时区（Go 静态二进制，golang:1.25 构建）
- [x] Release 资产：`reader-dev-linux-x64`（纯 Go 静态）+ `reader-dev-windows-x64.exe`（Go 交叉编译）
- [x] 后端 Go 单测（配置/规则引擎/存储/密码）

---

## ⏳ 剩余待办（未实现——如实标注）

| # | 项 | 状态与说明 |
|---|---|---|
| 1 | **Windows exe 未签名** | Go 原生交叉编译产出，未签名（SmartScreen 提示属预期）；如需签名需引入签名服务 |
| 2 | **69shuba 住宅代理验证** | 数据中心 IP 被 Turnstile 风控（实测 `400030` 环境风控——与 UA/指纹无关）；代理配置为设计方向，未实现 |
| 3 | **服务端 TLS + HTTP/2/3** | 计划（未实现）。当前服务端纯 HTTP（TCP 监听，无 TLS/QUIC）；设计：`READER_APP_TLS_CERT`/`READER_APP_TLS_KEY` |
| 4 | **客户端 QUIC（HTTP/3）** | 计划（未实现）。Go `net/http` 默认 HTTP/1.1（TLS ALPN 可协商 HTTP/2）；QUIC 不启用 |
| 5 | **EPUB zip 炸弹防护** | 条目大小/数量上限缺失；当前受 `READER_UPLOAD_MAX_MB` 缓解 |
| 6 | **多实例部署支持** | 单实例假设：SQLite + 内存态缓存（目录/正文/登录限流）不跨进程协调 |
| 7 | **macOS 发布资产** | CI 交叉编译矩阵已验证 darwin/arm64 构建，Release 暂未发布 mac 资产 |
| 8 | **本地书导入（9 格式）** | Go 端 `importBookPreview`/`uploadLocalBook` 等为骨架，EPUB/TXT/MOBI 解析迭代中 |
| 9 | **浏览器自动化（obscura/camoufox）** | Go 端质询求解/书源登录浏览器流未接入（Rust 版已实现，按 ARCHITECTURE 迭代） |

---

## 开发与发布策略（当前）

- **分支布局**：`master` = Go 重构发布主线（本文档）；`legacy` = Kotlin 稳定版（v4.x，ghcr.io/warpdotsys/reader-dev:latest）
- **发布工作流**（`docker-publish-go.yml`）：`v5.*` 标签触发 + 发版 guard（要求触发 SHA 为 `origin/master` 祖先，防止误发）+ 多架构镜像推送 + GitHub Release 资产
- **版本号**：以 `go.mod` 为准（当前 `5.0.0`）
- 许可策略：**永久不做用户/功能限制**（`READER_APP_USERLIMIT` 等 env 默认宽松）
