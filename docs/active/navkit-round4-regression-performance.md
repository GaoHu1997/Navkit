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

## 交付物

- 独立第四轮问题修复与验证报告。
- 全驱动/实际路由能力矩阵及性能对照表。
- 仅保留被报告按行引用的证据日志；完成后按 docs 生命周期合入结果文档并删除过程计划。
