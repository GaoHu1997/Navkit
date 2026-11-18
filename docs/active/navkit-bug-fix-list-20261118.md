# NavKit 待修复缺陷清单（截至第四轮测试 2026-11-18）

> ⚠️ **本文已被 [缺陷决策清单（代码复核版）](navkit-bug-decision-list-20261118.md) 取代。**
> 复核发现本文 3 处结论有误：D-R4-2（Keyset）为误报、D-R4-4（字典文案）为误报、
> D-R4-3（网格总数）存疑；D17 与 P2-R 已在代码中修复。请以决策清单为准。

> 依据：第四轮测试报告（`navkit-round4-feature-perf-report.md`）+ 第三轮报告遗留 +
> `navkit-round4-regression-performance.md` 的已知缺口。按修复优先级排列；
> 「复验」= 修复后可用 `.tmp-p4/` 下对应运行器直接回归（无需重新搭环境）。

## P0 发布阻断（1.0.2 查询分页核心功能在 MySQL 不可用）

| # | 编号 | 问题 | 现象与证据 | 修复方向 / 代码触点 |
|---|---|---|---|---|
| 1 | **D-R4-1** | `count_query_page` 在 MySQL 类型解码失败 | 查询服务器分页执行后总数统计 100% 失败：`Rust type Option<String> (as VARCHAR) is not compatible with SQL type BIGINT`；总行数"统计中"，**页码跳转/末页按钮永久禁用**。1万→1亿全部复现（桌面端）。注：PRODUCT-STATE 记载"真实 MySQL 端到端 1 项通过"，需对齐两处差异 | 排查 count 查询的 sqlx 行解码映射（`src-tauri/src/query_pagination.rs` 的 count 分支）；补桌面 MySQL 全量级回归 |
| 2 | **D-R4-2** | Keyset 锚点链路未激活 | 查询与表数据的"下一页"实际均执行 `LIMIT/OFFSET`（SQL 日志证实 `LIMIT 200 OFFSET 10000`）；`serverPageAnchor`/`next_after` 始终空 | 排查 `plan_query_keyset` 单列唯一主键判定在 MySQL 的返回路径；与 #1 同链路（分页规划）一并修 |

## P1 高

| # | 编号 | 问题 | 现象与证据 | 修复方向 |
|---|---|---|---|---|
| 3 | **D-R4-5** | 亿级单文件导入不可用 | 9.53GB CSV：解析 111 分钟零插入，RSS 峰值 ~8GB 且 ∝ 文件大小；导出/转储同规模 RSS 有界、20-30 分钟完成 | 导入管线分块流式解析（解析与插入重叠）+ 内存上界（`transfer_import_batch.rs` / PreparedScript prepare 溢写策略） |
| 4 | **D-R4-3** | s1m 表数据网格总数异常 | 100 万行表"最后一页"被钳到 offset 500000（应用内总数 ≈50.1 万）；firstId 与 pageOffset 一次不一致 | 复查表数据 `countTotal`（`include_total:true` 的 page_table）路径；网格页脚 countText 增加断言 |
| 5 | **D11** | 运行 SQL 文件拒绝含 `CREATE DATABASE/USE` 的文件 | 连续三轮复现（BATCH_SQL_PARSE_FAILED："SQL 不能改变数据库/身份/授权范围"）；迁移核心场景 | 第四轮开发已列专项：预扫描 → 精确库名清单 → 用户确认 → 授权绑定 → 仅放行声明库（`batch_sql_reader.rs` / `execution_policy.rs` / `batch_execution.rs`） |
| 6 | **CANCEL-1** | 真实 MySQL 取消执行返回 Unknown | 独立实测两轮失败（`docs/active/mysql-cancel-unknown-20261115.md`）；取消后效果状态未知而非明确回滚/完成 | 按该文档诊断：取消路径的效果状态判定 |

## P2 中

| # | 编号 | 问题 | 现象与证据 | 修复方向 |
|---|---|---|---|---|
| 7 | **D17** | TiDB 转储/备份生成不支持的事务语法 | `READ ONLY` 事务 → TiDB 全部报 1064，转储/备份不可用（第二轮发现，第三/四轮未复验） | TiDB 方言分支（`transfer.rs` 转储事务包装） |
| 8 | **D19** | SQL Server 备份未实现 | "命令 sqlServerBackup：尚未实现或当前版本不支持" | 实现或显式禁用入口并提示 |
| 9 | **P5-R** | 达梦多语句批量执行 `Unsupported SQL type` | 第二轮起未修复；运行 SQL 文件通道同样失败（JDBC 路径） | JDBC sidecar 批量执行类型映射（达梦） |
| 10 | **D13** | PostgreSQL「仅结构」转储产出空壳文件 | 第二轮发现未修复 | 仅结构分支文件写入（`transfer.rs`） |
| 11 | **D14** | 同步向导回退重置连接 | 向导"上一步"后连接选择被重置（第二轮） | SyncDialog 步骤状态保持 |
| 12 | **D8** | 还原进度 UI 停滞、无完成提示 | 第二轮发现；还原实际完成但 UI 不更新 | restore 进度回调/UI（`transfer_restore_stream.rs`） |
| 13 | **D5** | 树缓存：`refreshTree()` 不刷新已展开节点子树 | 第四轮自动化实测仍在（需折叠/重开连接才刷新）；第三轮"部分改进"未闭环 | 折叠失效逻辑扩展到 refreshTree 路径（`src/core/App.ts`） |
| 14 | **D9** | localhost/::1 无自动回退 | 仅新增错误分类与指引；同步向导默认目标仍报 `root@'::1'` Access denied | 产品决策：是否做仅限建连阶段的 127.0.0.1 回退（需论证不改认证/TLS 语义） |

## P3 低 / 待复验

| # | 编号 | 问题 | 状态 |
|---|---|---|---|
| 15 | **D-R4-4** | 字典向导第 4 步校验文案修正后不清除（滞留显示，导出可成功） | 第四轮新发现 |
| 16 | **P8-R** | 密码/输入框非清空输入会拼接 | 第二轮复现；第三、四轮未专项复测 |
| 17 | **D3/D4** | 未选中行删除静默无效；表单视图删除不稳定 | 第二轮发现，低危交互 |
| 18 | **D6** | 无障碍树退化（DBus 错误，偶发自恢复） | 第二/三轮偶发 |
| 19 | **D12/D18** | 特殊类型网格显示 NULL 面：PG 8 类（timetz/interval/point/line/xml/inet/cidr/macaddr）、SQL Server TINYINT=255 | 第一/二轮发现，未修复 |
| 20 | **D7 复验** | 跨库转储删源表——开发侧已修复（wire 测试 + 实库验证），桌面端建议一次跨库还原端到端复验后闭环 | 第四轮未构造该场景 |

## 已闭环（无需再列修复计划）

D15 密码泄露、D16 查询页锁死、D1 POINT 显示 NULL、P7-R 字典文件名反斜杠、
P10 备份目录、P2-R 备份/导入 OOM（流式化；亿级导入呈新形态问题 → 见 #3）、P3 查询超时 30s、YEAR 转储阻断、D10 同构类型膨胀（开发侧已修）。

## 建议修复顺序

1. **#1 + #2**（同一链路：分页规划/计数）——解除 1.0.2 发布阻断
2. **#3** 导入流式化（亿级场景）+ **#4** 网格总数
3. **#5** D11 受控管理脚本（迁移场景，三轮未解）
4. **#6** 取消 Unknown（数据一致性相关）
5. P2 批次：D17/D19/P5-R/D13/D14/D8/D5/D9（其中 D17/D19/P5-R/D13/D14/D8 在 box 1.0.2 环境可一次性复验后再修）
6. P3 批次随手修

## 复验工具

- 功能矩阵：`.tmp-p4/r4-func-matrix.mjs --conn=mysql|sqlite`（12 项自动回归）
- 分页回归：`.tmp-p4/r4-perf-paging.mjs --tables=s10k,...`（含 count/keyset/深跳断言，可直接验证 #1/#2/#4）
- 工具链回归：`.tmp-p4/r4-perf-tools.mjs --tools=... --scales=...`
- 夹具：`.tmp-p4/gen-fixtures.mjs`（万→亿 CSV/SQL + 100列×2000万宽表）
