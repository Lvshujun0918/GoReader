<div align="center">

# GoReader

**自托管 Web 阅读服务 —— 本地书阅读 · 书架 · 多用户**

Go + Vue 3 实现（后端 gin+gorm，前端 shadcn-vue）。导入本地书籍（TXT/EPUB/HTML），书架管理、目录切章、全文阅读，多用户隔离。

</div>

---

## ✨ 功能特性

### 📖 阅读体验
- 字体（12 档 + 离线网络字体）、行距/段距/字重/宽度/字距/缩进/对齐、主题预设（白色/米黄/绿色/灰黑夜/纯黑夜）、翻页模式（滚动/滑动/仿真/分页）、自动阅读、亮度、键盘翻页、Wake Lock 常亮
- 底部工具栏（字号/主题/目录/设置）、整页点击区域（左右翻页/切章，中间弹隐顶底栏）、真分页与自动翻页
- 全局简繁转换（自动检测/简/繁）、阅读偏好同步、每本书独立配置
- 正文缓存、全书搜索（本地书正文）、阅读统计、章节字数、书签
- 非文本书籍：音频/视频/漫画（图片逐页）/文件书

### 📁 本地书导入
- **TXT**（UTF-8/GBK 自动检测编码 + TXT 目录规则切章）
- **EPUB**（OPF/spine 目录导航）
- **HTML**
- 上传导入（含预览）、目录、正文、重扫、全书搜索

### 📤 导出与备份
- 导出：TXT（编码可选）/ EPUB（内嵌中文字体 + 完整目录导航）/ HTML
- 备份：WebDAV / zip（**恢复**——9 类目幂等）
- 数据迁移：legacy（Kotlin）JSON → SQLite 全量自动迁移（书/书签/规则/分组/用户配置——原文件保留可回退）

### 🌐 WebDAV
- WebDAV 服务器（全方法 + 路径穿越防护）

### 👥 多用户与安全
- argon2id 密码哈希（PHC——登录自动升级）、token 随机化、登录限流（直连 IP）、多设备 token
- 命名空间隔离、路径穿越防护、SQL 全参数化
- 服务监控页（内存/CPU/请求/在线）、日志

### 🎨 前端
Vue 3 + Vite + **shadcn-vue**（Tailwind + reka-ui + sonner），极简风格、响应式、深色主题、虚拟滚动、PWA、i18n（中/英）、命令面板（Ctrl+K）

---

## 🚀 快速开始

### 直接运行（Linux/Windows/macOS）
```bash
# 后端（Go 1.25+；CGO_ENABLED=0 纯 Go SQLite，无需 C 工具链）
go build -o GoReader ./cmd/server
# 前端
cd web-ui && npm install && npm run build && cd ..
# 运行
export READER_APP_WORKDIR="$PWD/data"
export READER_APP_SECURE=true
./GoReader
```
浏览器打开 `http://localhost:8080`。

### Docker（推荐）
```bash
docker pull ghcr.io/lvshujun0918/GoReader:latest
docker run -d --name GoReader -p 8080:8080 \
  -v "$PWD/data:/storage" \
  -e READER_APP_WORKDIR=/storage \
  -e READER_APP_SECURE=true \
  ghcr.io/lvshujun0918/GoReader:latest
```

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
docker run -d --name GoReader-rust \
  -v <同一数据卷>:/storage \
  -p 8080:8080 \
  -e READER_APP_WORKDIR=/storage \
  -e READER_APP_SECURE=true \
  ghcr.io/lvshujun0918/GoReader:latest

# 4. 启动时自动迁移（JSON → SQLite 全量——日志见「JSON→SQLite 迁移完成」）
#    原 JSON 文件保留（可回退）
```

### 迁移覆盖
用户 / 书架（含进度）/ 书签 / 替换规则 / TXT 目录规则 / HttpTTS / 分组 / 用户配置——全量，raw_json 保底。

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
| `READER_UPLOAD_MAX_MB` | `100` | 上传上限 |
| `READER_IMAGE_CACHE_MB` | `512` | 图片代理磁盘缓存上限 |
| `READER_TOKEN_TTL_DAYS` | `30` | token 过期天数 |
| `READER_DB_BACKUP` | `1` | 启动时 DB 快照备份 |
| `READER_AUTO_BACKUP_HOUR` | `3` | 每日自动备份小时 |
| `READER_LOCAL_BOOK_DIR` | 空 | 本地书监听目录 |
| `READER_LOG_DIR` | 空 | 日志目录（按大小轮转） |
| `READER_ALLOW_PRIVATE_NETWORK` | 关 | `1` 时允许抓取/探索内网与回环地址（本地代理场景需要） |

---

## 🧑‍💻 开发

```bash
go test ./...       # Go 单测（配置/存储/本地书）
cd web-ui && npm run build   # 前端（vue-tsc + vite）
node --test src/api/*.test.ts src/utils/*.test.ts   # 前端单测
```

### 结构
```
cmd/               # Go 入口（server）
internal/
├── api/           # gin 路由（/reader3/*、WebDAV）
├── model/         # gorm 数据模型
├── service/       # 业务（localbook 本地书解析）
├── storage/       # SQLite（迁移/CRUD/缓存/统计）
├── middleware/    # 缓存/限流/统计中间件
├── config/        # 配置（READER_APP_* env）
└── util/          # password(argon2)/md5/登录限流/...
web-ui/src/        # Vue3 + shadcn-vue 视图/组件/api/utils
web-simple/        # Kindle 轻量页
```

## 📄 License
[GNU General Public License v3.0](LICENSE) (GPL-3.0)
