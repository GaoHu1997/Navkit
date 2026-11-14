# NavKit 性能改动对比表（待用户决定保留）

> 当前为**审阅阶段**，不是全部协议实库验收或已提交版本。用户要求看完对比后再决定保留；本任务不自动提交，也不根据仓库总 diff 回退其他人的已有改动。
>
> 相关：[实施状态](<navkit-data-pipeline-performance-status.md>)、[原始方案](<navkit-data-pipeline-performance.md>)、[可计算数据 CSV](<navkit-performance-comparison-data.csv>)。

## 1. 比较规则

- 同条件数据只比较相同输入、输出结果、持久性与提交语义。除特别注明，均为 Windows、当前脏工作树、SQLite、debug 测试产物、UUID 隔离文件库。
- 导入与SQL全周期按预热后5轮中位数；早期D-1/D-2 diff只有单次观测，最新D-3/D-4追加每策略3次独立进程的真实公共入口/内存对照，仍不能冒称P95或普遍统计保证。CSV预览为有效文件相同前20行，窗口外整文件校验语义变化单列。
- 表中“耗时减少”是 `(旧耗时-新耗时)/旧耗时`；“倍率”是旧耗时/新耗时。新功能没有旧同规模可执行路径时，不给虚构提升百分比。
- RSS 是 **20ms 采样的整个 Rust 测试进程 working set**，不是某任务独占内存、WebView 内存、服务端内存或理论硬上限。2ms 的快路径可漏采瞬时峰，不能把一次采样当绝对保证。
- 以下值属于不同测试场景，不能相加成“全软件加速”。没有实库配置的 MySQL/PG 路径只有代码、本地模拟/门禁测试，不填吞吐数字。

## 2. 有真实数据的对比

| ID | 改了什么 / 条件 | 修改前 | 修改后 | 提高还是降低 | 为什么 / 当前默认 |
|---|---|---:|---:|---|---|
| I-1 | SQLite CSV 导入，1000 行，完整列、普通无trigger/FK表，单事务 | 59.940 ms | 23.039 ms | **2.602×，耗时减少61.56%** | 将每行一次执行减少成有界多值语句；读取、转换和最终提交均计时。满足门禁才启用，默认/冲突/副作用条件不满足时逐行 |
| I-2 | 同上，100000 行 | 10.504273 s | 2.455240 s | **4.278×，耗时减少76.63%** | 每批最多256行/目标约1MiB，保留原完整事务，不能据此外推 MySQL/PG |
| P-1 | CSV预览：23977785字节、200000行有效文件，只返回20行 | 667.592 ms；观测RSS中位峰94.14 MiB | 1.602 ms；观测RSS中位峰20.15 MiB | **明显提高；该样本耗时减少99.76%** | 不再完整读/解析全部行后截断。**不再报告窗口外坏行**，也不是全文件验证；快路径RSS采样可能漏峰 |
| S-1 | 10000行SQL导出单独耗时，1行INSERT对比256行INSERT | 577.878 ms | 570.695 ms | **约1.24%的变化接近噪声，不能证明导出提速** | 组包减少前缀，编码计量却增加工作；兼容逐行仍默认 |
| S-2 | 同一组SQL文件恢复，均为目标完整原子事务 | 2.187588 s | 0.393268 s | **5.563×，耗时减少82.02%** | 多行SQL减少解析与数据库执行次数。只代表该无副作用测试表；真实触发器/默认值的语句语义可能不同 |
| S-3 | 同上“导出+恢复”完整周期 | 2.790693 s | 0.970726 s | **2.875×，耗时减少65.22%** | 主要收益来自后续恢复，不是单纯导出；**显式勾选批量语义**才启用 |
| S-4 | 同上SQL文件体积 | 1087778 B | 579818 B | 体积减少约46.70%，不是时间收益 | 复用INSERT/表/列前缀。NULL、二进制、Unicode、整型极值round-trip测试；默认格式仍逐行 |
| D-1 | 有键diff，50行/侧：旧内存路径对比优化后的bounded路径 | 9.4655 ms | 19.5142 ms | **负优化：慢约2.062×，耗时增加106.16%** | 快照/门禁/有界存储带来固定成本；不能用省内存抵消延迟回退。**默认已恢复兼容路径**，低内存仅显式选择 |
| D-2 | 同上，100000行/侧 | 1.754858 s | 24.227887 s | **负优化：慢约13.806×，耗时增加1280.62%** | 本地磁盘索引、编码与逐对比较昂贵；初版曾42.858719s，批量scratch后虽缓解仍比旧路径慢。不可默认宣称高速 |
| G-1 | 后端原子recipe生成100000行（确认COMMIT与count/distinct） | 旧总量≤500行，**没有同规模路径** | 2.681 s；观测进程峰32.30 MiB | **新增规模能力，不能计算旧/新速度倍率** | 前端只传规则和小样本，Rust逐行生成/绑定，不累计全部rows/PKSet；仍完整单事务 |
| G-2 | 同上1000000行 | 旧总量≤500行，**没有同规模路径** | 28.306 s；观测进程峰32.55 MiB | **新增规模能力**；此次观测总量10×而RSS峰近似平台 | 仅限该数据/进程观测，不证明任意宽表/千万行/其他协议同样峰值；早前74.163s是另一时点，不能当修改前基线制造提升 |
| G-4 | 原子recipe生成10000000行，独立新测试进程 | 旧总量≤500行，无同规模路径 | **279.487 s；观测进程峰32.38 MiB** | 新增千万行能力，不给虚构速度提升 | 10000000行生成/sent/acked/confirmed COMMIT/受控count/distinct全部一致；20ms采样13773次。仍只限该SQLite数据，不外推宽字段、WebView或服务端峰值 |

**证据定位**：
- [I-1](<evidence/performance-20261113/import-ab-1000.log#L590-L605>)，[I-2](<evidence/performance-20261113/import-ab-100000-final.log#L615-L631>)。
- [D-1/D-2](<evidence/performance-20261113/sync-diff-ab-final.log#L615-L620>)，[初版负收益](<evidence/performance-20261113/sync-diff-ab.log#L602-L604>)。
- [P-1预览A/B](<evidence/performance-20261114/preview-ab-20261114.log#L621-L636>)、[S-1至S-4完整周期](<evidence/performance-20261114/sql-cycle-round1.log#L621-L636>)、[G-1/G-2规模RSS](<evidence/performance-20261114/recipe-rss-20261114.log#L621-L628>)、[G-4千万行](<evidence/performance-20261114/recipe-10m-20261114.log#L621-L625>)。
- 可复现入口：[预览A/B](<../../src-tauri/src/transfer_preview_bench.rs>)、[SQL全周期A/B](<../../src-tauri/src/transfer_sql_cycle_bench.rs>)、[recipe规模](<../../src-tauri/src/generated_recipe_tests.rs>)。CSV各行含日志名/标记；[数据复算](<../../.tmp/performance-implementation/build_comparison.py>)只重建本任务派生CSV，不改用户源代码。

### 2.1 用户追加核对：diff速度与内存、复选框位置

本轮直接调用当前[默认diff](<../../src-tauri/src/sync.rs#L749-L755>)与[bounded路径](<../../src-tauri/src/sync_diff.rs>)。每策略/规模运行3次独立新进程，交替启动次序，同样的UUID SQLite文件数据和差异结果；不在低内存进程先跑全量oracle污染allocator。数据为整数主键、短文本和3字节BLOB，不能外推宽表/其他协议。采样20ms，进程工作集包含Rust、SQLx和SQLite缓存，不是WebView/专用服务端或严格独占heap。

| ID / 每侧规模 | 默认耗时中位 | 低内存耗时中位 | 默认RSS观测峰中位 | 低内存RSS观测峰中位 | 实际取舍 |
|---|---:|---:|---:|---:|---|
| D-3 / 50行 | 4.977ms | 11.462ms | 25.83MiB | 26.50MiB | 慢2.30×；这组峰值反而高约0.67MiB（2.6%），没有观测到小任务省内存。低于采样间隔可能漏瞬时峰，不能保证微小差异 |
| D-4 / 100000行 | 1.918499s | 10.340559s | 104.12MiB | 36.15MiB | **省约67.97MiB（65.3%），但慢5.39×**。默认仍不启用低内存 |

10万行的比较前基线分别26.18/26.19MiB，峰值减各轮基线的增量中位分别77.94/9.98MiB，约减少87.2%；**这不等于把整个进程峰值减少87.2%**，也不等于精确每任务内存归属。3次源100000/目标99991，缺10/多1/变化3及主键样本均一致。

[12次测量及来源hash、3轮汇总](<evidence/performance-20261114/diff-memory-three-rounds-20261114.log#L1-L28>)；[原生测量入口](<../../src-tauri/src/sync_diff_memory_tests.rs>)。此前13.8×是另一时点/运行方式观测，本轮只改UI和测试采样，**没有改diff生产算法，不能说调整位置带来提速**。

位置根因：[原CSS](<../../src/styles/base.css>)把所有直接`.sync-table-only`子项设为`margin-left:auto`，复选框被单独推向结构操作组；现[组件](<../../src/dialogs/SyncDialog.ts>)将比较按钮+策略放入同一个“数据差异比较”组，结构操作另组，默认unchecked/默认`{req}`、显式`bounded:true`不变。窄窗工具组换行时采用映射页自然滚动，长结果不覆盖控件，结果区加只读状态通知。

[调整前](<../../artifacts/performance-review/low-memory-diff-location.png>)、[当前正常宽度截图](<../../artifacts/performance-review/low-memory-diff-adjusted-1080.png>)、[390px窄窗](<../../artifacts/performance-review/low-memory-diff-adjusted-390.png>)为真实当前组件+隔离示例IPC，不是旧桌面exe现场。[浏览器回归](<../../tools/tests/sync-diff-option.test.mjs>)验证分组/近邻/默认和显式参数/三种宽度/长结果不遮挡/整库模式隐藏，没有执行同步写入。本轮相关前端[49通过/0失败](<evidence/performance-20261114/diff-group-browser-verified-20261114.log#L51-L58>)、[构建通过](<evidence/performance-20261114/diff-group-build-verified-20261114.log#L24>)；diff语义[13通过/5ignored](<evidence/performance-20261114/diff-memory-semantic-regression-20261114.log#L640>)。前两次新增测试作用域/状态区假设错误已修正，未删除断言或把失败称通过。

## 3. 已改但没有等价性能基线的项目（不伪造数字）

| ID | 改了什么 | 修改前是什么 | 修改后是什么 | 提高还是降低 / 为什么 | 是否默认 / 保留待定 |
|---|---|---|---|---|---|
| C-1 | [数据生成标题](<../../src/dialogs/DataGeneratorDialog.ts>) | `${rows}`把二维数据转成字符串并进任务标题 | 只使用明确行数，不把值带入标题 | 确认减少多余分配与数据泄漏风险；**未做独立耗时测量** | 修正代码已落，待你决定 |
| E-1 | [遗留导出](<../../src-tauri/src/transfer.rs>) / [命令](<../../src-tauri/src/commands.rs>) | 定时/遗留表导出先取全表、再构造完整文本 | 复用现有游标、sink与create-only stage，limit/where/编码保留 | 空间复杂度改善方向可核验；**没有旧/新端到端RSS/吞吐A/B，不称提速**。视图/协议能力按实际门禁，SQL类型比旧全TEXT更严格 | 已用于该入口，需你审阅格式/能力变化 |
| R-1 | [详细还原](<../../src-tauri/src/transfer_restore_stream.rs>) / [固定源预扫](<../../src-tauri/src/transfer_sql_reader.rs>) | 全文件字符串+全部语句+每句报告Vec | 固定源完整预扫/有预算私有spool、50MiB单句、最多2048报告及完整汇总 | 内存不再随语句总数累计，但磁盘预扫/二次摘要/MAC有额外成本；**未测同语义旧/新完整还原A/B** | 详细入口已接；显式事务/CALL/普通SET等不安全兼容脚本会预扫拒绝，旧简单接口仍保留 |
| R-2 | [报告与公开停止](<../../src-tauri/src/commands.rs>) / [界面](<../../src/core/App.ts>) | 回滚前成功回执被算成成功；失败位置用steps长度；没有公开停止任务 | acked/committed/rolled_back/skipped/unknown分开；真实失败index；公开taskid可停止下一句，等待当前回执/回滚/清理 | 正确性/可操作性改善，**不是吞吐优化**；关闭窗口仍后台继续，不假装撤销 | 公开详细task入口已接，不改变旧简单接口 |
| B-1 | [SQLite任务快照](<../../src-tauri/src/transfer_backup_snapshot.rs>) | 每表目录/DDL/游标各次读取，不是整库同快照 | 文件库main的目录、表DDL、所选数据共用一只读事务 | 并发一致性更强；长读事务/WAL与读锁有成本，**新旧语义不同，不能直接报速度提升**。旧独立index/trigger等对象边界不冒称完整实例备份 | 该条件默认使用；其余已有兼容路径保留 |
| B-2 | [PG显式快照](<../../src-tauri/src/transfer_backup_native.rs>) | 普通逐表逻辑转储，未证明跨表同一时点 | 专用单连接RR只读事务、目录/OID锁/nonce、源关闭后才发布 | **未运行PG实库，吞吐/锁开销未知**；结构门禁很窄，nondefault STORAGE/COMPRESSION/访问方法拒绝；这是强语义能力不是免费提速 | 仅`pg_snapshot`显式选择，默认compat |
| B-3 | [MySQL显式源锁](<../../src-tauri/src/transfer_backup_native.rs>) / [说明](<../../src/dialogs/BackupDialog.ts>) | 每表事务，跨表与目录一致性未保证 | 单专用会话一次READ锁+nonce/reader验证；已知trigger/FK依赖拒绝 | **可能降低业务性能**：阻塞源写入/DDL，MySQL可能隐式锁关联表并升级WRITE、阻塞读写；无实库锁负载A/B，不宣称只有所选READ锁 | 仅`mysql_locked`显式选择，需授权与实库验证 |
| N-1 | [PostgreSQL COPY](<../../src-tauri/src/transfer_pg_copy.rs>) | 普通CSV/JSON逐行INSERT，完整事务 | 受限原生PG、已有heap表、完整列/类型白名单，COPY专用协议，完成/abort/回滚/未知关闭不重放 | **无PG实库A/B，无法判定提速**；能力限制严格，ROW EXCLUSIVE而非阻塞SELECT的ACCESS EXCLUSIVE；不能复制非等价默认/trigger/索引表语义 | 仅`pg_copy`显式选择，默认row不变 |
| G-3 | [SQLite分段recipe](<../../src-tauri/src/generated_recipe_segments.rs>) | 原子模式整任务提交/取消回滚全批 | 显式每1000行/8MiB绑定预算，锁内重验结构；失败/取消只回滚当前段，确认前缀保留 | **事务语义改变，不与atomic直接报速度倍率**；降低单段回滚范围但增加COMMIT/元数据成本，性能未测。PG/MySQL分段明确unsupported | 只有勾选“前缀不可撤销”才启用，atomic仍默认 |
| A-1 | [有限生产者准入](<../../src-tauri/src/transfer_admission.rs>) | 原128条日志上限不是并发/内存预算 | 明确import/export producer最多4；205MiB/producer保守预留，合计820MiB账本；超额立即拒绝，取消不早释放 | 资源保护，**不等于RSS硬限或吞吐提速**；第5并发任务会被拒绝，这属于行为变化，必须纳入你的保留决定。recipe/sync/dump没有被全局误限造成嵌套死锁 | 真正顶层producer已接；不无限排队 |
| T-1 | 既有[权限规划](<../../src-tauri/src/db_users.rs>)与[结构夹具清理](<../../src-tauri/src/structure_integration_tests.rs>) | SQL关键字大小写/role期望与实现不一致；pool closed后的size计数有收尾竞态 | 规范SQL关键字、不改变账户/权限集合；夹具按硬deadline等实际资源计数归零 | **验证辅助修正，不报性能收益**；不把原有测试失败误归到传输，也不删失败断言 | 单列辅助改动，便于你决定是否保留 |
| T-2 | [IPC/预览覆盖](<../../src/core/api.ts>) / [开发预览](<../../public/preview-bootstrap.js>) | 新bounded多键破坏原默认请求测试；新recipe命令无mock | 默认仍`{req}`，显式才传bounded；预览不伪造原生fingerprint/grant | 兼容与测试覆盖修正，**不是性能收益**；前端完整回归尚需最新复跑，不拿旧通过数替代 | 开发预览明确需要原生连接，不假授权 |

### 明确不计为本次新优化

- [备份列表64KiB读取](<../../src-tauri/src/storage.rs>)在开工前已由其他改动完成；不重复记功、不编造本次before/after。
- 全量同步原有keyset、多值批、SQLite增量typed spill/CAS/MAC/readback保留，没有删掉验证换速度。
- 未默认引入多worker读写、全局关闭约束、MySQL LOAD DATA、任意SQL并行/猜测合并或断线自动重放；没有证据就不为“全部实施”强开高风险路径。

## 4. 最终统一回归与证据

| 检查 | 最新结果 | 证据 / 口径 |
|---|---|---|
| 完整Rust lib | 1274通过 /0失败 /53 ignored | [完整日志](<evidence/performance-20261114/full-review-candidate-20261114.log#L1950>)；ignored不是通过 |
| 备份/还原/同步集成5个target | 20通过 /0失败 /1 ignored | [各target结果](<evidence/performance-20261114/integration-review-20261114.log#L1170-L1212>)；均为本地原生SQLite，专用实库用例未执行 |
| 完整前端 | 1336通过 /0失败 /0取消 /1 skipped | [完整汇总](<evidence/performance-20261114/full-ui-review-final-20261114.log#L1355-L1362>)；包括真实Chrome隔离IPC，不叫厂商实库验收 |
| TypeScript/Vite build | 通过 | [构建](<evidence/performance-20261114/build-review-final-20261114.log#L4-L24>)；未发布新桌面exe |
| 文档链接 | 通过 | [检查](<evidence/performance-20261114/doc-links-review-20261114.log>)，仍按当前源码核验 |

本任务active构成：3份当前文档（计划/实施状态/本审阅表）、1份派生CSV、21份按行号引用的证据日志（包含本轮追加的独立进程RSS与布局专项）；不移动或清理其他人的证据。已开发能力的结论并入[产品状态](<../completed/PRODUCT-STATE.md>)；完整计划剩余实现/现场闸门仍留active，不因局部通过标全部闭环。

## 5. 最终保留决策怎么给

本表 ID 用于逐项选择，不是提交清单。你可以回复例如：`保留 I-1/I-2/P-1/S-3/C-1/R-2；删除 D-1/D-2 对应新模式；B-3 暂不保留`。对共用模块/依赖我会先说明影响，再做精确修改，**不会对整个脏工作树执行 checkout/reset/stash**。

- 你看完前：不提交、不擅自删除已有用户改动。
- 负优化 D：默认已关闭；你可以选择保留为低内存特例，或删除新模式。
- 无基线或缺实库的 N/B/A/G：不自动推荐为“更快”，你可按新能力价值、限制和成本决定。
- 当前剩余最终验证/已知限制在实施状态逐项列出；直到完成约定范围与回归，不能把审阅表写成所有数据库现场验收完成。
