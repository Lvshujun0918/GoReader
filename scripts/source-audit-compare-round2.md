# 书源二次检测报告（对比首轮）

- 测试库: `target/search-test/storage/reader.db`（当前 344 书源，342 启用 / 2 禁用）
- 服务: http://127.0.0.1:8084（READER_APP_WORKDIR=target/search-test, READER_APP_SECURE=true, READER_SERVER_PORT=8084）
- 审计: `scripts/source-audit.py`（只读），关键词「诡秘之主」，并发 8，限速 ≤6 req/s，探测 8s / SSE 75s
- 首轮: 2026-08-04 19:33（431 源）；二轮: 2026-08-05 22:53（344 源）；两轮按 book_source_url 全量匹配（344/344）

## 1. 总览

| 分类 | 首轮(431) | 二轮(344) | 口径说明 |
|---|---|---|---|
| normal | 62 | **73** | 有结果，或明确「站内无此书」 |
| rule_engine_error | 259 | 245 | HTTP 正常但 0 结果 / 解析异常 / JS 报错 |
| site_down | 98 | 13 | DNS/连接/超时/TLS/4xx/5xx |
| no_search_url | 12 | 12 | 未配置 searchUrl（两轮完全一致） |
| audit_error | 0 | 1 | 天下书音：SSE 被本机 ESET 拦截(403)，非源问题 |

> 注意：主进程首轮后已移除 87 个书源，**全部为首轮 site_down**（这正是 98→13 的主因，非引擎改善）。
> 两轮均在相同 344 个源上对比（87 个已移除源不参与逐源对比）。

### 引擎错误类型对比（规则引擎问题）

| 错误类型 | 首轮 | 二轮(344 库) | 说明 |
|---|---|---|---|
| zero_results | 129 | 114 | HTTP 200 但 0 结果且无「无结果」特征 |
| js | 69 | 70 | JS 执行失败（shim 缺口为主） |
| other | 61 | 61 | 其他规则/解析异常 |
| 合计 | 259 | 245 | |

## 2. 逐源迁移矩阵（344 匹配源）

| 首轮 → 二轮 | 数量 | 含义 |
|---|---|---|
| rule_engine_error → normal | **29** | **改善：引擎修复生效** |
| normal → normal | 44 | 保持正常 |
| rule_engine_error → rule_engine_error | 227 | 仍引擎问题（详见 §4） |
| normal → rule_engine_error | **18** | 新增引擎问题（回归/站点变化，详见 §5） |
| site_down → site_down | 10 | 仍站挂（全部 http_403） |
| rule_engine_error → site_down | **3** | 站挂（新增）：有度中文/英语阅读/69书吧 |
| site_down → audit_error | 1 | 天下书音：审计链路被 ESET 拦截（首轮即 403 站挂） |
| no_search_url → no_search_url | 12 | 旁路，两轮一致 |

### 引擎错误类型变化（227 仍失败源中）

| 类型变化 | 数量 | 解读 |
|---|---|---|
| zero_results → zero_results | 94 | 依旧 0 结果（规则/站点侧） |
| other → other | 61 | 依旧 other 错误 |
| js → js | 48 | 依旧 JS 失败 |
| zero_results → js | 13 | 现在能执行 JS 但 shim 缺口暴露（引擎进步但未完成） |
| js → zero_results | 11 | JS 不再崩，但搜索 0 结果（规则/站点侧） |

## 3. 改善清单（29，首轮问题 → 二轮正常）

| 书源 | 首轮类型 | 二轮结果 |
|---|---|---|
| 中华诗词（优+） | js | 明确站内无此书（响应体命中特征「没有搜索到」） |
| 企鹅浏览 | zero_results | 搜索到 21 条结果 |
| 企鹅浏览 | zero_results | 搜索到 21 条结果 |
| 企鹅浏览（优） | zero_results | 搜索到 21 条结果 |
| 企鹅阅读 | zero_results | 搜索到 10 条结果 |
| 全本小说（优++） | js | 搜索到 20 条结果 |
| 刺猬猫吧 | zero_results | 搜索到 10 条结果 |
| 喜马拉雅（优+） | zero_results | 搜索到 12 条结果 |
| 夜天连看（优） | js | 搜索到 50 条结果 |
| 天涯知识（优+） | js | 明确站内无此书（响应体命中特征「没有找到」） |
| 安轻小说 | zero_results | 搜索到 4 条结果 |
| 心轻小说 | js | 搜索到 4 条结果 |
| 懒人听书（优+++） | zero_results | 搜索到 12 条结果 |
| 无奈书库（优） | js | 搜索到 30 条结果 |
| 滴答漫画（优） | zero_results | 搜索到 1 条结果 |
| 爱久久网（优+） | js | 明确站内无此书（响应体命中特征「没有搜索到」） |
| 猫九小说 | zero_results | 搜索到 18 条结果 |
| 猫九小说 | zero_results | 搜索到 12 条结果 |
| 猫眼看书（优++） | js | 搜索到 15 条结果 |
| 疯读小说（优++） | zero_results | 搜索到 4 条结果 |
| 疯读小说（优+） | zero_results | 搜索到 4 条结果 |
| 腾讯漫画 | zero_results | 搜索到 9 条结果 |
| 腾讯漫画 | zero_results | 搜索到 9 条结果 |
| 腾讯漫画 | zero_results | 搜索到 9 条结果 |
| 腾讯漫画  | zero_results | 搜索到 9 条结果 |
| 苏轻小说 | zero_results | 搜索到 4 条结果 |
| 轻菠萝包 | zero_results | 搜索到 4 条结果 |
| 长佩 | zero_results | 搜索到 10 条结果 |
| 阅友小说（优+） | js | 搜索到 5 条结果 |

改善明细：搜索到结果 26 个（企鹅浏览×2/企鹅阅读/全本小说/刺猬猫吧/喜马拉雅/夜天连看/安轻小说/心轻小说/懒人听书/无奈书库/滴答漫画/猫九小说×2/猫眼看书/疯读小说×2/腾讯漫画×4/苏轻小说/轻菠萝包/长佩/阅友小说），明确站内无此书 3 个（中华诗词/天涯知识/爱久久网）。

## 4. 仍引擎问题（227，按二轮错误类型分组）

### zero_results（105）

- 七点小说  `https://7xdian.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=82923）
- 七百小说（优+）  `https://m.x700txt.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=3638）
- 个性说网（优+）  `https://www.gexingshuo.com#一程`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=47581）
- 中文万维（优+）  `http://cread.com#`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=2）
- 九阅小说  `https://api.9yread.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=55）
- 书旗小说  `https://www.shuqi.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=31716）
- 二八看书（优++）  `https://www.28lu.net/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=3556）
- 五丁音乐（优+）  `http://5sing.kugou.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=6929）
- 企鹅阅读  `https://bookshelf.html5.qq.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=11121）
- 优品学习（优）  `https://www.ypppt.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=3863）
- 优品文档（导+）  `https://www.ypppt.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=3863）
- 优质资源（优）  `https://www.hdzyk.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=280）
- 全免漫画（优）  `https://api-cdn.kaimanhua.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=191）
- 全免漫画（优）  `https://api-cdn.kaimanhua.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=191）
- 全本小说（优）  `https://www.qb5.io`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=0）
- 八零小说（优++）  `http://wap.80zw.la/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=744）
- 包子漫画（优）  `https://cn.baozimhcn.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=146019）
- 南极小说（优）  `🐧`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=28006）
- 双语小说（英）  `http://www.shubang.net/book#`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=6320）
- 古典文学（优+）  `http://yz4.chaoxing.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=2）
- 古诗文网（优+）  `https://m.gushiwen.cn/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=61）
- 古诗文网（优+）  `https://m.gushiwen.cn`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=61）
- 古诗词网（优+）  `https://m.gushici.net/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=15710）
- 名著阅读（优++）  `https://api.520diandu.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=64）
- 吾爱破解（优+）  `https://www.52pojie.cn`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=19462）
- 商店小说（优）  `http://www.16kbook.net`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=12274）
- 喜马拉雅（优）  `http://search.ximalaya.com已校验`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=1627）
- 四三看书（优）  `http://m.43kanshu.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=4184）
- 墨辰整理书源系列7.0版  `墨辰整理书源大全`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=10124）
- 天地中文  `http://www.tiandizw.com`  — HTTP 200 但搜索 0 结果；重放失败（bodySize=None）
- 奇漫屋子（优）  `https://m.qimanwu.app`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=4364）
- 如漫画网（优+）  `http://m.rumanhua1.com/`  — HTTP 200 但搜索 0 结果；重放失败（bodySize=None）
- 安稳小说  `http://xiaoshuo.uc.cn`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=13325）
- 快看漫画  `http://m.kuaikanmanhua.com#未月十八发现`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=69）
- 快看漫画  `http://m.kuaikanmanhua.com#♤Haxc`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=69）
- 快看漫画  `https://m.kuaikanmanhua.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=69）
- 快看漫画  `http://m.kuaikanmanhua.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=69）
- 收费漫画  `https://mm.sfacg.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=26）
- 新读小说（繁）  `https://m.dxs.tw/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=22036）
- 星际漫画（优）  `http://www.xmanhua.com#♤Haxc`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=14367）
- 晋江文学  `https://m.jjwxc.net/channel/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=51）
- 晋江文学  `https://m.jjwxc.net/free`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=91074）
- 晋江文学  `http://android.jjwxc.net       `  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=51）
- 晋江评论  `https://m.jjwxc.net#app`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=51）
- 有度中文  `https://www.yodu.org`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=201733）
- 有度中文（优+）  `https://www.yodu.org##破冰`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=201733）
- 有度轻说（优+）  `https://www.yodu.org/qingxiaoshuo`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=201733）
- 松鹤庭沐（优）  `https://so.html5.qq.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=11121）
- 松鹤阅读  `DQuestQBall#001`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=11121）
- 次元小说（优）  `https://www.erciyan.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=4184）
- 武芊漫画（优+）  `https://comic.mkzcdn.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=33）
- 水山听书（优+）  `https://m.ting13.com##@遇知`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=45）
- 求书帮吧（优）  `https://www.qiushubang.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=3568）
- 海洋听书（优）  `http://www.ychy.org`  — HTTP 200 但搜索 0 结果；重放失败（bodySize=None）
- 漫播听书（优）  `https://api.kilamanbo.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=156）
- 漫画搬运（优+）  `https://www.antbyw.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=44485）
- 爱去小说（导）  `https://www.527txt.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=7273）
- 爱发电网（优）  `https://afdian.com#`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=56）
- 爱发电网（优）  `https://afdian.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=56）
- 爱奇艺漫  `https://www.iqiyi.com/manhua#♤Haxc`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=7595）
- 爱尚小说（优）  `http://www.23hh.la/`  — HTTP 200 但搜索 0 结果；重放失败（bodySize=None）
- 爱推书君（优）  `https://pre-api.tuishujun.com/`  — HTTP 200 但搜索 0 结果；重放失败（bodySize=None）
- 爱推书君（优）  `https://pre-api.tuishujun.com`  — HTTP 200 但搜索 0 结果；重放失败（bodySize=None）
- 爱看漫画（优+）  `https://m.kanman.com#♤Haxc`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=33）
- 爱看漫画（优+）  `https://m.kanman.com#Haxc1107`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=33）
- 爱看漫画（优）  `https://m.kanman.com已校验`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=33）
- 爱看漫画（优）  `https://m.kanman.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=33）
- 爱看漫画（优）  `https://m.kanman.com#`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=33）
- 爱看漫画（优）  `https://m.kanman.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=33）
- 独步小说（优）  `https://www.dbxsn.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=1451）
- 猫耳听书（优）  `https://www.missevan.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=4518）
- 猫耳广播（优）  `https://www.hhlqilongzhu.cn##猫耳FM广播剧`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=4）
- 猫耳有声（优）  `https://www.hhlqilongzhu.cn`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=4）
- 画涯爱子  `http://api.huaya.cc`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=95）
- 番茄短剧（优++）  `https://www.shanhuzs.com/`  — HTTP 200 但搜索 0 结果；重放失败（bodySize=None）
- 百度小说  `https://dushu.baidu.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=5671）
- 百度网盘（优++）  `https://pan.baidu.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=108）
- 盒子游戏（优）  `https://h.4399.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=93126）
- 盗随动漫（优+）  `https://myself-bbs.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=8215）
- 神话之后（优+）  `https://www.shenhuazhihou.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=7207）
- 稀饭动漫（优+）  `https://dm.xifanacg.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=20796）
- 笔尚小说  `https://www.bsxiaoshuo.com#yc1101`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=1893）
- 笔尚小说  `https://www.bsxiaoshuo.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=1893）
- 笔趣小说（优）  `https://m.bqgcn.net`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=23910）
- 米读小说（优+）  `https://api.midureader.com/#@遇知`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=23456）
- 纵横中文  `https://www.zongheng.com/##zhbyjm7783`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=20694）
- 网易小说  `https://m.yuedu.163.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=17175）
- 网络漫画（优）  `https://mm.sfacg.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=26）
- 花生小说（优）  `https://api.wan123x.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=0）
- 若初文学  `https://search.ruochu.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=16145）
- 英文小说（英）  `https://www.yingyuxiaoshuo.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=46929）
- 英文小说（英）  `http://novel.tingroom.com/wap`  — HTTP 200 但搜索 0 结果；重放失败（bodySize=None）
- 言情港吧（优）  `https://www.yanqinggang.com#`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=2431）
- 话本小说（优++）  `https://www.ihuaben.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=294）
- 贼吧网玩（导）  `https://www.zei8.vip`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=21）
- 起点中文  `https://www.qidian.com`  — HTTP 202 但搜索 0 结果；重放响应体未见无结果特征（bodySize=209）
- 超星网站（优+）  `http://yz4.chaoxing.com#`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=2）
- 轻说百科（优++）  `https://lnovel.tw`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=35325）
- 追光阅读（优）  `http://touchlife.cootekservice.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=109）
- 追光阅读（优）  `http://touchlife.cootekservice.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=109）
- 酷匠阅读（优+）  `https://app.kujiang.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=0）
- 酷我小说  `http://appi.kuwo.cn/novels/api/book`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=127）
- 酷我小说  `http://appi.kuwo.cn#`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=127）
- 霸王游戏（优）  `https://www.yikm.net`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=26221）
- 霸王街机（优）  `https://www.yikm.net/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=26221）

### js（61）

- 七猫小说（优++）  `https://api-bc.wtzw.com＃妍希`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 七猫小说（优++）  `https://api-bc.wtzw.com#yc1101b`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 七猫小说（优++）  `https://api-bc.wtzw.com#md`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 七猫小说（优+）  `https://api-bc.wtzw.com#♤ycb`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 七猫小说（优+）  `https://api-bc.wtzw.com/`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 万通蜡笔（优）  `https://wtr-lab.com`  — URL 构造失败: JS 执行失败: ReferenceError: org is not defined
- 中文看书（优）  `http://wap.zwkan.com`  — URL 构造失败: JS 执行失败: ReferenceError: org is not defined
- 乐乎文章（优）  `https://newsmiss.lofter.com`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 全免小说（优++）  `http://qmbook.taoyuewenhua.net/#@遇知`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 全免漫画（优）  `https://api-cdn.kaimanhua.com/##@遇知`  — 规则应用失败: JS 执行失败: ReferenceError: urlIP is not defined
- 全本小说（优）  `http://www.xqb5.cc`  — URL 构造失败: JS 执行失败: Error: java.ajax: url 不能为空
- 八零电子（导）  `https://m.txt80.cc/`  — 规则应用失败: JS 执行失败: ReferenceError: org is not defined
- 哔哩哔哩（优+++）  `哔哩哔哩`  — URL 构造失败: JS 执行失败: ReferenceError: getWbiEnc is not defined
- 喜马拉雅（优+）  `https://www.ximalaya.com/#乃星`  — 规则应用失败: JS 执行失败: ReferenceError: org is not defined
- 城堡小说（优+）  `https://www.96cbtxt.com/`  — URL 构造失败: JS 执行失败: Error: java.ajax: url 不能为空
- 大唐小说（优+）  `https://www.dtxsw.com`  — 规则应用失败: JS 执行失败: ReferenceError: cookie is not defined
- 天天书吧（优+）  `https://www.ttshu8.net`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 天天书吧（优+）  `https://m.ttshu8.com`  — 规则应用失败: JS 执行失败: ReferenceError: xGorgon is not defined
- 天天小说（繁++）  `https://ttks.tw/`  — 规则应用失败: JS 执行失败: TypeError: not a callable function
- 尔集小说（优++）  `https://www.esjzone.cc`  — 规则应用失败: JS 执行失败: TypeError: not a callable function
- 拷贝漫画（优++）  `https://www.mangacopy.com/`  — 规则应用失败: JS 执行失败: TypeError: not a callable function
- 无忧书城（优）  `https://www.51shucheng.net`  — URL 构造失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to object
- 时代音乐（优+）  `http://www.78497.com/`  — 规则应用失败: JS 执行失败: ReferenceError: org is not defined
- 曼哈漫画（优+）  `https://www.mangabz.com`  — 规则应用失败: JS 执行失败: TypeError: not a callable function
- 民间故事（优+）  `https://www.6mj.com`  — 规则应用失败: JS 执行失败: Error: 尚未设置文档内容：请先调用 java.setContent(html)
- 治能寄算（优+）  `https://你觉得还有网址?`  — 规则应用失败: JS 执行失败: ReferenceError: src is not defined
- 海词木稽（优）  `http://dict.cn已整理`  — 规则应用失败: JS 执行失败: ReferenceError: baseUrl is not defined
- 海词精选（优）  `http://dict.cn`  — 规则应用失败: JS 执行失败: ReferenceError: baseUrl is not defined
- 海词词典（优）  `http://dict.cn/`  — 规则应用失败: JS 执行失败: ReferenceError: baseUrl is not defined
- 潇社音乐（优+）  `http://fuciyuanbang.ciyuans.com`  — 规则应用失败: JS 执行失败: ReferenceError: org is not defined
- 炫动小说（优+）  `https://www.xdxss.com`  — URL 构造失败: JS 执行失败: ReferenceError: cookie is not defined
- 爱去小说（导）  `https://www.279txt.com/`  — 规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to object
- 爱淘小说（优++）  `https://tybook.taoyuewenhua.net`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 猫耳听书（优）  `https://www.missevan.com#♤guaner`  — 规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to object
- 猫耳听书（优）  `https://www.missevan.com#活力宝`  — 规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to object
- 猫耳听书（优）  `https://m.missevan.com/`  — 规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to object
- 猫耳有声（优）  `https://www.missevan.com#pb1025`  — 规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to object
- 猫耳音乐（优）  `https://www.missevan.com已校验`  — 规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to object
- 番茄小说（优+++）  `https://reading.snssdk.com#lcs`  — URL 构造失败: JS 执行失败: ReferenceError: xGorgon is not defined
- 番茄小说（优+）  `https://reading.snssdk.com#mgz`  — URL 构造失败: JS 执行失败: ReferenceError: xGorgon is not defined
- 百合爱会（优+）  `https://www.yamibo.com/site/novel`  — 规则应用失败: JS 执行失败: Error: java.ajax: url 不能为空
- 网易云说  `http://m.yuedu.163.com#yc1101`  — 规则应用失败: JS 执行失败: SyntaxError: EOF while parsing a value at line 1 column 0
- 艾格动漫（优）  `https://www.agedm.org/search`  — URL 构造失败: JS 执行失败: ReferenceError: org is not defined
- 艾格动漫（优）  `https://www.agedm.org/search#`  — URL 构造失败: JS 执行失败: ReferenceError: org is not defined
- 蓝批小说（优++）  `https://www.pixiv.net/novel`  — URL 构造失败: JS 执行失败: ReferenceError: urlIP is not defined
- 蓝批小说（优++）  `https://www.pixiv.net`  — URL 构造失败: JS 执行失败: ReferenceError: urlIP is not defined
- 蓝批漫画（优++）  `https://www.pixiv.net/manga`  — URL 构造失败: JS 执行失败: ReferenceError: urlIP is not defined
- 车群小说（优）  `http://www.qunxs.com/`  — URL 构造失败: JS 执行失败: Error: java.ajax 失败（http://www.qunxs.com/）: error sending request for u
- 轻之文库（优++）  `https://www.linovel.net#yc1101`  — 规则应用失败: JS 执行失败: ReferenceError: base_url is not defined
- 轻之文库（优+）  `https://www.linovel.net:443/`  — 规则应用失败: JS 执行失败: ReferenceError: cookie is not defined
- 轻文库说（优++）  `轻文库小说`  — URL 构造失败: JS 执行失败: ReferenceError: base_url is not defined
- 酷我音乐（优）  `https://www.kuwo.cn`  — URL 构造失败: JS 执行失败: ReferenceError: type is not defined
- 阅友小说（优++）  `https://goway.reader.yueyouxs.com/`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 阅友小说（优+）  `http://m.suixkan.com`  — 规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to object
- 阅友小说（优+）  `https://sma.yueyouxs.com/`  — 规则应用失败: JS 执行失败: TypeError: not a callable function
- 阅友小说（优）  `http://m.suixkan.com/`  — 规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to object
- 阅读助手（优+++）  `https://api-bc.wtzw.com`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 陶越文华（优++）  `https://qcbook.taoyuewenhua.net`  — URL 构造失败: JS 执行失败: TypeError: not a callable function
- 风云小说（优+）  `https://m.nauqmf.com`  — URL 构造失败: JS 执行失败: ReferenceError: cookie is not defined
- 风扇枕说（日+）  `https://kakuyomu.jp/`  — 规则应用失败: JS 执行失败: TypeError: not a callable function
- 鸠摩搜书（导）  `https://www.jiumodiary.com#`  — URL 构造失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to object

### other（61）

- 三叉小说（优）  `http://m.xxxbiquge.info`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{url=source.
- 个性说网（优+）  `https://www.gexingshuo.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=search/诡秘之主
- 个性说网（优+）  `https://www.gexingshuo.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=search/诡秘之主
- 中小说网  `https://h5.17k.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{java.put("s
- 书单推荐（优+）  `https://www.yuque.com/yuqueyonghun8txcr/psb8yc`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=https://
- 书旗小说  `书旗小说`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{host}}/sq/s
- 书香阁子（优+）  `https://www.sxgread.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{cookie.remo
- 企鹅阅读  `https://ubook.reader.qq.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=api/search?ke
- 八三中文（优+）  `https://www.83zws.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{url=source.
- 刺猬猫网  `https://www.ciweimao.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=get-search-bo
- 包子漫画（优++）  `https://manhuafree.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{bhost()}}/s
- 吉站漫画（优+）  `https://manhuafree.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=https://manhu
- 哎爱巴士  `https://www.ibus233.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
sckey = or
- 哔哩漫画（优+++）  `https://manga.bilibili.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
eval(Strin
- 国学经典（优+）  `https://guoxue.wunpu.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{cookie.remo
- 图书迷子（优）  `https://www.tushumi.cc`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{String(java
- 大美书网（优）  `https://www.dameishuwang.net/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=
- 天天小说（优+++）  `http://ttk.tw`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{
	var su=so
- 天悦小说（优）  `https://www.xtyxsw.org`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
let ul = s
- 太极小说（优+）  `https://69shux.co/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=search/%E8%AF
- 奥尔中文（优）  `https://www.83zws.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{cookie.remo
- 存档书库（优）  `https://archive.org`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
eval(Strin
- 安读书网（优+）  `https://www.88haoshu.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{url=source.
- 小米广播（优+）  `https://fm.music.xiaomi.com/fm`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=null
- 开心漫画（优+）  `https://www.kaixinman.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{cookie.remo
- 微博书源（优+）  `https://m.weibo.cn`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=
- 微博评论（优+）  `https://m.weibo.cn#`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=
- 我爱读者（优）  `https://www.52dzxy.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=#
- 我的书城（优++）  `https://wodushu.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{cookie.remo
- 月趣动漫（优+）  `https://www.qdm66.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
if(page==1
- 桔纸书屋（优+）  `https://m.juzhishuwu.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=modules/artic
- 棉花小说（优）  `http://www.mianhuatang8.net/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=jscookie.remo
- 永远漫画（优）  `https://www.yydsmh.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{cookie.remo
- 潇社音乐（优+）  `http://fuciyuanbang.ciyuans.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=fuciyuanphp/s
- 火球书库（优）  `http://www.huoqwk.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{url=source.
- 爱下电子（优）  `https://apiv2hans.aixdzs.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{source.getK
- 独步小说（优+）  `https://www.dbxsd.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=plus/search.p
- 猫眼看书（优++）  `http://download.yichnmedia.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error for url (data:;base64
- 猫眼看书（优）  `http://api.lfdapengu.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{source.book
- 番茄小说（优+）  `https://m.zym888.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
cookie.rem
- 番茄小说（优+）  `https://fq-book.netsite.cc`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{source.book
- 白浅小说（优）  `https://m.178yhr.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{url=source.
- 盗墓笔记（优+）  `http://www.daomubiji.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=-
- 看看阅读  `http://kkcc.top/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=api/search?ke
- 知轩藏书（导）  `https://zxcs.zip/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=search?q=诡秘之主
- 矮贼吧网（优）  `https://www.zei8.me#`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=e/sch/index.p
- 笔下文学（优+）  `https://www.17bxwx.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=search.html
- 红牛小说（优）  `https://www.songdalaw.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{cookie.remo
- 红薯网站  `https://g.hongshu.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=bookajax/sear
- 绿色小说（优）  `http://www.greentxt.net`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{url=source.
- 网飞猫网（优+）  `https://www.ncat1.app/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
uro=source
- 起点中文（优++）  `https://m.qidian.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL={{hq}}/search
- 轻次元姬  `https://www.ciyuanji.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=search/诡秘之主_0
- 轻说机翻（优+）  `https://n.novelia.cc`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
var provid
- 过期杂志（优+）  `https://www.52dzxy.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=#
- 零点看书（优）  `http://www.biqumx.com`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
cookie.rem
- 霓虹漫画（优）  `https://rawkuma.com/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=page/1/?s=诡秘之
- 青花鱼评（优）  `http://45.79.102.135/allcp.org/###`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=search.php?se
- 顶点小说（优）  `https://www.23ddw.net/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
cookie.rem
- 饭角有声（优）  `https://api.fanjiao.co/`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
body = "ke
- 魔陌音乐（优）  `魔音-MORIN`  — 请求 URL 非法（reqwest builder error）——searchUrl 含未编码/非法字符: builder error; 构造 URL=js
var api = 


### 仍失败源中错误类型变化者（24）

- 二八看书（优++）：js → zero_results（HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=3556））
- 全免漫画（优）：zero_results → js（规则应用失败: JS 执行失败: ReferenceError: urlIP is not defined）
- 八零电子（导）：zero_results → js（规则应用失败: JS 执行失败: ReferenceError: org is not defined）
- 南极小说（优）：js → zero_results（HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=28006））
- 名著阅读（优++）：js → zero_results（HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=64））
- 时代音乐（优+）：zero_results → js（规则应用失败: JS 执行失败: ReferenceError: org is not defined）
- 有度轻说（优+）：js → zero_results（HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=201733））
- 次元小说（优）：js → zero_results（HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=4184））
- 潇社音乐（优+）：zero_results → js（规则应用失败: JS 执行失败: ReferenceError: org is not defined）
- 爱发电网（优）：js → zero_results（HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=56））
- 猫耳听书（优）：zero_results → js（规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to ob）
- 猫耳听书（优）：zero_results → js（规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to ob）
- 猫耳听书（优）：zero_results → js（规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to ob）
- 猫耳听书（优）：js → zero_results（HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=4518））
- 猫耳有声（优）：zero_results → js（规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to ob）
- 猫耳音乐（优）：zero_results → js（规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to ob）
- 百合爱会（优+）：zero_results → js（规则应用失败: JS 执行失败: Error: java.ajax: url 不能为空）
- 网易云说：zero_results → js（规则应用失败: JS 执行失败: SyntaxError: EOF while parsing a value at line 1 colu）
- 言情港吧（优）：js → zero_results（HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=2431））
- 话本小说（优++）：js → zero_results（HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=294））
- 超星网站（优+）：js → zero_results（HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=2））
- 轻之文库（优+）：zero_results → js（规则应用失败: JS 执行失败: ReferenceError: cookie is not defined）
- 轻说百科（优++）：js → zero_results（HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=35325））
- 阅友小说（优+）：zero_results → js（规则应用失败: JS 执行失败: TypeError: not a callable function）

## 5. 新增引擎问题（18，首轮正常 → 二轮失败）

- [js] 书法小说（优+）  `http://www.sfwx.com/`  — 规则应用失败: JS 执行失败: TypeError: not a callable function
- [zero_results] 光社漫畫（优+）  `https://m.g-mh.org/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=55341）
- [js] 全本同人（优）  `https://www.qbtr.me/`  — 规则应用失败: JS 执行失败: ReferenceError: org is not defined
- [zero_results] 刺猬猫吧  `https://wap.ciweimao.com1`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=4642）
- [zero_results] 刺猬猫说  `刺猬猫`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=4642）
- [js] 博览群书（优）  `https://readnovelfull.com/`  — 规则应用失败: JS 执行失败: TypeError: cannot convert 'null' or 'undefined' to object
- [js] 安之原创  `http://www.azycjd.com#yc1101`  — 规则应用失败: JS 执行失败: TypeError: calling a builtin Map constructor without new is forbidden
- [zero_results] 猫耳听书（优+）  `https://www.missevan.com#乃星`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=4518）
- [js] 电线看书（优）  `https://101kanshu.com`  — 规则应用失败: JS 执行失败: Error: java.ajax: url 不能为空
- [zero_results] 百度知道（优）  `https://zhidao.baidu.com/msearch/#乃星`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=51）
- [zero_results] 百度知道（优）  `https://zhidao.baidu.com`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=51）
- [zero_results] 百度知道（优）  `https://zhidao.baidu.com/msearch`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=51）
- [js] 荔枝有声（优）  `https://m.lizhi.fm/`  — 规则应用失败: JS 执行失败: ReferenceError: type is not defined
- [zero_results] 轻之文库（优++）  `https://www.linovel.net`  — HTTP 200 但搜索 0 结果；重放失败（bodySize=None）
- [js] 轻之文库（优+）  `https://www.linovel.net#yc`  — 规则应用失败: JS 执行失败: TypeError: not a callable function
- [zero_results] 追书神器  `http://zhuishushenqi.com/`  — HTTP 200 但搜索 0 结果；重放响应体未见无结果特征（bodySize=22042）
- [js] 速读谷子（优++）  `https://www.sudugu.org/`  — 规则应用失败: JS 执行失败: TypeError: not a callable function
- [js] 野蛮漫画（优）  `https://mhbao.colacomic.com`  — 规则应用失败: JS 执行失败: ReferenceError: getWbiEnc is not defined

## 6. 站挂

### 仍站挂（10，全部 http_403 反爬拦截）

- 九怀小说  `https://www.jiuhuaiwenxue.com`
- 九怀文学  `https://www.jiuhuaiwenxue.com/`
- 国学書库（优+）  `https://book.mywebos.cn`
- 百度贴吧（优+）  `https://tieba.baidu.com#guaner`
- 百度贴吧（优）  `https://tieba.baidu.com#乃星`
- 百度贴吧（优）  `https://tieba.baidu.com#♤guaner`
- 百度贴吧（优）  `https://tieba.baidu.com`
- 篱笆好文（优+++）  `https://m.libahao.com`
- 裤裤漫画（优）  `http://www.kukuc.net`
- 酷安应用（优+）  `https://api.coolapk.com`

### 新增站挂（3，首轮引擎问题 → 二轮站挂）

- 有度中文（优++）  `https://www.youduzw.com`  — 请求失败: 内置浏览器解 CF 质询失败（https://www.youduzw.com/sa）: 内置浏览器求解失败: Turnstile 验证超时（30s）
- 英语阅读（英）  `http://m.enread.com#`  — 请求失败: error sending request for url (http://m.enread.com/index.php?keyword=%E8%A
- 🌙 69书吧  `https://www.69shuba.com`  — 请求失败: 内置浏览器解 CF 质询失败（https://www.69shuba.com/modules/article/search.php）: 内置浏览器求

### 审计链路异常（1）

- 天下书音（优）`https://m.shuyinfm.com`：二轮 SSE 调用被本机 ESET Security 拦截（HTTP 403 Blocked by ESET Security），重查确认非源问题；首轮为 http_403 站挂。

## 7. 错误根因聚类（可修线索）

### JS 错误子类型分布（70）

-  21  TypeError: not a callable function
-  11  TypeError: cannot convert 'null' or 'undefined' to object
-   9  ReferenceError: org is not defined
-   4  Error: java.ajax: url 不能为空
-   4  ReferenceError: urlIP is not defined
-   4  ReferenceError: cookie is not defined
-   3  ReferenceError: xGorgon is not defined
-   3  ReferenceError: baseUrl is not defined
-   2  ReferenceError: base_url is not defined
-   2  ReferenceError: getWbiEnc is not defined
-   2  ReferenceError: type is not defined
-   1  ReferenceError: src is not defined
-   1  Error: 尚未设置文档内容：请先调用 java.setContent(html)
-   1  SyntaxError: EOF while parsing a value
-   1  TypeError: calling a builtin Map constructor without new is forbidden
-   1  Error: java.ajax 失败（http://www.qunxs.com/）: error sending request for url (http:

## 8. 建议

**建议禁用（站点侧已确认不可用，非引擎问题）**：
- 仍站挂 10 个（http_403 反爬，两轮一致）：九怀小说/九怀文学/国学書库/百度贴吧×4/篱笆好文/裤裤漫画/酷安应用
- 天下书音（优）：首轮 403 站挂 + 二轮被 ESET 拦截无法审计；若 ESET 白名单放行可复查一次再定

**建议保留观察（新增站挂 3 个，可能是临时反爬/CF 质询）**：有度中文（CF 质询失败）、69书吧（CF 质询失败）、英语阅读（DNS 解析失败）

**可修（引擎侧，JS shim 补齐即可覆盖 70 个 js 错误的大多数）**：
- `not a callable function`（21）——shim 函数签名/绑定问题，最高优先级
- `cannot convert null/undefined to object`（11）——Object.keys/entries 等对 null 入参的容错
- `ReferenceError: org/urlIP/cookie/xGorgon/baseUrl/base_url/getWbiEnc/type/src is not defined`（29）——补全局 shim
- `java.ajax: url 不能为空`（4）——ajax 参数校验 shim
- `Map constructor without new`（1）、`SyntaxError: EOF while parsing a value`（1，JSON.parse 容错）

### other 错误（61）——全部同一根因：URL 构造失败

61 个 other 错误全部为 `请求 URL 非法（reqwest builder error）`，按 searchUrl 特征分三类（引擎侧可一次性修复）：
- 51 个：searchUrl 含非法/未解析字符（如 `forum::`、`{{source.bookSourceUrl}}` 模板变量未解析）
- 9 个：searchUrl 含未编码中文（其中多个为 `js`/`@js` 前缀的 JS 构造 URL 未被执行）
- 1 个：`{{host}}` 模板变量未替换（书旗小说）

### zero_results 错误（105）

HTTP 200 但 0 结果且重放响应体无「无结果」特征；多为站点侧反爬/接口变更或规则选择器失效，需逐源排查（audit 已排除「站内无此书」误判）。

