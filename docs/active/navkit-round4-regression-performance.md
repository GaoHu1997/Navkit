# NavKit 第四轮回归与全驱动性能优化计划

## 范围

- 修复第三轮确认的 D7、YEAR、D10、D5、D9、D11，并复查 D17/D19/P5-R/D13/D14/D8 的当前状态。
- 性能范围覆盖 NavKit 登记的全部数据库与实际驱动路由：原生 SQLite/MySQL/PostgreSQL、TDS、ClickHouse、ODBC、JDBC sidecar、HTTP/API、Native/NoSQL 等；每个操作按代码能力标记支持、明确不支持或未验收。
- 使用本机 MySQL/InnoDB 做真实导入、导出、转储/还原、同步、SQL 文件和查询性能锚点；使用 SQLite 做本地完整回归与 A/B。其他真实服务仅在现成隔离环境可用时验证；H2 只补充其 Java sidecar 的连接/元数据/DDL/DML/事务能力。
- 只在同语义、同事务/持久性、同输入和同实际入口的 A/B 中报告性能变化。

## 非目标

- 不把协议兼容别名、模拟 fixture、H2 或原生客户端当成 MySQL/PostgreSQL 实库证明。
- 不放开任意 `USE`、角色/用户/服务器授权 SQL；不取消范围绑定、类型校验、事务回滚、未知状态、MAC/CAS、文件原子发布或 fsync。
- 不以关闭约束、IGNORE/REPLACE、丢数据、静默转 NULL、自动重放或无限队列换吞吐。
- 不在没有实测收益时默认启用 MySQL/PG 多值写入、COPY、LOAD DATA、多 worker、快照/锁模式。
- 不覆盖、回退或清理工作树中其他用户改动，不提交或推送。

## 进入条件

- 第三轮报告与性能实施状态已读，当前工作树状态已记录；未跟踪第三轮报告保留。
- 已确认源代码的转储、导入、同步、连接、树缓存和 SQL 文件范围门禁路径。
- 真实 MySQL 凭据只从现有本地环境读取，不写入仓库或证据。

## 验收标准

1. D7 源库在跨库还原后结构和数据不变，目标库完整恢复；失败不会误发布。
2. YEAR、NULL、二进制、decimal、时间等导出值无损或明确报错，不伪装为 NULL。
3. 同引擎同步保留可验证的整数、精度/scale、时间精度；异构映射和类型 allowlist 不退化。
4. D5/D9/D11 具有专项回归：竞态不会覆盖新树数据；认证失败不被错误重试；普通 SQL 文件范围保护仍有效，受控管理脚本范围明确。
5. MySQL 与 SQLite 性能 A/B 包含提交、sync/publish、完整性校验、行数/字节、RSS 与错误/取消边界；无稳定收益的候选不默认启用。
6. 全驱动矩阵逐产品/实际驱动记录操作能力和证据等级；未配置真实服务写为未验收，不写成通过；不支持的操作在写入前明确失败。
7. 相关 Rust/前端测试、构建和文档链接检查结果如实记录。

## 代码触点

- 转储/导入/还原：`src-tauri/src/transfer.rs`、`src-tauri/src/transfer_export_cursor.rs`、`src-tauri/src/values.rs`、`src-tauri/src/transfer_import_batch.rs`、`src-tauri/src/transfer_restore_stream.rs`。
- 同步/映射：`src-tauri/src/sync.rs`、`src/dialogs/SyncDialog.ts`。
- 连接/树：`src-tauri/src/sqlx_runtime.rs`、`src/core/App.ts`。
- SQL 文件范围：`src-tauri/src/batch_sql_reader.rs`、`src-tauri/src/execution_policy.rs`、`src-tauri/src/batch_execution.rs`。
- 共享测量与证据：`src-tauri/src/transfer_stream.rs`、`src-tauri/src/performance_rss_probe.rs`、`src-tauri/tests/perf_matrix.rs`、`tools/tests/backup-dump-perf-harness.rs`。

## 验收结果（2026-11-15）

### 已修复并验证

| 项 | 根因与修改 | 证据 | 状态 |
|---|---|---|---|
| D7 跨库转储删源表 | MySQL 整库归档的 `DROP/INSERT` 带源库名，而 `SHOW CREATE TABLE` 的 `CREATE` 不带；还原到 B 时先删 A 表。整库归档改为目标相对对象，单表导出保持显式限定。入口：[transfer.rs](../../src-tauri/src/transfer.rs)、[transfer_export_cursor.rs](../../src-tauri/src/transfer_export_cursor.rs)。 | 本机 MySQL 8.0.44 实库：源 A 转储→B 还原，A 仍 4 行、B 值与 YEAR 一致；协议 wire 测试断言整库 INSERT 不带源库名。 | ✅ |
| YEAR 阻断转储 | YEAR 误走 SQLx 有符号整数兼容路径，SQLx 的 `i64` 兼容集合不含 YEAR，失败后提前返回被当作不可解码。改为经 `u64` 无损解码。入口：[values.rs](../../src-tauri/src/values.rs)。 | 协议 wire 测试 1901/2026/2155/NULL；MySQL 实库跨库还原包含同组值。 | ✅ |
| D10 同构类型膨胀 | 映射只看源类型文本与目标方言，同引擎也统一放大。新增 source-aware 映射：仅精确同一 MySQL 系列 ID 时保留经验证的类型；异构映射不变。入口：[sync.rs](../../src-tauri/src/sync.rs)。 | 186 个同步过滤测试通过；MySQL 实库建表为 `int`、`decimal(12,2)`、`datetime`，值一致。 | ✅ |
| D5 树库列表不刷新 | 连接节点折叠后未失效缓存，重新展开沿用旧 children。折叠时失效并清空连接子节点，异步代次防止迟到响应覆盖。入口：[App.ts](../../src/core/App.ts)。 | 78 个前端回归通过，包含折叠再展开、库列表变化、重复展开与迟到响应。 | ✅ |
| D9 localhost/::1 | SQLx `PoolTimedOut` 掩盖真实拒绝/认证差异。只增加本机连接错误分类与明确指引，不自动换地址、不重试认证失败、不改 TLS。入口：[sqlx_runtime.rs](../../src-tauri/src/sqlx_runtime.rs)。 | 3/3 Rust 测试：真实 ERR 包只尝试 1 次、拒绝端口不误报认证失败、TLS/未知错误不被改写。 | ✅（未完成自动回退，见下） |

### 未修复 / 已知缺口

| 项 | 当前事实 | 后续 |
|---|---|---|
| D11 `CREATE DATABASE/USE` SQL 文件 | 通用 SQL 文件路径仍拒绝改变数据库范围；受控管理脚本模式未完成端到端授权/UI 接入，临时实现已回滚，没有死代码或放宽保护。 | 🚧 需单独立项：完整预扫描 → 精确库名清单 → 用户确认 → 授权绑定 → 仅放行匹配声明。 |
| D9 `::1` 自动回退 | 未实现静默地址切换；仅提示明确选择 `127.0.0.1` / `::1`。认证失败不自动重试。 | 若产品要自动回退，必须先证明只发生在建连阶段且不改变认证/TLS 语义。 |
| D17/D19/P5-R/D13/D14/D8 | 缺少本轮对应的 TiDB / SQL Server / 达梦 / PostgreSQL / 还原进度现场环境，未复验。 | ⏳ 不在本轮宣称通过或修复。 |
| P8-R 非清空输入 | 本轮未做专项复测。 | ⏳ |

### 全驱动能力矩阵

- [生成器](../../tools/tests/round4-driver-matrix.mjs)、[14 项矩阵测试](../../tools/tests/round4-driver-matrix.test.mjs) 通过；产物 `artifacts/round4/driver-matrix.json`。
- 覆盖 101 个目录 ID + 2 个通用绑定，206 条实际路由、3502 个操作单元；运行证据全部为 `not-run`，不把源码分支或模拟夹具写成真实通过。
- 明确记录：JDBC 实际走 `Db::Http`，不继承原生/ODBC 的导入、还原、同步能力；通用 ODBC 的未识别 kind 会被 planned 门禁拒绝；关系型导入/同步只对 SQLite/MySQL/PostgreSQL/SQL Server/ClickHouse/ODBC 分支开放。性能机会指向共享导出序列化路径（167 条路由未实测），但只有 MySQL/SQLite 本轮有实测数据。

### 性能 A/B（本机、release）

| 场景 | 旧路径 | 新路径 | 结果 | 默认 |
|---|---:|---:|---|---|
| MySQL CSV 导入 1,000 行 | 中位 0.212667s | 中位 0.039004s | **5.452×**；5 轮交错、每值全量比对 | 严格门控自动启用 |
| MySQL CSV 导入 100,000 行 | 22.822644s | 1.183901s | **19.277×**；5 轮 | 同上 |
| MySQL CSV 导入 1,000,000 行 | 346.563731s | 12.915087s | **26.834×**；3 轮 | 同上 |
| MySQL JSON 导出 100,000 行 | 0.746216s | 0.548657s | **1.360×**；5 轮，输出字节一致 | 共享序列化路径 |
| SQLite JSON 导出 100,000 行 | 0.571091s | 0.242082s | **2.359×**；5 轮，输出字节一致 | 共享序列化路径 |

MySQL 导入门控仅适用于精确 `mysql` 8.0、InnoDB、已有表且无 prefix DDL、无 trigger/FK/default/generated、显式完整普通列、严格 SQL mode、足够 packet 与可验证全局元数据权限；不满足时继续逐行。整批仍在一个事务中，失败不拆批重放。

- 证据：[1k](../../artifacts/round4/import/mysql-release-1000-final.log#L4-L14)、[100k](../../artifacts/round4/import/mysql-release-100000-final.log#L4-L15)、[1M](../../artifacts/round4/import/mysql-release-1000000-final.log#L5-L8)、[门控实库](../../artifacts/round4/import/mysql-release-live-gate-final.log#L3-L6)、[MySQL JSON](../../artifacts/round4/json/mysql-json-100k.log#L663-L675)、[SQLite JSON](../../artifacts/round4/json/sqlite-json-100k.log#L660-L672)。
- MySQL 全进程峰值 working set：1k 23,351,296 B、100k 25,456,640 B、1M 25,604,096 B（采样上限，不是单任务硬限）。
- 未做其他数据库真实性能：TiDB/MariaDB/SQL Server/PostgreSQL/ClickHouse/ODBC/JDBC/HTTP/Native 无可用隔离实例或本轮未运行；不把协议相同当成性能等价。H2 也未执行性能测试。

### 最终回归

- `cargo check --lib --offline` 通过。
- 完整 Rust 库：**1340 passed / 0 failed / 62 ignored**（[日志](../../artifacts/round4/rust-lib-final.log)）。
- 前端 92/92 通过（[含驱动矩阵](../../tools/tests/round4-driver-matrix.test.mjs)）；`npm run build` 通过；文档链接检查通过。
- 工作树中另有其他并行任务的 ER / 更新对话框改动，未由本轮修改或回退，也不计入本轮通过范围。
