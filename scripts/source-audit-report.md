# 书源批量检测审计报告

- 生成时间: 2026-08-05 22:53:30
- 测试库: C:\Users\chong\pr-review\reader-dev\target\search-test\storage\reader.db（共 344 个书源）
- 关键词: 「诡秘之主」 | 并发: 8 | 探测超时: 8s
- 审计方式: 本地服务 http://127.0.0.1:8084 bookSourceDebugSSE（生产规则引擎逐步执行）+ 可达性探测

## 总览

| 分类 | 数量 | 占比 |
|---|---|---|
| 正常（有结果/站内无此书） | 73 | 21.2% |
| 站点挂了（网络/HTTP 错误） | 13 | 3.8% |
| 规则/引擎问题 | 245 | 71.2% |
| 未配置 searchUrl（旁路） | 12 | - |
| 审计链路异常（旁路） | 1 | - |

## 站点挂了明细（按错误类型）

| 类型 | 数量 |
|---|---|
| http_403 | 10 |
| timeout | 2 |
| network | 1 |

## 规则/引擎问题明细（按错误类型 + 示例源）

| 错误类型 | 数量 | 示例源（前 3） |
|---|---|---|
| zero_results | 114 | 百度网盘（优++）（https://pan.baidu.com）；天地中文（http://www.tiandizw.com）；追书神器（http://zhuishushenqi.com/） |
| js | 70 | 轻文库说（优++）（轻文库小说）；哔哩哔哩（优+++）（哔哩哔哩）；番茄小说（优+）（https://reading.snssdk.com#mgz） |
| other | 61 | 魔陌音乐（优）（魔音-MORIN）；书旗小说（书旗小说）；吉站漫画（优+）（https://manhuafree.com/） |

## 说明

- 分类口径：站点挂了 = DNS/连接/超时/TLS/重定向环（HTTP 层不可达）或 HTTP 4xx/5xx（搜索端点不可用）；
  规则/引擎问题 = HTTP 正常但 0 结果（重放未见无结果特征）/解析异常/JS 报错（css/jsonpath/xpath/regex/js/zero_results/other）；
  正常 = 有结果或明确“站内无此书”（响应体命中无结果特征标记）。
- 本审计只读：未修改/未删除/未禁用任何书源。
- 完整逐源明细见 source-audit-report.json。
