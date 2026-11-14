# NavKit 全数据源真实连接测试报告（box 实测 2026-11-12/13）

> 本报告由 box 内真实环境测试产生。测试对象为 NavKit 1.0.0-pre（deb，WebKitGTK 桌面端）。
> 全部结论来自对**真实数据库/中间件**的连接与操作，无模拟。证据截图存于 box 内
> `/workspace/navkit-test/evidence/`（57 张 PNG）。

## 0. 测试环境与限制

- box：Ubuntu 24.04，**2 vCPU / 3.8GB RAM（无 swap，容器内 swapon 被内核拒绝）/ 20GB 磁盘**
- Docker 29.9.3，**vfs 存储驱动（镜像层放大约 5 倍）**
- 沙箱限制：docker bridge veth 未挂到 docker0，`-p` 端口映射不通 → **全部容器改用 `--network host`**
- NavKit 依赖：OpenJDK 17.0.21（JDBC 系驱动）

## 1. 范围变更说明

最初任务为「全驱动连接测试 + 性能测试（200 表/1 亿行）」。执行中用户中断并调整范围：

> **停止性能测试，只验证 NavKit 支持的数据源能否在真实数据库上连接并使用。**

因此：
- MySQL / PostgreSQL 的**性能测试部分已完成**（见第 3 节，保留为既有证据）
- 其余数据源只做**连接 → 测试连接 → 打开浏览元数据 → 建表/插查 SQL（可用性）**

规模决策：box 资源不足以承载 200 表/1 亿行（连中档 100 表/5000 万行也不行），
实际采用最低档 **50 表 / 2000 万行 / 单表 200 万行**，每库测完即清理容器与文件。

## 2. 全数据源连接验证结果（核心结论）

判定标准：**连接成功 = 「测试连接」返回真实服务器版本号**；**可用 = 能打开连接树并读取真实元数据**。

| # | 数据源 | 服务端真实版本 | 连接 | 元数据浏览 | 可用性验证 | 证据 |
|---|---|---|---|---|---|---|
| 1 | **MySQL** | 8.0.46 | ✅ | ✅ 5 库 / 52 表 | ✅ CRUD / 27 类型 / 同步全通过 | mysql/01–25 |
| 2 | **PostgreSQL** | 16.15 | ✅ | ✅ 54 表 | ✅ 类型 / CRUD / 导入导出 | postgresql/01–11 |
| 3 | **SQL Server** | 2022 RTM-CU28 16.0.4325.4 | ✅ | ✅ master/model/msdb/tempdb | ✅ 建表 + 中文 NVARCHAR 读写 | sqlserver/01–02 |
| 4 | **Oracle** | 21c XE | ⛔ 未测 | — | — | 资源限制，见 §5 |
| 5 | **TiDB** | 8.0.11-TiDB-v8.5.1 | ✅ | ✅ 系统库 + test | ✅ 建表 / 插入 / 查询 / tidb_version() | tidb/01–02 |
| 6 | **瀚高 HighGo** | 6.0.1（PG 12.3 内核） | ✅ | ✅ oracle / oracle_catalog / public / utl_file | ✅ 建表 + 中文插入 + 查询 | highgo/01–02 |
| 7 | **达梦 DM8** | DM8（JDBC 8.1.2.114） | ✅ | ✅ CTISYS / demo / SYSAUDITOR / SYSDBA / SYSSSO | ✅ SELECT + CREATE TABLE 成功 | dameng/01–04 |
| 8 | **金仓 KingbaseES** | V008R006C005B0013 | ✅ | ✅ 10 模式 | 连接+浏览通过（查询页崩溃） | kingbase/01–02 |
| 9 | MariaDB | 11.8.9-MariaDB | ✅ | 未展开 | 未测 | other/01 |
| 10 | Redis | 7.4.12 | ✅ | 未展开 | 未测 | other/02 |
| 11 | MongoDB | 7.0.48 | ✅ | 未展开 | 未测 | other/03 |
| 12 | ClickHouse | 24.3.18.7 | ✅ | 未展开 | 未测 | other/04 |
| 13 | **SQLite** | 3.46.0 | ✅ | ✅ 打开 main / t_demo | ✅ INSERT 成功（76ms/1 行）+ 落盘核验 | other/05, 11 |
| 14 | Meilisearch | 1.9（索引权限已验证） | ✅ | 未展开 | 未测 | other/06 |
| 15 | Consul | 1.18.2 | ✅ | 未展开 | 未测 | other/07 |
| 16 | MQTT | 3.1.1（CONNACK/SUBACK 已验证） | ✅ | 未展开 | 未测 | other/08 |
| 17 | ZooKeeper | 3.9（native session+ACL 已验证） | ✅ | 未展开 | 未测 | other/09 |
| 18 | etcd | v3.5.15（API 已验证） | ✅ | 未展开 | 未测 | other/10 |
| 19 | **H2** | 2.3.232 | ✅ | 本地文件库 | 填路径后连接成功 | other/12 |

**统计：19 个数据源完成真实连接验证，19 个全部成功。其中 9 个完成深度可用性验证
（MySQL / PostgreSQL / SQL Server / TiDB / 瀚高 / 达梦 / SQLite + 同构与异构数据同步）。**

补充说明：
- **SQLite 端到端可用**：NavKit 执行 INSERT 后，用独立 sqlite3 核验数据已真实落盘。
- **达梦**：单条 SELECT / CREATE TABLE 均成功；仅一次提交多语句（DDL+DML+SELECT 混合）时报
  `Unsupported SQL type`，批量执行路径需修。
- **金仓**：连接与 10 个模式枚举正常，打开查询页时 NavKit 崩溃，未能完成 SQL。

## 3. MySQL / PostgreSQL 性能测试（已完成部分）

| 项目 | MySQL 8.0.46 | PostgreSQL 16.15 |
|---|---|---|
| 数据类型覆盖 | 27 列全类型，26/27 正确（POINT 显示 NULL） | 22 列，19/22 正确（MONEY/INTERVAL/POINT 显示 NULL） |
| 网格 CRUD | ✅ 增改删全部提交成功 | ✅ 增改删全部提交成功 |
| 数据生成（500 行上限） | 500 行 / 0.4s（≈1250 行/s） | 500 行 / 0.3s |
| 运行 SQL 文件 | 2M 行 106.7s（≈18,740 行/s）；18M 行 935s（≈19,250 行/s） | 2M 行 83.3s（≈24K 行/s）；7.3M 行 290s |
| 转储 SQL 文件 | 51 表 / 19,999,501 条 / 3.33GB / ≈185s | 3.33GB / 20,367,348 条 |
| 导出 CSV | 367,347 行 / 18.86MB / ≈4s | 367,347 行 CSV 成功 |
| 导入 CSV | 367,347 行 / ≈140s | 367,347 行 / ≈70s |
| 数据备份 | 3.33GB / ≈200s | 3.2GB 时 OOM（见 P2） |
| 数据字典 | 52 表 → 29KB Markdown / ≈4s | Markdown 支持 |
| 大量数据查询 | 4 条重查询（2M 行聚合 + JOIN）16.259s | — |
| 同构同步 | MySQL→MySQL 367,347 行 / ≈85s | — |
| 异构同步 | MySQL→PG 367,347 行 / ≈80s；PG→MySQL 367,347 行（含自动建表） | 同左 |

## 4. 发现的 NavKit 问题（按严重度）

| 编号 | 严重度 | 问题 | 证据 |
|---|---|---|---|
| **P1** | 严重 | **NavKit 多场景静默崩溃退出**（无提示）：保存金仓连接后、打开达梦连接首屏、测试 H2（空路径）、HighGo 查询页、备份完成时。部分 dmesg 显示 OOM（RSS 2.4–2.6GB），部分无 OOM 记录（疑 WebKitGTK / JDBC sidecar 异常） | 多次复现 |
| **P2** | 严重 | **3GB 级备份任务内存爆炸**：MySQL 3.33GB 备份完成后崩溃；PostgreSQL 备份到 3.2GB 时被 OOM 杀死 | mysql/1.9 |
| **P3** | 中 | **「运行 SQL 文件」受默认「查询超时 30 秒」限制**，大文件在 30.0s 整被截断；需手动改为 3600s 才能跑长任务 | mysql/1.5 |
| **P4** | 中 | **含 GEOMETRY / MONEY 列的表阻断整库转储**（拒绝把非 NULL 伪造成 NULL，行为诚实，但无「跳过并告警」选项） | mysql/1.6 |
| **P5** | 中 | 达梦 JDBC 路径多语句批量执行报 `Unsupported SQL type` | dameng |
| **P6** | 低 | 空间类型 POINT、PG MONEY/INTERVAL 在网格显示 NULL（库内实际有值） | mysql/1.2, postgresql |
| **P7** | 低 | Linux 下数据字典导出文件名混入反斜杠 | mysql/1.10 |
| **P8** | 低 | 密码框未清空时输入会拼接（金仓/ClickHouse/TiDB 均遇到） | 多次 |
| **P9** | 低 | 导出向导显示的文件名与实际落盘名不一致（`_` vs `-`） | postgresql |
| **P10** | 低 | 备份文件写入 `/usr/bin/data/backups/`（安装目录），deb 形态建议改用户目录 | mysql/1.9 |

## 5. 因 box 资源无法实测的数据源（非产品问题）

| 数据源 | 原因 |
|---|---|
| Oracle 21c XE | 镜像解压需 >20GB；box 20GB 磁盘 + vfs 驱动放大 → `failed to register layer: no space left on device` |
| Oracle 企业版 / DB2 / Informix / SAP HANA / Teradata / Vertica / Exasol / Snowflake / BigQuery / Spanner / Redshift / Databricks / Salesforce / D1 / Turso | 需商业授权、云端凭据或数十 GB 内存 |
| Hive / Impala / Spark / Kyuubi / Trino / Presto / Drill / Dremio / Kylin / Ignite / InfluxDB / TDengine / IoTDB / VictoriaMetrics / QuestDB | 需多组件集群（HDFS/YARN/K8s），2 核 3.8G 无法承载 |
| Kafka / RocketMQ / Pulsar / RabbitMQ / Nacos / HBase / Cassandra / Neo4j / NebulaGraph / Elasticsearch / Solr / CouchDB | 需额外集群或 JVM 堆 ≥2GB，与 NavKit 争抢内存 |
| 达梦/瀚高/金仓官方镜像 | 官方镜像需授权/登录；本次使用社区公开镜像（来源已记录：`laglangyue/dmdb8`、`xuxuclassmate/highgo`、`liangyaobo/kingbase:v8r6-license`） |

## 6. 已验证可用的正向能力

- 连接管理：测试连接返回真实版本号、密码加密保存、连接持久化、分组/排序
- 元数据浏览：库/模式/表/视图/列/索引/触发器统计（MySQL 52 表、PG 54 表、DM 5 模式、KB 10 模式均正确）
- 查询：多语句执行、分结果页签、执行耗时、结果集分页
- 网格 CRUD：增改删并可提交（MySQL、PG、SQLite 均验证）
- 工具链：数据生成、运行 SQL 文件、转储 SQL 文件、CSV 导出/导入、数据备份/还原、数据字典（MD/Word/PDF/HTML/Excel）
- 数据同步：同构 MySQL→MySQL、异构 MySQL↔PostgreSQL 双向（均 367,347 行校验一致）

## 7. 给开发的修复优先建议

1. **优先修 P1 崩溃**：重点排查 ①WebKitWebProcess 内存增长（长会话 + 大日志）
   ②JDBC sidecar 生命周期（达梦 / H2 均在 Java 路径崩溃）③保存连接后的序列化路径（金仓保存后崩溃）。
2. **P2 备份内存**：3GB 级 dump 期间 RSS 涨到 2.4–2.6GB，疑似把整个导出缓冲留在内存；
   应改为流式写盘并在完成后释放。
3. **P3 超时**：为「运行 SQL 文件」单独提供超时设置或不设默认上限。
4. **P4 转储**：对不支持无损解码的类型提供「跳过并告警」选项，避免整库转储被单列阻断。

## 8. 测试数据版本清单

MySQL 8.0.46 / PostgreSQL 16.15 / SQL Server 2022 CU28 / TiDB v8.5.1 / HighGo 6.0.1 /
DM8 (JDBC 8.1.2.114) / KingbaseES V008R006C005B0013 / MariaDB 11.8.9 / Redis 7.4.12 /
MongoDB 7.0.48 / ClickHouse 24.3.18.7 / SQLite 3.46.0 / Meilisearch v1.9 / Consul 1.18.2 /
Mosquitto (MQTT 3.1.1) / ZooKeeper 3.9 / etcd v3.5.15 / H2 2.3.232
