# NavKit 产品状态（与当前代码一致）

> **本文档是 `docs/completed/` 的唯一结果文档。** 它描述当前工作树代码的实际能力、验收状态与真实边界，
> 不是过程记录。任何一条能力的代码入口都必须可打开核对。
> 更新规则见 `.agents/skills/docs-lifecycle/SKILL.md`。
>
> 最后核对：2026-11-12｜代码基线：工作树（`53e79e8` + 未提交修改；包含 IPC 与例程备份修复）

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
| 导入导出：CSV/JSON/SQL 向导 | `src/dialogs/ImportExportDialog.ts`、`src-tauri/src/transfer.rs` | ⏳ | 导入冲突策略已有初版；混合失败与目标 CAS 实测未做 |
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

### 4.4 已接入的数据源

| 类型 | 列表 |
|---|---|
| 内置驱动（完整支持） | SQLite、MySQL / MariaDB、PostgreSQL |
| MySQL 协议族 | TiDB、OceanBase、PolarDB、TDSQL、Doris / StarRocks |
| PostgreSQL 协议族 | openGauss、KingbaseES、HighGo |
| 原生 HTTP 接口 | ClickHouse |
| 系统 ODBC 驱动 | 达梦 DM、Hive |
| JDBC / H2 | Java 11+ sidecar，严格匹配原包与厂商类 |

目录共 101 个条目（34 个 ODBC）。其余 90+ 类型仅提供图标与协议探测，**未做厂商实库认证**，
按「明确拒绝或协议模拟」处理；**目录条目数不等于已认证数据源数**。

## 五、实现归档（原 25 项 PL 任务）

原 `plan/` 的 25 项任务书已全部开发落地，结论并入本表后删除原件。状态按 §二 图例：

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

## 六、已知限制汇总

1. **平台**：只在 Windows 11 x64 验证过；无 macOS / Linux 结论。
2. **交付形态**：绿色版目录 + 单 exe，无 MSI/NSIS 安装包。
3. **只读保护**：非 SQLite 的任意只读 SQL 采用保守拒绝策略。
4. **取消与超时**：Java 侧无独立控制线程；PG/MySQL 有定向控制但失联与 quiescence 未完成。
5. **未知写**：默认不删除、不降级、不重放未知写操作；丢回执时冻结同一 execution ID。
6. **实库覆盖**：MySQL 8.0 / PostgreSQL 16 / SQLite 有真实服务器结论；其余厂商无。
7. **测试口径**：库全量串行单次已全绿；但仍存在 37 项条件 ignored（需 Java/H2、厂商包或实库），
   它们不是通过。厂商实库矩阵目标（`datasource_matrix`、`*_live_matrix`、`sim_*_catalogue`、
   `live_native_brokers`）需显式环境变量，本轮未运行。
8. **增量同步**：仅认证 SQLite 双端原生有界快照/typed 事务；视图无稳定业务键，配对时显式改全量物化。
9. **ER 原生逆向**：MySQL/PG 只读快照有隔离测试，但厂商实库 URL 未提供，验收仍为 ⏳；在线非 SQLite DDL 始终拒绝。
10. **驱动来源**：GBase 8c 只有目录与手动配置路径；候选 JAR 未核验指纹/类、未发布自有镜像，不构成自动下载。
11. **继续实施**：§五 标 🚧 的条目仍有代码缺口。

## 七、文档来源合并记录

本文档合并了以下过程文档的结论，原件已按生命周期规则删除：

| 原件 | 合并内容 |
|---|---|
| NavKit-完整任务书.md | 产品事实、界面版式基准约束 |
| 验收报告.md | 早期全量验收结论 |
| 14项修复验收报告.md | 14 项缺陷修复结论 |
| 全功能真实测试报告-20261107.md | 2026-11-07 动态测试结论 |
| 差异清单.md | 已失效（对照截图从仓库移除），未合并 |
| 界面规格表.md | 版式规格，转交 `docs/design/界面规格表.md` 常驻 |
| plan/01–27 任务书（25 份） | 并入 §五 实现归档 |
| plan/IMPLEMENTATION-STATUS.md | 并入 §三 验证状态与 §五 边界 |
| docs/验收证据/2026-11-11 两份续推报告 | 并入 §四 对象变更与 ER 能力行 |
| active/release-test-20261110（计划、问题清单、修复方案、交付报告、证据） | 并入 §三 验证状态、§三 实测修正表与 §六 限制 |
| active/尚未实现功能推进记录-20261111.md | 并入 §三 本轮实测、§四 对象变更/ER 能力行与 §五 PL-11/PL-19/PL-23 边界 |
| active/驱动下载来源政策与待办.md | 并入 §四 驱动来源政策行、§五 与 §六 第 10 条 |
| active/ER原生逆向下一切片-20261111.md、planned/ER原生逆向下一切片-20261111.md | 并入 §四 ER 能力行、§五 PL-23 与 §六 第 9 条 |
| active/当前轮次收尾与验证-20261111.md | 并入 §三 本轮全量结果与 §三 实测修正表 |
| 2026-11-12 IPC 与例程备份修复（直接结果归档） | §三 增补真实根因、错误边界、回归与未通过项，不新增已完成过程文档 |
