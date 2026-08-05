# 69shuba 强质询 camoufox 求解——完整诊断（2026-08-06）

## 结论（TL;DR）

**跑通到搜索前的全部环节；最终搜索被 Cloudflare Turnstile 环境风控（400030）拦截，
该拦截与 UA/头/指纹无关——真实 Chrome（无自动化 flag）同样失败。根因：本机出口 IP
为 DMIT（AS906，美西）数据中心 IP，Turnstile 挑战平台拒绝为其下发可解挑战。
建议：camoufox 配住宅 IP 代理（已支持 per-context proxy）后即可全自动。**

## 链路逐段结果

| 步骤 | 结果 | 证据 |
|---|---|---|
| 1. camoufox 导航 69shuba 首页 | ✅ 过（200，无 CF 边缘质询） | title=69书吧_更新最快...；真实书单渲染 |
| 2. UA 门禁（wire UA） | ✅ 过（需 Chrome wire UA） | Firefox wire UA 时首页即被拦；Chrome wire UA 直过 |
| 3. search.php 搜索 POST（页内 fetch） | ⚠️ 到挑战页（fetch 模式返回挑战页 HTML） | POST 200，7283B = 挑战页 |
| 4. search.php 搜索 POST（表单导航 navigate 模式） | ⚠️ 挑战页 Turnstile widget 渲染但永不产出 token | 40s 超时，clicks=68，hasInput=True |
| 5. Turnstile 求解 | ❌ 平台拒绝：event:fail code:**400030** | iframe 文档 `<script>var errCode = 400030; ... event:'fail'</script>`（见 `_evidence_turnstile_400030_iframe_doc.html`） |

## 关键发现

### 1. UA 覆盖的真实机制（camoufox_solver.py 已修复）
- Playwright `user_agent` 选项只改 **wire** UA；camoufox 指纹注入脚本
  （`setNavigatorUserAgent`）会把 JS 可见 `navigator.userAgent` 改回 Firefox——
  两侧不一致。
- 正解：`generate_context_fingerprint(config_overrides={'navigator.userAgent': ...})`
  让 wire 与 JS 一致为 Chrome；回退方案：追加 init script 二次 `defineProperty`
  补丁。两路均已实测（JS/WIRE 均为 Chrome）。
- Chrome UA 时自动补 `Sec-CH-UA` / `Sec-CH-UA-Mobile` / `Sec-CH-UA-Platform` 头
  （Firefox 引擎默认不发送，站点交叉验证会判非 Chrome）。

### 2. "请使用新版本的Google Chrome" 门禁的真相
不是站点 UA 检查——是 search.php 的 **Turnstile managed challenge 页横幅**：
页面含 `turnstile.render("#cfts", {sitekey: "0x4AAAAAAAarpkvdua7P4myE"})`，
token 经 `$.ajax POST /verify.php` 写 cookie 后 `location.reload()` 出结果。
首页无此挑战，搜索页有。**必须用页面导航式 POST（表单提交）让 widget 渲染**——
raw fetch 拿不到 widget 交互能力（已实现 `post.mode="navigate"`）。

### 3. 400030 是环境风控，与指纹无关（对照实验）
同一链路在以下环境全部同样失败（iframe 空文档/无 token）：
- camoufox headless + Firefox UA
- camoufox headless + Chrome UA（wire+JS 均 Chrome + sec-ch-ua）
- 真实 Chrome headed（Playwright 启动，带自动化标记）
- **真实 Chrome headed 纯手工启动（`--remote-debugging-port` attach，`navigator.webdriver=false`，真实 profile）**
- 官方 demo sitekey（`0x4AAAAAAABGpllqO9XmdphoA`）本地页同样不渲染

出口 IP：`69.63.202.161`，DMIT Cloud Services（AS906）美西数据中心。
Turnstile 挑战平台对数据中心 IP 直接返回 400030 fail 事件。

## 响应特征（供后续比对）
- 挑战页 URL：`https://www.69shuba.com/modules/article/search.php`（POST 后不变）
- 挑战页标题：`69书吧`（不是 "Just a moment"）
- 挑战页 DOM：`#cfts` 容器 + `input[name=cf-turnstile-response]`（值恒空）
- widget iframe：`https://challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/turnstile/f/av0/rch/<id>/<sitekey>/auto`（200，265KB 文档，body 空）
- fail 文档：`<script>var errCode = 400030; postToParent({source:'cloudflare-challenge', event:'fail', code:400030})</script>`（~1KB）
- 页面永不复载、/verify.php 永不被调用

## 建议
1. **住宅 IP 代理**：camoufox `AsyncNewContext(proxy={...})` 已支持 per-context 代理
   （自动派生 WebRTC IP/timezone）。读者端可配 `READER_CAMOUFOX_PROXY` 之类环境
   变量后全自动。**需在住宅 IP 下重跑 `scripts/_test_69shuba_nav.py` 验证**。
2. Rust 侧（camoufox.rs）已默认 Chrome wire UA（`READER_CAMOUFOX_UA` 可覆盖）；
   若需搜索 POST 全自动，后续把 `post` 字段透传到 camoufox::solve（当前 CfSolution
   无 postResult 字段——非本次范围）。
3. 书源 69shuba.json 的 ruleSearch 已有 startBrowserAwait CF 兜底逻辑，浏览器后端
   通过后无需改书源。

## 已落地代码（HEAD 已含）
- `scripts/camoufox_solver.py`：UA 覆盖（config_overrides + 回退补丁 + sec-ch-ua）、
  `post` 字段（`fetch` 默认 / `navigate` 表单提交 + 二次质询循环 + GBK 表单体解析）、
  点击逻辑修复（hasInput 存在但值恒空时仍点击；iframe 缺失时点 `.cf-turnstile` 容器）、
  质询特征增加 `#cfts` / `[name=cf-turnstile-response]`、post 循环等文档 readyState
- `src/service/camoufox.rs`：默认 Chrome/131 Windows UA（与 CDP 路径一致，
  `READER_CAMOUFOX_UA` 可覆盖）+ 单测 `test_solve_ua_default_and_env`
- 测试脚本：`scripts/_test_69shuba_live.py`（fetch 模式）、`scripts/_test_69shuba_nav.py`（navigate 模式）
- 证据：`scripts/_evidence_turnstile_400030_iframe_doc.html`、
  `scripts/_69shuba_final_challenge_page.html`

## 回归（全部通过）
- `python -m py_compile scripts/camoufox_solver.py`
- `/health` 200；mock-cf-site（8193）求解过；mock-turnstile-site（8194）点击→token→内容页
- `cargo test --lib service::camoufox`：4 passed（含新增 UA 单测）
