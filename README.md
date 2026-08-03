# reader-dev (Rust) ｜ Rust 重构开发分支（进行中，未发布）。稳定版见 legacy 分支（Kotlin，ghcr.io/warpdotsys/reader-dev）

Rust 实现的 Web 阅读服务，API 兼容 legacy 分支 `/reader3/*` 接口，数据兼容（JSON storage → SQLite 迁移）。
前端为 Vue 3（`web-ui/`），构建产物由后端统一托管（SPA fallback）。

## 功能清单

**书源**
- 多书源搜索 / 书籍详情 / 目录 / 正文（legacy 规则引擎：`ruleSearch` / `ruleBookInfo` / `ruleToc` / `ruleContent` / `ruleExplore`，兼容 legado 命名别名 `searchRule` / `bookInfoRule` / `tocRule` / `contentRule` / `exploreRule`）
- 书源管理：新增 / 编辑（基本信息 + 规则字段 textarea JSON）/ 启停 / 删除 / 分组筛选 / 失效检测（`getInvalidBookSources` 红色标记）/ 本地导入 / 远程导入 / 导出 `bookSource.json` / 订阅源（服务端入库优先，localStorage 降级）
- 书源调试：搜索 / 目录 / 正文逐步日志（SSE `bookSourceDebugSSE`）
- 书源登录：HTTP 登录流（`loginUrl` 占位符 / POST 表单 / `loginCheckJs` / Set-Cookie 按用户合并存库）、图片验证码截图回填（`getCaptcha` / `submitCaptcha`）、浏览器自动登录（CDP 轻量实现——表单填写 / 滑块验证码贝塞尔拖拽 / cookie 提取，依赖本机 Chrome/Edge）、手动 Cookie 粘贴降级
- FlareSolverr：Cloudflare 质询自动检测并转发解（可选，`FLARESOLVERR_URL` 环境变量；解出 cookie 与书源原 cookie 按 name 合并）
- 换源：`searchBookSource` 并发多源搜索 + 书名过滤去重

**阅读**
- 阅读器：进度服务端同步（`saveBookProgress`）、书签、替换规则、TXT 目录规则、HttpTTS 听书源
- 全局简繁转换（简体 / 繁体）、主题（浅色 / 深色 / 纸色 / 跟随系统）、排版偏好（12 项阅读偏好云端同步 `getUserConfig` / `saveUserConfig`）
- 阅读统计（`getReadingStats`：时长 / 字数累计）
- 整书缓存（`cacheBookOnServer` / `cacheBookSSE`：后台缓存 + SSE 进度 + 取消）、缓存管理（`clearCache` / `getCacheInfo`）、全书内容搜索（`searchBookContent`）

**本地书（7 格式）**
- EPUB / TXT / MOBI / AZW3 / PDF / FB2 / DOCX：上传导入 / 目录 / 正文 / OPDS 下载统一分派
- 文件管理（`/reader3/file/*`）、本地书重扫（`refreshLocalBook`）

**导出**
- 书籍导出多格式：TXT / EPUB / HTML（`exportBook`）；书源导出 `bookSource.json`

**OPDS**
- OPDS 1.2（Atom 导航 / 分组 / 分页 / OpenSearch / 获取正文 / 下载 / 封面缩略图）+ OPDS 2.0（JSON catalog：facets / groups / publications / images）+ OPDS-PSE 进度保存（`/opds/save/{id}`）
- 独立 OPDS 账号（secure 模式 Basic 认证：独立账号优先 → 系统用户账号 → accessToken）

**服务**
- 多用户账号体系：每用户独立书源 / 书架 / 进度 / 书签 / 订阅 / WebDAV 命名空间，权限与限额可配（`updateUser`）
- RSS 订阅、WebDAV、用户管理（secureKey 保护）、书源健康（`getAvailableBookSource`）、批量接口（删除 / 书签 / 分组 / RSS / 换源）
- 数据存储：SQLite（`storage/` 目录），legacy JSON 数据自动迁移（users.json / bookSource.json / books 等）

## 构建与部署

### 后端（Rust）

需要 Rust 工具链（stable；Windows 建议 MSYS2 gcc，见 `build-dev.ps1`）。

```bash
cargo build --release
./target/release/reader-dev            # 默认端口 8080，前端静态资源 web-ui/dist
```

环境变量（`READER_APP_*` 兼容 legacy 前缀，实现见 `src/lib.rs` `AppConfig`）：

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `READER_SERVER_PORT` | `8080` | 服务端口 |
| `READER_APP_WORKDIR` | 当前目录 | 工作目录（数据在 `{workdir}/storage`） |
| `READER_APP_WEB_ROOT` | `web-ui/dist` | 前端静态资源根（SPA fallback → index.html） |
| `READER_APP_SECURE` / `READER_APP_SECUREKEY` | 关 | 安全模式（secureKey 保护用户管理 / OPDS 独立账号） |
| `FLARESOLVERR_URL` | 空（禁用） | FlareSolverr 服务地址，如 `http://127.0.0.1:8191` |
| `READER_CHROME_PATH` | 自动发现 | Chrome/Edge 可执行文件路径（书源浏览器登录流） |

### 前端（Vue 3）

```bash
cd web-ui
npm install
npm run build      # 产物 web-ui/dist（vue-tsc 类型检查 + vite build），由后端托管
```

### 可选依赖

**FlareSolverr（反 Cloudflare 质询）** —— 可选。书源抓取命中 Cloudflare 质询（503 + 特征 HTML）时自动转发解，解出的 cookie 与书源原 cookie 按 name 合并存库（按用户）并记录 UA。不配置则直连降级（部分站点会失败）。

Docker 部署：

```bash
docker run -d --name flaresolverr -p 8191:8191 ghcr.io/flaresolverr/flaresolverr:latest
# 启动 reader-dev 时设置环境变量：
# FLARESOLVERR_URL=http://127.0.0.1:8191
```

**Chrome / Edge（书源登录浏览器流）** —— 可选。书源登录的「浏览器自动登录」通过 CDP 驱动本机 headless 浏览器：登录表单自动填写、滑块验证码自动拖拽（人类轨迹：贝塞尔曲线 + 随机噪声）、图片验证码截图回填、`Storage.getCookies` 提取 cookie 存库。

- Windows：自动检测 Edge / Chrome 常见安装路径（无需配置）
- Linux：需安装 `chromium` / `chromium-browser` / `google-chrome`，或用 `READER_CHROME_PATH` 显式指定
- 未安装浏览器时登录自动回退手动 Cookie 流程（接口返回「未安装浏览器」提示）

### 本地书格式依赖说明

- **EPUB / FB2 / DOCX**：纯 Rust 解析（zip / XML 解包），无外部依赖；DOCX 按标题样式分章
- **MOBI / AZW3**：内置解析（PalmDOC / AZW 头），无外部依赖
- **PDF**：内置 PDF 解析（lopdf）逐页提取文本，8MB 解压上限（超大 / 加密 PDF 可能失败）
- **TXT**：自动编码检测 + 目录规则分章（TXT 目录规则可在设置中配置）
- 上传仅接受上述 7 种扩展名，其余返回「仅支持 EPUB/TXT/MOBI/AZW3/PDF/FB2/DOCX」

## 文档

- `docs/ARCHITECTURE.md` —— 架构说明（模块 / 数据流 / 契约）
- `docs/FRONTEND.md` —— 前端说明（页面 / 状态 / API 契约）
- `docs/SECURITY.md` —— 安全审查报告
- `docs/legado-ref/` —— legado 参考（规则命名对照）
