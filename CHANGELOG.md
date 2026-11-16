# NavKit 更新日志

本文件记录 NavKit 各版本的变更。发布说明（面向终端用户）见 GitHub Release 页面，
能力边界与验收状态见 [`docs/completed/PRODUCT-STATE.md`](<docs/completed/PRODUCT-STATE.md>)。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

---

## [1.0.1] — 2026-11-16

`1.0.0` 之后的第一个补丁版本，集中在 **ER 关系图重做**、**软件更新弹窗**、**表格设计器交互** 三个方向的改进，
并补齐 openGauss / GaussDB 兼容性修复与降级路径。

### 新增与改进

- **ER 关系图核心重做（移植 dbx 关系图能力）**：ELK layered（Sugiyama）布局替代自研最长路径分层，
  交叉最小化 + 节点平衡 + `NETWORK_SIMPLEX` 分层；`elkjs` 动态 `import` 按需加载（独立 chunk）不拖慢启动；
  新增布局方向选择（LR/TB/RL/BT），大图（>400 节点或 >1200 边）自动退回连通分量聚簇网格布局；布局结果持久化。
- **虚拟外键自动匹配**：无物理外键时按命名约定（`user_id` / `userId` / `order_uuid`）+ 类型签名推断关系，
  单复数容错，物理外键优先、四元组去重；匹配面板支持逐条确认 / 忽略与多目标冲突标记，按「连接 + 库」持久化；
  推断边用虚线样式与物理外键区分。
- **边避障路由**：正交走廊枚举绕开中间表卡片，按障碍物边界动态生成通道；路由失败回退三段式正交折线。
- **ER 工具栏**：按钮禁止换行，空间不足时横向滚动。
- **软件更新弹窗重做**：打开即自动检查更新，移除手动「检查更新」按钮；收敛为
  checking / latest / available / downloading / ready / error 六状态机，版本 chip 对比与更新说明按行渲染为列表。
- **表格设计器**：「选项」页默认值候选改为弹层选择、索引方法类型对齐、SQL 预览高亮。

### 修复

- **openGauss 索引查询**：openGauss 7.0（9.2.4 内核）没有 `to_jsonb(record)`（42883），
  PG 索引查询改用 `to_json(x)->>'indnkeyatts'`（PG 9.x / 10 / 11+ / openGauss 均可用），并保留 PG 10 的 `indnatts` 回退。
- **openGauss / GaussDB B(MySQL) 兼容模式拒绝**：B/MYSQL 模式下标识符用反引号、`||` 可能被当作逻辑 OR，
  按 PG 语义生成的 SQL 会出错；表设计读写明确拒绝（`TABLE_PG_MYSQL_COMPAT_MODE_UNSUPPORTED`），A/PG 模式正常；
  表数据摘要的 PG 分支改用 `concat()` 而非 `||`。
- **PG 元数据分层降级**：索引元数据现代层用 `unnest ... WITH ORDINALITY` + `indnkeyatts`，
  失败时降级 `string_to_array` + `generate_series` 兼容层（PG 9.x 系与老 openGauss/GaussDB 可跑）；
  列元数据 `pg_catalog` 失败时退到标准 `information_schema.columns`；对象目录序列属性在缺少 `pg_sequence` 时改用 `information_schema.sequences`。
- **TiDB 表结构保存明确拒绝**：TiDB 的 `GET_LOCK` / `LOCK TABLES` 是 noop 兼容功能，不能提供保存所需的服务端锁；
  读取、预览仍可用，并移除旧版 `table_native` 实现。
- **传输 / 同步**：协议与批量导入相关调整；命名空间错误测试适配 `loadChildren` 重抛行为。

### 工程与发布

- **免安装便携包精简**：移除内置的 WebView2 在线引导程序（约 1.9 MB；安装包已通过 `embedBootstrapper` 内嵌，
  Win10/11 通常已自带运行时），便携包只保留 `NavKit.exe`、`data\` 与使用说明，减小下载体积。
- `README` 界面截图更新为当前版本；移除一次性的表设计器截图脚本。
- `package-lock.json` 补齐 `elkjs`，与 `pnpm-lock.yaml` 保持同步。

### 已知限制

- 与 1.0.0 基本一致，详见 [`docs/completed/PRODUCT-STATE.md`](<docs/completed/PRODUCT-STATE.md>)。
- macOS 包未做 Apple 公证与签名；驱动按需下载需要网络。

---

## [1.0.0] — 2026-11-14

首个正式版（General Availability）。相较 `1.0.0-pre`，本版本集中在**数据管线可靠性**、**差异比较**、
**备份与还原**、**新数据源导出**、**表结构与关系图**五个方向改动，
修复了预览版全驱动实测中暴露的一批实际问题，并补齐发布流程中的版本一致性门禁。

### 新增

- **查询首屏流式（opt-in，仅后端）**：新增独立流式执行 API，查询先返回首屏结果
  （默认 200 行，可选 1–1000，8 MiB 上限，首行较慢时最多等 1 秒），
  其余结果在后台持续读取并落入本地结果缓存。旧 `execute_sql_v2` 与 `ExecutionRequest` 语义不变。
  前端接入尚未实施；JDBC / ODBC / REST / HTTP 隧道不宣称支持。
- **大表分页不再全表 COUNT**：首屏预算＝当前页 + 1 行，用 `has_more` 判定下一页，
  页面绘制后再异步精确统计总行数 / 总页数；刷新或真实写入会使统计失效。
- **差异比较有界预览与自适应内存预算**：两侧独立读快照、不假设跨库排序规则；
  小预览不落盘，大预览只溢写一次且不重读任一源；
  同步对话框新增「内存预算控制」（自动 / 手动），按可用内存记账、准入时冻结上限、增量预留共享同一进程账本。
- **导入导出受控批处理**：导出准入与资源预约信封（顶层 import / export 生产者最多 4 个）；
  对「完整显式列、无触发器、无外键」的普通表启用多值批量导入（≤ 256 行 / 约 1 MiB），
  不满足条件时保留逐行；可选 SQL 导出分组（2–1000 行、1 KiB–4 MiB 预算，默认关闭）；
  显式 PostgreSQL text COPY；CSV 预览在解码分配前设总字节界限。
- **新数据源表级导出**：HTTP / REST 类数据源按不透明游标逐页逐条导出（CSV / JSON，
  SQL 与自定义 `where_clause` 明确拒绝）；MongoDB JSON 使用 canonical Extended JSON 保留 BSON 类型，
  CSV 两遍 spool 发现全部顶层字段。
- **备份与还原**：备份对话框新增「源一致性」下拉（`pg_snapshot` / `mysql_locked` / SQLite 同一只读事务）；
  SQLite 逻辑快照备份（同一读事务、不改持久化 PRAGMA）；原生逻辑备份会话（确认清理后才发布）；
  可跳过无法无损读取的表并告警；流式详细还原 + 公开停止（固定源预扫 → 专用物理会话 → 有界报告，
  1000 条 / 8 MiB 软阈值，明确区分已回执 / 已提交 / 已回滚 / 已跳过 / 未知）；取消时绝不发布半成品文件。
- **表结构与关系图**：只读 ER 元数据与可编辑结构校验解耦；
  分页探测契约（无 `COUNT` 时 `total` 仅为已证明下界）；SQLite 命名空间显式解析，避免 `temp` 遮蔽；
  ER 元数据改为不可变快照 + 有界 TTL（5 分钟）+ 合并加载 + 身份校验提交，渲染侧按可见区域切片；
  表设计器「选项」页改读真实信息模式（MySQL 家族引擎 / 字符集 / 排序规则 / 行格式候选）。
- **数据生成**：版本化确定性 native recipe（上限 1000 万行，总数不物化、逐行生成，
  SQLite 显式分段每 1000 行 / 8 MiB，预览最多 20 行样本）；
  修复公开预览完成判定缺陷（不再把「停止确认」布尔值当成成功标志）。
- **界面**：可搜索下拉框与可搜索输入框（在原生 `select` / `input` 之上叠加过滤浮层，
  DOM / 值 / 样式 / 消费者事件不变，支持逗号分隔多值）；
  备份对话框重做（对象搜索过滤、跳过不兼容表、加载失败重试、成功结果页与 `role="status"` 播报）；
  全局唯一对象标签页；数据网格列宽铺满容器（手动拖过的列锁定不参与分摊）；
  连接对话框新增显式「使用空密码 / 无密码认证」，测试连接加入 deadline。
- **值类型保真**：MySQL 复合几何保留 SRID 与 WKB 并可往返写回；
  PostgreSQL `INTERVAL` 保留月 / 日 / 微秒，`POINT` 与 `MONEY` 支持二进制与文本两种格式，
  `MONEY` 小数位在服务端会话按 `lc_monetary` 解析；
  无法无损解码的非 NULL 值显示显式标记而非 NULL，导出时按类型化错误拒绝。

### 修复

- **SQL 文件受默认 30 秒查询超时截断** → 批量执行面板新增文件级独立总超时（0 表示不限时）。
- **含 GEOMETRY / MONEY 的表阻断整库转储** → 支持跳过不兼容表并告警，并增加空间 / `INTERVAL` / `MONEY` 专门解码。
- **备份写入程序安装目录** → 备份列表改为有界读取，并支持切换存储根。
- **达梦 JDBC 多语句批量报 `Unsupported SQL type`** → JDBC 路径改为顺序批量执行。
- **数据字典导出文件名混入反斜杠** → 统一字典导出路径处理。
- **密码框未清空时输入会拼接** → 连接对话框改为显式的「使用空密码 / 无密码认证」选项。
- **导出向导显示的文件名与实际落盘不一致** → 统一导出路径推导。
- **JDBC sidecar 崩溃时无线索** → `stderr` 改为有界环形缓冲，记录非零退出码与 stderr 尾部。

### 性能

以下数字来自仓库内隔离环境的对照测量（Windows + SQLite + 独立测试库），用于说明改动方向，
不同数据库、硬件与数据分布下数值会不同。完整对照见
[性能对照](<docs/active/navkit-performance-comparison.md>)与[内存预算方案](<docs/active/navkit-adaptive-diff-memory-budget.md>)。

- CSV 导入（SQLite）：1000 行 59.940 ms → 23.039 ms（2.60×）；100000 行 10.504 s → 2.455 s（4.28×）。
- CSV 预览：23,977,785 B / 200000 行只返回 20 行，667.592 ms → 1.602 ms（−99.8%）；RSS 中位峰 94.14 → 20.15 MiB。
- SQL 组包全周期（10000 行）：2.791 s → 0.971 s（2.88×）；同文件恢复 2.188 s → 0.393 s（5.56×）；文件体积 −46.7%。
- 数据生成（后端 recipe）：100 万行 28.306 s / RSS 峰 32.55 MiB；1000 万行 279.487 s / RSS 峰 32.38 MiB。
- 大表分页首屏（百万行 SQLite）：27.26 ms → 24.37 ms（无筛选）、91.90 ms → 40.81 ms（带筛选）。
- 差异比较（10 万窄表）：旧默认 1.918 s / 104.12 MiB → 自适应自动 1.854 s / 87.10 MiB（零溢写）。

### 工程与发布

- CI 新增 Windows 验收作业：前端类型检查与构建、UI 安全回归（1399 项）、
  Rust 回归（库 1328 项 + 5 个集成目标），并校验发布身份。
- `release.yml` 新增独立 `acceptance` 作业；`build` 依赖 `acceptance`，`publish` 依赖 `build`；
  手工触发同样执行 `check-release.mjs --tag` 校验；构建使用 `--locked`。
- 版本一致性门禁（`tools/check-release.mjs`）：`package.json`、`package-lock.json`、`pnpm-lock.yaml`、
  `src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock` 六处版本必须一致，
  npm / pnpm 双锁依赖必须同步，tag 必须严格等于 `v<版本>`。
- 发布时自动生成 `navkit-win64.json` 更新清单（含 SHA-256 与大小）；
  正式版标记为 `latest`，带 `-` 的预览版标记为 `prerelease`；客户端内置 10 个逐级加速镜像。
- `README` 重写为面向用户的产品说明，新增 `assets/screenshots/` 真实界面截图。
- 文档生命周期重构为 `completed/` / `active/` / `planned/` / `design/` 四条线，历史原型归档到 `docs/design/archive/`。
- 新增仓库根目录 `CHANGELOG.md`，逐版本记录变更。

### 已知限制

- 增量同步仅认证 SQLite 双端原生通道；视图无稳定业务键，配对时显式改全量物化。
- 查询首屏流式未接入前端。
- 低内存差异比较在 50 行、1000 行、宽字段等小数据量场景反而更慢，「内存预算控制」默认不勾选。
- Redis 无表级导出；MQTT 拒绝全量导出；Pulsar 导出目录 / 主题统计；etcd 与 ZooKeeper 导出单 key / znode 当前值。
- HTTP / REST 导出不支持 SQL 与自定义 `where_clause`；MongoDB 未实现跨集合 / 跨页一致快照。
- PostgreSQL COPY 能力限制严格；`mysql_locked` 可能隐式升级为写锁并阻塞源端读写。
- 数据生成 recipe 仅在 SQLite 原生完成千万行验证，PostgreSQL / MySQL 分段模式写入前明确拒绝。
- 内存预算是核算的比较缓冲，不是进程 RSS 或分配器硬限。
- 金仓保存、HighGo 查询、达梦首屏、H2 空路径等静默退出仍缺运行时崩溃证据，已补 sidecar 退出码与有界 `stderr`，不代表修复。
- 数 GB 级备份与数百万行导入在极低内存环境下仍有内存峰值风险。
- macOS 包未做 Apple 公证与签名；驱动按需下载需要网络。

---

## [1.0.0-pre] — 2026-11-12

首个公开预览版（Pre-release），用于提前体验与验证。

### 新增

- **多数据源管理**：MySQL / MariaDB、PostgreSQL、SQLite、SQL Server、Oracle、ClickHouse、Redis、MongoDB 等原生连接，
  以及 ODBC / JDBC / H2 通用连接；驱动按需下载。
- **数据库对象与表设计**：对象树浏览、表结构设计、字段 / 索引 / 外键编辑、结构同步。
- **SQL 与查询**：SQL 编辑器与补全、查询结果表格、筛选 / 排序 / 分页、结果导出。
- **数据导入导出与同步**：导入导出向导、数据同步、批量执行与执行日志。
- **辅助能力**：ER 图、数据字典、调度任务、服务器监控、历史记录。
- **便携模式**：配置、密钥、收藏与备份均保存在程序目录，连接凭据使用 AES-256-GCM 加密存储。

### 工程与发布

- 首次建立多平台自动构建：Windows（x64 / ARM64）、macOS（Apple Silicon / Intel）、Ubuntu（x64）。
- 统一产物命名为 `NavKit-<版本>-<系统>-<架构>-<类型>.<扩展名>`。
- 补齐 ODBC 运行时依赖：`.deb` 声明 `libodbc2`、macOS 内置 unixODBC、便携版附 WebView2 引导程序。
- 发布上传仅取顶层文件，deb 依赖去重；macOS 同时产出 app 与 dmg 并健壮打包便携版。

### 已知限制

- 这是预览版，接口与数据格式仍可能调整，请勿用于生产关键数据，升级前请备份。
- 部分数据库类型依赖本机驱动或厂商服务，需要在联网且具备相应驱动的环境中使用。
