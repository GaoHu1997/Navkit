# 数据链路性能实施状态（2026-11-14，待用户保留决策）

关联：[完整计划](<navkit-data-pipeline-performance.md>)。**尚未全部完成，不能归档为全链路已优化。**

## 本轮落地与验收

| 项目 | 代码入口 | 当前证据/边界 |
|---|---|---|
| P1 标题与预览 | [DataGeneratorDialog](<../../src/dialogs/DataGeneratorDialog.ts>)、[有界 CSV 预览](<../../src-tauri/src/transfer_preview.rs>) | 修复数组字符串化；CSV 解码在记录分配前有总预算；过窗口的坏行不当成已经完整校验；空文件/编码/多行记录测试 |
| 遗留表导出 | [commands](<../../src-tauri/src/commands.rs>)、[transfer](<../../src-tauri/src/transfer.rs>) | 复用现有单游标与 stage/sink；回灌测试已修复实际目标标识符；不宣称所有旧/新协议都已完成实库性能对照 |
| SQLite 导入批写 | [批写门禁](<../../src-tauri/src/transfer_import_batch.rs>) | 同一事务；完整显式列、无 trigger/FK 的普通表才批写；≤256 行/约1MiB/单行硬限；冲突/default/不匹配走旧逐行策略，无错误重放；当前仍使用既有精确 literal 转换，不是通用 typed 参数批实现 |
| 大数据生成 | [recipe](<../../src-tauri/src/generated_recipe.rs>)、[pinned worker](<../../src-tauri/src/batch_execution.rs>)、[运行时授权](<../../src-tauri/src/generated_recipe_runtime.rs>)、[显式分段](<../../src-tauri/src/generated_recipe_segments.rs>) | UI 大总量只传规则，不传全部 rows；最多20行样本、版本/固定时间/全局行号/结构指纹；recipe独立single writer。atomic完整事务仍默认；SQLite显式segmented需要确认前缀不可撤销，每1000行/8MiB绑定预算先到提交，锁内逐段验结构；PG/MySQL segmented写前拒绝。已采样100k/1m进程RSS，千万行已独立真实验证279.487s/RSS观测32.38MiB，count/distinct/COMMIT确认，不与旧≤500同规模计算提速 |
| 低内存 diff | [diff](<../../src-tauri/src/sync_diff.rs>)、[自适应存储](<../../src-tauri/src/sync_diff_store.rs>)、[兼容策略](<../../src-tauri/src/sync_diff_compat.rs>) | 小数据有界内存，超阈值一次转磁盘并128行批写scratch；原排序/精确类型保护保留。但时间对照为负收益，**不默认开启**，API/界面有显式 bounded 选项。PG/MySQL bounded 模式限定2048行/16MiB，默认兼容路径保留原能力，不将限制推广到默认功能 |
| SQLite 快照备份 | [快照读取](<../../src-tauri/src/transfer_backup_snapshot.rs>)、[测试](<../../src-tauri/src/transfer_backup_snapshot_tests.rs>) | 文件库/main 的 DDL、目录和所选数据在同一只读事务；并发更新/DDL后的备份仍旧版本；默认非文件/其他通道保持兼容路径。没有把其他数据库逐表读取说成整库同快照；不是完整实例/物理备份或PITR |
| 详细流式还原 | [还原器](<../../src-tauri/src/transfer_restore_stream.rs>)、[固定源与MAC](<../../src-tauri/src/transfer_sql_reader.rs>) | 完整预扫→私有有预算spool→专用连接/raw SQL；原1000条/8MiB软DML提交边界，报告≤2048条及真实失败index/acked/committed/rollback/unknown；只允许已验证备份脚本政策，显式事务/普通SET等有兼容拒绝。公开restore_backup_detailed已接可选taskid及停止后续语句/后台继续，19项还原回归通过；旧restore_sql_file保持原接口 |
| SQL显式组包 | [assembler](<../../src-tauri/src/transfer_sql_batch.rs>)、[向导](<../../src/dialogs/ImportExportDialog.ts>) | 默认0/1仍逐行，显式选批且确认恢复语句语义；UTF8及目标编码硬限、跨页flush、single超限不发布；并发task-local不污染旧导出；多行不是触发器/default语义等价声明 |

| 原生COPY | [COPY模块](<../../src-tauri/src/transfer_pg_copy.rs>)、[导入向导](<../../src/dialogs/ImportExportDialog.ts>) | 仅pg_copy显式选项；原生PG/已有heap表/完整列映射/类型白名单，无trigger/FK/CHECK/index/RLS/partition/default依赖；codec控制NULL/空串/转义/精确数值/bytea，abort/finish/rollback/unknown关闭不重放；6项本地通过，实际PG用例未配置未执行，不宣称实库收益 |
| 原生任务级备份 | [native备份](<../../src-tauri/src/transfer_backup_native.rs>)、[门禁测试](<../../src-tauri/src/transfer_backup_native_tests.rs>) | 默认compat不变；pg_snapshot单连接只读RR/nonce/目录/OID锁且PG14+结构属性门禁；mysql_locked原生8.0/8.4显式源读锁、共享reader/nonce/关闭后发布。MySQL可能隐式锁关联对象/升级WRITE，已知trigger/FK依赖拒绝；不得称仅所选锁或免费提速。8项本地通过，4项实库未执行 |
| 传输生产者准入 | [配额](<../../src-tauri/src/transfer_admission.rs>)、[任务接入](<../../src-tauri/src/transfer.rs>) | import/export顶层最多4 producer；每205MiB/总820MiB是保守字节预留账本，不是allocator/driver/RSS硬限。超额立即拒绝，cancel/drop不释放，仅工作settle/finish；nested串行共享至最后成员结束，旧128日志规则保留；12项本地通过。recipe/sync/dump各有既有预算，不声称所有入口统一硬限 |

## 当前实际性能记录

仅当前 Windows 工作树、SQLite、debug test profile、隔离 UUID 文件库。初始化/预热与表重建按对应测试明确记录，结果不外推到 MySQL/PG 或用户环境。下面不是所有链路完成证明。

- 导入 1000 行：逐行 0.059940 s，批写 0.023039 s，中位数比 2.602（预热后5轮、A/B交错）。
- 导入 100000 行：逐行 10.504273 s，批写 2.455240 s，中位数比 4.278（预热后5轮、A/B交错）。
- 第一版 disk diff：50行旧8.6511ms/新53.5663ms；100000行旧1.8352323s/新42.858719s，确认负收益。
- 优化后 bounded diff：50行旧9.4655ms/新19.5142ms；100000行旧1.754858s/新24.2278865s。虽改善但仍负收益，默认继续兼容内存策略。两次不同运行不直接推导稳定百分比收益。
- 最新pinned原子recipe：100000行2681ms/RSS观测峰32.30MiB、1000000行28306ms/32.55MiB，最终count/distinct/COMMIT通过。旧500行模式无同规模可执行基线；更早2743ms/74163ms是其他时点，不能制造前后提速。RSS按20ms采样整个测试进程，不是每任务/driver/数据库侧硬限。
- CSV有效文件预览：23,977,785字节/200000行，只返回20行，旧完整读解析667.592ms→有界1.602ms；观测进程中位峰94.14→20.15MiB。快路径短于采样间隔，可能漏瞬时峰；窗口外坏行不再当作已验证。
- SQL组包全周期（10000行、同一原子恢复事务）：2.790693s→0.970726s，2.875×；单独导出577.878→570.695ms接近噪声，不能称导出提速。多行恢复语句语义需要显式确认。
- 用户追加核对已测真实默认/低内存公共入口，各3次独立进程：50行4.977/11.462ms、峰25.83/26.50MiB；10万行1.918499/10.340559s、峰104.12/36.15MiB（省65.3%，慢5.39×），比较前基线26.18/26.19MiB。峰增量77.94/9.98MiB是另一口径，不说总峰省87.2%。[汇总及来源hash](<evidence/performance-20261114/diff-memory-three-rounds-20261114.log#L1-L28>)；20ms采样/小任务可漏峰，不外推宽表或其他协议。
- 低内存选项已与比较按钮成组；修复auto margin漂移和窄窗长结果遮挡，默认/显式参数语义不变。[真实UI测试](<../../tools/tests/sync-diff-option.test.mjs>)含三种宽度、模式隐藏与只读diff，不执行同步。
- 用户要求的逐项before/after/提升或下降/原因表见[完整审阅表](<navkit-performance-comparison.md>)与[CSV数据](<navkit-performance-comparison-data.csv>)。修改保持未提交，待用户选择是否保留。

证据入口（只增不改，逐行可核对）：
- [1000行导入](<evidence/performance-20261113/import-ab-1000.log#L590-L605>)。
- [100000行导入](<evidence/performance-20261113/import-ab-100000-final.log#L615-L631>)。
- [初版负收益 diff](<evidence/performance-20261113/sync-diff-ab.log#L602-L604>)。
- [优化后仍负收益 diff](<evidence/performance-20261113/sync-diff-ab-final.log#L615-L620>)。
- [recipe 实际规模](<evidence/performance-20261113/recipe-scale-final.log#L615-L620>)。

## 已运行回归（后续每项改动仍需复验）

- P1 transfer_streaming：16 passed。
- import_batch：4 passed，1 ignored性能基准另跑。
- 最新recipe（原子+分段）：[18 passed](<evidence/performance-20261114/recipe-segments-20261114.log#L622-L643>)；规模基准单独运行。新增未接齐前端活计时测试曾失败后撤回，不计为通过，原有失败断言未删除/弱化。
- 最新公开还原回归：19 passed；DELIMITER fixture 需独立行，不放宽解析器。
- SQL batch：6 passed，包含真实 SQLite 回灌与语句边界可观察差异。
- SQLite snapshot backup：3 passed。
- 最新 diff 策略及 store 迁移：13 passed（有界模式与默认兼容路径均对照），4 ignored 另列基准/实库测试；最终默认开关策略不启用已证实负收益的 bounded 模式。
- 最新完整前端：[1336 passed /0 failed /0 cancelled /1 skipped](<evidence/performance-20261114/full-ui-review-final-20261114.log#L1355-L1362>)；[TypeScript+Vite构建通过](<evidence/performance-20261114/build-review-final-20261114.log#L4-L24>)。不是桌面原生现场验收；初轮6失败和本轮新增夹具1失败未抹掉。
- 最新Rust完整候选：[1274 passed /0 failed /53 ignored](<evidence/performance-20261114/full-review-candidate-20261114.log#L1950>)；[准入12 passed](<evidence/performance-20261114/admission-connected-20261114.log#L622-L634>)；[native备份8 passed/4实库ignored](<evidence/performance-20261114/native-review-final-20261114.log#L620-L634>)。
- [CSV有效样本A/B](<evidence/performance-20261114/preview-ab-20261114.log#L621-L636>)、[SQL完整周期](<evidence/performance-20261114/sql-cycle-round1.log#L621-L636>)、[百万RSS](<evidence/performance-20261114/recipe-rss-20261114.log#L621-L628>)、[千万独立进程](<evidence/performance-20261114/recipe-10m-20261114.log#L621-L625>)均真实通过。保留之前失败日志在本任务临时目录，不用后续限定通过覆盖之前全库失败口径。
- 文档/构建最终闸门仍复跑；不运行整文件格式化、checkout/reset或覆盖其他用户工作树改动。

## 剩余必须完成/不得虚报项

1. 本轮统一回归已完成：Rust1274/0/53ignored，完整前端1336/0/1skipped，5target集成20/0/1ignored及构建/文档闸门通过；初轮6失败、新增夹具1失败和链接产物占用错误记录仍保留，不称历次全绿。下一次按用户选择保留/删除后须再次回归。
2. 用户已选择先代码与本地/模拟验证，不配置PG/MySQL实库。COPY与原生任务级备份已接，但专用实库的类型/断线/取消/目录变化/源锁影响仍未验收；默认保持兼容，不凭“已实现”宣称已提速。MySQL/PG通用typed参数批、LOAD DATA和多worker没有安全收益证据，未强开。
3. segmented仅SQLite；PG/MySQL逐段会话/错误语义未证明而明确写前拒绝。准入仅覆盖顶层import/export producer，不是所有入口/交互连接/driver预读的统一硬限。
4. 已有进程RSS观测和SQL导出+恢复全周期A/B；driver/WebView/服务端RSS、真实网络RTT、交互延迟分位数、多任务1/2/4混合负载与WAL/锁成本尚无现场曲线，不能把字节预留作为实测。
5. bounded diff仍明确负收益，默认兼容模式保留；未来若要替代须通过同语义性能闸门。当前不为了阶段标签去吞掉延迟回退。
6. 整理逐项审阅表、最终可复现证据与结果文档归档；在用户看表作决定前不提交、不擅自回退其既有脏树。

## 可复现命令

```powershell
cargo test --offline --manifest-path src-tauri/Cargo.toml --lib transfer_import_batch -- --test-threads=1
$env:NAVKIT_IMPORT_BENCH_ROWS='100000'
cargo test --offline --manifest-path src-tauri/Cargo.toml --lib benchmark_actual_csv_import_row_vs_batch -- --ignored --nocapture --test-threads=1
cargo test --offline --manifest-path src-tauri/Cargo.toml --lib recipe_ -- --test-threads=1
cargo test --offline --manifest-path src-tauri/Cargo.toml --lib recipe_benchmark_ -- --ignored --nocapture --test-threads=1
cargo test --offline --manifest-path src-tauri/Cargo.toml --lib sync_diff -- --test-threads=1
cargo test --offline --manifest-path src-tauri/Cargo.toml --lib sqlite_actual_diff -- --ignored --nocapture --test-threads=1
cargo test --offline --manifest-path src-tauri/Cargo.toml --lib restore_ -- --test-threads=1
cargo test --offline --manifest-path src-tauri/Cargo.toml --lib sql_batch_ -- --test-threads=1
cargo test --offline --manifest-path src-tauri/Cargo.toml --lib backup_snapshot -- --test-threads=1
node --test tools/tests/data-generator-recipe.test.mjs tools/tests/batch-execution-service.test.mjs tools/tests/sql-export-batch.test.mjs tools/tests/import-wizard.test.mjs
npm run build
python tools/check-doc-links.py
```

验收标准仍是完整计划的正确性、事务与资源闸门。当前文档明确进行中；开发与测试完成的能力以后逐项并入结果文档，未完成任务不能因已有几组通过测试而全部归档。
