# 数据同步动态验收

本目录专门验收实际数据同步，不以源码检查、SQL 字符串匹配、服务 ready 或驱动加载成功代替同步成功。测试调用生产 `synchronize`、`diff`、`synchronize_database`，创建隔离的数据，实际写入后独立回读全部行/列，并检查失败、取消和清理。

2026-11-07 已实际执行结果见[完整测试报告](<D:/github/tauri-navicat/tools/acceptance/sync/TEST-REPORT-2026-11-07.md>)和[机器证据核对记录](<D:/github/tauri-navicat/tools/acceptance/sync/sync-run-record-final-20261107.json>)。本说明供复现，不能将预期用例数量直接当作另一次运行的通过结果。

## 范围与证据等级

| 路线 | 环境与方向 | 完整性要求 |
|---|---|---|
| 真实数据库 | SQLite、MySQL、PostgreSQL 的 3×3 矩阵；3 个同构方向、6 个异构方向 | 9 方向×12 场景＝108；缺连接记 FAIL/NOT_RUN，不能算通过 |
| SQL Server/TDS 模拟 | 生产 tiberius 客户端→本机严格 TDS 模拟器→可变 SQLite 存储 | 8 个场景；不是 SQL Server 厂商服务 |
| ClickHouse 模拟 | 生产 HTTP 客户端→严格 HTTP/SQL 子集模拟器→可变 SQLite 存储 | 8 个场景；不是 MergeTree/异步 mutation 认证 |
| ODBC 模拟 | 真实 Windows ODBC manager→现有测试 facade→真实 DuckDB 文件 | 通用 ODBC 7、Oracle profile 6、Dameng profile 6、Db2 profile 7；均不是对应厂商数据库 |
| 补充回归 | 真实 SQLite 文件的差异/增量、精度、键、取消、2000 行完整回读，以及导入导出 | 13 个新边界场景，另保留既有测试；不是吞吐基准 |

**42 个模拟场景和 108 个真实场景是完整性要求，不是预先宣布 PASS。** 实际状态、数量和错误以本轮 JSON、日志及进程退出码为准。模拟报告始终标记 `vendor_server_status=NOT_TESTED`、`vendor_real_pass=0`。

真实 MySQL/PostgreSQL 使用同一台本机服务器上的独立 UUID 数据库/模式与独立生产连接池，不冒充跨机器测试。本机已有真实服务，因此没有使用来源不明的公开可写数据库；只读公开库也不能验证目标写入。

## 从工作区根目录复现

### 1. 普通 Rust / SQLite 回归

原子文件编辑可能保留 mtime，Cargo 会误用旧产物。修改源代码后，应先清本项目编译产物：

```bash
cargo clean --offline --manifest-path src-tauri/Cargo.toml -p navkit
npm run test:rust
# 仅跑同步相关集成测试：
npm run test:sync
```

`clean -p navkit` 只处理生成的编译产物，不删除用户数据库、连接配置、便携数据目录或现有 ODBC DLL。普通命令中的条件性 `ignored` 不作为实库通过数。

### 2. 真实 MySQL/PostgreSQL 矩阵

测试账号须允许创建并删除本次 UUID 数据库/模式。禁止指向生产数据；不要把密码写进仓库或报告。以下是 Bash 环境变量示例，密码由操作者提供：

```bash
export NAVKIT_MYSQL_HOST=127.0.0.1
export NAVKIT_MYSQL_PORT=3306
export NAVKIT_MYSQL_USER=root
export NAVKIT_MYSQL_PASSWORD='<测试账号密码>'
export NAVKIT_PG_HOST=127.0.0.1
export NAVKIT_PG_PORT=5433
export NAVKIT_PG_USER=postgres
export NAVKIT_PG_DATABASE=postgres
# 需要密码时另设 NAVKIT_PG_PASSWORD。
export NAVKIT_SYNC_LIVE_REPORT='D:/github/tauri-navicat/tools/acceptance/sync/live-new-run.json'
npm run test:sync:live
```

命令显式运行 `ignored` 的实库测试，并启用 `--no-fail-fast`；一个目标失败不会遮住后续目标。新矩阵中连接失败必须记录，全部方向与场景执行完后统一判定失败。旧导入导出实库验收另核完整行数据。

12 个实库场景：新建同步、仅结构后仅数据、替换、重建、增量插入、增量更新、精确整数/小数与空白的异构幂等、Unicode 复合主键、过滤与列映射、约束失败的事务回滚、整库部分失败后继续及视图物化、非法选项不改变目标。

每方向检查 NULL/空字符串、中文与 emoji、引号/反斜杠/换行、BLOB/空 BLOB、布尔、精确小数及微秒时间。失败也进入报告；先删除并核对自建命名空间，再关闭池与清理 SQLite 自有目录。

### 3. 六套状态化模拟（推荐统一入口）

需要 Python 3；四种 ODBC profile 另需本机 Windows 与仓库现有的两个测试 DLL。执行器会检查 DLL 存在与前后哈希，不安装或重建驱动，不回退为静态测试。

```bash
export NAVKIT_PYTHON='C:/Users/gaohu/AppData/Local/Programs/Python/Python313/python.exe'
# 必须是绝对路径；每轮使用全新目录，不能覆盖或借用旧报告。
export NAVKIT_SYNC_SIM_REPORT_DIR='D:/github/tauri-navicat/tools/acceptance/sync/simulated-new-run'
npm run test:sync:simulated

# 可选只运行指定协议；不是完整六套验收：
node tools/acceptance/sync/run-simulations.mjs --protocols=odbc --odbc-kinds=oracle,dameng,db2
```

统一执行器先 `cargo clean -p navkit`，再串行编译/运行六套真实产品同步测试。不要并发启动另一个 Cargo 任务或修改受追踪源码。每套保存 JSON 与 `.log`，最终汇总源码哈希、实际测试二进制哈希、DLL 哈希、数量、错误和清理结果；缺报告、未执行足量场景、非零退出、厂商标识混淆、源码/DLL/二进制变化或清理不完整，均不能 PASS。

ODBC 双向二进制在每方向的一个场景中检查四行：`00FF1080`、NULL、空 BLOB、`00017F80FEFF41C3A9F09F9982`。除了产品 `0xHEX` 回读，还检查 SQLite 原始 `Vec<u8>` 与存储类型、DuckDB 实际 `typeof`/`hex`，不允许文本十六进制冒充真实字节。SQL Server 验证双向二进制及确定性中间/最后语句取消。ClickHouse 验证同时存在缺失行和变化行时，在任何 INSERT 前安全拒绝不支持的 UPDATE，目标保持不变。

## 结果与限制

- 新矩阵/模拟报告记录运行时间、源码 SHA-256 和实际执行二进制 SHA-256；源码文件哈希本身不能证明二进制已包含修复。模拟汇总另保留成功的包产物清理日志。
- 对具备事务的 SQLite/MySQL/PostgreSQL 已存在目标表，验证 DML 失败/取消回滚。MySQL DDL 隐式提交，不承诺建表/重建整体回滚。
- SQL Server/ODBC/ClickHouse 当前通用通道按语句自动提交；错误/取消不伪造回滚。已执行写入可能保留，任务仍必须正确标记取消而非完成。
- 增量是全表读取后的主键比较，不是 binlog/WAL CDC；不删除目标独有行。缺完整主键/NULL 主键/重复键时应明确拒绝不能安全比较的操作。
- ClickHouse 普通 UPDATE 增量更新不支持，验证的是提前拒绝且零写入；仅插入缺失行可单独测试。
- 未测试真实 SQL Server、ClickHouse、Oracle、Dameng、Db2 厂商服务，未认证全方言、TLS/认证、完整 LOB/流式读取、跨机器网络故障、CDC 或大规模性能。Oracle/Dameng/Db2 此处仅显式 ODBC 测试通道，不把结果倒算为当前默认 JDBC 路线的同步验收。
- 测试通过生产 Rust 同步入口，不声称完成 GUI 点击端到端验证；Node UI 回归只是辅助证据。
- 模拟器仅有有限的可变 SQL/wire 子集；fixture 离线自检不计产品同步通过。进程被强制杀死时的 RUNNING 或未清理记录也不能计通过。

详细源码与模拟边界见 [真实矩阵](<D:/github/tauri-navicat/src-tauri/tests/e2e_sync_live_matrix.rs>)、[精度/键/取消回归](<D:/github/tauri-navicat/src-tauri/tests/e2e_sync_regressions.rs>)、[模拟入口](<D:/github/tauri-navicat/src-tauri/tests/e2e_sync_simulated.rs>)、[模拟器说明](<D:/github/tauri-navicat/src-tauri/tests/support/sync_simulated/README.md>) 与 [串行执行器](<D:/github/tauri-navicat/tools/acceptance/sync/run-simulations.mjs>)。

本次原始失败证据继续保留： [边界修复前](<D:/github/tauri-navicat/tools/acceptance/sync/regressions-before-fixes.json>)、[首次 90 场景实库基线](<D:/github/tauri-navicat/tools/acceptance/sync/live-baseline-90.json>)、[108 场景修复前](<D:/github/tauri-navicat/tools/acceptance/sync/live-108-before-final-fixes.json>)、[被拒绝作为新源码认证的缓存运行](<D:/github/tauri-navicat/tools/acceptance/sync/live-cached-build-rejected.json>)、[ODBC 二进制 ABI 探针](<D:/github/tauri-navicat/tools/acceptance/sync/odbc-binary-probe.txt>)。缓存运行与探针不能冒充最终全套同步结果。
