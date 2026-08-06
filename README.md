<div align="center">

# Reader Dev

**自托管 Web 阅读服务 —— 书源搜索 · 本地书仓 · OPDS · WebDAV · 多用户**

Rust + Vue 3 实现。书源规则引擎、9 种本地书格式、OPDS 1.2/2.0 + PSE、进程内反检测浏览器（obscura stealth）、多用户隔离。

</div>

---

## ✨ 功能特性

### 📚 书源与抓取
- **规则引擎**：CSS 链式（元素级——索引/排除/属性选择器/`||`/`&&`/`%%`/JS 嵌套）、JSONPath、XPath、正则（多段替换）——legacy/legado 双命名
- **书源管理**：增删改（规则字段 JSON 编辑）、启停、分组、失效检测、本地/远程导入、导出、订阅源
- **书源调试**：搜索/目录/正文逐规则逐步日志（SSE 流式）
- **书源登录**：`loginUrl` 登录流 + 图片验证码回填 + 手动 Cookie
- **换源**：并发多源搜索 + 书名过滤去重 + 结果内书源名二次过滤 + 刷新

### 🛡️ 反检测（进程内——obscura）
- **obscura 浏览器引擎内嵌**（Rust 实现——`--stealth` 构建：BoringSSL TLS 指纹模拟/反检测/追踪器拦截）——作为唯一浏览器后端（无系统浏览器依赖）
- **Cloudflare 质询**：检测（503/403 + 特征）→ 浏览器内质询等待循环 → cookie 提取合并（按用户隔离）→ **原请求重试**
- **Turnstile 验证码**：widget 检测 → **iframe 内 JS 点击**（frame 上下文执行）→ token 轮询
- **登录滑块**：JS 合成事件拖拽
- **会话管理**：按用户独立实例、闲置回收、求解前清 cookie 防跨用户泄漏
- **可选代理**：`READER_OBSCURA_PROXY`（住宅代理等）

### 📖 阅读体验
- 字体（12 档 + 离线网络字体）、行距/段距/字重/宽度/字距/缩进/对齐、主题（亮/暗/暖色/自定义/跟随系统）、翻页模式（滚动/滑动/仿真）、自动阅读、亮度、键盘翻页、Wake Lock 常亮
- 全局简繁转换（自动检测/简/繁）、12 项阅读偏好云端同步、每本书独立配置
- 整书缓存（SSE 进度）、正文缓存、全书搜索（本地书 + 已缓存书源书）、阅读统计、章节字数
- 非文本书籍：音频/视频/漫画（图片逐页）/文件书

### 📁 本地书（9 格式）
EPUB · TXT · MOBI · AZW3 · PDF · FB2 · DOCX · CBZ（漫画）· UMD —— 上传导入（含预览）、目录、正文、重扫、全书搜索
- **双轨同步仓**：文件与 DB 双向同步（文件变更自动导入/重扫；DB 书自动生成 epub 镜像）

### 📤 导出与备份
- 导出：TXT（编码可选）/ EPUB（内嵌中文字体 + 完整目录导航）/ HTML
- 备份：WebDAV / zip（含 MongoDB 可选）——**恢复**（zip/WebDAV——9 类目幂等，兼容 legacy 备份）
- 数据迁移：legacy（Kotlin）JSON → SQLite 全量自动迁移（书/书源/书签/规则/RSS/分组/用户配置——原文件保留可回退）

### 🌐 OPDS & WebDAV
- **OPDS 1.2**（导航/分组/分页/搜索/获取/下载/封面）+ **2.0**（JSON catalog）+ **PSE**（进度保存）
- 独立 OPDS 账号（sha256）或系统账号/token 三路认证
- **WebDAV 服务器**（全方法 + 路径穿越防护）

### 👥 多用户与安全
- argon2id 密码哈希（PHC——登录自动升级）、token 随机化、登录限流（直连 IP）、多设备 token
- 命名空间隔离、路径穿越防护、SSRF 防护、图片缓存按用户隔离、SQL 全参数化
- 服务监控页（内存/CPU/请求/在线/书源成功率）、日志

### 🎨 前端
Vue 3 + Vite + Element Plus，极简风格、响应式、深色主题、虚拟滚动、SSE 流式、PWA、i18n（中/英）、命令面板（Ctrl+K）

---

## 🚀 快速开始

### 直接运行（Linux/Windows/macOS）
```bash
# 后端
cargo build --release
# 前端
cd web-ui && npm install && npm run build && cd ..
# 运行
export READER_APP_WORKDIR="$PWD/data"
export READER_APP_SECURE=true
./target/release/reader-dev
```
浏览器打开 `http://localhost:8080`。

> 反检测浏览器：需下载 [obscura](https://github.com/h4ckf0r0day/obscura) stealth 构建（`-stealth` 资产），放同目录或配置 `READER_OBSCURA_BIN`（Windows/Linux 自动探测）。无 obscura 时质询类功能降级报错（普通抓取不受影响）。

### Docker（推荐）
```bash
docker pull ghcr.io/warpdotsys/reader-dev:latest
docker run -d --name reader-dev -p 8080:8080 \
  -v "$PWD/data:/storage" \
  -e READER_APP_WORKDIR=/storage \
  -e READER_APP_SECURE=true \
  ghcr.io/warpdotsys/reader-dev:latest
```
镜像内置 obscura（stealth）+ chromium 依赖移除——**反检测能力开箱即用**。

---

## 🔄 从 legacy（Kotlin）Docker 迁移

### 只换镜像（数据零改动）
```bash
# 1. 备份（保险）
docker exec <旧容器> tar czf /tmp/backup.tar.gz /storage
docker cp <旧容器>:/tmp/backup.tar.gz .

# 2. 停旧容器（数据卷不动）
docker stop <旧容器>

# 3. 起新容器（同一数据卷——挂载路径保持）
docker run -d --name reader-dev-rust \
  -v <同一数据卷>:/storage \
  -p 8080:8080 \
  -e READER_APP_WORKDIR=/storage \
  -e READER_APP_SECURE=true \
  ghcr.io/warpdotsys/reader-dev:latest

# 4. 启动时自动迁移（JSON → SQLite 全量——日志见「JSON→SQLite 迁移完成」）
#    原 JSON 文件保留（可回退）
```

### 直接跑二进制
```bash
# release 资产 reader-dev-linux-x64-musl（静态——任何发行版）
READER_APP_WORKDIR=/storage READER_APP_SECURE=true ./reader-dev-linux-x64-musl
```

### 迁移覆盖
用户 / 书架（含进度）/ 书源 / RSS / 书签 / 替换规则 / TXT 目录规则 / HttpTTS / 分组 / 用户配置——全量，raw_json 保底。

---

## ⚙️ 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `READER_SERVER_PORT` | `8080` | 端口 |
| `READER_APP_WORKDIR` | 当前目录 | 数据目录（storage/ 下） |
| `READER_APP_WEB_ROOT` | `web-ui/dist` | 前端静态根 |
| `READER_APP_SECURE` | 关 | 多用户安全模式 |
| `READER_APP_MINUSERPASSWORDLENGTH` | `8` | 密码最小长度 |
| `READER_APP_INVITECODE` | 空 | 注册邀请码 |
| `READER_OBSCURA_BIN` | 自动探测 | obscura 可执行文件路径 |
| `READER_OBSCURA_URL` | 空 | 连接既有 obscura CDP 服务 |
| `READER_OBSCURA_PROXY` | 空 | obscura 代理（如 socks5://127.0.0.1:1080） |
| `READER_UPLOAD_MAX_MB` | `100` | 上传上限 |
| `READER_IMAGE_CACHE_MB` | `512` | 图片代理磁盘缓存上限 |
| `READER_TOKEN_TTL_DAYS` | `30` | token 过期天数 |
| `READER_DB_BACKUP` | `1` | 启动时 DB 快照备份 |
| `READER_AUTO_BACKUP_HOUR` | `3` | 每日自动备份小时 |
| `READER_LOCAL_BOOK_DIR` | 空 | 本地书监听目录 |
| `READER_LOG_DIR` | 空 | 日志目录（按大小轮转） |

---

## 🧑‍💻 开发

```bash
cargo test          # 480+ 单测与集成（规则引擎/格式解析/obscura/CF 质询/WebDAV）
cd web-ui && npm run build   # 前端（vue-tsc + vite）
npm test            # 前端单测（54+）
```

### 结构
```
src/
├── api/          # axum 路由
├── model/        # 数据模型
├── parser/       # 规则引擎（css_chain/js/rule/xpath/jsonpath）
├── service/      # 业务（browser(obscura CDP)/crawler/search/explore/local_book/opds/...）
├── storage/      # SQLite（迁移/CRUD/缓存/统计）
└── util/         # password(argon2)/regex/md5/...
web-ui/src/       # Vue3 视图/组件/api/utils
web-simple/       # Kindle 轻量页
scripts/          # 审计/测试工具（api-scan/mock 站点等）
docs/             # SECURITY/ARCHITECTURE/ROADMAP/FRONTEND
```

---

## 📚 文档
- `docs/SECURITY.md` —— 安全设计（argon2/SSRF/隔离/限流）
- `docs/ARCHITECTURE.md` —— 架构（obscura 内嵌/分层）
- `docs/ROADMAP.md` —— 路线图与待办
- `docs/legado-ref/ruleHelp.md` —— 规则参考

## 📌 分支
| 分支 | 说明 |
|---|---|
| `master` | **Rust 版（当前）——v5.0.0** |
| `legacy` | Kotlin 稳定版（ghcr v4.x） |

## 💝 赞助
| 网络 | 币种 | 地址 |
|---|---|---|
| Arbitrum | USDC | `0x0B704AcC2EdD28DdaE80e03f1a98e2cD00B0B5ae` |
| Ethereum | USDT | `0x0B704AcC2EdD28DdaE80e03f1a98e2cD00B0B5ae` |
| Arbitrum | USDC.e | `0x0B704AcC2EdD28DdaE80e03f1a98e2cD00B0B5ae` |

> 地址已通过 EIP-55 校验。转账前请核对网络与币种。

## 📄 License
[GNU General Public License v3.0](LICENSE) (GPL-3.0)
