<div align="center">

# Reader Dev

**自托管 Web 阅读服务 —— 书源搜索 · 本地书仓 · OPDS · WebDAV · 多用户**

Rust + Vue 3 实现的现代化阅读服务器，API 与数据兼容 legacy 分支（Kotlin），支持多书源规则引擎、7 种本地书格式、OPDS 1.2/2.0 与 PSE、书源登录与反爬（内嵌浏览器 + FlareSolverr 能力）、多用户隔离。

</div>

---

## ✨ 功能特性

### 📚 书源与抓取
- **规则引擎**：legacy/legado 双命名规则（`ruleSearch`/`searchRule`、`ruleToc`/`tocRule` 等）——CSS 链式选择器、JSONPath、XPath、Regex、JS（boa 沙箱，含 `java.*`/`source.*` shim 与 AES 解密）
- **书源管理**：增删改（规则字段 JSON 编辑）、启停、分组、失效检测、本地/远程导入、导出、订阅源
- **书源调试**：搜索/目录/正文逐规则逐步日志（SSE 流式）
- **书源登录**：`loginUrl` 登录流 + `loginCheckJs` 校验 + Set-Cookie 按用户合并；图片验证码截图回填；**CDP 浏览器自动登录**（表单/滑块贝塞尔轨迹/cookie 提取——本机 Chrome/Edge 自动发现）
- **反爬**：Cloudflare 质询检测 → **进程内浏览器求解**（内嵌 FlareSolverr 能力——无需外部容器）或可选外部 `FLARESOLVERR_URL`；cookie 按 name 合并 + UA 记录
- **换源**：并发多源搜索 + 书名过滤去重，一键切换

### 📖 阅读体验
- 阅读器：字体（12 档 + 离线网络字体）、行距/段距/字重/宽度/字距/缩进/对齐、主题（浅色/深色/纸色 + 界面深色独立）、翻页、自动阅读、进度服务端同步、书签、划词（复制/搜索）、预加载、TTS（Edge TTS + HttpTTS 音色/倍速）、**亮度滑条、键盘翻页、目录当前章高亮**
- **全局简繁转换**（自动检测/简/繁三态）、阅读偏好 12 项云端同步（多端一致）
- 整书缓存（后台并发 + SSE 进度 + 取消）、正文缓存、缓存管理、**全书内容搜索**（含文件型本地书）
- 阅读统计（时长/字数累计）、书架排序/分组折叠/网格密度/进度角标

### 📁 本地书（7 格式）
EPUB · TXT · MOBI · AZW3 · PDF · FB2 · DOCX —— 上传导入（含预览）、目录、正文、重扫、全书搜索、OPDS 下载统一分派（PDF 8MB 解压上限防炸弹、DOCX 标题样式分章、TXT 编码自动检测）

### 📤 导出与备份
- 书籍导出：TXT / EPUB / HTML；书源导出；**备份恢复闭环**（`backupToWebdav` + `restoreFromZip`/`restoreFromWebdav`——9 类目幂等恢复，兼容 legacy 备份布局）

### 🌐 OPDS & WebDAV
- **OPDS 1.2**（Atom 导航/分组/分页/OpenSearch/获取/下载/封面）+ **OPDS 2.0**（JSON catalog：facets/groups/publications）+ **OPDS-PSE**（进度保存/读取）
- **独立 OPDS 账号**（sha256+salt）或系统账号 Basic / token 三路认证
- **WebDAV 服务器**（OPTIONS/PROPFIND/GET/PUT/MKCOL/DELETE/MOVE/COPY/LOCK/UNLOCK——路径穿越防护）——可作为 Calibre/坚果云客户端存储

### 👥 多用户与安全
- 多用户注册/登录（token 随机化）、用户权限开关（WebDAV/本地书仓/书源/RSS）、用户管理
- 命名空间隔离（书架/书源 cookie/配置/文件全部按用户）、路径穿越防护（白名单 + 组件级归一化）、SQL 全参数化、OPDS/WebDAV 独立认证
- 安全设计详见 [`docs/SECURITY.md`](docs/SECURITY.md)

### 🎨 前端（web-ui）
Vue 3 + Vite + Element Plus，A 版极简风格（近白底/细字重/强调色/圆角），响应式移动端适配、深色主题、虚拟滚动书架、SSE 流式搜索/调试/缓存进度

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

### Docker（推荐——镜像内置 chromium，验证码/CF 质询浏览器流可直接使用）

```bash
# 构建（多阶段：Rust 编译 + 前端构建 + 运行镜像（debian + chromium））
docker build -t reader-dev .

# 运行（数据持久化到 ./data）
docker run -d --name reader-dev -p 8080:8080   -v "$PWD/data:/data"   -e READER_APP_WORKDIR=/data   -e READER_APP_SECURE=true   reader-dev
```

> 容器内验证码路径：**内置 chromium**（`READER_CHROME_PATH=/usr/bin/chromium`）→ 登录表单/滑块自动处理、CF 质询进程内求解；无需外部 FlareSolverr 容器（如需仍可 `FLARESOLVERR_URL` 指向 sidecar）。

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
| `READER_CHROME_PATH` | 自动发现 | Chrome/Edge 路径（书源登录浏览器流） |
| `FLARESOLVERR_URL` | 空（内嵌求解） | 可选外部 FlareSolverr 服务地址 |

---

## 📦 部署说明

### 书源浏览器流依赖
- **Windows**：自动发现系统 Edge/Chrome，无需安装
- **Linux/容器**：`apt install chromium`（或 `chromium-browser`/`google-chrome`），或用 `READER_CHROME_PATH` 显式指定
- 无浏览器时：登录/CF 质询降级为明确报错 + 手动 Cookie 粘贴

### 反爬（Cloudflare）
- **默认内嵌求解**：检测到 CF 质询自动用本机浏览器执行 JS 质询并提取 `cf_clearance`（无需外部服务）
- 可选：部署 FlareSolverr（Docker）并通过 `FLARESOLVERR_URL` 指定

### 反向代理（HTTPS）
服务端为 HTTP；公网部署建议前置 nginx/Caddy 终止 TLS：

```nginx
server {
    listen 443 ssl;
    server_name reader.example.com;
    client_max_body_size 200m;              # 上传大文件
    location / { proxy_pass http://127.0.0.1:8080; proxy_buffering off; }  # SSE
}
```

---

## 📖 使用指南

- **书源导入**：书源管理 → 本地导入 / 远程导入 / 订阅源；调试器逐规则排查
- **OPDS 接入**：外部阅读器（Legado/静读天下）添加 `http://host:port/opds`（可配独立账号，设置页查看并复制）
- **WebDAV**：`http://host:port/reader3/webdav/`（系统用户 Basic 认证，需开启 WebDAV 权限）
- **备份/恢复**：设置页 → 备份到 WebDAV / 下载备份；恢复通过 `POST /reader3/restoreFromZip`
- **全书搜索**：详情页 → 全书搜索（本地书，含文件型）；书架搜索可切「全书」范围

---

## 🧑‍💻 开发

```bash
cargo test        # 后端测试（255+，含 CF 求解/OPDS/本地格式集成测试）
cd web-ui && npm run build   # 前端（vue-tsc 类型检查 + vite）
```

### 项目结构

```
src/
├── api/          # axum 路由（/reader3/*、/opds、/opds-save）
├── model/        # 数据模型（book/book_source/rss/user/...）
├── parser/       # 规则引擎（css_chain / js / rule / xpath）
├── service/      # 业务（search/crawler/explore/local_book/epub/opds/rss/
│                 #   browser(CDP)/login(书源登录)/export_book/debug/cache_job/health）
├── storage/      # SQLite（迁移/CRUD/书源 cookie/正文缓存/阅读统计）
└── util/         # md5/sha256 等
web-ui/src/
├── views/        # 12 个视图（书架/阅读/搜索/详情/探索/书源/文件/用户/规则/RSS/设置/登录）
├── components/   # LogoMark / ErrorBoundary
├── api/          # 后端接口封装（24 个模块）
└── utils/        # chinese(简繁)/uiTheme/readerConfig/download/...
scripts/          # api-scan.py（API 扫描）/ mock-cf-site.py（CF 质询 mock）
tests/            # 集成测试（cf_solve 等）
docs/             # SECURITY.md / ROADMAP.md / ARCHITECTURE.md / legado-ref/
```

---

## 🧪 测试

- 后端 259+ 单测与集成测试（规则引擎/存储迁移/OPDS XML/本地书 7 格式解析/CF 质询端到端/WebDAV 全方法）
- `scripts/api-scan.py`：API 全接口扫描（正常/空参/错参/边界）
- 前端 `vue-tsc --noEmit` 严格类型检查 + vite 构建

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
