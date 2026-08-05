# 书源批量检测审计报告

- 生成时间: 2026-08-04 19:33:19
- 测试库: C:\Users\chong\pr-review\reader-dev\target\search-test\storage\reader.db（共 431 个书源）
- 关键词: 「诡秘之主」 | 并发: 8 | 探测超时: 8s
- 审计方式: 本地服务 http://127.0.0.1:8086 bookSourceDebugSSE（生产规则引擎逐步执行）+ 可达性探测

## 总览

| 分类 | 数量 | 占比 |
|---|---|---|
| 正常（有结果/站内无此书） | 62 | 14.4% |
| 站点挂了（网络/HTTP 错误） | 98 | 22.7% |
| 规则/引擎问题 | 259 | 60.1% |
| 未配置 searchUrl（旁路） | 12 | - |
| 审计链路异常（旁路） | 0 | - |

## 站点挂了明细（按错误类型）

| 类型 | 数量 |
|---|---|
| network | 44 |
| http_404 | 16 |
| http_403 | 11 |
| timeout | 10 |
| http_400 | 3 |
| http_500 | 3 |
| http_503 | 3 |
| http_422 | 2 |
| redirect_loop | 2 |
| http_521 | 1 |
| http_401 | 1 |
| http_502 | 1 |
| http_492 | 1 |

## 规则/引擎问题明细（按错误类型 + 示例源）

| 错误类型 | 数量 | 示例源（前 3） |
|---|---|---|
| zero_results | 129 | 墨辰整理书源系列7.0版（墨辰整理书源大全）；潇社音乐（优+）（http://fuciyuanbang.ciyuans.com）；酷我小说（http://appi.kuwo.cn/novels/api/book） |
| js | 69 | 轻文库说（优++）（轻文库小说）；哔哩哔哩（优+++）（哔哩哔哩）；全本小说（优）（http://www.xqb5.cc） |
| other | 61 | 魔陌音乐（优）（魔音-MORIN）；书旗小说（书旗小说）；图书迷子（优）（https://www.tushumi.cc） |

## 说明

- 分类口径：站点挂了 = DNS/连接/超时/TLS/重定向环（HTTP 层不可达）或 HTTP 4xx/5xx（搜索端点不可用）；
  规则/引擎问题 = HTTP 正常但 0 结果（重放未见无结果特征）/解析异常/JS 报错（css/jsonpath/xpath/regex/js/zero_results/other）；
  正常 = 有结果或明确“站内无此书”（响应体命中无结果特征标记）。
- 本审计只读：未修改/未删除/未禁用任何书源。
- 完整逐源明细见 source-audit-report.json。
