# 数据同步动态测试报告

日期：2026-11-07（Asia/Shanghai）。结论：**当前源码在干净构建后，已完成下述真实数据库与状态化协议模拟测试；不是静态检查。**

## 1. 最终实际结果

| 验收项 | 结果 | 证据 |
|---|---:|---|
| 真实 SQLite/MySQL/PostgreSQL 同构＋异构矩阵 | **108/108 场景通过，0 失败/未执行** | [逐方向/场景/回读/清理](<D:/github/tauri-navicat/tools/acceptance/sync/live-report.json>) |
| SQL Server/TDS、ClickHouse、四种 ODBC profile 状态化模拟 | **42/42 场景通过，0 失败** | [六套汇总与实际二进制哈希](<D:/github/tauri-navicat/tools/acceptance/sync/simulated-final-clean-20261107/simulations-run.json>) |
| 新增真实 SQLite 精度/主键/取消/批量回归 | **13/13 场景通过** | [独立 SQL 回读结果](<D:/github/tauri-navicat/tools/acceptance/sync/regressions-report.json>) |
| 既有 SQLite 差异/增量集成测试 | **12/12 测试通过** | [核心回归日志](<D:/github/tauri-navicat/tools/acceptance/sync/core-final-clean-20261107.log#L423>) |
| 既有导入导出/同步验收 | **SQLite 1 项＋真实 MySQL/PostgreSQL 1 项通过** | [SQLite 日志](<D:/github/tauri-navicat/tools/acceptance/sync/core-final-clean-20261107.log#L451>)、[实库日志](<D:/github/tauri-navicat/tools/acceptance/sync/live-final-clean-20261107.log#L214>) |
| Rust 库普通回归 | **274 通过、0 失败、23 ignored** | [实际执行日志](<D:/github/tauri-navicat/tools/acceptance/sync/core-final-clean-20261107.log#L405>) |
| 当前 Node 辅助回归 | **167 通过、0 失败、0 skipped** | [日志](<D:/github/tauri-navicat/tools/acceptance/sync/ui-final-20261107.log>) |
| 已执行证据的一致性核对 | **7/7 通过，校验程序退出 0** | [机器运行记录](<D:/github/tauri-navicat/tools/acceptance/sync/sync-run-record-final-20261107.json>) |

场景数是独立测试场景集合，不是 Rust 测试函数数、SQL 调用数或行数；108 个矩阵场景和 13 个边界场景各由一个 Rust 测试函数汇集执行。条件性 ignored 没有计为通过，模拟没有计为厂商实库认证。

## 2. 同构与异构真实矩阵

实测版本：**SQLite 3.46.0、MySQL 8.0.44、PostgreSQL 16.4**。每格表示该方向 12 个场景均通过：

| 源 → 目标 | SQLite | MySQL | PostgreSQL |
|---|---:|---:|---:|
| SQLite | 12/12，同构 | 12/12，异构 | 12/12，异构 |
| MySQL | 12/12，异构 | 12/12，同构 | 12/12，异构 |
| PostgreSQL | 12/12，异构 | 12/12，异构 | 12/12，同构 |

- 3 个同构方向＋6 个异构方向，分别使用自建 UUID 命名空间、独立生产连接池与隔离 SQLite 文件；MySQL/PostgreSQL 位于同一台本机服务器，不宣称跨机器。
- 实际执行建表、仅结构后仅数据、替换、重建、增量插入/更新、过滤/映射、整库部分失败后继续与视图物化；验证非法选项不会修改目标。
- 源端用独立绑定参数建数，目标全部行/列用独立 SQL 回读核对：NULL/空字符串、中文/emoji、首尾空白、引号/反斜杠/换行、BLOB/空 BLOB、布尔、精确小数、微秒时间。
- 精度场景包含超过 2^53 的整数和 DECIMAL(38,20)，再次增量执行必须 **0 次写入**；复合 Unicode 主键覆盖空白与特殊字符。
- 已存在目标表的 DML 约束失败验证回滚；6 个 endpoint 加 SQLite 自有目录共 7 项清理检查全部成功。
- 本机已有真实服务，因此没有使用来历不明的公开可写数据库；只读公开库不能验证目标同步写入。

## 3. 无厂商环境时的动态模拟

六套均经过生产同步入口、实际协议/驱动和可变存储，不返回固定成功行：

| 测试路线 | 实际运行链路 | 通过数 |
|---|---|---:|
| SQL Server/TDS | tiberius→严格本机 TDS 服务→可变 SQLite | 8/8 |
| ClickHouse | 生产 HTTP 客户端→严格 HTTP/SQL 子集→可变 SQLite | 8/8 |
| 通用 ODBC | 真实 Windows ODBC manager→现有 facade→DuckDB 文件 | 7/7 |
| Oracle profile（ODBC） | 同上，Oracle 家族有限 SQL/metadata 翻译 | 6/6 |
| Dameng profile（ODBC） | 同上，Dameng 家族有限 SQL/metadata 翻译 | 6/6 |
| Db2 profile（ODBC） | 同上，Db2 家族有限 SQL/metadata 翻译 | 7/7 |

- ODBC 双向核对四行：`00FF1080`、NULL、空 BLOB、`00017F80FEFF41C3A9F09F9982`；除了 `0xHEX` 回读，还独立核 SQLite 原始字节/存储类型及 DuckDB 实际 BLOB 的 `typeof`/`hex`，不接受十六进制文本冒充字节。
- SQL Server/TDS 验证双向二进制，以及真实挂钩暂停在中间/最后一次写入时取消；任务必须返回取消，而已自动提交的两行保留，不伪造回滚。
- ClickHouse 验证 Nullable、合法 DELETE WHERE、增量插入；同时存在缺失行与变化行时，不支持的 UPDATE 必须在任何 INSERT 前拒绝，完整目标不变，processed=0。
- 通用 ODBC/Db2 缺可靠源主键 metadata 时，增量比较明确拒绝且目标不变；这属于安全预期通过，不是宣称支持该路径的增量更新。
- 六套自有连接、Python 子进程和 UUID 目录清理全部成功。两个现有 ODBC DLL 没有重建，前后哈希一致。

**真实 SQL Server、ClickHouse、Oracle、Dameng、Db2 厂商服务均未测试：`vendor_server_status=NOT_TESTED`、`vendor_real_pass=0`。ODBC profile 不倒算为当前默认 JDBC 路线已验证。**

## 4. 动态发现及修复

首次新增 SQLite 边界测试曾真实执行出 **3/13 通过、10/13 失败**；普通实库扩展矩阵曾 **107/108**；预修复模拟曾 **29/38**。原始失败保留，没有改为 XFAIL 或降低比较标准。模拟后续新增双向四值二进制场景后完整数为 42，前后不是同一个大小的用例集。

修复包括：

1. **精确比较**：用 BigDecimal 代替 f64；考虑实际目标数值类型，消除 TEXT→MySQL DECIMAL 补零造成的重复 UPDATE；非数值文本保留有意义的空白。
2. **主键安全**：结构化编码避免空白/分隔符碰撞；要求完整复合主键映射，拒绝不能安全定位的 NULL/重复键；BLOB 主键 WHERE 按真实类型生成字面量。
3. **无损二进制**：SQL Server 使用合法 `0xHEX`、不再 UTF-8 宽容解码任意字节；ODBC 按 SQLDescribeCol 类型码走 C_BINARY，保留 NULL/空 BLOB，并传递源列类型以免反向同步映射成 TEXT。
4. **取消与错误语义**：整库最后一表取消必须传播；通用通道最后一条执行后再检查取消；只在真正事务回滚后描述回滚，自动提交通道明确告知已执行写入可能保留。
5. **ClickHouse 方言与安全预检**：Nullable 包装、DELETE WHERE，以及不支持的 UPDATE 在任何写入前拒绝。
6. **验收真实性**：修正旧测试缺字段导致的编译失败；去掉硬编码实库密码和连接失败静默通过；显式运行实库并保留各目标退出码。

ODBC 根因先通过真实 Windows ODBC ABI 探针核实：二进制列 SQLDescribeCol 返回 `SQL_VARBINARY=-3`，C_BINARY 返回正确字节，C_CHAR 返回转义文本；没有把 fixture 行为未经调查归咎产品。[完整探针](<D:/github/tauri-navicat/tools/acceptance/sync/odbc-binary-probe.txt>)是根因证据，不算新增同步场景。

## 5. 防止旧编译缓存冒充修复认证

原子源码编辑保留 mtime 时，Cargo 曾把新内容误判 Fresh：运行报告读取了当前源码哈希，但执行的旧测试程序甚至遗漏新追踪文件。该轮没有当作最终新源码通过证据。

最终只清 `navkit` 项目生成的编译产物，再真实重编译。模拟程序首次构建耗时 2 分 05 秒；随后库测试重新编译 1 分 35 秒，实库测试编译 10.07 秒。报告记录实际执行二进制 SHA-256，而不是仅记录源码文件哈希：

- 模拟六套同一二进制：`a7f3ebe9287e65455724cf4bdc466ea5c9abbd1bcead99a32623c62a32156c15`。
- 真实矩阵二进制：`96738ae1b63d3c776446c18b9f8b53c90466e29d6dc6c544423908a4b4570fca`。
- 源码、执行二进制、现有 DLL、逐场景报告及日志哈希由[机器运行记录](<D:/github/tauri-navicat/tools/acceptance/sync/sync-run-record-final-20261107.json>)核对；校验 7/7 通过。

[边界修复前](<D:/github/tauri-navicat/tools/acceptance/sync/regressions-before-fixes.json>)、[90 场景基线](<D:/github/tauri-navicat/tools/acceptance/sync/live-baseline-90.json>)、[108 场景修复前](<D:/github/tauri-navicat/tools/acceptance/sync/live-108-before-final-fixes.json>)、[被拒绝作为新源码认证的缓存运行](<D:/github/tauri-navicat/tools/acceptance/sync/live-cached-build-rejected.json>)均继续保留。

## 6. 复现与明确未覆盖项

完整环境变量、权限、隔离方式和命令见[同步动态验收说明](<D:/github/tauri-navicat/tools/acceptance/sync/README.md>)。核心入口：

```bash
cargo clean --offline --manifest-path src-tauri/Cargo.toml -p navkit
npm run test:rust
npm run test:sync:live       # 先配置隔离测试账号
npm run test:sync:simulated  # Python；ODBC 另需现有 Windows 测试 DLL
```

本次直接测试当前源码编译的 Rust 同步引擎，不是 GUI 点击端到端或重新打包发行程序验收；Node 167 项仅作辅助回归。未认证跨机器网络故障、CDC、全厂商方言/TLS/认证、超大 LOB/流式读取或大规模吞吐。增量是全表主键比较、不删除目标独有行。MySQL DDL 会隐式提交，SQL Server/ODBC/ClickHouse 通用通道不承诺整批回滚。ClickHouse 普通 UPDATE 不支持，验证的是安全预拒绝。

最终构建/测试保留既有非致命 warnings；没有为这次验收修改无关驱动/监控模块以消除警告，也没有将历史 259/166 的普通回归快照改记为本轮结果。
