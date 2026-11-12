# NavKit — 数据库管理工具

用 **Tauri 2 + Rust + 原生 TypeScript** 实现的桌面数据库客户端，
产出**免安装绿色版单文件 exe**。

> 说明：应用图标、功能图标均为原创绘制（内联 SVG），产品标识为 NavKit，
> 不含任何第三方商业软件的资源。
> 例外：各数据源的连接图标使用**官方品牌 logo**（统一取自各厂商公开的品牌资源集，如 MySQL 海豚、PostgreSQL Slonik、
> Oracle 标志、MongoDB 叶片等 101 种，素材存于 tools/db_art/，经 tools/gen_dbicons.py 生成），
> 不使用自绘品牌图形；彩色=已连接 / 灰色=未连接，**规划中（适配器未提供）的数据源图标固定置灰**。
> 例外：Apache Druid（未收录于统一资源集）保留 Apache 官方标志；「自定义 ODBC」为通用连接类型、无品牌标志。

---

## 一、快速开始

```bash
npm install && npm run tauri build
```

Windows 一键构建（同时产出绿色版目录）：

```powershell
powershell -ExecutionPolicy Bypass -File .\build.ps1
```

产物：

- `src-tauri/target/release/navkit.exe` —— 独立可执行文件
- `dist-portable/` —— 绿色版目录（exe + `data/` + WebView2 引导程序 + 说明）

开发期浏览器预览（不启动 Rust 后端，使用模拟 IPC）：

```bash
npm run dev            # 打开 http://localhost:5183/preview.html
```

真实桌面开发（连接真实数据库，不加载演示 mock）：

```bash
npm run tauri:dev       # 使用 /index.html
```

浏览器预览只用于界面演示，不能据此判定数据库操作或原生窗口按钮测试通过。

### 本轮新增能力与边界

- 多条件 AND/OR 筛选、明确的大小写敏感选项、多字段优先级排序；空结果不禁用筛选/排序。
- 数据源可拖入/移出分组文件夹、拖到连接上半/下半插入前/后，并在重启后保留顺序；也支持 Alt+↑/↓ 排序、Ctrl+X/V 移组、Ctrl+Shift+V 插到目标后。数据库显式打开后显示展开箭头。
- 顶部「查询」「备份」紧随「函数」，暂隐藏「其它」「图表」入口；「连接」直接打开新建连接，不显示展开箭头。
- SQL 提示读取当前连接/模式的真实表、视图与字段，识别 FROM/JOIN、别名和限定名；选区优先执行，无选区执行全文。选中 SQL 可右键运行，也可 Shift+F10 打开菜单、Esc 关闭。查询页不再提供「文本」显示格式操作。
- 查询创建工具按真实表 / 视图结构选择字段、AND 条件、排序及行数，生成对应方言 SELECT 预览；确认后只插入光标 / 替换选区，不自动执行，不覆盖整篇 SQL。切表异步请求、预览失效及跨标签回写均有保护。
- 手动事务由原生 SQLite / MySQL 协议 / PostgreSQL 协议持有真实事务连接，绑定查询页与数据库 / 模式；开始、提交、回滚、关闭连接或退出均有明确确认。错误后只允许回滚，其它自动提交写入工具停用；SQLite 仅 main，MySQL 要求当前库全部表为 InnoDB，写入仅支持无触发器的当前库单表 DML。事务内仅支持 SELECT/WITH 与 DML，解释计划需先结束事务。
- 结构页字段上下移动对原生 MySQL / MariaDB 既有字段启用：保留 SHOW CREATE 的完整列定义，保存前复核模式与定义，一次最小 ALTER 并读取真实顺序确认。不能与其它字段修改混合；只读、ODBC 与不支持物理重排列的方言提供禁用原因，不靠重建表冒充排序。
- 查询执行目前不支持中断正在运行的语句，「停止」明确禁用；执行期间不能切换连接/数据库，以避免结果归属错误。
- 同步 / 生成对 SQLite 仅支持 main 命名空间，拒绝附加库 / temp 同名阴影以防误写；自定义 ODBC 和未支持的工具协议会明确反馈，不冒充成功。
- CSV/JSON/SQL 导入导出；表数据同步支持仅结构/仅数据/二者、列名与类型映射、主键差异比较及增量插入/更新。SQLite、MySQL、PostgreSQL 的 3 个同构/6 个异构方向已完成本轮真实 108 场景；SQL Server/TDS、ClickHouse 与四种 ODBC profile 另完成 42 个状态化协议模拟场景，不算对应厂商实库或默认 JDBC 路线认证。详见[本轮完整动态测试报告](<tools/acceptance/sync/TEST-REPORT-2026-11-07.md>)、[复现说明](<tools/acceptance/sync/README.md>)及[实库逐场景记录](<tools/acceptance/sync/live-report.json>)。
- MySQL DDL 会自动提交，重建目标表不可整体回滚；取消在数据库操作之间协作执行，不强制终止正在执行的语句。SQL Server/ClickHouse/ODBC 当前通用同步通道不承诺事务回滚；ClickHouse 标准 UPDATE 增量更新在写入前明确拒绝，可关闭更新仅插入缺失行。
- 数据生成、服务器只读监控、日志检索以及详细对象元数据。
- 自定义驱动支持本机 64 位 ODBC 已安装驱动、DSN、可信原生 DLL，以及真实 Java sidecar 加载 JDBC JAR；**JAR 不作为原生 DLL 加载**。JDBC / H2 需要 Java 11+ 和 source compiler，严格匹配原包、必需依赖、厂商类、SPI 与 URL。当前原件清单含 31 套完整包（28 套官方来源、3 套明确标注的第三方固定版本原件），覆盖 34 个类型。15 套官方包专项与其余 8 组 Java 11/17 专项分别保留，不把闭端口拒连、驱动 ready 当作厂商服务器认证。Access、H2、DuckDB 另有真实本地 SQL；BigQuery 有真实 OAuth HTTP 拒绝，Spanner 有真实 gRPC 拒绝。详见[当前原件及依赖清单](<tools/acceptance/simulation/driver-registry-final-20261107.json>)与[完整验收说明](<tools/acceptance/simulation/README.md>)。
- 原有十种规划数据源已接入实际协议：Druid、H2、DynamoDB、Cassandra、NebulaGraph、Kafka、RocketMQ、MQTT、ZooKeeper、etcd。不同产品、TLS/认证能力与实库验证状态不得混为一谈；详见[原生适配使用与边界](<tools/acceptance/datasources/NATIVE_ADAPTERS.md>)。
- 新建连接卡片原标签位置提供驱动状态和直接下载。**31 套原件、34 个类型的完整生产桌面流程已通过 71 项检查**，包括实际点击下载、单次 IPC、完整包/依赖校验、选中项和焦点保留。H2 / DuckDB / Access 还执行真实 SQL。Caché / SUNDB / OSCAR 下载来源明确为第三方固定版本，非厂商官网；ArgoDB 使用官方 QuarkDriver 而非 Inceptor，托管模式仅空凭据 auth=noSasl。Salesforce / HBase 新增内置 REST，无需数据库驱动；含 SOQL/CRUD 或 scanner/二进制读写的状态化实适配器测试。当前目录为 66 个内置类型、34 个可下载类型及 2 个需显式目标的通用入口，不捏造通用 ODBC/JDBCX 的唯一驱动。详见[最终桌面记录](<tools/acceptance/simulation/desktop-final-verified-20261107/report.json>)及[获取范围和限制](<tools/acceptance/simulation/driver-download-limitations-20261107.md>)。
- [全目录逐项验收](<tools/acceptance/simulation/README.md>) 的完整历史快照覆盖全部 102 项：8 项实库参考、94 项状态化协议/兼容模拟，0 失败、0 缺项；[逐项报告](<tools/acceptance/simulation/coverage-report.md>)保留每项方式与限制。后续完整 JDBC 下载及 Salesforce/HBase 原生 REST 有独立增量证据，**不把早期 ODBC 合同测试倒算为新默认 JDBC/REST 路线全量重跑**。最终 Rust 普通回归 259 通过、0 失败、23 项按条件 ignored；UI 166 通过。协议模拟、ODBC 测试代理及同家族复用不算对应厂商实库认证；[历史实库矩阵](<tools/acceptance/datasources/README.md>)单独保留。
- [最终证据校验脚本](<tools/acceptance/simulation/create-run-record-final-20261107.mjs>)已实际退出 0，生成 `result=PASS` 的[机器运行记录](<tools/acceptance/simulation/run-record-final-20261107.json>)；交付摘要见[验收报告](<tools/acceptance/simulation/final-acceptance-20261107.md>)。该结果对应固定程序与各自哈希快照；同期同步模块和 dist 后续改动单独记录，不将固定程序验收冒充整个最新工作区的新测试。
- Windows 未注册 DLL 加载需要唯一临时用户 DSN，仅保存 Driver 路径，连接建立或失败后清理。此为纯便携存储以外的明确临时注册表例外，不永久安装驱动，不写机器级 ODBC，不保存明文密码到 DSN。
- 打包原地更新程序，不删除 `dist-portable/data`，连接、密码密钥、收藏及备份原样保留。

---

## 二、界面规格

界面为桌面客户端密度的自绘 UI，全部尺寸取自 `src/styles/base.css` 设计令牌
（详细像素规格另见 [`docs/界面规格表.md`](docs/界面规格表.md)）：

```
标题栏（含菜单栏）36px   #F7F8FA，菜单行 24px / 字号 12px
Ribbon       44px   彩色图标 + 标签，选中块 #E3EDFD
标签条       32px   激活标签自动滚入可视区
标签页工具栏 32px
状态栏       26px   顶边 #E4E7EC
左侧树       264px（可拖拽）      信息面板 292px（240–400px）
```

悬停底色 `#E8EBEF`，主色 `#2B62D9`；
字体 Microsoft YaHei UI，菜单 12px / Ribbon 与树 13px，
网格表头 28px、数据行 26px、树行高 24px。

---

## 三、图标

应用内全部图标为 **原创手绘内联 SVG**（`src/ui/icons.ts`），
应用图标（标题栏 logo、任务栏图标、关于对话框）由
`tools/make_original_icon.py` 生成（三层等距菱形透明悬浮堆叠、无背景容器：
顶层青色 = 活动数据、中层白色 = 数据本体、底层靛蓝 = 存储基座，每层带同族加深细描边）。

---

## 四、UI 细节

| 元素 | 规格 |
|---|---|
| 标题栏（含菜单栏） | 36px，菜单行 24px / 字号 12px |
| Ribbon 高 | 44px |
| 左侧树宽 | 264px（可拖拽） |
| 标签条高 | 32px，激活标签自动滚入可视区 |
| 信息面板宽 | 292px（240–400px） |
| 状态栏高 | 26px |
| 悬停底色 | `#E8EBEF` |
| Ribbon 选中 | `#E3EDFD` |
| 网格行高 | 26px（表头 28px） |

已实现的细节：

- 菜单栏：`文件 编辑 查看 收藏夹 工具`，右侧固定项 `数据源同步 选项`；
  查询标签页激活时追加「查询」菜单，含 Windows 加速键
- Ribbon：连接·新建查询｜表·视图·函数·查询·备份｜用户·自动运行，
  「新建查询」「备份」后有竖分隔线；「连接」直接打开新建连接，
  「用户」直接打开用户管理页；「其它」暂隐藏
- 标签页工具栏随标签类型变化：
  - 表数据页：文本 / 筛选 / 排序 / 导入 / 导出 / 数据生成 / 设置（可见列）
  - 查询页：开始事务 / 保存 / 查询创建工具 / 美化 SQL / 代码段 / 导出结果
  - 表设计器：保存 / 添加字段 / 插入字段 / 删除字段 / 主键 / 上移 / 下移
- 网格底栏：首页 / 上一页 / 页码输入框 / 下一页 / 末页 / 更多 + 视图切换
- 状态栏：左侧连接图标 + 连接名、对象图标 + 对象名；右侧 `第 N 条记录（共 M 条）于第 P 页`
- 信息面板：对象图标 + 名称 + 副标题，下方成对的「标签 / 值」字段行
- 连接对话框：常规 / 高级 / 数据库 / SSL / SSH / HTTP 六个选项卡，
  常规页顶部两个大图标选择器（连接 / 数据库）
- 表设计器：字段 / 索引 / 外键 / 检查 / 触发器 / 选项 / 注释 / SQL 预览 八个选项卡

---

## 五、功能清单

| # | 功能 | 实现位置 |
|---|---|---|
| 1 | 连接管理（新建/编辑/删除/测试、分组、密码加密） | `src/dialogs/ConnectionDialog.ts`、`src-tauri/src/commands.rs` |
| 2 | 对象树（库/模式 → 表/视图/存储过程/函数/触发器） | `src/core/App.ts` `loadChildren` |
| 3 | 表数据网格（分页、排序、筛选、行内编辑、增删行、提交/回滚） | `src/ui/DataGrid.ts` |
| 4 | 表结构（列/类型/主键/索引/外键/注释 + ALTER 语句预览） | `StructureTabView` |
| 5 | 查询编辑器（CodeMirror 6、高亮、快捷键、自动补全、执行全部/选区） | `src/ui/SqlEditor.ts` |
| 6 | 结果网格（复制单元格/整行、导出 CSV/JSON/SQL、列宽记忆） | `DataTabView` / `QueryTabView` |
| 7 | 导入导出向导（步骤条 + 真实执行） | `src/dialogs/ImportExportDialog.ts` |
| 8 | 工具（服务器信息、数据传输、SQL 转储） | `src-tauri/src/transfer.rs` |
| 9 | 备份 / 还原（整库转储为 SQL：结构 + 数据 + 视图；备份列表、新建/还原/删除备份、打开所在文件夹） | `src/dialogs/BackupDialog.ts`、`src-tauri/src/transfer.rs` `dump_database`、`commands.rs` |
| 10 | 用户管理（MySQL 协议族读 `mysql.user` 真实数据；建户 / 删户 / 改密码） | `UsersTabView`、`commands.rs`、`src/dialogs/UserDialog.ts` |
| 11 | 视图设计器（读取定义 → 编辑 → `CREATE OR REPLACE VIEW` 真实保存） | `SqlDesignTabView`、`commands.rs` `get_view_definition` |
| 12 | 函数 / 存储过程（源码查看与整体重建） | `commands.rs` `get_routine_definition` |
| 13 | 自动运行（任务保存 / 到期自动执行 / 立即运行 / 运行历史，任务存 `data/jobs.json`） | `ScheduleTabView`、`commands.rs` `run_job_now` |
| 14 | 通用（最近打开、收藏夹、设置、全中文界面） | `SettingsDialog.ts` / `storage.rs` |

已接入的数据库协议示例（不等于每个产品已完成实库验收；实际结果见[全目录实库报告](<tools/acceptance/datasources/README.md>)）：

| 协议 | 数据库 | 默认端口 |
|---|---|---|
| 内置驱动 | SQLite / MySQL(MariaDB) / PostgreSQL | — / 3306 / 5432 |
| MySQL 协议 | TiDB (4000)、OceanBase (2881)、PolarDB (3306)、TDSQL (3306)、Doris/StarRocks (9030) | — |
| PostgreSQL 协议 | openGauss (5432)、KingbaseES (54321)、HighGo (5866) | — |
| 原生 HTTP 接口 | ClickHouse (8123) | — |
| 系统 ODBC 驱动 | 达梦 DM (5236)、Hive (10000)（需安装厂商 ODBC 驱动，连接串可用「参数」字段覆盖） | — |

---

## 六、便携模式

- 全部配置写入 **exe 同目录的 `./data/`**：`config.json`、`connections.json`、
  `favorites.json`、`recent.json`、`key.bin`、`logs/`、`cache/`、`backups/`（备份文件）、
  `jobs.json` / `job_history.json`（自动运行）
- 不永久安装或写机器级 ODBC 注册表；Windows 未注册原生 DLL 通过仅含 Driver 路径的唯一临时用户 DSN 连接，建立或失败后清理。
- 连接密码使用 **AES-256-GCM** 加密（密钥在 `data/key.bin`）
- 整个文件夹拷走即用

### WebDAV 数据源同步

这是独立的**连接配置同步**，不是工具菜单中同步表结构或表数据的功能。入口为菜单栏右侧「数据源同步」，或「选项」中的「数据源同步」。

**固定同步范围**：数据源全部连接字段（含密码、连接参数、本机文件路径文本）、空分组、分组归属、组内顺序。**不同步** SQL 文件、备份、收藏、历史、任务、应用设置、数据库内容/文件、自定义驱动定义、DLL/JAR、证书或私钥附件；不包含代理或请求转发。同步配置本身也不上传。

1. 填写 WebDAV **基础目录**、用户名和密码；推荐 HTTPS。HTTP 虽然传输的是加密同步包，但认证凭据仍可能被窃取。
2. 为所有设备设置相同的**独立同步加密口令**，至少 8 个字符，不能与 WebDAV 密码或认证令牌相同；推荐使用长随机口令并另行妥善保存。遗失口令无法解密远端。
3. 如需特殊认证，在「自定义请求头」中按每行 `Name: value` 填写，支持空行和 `#` 注释；`Authorization` 优先于 Basic。条件写入等协议保护头禁止覆盖。密码、口令、请求头不会回显；留空保留旧值，通过对应清除复选框删除。关闭本机加密保存后，这三项秘密仅当前会话保留，重启后需要重新录入。
4. 「保存配置」后「测试连接」进行**只读**连通性检查，不创建/覆盖快照，也不能据此证明写权限或口令正确。首次确认合并才按需创建同步目录及文件。
5. 点击「预览同步」，核对新增、修改、删除、分组/顺序及本机依赖警告。每项冲突必须明确选择本机或远端版本，再勾选确认并点击「确认合并」。预览 3 分钟过期，修改配置或任一端数据变化后须重新预览。

远端仅使用 `navkit-sync/datasources.enc`，内容通过 AES-256-GCM、PBKDF2-SHA256（600,000 次迭代）整包加密。密码从本机解密后进入同步包，下载后用目标设备本机密钥重新加密；**不传输 `key.bin`**。本机同步配置、基线和恢复记录写入加密的 `data/datasource_sync.enc`。修改 URL 或用户名会清除旧端点的秘密及基线；修改实际生效的认证身份也会重置基线，重新接入已有远端须确认。

默认**仅手动**；可选每 3 / 15 / 30 / 60 分钟，只在应用运行时执行，不在启动时立即同步。自动同步不选择冲突，也不自动确认首次接入、删除或恢复；有查询/任务、手动事务、表格或结构未提交编辑时延后。需要操作时状态栏提示，不在后台循环弹窗。

- 合并使用稳定连接 ID 的三方比较、删除标记及独立组织/排序冲突；不使用设备时间静默选择版本。
- 首次上传使用 `If-None-Match: *`，更新使用强 ETag 的 `If-Match`；远端变化或缺少可靠强 ETag 时拒绝盲写。上传后重新读取并核验实际内容。401/403、限流、错误口令、损坏文件不会被当作空远端覆盖。
- 本地批量应用使用原子持久化及版本校验，活动数据库操作与驱动变更同提交互斥。只改名称、分组或顺序不关闭连接池；真正改变/删除连接时暂停旧页面、保留未保存 SQL，不自动重连新端点，查询页须明确重新选择连接。
- 基线保存失败但连接已落盘时，界面仍刷新真实数据源并暂停受影响的旧页面，明确提示「同步尚未完成」，不记录成功时间；保留加密恢复记录，修复本机写入权限后手动重新预览恢复。
- 数据库绝对路径、驱动 ID/参数只同步为文本；新设备缺少对应路径或驱动时提示需要本机配置，不复制附件、不自动连接，也不会因此自动创建空数据库。

验证：`npm run build`、`npm run test:ui`、`cargo check --offline` 和 `cargo test --offline --lib`（后两条在 `src-tauri` 下运行）。离线浏览器交互验证运行 `node tools/tests/datasource-sync-browser.mjs` 与 `node tools/tests/driver-manager-browser.mjs`，需要已安装 Chrome（可通过 `BROWSER_EXE` 指定），不启动服务、不连接真实 WebDAV、不读取用户数据。

---

## 七、目录结构

```
tauri-navicat/
├── src/                        前端（原生 TypeScript，无框架）
│   ├── core/                   主控制器、IPC 桥接、状态机
│   ├── ui/                     原生组件（网格、编辑器、树、菜单、图标）
│   ├── dialogs/                对话框
│   └── styles/base.css         像素规格样式表
├── src-tauri/src/              后端 Rust（Tauri 2）
│   ├── commands.rs             IPC 命令层
│   ├── db.rs / storage.rs      连接池、便携存储与 AES-256-GCM
│   ├── execution*.rs           统一执行门禁、授权、预算、取消
│   ├── mutation.rs             受控写入（typed 绑定 / CAS / 读回）
│   ├── catalog.rs / schema.rs  元数据与对象目录
│   ├── transfer.rs             导入导出 / 传输 / 转储
│   └── *_tests.rs              模块内回归测试
├── bridge/http-tunnel/         JDBC sidecar 网关（Java）
├── tools/                      测试与辅助脚本（acceptance 数据夹具不入库）
├── testdata/                   轻量测试夹具
└── docs/                       文档，按生命周期分三条线
    ├── completed/              只有结果：PRODUCT-STATE.md
    ├── active/                 进行中任务书、测试轮次、证据日志
    ├── planned/                尚未开工的计划
    └── design/                 设计系统、界面规格、UI 原型（常驻）
```

源码文件清单不在这里维护——它会随代码漂移。某项能力的代码入口以
[产品状态文档](<docs/completed/PRODUCT-STATE.md>) 为准，文档维护规则见
[文档索引](<docs/README.md>) 与 skill `.agents/skills/docs-lifecycle/SKILL.md`。

---

## 八、技术栈

- **Tauri 2.x**，Rust 2021 edition
- **Vite 6 + 原生 TypeScript + 原生 CSS**（无 React/Vue/Tailwind，避免框架默认样式破坏像素还原）
- **CodeMirror 6** SQL 编辑器（高亮、行号、快捷键、自动补全）
- **sqlx 0.8**（MySQL / PostgreSQL / SQLite，异步 tokio）
- **手写虚拟滚动 DataGrid**（无第三方表格库）

---

## 九、隐私与网络

- **无遥测、无埋点、无崩溃上报、无自动更新检查**；
- 前端 CSP 限定 `connect-src 'self' ipc:`，网页层无法发起外部请求；
- 数据库连接与 WebDAV 同步请求均发往用户自己配置的地址；WebDAV 功能默认仅手动同步，不做代理或请求转发。
