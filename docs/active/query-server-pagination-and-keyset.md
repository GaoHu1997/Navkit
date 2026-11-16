# 查询页服务器分页 + 导出重查全量 + 表数据深分页（分支 feature/query-server-pagination）

基线 `3b67b61`，独立工作树 `D:\github\navkit-query-pagination`。

## 一、目标与方案

用户明确的三件事：

1. **查询 SQL 分页改成服务器端分页**：每页只执行当前页 SQL，不再先读全量到缓存；
2. **导出时重新查询全量**：只有用户点导出才付全量代价，且仅对“可安全重跑”的 SQL；
3. **表数据深分页优化**：连续翻页用 Keyset，任意跳页保留 OFFSET。

核心权衡（已与用户确认）：查询页不再依赖“执行一次 + 完整缓存”，因此
查询结果的复制、筛选、编辑、导出语义要重新界定——默认只对当前页生效，
全量导出通过“重新执行原始 SQL”获得，并明确提示“导出会重新执行查询、结果可能与页面不同”。

## 二、非目标

- 不改写为 AST round-trip 后执行非分页语义的语句；不改 `DROP`/`TRUNCATE` 等写语句。
- 不覆盖 SQL Server / ODBC / JDBC 的服务器分页（先回退到原路径）。
- 不承诺无唯一排序时的跨页一致性；深分页 Keyset 仅在有唯一键时启用。
- 不扩大现有 512 MiB / 2 GiB 缓存配额；不关闭缓存私有权限保护。

## 三、已完成（本分支，均已编译通过 + 测试通过）

### 3.1 后端分页规划与安全判定（`query_pagination.rs`，20 单元测试）

- 引入 `sqlparser = "0.62"`（思路借鉴 dbx Apache-2.0，非逐行复制）。
- `plan_query_page(kind, sql, limit, offset) -> PagePlan`：单条 SELECT/WITH 生成当前页 SQL；
  拒绝 SELECT INTO、可执行注释/优化器提示、`LIMIT ... BY`；已有顶层 LIMIT 用子查询包装；
  无法解析或不支持的协议 `Unsupported` 回退。覆盖 SQLite/MySQL/PostgreSQL/ClickHouse。
- `rerunnable_read(kind, sql)`：导出“重新查询全量”前的可重跑判定（单条只读 SELECT/WITH）。
- `keyset_predicate(column, ascending, value, numeric)`：表数据 Keyset 单列唯一键谓词，
  类型感知、单引号转义防注入，NULL/非数字配数字列返回 None 回退 OFFSET。
- 验证：`cargo test --lib query_pagination::` → **20 passed**。

### 3.2 后端命令（编译通过）

- `plan_query_pagination(connection_id, sql, limit, offset)`：只生成当前页 SQL，不发 SQL。
- `export_query_requery(opts, sql, owner_id, namespace)` + `export_query_requery_service`：
  校验可重跑 → 重新执行原始 SQL → 从本次完整缓存导出；超配额时经 `ensure_complete` 明确拒绝，
  不静默漏行。3 个隔离 SQLite 集成测试通过（安全 SELECT 全量导出、写语句拒绝且无文件、多语句拒绝）。

### 3.3 表数据 Keyset 接入 `page_table`（3 集成测试）

- `PageRequest.after` 锚点 + `PageResult.next_after`；`schema_page::keyset_for_page` 在
  “单列唯一主键 + offset==0 + 有效锚点”时生成 Keyset 谓词，否则回退 OFFSET。
- 验证：`keyset_tests::` 3 项通过——锚点续页 SQL 无 OFFSET 且返回正确行与 next_after；
  无锚点回退 OFFSET；复合排序回退 OFFSET。

### 3.4 前端接入（构建 + 128 项 UI 测试通过）

- `api.planQueryPagination` / `api.exportQueryRequery` 封装。
- 查询页：`execute` 先规划当前页 SQL 执行并记录原始 SQL；翻页走服务器重查
  （`loadServerResultPage` / `executeServerPage`），分页器显示“服务器分页·第 N 页”，禁用跳页/末页。
- 导出：服务器分页模式下 `exportResultSnapshot` 改走 `export_query_requery`（会重新查询，结果可能与页面不同）。
- 表数据页：`PageRequest.after` 锚点，仅“下一页”按钮携带上一页 `next_after`，加载后清除；跳页/上一页/末页回退 OFFSET。
- 验证：`npm run build`（tsc + vite）通过；`query-result-host`/`execution-controller`/`query-execution`/`data-grid-filter-sort` **128 项通过**。

### 3.5 已看页缓存（回翻不查库）

- 查询页翻过的页存在客户端 `serverPageCache`（`Map<offset, MultiResult>`，上限 8 页，超限淘汰最旧）。
- 命中缓存直接显示并提示“已看页缓存，未访问数据库”；重新执行、清空结果、切换连接时整体作废。
- 验证：`npm run build` 通过；前端 UI 子集 128 项通过。

### 3.6 查询页 Keyset（锚点续页，仅唯一键）

- `analyze_keyset`：结构判定（单表 SELECT、无 JOIN/聚合/多列排序/已有 LIMIT、投影为纯列引用、单列 ORDER BY）。
- `plan_keyset_page`：把锚点谓词**并入原 WHERE**（非外层子查询），让数据库用唯一键索引定位。
- `plan_query_keyset` 命令：结构 + 元数据双重判定，只有排序列 = 单列唯一主键才 `supported`；
  `plan_query_keyset_page` 生成锚点 SQL（只生成不执行）。
- 前端：仅“下一页”用锚点，规划被拒一次后置 `serverPageAnchorDisabled` 永久回退 OFFSET；跳页/上一页仍用 OFFSET。
- 验证：`query_pagination::` **25 项**单元通过（新增 5 项形状与谓词合并）；真实 MySQL 端到端验证锚点 SQL 从 700 定位到 701 且无 OFFSET。

## 四、待做（真实环境验收，⏳）

- 真实 PostgreSQL 与真实桌面 E2E 尚未运行（MySQL 已做服务层端到端，SQLite 已做集成）。
- 现有前端仅子集 UI 回归；完整 `test:ui` 与桌面 E2E 尚未运行，不称已全绿。

## 五、验收标准

- 单语句只读查询：首屏与全量行数无关；翻页仅执行当前页。
- 用户写了 `LIMIT n` 的查询：分页范围不超过 n；导出为其全部结果（≤ n）。
- 不可重跑 SQL：导出不得静默全量重跑；明确提示并仅导出已加载页。
- 深分页：有唯一键且连续下一页时数据库不扫描前缀（抓 SQL 证据）；无键/跳页回退 OFFSET 且如实说明。
- 所有改动附带单元/集成测试；前端构建与相关 UI 回归通过。真实 SQLite 集成 + **真实 MySQL 端到端**（服务器分页执行、用户 LIMIT 包装、Keyset 深层 `id>500` 定位、next_after=600）均已通过；PostgreSQL 与桌面 E2E 仍为 ⏳。

## 六、风险

- 前端查询页改造触及 `App.ts` 核心执行与结果面板，回归风险最高，需单独小步验证。
- 移除“完整缓存”会改变复制/筛选/编辑范围，必须向用户明确新语义。
- Keyset 复合键、升降序、NULL、排序规则、并发修改需逐项处理，不能机械套用。
