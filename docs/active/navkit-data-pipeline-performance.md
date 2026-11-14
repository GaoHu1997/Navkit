# NavKit 完整性能优化方案：开源实现与 Navicat 官方行为对照

## 1. 状态、范围与结论

- **状态：active，已按用户授权开工。** 原始研究依据保留；当前实施以真实入口、回归测试和前后对照为准，未经验证的快路径不默认开启。
- 研究日期：2026-11-13。NavKit 核验基线为当前工作树，HEAD `5d01dae124dbcd618bbfb0f0528dbfae42a5a424`，但存在大量未提交及并行改动；以下行号是本轮读取时的位置，不代表该 commit 的干净版本。
- 外部依据：已核验 GitHub/Gitee 的固定 commit 原始文件，实际阅读实现与失败分支；Navicat 只引用 17 Windows 官方手册的公开行为，不推测闭源算法。没有用搜索摘要、项目名气或 README 宣称替代源码；未运行上游基准。
- 本次扩展按用户指定的 dbswitch、SmartSQL、dbx 仓库研究，不以同名项目替代。`dbs` 未提供可确定身份的地址，列为待补证，不计入证据或影响本版结论。
- **研究阶段不承诺倍数收益；实现阶段已有隔离 SQLite 回归与 A/B。** 当前实测仅适用于记录的工作树、数据库、数据与事务语义，不外推到其他协议。初版磁盘差异比较已测得负收益，正在通过有界内存和批量溢写纠正；完整范围尚未完成。
- 范围：数据同步、数据导入、数据导出、大数据量生成、数据库备份/还原、运行 SQL 文件、SQL 转储，以及这些实际入口的端到端验证。
- 非目标：重写所有驱动、开放任意 SQL 并行执行、默认改变事务/错误策略、实现通用断点续传、用降低持久性或删校验换吞吐。实现仅改本方案涉及代码及测试，不回退或覆盖用户已有其他改动；未通过验证的能力不得标为完成。

**推荐顺序不是“先加线程”：先移除全量物化与逐行往返 → 在原事务边界内做受控批写 → 后端流式生成 → 完成快照/协议能力验证 → 最后才考虑并发。** 已有的流式、有界队列和正确性措施保留。

## 2. 开源源码与官方公开行为：证据索引

### 2.1 已读取的项目与版本

| 项目 | 本轮固定版本 | 为什么选它 |
|---|---|---|
| [DBeaver](https://github.com/dbeaver/dbeaver/tree/cc7e36f3ec5e78a88d76d42bae51be7c5d7147f9) | `cc7e36f3ec5e78a88d76d42bae51be7c5d7147f9` | 与 NavKit 相近的异构数据库传输工具；看批量、提交及驱动能力如何分离 |
| [pgloader](https://github.com/dimitri/pgloader/tree/3e06d1b9e1f2eba0d14fe49cb65e8eb570fc27d5) | v3.6.9，`3e06d1b9e1f2eba0d14fe49cb65e8eb570fc27d5` | 看 reader/queue/writer、COPY、转换及坏行恢复的成本和语义 |
| [mydumper / myloader](https://github.com/mydumper/mydumper/tree/8b47d633cd97b0e09700ae2a275fdfc66d8f6c4b) | `8b47d633cd97b0e09700ae2a275fdfc66d8f6c4b` | 看快照协调、分片调节、按字节组 SQL 和事务断线处理 |
| [PostgreSQL](https://github.com/postgres/postgres/tree/3d6a828938a5fa0444275d3d2f67b64ec3199eb7) | REL_18_0，`3d6a828938a5fa0444275d3d2f67b64ec3199eb7` | 直接看 pg_dump / psql 的读取、快照、COPY 协议和 SQL 扫描 |
| [sysbench](https://github.com/akopytov/sysbench/tree/3ceba0b1e115f8c50d1d045a4574d8ed643bd497) | `3ceba0b1e115f8c50d1d045a4574d8ed643bd497` | 看大数据准备阶段逐行生成、批包写入，不在前端积累全部数据 |
| [dbx](https://github.com/t8y2/dbx/tree/62e3e9e4ba38c4a2b4164bb6e263c4fc84e245d0) | `62e3e9e4ba38c4a2b4164bb6e263c4fc84e245d0` | 与 NavKit 同类桌面数据库工具；验证分块生成、SQL 组包、有界队列和阻塞隔离 |
| [SmartSQL（指定 Gitee 项目）](https://gitee.com/dotnetchina/SmartSQL/tree/c354096c04c618fe1f05f0a5350e5d7583e51178) | `c354096c04c618fe1f05f0a5350e5d7583e51178` | 数据库文档/元数据/查询桌面工具，**不是同名 SmartSql ORM**；看本地 prepared 复用、模板和后台导出，不当作七链路吞吐标杆 |
| [dbswitch（指定 Gitee 分支）](https://gitee.com/cbynd/dbswitch/tree/555cb0b8c6f32f4ba6d63fa5ba48bdbc2f49e0ce) | `555cb0b8c6f32f4ba6d63fa5ba48bdbc2f49e0ce`，POM 1.6.10 | 用户指定的 fork，核验实际同步/批写路径，不用上游新版本代替 |
| [Navicat 17 Windows 手册](https://www.navicat.com/manual/online_manual/en/navicat_17/win_manual/data_migration.html) | 2026-11-13 读取的版本 17 官方手册 | **闭源行为证据**：extended INSERT、最大语句、事务与执行选项；不提供内部队列、快照或并行实现的证明 |

### 2.2 具体实现与不能照搬的地方

**E1 — DBeaver：批写大小、事务提交、native bulk 是不同层。**

- [传输设置：59–70](https://github.com/dbeaver/dbeaver/blob/cc7e36f3ec5e78a88d76d42bae51be7c5d7147f9/plugins/org.jkiss.dbeaver.data.transfer/src/org/jkiss/dbeaver/tools/transfer/database/DatabaseConsumerSettings.java#L59-L70) 设置 `commitAfterRows=10000`、`multiRowInsertBatch=500`、`useBulkLoad=false`；这不代表默认启用了多行 INSERT。
- [批次/提交判定：395–434](https://github.com/dbeaver/dbeaver/blob/cc7e36f3ec5e78a88d76d42bae51be7c5d7147f9/plugins/org.jkiss.dbeaver.data.transfer/src/org/jkiss/dbeaver/tools/transfer/database/DatabaseTransferConsumer.java#L395-L434) 是逐行加入 batch，然后按设置执行/提交。不能把 500 行语句批与 10000 行提交间隔混成一个参数。
- [JDBC 能力门控：380–405](https://github.com/dbeaver/dbeaver/blob/cc7e36f3ec5e78a88d76d42bae51be7c5d7147f9/plugins/org.jkiss.dbeaver.model.jdbc/src/org/jkiss/dbeaver/model/impl/jdbc/struct/JDBCTable.java#L380-L405) 只有配置开启且具备 `GROUP_ROWS` 能力时采用多行实现；[bulk 分支：284–310](https://github.com/dbeaver/dbeaver/blob/cc7e36f3ec5e78a88d76d42bae51be7c5d7147f9/plugins/org.jkiss.dbeaver.data.transfer/src/org/jkiss/dbeaver/tools/transfer/database/DatabaseTransferConsumer.java#L284-L310) 检查实际 loader 能力，而不是按数据库名字一律开启。
- **借鉴：** 拆分“读取块/写入批/提交范围”，能力不满足时执行前选择普通路径。**不照搬：** 10000 行提交会改变 NavKit 当前全文件事务；本层未提供我们需要的硬字节预算，JDBC batch 也不等于 SQLx 所有驱动都能使用。

**E2 — pgloader：有界流水线减少等待，COPY 减少逐行协议往返。**

- [reader/writer 与队列：112–145](https://github.com/dimitri/pgloader/blob/3e06d1b9e1f2eba0d14fe49cb65e8eb570fc27d5/src/load/copy-data.lisp#L112-L145) 使用固定容量队列；是否并发读取取决于数据源能力，不是所有源都分片并读。
- [COPY 模式选择：41–69](https://github.com/dimitri/pgloader/blob/3e06d1b9e1f2eba0d14fe49cb65e8eb570fc27d5/src/pg-copy/copy-from-queue.lisp#L41-L69) 区分遇错停止的流式 COPY 与为坏行恢复保批的路径；[参数准备：87–102](https://github.com/dimitri/pgloader/blob/3e06d1b9e1f2eba0d14fe49cb65e8eb570fc27d5/src/pg-copy/copy-from-queue.lisp#L87-L102) 预先准备转换配置。
- [批上限：31–44](https://github.com/dimitri/pgloader/blob/3e06d1b9e1f2eba0d14fe49cb65e8eb570fc27d5/src/pg-copy/copy-batch.lisp#L31-L44) 行数/配置的字节条件决定满批，但字节配置可为 `nil`，**不能据此声称默认严格有界内存**。
- [恢复路径：137–171](https://github.com/dimitri/pgloader/blob/3e06d1b9e1f2eba0d14fe49cb65e8eb570fc27d5/src/pg-copy/copy-retry-batch.lisp#L137-L171) 先回滚失败批，再以局部事务恢复、提交。**不能复制到要求整体原子的任务，也不能作为断线未知结果自动重放的依据。**
- [项目自身的成本说明：63–77、109–113](https://github.com/dimitri/pgloader/blob/3e06d1b9e1f2eba0d14fe49cb65e8eb570fc27d5/docs/batches.rst#L63-L113) 明确直接 COPY 合法数据可比经转换流水线更快，默认数值也不是普适最优值。这是本方案不增加无必要转换层、不照抄批大小的依据。

**E3 — mydumper：并行建立在协调快照之上；分片和 SQL 包受成本约束。**

- [一致快照初始化：507–569](https://github.com/mydumper/mydumper/blob/8b47d633cd97b0e09700ae2a275fdfc66d8f6c4b/src/mydumper/mydumper_working_thread.c#L507-L569) 设置隔离级别、启动快照，并在相关模式校验 worker 的 GTID 位点；不一致会重试/失败。多个连接分别 `BEGIN` 不等于相同快照。
- [整数分片调节：828–869](https://github.com/mydumper/mydumper/blob/8b47d633cd97b0e09700ae2a275fdfc66d8f6c4b/src/mydumper/mydumper_integer_chunks.c#L828-L869) 按上次行数、耗时和目标查询时长调节 step，再限制 min/max。仅借鉴“以观测成本调节”，不宣称它适用于无稳定键或任意数据源。
- [SQL 输出组包：1128–1173](https://github.com/mydumper/mydumper/blob/8b47d633cd97b0e09700ae2a275fdfc66d8f6c4b/src/mydumper/mydumper_write.c#L1128-L1173) 逐行读取、格式化，接近 `statement_size` 刷写并复用 INSERT 前缀；超大单行有专门分支。因此该阈值不是所有情况的硬上限，NavKit 必须另外校验单行及接收端限制。
- [myloader 活跃事务断线：319–358](https://github.com/mydumper/mydumper/blob/8b47d633cd97b0e09700ae2a275fdfc66d8f6c4b/src/myloader/myloader_restore.c#L319-L358) 在 `!is_schema && commit_count > 1` 的相关分支停止，明确重连重放一条会丢失事务。**其他分支仍有 retry，不能扩大成全项目 exactly-once 保证，更不能照搬 retry。**

**E4 — PostgreSQL：COPY 是专用协议；多连接必须共享导出的快照。**

- [pg_dump 快照：1478–1552](https://github.com/postgres/postgres/blob/3d6a828938a5fa0444275d3d2f67b64ec3199eb7/src/bin/pg_dump/pg_dump.c#L1478-L1552) 在只读事务里工作，leader 可导出 snapshot，worker 设置同一 snapshot；[956–957](https://github.com/postgres/postgres/blob/3d6a828938a5fa0444275d3d2f67b64ec3199eb7/src/bin/pg_dump/pg_dump.c#L956-L957) 并行 dump 限定 directory 格式。**不能把这些前提从“多线程备份”中删掉。**
- [数据写出：2350–2386](https://github.com/postgres/postgres/blob/3d6a828938a5fa0444275d3d2f67b64ec3199eb7/src/bin/pg_dump/pg_dump.c#L2350-L2386) `COPY TO stdout → PQgetCopyData → WriteData → PQfreemem`；[2436–2459](https://github.com/postgres/postgres/blob/3d6a828938a5fa0444275d3d2f67b64ec3199eb7/src/bin/pg_dump/pg_dump.c#L2436-L2459) 校验结束状态、排空结果。不是把全表读进数组。
- [INSERT 模式：2490–2550](https://github.com/postgres/postgres/blob/3d6a828938a5fa0444275d3d2f67b64ec3199eb7/src/bin/pg_dump/pg_dump.c#L2490-L2550) 使用游标、每次 `FETCH 100`，并准备可复用的 INSERT 前缀；这与 COPY 是不同输出路径，不能把 COPY 的性能推给 SQL INSERT 格式。
- [psql 扫描执行：378–450](https://github.com/postgres/postgres/blob/3d6a828938a5fa0444275d3d2f67b64ec3199eb7/src/bin/psql/mainloop.c#L378-L450) 状态 lexer 识别完整 SQL 再 `SendQuery`、复用缓冲；不是任意按分号切分或重排 SQL。
- [psql COPY：508–572](https://github.com/postgres/postgres/blob/3d6a828938a5fa0444275d3d2f67b64ec3199eb7/src/bin/psql/copy.c#L508-L572) 使用 8192 字节缓冲分段发送；[681–731](https://github.com/postgres/postgres/blob/3d6a828938a5fa0444275d3d2f67b64ec3199eb7/src/bin/psql/copy.c#L681-L731) 发送结束信号并处理服务端结果。普通 SQL reader 并不能自动支持 `COPY FROM STDIN` 数据体。

**E5 — sysbench：边生成边批写；数据规模不等于内存规模。**

- [数据准备：192–247](https://github.com/akopytov/sysbench/blob/3ceba0b1e115f8c50d1d045a4574d8ed643bd497/src/lua/oltp_common.lua#L192-L247) 循环生成一行，交给 bulk insert，结束后完成写入；不先生成整个二维数组。[线程分工：84–94](https://github.com/akopytov/sysbench/blob/3ceba0b1e115f8c50d1d045a4574d8ed643bd497/src/lua/oltp_common.lua#L84-L94) 是按表分配，不是同一张表任意并发分片。
- [批包实现：946–1066](https://github.com/akopytov/sysbench/blob/3ceba0b1e115f8c50d1d045a4574d8ed643bd497/src/db_driver.c#L946-L1066) 根据包含 SQL 前缀等开销的字节数 flush；[常量：42–46](https://github.com/akopytov/sysbench/blob/3ceba0b1e115f8c50d1d045a4574d8ed643bd497/src/db_driver.c#L42-L46) 包大小为 512 KiB。`ROWS_BEFORE_COMMIT=1000` 只在 `needs_commit` 分支累计刷包行数后判断，不是通用“每 1000 行提交”。
- [随机状态：214–239](https://github.com/akopytov/sysbench/blob/3ceba0b1e115f8c50d1d045a4574d8ed643bd497/src/sb_rand.c#L214-L239) 是线程初始化 RNG，**没有为 NavKit 提供与分块/调度无关的逐行确定性保证**。
- **不照搬：** 自建基准表后建二级索引的流程、事务能力假设、超大行边界处理。NavKit 面向已有表，不能擅自删索引、延迟约束或关闭触发器。

**E6 — dbx：与本项目接近的分块/调度案例，但软阈值不是硬内存界。**

- [生成执行循环：161–245](https://github.com/t8y2/dbx/blob/62e3e9e4ba38c4a2b4164bb6e263c4fc84e245d0/apps/desktop/src/composables/useDataGenerateSession.ts#L161-L245) 在前端按块生成、组装 SQL、按字节拆分并等待执行，再让出事件循环；不是一次构造全部目标行。`useTransaction` 随每次调用传入，不能声称整个生成任务共用同一事务；按行生成后才做字节拆分，也不能证明生成瞬间的硬内存限制。NavKit 借其分块思想，但采用后端 recipe，避免长期让 WebView 承担生成与 SQL 转义。
- [JSON 两遍读取：2479–2551](https://github.com/t8y2/dbx/blob/62e3e9e4ba38c4a2b4164bb6e263c4fc84e245d0/crates/dbx-core/src/data/table_import.rs#L2479-L2551) 先扫描列集合/计数，后重新打开逐行发送到容量 2 的通道。证明可不持有全部行，但额外一遍 I/O、列集合/单行预算、两遍之间源变化必须单独处理；不是所有格式都应无条件预扫两遍。
- [SQL 批次策略：2391–2432](https://github.com/t8y2/dbx/blob/62e3e9e4ba38c4a2b4164bb6e263c4fc84e245d0/crates/dbx-core/src/data/sql_file_import.rs#L2391-L2432) 在手动事务模式保留逐句执行边界；其他合并/失败逐条恢复不能套用 NavKit 的 Unknown/禁止重放约束。它的批字节目标允许额外一个大语句，不能把配置的 8 MiB 当实际 RSS 上限。
- [SQL assembler：1979–2118](https://github.com/t8y2/dbx/blob/62e3e9e4ba38c4a2b4164bb6e263c4fc84e245d0/crates/dbx-core/src/data/database_export.rs#L1979-L2118) 复用前缀，区分 identity 与方言，并按行数/语句字节 flush；SQL Server 字节计算使用 UTF-16。超大首行仍能进入语句，且此函数返回本次 `Vec<String>`，不证明整个调用链严格有界。NavKit 要增加单行硬限，并直接向现有 sink 刷写。
- [导出调度：16–19、52–87](https://github.com/t8y2/dbx/blob/62e3e9e4ba38c4a2b4164bb6e263c4fc84e245d0/crates/dbx-core/src/data/export_runtime.rs#L16-L87) 把带同步格式化/写文件的工作放入 blocking worker；有模拟阻塞不饿死异步 worker 的测试代码，**本轮仅读，未运行**。借鉴隔离机制，不借无限扩张线程；须限制并发、资源所有权和取消后的 join。

**E7 — Navicat：参考“显式选项和边界”，不把官方说明当源码。**

- [数据传输高级选项](https://www.navicat.com/manual/online_manual/en/navicat_17/win_manual/data_transfer_1.html) 明确提供 extended INSERT、maximum statement size、use transaction、use single transaction、源/目标锁等选项，且选项随数据库类型变化。最大语句值针对 extended INSERT，单行超出时使用普通 INSERT，**不是普通单行无限安全**。这支持 NavKit 将组包、事务、快照/锁分别建模，而不是混为“高速开关”。
- [导入模式](https://www.navicat.com/manual/online_manual/en/navicat_17/win_manual/import_wizard_7.html) 公开 extended INSERT/最大语句/一次多查询，也列有忽略外键和遇错继续。只借前者的受控策略选择；后两者会改变正确性，不成为本方案默认提速手段。官方未给内部实现和可比基准，不推导它一定比 NavKit 快多少。
- [执行 SQL 文件](https://www.navicat.com/manual/online_manual/en/navicat_17/win_manual/dump_execute_sql.html) 暴露一次执行多条、关闭自动提交、遇错继续等选项；这是产品行为证据，**不能证明任意 SQL 合并仍语义等价或失败后可安全重放**。NavKit 的普通模式保持顺序与原始事务，特殊快模式必须先建立另一份明确契约。
- [生成配置](https://www.navicat.com/manual/online_manual/en/navicat_17/win_manual/data_generate_2.html) 允许每表指定数据量、列生成规则及唯一/NULL 等配置，但明确说明生成顺序不自动考虑依赖，需手动调整。可借配方与总量独立表达；不能据此声称自动 FK 拓扑、多线程、百万行性能或常量内存。
- [同步映射](https://www.navicat.com/manual/online_manual/en/navicat_17/win_manual/data_sync_2.html) 与[差异查看](https://www.navicat.com/manual/online_manual/en/navicat_17/win_manual/data_sync_3.html) 分离键/字段映射、比较预览和部署。NavKit 的分页差异报告参考这一契约；官方页面没有披露 hash、排序归并或 CDC 算法，不能从 UI 推断实现。
- [备份入口](https://www.navicat.com/manual/online_manual/en/navicat_17/win_manual/backup_restore.html) 明确区分内建备份、Oracle Data Pump、SQL Server Backup、MongoDump；借鉴“逻辑 SQL 转储”和“原生备份”的能力区分，不把一种文件包装成所有数据库原生备份。

**E8 — SmartSQL：只借已证明的局部机制，不借同名 ORM 的光环。**

- [本地 SQLite 事务批：1445–1485](https://gitee.com/dotnetchina/SmartSQL/blob/c354096c04c618fe1f05f0a5350e5d7583e51178/SmartSQL/SmartSQL.Framework/SQLite.cs#L1445-L1485) 是事务里 `foreach Insert`，不是多值 SQL；[连接内 prepared cache：1723–1774](https://gitee.com/dotnetchina/SmartSQL/blob/c354096c04c618fe1f05f0a5350e5d7583e51178/SmartSQL/SmartSQL.Framework/SQLite.cs#L1723-L1774) 和[绑定/Step/Reset：3181–3223](https://gitee.com/dotnetchina/SmartSQL/blob/c354096c04c618fe1f05f0a5350e5d7583e51178/SmartSQL/SmartSQL.Framework/SQLite.cs#L3181-L3223) 证明本地存储的语句复用。不能扩大为远程导入已批量化；缓存也未在此层证明 DDL 版本失效和 LRU，NavKit 不照抄字典。
- [模板：31–85](https://gitee.com/dotnetchina/SmartSQL/blob/c354096c04c618fe1f05f0a5350e5d7583e51178/SmartSQL/SmartSQL.DocUtils/RazorEngine/RazorTpl.cs#L31-L85) 将模板内容哈希作为编译键，但 FileInfo 版本仍每次读文件，包装层未证明旧键淘汰。这是模板编译缓存，不是元数据/数据库计划缓存；本计划只将规则编译缓存列为 profiling 后候选，不无端引入第二套模板引擎。
- [后台文档导出：985–1053](https://gitee.com/dotnetchina/SmartSQL/blob/c354096c04c618fe1f05f0a5350e5d7583e51178/SmartSQL/SmartSQL/Views/ExportDoc.xaml.cs#L985-L1053) 分离 DTO、生成器和后台工作，但仍会累计对象/列集合，进度回调还有 `Thread.Sleep(50)`。NavKit 借职责拆分/后台隔离，不借全量 DTO 或人为睡眠；后台运行不等于更少内存、更高吞吐。
- [查询执行：480–523](https://gitee.com/dotnetchina/SmartSQL/blob/c354096c04c618fe1f05f0a5350e5d7583e51178/SmartSQL/SmartSQL/UserControl/SqlQuery/UcSqlQueryMain.xaml.cs#L480-L523) 取解析列表最后一条 SQL 执行；[备注导入：92–161](https://gitee.com/dotnetchina/SmartSQL/blob/c354096c04c618fe1f05f0a5350e5d7583e51178/SmartSQL/SmartSQL/Views/ImportMark.xaml.cs#L92-L161) 是 XML 备注的逐项写入，不是数据行导入。二者都不能作为 NavKit 任意脚本执行、原子还原或大数据生成的范例。

**E9 — dbswitch：驱动化流式读取、可选 COPY，以及有严格前提的有序归并。**

- [读取连接与游标：66–84](https://gitee.com/cbynd/dbswitch/blob/555cb0b8c6f32f4ba6d63fa5ba48bdbc2f49e0ce/dbswitch-dbcommon/src/main/java/com/gitee/dbswitch/dbcommon/database/AbstractDatabaseOperator.java#L66-L84) 显式设置非自动提交、forward-only/read-only 和 fetch size；不同驱动仍须验证实际是否流式。不是 keyset 分页，也不代表所有 JDBC/SQLx 驱动行为相同。
- [行/字节缓存：301–339](https://gitee.com/cbynd/dbswitch/blob/555cb0b8c6f32f4ba6d63fa5ba48bdbc2f49e0ce/dbswitch-data/src/main/java/com/gitee/dbswitch/data/handler/MigrationHandler.java#L301-L339) 先加入行再估计对象大小，达到行数或 128 MiB 软阈值同步写出；[增量缓存：399–450](https://gitee.com/cbynd/dbswitch/blob/555cb0b8c6f32f4ba6d63fa5ba48bdbc2f49e0ce/dbswitch-data/src/main/java/com/gitee/dbswitch/data/handler/MigrationHandler.java#L399-L450) 合计增删改缓存后刷写。不是全局字节背压，超大行、编码副本与多表并发可放大内存，不能把 128 MiB 抄成 NavKit 默认值。
- [PG 二进制 COPY：52–142](https://gitee.com/cbynd/dbswitch/blob/555cb0b8c6f32f4ba6d63fa5ba48bdbc2f49e0ce/dbswitch-pgwriter/src/main/java/com/gitee/dbswitch/pgwriter/row/SimpleRowWriter.java#L52-L142) 使用 `COPY FROM STDIN BINARY`、PGCopyOutputStream 与逐行编码，支持将 COPY 独立为 writer 能力。已追调用链会为上游批新建连接，并有移除 NUL 的处理；本方案**不复制静默改值，也不把资源 close 当成确认 COPY 取消/事务回滚**。
- [有序归并：199–217、282–338](https://gitee.com/cbynd/dbswitch/blob/555cb0b8c6f32f4ba6d63fa5ba48bdbc2f49e0ce/dbswitch-dbchange/src/main/java/com/gitee/dbswitch/dbchange/ChangeCalculatorService.java#L199-L338) 对两端按主键排序，保留当前行、比较后推进游标，输出新增/删除/变更。归并扫描步数为 O(N+M)，但排序、每列比较、驱动缓冲、写批成本另计；这是全量比较计算增量，**不是日志 CDC**。
- [比较器风险：439–480](https://gitee.com/cbynd/dbswitch/blob/555cb0b8c6f32f4ba6d63fa5ba48bdbc2f49e0ce/dbswitch-dbchange/src/main/java/com/gitee/dbswitch/dbchange/ChangeCalculatorService.java#L439-L480) 字符串按 Java 比较，相关数值走 double，部分转换失败返回相等。数据库排序与客户端比较是否同序未获证明。NavKit 必须精确保留 decimal、NULL、二进制和 collation；不关闭类型检查、不用“异常=相等”，不删除现有 typed spill/CAS/MAC/读回。
- [批写事务：179–202](https://gitee.com/cbynd/dbswitch/blob/555cb0b8c6f32f4ba6d63fa5ba48bdbc2f49e0ce/dbswitch-dbsynch/src/main/java/com/gitee/dbswitch/dbsynch/AbstractDatabaseSynchronize.java#L179-L202) 按操作执行 batchUpdate 后提交，update/delete 也各有事务；不等于整个同步或增删改共同原子。不能把这种提交范围、仅按 PK 覆盖更新代替 NavKit 的原子承诺与 CAS。
- [表任务调度：109–169、204–213](https://gitee.com/cbynd/dbswitch/blob/555cb0b8c6f32f4ba6d63fa5ba48bdbc2f49e0ce/dbswitch-data/src/main/java/com/gitee/dbswitch/data/service/MigrationService.java#L109-L213) 为表提交 CompletableFuture，相关调用没有显式 executor；捕获等待线程中断不证明数据库 worker 已停止。NavKit 只借独立表任务分解，不借无全局预算的铺开并发，取消必须落实到连接与资源回收。

## 3. NavKit 当前真实入口：哪些已做，哪些确有问题

| 链路 | 本轮核验事实与代码触点 | 判断 |
|---|---|---|
| CSV/JSON 导入 | [ImportMapper / 事务执行：1393–1450](<../../src-tauri/src/transfer.rs#L1393-L1450>) 每行拼 INSERT，事务在循环外开启、末尾提交；[generic：1460–1475](<../../src-tauri/src/transfer.rs#L1460-L1475>) 是不同的非事务路径 | 确有逐行协议调用；不是“每行都提交”，不要诊断错层 |
| 交互式导出 | [ExportSink：1753–1815](<../../src-tauri/src/transfer.rs#L1753-L1815>) 已缓冲、逐行写、最终同步；[游标入口](<../../src-tauri/src/transfer_export_cursor.rs#L18-L45>) 已分驱动 | 不重写成第二套“流式导出”；测序列化、协议和磁盘分别占多少 |
| 遗留导出 | [run_export：1169–1204](<../../src-tauri/src/commands.rs#L1169-L1204>) 仍先取全部行再构造完整文本；[定时入口](<../../src/core/App.ts#L5659>) 使用旧 API | 优先统一实际定时路径，但必须保留定时任务的权限、范围和文件目的地语义 |
| 数据生成 | [生成器：96–127](<../../src/core/dataGenerator.ts#L96-L127>) 全量 `Array.from`、全量 PK Set；[预览：302–305](<../../src/dialogs/DataGeneratorDialog.ts#L302-L305>) 先全生成再展示 20 行；[授权/克隆](<../../src-tauri/src/batch_execution.rs#L89-L116>) 处理完整 rows | 不能调大 500 行上限冒充大数据支持；前端、IPC、后端均需避免全量物化 |
| 全量同步 / 差异预览 | [全量分页与批写：732–848](<../../src-tauri/src/sync.rs#L732-L848>) 已有 2000 行分页、256 行/约 1 MiB 多值批；有 PK 用 keyset、无 PK 用 OFFSET；[diff：646–700](<../../src-tauri/src/sync.rs#L646-L700>) 两侧全部加载并建 HashMap | 不重复实现已有批写；diff 内存、无键翻页成本和源快照才是研究重点 |
| SQLite 增量同步 | [同步执行：155–215](<../../src-tauri/src/sync_stream.rs#L155-L215>) 已源事务/游标/spill、目标事务、结构/数据校验和读回；[lookup：123–127](<../../src-tauri/src/sync_stream.rs#L123-L127>) 使用业务键比较 | 不是旧 diff 的双 HashMap；先 EXPLAIN 和分阶段计时，不删 CAS/MAC/读回 |
| 备份 | [dump_database：352–488](<../../src-tauri/src/transfer.rs#L352-L488>) 逐表写 DDL/数据并 stage publish；[append_dump_data：86–93](<../../src-tauri/src/transfer_export_cursor.rs#L86-L93>) 每表重新建立导出生命周期 | 已流式、文件发布原子；**不是整库共享一致快照** |
| 还原 | [restore_sql_file_detailed：929–950](<../../src-tauri/src/transfer.rs#L929-L950>) `read_to_string → script_statements → RestoreStep Vec`；[解析：985–995](<../../src-tauri/src/transfer.rs#L985-L995>) 包含 MySQL DELIMITER/备份词法规则 | 全文件与语句/报告集合均可能线性增长；不能简单换普通 SQL worker 而丢失预检语义 |
| 运行 SQL 文件 | [实际 worker：334–362](<../../src-tauri/src/batch_execution.rs#L334-L362>) 已 blocking scanner、容量 2 队列、同连接逐语句执行、回执/未知状态处理 | 不再重复“改流式”；没有应用自动事务包装不等于文件自身不能含事务 |
| SQL 转储 | [ExportSink SQL 分支](<../../src-tauri/src/transfer.rs#L1786-L1802>) 已流式但按行输出；备份数据也由导出游标写入 | 可探索有界多行 INSERT，主要预期收益在文件体积、解析与后续恢复，不保证导出本身一定变快 |

MySQL 当前单次导出在[专用只读 RR 会话](<../../src-tauri/src/transfer_mysql_export.rs#L61-L112>)内读取，但每表各自开始/结束；PG/SQLite 的当前导出游标也没有跨整库共享事务。即使连接池复用同一 socket，也不意味着复用同一快照。DDL 元数据读取同样要纳入一致性设计。

## 4. 七条链路的具体优化设计

以下均为 **NavKit 适配方案，尚未实现/验证**，不是声称上游已有完全相同功能。

### 4.1 数据导入：先减少往返，保持原事务

**依据：E1/E2 的批写与 COPY，E6 的两遍输入代价，E7 的显式批/事务选项，E8 的连接内 prepared 复用，E9 的驱动化 reader/writer。**

输入预扫按任务需要选择：CSV 已知映射不无条件增加整文件预扫；JSON 若必须发现列并集，先固定源、限制列集合和单行预算，再有界预扫，复用安全 spool/固定句柄而不是两遍按可变路径重开。预扫保证、额外 I/O 与首批延迟明确计入结果。转换失败、无法表达的字符或类型必须报错，不借 dbswitch 的改值/置 NULL 换吞吐。

1. 保留现有流式输入；表/列元数据、转换规则在每任务/合法生命周期内准备一次。行映射输出带类型的值，不默认先转 SQL 字面量再解析。
2. 给现有 writer 增加“参数化逐行”和“受控参数化多行”两种策略。SQLite/MySQL/PG 先逐项验证；其他驱动不因为名称类似而默认具备同样能力。SQL 文件仍走 SQL 执行语义，不能直接套 CSV/JSON 的 typed-row writer。
3. 批边界同时受行数、编码后字节、单值/单行、绑定参数总数、驱动/服务端报文限制约束。完整 accounting 包含前缀、占位符、转义/编码膨胀等；满批 flush 后仍需再次检查当前行是否可接受。超限明示失败，不截断/漏行。
4. **写入批不等于提交批。** 当前全文件原子路径继续在同一事务内执行多个批，末尾一次确认 COMMIT；不能把 DBeaver 的提交间隔照抄进来。非事务路径明确报告已确认前缀。
5. 多行化可能改变语句级触发器、语句级时间值、约束检查/失败粒度、默认表达式等行为。第一批仅放行已证明语义等价的表/模式；其余保持参数化逐行，用户明确选择新语义才另立策略。
6. 能力不支持可在**发送前**选普通路径。发生写入错误后不得自动拆批逐行重放；只有已确认整个相关事务回滚、没有不可回滚副作用且授权策略允许，才可设计独立诊断/重试。提交/执行结果未知一律停止。
7. PG COPY 作为第二阶段独立能力：明确支持的类型、NULL/DEFAULT/identity、约束/触发器、错误位置及 COPY 协议退出/排空。没有证明收益前，不给小文件绕一圈 staging/COPY/再搬运。MySQL native bulk 后续单独调查，**本轮没有足够实现证据批准默认 LOAD DATA**。

**验收：** 同一文件同一事务策略，减少真实数据库执行调用数；数据/副作用一致，坏行和 COMMIT 断线不误报成功；小文件不因额外初始化明显变慢。

### 4.2 数据导出：补真正的全量入口，不推倒已有流式

**依据：E2/E4 的流式输出，E6 的同步工作隔离，E9 的驱动化游标；隔离设计见 §4.8。**

1. 让遗留定时导出复用现有游标、sink 和安全文件发布能力。只替换数据搬运实现，不绕过现有任务授权、过滤/字段范围、路径校验和取消。
2. 在现有通路记录 acquire/read/convert/encode/write/finalize 的时间、读取字节、输出字节、缓冲峰值。逐项判断瓶颈在驱动缓冲、行类型转换、转义还是磁盘，避免先增加队列/复制反而更慢。
3. 已同步顺序流可保持不加队列；只有实测读/写等待可以重叠时，才加最多少量块的有界流水线。每个队列必须有**字节上限**，不仅是 item 个数。
4. PG 专用 COPY 导出仅供明确的原生格式/协议选项，不假装 CSV/JSON/兼容 INSERT 文件都可以无损替换。默认 CSV/JSON/SQL 输出继续通过 round-trip 验证；驱动 fetch size 不作为内存有界的充分证明。
5. 低风险先修：[CSV 预览](<../../src-tauri/src/transfer.rs#L1139-L1144>) 达到记录限制即停止，保留编码/引号/跨行 CSV 语义；[备份列表](<../../src-tauri/src/commands.rs#L1571-L1581>) 只读有字节上限的头部，不能为 12 行扫描整个大文件，也不能被一条巨长首行耗尽内存。

**验收：** 百万行定时任务不再全表物化；CSV/JSON/SQL 输出等价且失败不发布完整文件名；小数据首字节时间不回退。

### 4.3 大数据量生成：独立 recipe 任务，支持百万/千万行

**依据：E5 的逐行生成/批包、E6 的分块后等待执行、E7 的配方/总量配置、E1/E2/E9 的 writer 分层。确定性、授权和事务状态机是 NavKit 新设计，不归功于这些项目。**

与 dbx 的区别是生成与 SQL 编码留在后端，而不是长期由 WebView 组 SQL；与 Navicat 手册相同，必须明确生成依赖策略。首版先支持单表/用户确认的表顺序，多表含 FK 时预检引用数据与顺序；循环依赖或无法保证的数据明确拒绝/要求专门计划，不自动关闭 FK。不能把“唯一值选项”当成与库存及并发写入永不冲突的保证。

- 保留当前小任务模式及限制；[当前 shape 限制](<../../src-tauri/src/data_generator.rs#L10-L40>)为 500 行、128 列、单值 16 KiB、值字节和 2 MiB，后者不等于完整 IPC 序列化大小。**不直接抬上限。**
- 新增独立“大数据生成”入口。前端只提交 recipe：规则版本、总行数、列规则、seed、冻结时间、目标/模式、schema fingerprint、事务策略。后端返回至多 20 行样本和规范化 plan hash；执行复用同一个获授权计划，不传百万 rows。新任务继承[受控批任务的授权与目标约束](<../../src-tauri/src/batch_execution.rs#L107-L151>)：先限定已支持的原生 SQL 通道、owner/目标绑定、只读检查、单次 grant/CAS、真实取消和排空；不切回旧 insert_batch 绕过控制。
- 应用生成值定义为 `F(ruleVersion, seed, columnId, globalRowIndex, frozenTime)`，不依赖线程/块边界；预览和执行使用同一引擎。当前 UUID/时间行为不能直接沿用来宣称可复现。DB default/identity/trigger 最终值标为“数据库执行时决定”，不能承诺与样本完全一致。
- 后端 iterator → 有界值块 → 已验证 writer。先单生产者/单 writer，SQLite 保持单 writer；只有 profiling 证明生成 CPU 是瓶颈才并行生成不同全局行号范围。内存与块预算相关，不与总行数成正比。
- 起测候选：500/1000/2000 行，512 KiB/1 MiB 编码预算，队列 1/2 块；均是**实验点，不是默认最优配置**，还要服从列数、参数、协议及单行限制。
- 主键/唯一值规则先验证类型容量，区分本任务内部、并行分片之间、与库中存量/并发写入之间的不重叠保证；按数据库实际相等性检查，不能把字符串不同当作唯一，须考虑 collation、CHAR 尾空格、数值转换等。不能用 `MAX(id)+1` 当作并发预留，也不截断后宣称唯一。无法可靠预留的存量表只承诺确定性与约束保留，不承诺必然插入成功。复合唯一、FK、CHECK、触发器仍由真实数据库验证；不使用 IGNORE/REPLACE，冲突后停止尾部、确认回滚当前段，保留已提交段；禁止自动换 seed/跳行掩盖错误。
- 事务提供明确契约：①原子模式最后一次提交，提前提示大事务锁/WAL/undo/回滚代价；②大任务可显式选择分段提交，取消只回滚当前未提交段，已提交前缀保留。不可把②偷偷作为①的性能优化。每段在同一固定物理会话内先取得对应事务/对象锁，再读取真实元数据并与授权 fingerprint 比较，之后才写入；不能在事务外检查一次便跨段使用。fingerprint 覆盖所依赖的列类型、default/generated、唯一约束/排序规则及影响批写等价性的 trigger 条件；漂移即停止，如实保留已提交前缀。
- 进度分开记录 generated / sent / acknowledged / committed；确认 ROLLBACK 后才能说回滚成功。COMMIT 回执丢失为 Unknown，不自动重放。确定性生成不是 exactly-once；持久断点恢复需要数据库内原子批标记等协议，**不纳入首版**。
- 单独修正[生成任务标题：211–218](<../../src/dialogs/DataGeneratorDialog.ts#L211-L218>)中 `${submitted.rows}` 的数组字符串化，普通模式使用 `rows.length`，recipe 模式用显式 count，避免数据串入标题和大字符串分配。

**验收：** 100 万/1000 万行窄表、宽表、Unicode、真实约束；改变块大小/允许的 worker 数后指定样本 hash 一致；内存进入稳定平台；核对最终行数、值域、唯一性、失败/取消后的持久化边界。不能只测“生成数组”的时间而不计写入与最终提交。

### 4.4 数据同步：先索引与内存，再有限并发

**依据：E9 的有序归并/驱动游标，E7 的预览与部署分离，E2 的有界流水线，E3 的分片成本控制，E4 的快照前提。**

归并只作为**差异预览候选实现**：两端均有稳定唯一键、数据库 ORDER BY 与客户端 comparator 同序、类型/NULL 语义确定、快照契约可接受时，以当前两行推进做 O(N+M) 扫描；数据库排序可能额外耗时/用临时盘，写批也占内存，必须计入。若跨库 collation/数值表示不匹配，退回保类型 spill/索引化比较；不得按显示字符串比较，不把 DECIMAL 转 double，更不能比较异常视为相等。复合键要验证字典序与实际索引顺序。执行仍使用现有授权、CAS/MAC 和读回，不能将预览生成的旧差异直接覆盖目标，也不因此开放当前不支持的非 SQLite 增量或 delete-missing。

1. 保留全量同步已有 keyset/多行写；不把既有 2000/256 行参数重命名就当优化。计量源 read、目标等待、DML、commit，定位源分页与目标事务跨网络等待。
2. 差异预览是明确全量热点：先限制 UI 返回的差异样本和总览数据，再以保类型的外部 spill/索引化比较替代两侧全量 HashMap。只有稳定唯一键、排序规则和 NULL/类型等价性都确定时才采用排序归并；无键不能擅自去重或把显示文本当行身份。
3. 差异预览和执行一致性分开声明；未固定源快照的 keyset 并不保证看到同一时点。避免为了加速把“可变源预览”冒充可重放执行计划。
4. SQLite 增量先对实际 `COLLATE BINARY IS ?` 业务键查询做 EXPLAIN/计数，确认是否走索引；不直接在用户库新建永久索引。复用准备语句/列映射；优化本地 scratch 的事务批次可实验，但保留 seal、MAC、CAS、类型编码、立即/最终读回及碰撞检查。
5. 目标事务缩短需要先确定同步契约：可选择先在稳定源快照下写私有 spool、封存后再开目标事务，但它增加磁盘 I/O 和等待，只能在实测锁占用确实改善时用，不能无条件双写中间盘。
6. 分片/并发仅适用于具有稳定分区、已证明源快照和顺序/外键语义的路径。没有这些能力继续单 reader/single writer；SQLite 目标不通过多 writer 抢锁提速。源无键 OFFSET 的替代需由真实驱动游标能力验证，不能用不稳定伪 keyset。

**验收：** 无漏行/重行、NULL/二进制/不同排序规则不误判；diff 大输入不线性占内存；目标变更/源并发修改/取消仍触发原校验。完整计入 spill、预检和最终验证，不只对比 DML 部分。

### 4.5 数据备份与还原：一致性是并行的进入条件

**依据：E3/E4 的快照与协议、E7 的逻辑转储/原生备份分离、E6 的输出工作隔离。**

产品层先明确当前“备份”是逻辑 DDL/数据转储及其已支持对象范围，不宣称等价于全实例原生备份、物理备份或 PITR。原生工具委托属于后续独立能力：另审版本、安装依赖、授权/凭据、输出校验、进程取消与恢复兼容性，不能因为 Navicat 存在该入口就直接新增外部命令。本轮优先修当前逻辑备份的一致性、还原内存和可测吞吐。

1. 现有 backup 的流式写和 stage 原子 publish 保留；**文件原子发布 ≠ 数据库整库快照**。先建立任务级 read session，让数据与必要元数据处在明确的生命周期，并处理并发 DDL。
2. PG 先单只读快照覆盖任务；未来并行需 export/import snapshot 和对象锁/依赖策略。MySQL 按真实版本、存储引擎、权限和 DDL 协调能力设计；不同连接自行 RR 不够，TiDB 等兼容协议不能照抄 MySQL SQL。SQLite 先单只读事务贯穿所需读取，并监控长读事务对 WAL/写入的影响。
3. 快照能力不满足则拒绝“一致备份”承诺或清楚标为较弱模式，不能默默降级后仍显示一致。正确性修复可能增成本，须独立披露，不能混进纯性能 A/B 宣称提速。
4. 后续并行先选独立表/受控片，worker 数从 1/2/4 实验。队列按字节限制，观测服务器 CPU、I/O、锁、WAL/undo、业务延迟；不默认“CPU 有多少核就开多少连接”。需要分块目录格式才另立 manifest/校验/兼容策略，单纯多行 INSERT 不叫新格式版本。
5. 还原去除全文件和无界结果集合，但先建立精确兼容清单。当前[原生还原执行器：803–895](<../../src-tauri/src/transfer.rs#L803-L895>)是在同一专用物理连接上，连续 DML 按 1000 条/8 MiB 软阈值分批提交，遇非 DML 先提交；不是整文件事务，先前已提交批不会因后续失败回滚。其他通道有逐条非事务执行，不能静默缩窄支持范围。流式化不改变这些边界，也不把软阈值误说成单句硬上限。
6. 现状是先完整读取/解码、切句，再建立执行连接：MySQL 的备份 reader 会处理 DELIMITER 并严格检查其词法规则，但 PG/SQLite 仍用[简易 split](<../../src-tauri/src/db.rs#L950-L1008>)，没有完整 dollar-quote/trigger-body 状态或全部 EOF 错误检查。不能把现状夸成所有方言严格语法预检。迁移方案为“固定源 → 有界完整预扫描/私有 spool → 顺序执行 → 有界审计/分页报告”，保留当前能提前发现的错误零发送；补齐方言解析另立正确性测试，不用边读边写偷偷扩大已执行前缀风险。磁盘上限/满盘/cleanup 纳入验收。
7. [既有 SQL 输入准备器：193–246](<../../src-tauri/src/transfer_sql_reader.rs#L193-L246>)已有固定句柄、源摘要、帧 spool/MAC 可借鉴，但普通入口的守卫白名单、协议和整文件事务与 restore 不同；MySQL 例程也可能要求文本协议。只复用已验证的底层能力，不直接替换为普通 SQL worker 或 prepared-script executor。备份守卫跳过、后续 skipped、语句编号/截断等报告接口需兼容；当前批回滚前记入的 `ok/succeeded` 不能当成已持久化行/语句数，应补充已回执、已提交、已回滚、跳过、未知状态，不固化这个缺陷。
8. 现有[转储头部会话设置](<../../src-tauri/src/transfer.rs#L399-L410>)包含约束/复制角色相关指令；本方案不假装它们不存在，不扩大其范围或把它们当新增提速手段。记录实际语义，异常时清理/关闭专用会话，独立审查是否符合用户承诺；不能声称当前所有提前错误路径都已经确认关闭。

**验收：** 并发跨表更新下检验备份关系一致性；恢复到空库比较 schema、行值、约束及对象；磁盘满/取消不留下被误认完整的备份；任何不确定执行不重放。备份结束时间包括 flush/sync/publish，恢复时间包括最终提交与验证。

### 4.6 运行 SQL 文件：普通脚本保持顺序，快通路只处理可信结构

**依据：E4 的 psql lexer/专用 COPY，E3 的事务断线教训，E6 的手动事务逐句边界，以及 E7 对执行选项的明确区分。**

1. 当前真实 UI 链路已有有界扫描队列和逐句回执，不再立“流式 reader 重写”项目。对 lexer、SHA-256、脱敏模板、审计、数据库 execute、结果排空分别 profiling；保留审计含义，只在测到瓶颈后减少重复遍历/分配。
2. 任意脚本默认按原序、原事务/session 语义执行。不静默注入每 N 条 BEGIN/COMMIT，不合并含 DDL、变量、临时表、存储过程、显式事务的语句，也不分配到多连接并发。
3. 大 SQL 恢复的首选性能路径是**在可控生产端生成有界多行 INSERT**（下一节），而不是运行时对未知 SQL 做字符串猜测合并。这能减少解析/执行次数，同时保留普通脚本语义。
4. 未来若提供“纯数据、应用管理事务”选项，必须先完整分类预检、明确方言/隐式提交/副作用限制、重新授权，并准确区分原语句回执与提交状态；不支持时在开始前拒绝。不把此选项当默认优化。
5. 支持 PG `COPY FROM STDIN` 必须实现数据体/协议状态、错误/结束/排空，不是放大 parser 队列。编码、单语句上限、DELIMITER、取消和 Unknown 均保留。

**验收：** 大小 SQL、显式事务、失败前缀、混合 DDL/DML、过程体、COPY 数据体（若声明支持）、编码/注释引号均对照；执行/提交未知时停止尾部，不能出现重连后继续或自动重放。

### 4.7 SQL 转储：按字节组织多行 INSERT，兼容模式保留

**依据：E3 的 statement buffer、E6 的方言化 assembler、E7 的 extended INSERT/最大语句选项，E4 的分离 COPY/INSERT 路径。**

1. 在现有 sink 上增可选 SQL insert assembler：相同目标、相同列集合复用 INSERT 前缀，按行数和最终编码字节 flush；到表/列/模式边界必须刷写。不要先拼完整表字符串。
2. 同时服从目标 dialect、最大 SQL/包、恢复 parser 的单句上限。单行超限给明确错误或使用已验证的独立合法表示，不截断，不照抄上游“警告后照写”。NULL、二进制、时区、精度、转义和 generated/default 列全部 round-trip。
3. 多行 INSERT 会改变恢复时语句级触发器等语义，**兼容逐行模式保留且不默认替换**；只有目标条件已验证或用户显式选“批量恢复语义”才开启。普通 `.sql` 仍是 SQL，无需因为语句行数变化捏造新文件格式版本。
4. 主指标为“导出 + 恢复”全周期、文件体积和恢复调用数；若组包导致单纯导出更慢，应披露并让用户选择，而非以恢复收益掩盖导出回退。PG 原生 COPY/分块归档是另一能力档，分别测试、分别声明兼容性。

### 4.8 共用能力与资源预算：避免七条链路各自调参、互相抢资源

**依据：E1/E2/E6/E9 的分层与背压、E8 的局部复用；以下统一约束是 NavKit 的适配设计。** 不先建一套覆盖所有驱动的大框架。先在现有 reader、writer、sink 与任务生命周期提取最小公共接口，保留不同任务的事务和授权执行器。

| 层 | 统一约束 | 不应发生的负优化 |
|---|---|---|
| Reader | 明确驱动实际游标/抓取行为、固定源和快照能力；行/列/单值限制；按需决定一遍或预扫后执行 | 仅凭 fetchSize 宣称流式；为简单 CSV 强加全文件二次扫描；重新按路径打开导致 TOCTOU |
| Mapper / rule | 任务内预解析列映射、类型转换与规则；prepared 优先利用 SQLx 现有连接级能力，先 profile 命中率与 prepare 次数 | 重复造无限 prepared cache；使用陈旧 schema；转换失败悄悄置 NULL、剔除 NUL、decimal 转 double |
| Batch | flush 同时受行、最终编码字节、参数数、单行/值及服务端能力约束；满批后重验大行；独立于 COMMIT | 将 128 MiB 软缓存或队列容量 2 当硬内存保证；为省往返静默改变提交范围 |
| 调度 | 全局并发任务额度 + 每任务字节额度 + 活跃连接/worker 限额；排队前预留内存；取消先停止新准入，已占配额在对应缓冲实际 drop、worker join、连接排空/回收后释放，spool 配额在实际 cleanup 后释放 | 每表各开 128 MiB，再由无限任务/线程倍增；抢占交互查询全部连接；仅收到取消请求就提前释放并重复分配配额 |
| File sink | 直接流式写现有 stage；输出块、压缩（如有）与编码副本计预算；最终 flush/sync/publish 计时 | 为组 SQL 再积累整表 Vec/String；把 flush/fsync 移出计时；无限 blocking 线程 |
| 完成状态 | generated/scanned、sent、acknowledged、committed、rolled_back、skipped、unknown 分开；连接排空/回收由原生命周期负责 | 将 batch 返回或关闭句柄当持久化成功；取消后后台仍继续发 SQL；提交未知自动重放 |

**可核算的内存模型：** `M_client ≈ M_base + Σ(驱动缓冲 + 在构造/编码的块 + 排队块 + 在写块 + 最大语句/行 + 有界报告/缓存)`。至少分别计入 WebView 与 Rust 进程；客户端之外的服务端缓存、WAL/undo、临时排序与磁盘 spool 单列。配额在分配/入队前核验，未知驱动缓冲不能假装已受控；初期单 writer、短队列，候选预算通过实验决定，不把上游数值当默认答案。

**输出工作与交互隔离：** 当前[导出命令](<../../src-tauri/src/commands.rs#L1341-L1366>)直接 await，而[格式化/写文件](<../../src-tauri/src/transfer.rs#L1753-L1815>)含同步操作；另一些导出/数据库任务已有 blocking 路径，不能全局重复套线程。参考 dbx，先测“大导出同时执行轻量查询/取消”的命令延迟及 runtime 调度延迟，确认饥饿才把同步重段隔离到受限 worker，配合字节背压与完成 join；`spawn_blocking` 本身不会让已启动的工作自动取消。无等待重叠收益时仍用简单顺序实现。

**prepared / 元数据缓存边界：** SmartSQL 仅证明局部复用有实现，未证明 NavKit 需要新缓存。先采用任务内列映射与规则编译复用，测 prepare/转换开销；确需额外缓存时，限定连接代际、数据库/schema、完整 SQL、类型签名、必要结构 fingerprint 与容量/淘汰策略。新连接、结构漂移或能力改变应失效；不能用跨事务的缓存替代执行前真实元数据校验。

**驱动策略选择（均为待验证能力，不按品牌直接开启）：**

| 通道 | 起始安全路径 | 候选快路径 | 启用前必须证明 |
|---|---|---|---|
| SQLite | 同连接事务内参数化逐行、单 writer | 条件允许的多行 INSERT | 绑定上限、default/约束/trigger 等价性、锁等待与取消回滚 |
| MySQL / 兼容协议 | 现有专用会话/事务，参数化逐行 | 有界多行 INSERT、受控读分片 | 存储引擎、包/参数限制、SQL mode、版本、快照/DDL 与隐式提交行为；兼容协议不等于能力相同 |
| PostgreSQL | 同事务参数化逐行 | 多行 INSERT；独立 typed COPY writer；原生 COPY 导出 | 类型编码、NULL/default/identity、trigger/权限、COPY 错误退出/排空、同快照 |
| 其他通道 | 保留当前明确支持的普通路径及其事务声明 | 对应驱动经测试的 batch/native adapter | 实际 API/事务/取消能力；unsupported 显示原因，不回落到另一个数据库方言 |

**本次新增参考对方案的实质变化：** dbswitch 使同步增加“同序、同型、固定快照才可归并”的明确分支和全局预算；dbx 增加运行时饥饿实验、导出隔离与大行软阈值检查；Navicat 将批量、事务、错误模式和原生备份拆成独立能力选项；SmartSQL 将 prepared/规则复用列为受生命周期约束的局部优化，而非全局缓存重构。

## 5. 实施阶段与进入条件

| 阶段 | 进入条件 | 范围 / 代码触点 | 完成标准 |
|---|---|---|---|
| P0：建立当前真实基线 | 冻结本次待改工作树及配置；准备可销毁 SQLite/MySQL/PG 隔离库；禁止用旧报告数值代替 | [批任务](<../../src-tauri/src/batch_execution.rs>)、[传输](<../../src-tauri/src/transfer.rs>)、[同步](<../../src-tauri/src/sync.rs>)的阶段计量与实际 UI/API 驱动基准 | 记录真实入口、参数、事务策略、端到端耗时/内存/正确性；能复现失败和取消 |
| P1：低风险消除无界开销 | P0 固定；每项能独立对照 | [标题/预览](<../../src/dialogs/DataGeneratorDialog.ts>)、[CSV 预览/备份列表/遗留导出](<../../src-tauri/src/commands.rs>)及现有 sink；分离小补丁，不混重构 | 不再数组入标题、前缀读取有上限；遗留导出走有界数据路径且行为相同 |
| P2：原事务内批写与 SQL 输出选项 | 类型/约束/触发器与错误矩阵明确，驱动参数/报文限已探测 | [ImportMapper/writer/sink](<../../src-tauri/src/transfer.rs>)、[生成写入](<../../src-tauri/src/batch_execution.rs>) | 小/大数据、成功/失败均通过；逐行策略可在执行前选择，Unknown 不退回重跑 |
| P3：大数据生成与流式还原 | recipe 授权/确定性/提交契约确认；restore 原语义清单完成 | [生成计划](<../../src/core/dataGenerator.ts>)、[批任务](<../../src-tauri/src/batch_execution.rs>)、[还原](<../../src-tauri/src/transfer.rs>) | 百万/千万行生成内存平台；大文件还原保预检/守卫/错误定位，报告不无界 |
| P4：同步与一致备份 | 源快照、比较语义、磁盘预算、并发 DDL/锁策略通过正确性验证 | [sync_stream](<../../src-tauri/src/sync_stream.rs>)、[spill](<../../src-tauri/src/sync_stream_spill.rs>)、[备份游标](<../../src-tauri/src/transfer_export_cursor.rs>) | diff 不全量持有两侧；整库一致性声明有证据；不删除校验换性能 |
| P5：native bulk / 有限并发 | P2/P4 单 writer 稳定且 profiling 证明可获益；负载上限明确 | [MySQL 导出能力](<../../src-tauri/src/transfer_mysql_export.rs>)与驱动分支、PG 专用 COPY、任务调度 | 只为经验证的协议/模式启用，吞吐改善不以业务延迟、持久性或恢复语义为代价 |

本次扩展对应的落地检查点：P0 增加导出与交互并发延迟、真实驱动缓冲/prepare 计数；P1 仅在发现 runtime 饥饿时增加受限阻塞隔离；P2 统一字节/单行/参数预算及缓存生命周期；P3 同时检验后端生成与多表依赖限制，**大任务开放前须具备最小全局任务准入、交互连接保留和总在途字节预算**，即使每任务只有一个 writer 也不能省略；P4 增加 merge-diff 同序验证与保类型 fallback；P5 仅将已有全局配额扩展到任务内部多 worker，不能只给单个队列加上限。阶段按依赖推进，不把所有源证据合成一次大重构。

各阶段可拆小 PR，但**不能将数据库写入已发生后的错误“回退旧实现”当作回滚功能**。开关只控制下一任务/尚未发送的合法决策点。实现业务代码后计划再移到 active；本次研究完成不等于优化已落地，不更新产品能力为“已优化”。

## 6. 防负优化的实验与发布门槛

### 6.1 数据、环境与路线必须可复现

- 规模：1000 行小任务、10 万行中任务、100 万行大任务；生成额外 1000 万行。大 SQL/备份按字节补测，例如 1 GiB 及受环境磁盘预算允许的更大文件；不能只以行数表达长文本数据。
- 类型：窄数值、宽文本/Unicode、NULL、二进制、大字段、decimal、时区；普通表及 PK/复合唯一/FK/CHECK/行级与语句级触发器、default/generated 列。不能只测无约束的空表。
- 环境：记录 DB/驱动版本、服务端配置、持久性、连接方式、并发负载、磁盘和 RTT。1/20/80 ms RTT 可作网络模拟实验点，不代表产品默认网络。事务模式、输出格式和一致性级别相同才可直接 A/B。
- 路线：实际受控任务/UI 对应 API；完整经过授权、文件验证、解析、写入、提交/flush、校验与报告。禁止只 benchmark 已不被当前 UI 使用的旧函数。

### 6.2 测什么

- wall time、rows/s、MiB/s、首结果/首字节、最终提交与 fsync/publish 耗时。
- 前端与 Rust 进程分别取峰值 RSS、分配/复制量；队列峰值字节、活跃批数、spill 字节；服务端内存/WAL/undo/锁等待另算，不用客户端低内存掩盖服务端爆涨。
- execute/COPY/prepare 调用数、发送字节、重试次数、连接数、CPU、I/O；预览与 UI 事件是否拖慢主线程。大导出并行轻量查询/取消，采集足量样本的命令延迟分位数、runtime 调度延迟与 WebView 长任务；区分数据库负载、磁盘阻塞与前端卡顿，不只用总吞吐做“流畅”指标。
- 多任务压力：1/2/4 个任务与窄/宽/超大行组合，验证总在途字节、worker/连接数和交互连接保留量；队列满能背压而非继续积累。预扫型 JSON 测列集合爆炸、两遍间源修改；diff 测 decimal 极值、不同 collation、复合键与排序落盘；COPY 注入中途错误、结束失败、取消及回执丢失。
- 取消分两段：停止继续发送的响应时间，以及排空/回滚确认时间。数据库大事务回滚可能很慢，不能为了让“取消更快”提前假报成功或取消完成。
- 数据正确性：类型感知全量比对/分块校验、行数、关键约束、trigger 副作用、schema 及 round-trip；不能只数行或抽样就宣布恢复无损。

### 6.3 实验和合并规则

1. 小/中任务预热后至少 5 次 A/B 交错运行，报告中位数、分布/极差；大任务至少 3 次并记录资源曲线。每轮从等价数据库状态和同一源输入重建，重置身份序列及相关 session 设置，不能让前轮导入/生成/恢复的数据留给后轮；记录冷/热缓存政策，重建成本按任务目标另列。样本不足时不报有统计意义的 P95，不挑最快一轮。
2. 批参数、队列深度、worker 数分开调，每次只改一个主要变量。先测 1 worker，再 2/4；若 CPU/I/O/业务查询延迟变差则停止增加，不默认开启自适应复杂控制器。
3. **硬门槛：** 数据/顺序/约束/授权/持久性/Unknown 语义不回退；无漏行、重复、静默截断、错误成功态；失败不发布完整备份。任一失败不合并到默认路径，性能再高也不例外。
4. **性能门槛（本项目建议，非上游给出的数字）：** 对小任务 >5% 且超出重复实验噪声的回退必须定位并修复/保持旧路径；大任务收益必须可重复且覆盖最终提交/校验，不能只减少一次 execute 就宣称改善。5% 是候选发布容忍线，不是“保证不会慢”的证明。
5. 内存不要求零增长；要求增长可由明确 batch/queue/driver/spool budget 解释。数据扩大 10 倍后，稳态客户端 RSS 不应随总量近似线性增长。报告峰值与上限来源，包含单个大行/大语句。
6. 对新增强一致性、部分提交或新格式路径，单独报告语义和成本；不能与较弱旧模式比较后混称纯性能优化。默认仅启用已经证实等价且获益的能力，其余显式 opt-in。
7. 注入坏行、网络执行中断、COMMIT 回执丢失、ROLLBACK 失败、取消、磁盘满、结构变化和并发写入；核查已提交前缀/未知状态及恢复边界。未知状态不重试、不自动重连续尾。

## 7. 明确排除的“看似提速”

- 不关闭 WAL/fsync、扩大关闭约束/触发器范围、使用 IGNORE/REPLACE 吞错。
- 不把已有有界流式换成 `fetch_all`/整文件 String，也不以无限队列换瞬时吞吐。
- 不对任意 SQL 合并、并行、注入事务；不混淆 batch flush 与 commit。
- 不通过删 MAC/CAS/读回、抽样替代必须的校验、减少授权核验来美化性能数据。
- 不抄 DBeaver 的 10000 行提交、pgloader 的恢复策略、myloader 的所有 retry、sysbench 的建索引/事务假设；它们解决的问题和 NavKit 默认承诺不同。
- 不为了大数据生成把当前 500 行限制改成百万，不默认全局 PK Set，不把相同 seed 当成安全续传凭证。
- 不把所有表强塞 COPY/staging/多 worker。小数据常被初始化成本支配，单顺序流水线可能更好。
- 不抄 dbswitch 的 128 MiB 软阈值、DECIMAL→double、比较失败=相等、转换失败=NULL、无 CAS 覆盖更新和分操作提交；不把“全量归并 diff”包装成 CDC。
- 不抄 dbx 的前端长期组 SQL、失败后自动逐句重试或以软包阈值冒充 RSS 硬限；不抄 SmartSQL 的全量 DTO、无限字典缓存和人为 sleep；不将 Navicat 的关闭约束/遇错继续选项当默认优化。

**最终决策：先交付可测的小步优化，默认路径以等价性和无明显回退为准；大数据、native bulk 和并行分别提供明确契约。没有基准证据，就只叫候选方案，不叫已提速。**
