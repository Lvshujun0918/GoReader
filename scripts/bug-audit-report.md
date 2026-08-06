# Bug 审计报告（全面审计 · 只读）

- 审计对象：`C:/Users/chong/pr-review/reader-dev` @ master（HEAD `bd6af63`）
- 审计方式：静态代码审计 + 单元测试全量回归 + 独立实例（8085，测试库 `target/search-test`）curl 实测
- 审计日期：2026-08-06
- 约束：只读审计，未改任何功能代码；审计期间在测试库产生的数据已清理
- 工作区并行 agent 的 WIP（`src/service/browser.rs`、`tests/captcha_matrix.rs` 等未提交改动）不在审计范围

---

## 一、审计覆盖范围

### 前端（web-ui）
| 模块 | 覆盖点 |
|---|---|
| CommandPalette.vue / utils/commandPalette.ts | 键盘导航、过滤、动态搜索命令、事件监听清理 |
| ServerStatsView.vue | 10s 轮询定时器/可见性暂停、计数倒计时、字段契约 |
| StoreView.vue | 书仓浏览、批量导入、secure 探测、失效勾选清理 |
| ReaderView.vue（5462 行） | 翻页模式 4 态切换/监听增删、flip 分页、Wake Lock、自定义主题、每日时长统计、TTS/自动播放定时器、卸载清理 |
| readerPageMode / readerTheme / bookConfig / customCss / readerBg / dailyStats / shelfView / sourceGroupOrder / wakeLock | 边界/空值/损坏数据回退、localStorage 读写 |
| BookshelfView.vue | 虚拟滚动、视图模式、进度角标、置顶 |
| BookDetailView.vue | 进度环（SVG dasharray）数据 |
| SearchView.vue | 精确搜索 exact 参数透传（普通 + SSE） |
| SettingsView.vue | 阅读偏好云端合并、主题自定义 |
| i18n.ts（372 键） | 缺失键回退、语言切换 |
| App.vue / router / main.ts | 主题/CSS/语言初始化、路由守卫 |
| public/sw.js + manifest | PWA 缓存策略、离线壳 |

### 后端（src）
| 模块 | 覆盖点 |
|---|---|
| service/image_cache.rs | LRU 容量、并发去重 in-flight、键安全、磁盘原子写、跨命名空间 |
| service/monitor.rs + middleware/stats.rs | 请求计数、跨天滚动、Top 接口 cap、CPU/内存采样、在线会话 |
| service/mongodb_backup.rs | 备份/恢复幂等、字段保真、token 处理 |
| service/camoufox.rs | 求解链、超时、cookie 传递 |
| util/login_limit.rs + client_ip | 锁定语义、XFF 伪造、DoS |
| util/db_backup.rs | WAL checkpoint、保留份数、幂等 |
| middleware/upload_limit.rs | 413 → JSON |
| parser/（js 3013 行 / rule 1608 行 / css_chain 1134 行 / xpath 382 行） | 引擎重写：CSS 链/索引/组合分隔、JSONPath v2、XPath、JS 桥、循环上限、正则回退 |
| storage/mod.rs 迁移 | 幂等补列、书源 upsert ON CONFLICT、字数查询 |
| api/router.rs | assets/proxy、searchBookMulti/SSE exact、exportBook、getServerStats 等 |

### 回归实测（curl，独立实例 http://127.0.0.1:8085，secure 模式 + target/search-test 库）
登录（含限流实测）、书架、模糊/精确搜索（普通 + SSE）、本地书 TOC（含 chapterWordCount）、章节正文、进度保存、书签增删查、TXT(GBK)/EPUB 导出、OPDS（XML feed + JSON catalog）、WebDAV 备份、userConfig 存取、图片代理缓存、getServerStats、上传拦截、getUsers secureKey 校验、SSRF 探测、跨用户缓存命中探测。

### 测试回归
- `cargo test --lib`：**448 passed / 0 failed**（含引擎 88 项 parser 测试、迁移/存储测试）
- `cargo test --test '*'`：3 passed
- `node --test web-ui/src/utils/*.test.ts`：**49 passed / 0 failed**

---

## 二、Bug 清单（按严重度）

### Blocker（数据丢失/崩溃/不可用）
无。未发现 panic 路径或直接数据丢失风险。

### Major（安全 / 功能缺失 / 可用性）

**M1. `/assets/proxy` 无 SSRF 防护（可读内网/回环响应）** — 安全
- 位置：`src/api/router.rs` `assets_proxy`（仅校验 http/https scheme）；`src/service/crawler.rs` `fetch_image`
- 复现（已实测）：`GET /assets/proxy?url=http://127.0.0.1:8085/reader3/getSystemInfo&accessToken=...` → 返回本服务内部 JSON（含内存/CPU/请求统计）。非 secure 模式 `resolve_namespace` 直接返回 `default`，**无需任何认证**即开放代理。
- 影响：任意访问者可扫描内网、读取云元数据（169.254.169.254）、以服务器 IP 访问内部管理面；响应完整回显。
- 建议：解析后校验目标 IP 非回环/私网/链路本地（DNS 解析后校验 + 拒绝重定向到私网）；非 secure 模式可考虑要求访问口令；对代理加 per-IP 限流。

**M2. 图片磁盘缓存跨用户共享（缓存键不含命名空间）** — 隐私泄漏
- 位置：`src/service/image_cache.rs` `cache_key()` = `md5(url) 前16位`，不含 `ns`；`assets_proxy` 传入的 namespace 仅用于回源时附加 cookie
- 复现（已实测）：用户 A 请求 `http://127.0.0.1:8085/logo.png`（回源写盘，`max-age=3600`）；用户 B 用自己 token 请求同一 URL → `Cache-Control: immutable`（磁盘命中），拿到的是 **A 的会话 cookie 回源得到的字节**。
- 影响：secure 多用户下，B 可读到 A 凭据拉取的个性化图片内容（登录后可见图、鉴权 URL）；同 URL 内容按 cookie 分用户时串图。
- 建议：缓存键并入 namespace（如 `md5("{ns}|{url}")`）或缓存目录按 ns 分区；referer 差异导致的个性化内容同理（可选并入键）。

**M3. 登录限流可被 X-Forwarded-For 伪造绕过 + 无代理时账户可被远程锁定** — 安全/可用性
- 位置：`src/api/router.rs` `client_ip()`（信任 XFF）；`src/util/login_limit.rs`（键 = username|ip）
- 复现（已实测）：同一用户名错误密码 5 次 → 第 6 次被锁；但第 6 次请求加 `X-Forwarded-For: 9.9.9.9` → 立即放行继续试密码。反向：直连（无 XFF）时 ip=""，任意远端 5 次失败即可把任意用户名锁 5 分钟（账户 DoS）。
- 建议：限流键改为「用户名 + 真实对端 IP（仅信任直连 socket 或白名单代理）+ 全局用户名级计数分层」；锁定按用户名全局而非 (user,ip) 对；对注册接口同样限流。

**M4. 书架「封面墙」功能未接线（dead code）** — 功能缺失
- 位置：`web-ui/src/utils/shelfView.ts`（`parseShelfView`/`shelfViewMetrics` 完整实现且有单测）；`web-ui/src/views/BookshelfView.vue` 第 62-68、1865-1868 行
- 现象：BookshelfView `viewMode` 仅 `'grid'|'list'`，载入时 `if (raw === 'grid' || raw === 'list')` 直接丢弃 `'wall'`；视图切换按钮只切 grid/list。i18n `shelf.view.wall` 等键与 CSS 无引用方。
- 复现：localStorage 写入 `reader_shelf_view='wall'` 刷新书架页 → 仍为网格。
- 建议：BookshelfView 接入 wall 模式（三态切换 + 按 `shelfViewMetrics('wall')` 计算虚拟滚动行高列数）。

**M5. PWA Service Worker 以 cache-first 拦截全部同源 GET（含动态接口/图片代理）** — 缓存错误/陈旧
- 位置：`web-ui/public/sw.js` `fetch` 监听（`cacheFirst` 应用于所有非导航同源 GET）；`web-ui/src/api/users.ts` `probeSecureMode`（fetch GET）
- 现象/影响：
  1. `/assets/proxy` 回源失败时返回 **HTTP 200 + JSON 错误体**，SW 视为 `response.ok` 永久缓存 → 一次瞬时上游故障 = 封面永久损坏（直到 SW 版本号变更）；
  2. `/reader3/file/download` 封面/背景图 cache-first 且无 TTL → 用户更换封面后永远显示旧图；
  3. `STATIC_CACHE` 无容量上限/无清理策略，长期累积膨胀；
  4. `probeSecureMode` 的 `_t` 防缓存参数反而让 SW 为每次探测新建缓存条目（只增不减）。
- 建议：fetch 拦截按路径排除 `/reader3/*`（API 一律 network-only）；`/assets/proxy`、`file/download` 用 network-first 或按响应 Cache-Control 尊重 TTL；缓存加容量上限与清理。

**M6. JS 桥接 `block_on_task` 阻塞 async worker + 超时后线程不回收** — 可用性
- 位置：`src/parser/js.rs` `block_on_task`（941 行起）及调用方 `java.ajax`/`Reload`/`java.startBrowserAwait`/`getWbiEnc`（每桥接调用新建线程 + 完整 current-thread tokio runtime）
- 现象：桥接调用在 axum worker 线程上同步等待最多 60s——并发书源规则执行（SSE 多书源搜索）可占满全部 worker 使整个服务无响应；`recv_timeout` 超时后**工作线程不终止**，继续运行直到内部 fetch 超时（线程+运行时泄漏，持续高压下累积）。
- 建议：桥接调用改为 `tokio::task::spawn_blocking`/独立任务 + `tokio::time::timeout`（异步等待，不占 worker）；超时路径需能取消底层 future（或将 crawler 超时下调）。

### Minor

**m1. MongoDB 恢复丢失多设备 token**：`src/storage/mod.rs` `insert_user`（INSERT OR REPLACE 未绑定 `token_map` 列）——`restore_from_mongodb` 恢复 users 后全部用户 token_map 清空（次设备全部掉线）；备份文档中 token_map 明明存在。建议恢复路径单独 upsert token_map。

**m2. 监控接口未鉴权**：`/reader3/getSystemInfo`、`/reader3/getServerStats` 在 secure 模式也无 token 校验（已实测匿名可读），泄露版本/端口/内存/CPU/在线会话数/接口热度。建议至少要求登录。

**m3. 监控页路由标题缺 i18n 键**：`router/index.ts` 用 `titleKey: 'route.serverStats'`，但 `i18n.ts` 只有 `nav.serverStats`，缺 `route.serverStats` → 页面标题显示字面量 `route.serverStats · 夜读`。

**m4. 进度环/角标百分比公式**：`BookDetailView.readProgress` 与 `BookshelfView.bookProgress` 均用 `cur/total`（`durChapterIndex` 为 0 基）——读完第 1 章显示 0%，读完最后一章（cur=total-1）显示 99% 且「读完变绿」态不可达（除非后端置 cur=total）。建议 `(cur+1)/total` 或后端统一语义。

**m5. 书源分组排序工具未接线**：`utils/sourceGroupOrder.ts`（`mergeGroupOrder`/`reorderGroup`，含单测）无任何视图引用——书源分组胶囊顺序持久化实际未生效。SourceManageView 只有书源行级拖拽（weight）。建议接入或删除死代码。

**m6. 图片缓存 in-flight 表在请求取消时泄漏条目**：`image_cache.rs` `get_or_fetch`——future 在 `gate.lock().await` 或回源中途被取消（客户端断开）时，`self.inflight.remove(&key)` 不执行；条目永久滞留（每次取消的唯一 URL 各留一条，长期运行累积）。建议用 RAII guard 或 Drop 清理。

**m7. 图片缓存键 64 位截断**：`md5(url)` 前 16 hex（64 bit）作键——理论生日碰撞（~2^32 构造代价）可致串图/投毒；建议用完整 32 hex（文件名为 32 字符无副作用）。

**m8. 磁盘命中下发 `immutable` 一年**：`assets_proxy` 命中缓存回 `max-age=31536000, immutable`——上游封面换图后浏览器侧一年内不更新（注释已承认依赖 LRU 淘汰，但 LRU 只清容量超限，不保证换图）。建议命中态也下发短 TTL 或 ETag 校验。

**m9. 缓存 I/O 为阻塞式**：`image_cache` 的 `read_disk`/`write_disk` 在 async 上下文内做 `std::fs::read/write/rename`（且持索引锁），大图写盘可阻塞 worker 数十 ms。建议 `spawn_blocking`。

**m10. `getServerStats` 自计数**：stats 中间件挂最外层，监控页每 10s 轮询自身 +1，且静态资源请求也计入「今日请求量」。建议排除 `/reader3/getServerStats`/静态路径或标注。

**m11. 正则回退规则尾字符误判为索引**：`css_chain.rs` `parse_index_spec` legacy 分支——以 `.`/`!`/`:` 结尾的 legacy 正则规则（如 `(.+?)\.`）在 CSS 解析失败回退正则前被剥掉尾字符，匹配语义漂移。建议回退路径先做 CSS 合法性判定或对含正则元字符的规则跳过索引解析。

**m12. camoufox cookie 外传**：`camoufox.rs` `solve()` 把当前书源 cookie 随请求发给 `READER_CAMOUFOX_URL`——默认 127.0.0.1 无问题，但配置为远端地址时 cookie 将明文传给第三方。建议文档标注风险或加鉴权头。

**m13. login_limit 表在持续伪造 IP 攻击下无硬上限**：`prune` 只在 `len >= 8192` 时清非锁定项，持续换 (user,ip) 攻击可维持大表 + 每次登录 O(n) 扫描。建议按桶上限（如每 IP 前缀）或全局限流。

**m14. mongodb 备份逐文档往返、无事务**：大书架（千本）备份为千次网络往返；中断产生部分备份（无整体一致性标记）。建议 bulkWrite + 集合级顺序校验。

---

## 三、回归抽查记录（curl 实测 · 8085 独立实例）

| 链路 | 结果 | 备注 |
|---|---|---|
| 登录（isLogin=true）/注册 | ✅ | 多设备 token 返回正常 |
| 登录限流 | ✅/⚠️ | 5 次锁定生效；XFF 可绕过（M3） |
| 书架 getBookshelf | ✅ | |
| 搜索（模糊）searchBookMulti | ✅ | 多书源并发返回 |
| 搜索（精确 exact=1）普通+SSE | ✅ | 等值过滤生效；SSE 事件流正常 |
| 阅读：getBookToc（本地 epub） | ✅ | 含 chapterWordCount |
| 阅读：getBookContent（chapterUrl） | ✅ | |
| 进度 saveBookProgress | ✅ | |
| 书签 saveBookmark / getBookmarks / deleteBookmark | ✅ | |
| 导出 exportBook（epub 72KB / txt-gbk 90KB） | ✅ | 格式/编码参数生效 |
| OPDS（/opds XML + /opds/catalog JSON） | ✅ | |
| WebDAV backupToWebdav | ✅ | 生成 legado/backup-*.zip |
| userConfig 存取 | ✅ | reader_* 键原样存取 |
| getServerStats / getSystemInfo | ✅/⚠️ | 字段齐全；匿名可读（m2） |
| 图片代理 /assets/proxy | ✅/⚠️ | 缓存写盘/命中正常；SSRF（M1）、跨 ns 共享（M2）已实测 |
| 上传超限（非白名单扩展名） | ✅ | 扩展名白名单拦截 |
| getUsers（secure 管理校验） | ✅ | 需 secureKey |
| localStore 列表 | ✅ | 空目录返回空数组 |
| 迁移幂等 | ✅ | 存量库二次启动迁移无异常；ensure_column_typed 幂等 |
| 引擎单测 / 全量单测 / 前端单测 | ✅ | 448 + 3 + 49 全过 |

---

## 四、结论

- 引擎重写（css_chain/rule/xpath/jsonpath）质量高：88 项 parser 测试全过，组合/索引/过滤/JS 链边界均有覆盖；未见回归。
- 主要风险集中在**新增网络面**（图片代理 SSRF + 跨用户缓存）与**登录限流可绕过**——建议优先处理 M1/M2/M3。
- 前端生命周期管理总体规范（ReaderView 卸载清理完整）；主要问题为**功能未接线**（封面墙）与 **PWA 缓存策略越界**（M4/M5）。
- 未发现 panic/数据丢失级 blocker。
