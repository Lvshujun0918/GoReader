<div align="center">

# Reader Dev

**自托管 Web 阅读服务（v1.0.0） —— 书源搜索 · 本地书仓 · OPDS · WebDAV · 多用户**

Rust + Vue 3 实现的现代化阅读服务器，API 与数据兼容 legacy 分支（Kotlin），支持多书源规则引擎、7 种本地书格式、OPDS 1.2/2.0 与 PSE、书源登录与反爬（内嵌浏览器 + FlareSolverr 能力）、多用户隔离。

</div>

---

## ✨ 功能特性

### 📚 书源与抓取
- **规则引擎**：legacy/legado 双命名规则（`ruleSearch`/`searchRule`、`ruleToc`/`tocRule` 等）——CSS 链式选择器、JSONPath、XPath、Regex、JS（boa 沙箱，含 `java.*`/`source.*` shim 与 AES 解密）；**正则 lookbehind**（`(?<=…)` 规则主体与 `##` 替换段经 fancy-regex 兼容层）
- **JS shim（完整 legacy 集）**：`java.ajax` / **`java.startBrowserAwait`**（内置浏览器加载页面并等待完成——走与验证码求解同一浏览器流）/ `setContent` / `getString` / `getElements` / `getWebViewUA` / `encodeURI` 等
- **书源管理**：增删改（规则字段 JSON 编辑）、启停、分组、失效检测 + **失效源一键禁用**、本地/远程导入、导出、订阅源、**header（JSON）/loginUrl/cookie 编辑**
- **书源调试**：搜索/目录/正文逐规则逐步日志（SSE 流式）
- **书源登录**：`loginUrl` 登录流 + `loginCheckJs` 校验 + Set-Cookie 按用户合并；图片验证码截图回填；**CDP 浏览器自动登录**（表单/滑块贝塞尔轨迹/cookie 提取——obscura 反检测浏览器后端）
- **反爬（验证码/CF 质询 bypass）**：Cloudflare 质询检测（503/403 + 特征 HTML）→ **进程内 obscura 浏览器求解**（stealth 构建：BoringSSL TLS 指纹模拟/反检测/追踪器拦截 + **质询重试**（原 method/body/headers + 新 cookie）+ cookie 按 name 合并复用——无需外部容器）或可选外部 `FLARESOLVERR_URL`；**Turnstile 验证码**（widget 检测 → 自动点击 → 读取 `cf-turnstile-response`）；**69shuba 等真实书源实测**（scripts/69shuba.json）
- **相关推荐**：`ruleRelated` 详情页相关书籍（同 ruleExplore 规则风格）；**探索源扩充**（内置探索源清单，与 bookSource.json 同构——可直接导入）
- **换源**：并发多源搜索 + 书名过滤去重，一键切换

### 📖 阅读体验
- 阅读器：字体（12 档 + 离线网络字体）、行距/段距/字重/宽度/字距/缩进/对齐、主题（浅色/深色/纸色 + 界面深色独立 + **纸纹**（噪点纹理开关））、**滑动/滚动双翻页模式**、自动阅读、进度服务端同步、书签、划词（复制/搜索 + **划词朗读**）、预加载、TTS（Edge TTS + HttpTTS 音色/倍速 + **音色列表 10 分钟缓存**）、**亮度滑条、键盘翻页、快捷键（书架 g/s/r）、目录当前章高亮、顶部细进度条（点击跳章）、卷标题分隔**
- **全局简繁转换**（自动检测/简/繁三态）、阅读偏好 12 项云端同步（多端一致）
- **续读**：详情页按书架进度显示「续读 第 N 章」一键继续；**复制本章**（剪贴板全文）；**正文图片代理**（`/assets/proxy?fmt=webp&q=80`——webp 转码省流量，失败回退原图透传）
- 整书缓存（后台并发 + SSE 进度 + 取消）、正文缓存、缓存管理、**全书内容搜索**（含文件型本地书）
- 阅读统计（时长/字数累计）、书架排序/分组折叠/**分组拖拽排序**/网格密度/进度角标/**置顶**/最近阅读

### 📁 本地书（7 格式）
EPUB · TXT · MOBI · AZW3 · PDF · FB2 · DOCX —— 上传导入（含预览）、目录、正文、重扫、全书搜索、**元数据编辑（书名/作者/标签/简介）**、OPDS 下载统一分派（PDF 8MB 解压上限防炸弹、DOCX 标题样式分章、TXT 编码自动检测）

- **双轨同步仓**：文件与 DB 双向对账——书仓目录（`storage/data/{user}/books/` + 可选 `READER_LOCAL_BOOK_DIR`）由后台文件监听（300ms 去抖批量）自动同步：新文件自动导入、修改自动重扫、删除保留书籍与进度、仅 DB 书自动生成 epub 落仓（幂等，无事件循环）
- **迁移工具**：legacy `loc_book` 文件书一键迁入 DB（`migrateLocBook`——保留原记录/阅读进度，章节转 DB 直读）

### 📤 导出与备份
- 书籍导出：TXT（**编码可选 utf-8/gbk/gb2312/gb18030**）/ EPUB / HTML；**书源书导出并发抓章**（并发 4，错误章跳过不中断）；书源导出
- **备份恢复闭环**（`backupToWebdav` + `restoreFromZip`/`restoreFromWebdav`——9 类目幂等恢复，兼容 legacy 备份布局）
- **自动备份**：启动时 DB 快照（`reader.db.bak-{ts}`，保留 5 份，`READER_DB_BACKUP=0` 禁用；WAL checkpoint 保证一致）+ 每日定时备份（`READER_AUTO_BACKUP_HOUR` 默认 03:00，保留 7 份）

### 🌐 OPDS & WebDAV
- **OPDS 1.2**（Atom 导航/分组/分页/OpenSearch/获取/下载/封面）+ **OPDS 2.0**（JSON catalog：facets/groups/publications）+ **OPDS-PSE**（进度保存/读取）
- **独立 OPDS 账号**（sha256+salt）或系统账号 Basic / token 三路认证
- **WebDAV 服务器**（**OPTIONS 预检**（DAV 1,2 + Allow 头，不校验认证——客户端兼容）/PROPFIND/GET/PUT/MKCOL/DELETE/MOVE/COPY/LOCK/UNLOCK——路径穿越防护）——可作为 Calibre/坚果云客户端存储

### 👥 多用户与安全
- 多用户注册/登录：token 随机化（uuid v4）+ **多设备会话**（每用户最多 5 个 token 并存，互不干扰）+ **登录限流**（用户名+IP 失败 5 次锁 5 分钟）+ **token 过期**（`READER_TOKEN_TTL_DAYS` 默认 30 天）；用户权限开关（WebDAV/本地书仓/书源/RSS）、用户管理
- 命名空间隔离（书架/书源 cookie/配置/文件全部按用户）、路径穿越防护（白名单 + 组件级归一化）、SQL 全参数化、OPDS/WebDAV 独立认证、**上传大小上限**（`READER_UPLOAD_MAX_MB` 默认 100MB，超限 413 明确错误）
- 安全设计详见 [`docs/SECURITY.md`](docs/SECURITY.md)

### 🎨 前端（web-ui）
Vue 3 + Vite + Element Plus，A 版极简风格（近白底/细字重/强调色/圆角），响应式移动端适配、深色主题、虚拟滚动书架、SSE 流式搜索/调试/缓存进度；**骨架屏（shimmer 扫光）、移动端下拉刷新、搜索加载更多/热词/历史、快捷加入书架、跨书书签汇总、探索书单收藏、记住我（token 存 sessionStorage）、404 页、全局快捷键、元数据编辑、书源 header 编辑、分组拖拽、置顶、纸纹**；**前端 CI**（GitHub Actions：web-ui 路径触发 vue-tsc 严格类型检查 + vite 构建）

---

## 🚀 快速开始

### 本地运行（Windows/Linux/macOS）

```bash
# 1. 构建后端（Rust 1.75+）
cargo build --release

# 2. 构建前端
cd web-ui && npm install && npm run build && cd ..

# 3. 运行
export READER_APP_WORKDIR="$PWD/data"     # 数据目录（可选）
export READER_APP_SECURE=true             # 开启多用户安全模式（可选）
./target/release/reader-dev
```

浏览器打开 `http://localhost:8080`（前端/API/封面/OPDS/WebDAV 同端口）。

> 非 secure 模式单用户（default）；secure 模式注册/登录后使用。

### Docker（推荐——镜像内置 obscura 反检测浏览器，验证码/CF 质询浏览器流可直接使用）

```bash
# 构建（多阶段：Rust 编译 + 前端构建 + obscura release 下载 + 运行镜像（debian））
docker build -t reader-dev .

# 运行（数据持久化到 ./data）
docker run -d --name reader-dev -p 8080:8080   -v "$PWD/data:/data"   -e READER_APP_WORKDIR=/data   -e READER_APP_SECURE=true   reader-dev
```

> 容器内验证码路径：**内置 obscura**（`READER_OBSCURA_BIN=/opt/obscura/obscura`——release stealth 构建，构建期从 GitHub Releases 下载，amd64/arm64 自动选资产）→ 登录表单/滑块自动处理、CF 质询进程内求解；无需外部 FlareSolverr 容器（如需仍可 `FLARESOLVERR_URL` 指向 sidecar）。

---

## ⚙️ 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `READER_SERVER_PORT` | `8080` | 服务端口 |
| `READER_APP_WORKDIR` | 当前目录 | 工作目录（数据在 `{workdir}/storage`） |
| `READER_APP_WEB_ROOT` | `web-ui/dist` | 前端静态资源根（SPA fallback） |
| `READER_APP_SECURE` / `READER_APP_SECUREKEY` | 关 | 多用户安全模式（secureKey 保护管理接口） |
| `READER_APP_MINUSERPASSWORDLENGTH` | `8` | 注册密码最小长度 |
| `READER_APP_USERLIMIT` | `500000` | 用户数上限 |
| `READER_APP_INVITECODE` | 空 | 注册邀请码（配置后注册必须） |
| `READER_LOG_DIR` | 空（仅控制台） | 日志目录（控制台 + 文件，按大小轮转 10MB×7） |
| `READER_LOG_MAX_SIZE_MB` / `READER_LOG_MAX_FILES` | `10` / `7` | 日志轮转参数 |
| `READER_OBSCURA_BIN` | 自动发现 | obscura 可执行文件路径（浏览器流唯一后端；默认探测：同目录 → 系统 PATH） |
| `READER_OBSCURA_URL` | 空 | 连接既有 obscura CDP 服务（ws:// 或 http:// 端点；配置后不再 spawn 进程） |
| `FLARESOLVERR_URL` | 空（内嵌求解） | 可选外部 FlareSolverr 服务地址 |
| `READER_UPLOAD_MAX_MB` | `100` | 上传大小上限（MB，multipart 导入/文件上传/备份恢复；超限返回明确错误） |
| `READER_TOKEN_TTL_DAYS` | `30` | token 过期天数（基于最近登录时间；过期需重新登录） |
| `READER_LOCAL_BOOK_DIR` | 空 | 双轨书仓附加目录（文件监听 + 对账；secure 模式下归 default 命名空间） |
| `READER_DB_BACKUP` | `1` | 启动数据库快照开关（`0` 禁用；保留最近 5 份 `reader.db.bak-*`） |
| `READER_AUTO_BACKUP_HOUR` | `3` | 每日自动备份小时（0-23；备份到 `webdav/legado/auto-YYYYMMDD.zip`，保留 7 份） |

> **配置生效时机**：所有环境变量在服务启动时读取一次——修改后需**重启进程**生效（无运行时热加载）。

---

## 📦 部署说明

### 书源浏览器流依赖（obscura——唯一浏览器后端）
- **obscura**：Rust headless 浏览器（stealth 构建——BoringSSL TLS 指纹模拟/反检测/追踪器拦截；CDP 兼容）。下载：GitHub Releases 的 `-stealth` 资产（`obscura-x86_64-linux-stealth.tar.gz` / `obscura-aarch64-linux-stealth.tar.gz` / `obscura-x86_64-windows-stealth.zip`）
- **Windows**：解压后设置 `READER_OBSCURA_BIN` 指向 `obscura.exe`（或放入 PATH）
- **Linux/容器**：下载解压到任意目录并设置 `READER_OBSCURA_BIN`；Docker 镜像已内置（`/opt/obscura/obscura`）
- **既有服务**：已运行 `obscura serve --port 9222 --stealth` 时，配置 `READER_OBSCURA_URL=http://127.0.0.1:9222` 直连复用（不再 spawn 进程）
- 无浏览器时：登录/CF 质询降级为明确报错 + 手动 Cookie 粘贴

### 反爬（Cloudflare）
- **默认内嵌求解**：检测到 CF 质询自动用本机浏览器执行 JS 质询并提取 `cf_clearance`（无需外部服务）
- 可选：部署 FlareSolverr（Docker）并通过 `FLARESOLVERR_URL` 指定

### 本地书双轨书仓
- **默认书仓目录**：`storage/data/{user}/books/`（secure 按用户隔离；非 secure 为 `default`）——放入文件（EPUB/TXT/MOBI/AZW3/PDF/FB2/DOCX）自动导入书架，修改自动重扫，删除保留书籍与进度
- **附加目录**：`READER_LOCAL_BOOK_DIR`（可选，同样被文件监听 + 对账；secure 多用户模式下统一归 `default` 命名空间）
- 仅 DB 书（无文件关联）自动生成 epub 落书仓（文件名 `{书名}.epub`，冲突加后缀）

### 上传限制 / token 过期 / 迁移备份
- **上传限制**：`READER_UPLOAD_MAX_MB`（默认 100MB）——multipart 导入/文件上传/备份恢复统一上限，超限返回 413 明确错误；反向代理 `client_max_body_size` 需匹配（见上）
- **token 过期**：`READER_TOKEN_TTL_DAYS`（默认 30 天）——基于用户最近登录时间，过期需重新登录（前端自动跳登录页）
- **迁移备份**：启动时自动快照 `storage/reader.db` → `reader.db.bak-{YYYYMMDDHHMMSS}`（保留最近 5 份，WAL checkpoint 保证一致；`READER_DB_BACKUP=0` 禁用）

### 反向代理（HTTPS）
服务端为 HTTP；公网部署建议前置 nginx/Caddy 终止 TLS：

```nginx
server {
    listen 443 ssl;
    server_name reader.example.com;
    client_max_body_size 200m;              # 上传大文件（与 READER_UPLOAD_MAX_MB 匹配）
    location / { proxy_pass http://127.0.0.1:8080; proxy_buffering off; }  # SSE
}
```

### 单实例假设
服务以**单实例**运行为前提：SQLite 数据文件 + 内存态缓存（目录/正文/语音列表/登录限流）不做跨进程协调；
请勿对同一数据目录启动多个进程（多副本部署请按实例拆分数据目录）。

### HTTP/3（QUIC）说明
- 客户端：`reqwest` 已启用 `http3` 特性——**出站请求（书源抓取等）支持 HTTP/3**（服务可用时自动协商）。
- 服务端：监听为 TCP（`axum::serve`）——**入站 QUIC/HTTP3 未启用**；如需对公网提供 HTTP/3，请在反向代理（如 Caddy/nginx quic）处终止 QUIC 并回源 HTTP/1.1。

---

## 🔄 从 legacy（Kotlin）Docker 迁移到 Rust 版

### 方式一：只换镜像（数据零改动）
```bash
# 1. 备份（保险）
docker exec <旧容器> tar czf /tmp/backup.tar.gz /storage
docker cp <旧容器>:/tmp/backup.tar.gz .

# 2. 停旧容器（保留数据卷）
docker stop <旧容器>
# 数据卷不动（挂载路径保持不变——默认 /storage）

# 3. 起新容器（同一数据卷）
docker pull ghcr.io/warpdotsys/reader-dev:latest
docker run -d --name reader-dev-rust   -v <同一数据卷>:/storage   -p 8080:8080   -e READER_APP_WORKDIR=/storage   -e READER_APP_SECURE=true   ghcr.io/warpdotsys/reader-dev:latest

# 4. 启动时自动迁移（JSON → SQLite——书/书源/书签/替换规则/TXT 规则/HttpTTS/分组/用户配置/RSS 全量，raw_json 保底）
#    原 JSON 文件保留在 storage/data/（不删除——可回退）
docker logs -f reader-dev-rust   # 看到「JSON→SQLite 迁移完成」即成功
```

### 方式二：直接跑 Rust 二进制
```bash
# Linux（release 资产下载 reader-dev-linux-x64）
chmod +x reader-dev-linux-x64
READER_APP_WORKDIR=/storage READER_APP_SECURE=true ./reader-dev-linux-x64
# 首次启动同样自动迁移 legacy JSON 数据
```

### 迁移覆盖
| 数据 | 说明 |
|---|---|
| 用户 / 书架（含进度）/ 书源 / RSS | ✅ 全量 |
| 书签 / 替换规则 / TXT 目录规则 / HttpTTS / 分组 / 用户配置 | ✅ 全量（本次补全） |
| 原 JSON 文件 | 保留（可回退） |

### 直接跑 Rust 的能力说明（分层）
- **本地直跑**：obscura stealth 构建（`READER_OBSCURA_BIN` 指向下载的二进制——Windows/Linux/macOS release 资产）——覆盖 CF JS 质询/Turnstile 基础/滑块
- **Docker 镜像**：内置 obscura（stealth）+ python + camoufox（强质询兜底——69shuba 级）
- **强质询边界**：数据中心 IP 会被 Turnstile 风控（400030）——需住宅代理（camoufox 支持 per-context proxy，配置 `READER_PROXY_URL` 或书源代理字段）

## 📖 使用指南

- **书源导入**：书源管理 → 本地导入 / 远程导入 / 订阅源；调试器逐规则排查；失效检测后一键禁用
- **双轨书仓**：直接把书文件放入书仓目录（或 `READER_LOCAL_BOOK_DIR`）自动入架；`migrateLocBook` 迁移 legacy 文件书
- **OPDS 接入**：外部阅读器（Legado/静读天下）添加 `http://host:port/opds`（可配独立账号，设置页查看并复制）
- **WebDAV**：`http://host:port/reader3/webdav/`（系统用户 Basic 认证，需开启 WebDAV 权限）
- **备份/恢复**：设置页 → 备份到 WebDAV / 下载备份；恢复通过 `POST /reader3/restoreFromZip`
- **全书搜索**：详情页 → 全书搜索（本地书，含文件型）；书架搜索可切「全书」范围

---

## 🧑‍💻 开发

```bash
cargo test        # 后端测试（370+，含 CF 求解/Turnstile/OPDS/本地格式集成测试）
cd web-ui && npm run build   # 前端（vue-tsc 类型检查 + vite）
```

### 项目结构

```
src/
├── api/          # axum 路由（/reader3/*、/opds、/opds-save）
├── model/        # 数据模型（book/book_source/rss/user/...）
├── parser/       # 规则引擎（css_chain / js / rule / xpath）
├── service/      # 业务（search/crawler/explore/local_book/epub/opds/rss/
│                 #   browser(CDP 专精浏览器)/login(书源登录)/export_book/debug/
│                 #   cache_job/health/tts/local_sync(双轨书仓)/schedule(定时任务)/
│                 #   imaging(webp 代理)）
├── storage/      # SQLite（迁移/CRUD/书源 cookie/正文缓存/阅读统计）
└── util/         # md5/sha256/login_limit(登录限流)/db_backup(启动备份)
web-ui/src/
├── views/        # 13 个视图（书架/阅读/搜索/详情/探索/书源/文件/用户/规则/RSS/设置/登录/404）
├── components/   # LogoMark / ErrorBoundary
├── api/          # 后端接口封装（27 个模块）
└── utils/        # chinese(简繁)/uiTheme/readerConfig/download/...
scripts/          # api-scan.py（API 扫描）/ mock-cf-site.py / mock-slider-site.py /
                  # mock-turnstile-site.py（质询 mock）/ 69shuba.json（真实书源验证）
tests/            # 集成测试（cf_solve / turnstile_solve / captcha_matrix 等）
.github/workflows/ # rust-ci.yml / docker-publish.yml / frontend-ci.yml（前端 CI）
docs/             # SECURITY.md / ARCHITECTURE.md / legado-ref/
```

---

## 🧪 测试

- 后端 **370+** 单测与集成测试（规则引擎/存储迁移/OPDS XML/本地书 7 格式解析/CF 质询端到端/Turnstile/WebDAV 全方法）
- `scripts/api-scan.py`：API 全接口扫描（正常/空参/错参/边界）；`mock-cf-site.py` / `mock-slider-site.py` / `mock-turnstile-site.py`：验证码质询 mock 站点
- 前端 `vue-tsc --noEmit` 严格类型检查 + vite 构建（GitHub Actions 前端 CI 自动执行）

---

## 💬 交流

Telegram 群：[t.me/readerdev](https://t.me/readerdev)（使用反馈 / 书源交流）

## 📚 文档

- [`docs/SECURITY.md`](docs/SECURITY.md) —— 安全设计审查
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) —— 架构设计
- [`docs/ROADMAP.md`](docs/ROADMAP.md) —— 路线图
- [`docs/legado-ref/ruleHelp.md`](docs/legado-ref/ruleHelp.md) —— 书源规则参考（legado）

---

## 📌 分支说明

| 分支 | 说明 |
|---|---|
| `master` | **Rust 重构版（当前）**——本文档 |
| `legacy` | Kotlin 稳定版（ghcr.io/warpdotsys/reader-dev:latest / v4.x 双平台镜像） |

---

## 📄 License

[GNU General Public License v3.0](LICENSE) (GPL-3.0)
