# reader-dev 前端规划（Rust 版）

> 决策：**不复用 legacy 前端构建产物**（安全漏洞过多），全新前端。
> 状态：规划中（后端切片推进时并行开发）。

---

## 1. 技术选型

| 项 | 选择 | 理由 |
|---|---|---|
| 框架 | **Vue 3**（Composition API） | 生态成熟、legacy 前端同为 Vue（迁移心智低） |
| 构建 | **Vite** | 现代、快、tree-shaking 好 |
| 语言 | **TypeScript** | 类型安全（API 契约可共享） |
| 状态 | **Pinia** | Vue 3 官方推荐 |
| UI | **Element Plus**（或 Naive UI） | 组件完备（legacy 用 Element UI，迁移平滑） |
| 请求 | **axios** | 与后端 /reader3/* 对接 |
| 内嵌 | **rust-embed**（后端编译时嵌入 dist） | 单二进制全功能 |

## 2. 安全要求（硬性）

- **依赖零已知漏洞**：npm audit 门禁（CI 中阻断高危）
- **CSP 头**：后端下发 Content-Security-Policy（禁内联脚本/限制来源）
- 无 `eval`/`new Function`（书源规则 JS 在后端执行，前端不碰）
- 依赖锁定（package-lock.json）+ 定期 dependabot
- 构建产物最小化（Vite 默认）+ SRI（可选）

## 3. 页面结构

```
/                   书架（虚拟列表、分组、搜索入口、书源/书仓入口）
/reader/{url}       阅读页（翻页/滚动/听书/目录/进度）
/search            搜索页（多源并发 + SSE 进度）
/source            书源管理（列表/分组/导入导出/调试）
/rss               RSS 订阅
/user              用户管理（secure 模式）
/files             文件管理（书仓/WebDAV/数据目录）
/settings          设置
```

## 4. API 对接（Rust 后端 /reader3/*）

| 前端功能 | API |
|---|---|
| 登录/注册 | POST /reader3/login（accessToken 持久化） |
| 书架 | GET /reader3/getBookshelf |
| 搜索 | POST /reader3/searchBook / searchBookMulti（+SSE） |
| 详情/目录/正文 | bookInfo / bookToc / bookContent（切片 3-4） |
| 书源 | GET /reader3/getBookSources / saveBookSources |
| 文件 | /reader3/file/*（WebDAV 目录复用） |

## 5. 迭代顺序

1. 脚手架：Vite + Vue3 + TS + Pinia + axios 封装 + 登录页
2. 书架页（虚拟列表）+ 搜索页
3. 阅读页（核心：翻页渲染 + 进度）
4. 书源/设置/文件管理
5. rust-embed 内嵌 + CSP + 构建流水线（GitHub Actions）

## 6. 与后端开发并行

- 后端切片 3-7 进行时，前端按 1-5 顺序并行开发
- API 契约以实际后端为准（联调驱动）
