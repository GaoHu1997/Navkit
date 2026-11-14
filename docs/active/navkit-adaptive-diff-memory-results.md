# 自适应内存预算diff实施与对比（进行中）

> 一期代码与最终本地测量已完成，**保留决策仍待用户审阅，不能称全数据形状性能闸门通过**。不提交。
>
> [最终34组/82次进程对比表](<navkit-adaptive-diff-memory-measurements.md>)、[CSV数据](<navkit-adaptive-diff-memory-comparison.csv>)；旧默认/旧固定低内存与自动/手动预算同条件对照，核算峰与RSS分列。
>
> [本轮方案](<navkit-adaptive-diff-memory-budget.md>)；[之前七链路对比](<navkit-performance-comparison.md>)不被本轮试跑数值覆盖。

## 实现范围

- [预算控制器](<../../src-tauri/src/diff_memory_budget.rs>)：Windows有效物理/commit可用内存保守最小值；Linux可见cgroup/可用内存，未认证平台明确拒绝。自动任务ceil约可用余量25%，共享池40%，保留10%；manual最高额度受共享池/有效资源限制。上限不是预分配。
- [新比较管线](<../../src-tauri/src/sync_diff.rs>)：仅原生文件SQLite，两侧各自固定只读快照；新[HashMap Store](<../../src-tauri/src/sync_diff_store.rs>)按需收费、rehash预留新旧容量、保留精确键和差异语义。额度不足auto/manual同次读取逐块迁移原disk索引，speed明确拒绝而不无限增长。
- 新UI“内存预算控制”：auto/manual，关闭是speed保底保护；非SQLite保留原兼容调用。请求冻结、晚到preview/result隔离；可预估有效available与请求额度，结果显示当时实际ceiling/核算峰/结束占用/路径/溢写。见[API](<../../src/core/api.ts>)、[SyncDialog](<../../src/dialogs/SyncDialog.ts>)、[UI/API测试](<../../tools/tests/sync-diff-option.test.mjs>)。
- 原`diff`/`diff_bounded`调用保持兼容，旧报告不序列化新增memory:null。新控件不改变同步写入，后端重新采样并校预算，前端不是资源授权。

## 必须说明的边界

1. 最低64MiB，48MiB是保守暂存承诺、未物理预分配。十分宽的行还须过最坏暂存guard，拒绝而不截断。期初64MiB不是整个进程RSS硬上限。
2. 额度≥512MiB且共享池允许时，有限预读8行；两reader最坏额外112MiB承诺**先计入预算**，原native8MiB单行限制不放宽。其余单行预读。实际驻留常低于核算预留。
3. 已收到的IPC消息/驱动/raw行/allocator滞留、报告IPC不属于严格核算的任务heap。保守索引容量和临时预留峰，不是实测RSS。
4. 真实reader关闭、maps退场与scratch清理后归还额度，成功report.used_bytes=0。调用方drop不是强杀，owned任务安全继续至settle，activity/DB owner也保留；当前不支持即时停止在途只读比较。
5. 第一期全局池只覆盖新预算diff，不冒称所有七链路任务统一硬限。旧legacy None仍保旧语义，manual模式关闭不是资源绕过。
6. 可用采样1秒缓存、批边界压力检查，初始任务ceiling冻结，不把自身正常分配使available下降再重复乘25%缩上限。压力/采样失效时停增长，必要溢写/错误；不把尚未回收内存称已受控。

## 真实初轮错误和修正

- 首次controller6通过；diff19通过/2失败/6ignored。新100k数据fixture缺`FROM n`；共用oracle误把新的file-only模式跑进原private-memory case。局部修正后21通过/0失败；旧memory default/bounded仍测不删断言。
- 只读审查发现显式mapping在1MiB校验前clone，已增加借用请求的列数、身份、WHERE和映射字节检查，在owned复制与新管线开始前拒绝；超大target_type/1025mappings测试不允许先花预算外大块内存再报错。
- 初pilot：新自动2.51s，旧默认1.80s，约39%回退，不能因零spill当成功。仅有限预读并未解决（2.45s）；随后预编译列定位、相同原始值直接判相同，避免确定相同数字/文本重复分配与BigDecimal解析。仍执行数值扩展等保护，变化值保持原精确归一化。
- 第三pilot：默认1.799548s/自动1.861311s，手动足够1.862443s，speed1.873224s；auto零spill，64MiB手动10.232171s/disk。**单次试跑不作为最终收益结论**；后面数据才用于用户保留决策。

## 验证与正在运行的对照

- 完整初候选Rust：1307通过/0失败/56ignored（后续新增shape/有限预读/同值快路径仍须最终完整复跑）。
- 完整前端：1337通过/0失败/1skipped；相关71通过，TypeScript/Vite构建通过。浏览器是隔离mockIPC，不冒称原生硬件曲线。
- 最终对照脚本：[run-benchmarks](<../../.tmp/adaptive-diff-implementation/run-benchmarks.ps1>)、[独立进程用例](<../../src-tauri/src/sync_adaptive_memory_bench.rs>)、[汇总](<../../.tmp/adaptive-diff-implementation/summarize.py>)。
- 同一最终源码：每进程只跑一条实际公共入口，default/fixed_low/auto/manual足够/manual64MiB/speed；10万窄表5轮、50/1000行3轮、宽字段和百万行各1轮（规模用例仅观测，不冒称稳定分位数）。
- 输出同时包含 wall time、whole-process RSS基线/峰/增量、有效available、任务ceiling、含暂存预留的核算peak、溢写主文件真实字节、backend、pressure_events。所有报告校同样缺10/多1/变化3及完整键样本。
- 初始化表/数据在计时前；采样20ms，短任务会漏瞬时峰。没有真实厂商PG/MySQL、网络/服务器内存曲线，不把available或task budget当进程实际峰。

## 最终一期结论与保留建议

- 10万窄表5次中位：旧默认1.808112s/RSS104.50MiB；旧固定低预算9.502948s/36.10MiB；新auto1.854495s/87.10MiB、zero spill；新manual2GiB1.856158s/87.23MiB、zero spill。新足额路径相对旧默认仅约2.6%时间回退，内存少约16.7%；比旧极低预算快约5.12倍。**这一组符合预算足够时速度相近的目标，不外推所有数据。**
- 手动64MiB：10万窄表10.332925s/RSS41.84MiB，临时文件15.01MiB，核算峰60.17MiB（含暂存reservation，非RSS）。百万行单次95.445511s/RSS41.84MiB，证明小额度按同次读取溢写，但成本高，不当常规默认。
- 百万窄表单次：默认20.464807s/828.47MiB，auto21.019529s/654.88MiB，约2.7%慢/21%省峰值；只有一轮规模观测，不称稳定统计保证。
- **仍有负优化/未过闸门场景**：50行auto12.739ms vs旧5.537ms（约+7.2ms）；1000行auto25.671 vs20.027ms（约+5.6ms）；1000行/每行额外4KiB文本auto118.015 vs56.441ms；10万行额外256B文本auto2.475695 vs1.969641s（约+25.7%，RSS139.30 vs162.13MiB）。完整扫描预算校验与固定快照/owned工作有代价，不能省掉校验制造普遍提速。
- 自动控制仍显式，checkbox默认不勾选；新SQLite关闭模式是speed保底预算化管线，不代表legacy None，也不代表无限内存。新speed在上述宽表/小表同样有回退，必须纳入保留选择。旧API不带memory参数仍保持原默认算法/能力，不悄悄改参数含义。
- 本地完整候选[1309通过/0失败/56ignored](<evidence/adaptive-diff-20261114/backend-full-final.log#L1999>)；最后[借用shape 1通过](<evidence/adaptive-diff-20261114/borrowed-shape-final.log#L635>)和[预读/控制器7通过](<evidence/adaptive-diff-20261114/controller-tests-final.log#L641>)。完整UI[1337通过/0失败/1skipped](<evidence/adaptive-diff-20261114/frontend-full.log#L1356-L1363>)，相关71通过+[构建通过](<evidence/adaptive-diff-20261114/frontend-build.log#L24>)。无专用PG/MySQL实库、全平台/全入口资源或实际多task混合压力硬限结论。

本轮可供用户决定：保留一期受控策略并接受小/宽表回退；仅显式使用新自动/手动控制且保持旧正常入口；或继续优化宽字段/小任务的快路径后再保留。不能擅自认定已全性能验收完成。

## 收尾验收

1. 最终所有正确性与控件兼容通过；小/大任务额度够不按2048行门槛spill；超额度disk或明确拒绝，不返回不完整结果。
2. 10万窄表新足额模式中位与原默认尽量接近（候选≤10%回退），不足则显式报告并不默认替换。
3. 并发/压力/释放有隔离controller tests，whole-process memory与核算峰分列；大内存机器不固定极低策略，低额度实际触发disk并有真实开销。
4. 产出最新对比表/CSV、可复现日志和新开关截图；已开发能力按生命周期合并结果文档，剩余现场验证标清，等待用户决定，不自动commit。
