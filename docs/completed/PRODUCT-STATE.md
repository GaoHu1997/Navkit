# NavKit 产品状态（与当前代码一致）

> **本文档是 `docs/completed/` 的唯一结果文档。** 它描述当前工作树代码的实际能力、验收状态与真实边界，
> 不是过程记录。任何一条能力的代码入口都必须可打开核对。
> 更新规则见 `.agents/skills/docs-lifecycle/SKILL.md`。
>
> 最后核对：2026-11-15｜代码基线：工作树（`1d01834` + 未提交修改；保留既有 IPC/例程/分页/数据源修复，新增受控数据管线代码、本地正确性/规模验证与显式策略；性能改动仍待用户逐项决定保留，不代表已发布桌面程序）

> 最新修复验收基线：2026-11-14 Windows 程序 `5670dd85…6b930a84`，公开大总量预览完成判定已修复并经真实 IPC 验证；旧 F44373CB 失败结论保留为历史，后续未验收改动不覆盖本结论。

## 一、产品定位

NavKit 是 **Tauri 2 + Rust + 原生 TypeScript** 的桌面数据库客户端，产出免安装绿色版单文件 exe。
配置、密钥、日志、备份全部写入 exe 同目录 `./data/`，整个文件夹拷走即用。

## 二、验收状态图例

| 标记 | 含义 |
|---|---|
| ✅ | 有真实服务器 / 真实桌面的通过证据 |
| ⏳ | 代码完成，缺真机或专用实库的现场验收 |
| 🚧 | 仍有已确认的功能缺口，需要继续写代码 |

## 三、当前验证状态

### 2026-11-14 公开 recipe 预览完成判定修复（最新）

**上一轮确定的发布阻断已消除，新修复快照的本轮验证通过。** [完成判定](<../../src-tauri/src/generated_recipe_runtime.rs#L36-L48>)复用正式批任务 finish/outcome，不再把停止确认布尔值当成功标志；只有成功、资源已排空、事务为 none 且没有发送生成写入才返回样本。原失败诊断、取消/超时、未确认停止、未知清理和在途资源均严格拒绝，不重放。

- [修复程序](<../../artifacts/release-preview-fixed-20261114/NavKit.exe>)：版本仍为 `1.0.0-pre`，SHA-256 `5670dd855aed67a98f38c9b524860cabf7fa197a8adc96ddafe6ce3e6b930a84`。
- [修复版 NSIS](<../../artifacts/release-preview-fixed-20261114/NavKit-1.0.0-pre-windows-x64-setup.exe>)：SHA-256 `70a5cbdc5c145d97936be7dfaa5dc9778346fe875a2c45fdf2dc87e69a2733f0`，构建通过，未执行安装/卸载。
- [新增生命周期回归](<../../src-tauri/src/generated_recipe_tests.rs>) 4/4；[完整库](<../../.tmp/recipe-preview-fixed-rust-complete-20261114.log#L1388>) 1328 通过、0 失败、56 ignored；[相关前端](<../../.tmp/recipe-preview-fixed-related-ui-20261114.log#L68-L73>) 65/65 通过。前一轮完整 UI 1399/1 skipped 是历史快照，本轮不冒称重新运行完整 UI。
- [真实公开 IPC](<../../artifacts/release-native-acfd4963-fb0d-41ca-b21d-f41adadd5ba1/report.json>) 8/8 通过，无 preview mock、pageErrors=[]；合法 1000 行计划返回 20 行样本、目标仍是原 2 行；DESC 主键遗漏仍拒绝且零写入。
- [固定哈希真实桌面工作流](<../../tools/test-artifacts/database-workflows-native-4ced4b54-cb26-4a65-b6ae-8ff6a01ff611/report.json>) 12/12 组通过；[查询缓存报告](<../../artifacts/query-spool-1794655914351/report.json>) 6/6 组通过，12005 行、标准末页 5 行、CSV/JSON/SQL 完整导出不重放，失败清除旧快照且后续执行独立。
- TypeScript/Vite、[Windows release](<../../.tmp/recipe-preview-fixed-release-build-20261114.log#L1173-L1174>)与[NSIS 构建](<../../.tmp/recipe-preview-fixed-nsis-20261114.log>)通过。旧 F44373CB 程序未覆盖；[旧快照失败报告](<../../artifacts/release-candidate-20261114/acceptance-f44373cb.md>)保留为历史，不改写成通过。

仅证明本修复版 Windows x64 / 隔离 SQLite 及所列回归。ignored、其它厂商实库、macOS/Linux/ARM64、安装/卸载、签名及跨版本更新演练仍不计通过。未发布、未推送、未打标签，未替换用户正在运行的程序；正式新版本号仍须另行确认并统一改号重建。

### 2026-11-14 早期 F44373CB 封存快照验收（历史）

**结论：暂不建议发布该快照。** 用户已选择只验收封存程序，后续其他会话的工作树改动不计入本轮通过。[封存程序](<../../artifacts/release-candidate-20261114/NavKit-1.0.0-pre-windows-x64-portable-final.exe>)版本仍为 `1.0.0-pre`，SHA-256 `f44373cba36b1168de65fa07b60c1bf37e1025a6abf56cf98714278357eee20f`；未发布、未推送、未打标签，未替换用户现有程序。

| 验证层级 | 最终结果 | 代码 / 证据入口与边界 |
|---|---|---|
| 完整 UI、历史浏览器补验 | 1399 通过、0 失败、1 默认跳过；该浏览器专项显式启用后 17/17 | [测试入口](<../../package.json>)、[UI 结果](<../../.tmp/release-acceptance-ui-complete-20261114.log#L1420-L1427>)、[浏览器结果](<../../.tmp/release-acceptance-history-browser-complete-20261114.log#L17-L24>)；不把默认跳过算通过 |
| 完整 Rust 库与五个集成目标 | 库 1324 通过、0 失败、56 ignored；集成 20 通过、0 失败、1 ignored | [库结果](<../../.tmp/release-acceptance-rust-scope-final-20261114.log#L1384>)、[集成](<../../.tmp/release-acceptance-integration-final-20261114.log#L1183-L1233>)；内部测试通过不能替代公开 IPC |
| 封存程序真实数据库工作流 | 12/12 组通过 | [固定哈希原生报告](<../../tools/test-artifacts/database-workflows-native-b231973e-2a11-4b7b-a4d7-84cfcd16050c/report.json>)；真实 Tauri / Windows x64 / 隔离 SQLite，无 preview mock，含授权、取消回滚、结构、索引、触发器、编辑与导出 |
| 查询缓存与全量导出 | 6/6 组通过；12005 行、每页 200、标准末页 5 行，三格式导出不重放 | [真实缓存报告](<../../artifacts/query-spool-1794646951975/report.json>)、[摘要](<../../.tmp/release-acceptance-native-spool-verified-20261114.log#L1-L7>)；错误重跑清除旧快照，后续执行独立 |
| 发布工程与构建 | 版本 / 双锁 / 标签门禁 10/10，隔离 npm ci、TypeScript/Vite、Windows release、NSIS 构建通过 | [发布门禁](<../../tools/check-release.mjs>)、[CI](<../../.github/workflows/ci.yml>)、[发布流程](<../../.github/workflows/release.yml>)、[NSIS 结果](<../../.tmp/release-acceptance-nsis-signed-cache-20261114.log#L1-L6>)；签名微软引导程序只复用于工具缓存，未安装运行时；安装 / 卸载与全平台未验证 |
| 公开大总量 recipe 预览 | **旧快照失败；最新修复版已通过** | [真实失败报告](<../../artifacts/release-native-6c2cf968-4798-424c-9e8f-49a87b1b9657/report.json>)；合法 1000 行预览返回 `EXECUTION_EFFECTS_UNKNOWN` /「样本物理会话排空/关闭未确认」 |

已修复前端文件/目标映射迟到、元数据/预览代次、自由输入 Enter、整库源清单身份与备份目录就绪；后端修复内存还原误目标、生成列可插入投影、显式选表缺失、DESC 主键身份、内部表字面前缀、独立索引/触发器备份顺序。入口：[导入](<../../src/dialogs/ImportExportDialog.ts>)、[生成器](<../../src/dialogs/DataGeneratorDialog.ts>)、[输入增强](<../../src/ui/SearchableInput.ts>)、[同步](<../../src/dialogs/SyncDialog.ts>)、[备份](<../../src/dialogs/BackupDialog.ts>)、[快照备份](<../../src-tauri/src/transfer_backup_snapshot.rs>)、[详细还原](<../../src-tauri/src/transfer_restore_stream.rs>)、[主键识别](<../../src-tauri/src/generated_recipe_segments.rs>)与[旧生成入口](<../../src-tauri/src/data_generator.rs>)。

旧快照已修正摘要前缀，但当时仍误用停止确认标志而拒绝正常预览。19:03 后续请求中已修复完成判定，详见上面的最新验证；[历史失败证据](<../../artifacts/release-candidate-20261114/acceptance-f44373cb.md>)保留，不能继续作为当前发布阻断。

### 2026-11-13 独立分支：查询首屏流式后端

**2026-11-14 已合并 main**：main 快进到 `d81541e`，原有暂存/未暂存/未跟踪内容保留；重叠处同时保留原缓存 scratch 优化、性能基准与新增流式测试。实际合并工作树重新构建后，`cargo test --manifest-path src-tauri/Cargo.toml --locked --lib execution -- --test-threads=1 --nocapture`：129 通过、0 失败、6 ignored；`query_spool::tests::`：6 通过、0 失败。以下独立分支测试数据为合并前快照，不能替代本次结果；本次未重跑全库、未接入流式前端、未推送远端。

| 能力 | 验收 | 代码入口与边界 |
|---|---|---|
| 首屏预览与后台全量续读 | ⏳ 后端已实现，缺真实桌面与专用 MySQL 性能验收 | [流式观察契约](<../../src-tauri/src/execution_stream.rs>)、[命令入口](<../../src-tauri/src/commands.rs>)；只新增 opt-in API，旧 execute_sql_v2 和 ExecutionRequest 不变，2026-11-14 已快进合并 main，既有未提交修改保留，不接前端 |
| 取消、异常与最终回执 | ⏳ 原生生命周期回归通过 | [执行引擎](<../../src-tauri/src/execution.rs>)；同一拥有者任务继续完整读取、监督取消与清理，再由 Entry::finish 发布终态；临时 preview 绝不等价于成功，失败撤销缓存，Unknown 的迟到清理不能变成成功 |
| 完整缓存分页/导出 | ⏳ 本地 SQLite 全链路验证，缺用户实库验收 | [成功终态门](<../../src-tauri/src/query_spool.rs>)、[SQLite lineage 观察](<../../src-tauri/src/query_execution.rs>)；首屏之外仍写原执行全量缓存，成功清理后才开放缓存读/导出，不重跑 SQL |
| MySQL 原生驱动流式验证 | ⏳ 回环协议模拟通过，非厂商实库结论 | [MySQL 8 协议模拟回归](<../../src-tauri/src/execution_stream_mysql_tests.rs>)；真实 SQLx 驱动、200 行后阻塞尾部，验证提前首屏、10050 行最后页、迟到错误撤销与定向 KILL QUERY；TLS/非空密码/多结果/用户具体 SQL 未实测 |

**独立分支验证（合并前）**：`cargo test --manifest-path src-tauri/Cargo.toml --locked --lib execution -- --test-threads=1 --nocapture`：112 通过、0 失败、3 ignored；`cargo test --manifest-path src-tauri/Cargo.toml --locked --lib query_spool::tests:: -- --test-threads=1`：6 通过、0 失败。测试入口：[观察通道生命周期](<../../src-tauri/src/execution_stream_tests.rs>)、[实际命令服务与全量导出](<../../src-tauri/src/execution_stream_service_tests.rs>)、[旧 C1 回归](<../../src-tauri/src/execution_tests.rs>)。

**性能证据边界（合并前独立分支）**：本地 debug 构建、SQLite 50000 行的最近一次服务验证：默认首屏 200 行在 45.58 ms 返回，完整读取与清理在 1145.40 ms 完成；删除原表数据后仍从本次缓存导出 50000 行。它验证的是首屏/全量阶段解耦与不重放，不是对用户远程 MySQL SQL 的 DBX/DBeaver/NavKit 同环境速度对比，不能据此宣称界面已达到 1 秒。

**独立分支完整库回归不是全绿（合并前）**：最后一次完整库运行（末次快照资源标志修补前）1042 通过、2 失败、38 ignored；失败为 [账户权限断言](<../../src-tauri/src/db_users.rs#L789-L793>) 和 [SQL Server 模拟器认证连接重置](<../../src-tauri/src/transfer_export_protocol_tests.rs#L90-L94>)，两项单跑均复现，相关模块本轮未改。末次快照修补后，以上执行相关 112 项及缓存 6 项已重建复跑，不能把专项通过写成全库通过。完整文档链接检查仍有基线遗留的 4 个历史产物链接缺失；本次新增代码入口均存在。

**接入契约**：先调用 issue_execution_grant，再以原请求调用 execute_sql_streaming，参数为 `{ request, options: { page_rows: 200 } }`；返回 `{ status, preview, outcome }`。首行慢时最多等待 1 秒便返回当前状态（这不是 SQL 超时），后续按原 execution/connection/owner/namespace 调 execution_stream_snapshot 轮询。preview.provisional=true、observed_rows 为已观察行数下界；停止仍用 cancel_execution。只有 outcome 是终态，result 只带元数据/总行数/spool_id，不留永久全量 rows 副本；最终成功后从 spool_id 调 fetch_query_rows 与 export_query_spooled。

**现阶段限制**：默认首屏 200 行，可选 1..1000，8 MiB 保守上限；只提前展示 result_index=0，第一结果无行时暂时没有预览。后台期间暂不开放缓存翻页/导出和编辑。完整导出仍受既有 512 MiB 单结果 / 2 GiB 总缓存配额约束，失败明确拒绝完整导出，不静默漏行。JDBC/ODBC/REST/HTTP 隧道不宣称支持该流式 API；PostgreSQL 复用观察点但未做专用 PG 流式实库验收。前端接入尚未实施：[未开工范围与验收条件](<../planned/query-streaming-frontend.md>)。



**2026-11-13 全驱动实测问题修复（P1–P10）**：

- 真实环境测试报告见 [navkit-all-drivers-test-2026-11-13.md](navkit-all-drivers-test-2026-11-13.md)。本轮修复结论、逐项状态与证据见 §五 `DRIVER-FIX-01`。
- ✅ 代码 + 测试：备份列表不再为前 12 行备注 `read_to_string()` 整个 3GB 文件（[有界前缀读取](<../../src-tauri/src/storage.rs>)，3 GiB 稀疏文件回归）；SQL 文件任务新增独立总超时（默认 0 = 不限，[对话框](<../../src/ui/BatchExecutionPanel.ts>)）；备份向导新增显式「跳过无法无损读取的表」并逐表告警（[备份策略](<../../src-tauri/src/transfer.rs>)、[向导](<../../src/dialogs/BackupDialog.ts>)）；数据字典导出统一目录 + 文档名路径（[对话框](<../../src/dialogs/DataDictionaryDialog.ts>)）。
- ✅ 真实 MySQL 8.0.46：POINT / LINESTRING / POLYGON / MULTIPOINT / MULTILINESTRING / MULTIPOLYGON / GEOMETRYCOLLECTION 共 7 种几何按新解码转换后写回 `ST_GeomFromWKB`，WKT 与 SRID 全部一致。MySQL 内部 GEOMETRY 不是标准 WKB（坐标 `(y,x)`、复合类型带计数），直接交给 `ST_GeomFromWKB` 会产生错位几何——这一方案在实测中被否决并已修正（[解码](<../../src-tauri/src/values.rs>)）。
- ⏳ JDBC 多语句在**同一物理会话**内按源顺序逐条执行，保留 PL/SQL 块内分号、遇错停止且不重放（[JDBC 通道](<../../src-tauri/src/jdbc_db.rs>)）；真实达梦服务器本轮未复测。
- 🚧 金仓保存、HighGo 查询、达梦首屏等静默退出仍无运行时崩溃证据。已把 JDBC/H2 sidecar 的 `stderr(Stdio::inherit())` 改为有界环形缓冲并记录非零退出码与 stderr 尾部（[进程诊断](<../../src-tauri/src/jdbc_db.rs>)、[H2](<../../src-tauri/src/h2_db.rs>)），不宣称这些崩溃已修复。
- 验证：`cargo check --lib --offline` 通过；定向 Rust 14 通过 / 1 ignored；全量 `cargo test --lib` **1177 通过、4 失败、38 ignored**，4 项失败在 `db_users`/`structure_*` 未改动路径，不计入本轮通过；`npm run build` 通过，前端定向 **80/80**。

**2026-11-13 新增数据源表级导出**：

- [统一导出入口](<../../src-tauri/src/transfer_export_cursor.rs>)已分派 ClickHouse、通用 ODBC、MongoDB 及 HTTP/Native 适配器，不回退全表物化；[输出发布](<../../src-tauri/src/transfer_output.rs>)使用暂存文件，成功后原子发布，错误/取消不覆盖已有目标。[HTTP/Native 回归](<../../src-tauri/src/transfer_http_export_tests.rs>)、[ClickHouse 回归](<../../src-tauri/src/transfer_clickhouse_export_tests.rs>)及[ODBC 类型/值回归](<../../src-tauri/src/odbc_db.rs#L420-L477>)、[ODBC 方言回归](<../../src-tauri/src/transfer_export_cursor.rs#L449-L467>)覆盖协议和完整性边界；详细格式、对象和预算限制见 §六第 12–13 条。
- 最新重新编译的 Rust 定向回归全部通过：`cargo test --offline --manifest-path src-tauri/Cargo.toml --lib --no-run --message-format=json` 后，用同一新产物串行执行模块筛选（[可复现脚本](<../../.tmp/run-export-final-v4-20261113.sh#L5-L25>)）。`export` **113/113**（[结果](<../../.tmp/export-final-v4-rust-export-20261113.log#L129>)）、HTTP **38/38**、HBase/Salesforce **17/17**、Native HTTP **9/9**、RocketMQ **5/5**、MQ **2/2**、CQL **2/2**、Native REST **3/3**、流式传输 **11/11**、文件发布 **8/8**、spool **8/8**、ODBC 类型/值 **6/6**、ClickHouse 行解析 **11/11**及定义解析 **2/2**（[HTTP 结果](<../../.tmp/export-final-v4-rust-http_db-tests-20261113.log#L42>)、[Native 结果](<../../.tmp/export-final-v4-rust-native_http-native_api_export_tests-20261113.log#L23>)）。模块筛选存在重叠，不相加为独立测试总数，也不是全库 Rust 测试通过。`cargo check --manifest-path src-tauri/Cargo.toml --lib --offline` [通过](<../../.tmp/export-final-v4-rust-check-20261113.log#L1182-L1183>)，仍有 151 条编译警告。
- [Windows Native 测试夹具](<../../src-tauri/src/native_api_export_tests.rs#L25-L74>)修复非阻塞监听器接受连接后的 `WouldBlock` 和析构时二次 panic；新增分段 HTTP 请求和保留原 panic 的回归。日志中 `fixture assertion failure` / `request assertion failure` 是 `catch_unwind` 用例故意触发且正确捕获的输出，并非测试失败。Weaviate 回归按列名检查完整数据，不依赖 JSON 属性顺序；未知/缺失结果结构仍要求报错（[回归](<../../src-tauri/src/http_db_tests.rs>)）。
- 最新扩展前端定向回归 **111 项：110 通过、1 失败**：`node --test --test-concurrency=1 tools/tests/query-result-host.test.mjs tools/tests/sql-completion-execution.test.mjs tools/tests/database-workflows.test.mjs tools/tests/approved-ui-styles.test.mjs tools/tests/ipc-errors.test.mjs`（[汇总与失败](<../../.tmp/export-final-v6-scoped-ui-20261113.log#L113-L141>)）。查询结果 host、执行、spool 全量导出、分页与忙状态冻结用例通过（[导出回归](<../../.tmp/export-final-v6-scoped-ui-20261113.log#L96-L105>)）；该次唯一失败为工具栏别名断言 `primary-key`，不删除或弱化断言。查询结果 [host 夹具](<../../tools/tests/query-result-host.test.mjs#L107-L112>)补齐绕过 App 构造器时缺失的 `ribbon.updateDisabled`，四个离页用例保留原断言并验证工具栏更新和忙状态复位；生产离页守卫未因此改动。前一轮相关 **37/37**（[历史结果](<../../.tmp/export-final-v4-scoped-ui-20261113.log#L38-L45>)）为不同工作树时点，不替代最新结果。`npm run build`（TypeScript + Vite）[通过](<../../.tmp/export-final-v6-build-20261113.log#L3-L24>)，仍有 dialog 静态/动态混用及大 chunk 警告；以上不等同于厂商实库或真实桌面验收。
- 最新完整 UI 回归 **1291 项：1287 通过、2 失败、1 超时取消、1 跳过**：`npm run test:ui`（[汇总](<../../.tmp/export-final-v6-ui-20261113.log#L1309-L1316>)、[未通过详情](<../../.tmp/export-final-v6-ui-20261113.log#L1320-L1344>)）。查询结果 host、离页守卫与 SQL 执行/缓存用例通过，执行时钟及 ribbon 夹具错误不再出现；spool 全量导出、分页/导出冻结与关闭资源回收均通过（[host 结果](<../../.tmp/export-final-v6-ui-20261113.log#L712-L724>)、[导出/分页结果](<../../.tmp/export-final-v6-ui-20261113.log#L1107-L1117>)）。2 项失败分别是数据生成器连接失败提示的断言值为 `undefined`、连接编辑保存回归读取 `config` 为 `undefined`；ER 图真实 Chrome 用例因 `test timed out after 45000ms` 计为取消而非通过。这些未通过项保留，没有删除或弱化断言，本次不宣称全量全绿。工具栏图标别名断言在此轮完整回归通过（[结果](<../../.tmp/export-final-v6-ui-20261113.log#L19-L22>)），前面的扩展定向失败仍作为该次快照保留。前一次 **1290 项：1258 通过、31 失败、1 跳过**（[历史汇总](<../../.tmp/export-final-v4-ui-20261113.log#L1308-L1315>)）及下节自动分页结果为不同工作树时点的历史快照，不替代此轮结果。
- 新来源能力标为 **⏳**：代码与对应测试已落地；尚缺厂商 live 数据库及真实桌面验收，ODBC 目录覆盖不等于逐厂商认证。向导仍统一显示格式与 WHERE 选项，不支持的组合由后端明确拒绝，未声称 UI 自动隐藏能力。

**2026-11-13 大表首屏去计数与自动分页统计**：

- [数据页](<../../src/core/App.ts>)打开和翻页显式发送 `include_total=false`；[后端分页](<../../src-tauri/src/schema.rs>)及[分页预算/结果契约](<../../src-tauri/src/schema_page.rs>)首屏数据查询预算为当前页大小 + 1 行，以 `has_more` 判断下一页，不把已见下界冒充精确总数。普通 SQL/Mongo 分页不先做 `COUNT(*)` / 文档计数；空表仍保留列/主键，越界空页不错误宣称偏移量就是精确总数，兼容未升级调用方默认统计的旧协议。
- 表结构推导仅使用有界样本；[元数据命令](<../../src-tauri/src/commands.rs>)支持 `include_row_count=false`。数据页首次结果显示后再读取信息侧栏，侧栏走 metadata-only，不再触发通用驱动或 Mongo 的隐藏计数。设计器未传新选项时保留原契约。
- [数据页](<../../src/core/App.ts#L7775-L7871>)立即呈现加载状态和 `aria-live` 提示；当前页绘制后等待两次 `requestAnimationFrame`，自动发送独立 `include_total=true, offset=0, limit=0` 统计。成功只更新精确总行数、总页数与导航，不重置网格、焦点或编辑草稿。未知总数时下一页依据多取的一行，末页需精确总数；失败内联显示并可手动重试，不做统计轮询或无限自动重试。HTTP 来源未提供精确计数时明确不支持，不冒充成功。
- 总数快照以连接/模式/表/已应用筛选为键；页码、页大小、排序复用快照，刷新、筛选与实际插入/删除/保存使其失效，全部或部分保存结束后重新统计。版本与筛选键拒绝过期响应；新短页证据优先，当前页已见下界超过旧快照也会失效。不用草稿数加减伪造过滤总数；事务/忙连接/未知写入阻断统计，关闭视图取消排队帧并拒绝迟到回执。外部写入不会主动轮询，需刷新或由新页证据触发失效。
- 最新相关前端回归 **159/159 通过**：`node --test --test-concurrency=1 tools/tests/data-grid-filter-sort.test.mjs tools/tests/mutation.test.mjs tools/tests/manual-transaction.test.mjs tools/tests/manual-transaction-controller.test.mjs tools/tests/cell-preview-isolation.test.mjs tools/tests/ipc-errors.test.mjs tools/tests/namespace-semantics.test.mjs`；[定向结果](<../../.tmp/auto-total-related-final.log#L160-L167>)覆盖双帧延迟、草稿保护、快照复用、刷新/真实写入失效、部分保存、事务/未知写入、迟到防护及关闭取消。`npm run build`（TypeScript + Vite）[通过](<../../.tmp/auto-total-build-final.log>)。`npm run test:ui` 最新结果 **1276 项：1268 通过、7 失败、1 跳过**（[汇总](<../../.tmp/auto-total-ui-final.log#L1294-L1301>)）；失败为字典/生成器提示断言（2 项）、离页守卫 ribbon mock 缺 `updateDisabled`（4 项）及连接编辑保存回调未触发（1 项），分页统计用例通过，未删除或弱化无关断言。前一轮首屏回归 **92/92** 和完整 UI **1221 项：1214 通过、6 失败、1 跳过**保留为历史结果，不当作最新通过证据（[历史定向](<../../.tmp/table-page-related-final.log#L93-L101>)、[历史全量](<../../.tmp/table-page-ui-final.log#L1237-L1244>)）。
- 首屏去计数阶段的后端快照（历史证据，非自动统计后的重新编译结果）：[`page` 定向 16/16](<../../src-tauri/target/page-complete-tests.log#L597-L617>)、[原生 REST 3/3](<../../src-tauri/target/page-complete-rest-tests.log#L595-L600>)、[真实 SQLite 集成 5/5](<../../src-tauri/target/page-complete-e2e-tests.log#L1127-L1134>)通过。真实 SQL history 验证首屏无 `COUNT`、`LIMIT 201 OFFSET 0` 返回 201 行后裁剪为 200；覆盖空表/NULL 主键/元数据、整页与尾页、越界、显式 count-only、筛选、main/temp/attached 命名空间及通用 DDL 不计数。ClickHouse 为真实产品驱动连接 wire 模拟服务，不是厂商 live。最新 `cargo check --manifest-path src-tauri/Cargo.toml --lib --offline` [**通过**](<../../.tmp/auto-total-cargo-latest.log#L1107-L1108>)，有 145 条编译警告。前两次失败保留为历史：[E0061](<../../.tmp/auto-total-cargo-check.log#L146-L164>) 为 ClickHouse `spawn_row_stream` 缺 `types`/`max_record_bytes` 参数，[E0433](<../../.tmp/auto-total-cargo-recheck-final.log#L146-L152>) 为 [流式导出游标](<../../src-tauri/src/transfer_export_cursor.rs>)未声明 `http_export` 模块；两项均由并行导出改动补齐，最终检查不再出现。本次统计功能未扩大到独立导出修复。
- [百万 SQLite 历史实测](<../../src-tauri/target/page-complete-tests.log#L612-L615>)：debug 内存库实际验证 1,000,000 行，每页 200 行，两路径预热后各 7 轮交替，含元数据与 SQL history；未过滤中位数有计数 **27.26 ms** / 无计数 **24.37 ms**（**1.12×**），非索引 `name != absent` 筛选 **91.90 ms / 40.81 ms**（**2.25×**）。这是本机该轮测量，不是数据库服务器或端到端桌面首屏耗时承诺。
- 性能边界：移除了首屏前软件强制全表计数，后台精确 COUNT 本身仍可能昂贵，不代表任意 SQL 都是固定耗时。无索引排序、低选择性筛选、深页 `OFFSET`、网络/驱动延迟仍可能触发数据库侧昂贵工作；不支持原生偏移的 TOP/Hive 方言仍需读取前缀再跳过，不能据此宣称深页读取也有固定行预算；不宣称任意亿级表或所有厂商实库毫秒级首屏。代码与相关回归已完成，本次未发布新的桌面可执行程序，也未在用户原百万/亿级数据库复测。

**2026-11-12 IPC 错误与例程备份修复**：

- 例程转储不再通过 SHOW CREATE PROCEDURE 探测函数：先从真实 MySQL 目录取得类型，处理文本/二进制元数据、同名函数与过程、重复选择；选中例程不存在或不可读取时明确失败，不静默遗漏。[例程读取](<../../src-tauri/src/mysql_routines.rs>)、[备份与还原](<../../src-tauri/src/transfer.rs>)。
- MySQL 例程备份使用有界、无冲突标点 DELIMITER；还原分句不会把客户端指令发送给数据库。例程 DDL 使用无参数 COM_QUERY，避免 prepared protocol 不支持的 1295。只允许备份生成器三个精确会话守卫，其它控制语句继续拒绝。MySQL DDL 隐式提交，失败不能宣称整体回滚。
- [IPC 边界](<../../src/core/execution.ts>)保留数据库原始信息、code、operation_id、retryable、command、cause；旧 String(error) 用户提示不暴露包装类名。API、查询结果适配器、公共 Dialog/Wizard、文件选择按钮与启动消息均保留错误事实，不将失败当成功、空数据或用户取消。[IPC 回归](<../../tools/tests/ipc-errors.test.mjs>) 20/20 通过。
- [数据源同步设置](<../../src/dialogs/DatasourceSyncTab.ts>)未成功读取基线不能保存；[导入映射](<../../src/dialogs/ImportExportDialog.ts>)只有明确 TABLE_NOT_FOUND 才接受新表逻辑，权限/网络/未知错误保留草稿并阻断执行；旧任务列表读取失败保留已加载快照。
- 最新完整前端回归：1138 项，1136 通过、1 失败、1 跳过；唯一失败为并行监控布局固定 height=640 与原自然高度断言冲突，未删除/弱化断言。[完整日志](<../../.tmp/ipc-ui-final.log#L1150-L1155>)。TypeScript 与前端构建通过；扩展后端已有 181 通过、2 项专用实库取消用例 ignored，不计通过。
- ✅ 真实 MySQL + Windows/Tauri 最新交付程序 **8/8 通过**：fn_add10(5)=15、函数定义/调用、同名函数与过程、完整备份、15 条还原语句全部成功、typed 过程多结果、缺失例程/权限拒绝、编辑界面且 pageErrors=[]。[最新实库报告](<../../artifacts/routine-ipc-d7e000183c6643e2b29b927e1f4906ad/report.json>)、[界面证据](<../../artifacts/routine-ipc-d7e000183c6643e2b29b927e1f4906ad/function-editor.png>)；通过 pnpm test:routines:native 复跑：[实库入口](<../../tools/tests/routine-ipc-native.mjs>)。新鲜隔离 Cargo 产物 **17/17 通过**（真实还原分句、精确守卫/控制拒绝及既有词法回归）。不以旧缓存或零项匹配当通过。
- 交付 [嵌入前端的修复程序](<../../dist-portable/navkit-ipc-fixed.exe>)，SHA-256 e283f29f5151d12c4a4f8537489ff100d8e14fdfe946472c819e181c3acf9ff1，与实库所测完全一致；原运行中程序未替换，退出旧版后启动同目录修复版沿用现有 data。临时测试用户/数据库已删除；部分自建临时目录因 Windows EPERM 保留，没有修改 ACL。普通 SQL 查询入口的 CALL 仅返回影响行信息，多结果应使用正式 typed 例程调用通道；未宣称所有厂商/平台实库全覆盖。



**2026-11-12 全功能桌面验收轮（IPC+GUI 全遍历，79 用例 / 186 截图）**：

| 层级 | 内容 | 结果 |
|---|---|---|
| 桌面 E2E（本轮新增） | `manual-runner-20261112.mjs`：真实桌面 + 真实 MySQL/SQLite，菜单/连接/网格/设计器/查询/备份/函数/数据生成/监控/用户/调度/关系图/字典/同步/导入导出/驱动/设置/任务中心/事务/退出守卫全覆盖，逐步截图 | **79/79 通过，控制台 0 报错**（证据：`docs/active/evidence/manual-20261112/`，报告：`docs/active/全功能桌面验收报告与用户手册-20261112.md`） |
| 修复回归 | `cargo test --lib batch_execution` 21 例、`table_workflows` 32 例、`npm run test:ui` 1105 例 | 全部通过 / 0 失败 |

**本轮桌面验收实测修正的 2 处产品缺陷（均有修改前后截图与回归覆盖）**：

| 问题 | 根因 | 修复 |
|---|---|---|
| MySQL 表设计器打开任意表即报 `IpcError 1064`，字段元数据不可读、无法设计表 | 元数据查询 `GENERATION_EXPRESSION AS generated`——`generated` 为 MySQL 保留字且别名未加反引号（同语句 `default` 已正确加引号） | `table_workflows_native_metadata.rs` 别名改为 `` AS `generated` ``；设计器五页签/添加字段/真库生效全链路复验通过 |
| MySQL 数据生成写入与批量任务（GeneratedRows/SqlFile）全部失败：「结果未知 0/5 · 任务结束回执失败」，0 行写入 | `SELECT CONNECTION_ID()` 返回 `BIGINT UNSIGNED`，批量模块用 `i64` 解码（其余 4 处同查询均为 `u64`），在发出任何 SQL 前即失败 | `batch_execution.rs` 解码改 `u64`；探针复验「已扫描 5，执行回执 5，影响行 5，事务 committed」 |

**2026-11-12 新功能轮（数据字典导出 / 软件更新 / 后台任务中心 / 关系图）**：

| 层级 | 内容 | 结果 |
|---|---|---|
| L1 基础检查 | `tsc --noEmit` + `npm run build` | 通过，0 类型错误 |
| L1 后端编译 | `cargo check --lib` | 通过 |
| L2 前端单测 | `npm run test:ui` | **1105 通过 / 0 失败**（含新增 background-tasks、er-diagram、data-dictionary-dialog、update-dialog 四个套件；同步修复遗留的 ui-completion-styles F04/F05、tool-commands 生成器桩、database-workflows 批量桩与 object-tools 字节统计规格对齐） |
| L3 Rust 单测 | `cargo test --lib` | **996 通过 / 0 失败**（含新增 updater 7 例、data_dictionary 11 例） |
| 预览夹具 | `public/preview-bootstrap.js` 补齐 7 个新命令同形响应 | preview-mock-coverage 通过 |

本轮新功能实现要点：更新采用 magpie 的换名替换方案（运行中 exe 改名 `.old` → 新版落位 → 分离启动 → 新版启动清理旧版，落位失败回滚）；字典导出五种格式全部由 Rust 端生成（xlsx/docx 手写 OOXML，PDF 手写写入器 + 系统字体子集内嵌）；后台任务为前端任务中心统一登记，长任务本体仍是后端 invoke。

本轮（2026-11-11 收尾轮）在**新鲜二进制**上串行重跑，结果如下：

| 层级 | 内容 | 结果 | 边界 |
|---|---|---|---|
| L1 基础检查 | `tsc --noEmit` + `vite build` | 通过，0 类型错误 | Vite 保留既有分块警告 |
| L1 后端编译 | `cargo check --lib --offline` | 通过 | 130 条既有警告（未新增阻断） |
| L2 前端单测 | `npm run test:ui`，50 套件 1200 用例 | **1200 通过 / 0 失败 / 0 跳过**（170s） | 默认已串行（`--test-concurrency=1`），含真实 history 浏览器用例 |
| L3 Rust 单测 | `cargo test --lib`，1079 用例 | **1042 通过 / 0 失败 / 37 条件 ignored**（791s，串行，`cargo clean -p navkit` 后重编） | ignored 需 Java/厂商包/实库，不计通过 |
| L3 Rust 集成 | 15 个集成目标 | **36 通过 / 0 失败 / 8 条件 ignored**，15 目标全部编译通过 | 实库矩阵/模拟服务目标需显式环境变量，ignored 不计通过 |
| L4 HTTPS/H2 生产链路 | `http-tunnel-client` + Rust 精确用例 | **通过**：加密配置、真实 C1 授权、取消、命名空间绑定、重开持久化 | 仅本机临时 HTTPS/Java 11/H2 夹具 |
| L5 真实桌面 E2E | desktop14，14 需求 / 31 断言 | 31/31（本轮未重跑，沿用当日证据） | 仅 Windows |
| L6 桌面补充 | round1-extra，21 场景 | 21/21（同上） | 仅 Windows |
| L7 生产交付形态 | tauri build 产物冒烟，13 项 | 13/13（同上） | `bundle.active=false`，无安装包 |

**本轮实测修正的 5 处测试/产品问题（均有回归覆盖）**：

| 问题 | 根因 | 修复 |
|---|---|---|
| 整库同步被勾选的视图在增量模式下失败 | 增量通道对视图 fail-closed，但视图配对未保留物化路径 | 视图配对显式改全量物化并写明原因，不冒充增量快照 |
| HBase REST 夹具长跑偶发超时 | 夹具请求预算 3s，长跑下环回 connect 超 WSAETIMEDOUT | 夹具预算放宽至 15s，断言不变 |
| 驱动指纹测试写真实驱动目录 | 测试污染真实 `data/managed-drivers` | 改为私有空目录并注入路径，删除后指纹还原 |
| 调度 DAG 成功路径在负载下超时 | 两动作+真实 fsync 用 10s 预算 | 该成功路径用例改 60s，取消/超时用例保持 10s |
| 结构同步夹具 Windows 句柄残留 | 池关闭后句柄短暂未释放 | 断言池 size=0 后，仅对共享冲突做有界重试 |

**未覆盖、不得计为通过**：macOS / Linux；真实安装包；其余 90+ 数据源类型（仅协议模拟与拒绝路径）；
真实 WebDAV 服务器（仅隔离浏览器级）；J2EE/JDBC 全厂商实库；MySQL/PG 原生 ER 逆向的厂商实库验收
（`NAVKIT_ER_REVERSE_MYSQL_URL` / `NAVKIT_ER_REVERSE_PG_URL` 均未设置）。

## 四、能力矩阵

### 4.1 连接与数据源

| 能力 | 代码入口 | 验收 | 边界 |
|---|---|---|---|
| 连接管理：新建/编辑/删除/测试、分组、密码加密 | `src/dialogs/ConnectionDialog.ts`、`src-tauri/src/commands.rs`、`src-tauri/src/storage.rs` | ✅ | — |
| 密码与密钥：AES-256-GCM，密钥 `data/key.bin` | `src-tauri/src/storage.rs` | ✅ | 本机加密，不做跨机密钥分发 |
| 连接参数与 TLS：CA、主机名、客户端证书 | `src-tauri/src/connection_params.rs` | ⏳ | 真实 CA/mTLS 网络实测未跑；不能明文降级 |
| SSH 隧道：known_hosts 校验、密钥变化拒绝 | `src-tauri/src/connection_runtime.rs` | 🚧 | SSH+TLS 身份分离未支持；真实 SSH 认证未实测 |
| HTTP 数据库隧道：JDBC sidecar 网关 | `bridge/http-tunnel/NavkitHttpTunnel.java`、`src-tauri/src/http_db.rs` | ✅ | 本机临时 HTTPS/Java 11/H2 参考服务全链路通过；不等同通用代理，其他目标仍拒 |
| 连接保活：真实端点 generation、owned lease | `src-tauri/src/connection_health.rs` | ⏳ | 原生协议覆盖不全；SQLite 按策略跳过 |
| 原生协议 TLS | `src-tauri/src/native_mq.rs` 等 | 🚧 | Kafka/MQTT 已接真实 TLS 构造，其余四协议明确拒绝（未实现） |
| 驱动管理：ODBC / 真实 Java sidecar（JDBC） | `src-tauri/src/driver_manager.rs`、`src-tauri/src/jdbc_db.rs` | ⏳ | JAR 不作为原生 DLL 加载；远端 H2 与其他 JDBC 厂商未认证 |
| 驱动来源政策与固定指纹 | `src-tauri/src/managed_mirror.rs`、`src-tauri/src/driver_download.rs` | ✅ | 源码不含驱动二进制；HTTPS+固定大小/SHA-256+包结构验证；不执行安装器、不代接受许可 |
| 数据源 WebDAV 同步 | `src-tauri/src/datasource_sync/` | ⏳ | 仅同步连接配置；真实 WebDAV 服务器未实测 |

### 4.2 浏览与编辑

| 能力 | 代码入口 | 验收 | 边界 |
|---|---|---|---|
| 对象树：库/模式 → 表/视图/例程/触发器 | `src/core/App.ts`、`src-tauri/src/catalog.rs` | ⏳ | 完整身份与分页已接；驱动 adapter 远端完整性未实证 |
| 表数据网格：分页、排序、筛选、行内编辑 | `src/ui/DataGrid.ts` | ✅ | — |
| 大表分页：首屏不计数、绘制后自动统计总行数/总页数、写入/刷新失效与迟到防护 | [数据页](<../../src/core/App.ts#L7775-L7871>)、[后端分页](<../../src-tauri/src/schema.rs>)、[回归](<../../tools/tests/data-grid-filter-sort.test.mjs>) | ⏳ | 定向前端 159/159、前端构建与最新 Rust 离线编译检查通过；Rust 16+3+5 和百万 SQLite 为首屏阶段历史证据；用户原库桌面及其它厂商实库尚未现场验收，外部写入需刷新 |
| 受控写入：typed 绑定、CAS、同事务读回 | `src-tauri/src/mutation.rs`、`src-tauri/src/controlled_execution.rs` | ⏳ | 约束失败/资源隔离负向复验未做；未知提交冻结不重放 |
| 查询结果受控编辑与筛选排序 | `src/core/query-*.ts` | ⏳ | 只有能证明单表唯一键的来源才可写；Windows 关闭后文件占用失败待诊断 |
| 表结构：列/类型/主键/索引/外键/注释 + ALTER 预览 | `src-tauri/src/design.rs` | ⏳ | 复合有序键、约束编辑、部分 DDL 失败复核未闭环 |
| 对象变更：受控删除、完整基线、未知结果防重放 | `src-tauri/src/object_mutation.rs`、`src/core/object-mutation.ts` | ✅ | 仅 SQLite/main 用户触发器开放 `drop: limited`；视图触发器与内部对象不开放 |
| 查询编辑器：CodeMirror 6、高亮、补全、选区执行 | `src/ui/SqlEditor.ts` | ✅ | — |
| 查询执行门禁：只读保护、取消、执行预算 | `src-tauri/src/execution_policy.rs`、`src-tauri/src/execution.rs`、`src-tauri/src/execution_bound.rs` | ⏳ | 非 SQLite 任意只读 SQL 保守拒绝；Java 独立控制线程未实现；其他协议缺真实取消 |
| 手动事务：原生连接持有、明确提交/回滚 | `src-tauri/src/manual_transaction.rs` | ✅ | SQLite 仅 main；MySQL 要求当前库全表 InnoDB；事务内仅 SELECT/WITH 与 DML |
| 离页保存提醒 | `src/core/App.ts`、`src/dialogs/LeaveGuardDialog.ts` | ✅ | 关闭前询问是否保存查询 SQL；结构、模型和行草稿丢弃需确认；本机恢复稿已移除 |
| 网格复制：TSV/NULL/精度/二进制 | `src/core/grid-clipboard.ts` | ⏳ | OS 剪贴板真实读回未实机验收 |
| 例程：定义读取、安全重建、参数调用 | `src-tauri/src/commands.rs`、`src/core/routine-*.ts` | ⏳ | MySQL 既有例程破坏性重建/权限恢复未闭环；PG/MySQL 专用实库未提供 |
| 视图设计器：读取定义 → 编辑 → 保存 | `src-tauri/src/commands.rs` | ✅ | — |
| 关系图：表结构图 + 工程 ER 图、FK 关系与基数、搜索/自动布局/JOIN SQL | `src/core/erDiagram.ts`、`src/dialogs/ErDiagramDialog.ts` | ✅ | 元数据逐表读取（并发 6）；建模关系仅会话内标注，不写库；无 Layers/新建表工程流 |
| 后台任务中心：导入/导出/SQL 文件/转储/备份/数据生成/同步/字典/更新统一登记 | `src/core/backgroundTasks.ts`、`src/ui/TaskCenterPanel.ts` | ✅ | 进度取自各任务现有回执通道；点击已完成任务打开详情或产物目录 |

### 4.3 传输、同步与工具

| 能力 | 代码入口 | 验收 | 边界 |
|---|---|---|---|
| 导入导出：CSV/JSON/SQL 向导 | `src/dialogs/ImportExportDialog.ts`、`src-tauri/src/transfer.rs`、`src-tauri/src/transfer_export_cursor.rs`、`src-tauri/src/transfer_http_export.rs`、`src-tauri/src/transfer_mongo_export.rs` | ⏳ | 表级导出新增 ClickHouse/通用 ODBC（CSV/JSON/表数据 SQL）、HTTP/Native 与 MongoDB（CSV/JSON）；Mongo JSON 为 canonical Extended JSON。Redis 例外、MQTT 历史导出明确拒绝；非关系来源不生成 SQL、不接受 SQL WHERE。真实厂商验收及导入混合失败/目标 CAS 仍未做 |
| 数据传输：仅结构/仅数据/二者、列映射、主键比较、增量 | `src-tauri/src/transfer.rs`、`src-tauri/src/datasource_sync/`、`src-tauri/src/sync_stream.rs` | ⏳ | 一致快照、有界增量与别名自同步未验收；增量仅 SQLite 双端原生通道 |
| 结构差异同步 | `src-tauri/src/transfer.rs`、`src-tauri/src/structure_commands.rs` | ⏳ | 其他方言、外键/检查变更、保数据重建未闭环 |
| 大数据流式传输 | `src-tauri/src/transfer.rs` | ⏳ | 增量分支、单超大驱动行内存上界、跨厂商背压未实测 |
| 备份 / 还原：整库 SQL 转储、备份列表与恢复 | `src/dialogs/BackupDialog.ts`、`src-tauri/src/transfer.rs` | ✅ | MySQL DDL 自动提交，重建目标表不可整体回滚 |
| 数据字典导出：Word/PDF/Excel(单 sheet/每表)/HTML/Markdown | `src-tauri/src/data_dictionary.rs`、`src/dialogs/DataDictionaryDialog.ts` | ✅ | 模板列集照 SmartSQL；Word/Excel/HTML 仅表，PDF/Markdown 含视图与存储过程；PDF 内嵌系统中文字体（SimHei/等线），无字体时报错不生成 |
| 软件更新：检查/下载(SHA-256+断点续传)/换名替换重启/清理旧版 | `src-tauri/src/updater.rs`、`src/dialogs/UpdateDialog.ts` | ✅ | 清单挂自有镜像仓库 Navkit-drivers（可用 feed.json/环境变量覆盖）；换名替换参考 magpie，落位失败自动回滚；未做真实大版本跨级演练 |
| 自动运行：任务保存、到期执行、历史 | `src-tauri/src/commands.rs`、`src/core/scheduler*`、`src-tauri/src/scheduler_offline.rs` | 🚧 | Cron/DST/lease/DAG/离线安装计划内核已接；系统任务注册与真实 headless 运行未做 |
| 用户管理：MySQL 协议族 `mysql.user` 真实数据、编辑用户与权限管理（常规/高级/成员属于/成员/服务器权限/权限/SQL 预览） | `src-tauri/src/db_users.rs`、`src/dialogs/UserDialog.ts` | ✅ | 仅 MySQL 协议族；角色页依赖 MySQL 8.0+ 的 `mysql.role_edges` |
| 服务器监控、数据生成、日志检索 | `src-tauri/src/commands.rs` | ✅ | — |
| 通用：最近打开、收藏夹、设置、全中文界面 | `src/dialogs/SettingsDialog.ts`、`src-tauri/src/storage.rs` | ✅ | — |

### 4.3.1 数据管线代码与明确能力边界

| 能力 | 代码入口及本地测试 | 验收 | 边界 |
|---|---|---|---|
| 有界CSV样本与遗留表导出游标化 | [预览](<../../src-tauri/src/transfer_preview.rs>)、[导出](<../../src-tauri/src/transfer.rs>)、[真实样本oracle测试](<../../src-tauri/src/transfer_preview_bench.rs>) | ⏳ | 样本不代表完整文件校验；输出stage成功后发布；其他驱动预读和桌面现场未验证 |
| 原完整事务内SQLite门控多值导入 | [writer/gate](<../../src-tauri/src/transfer_import_batch.rs>)、[任务接入](<../../src-tauri/src/transfer.rs>) | ⏳ | 完整显式列、无trigger/FK普通表；≤256行/约1MiB；未统一为全协议typed参数writer，未知回执不重放 |
| 后端确定性大总量recipe、SQLite显式分段提交 | [规则](<../../src-tauri/src/generated_recipe.rs>)、[授权指纹](<../../src-tauri/src/generated_recipe_runtime.rs>)、[分段](<../../src-tauri/src/generated_recipe_segments.rs>)、[原生测试](<../../src-tauri/src/generated_recipe_tests.rs>)、[界面](<../../src/dialogs/DataGeneratorDialog.ts>) | ✅ Windows/SQLite 公开预览；其它实库 ⏳ | 最多20行预览；atomic默认；segmented仅SQLite且确认前缀不可撤销，每1000行/8MiB绑定预算先到；结构逐段锁内核验；PG/MySQL分段写前拒绝。已验证隔离SQLite千万行count/distinct/确认COMMIT，不能外推任意宽表/driver峰值 |
| 独立低内存差异比较模式 | [默认兼容](<../../src-tauri/src/sync_diff_compat.rs>)、[bounded](<../../src-tauri/src/sync_diff.rs>)、[存储](<../../src-tauri/src/sync_diff_store.rs>)、[独立进程采样](<../../src-tauri/src/sync_diff_memory_tests.rs>)、[选项交互测试](<../../tools/tests/sync-diff-option.test.mjs>) | ⏳ | 默认不启用；精确值/键比较保留；比较按钮与内存策略同组、只读/整库隐藏，窄窗口长结果不覆盖工具。实际时间/内存取舍记录在当前用户审阅表，不当普遍峰值保证。PG/MySQL bounded ≤2048行/16MiB，不推广限制到默认功能 |
| SQLite文件main备份目录/DDL/所选数据同一只读快照 | [读取](<../../src-tauri/src/transfer_backup_snapshot.rs>)、[并发/roundtrip测试](<../../src-tauri/src/transfer_backup_snapshot_tests.rs>) | ⏳ | 仅受支持的逻辑对象范围；显式索引与所选对象触发器同快照捕获、数据后重建，不自动补全未选引用对象；不含全实例/PITR；非main/内存或其他协议保留兼容分支 |
| PG任务级RR只读快照、MySQL源读锁显式逻辑备份 | [源会话与发布](<../../src-tauri/src/transfer_backup_native.rs>)、[门禁/计划测试](<../../src-tauri/src/transfer_backup_native_tests.rs>)、[共享reader](<../../src-tauri/src/transfer_mysql_export.rs>)、[选择](<../../src/dialogs/BackupDialog.ts>) | ⏳ | 默认compat；PG普通表/OID锁/nonce，结构PG14+且拒绝不能完整保留属性；MySQL原生8.0/8.4/InnoDB/一次READ锁。可能隐式关联锁升级WRITE，已知trigger/FK依赖拒绝，不保证只锁所选；无实库URL，实际锁负载/断线验收未做 |
| 固定源流式详细还原与公开停止后续语句 | [预扫/MAC](<../../src-tauri/src/transfer_sql_reader.rs>)、[执行/报告](<../../src-tauri/src/transfer_restore_stream.rs>)、[测试](<../../src-tauri/src/transfer_restore_stream_tests.rs>)、[公开命令](<../../src-tauri/src/commands.rs>) | ⏳ | 执行顺序/原DML软提交边界保留；最大50MiB语句/2048报告。区分回执与确认提交/回滚/未知；关闭界面后台继续，停止等待在途操作。不安全任意事务/SET/COPY等预扫拒绝，普通SQL任务走原独立路径 |
| 显式SQL INSERT组包与受限原生PG text COPY | [SQL组包](<../../src-tauri/src/transfer_sql_batch.rs>)、[COPY](<../../src-tauri/src/transfer_pg_copy.rs>)、[向导](<../../src/dialogs/ImportExportDialog.ts>) | ⏳ | SQL默认单行，组包需确认恢复语句语义；COPY默认关闭，PG heap完整列/类型白名单，无trigger/FK/CHECK/index/default依赖；abort/finish/rollback/unknown关连接不重放，实PG验证未做 |
| 有限import/export生产者准入 | [控制器](<../../src-tauri/src/transfer_admission.rs>)、[任务](<../../src-tauri/src/transfer.rs>)、[生命周期测试](<../../src-tauri/src/transfer_admission_task_tests.rs>) | ⏳ | 默认最多4producer，205MiB/producer和820MiB总预留是保守账本不是RSS/allocator硬限；第五个立即拒绝而非无限排队；cancel/drop不早释放，完成实际worker/清理后finish；串行nested末成员释放。不是所有任务/driver统一内存硬限 |

可复现正确性命令：`cargo test --offline --manifest-path src-tauri/Cargo.toml --lib --no-fail-fast -- --test-threads=1`、`cargo test --offline --manifest-path src-tauri/Cargo.toml --lib recipe_ -- --test-threads=1`、`node --test tools/tests/data-generator-recipe.test.mjs tools/tests/backup-consistency.test.mjs tools/tests/ipc-errors.test.mjs`。最新完整Rust候选 **1274通过/0失败/53 ignored**；分段/原子recipe **18通过**，准入 **12通过**，native备份 **8通过/4实库ignored**，公开还原 **19通过**。最新完整前端 **1336通过/0失败/1 skipped**、备份/还原/同步5个集成target **20通过/0失败/1 ignored**，TypeScript/Vite构建与文档链接闸门通过；不是专用PG/MySQL现场验收，旧失败快照不抹掉。本节记录已落地代码，不等同全计划全部协议性能闸门完成；具体收益/负收益与保留选择只放当前审阅报告，不将ROI历史塞进结果文档。

| 可用内存预算差异比较一期 | [系统采样/共享额度](<../../src-tauri/src/diff_memory_budget.rs>)、[预算读取/快照](<../../src-tauri/src/sync_diff.rs>)、[容量核算/HashMap/溢写](<../../src-tauri/src/sync_diff_store.rs>)、[控制器回归](<../../src-tauri/src/diff_memory_budget_tests.rs>)、[独立测量入口](<../../src-tauri/src/sync_adaptive_memory_bench.rs>)、[开关/请求冻结](<../../src/dialogs/SyncDialog.ts>) | ⏳ | 仅原生文件SQLite；自动/手动、speed关闭仍有保底保护。预算按有效available共享按需使用，不是RSS硬上限；原diff/旧bounded调用保持兼容。最低64MiB含48MiB暂存承诺；足额有限预读额外最坏容量先收费；quota在真实close后归零。小/宽表仍有可观测回退，未声明全部性能闸门通过或默认开启auto；厂商/全平台/实际混合并发压力未验收，保留决定待用户 |

### 4.4 已接入的数据源

| 类型 | 列表 |
|---|---|
| 内置驱动（完整支持） | SQLite、MySQL / MariaDB、PostgreSQL |
| MySQL 协议族 | TiDB、OceanBase、PolarDB、TDSQL、Doris / StarRocks |
| PostgreSQL 协议族 | openGauss、KingbaseES、HighGo |
| 原生 HTTP 接口 | ClickHouse |
| 系统 ODBC 驱动 | 达梦 DM、Hive；表级导出分派已覆盖全部 34 个 ODBC 目录类型，需用户安装兼容驱动，不等于厂商实库认证 |
| JDBC / H2 | Java 11+ sidecar，严格匹配原包与厂商类 |

表设计读取按服务端实际能力探测，不按连接类型等同：
- MySQL 协议族：MySQL 8 用 `@@server_uuid` + `information_schema.INNODB_TABLES`；MariaDB 用 `INNODB_SYS_TABLES`；TiDB 用 `TIDB_TABLES.TABLE_ID`。TiDB 没有触发器，其空目录按方言事实处理；TiDB 的 `GET_LOCK`/`LOCK TABLES` 是 noop 兼容功能，**表结构保存明确拒绝**（读取、预览仍可用）。
- PostgreSQL 协议族：`attgenerated`/`attidentity`/`attcompression`/`indnkeyatts`/`pg_sequence`/`relrowsecurity` 等版本相关列逐个探测，缺失时降级并在设计器 limitations 中标注；解构约束/索引改用所有版本都接受的单参数 `pg_get_*`。
  - openGauss 7.0.0-RC3（9.2.4 内核）实测没有 `to_jsonb(record)`（42883），索引键列改用 `to_json(x)->>'indnkeyatts'`：openGauss 与 PG 11+ 直接取到 `indnkeyatts`，PG 10 取不到时回退 `indnatts`（PG 10 无 INCLUDE，语义等价）。`WITH ORDINALITY` 实测支持，无需替换 `generate_subscripts`。
  - 索引元数据分两层：现代层用 `unnest ... WITH ORDINALITY` + `indnkeyatts`；失败时降级 `string_to_array` + `generate_series` 兼容层（不依赖 ORDINALITY/indnkeyatts，PG 9.x 系与老 openGauss/GaussDB 可跑）。
  - 列元数据带最后兜底：`pg_catalog` 查询失败时退到标准 `information_schema.columns`，只丢 identity/generated 细节并在 limitations 说明。
  - openGauss/GaussDB 的 B(MySQL) 兼容模式（`pg_database.datcompatibility` = B/M）标识符用反引号、`||` 可能被当逻辑 OR，按 PG 语义生成 SQL 会出错，**表设计读写明确拒绝**（`TABLE_PG_MYSQL_COMPAT_MODE_UNSUPPORTED`），A/PG 模式正常。表数据摘要的 PG 分支改用 `concat()` 而非 `||`。
  - 对象目录的序列属性：探测 `pg_sequence` 是否存在，缺失时改用 `information_schema.sequences`（openGauss/GaussDB 无 `pg_sequence`）。同一 owned 会话内 PG 方言能力探测结果会缓存，保存表设计时不再对每步 DDL 重复探测。
- Doris/StarRocks/SelectDB/Databend/Manticore、CockroachDB/Redshift/Cloudberry/QuestDB 的表目录/DDL 与 MySQL/PostgreSQL 不兼容，表设计入口明确拒绝，不把 1054/1146/42703 裸抛给界面。

目录共 101 个条目（34 个 ODBC）。其余 90+ 类型仅提供图标与协议探测，**未做厂商实库认证**，
按「明确拒绝或协议模拟」处理；**目录条目数不等于已认证数据源数**。

## 五、实现归档（原 PL 任务与后续实现）

原 `plan/` 的 25 项任务书已全部开发落地，后续完成的实现也并入本表；结论归档后删除过程原件。状态按 §二 图例：

| ID | 任务 | 已落地 | 待办 |
|---|---|---|---|
| PL-01 | 只读连接执行保护 | SQLite 实际只读、authorizer、保存配置权威门禁 | 远端/驱动复杂只读复核 🚧 |
| PL-02 | 适配器图形化写入闭环 | 类型化值、唯一 C2 计划/预算、同事务 CAS/读回、未知提交冻结、JDBC/H2 桥 | 真实 H2 六入口、其他厂商负向复验 ⏳ |
| PL-03 | 查询取消与停止 | 统一 C1 注册表、授权握手、定向控制、资源未排空隔离 | 所有协议与新工具通道接入、真实网络失联 🚧 |
| PL-04 | 查询执行超时 | 起始预算、单一停止原因、Unknown/资源排空 | 新类型化调用通道、网络排队竞态验收 ⏳ |
| PL-05 | 表属性与主键保存 | 方言设计计划、前端默认值/主键/注释保存初版 | C1 接入、真实方言读回、部分 DDL 失败复核 🚧 |
| PL-06 | 索引外键检查触发器 | 索引/外键编辑、真实约束与触发器读取 | 有序复合键、约束编辑、触发器安全写 🚧 |
| PL-07 | 离页保存提醒 | 关闭时未保存 SQL 保存到已保存查询、其它编辑显式丢弃、进行中操作阻止离页 | 本机恢复稿删除后无跨实例恢复路径 ✅ |
| PL-08 | 网格复制 | 多格式编码、NULL/精度/二进制、失败入口 | OS 剪贴板真实读回 ⏳ |
| PL-09 | 例程安全重建 | 精确身份、加载时定义基线、PG 受控 body 替换、一次性 CREATE | MySQL 破坏性重建/权限恢复、外部 DDL 竞争 🚧 |
| PL-10 | 结构差异同步 | 完整有序键/引用配对、双端物理身份、一次 review 执行、事务读回 | 其他方言、外键/检查变更、保数据重建 🚧 |
| PL-11 | 业务键增量数据同步 | SQLite 快照游标、业务键 typed 定位/原值 CAS/全行读回、视图配对显式全量物化 | 实库复验、正式删除预览、跨厂商 🚧 |
| PL-12 | 导入冲突处理 | 冲突选项与后端规则初版 | 混合失败、目标 CAS、类型化绑定实测 ⏳ |
| PL-13 | 大数据流式传输 | 非 UTF8 固定 buffer、SQL 完整预扫、单 SELECT cursor、原子不覆文件 | 增量分支、单超大行内存上界、跨厂商背压 🚧 |
| PL-14 | MySQL 高级连接参数 | charset/collation/socket 严格诊断，每物理会话核对生效参数 | 真实 MySQL 握手与服务端配置验收 ⏳ |
| PL-15 | TLS 证书与客户端认证 | 严格别名/模式/证书验证、密文运行配置、每会话 TLS 核验 | 真实 CA/主机名/mTLS 网络实测 ⏳ |
| PL-16 | SSH 隧道 | known_hosts 校验、密钥变化拒绝、owned 转发与资源生命周期 | SSH+TLS 身份分离、真实认证与断线回归 🚧 |
| PL-17 | HTTP 数据库隧道 | 受信 HTTPS、固定 server-target-session 世代、禁重放、H2 参考服务、加密 profile | 真 HTTPS 客户端故障、DDL/手动事务端到端 🚧 |
| PL-18 | 原生协议 TLS | Kafka/MQTT 真实 TLS 构造 | 其余四协议实现与专用服务握手 🚧 |
| PL-19 | 数据库对象管理 | 完整 identity/signature/parent、分页、精确定义、树/对象页接线 | trigger/event/sequence 受控写与启停 🚧 |
| PL-20 | JDBC 与 H2 图形化编辑 | 认证能力、PreparedStatement/同会话 CAS/生成键/读回、公共 C2/IPC 接线 | 六入口 Rust→Java 实库复验、其他厂商 DDL 写 🚧 |
| PL-21 | 查询结果受控编辑与筛选排序 | SQLite 真实列 origin、typed 行令牌、C2 事务内 ABA 校验、页代际 | Windows 关闭后文件占用诊断、其他驱动来源 ⏳ |
| PL-22 | 例程参数调用 | 完整参数 metadata、typed 一次性计划、多结果/OUT/INOUT、C1 执行/停止 | 专用 PG/MySQL 服务、自定义类型 🚧 |
| PL-25 | 高级调度与任务链 | Cron/IANA/DST/持久 CAS/lease/DAG、服务端 opaque 审批、模板 UI、离线安装计划 | 工具执行链真实验收、离线 OS runner/系统任务 🚧 |
| PL-27 | 连接保活 | 后端 generation/owned lease/activity、前端单请求单 timer、迟到隔离 | 最新 Rust 生命周期与失联服务实测 ⏳ |
| PAGE-TOTAL-01 | 首屏后自动总行数/总页数 | [数据页](<../../src/core/App.ts#L7775-L7871>)双帧后 count-only；筛选键/版本快照，刷新与真实写入失效，保存结束重算，错误手动重试与关闭取消；[定向回归](<../../tools/tests/data-grid-filter-sort.test.mjs>) | 用户原库桌面与其它厂商实库现场验收 ⏳；后台 COUNT 无固定耗时承诺，外部写入需刷新 |
| EXPORT-01 | 新增数据源表级导出 | [统一游标](<../../src-tauri/src/transfer_export_cursor.rs>)接入 ClickHouse/全 ODBC；[HTTP/Native 导出](<../../src-tauri/src/transfer_http_export.rs>)用协议专属游标及动态列磁盘暂存；[Mongo 导出](<../../src-tauri/src/transfer_mongo_export.rs>)保留 BSON 类型和 missing/null；成功原子发布、失败不覆盖旧文件 | 厂商实库及新来源真实桌面验收 ⏳；视图 SQL、历史消息及快照边界见 §六 |
| RELEASE-ACCEPT-01 | 封存 Windows 发布验收与安全修复 | [前端清单/映射门禁](<../../src/dialogs/ImportExportDialog.ts>)、[快照备份完整性](<../../src-tauri/src/transfer_backup_snapshot.rs>)、[原生还原](<../../src-tauri/src/transfer_restore_stream.rs>)、[预览摘要](<../../src-tauri/src/generated_recipe_runtime.rs>)、[发布身份门禁](<../../tools/check-release.mjs>)；完整回归与固定哈希原生工作流，Windows release/NSIS 构建 | 旧快照的公开预览完成误判已由 RECIPE-PREVIEW-FIX-01 消除；后续源码不自动算通过；跨平台、专用实库与安装/卸载 ⏳ |
| RECIPE-PREVIEW-FIX-01 | 原生公开预览完成误判 | [正式终态与零写入门禁](<../../src-tauri/src/generated_recipe_runtime.rs#L36-L48>)、[生命周期回归](<../../src-tauri/src/generated_recipe_tests.rs>)；4/4 新回归、完整库1328通过/56ignored、相关前端65/65、真实公开IPC8/8、新快照12组原生工作流与缓存6组通过 | Windows/SQLite 已验证 ✅；其它平台/厂商实库及安装/卸载 ⏳；未自动发布 |
| DRIVER-FIX-01 | 全驱动实测问题 P1–P10 | [备份列表有界读取](<../../src-tauri/src/storage.rs>)、[存储根切换](<../../src-tauri/src/storage.rs>)、[SQL 文件独立超时](<../../src/ui/BatchExecutionPanel.ts>)、[JDBC 顺序批量](<../../src-tauri/src/jdbc_db.rs>)、[空间/INTERVAL/MONEY 解码](<../../src-tauri/src/values.rs>)、[跳过不兼容表](<../../src-tauri/src/transfer.rs>)、[字典路径](<../../src/dialogs/DataDictionaryDialog.ts>) | 达梦/金仓/HighGo 现场复测与未归因崩溃 🚧；真实 MySQL 几何重建 ✅ |

| PERF-01 | 数据管线代码与受控策略 | [有界预览/原事务批写](<../../src-tauri/src/transfer.rs>)、[recipe原子/SQLite分段](<../../src-tauri/src/generated_recipe_segments.rs>)、[详细还原](<../../src-tauri/src/transfer_restore_stream.rs>)、[显式native备份](<../../src-tauri/src/transfer_backup_native.rs>)、[COPY](<../../src-tauri/src/transfer_pg_copy.rs>)、[生产者准入](<../../src-tauri/src/transfer_admission.rs>)；本地正确性/规模与前端覆盖 | 已开发范围⏳现场验收；全协议typed参数批/PG-MySQL分段/所有入口资源和交互保留预算🚧；bounded diff负收益默认关闭；当前审阅表待用户决定保留，不自动提交 |

| ROUND4-FIX-01 | 第三轮回归修复与实库性能 | [整库转储目标相对身份](<../../src-tauri/src/transfer.rs>)、[YEAR 无损解码](<../../src-tauri/src/values.rs>)、[同引擎类型映射](<../../src-tauri/src/sync.rs>)、[连接树刷新](<../../src/core/App.ts>)、[本机 MySQL 错误分类](<../../src-tauri/src/sqlx_runtime.rs>)、[严格 MySQL 导入批写门控](<../../src-tauri/src/transfer_import_batch.rs>)、[共享 JSON 写出](<../../src-tauri/src/transfer_json_row.rs>)、[全驱动矩阵生成器](<../../tools/tests/round4-driver-matrix.mjs>) | MySQL 实库已验证 D7/YEAR/D10 与导入 5.452×/19.277×/26.834×；SQLite/MySQL JSON 导出 2.359×/1.360×；D11 `CREATE DATABASE/USE` 仍 🚧；其他厂商实库性能未验收 ⏳ |

## 六、已知限制汇总

1. **平台**：只在 Windows 11 x64 验证过；无 macOS / Linux 结论。
2. **交付形态**：本轮 Windows 绿色版单 exe 与 NSIS 均已构建；NSIS 尚未实际安装 / 卸载验收，签名、SmartScreen 与跨版本更新演练未验证。
3. **只读保护**：非 SQLite 的任意只读 SQL 采用保守拒绝策略。
4. **取消与超时**：Java 侧无独立控制线程；PG/MySQL 有定向控制但失联与 quiescence 未完成。
5. **未知写**：默认不删除、不降级、不重放未知写操作；丢回执时冻结同一 execution ID。
6. **实库覆盖**：MySQL 8.0 / PostgreSQL 16 / SQLite 有真实服务器结论；其余厂商无。
7. **测试口径**：最新修复版库1328通过/56条件ignored、相关前端65通过、真实公开预览8/8。前一轮完整UI1399通过/1默认跳过及浏览器17/17是历史结果，不冒称本轮完整UI重跑，也不代表后续并行工作树通过。
   ignored 不是通过。厂商实库矩阵目标（`datasource_matrix`、`*_live_matrix`、`sim_*_catalogue`、
   `live_native_brokers`）需显式环境变量，本轮未运行。
8. **增量同步**：仅认证 SQLite 双端原生有界快照/typed 事务；视图无稳定业务键，配对时显式改全量物化。
9. **ER 原生逆向**：MySQL/PG 只读快照有隔离测试，但厂商实库 URL 未提供，验收仍为 ⏳；在线非 SQLite DDL 始终拒绝。
10. **驱动来源**：GBase 8c 只有目录与手动配置路径；候选 JAR 未核验指纹/类、未发布自有镜像，不构成自动下载。
11. **继续实施**：§五 标 🚧 的条目仍有代码缺口。
12. **导出格式与对象语义**：ClickHouse/ODBC 导出表数据 SQL，不宣称完整 schema 备份；ClickHouse 与 Oracle ODBC 视图使用真实定义，其它 ODBC 视图 SQL 明确拒绝。HTTP/Native JSON 保持适配器字符串/null 表示，不能冒称统一类型化备份；Mongo canonical Extended JSON 保留嵌套/BSON、CSV 为动态列并集。Mongo/HTTP/Native 不接受 SQL WHERE、SQL 输出；Redis 无表级导出，MQTT 无标准历史读取；Pulsar 为目录/主题统计，etcd/ZooKeeper 为单 key/znode 当前值，均非消息历史或递归备份。入口见[SQL 游标](<../../src-tauri/src/transfer_export_cursor.rs>)、[HTTP/Native 导出](<../../src-tauri/src/transfer_http_export.rs>)、[Mongo 导出](<../../src-tauri/src/transfer_mongo_export.rs>)。
13. **导出快照与预算**：SQL 单 SELECT 游标不依赖分页 OFFSET；ODBC 在接收端限制行数，不假定所有厂商支持同一 LIMIT 方言。HTTP 页默认 64 MiB、单记录 16 MiB、列数 16,384、令牌 64 KiB、游标历史 1,024，HBase/Salesforce 页限为 16 MiB；超限报错且不发布半成品。Consul/Nacos/RabbitMQ/Pulsar 为有界本地列表分段，Nebula/部分 REST offset 非一致快照；服务端深页/会话/TTL 限制仍适用。SQLx/ODBC 单个 LOB 可能先由驱动分配再检测，不宣称任意单超大行固定内存；Mongo 未实现跨集合/跨页一致快照。HBase scanner/ES PIT 按实际协议清理，Salesforce locator 无 DELETE 接口，提前停止不伪造远端清理。预算入口见[流式契约](<../../src-tauri/src/transfer_stream.rs>)及[HTTP 适配器](<../../src-tauri/src/http_db.rs>)。
14. **全驱动实测遗留**：金仓保存、HighGo 查询、达梦首屏、H2 空路径等静默退出仍缺运行时崩溃证据（已补 sidecar 退出码与有界 stderr，不代表修复）；达梦/金仓/HighGo 未在本次环境复测；PostgreSQL MONEY/INTERVAL/POINT 的网格解析需真实 PG 现场验收。入口见 [DRIVER-FIX-01](<../../src-tauri/src/values.rs>)。

15. **生成预览验收边界**：正常无取消完成已修复，不再误报 Unknown；失败、取消、超时、未知清理与在途资源仍拒绝返回样本。本次公开 IPC 只实测 Windows/SQLite，不外推全部厂商与压力环境。

16. **第四轮遗留**：D11 含 `CREATE DATABASE/USE` 的 SQL 文件仍未接入受控管理脚本授权，通用路径继续拒绝；D9 只对 localhost 认证/连接错误做了明确分类，没有自动 IPv4/IPv6 回退；TiDB/SQL Server/达梦/PostgreSQL 等现场项未复验。MySQL 导入批写只对精确 `mysql` 8.0/InnoDB/无副作用普通表自动启用，其他协议仍逐行。全驱动矩阵是源码能力清单，运行证据均为 not-run，不能当作厂商实库通过。

## 七、文档来源合并记录

本文档合并了以下过程文档与直接结果。已完成过程原件按生命周期删除；明确功能缺口的当前轮次另留 active，并在已知限制中记录：

| 原件 | 合并内容 |
|---|---|
| NavKit-完整任务书.md | 产品事实、界面版式基准约束 |
| 验收报告.md | 早期全量验收结论 |
| 14项修复验收报告.md | 14 项缺陷修复结论 |
| 全功能真实测试报告-20261107.md | 2026-11-07 动态测试结论 |
| 差异清单.md | 已失效（对照截图从仓库移除），未合并 |
| 界面规格表.md | 版式规格，已并入 `docs/design/DESIGN.md`（唯一权威设计规范） |
| plan/01–27 任务书（25 份） | 并入 §五 实现归档 |
| plan/IMPLEMENTATION-STATUS.md | 并入 §三 验证状态与 §五 边界 |
| docs/验收证据/2026-11-11 两份续推报告 | 并入 §四 对象变更与 ER 能力行 |
| active/release-test-20261110（计划、问题清单、修复方案、交付报告、证据） | 并入 §三 验证状态、§三 实测修正表与 §六 限制 |
| active/尚未实现功能推进记录-20261111.md | 并入 §三 本轮实测、§四 对象变更/ER 能力行与 §五 PL-11/PL-19/PL-23 边界 |
| active/驱动下载来源政策与待办.md | 并入 §四 驱动来源政策行、§五 与 §六 第 10 条 |
| active/ER原生逆向下一切片-20261111.md、planned/ER原生逆向下一切片-20261111.md | 并入 §四 ER 能力行、§五 PL-23 与 §六 第 9 条 |
| active/当前轮次收尾与验证-20261111.md | 并入 §三 本轮全量结果与 §三 实测修正表 |
| 2026-11-12 IPC 与例程备份修复（直接结果归档） | §三 增补真实根因、错误边界、回归与未通过项，不新增已完成过程文档 |
| active/automatic-page-totals.md | 自动统计及快照/写入/事务/迟到边界并入 §三、§四能力行与 §五 PAGE-TOTAL-01；159/159 定向、前端构建与最新 Rust 离线编译检查通过，全量 UI 未通过及前两次 Rust 编译失败保留为历史，过程原件删除 |
| 2026-11-13 新增数据源表级导出（直接结果归档） | ClickHouse/全部 ODBC、HTTP/Native 与 Mongo 分派、真实分页/资源回收/文件发布及格式/对象/预算边界并入 §三、§四、§五 EXPORT-01 和 §六；本地协议夹具不作为厂商实库认证，不新增已完成过程文档 |
| active/driver-test-fixes.md | 并入 §三 2026-11-13 全驱动实测问题修复块、§五 DRIVER-FIX-01、§六 第 14 条；原件按生命周期删除 |
| 2026-11-14 封存发布验收（结果合并） | §三合并快照哈希、完整回归、真实原生工作流、构建与明确不通过结论；§六记录公开 recipe 预览完成误判，旧失败报告已移入 artifact 历史区；完成判定后续修复见 RECIPE-PREVIEW-FIX-01，不把后续并行源码改动当作已通过 |
| active/release-acceptance-2026-11-14.md（后续修复归档） | 公开预览复用正式终态并保留严格未知拒绝；完整库、相关前端、固定哈希真实IPC/工作流/缓存与新构建结果并入§三/§五；旧失败报告保留artifact，过程文件从active删除 |
| active/navkit-round4-regression-performance.md | 并入 §五 ROUND4-FIX-01、§六 第 16 条；D7/YEAR/D10/D5/D9 与全驱动矩阵、MySQL/SQLite 性能结论归档；D11 仍标 🚧，过程文档保留在 active 以对应未完成缺口 |
| active/query-streaming-backend.md | §三 合并独立流式后端、成功终态导出门、MySQL 回环/生命周期回归与真实边界；前端未开工项放 planned/query-streaming-frontend.md |
