# 同步协议模拟动态验收 v1

本目标确实调用产品 `synchronize` / `diff`，执行真实 INSERT/UPDATE/DDL 后完整回读，而不是匹配 SQL 字符串或返回固定行。三个测试均 `#[ignore]`，必须显式选择。没有运行厂商服务，所有报告固定 `vendor_server_status=NOT_TESTED`、`vendor_real_pass=0`。

推荐在工作区根目录运行 `npm run test:sync:simulated`：统一执行器会先清本项目 Cargo 旧产物，顺序执行六套、严格校验实际二进制/源码/DLL 哈希与 42 个场景数量，保留独立报告并综合检查每套退出码。详见[同步动态验收说明](<D:/github/tauri-navkit/tools/acceptance/sync/README.md>)。下列单项命令仅用于诊断，不应把最后一个命令的退出码当作六套全部通过。

## 手动诊断（Bash，工作区根目录）

手动诊断前也应 `cargo clean --manifest-path src-tauri/Cargo.toml -p navkit`，避免保留 mtime 的原子源码编辑被 Cargo 误判 Fresh。勿与已有 Cargo 任务竞争同一 target。不要用真实用户连接/密码。以下密码全部由 fixture 生成；ODBC 使用仓库公开的合成契约字符串。

```bash
export NAVKIT_PYTHON='C:/Users/gaohu/AppData/Local/Programs/Python/Python313/python.exe'
# 选择本轮独立的绝对目录；不要复用历史报告来宣称本轮通过。
export NAVKIT_SYNC_SIM_REPORT_DIR='D:/github/tauri-navkit/artifacts/sync-simulation-current-round'

cargo test --manifest-path src-tauri/Cargo.toml --test e2e_sync_simulated --no-run
cargo test --manifest-path src-tauri/Cargo.toml --test e2e_sync_simulated sync_simulated_tds -- --ignored --test-threads=1 --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --test e2e_sync_simulated sync_simulated_clickhouse -- --ignored --test-threads=1 --nocapture

export NAVKIT_SIM_ODBC_DRIVER='D:/github/tauri-navkit/tools/acceptance/simulation/odbc/runtime/navkit_sim_odbc.dll'
export NAVKIT_SIM_ODBC_BACKEND='D:/github/tauri-navkit/tools/acceptance/simulation/odbc/runtime/sim_duck_backend.dll'
status=0
for kind in odbc oracle dameng db2; do
  NAVKIT_SYNC_SIM_ODBC_KIND="$kind" cargo test --manifest-path src-tauri/Cargo.toml --test e2e_sync_simulated sync_simulated_odbc -- --ignored --test-threads=1 --nocapture || status=1
done
# 上述 status 应由外层执行器检查，不要仅采用最后一种协议的退出码。
echo "ODBC aggregate status=$status"
test "$status" -eq 0
```

默认 `NAVKIT_PYTHON=python`；报告目录缺省为系统临时目录下全新 UUID 目录并输出位置；ODBC kind 缺省 `odbc`。ODBC 两个 DLL 变量必须显式指定为现有绝对路径，不自动安装驱动、不降级到 JDBC、不跳过连接错误。本目标不构建 DLL。

## 场景与证据边界

- TDS：实际 tiberius 登录/RPC/结果 token，SQLite 可变存储；SQLite→TDS、TDS→SQLite、TDS→TDS 的结构/数据/重建/替换/列映射/where/diff/增量与幂等；二进制两方向；约束失败后的已提交行；确定性第2次 INSERT 挂钩取消中间/最后语句。严格拒 SQL Server 非法 `X'hex'` 与 `LIMIT`；有限 MAX/VARBINARY 编码支持，不认证完整 T-SQL/TLS/代码页/厂商 schema。
- ODBC：实际 Windows ODBC manager→现有 test-only facade/proxy→真实 DuckDB file。各 kind 三方向 full/diff；SQLite 源增量；Oracle/Dameng 专门主键 metadata 的反向增量；通用/Db2 路径缺主键 metadata 时明确拒绝非空增量；约束失败/部分提交。SQLite↔ODBC 二进制双向均核 `00FF1080`、NULL、空 BLOB、`00017F80FEFF41C3A9F09F9982` 四行：产品完整回读必须为 `0xHEX`；SQLite 另核 `typeof` 与原始 `Vec<u8>`，DuckDB 另核 `typeof`/`hex`，禁止用文本十六进制替代真实 BLOB。fixture 建数使用 DuckDB 的 `from_hex`，仅是测试数据设置。代理仅翻译有限协议族 SQL，不是各厂商实库，不能用其通过认证 Oracle/Db2 方言或类型。
- ClickHouse：实际 reqwest HTTP/Basic Auth/JSONCompactEachRowWithNamesAndTypes；SQLite 可变存储与实际 DDL metadata。三方向 non-NULL full/diff；NULL/Nullable 分开；先核增量 insert，再同时留下一个缺失行与一个 changed 行，要求不支持的标准 UPDATE 在任何 INSERT 前拒绝，完整目标不变、diff 缺失/改变仍各一、任务 processed=0；缺源 PK metadata 拒绝；replace 正确轻量 DELETE WHERE 的期望行为回归。严格拒普通 UPDATE、String NULL、无 WHERE DELETE；不模拟 MergeTree、异步 mutation、精确 decimal、背压或厂商反斜杠字面量。
- NULL/binary/最后语句取消/ClickHouse replace 场景断言正确产品行为；未修的产品缺陷会让测试明确 FAIL，不改为 expected-PASS/XFAIL。失败仍继续后续独立 UUID 场景。

运行上述六个 suite 的预期场景数为 **42**：TDS 8、ClickHouse 8、通用 ODBC 7、Oracle profile 6、Dameng profile 6、Db2 profile 7。四种二进制值在每方向的一个 case 内核验，ClickHouse 混合拒绝仍合并在原增量 case 中；数量只是完整性检查，实际结果由本轮报告与退出码决定。

每个 case 30秒 deadline，loopback ready 20秒。开始 case 前、结果后落 JSON checkpoint。结束显式 close 核心池、kill/reap 自有 Python 子进程，验证并仅删除自己创建的 UUID 临时目录；清理失败也导致 FAIL。报告保留实际读回、diff/任务状态、错误与 wire SQL。进程被操作系统强制终止时至多保留 RUNNING checkpoint，不能声称完成清理。ODBC 原生 blocking 调用若驱动不返回，Rust future 超时不等于强制取消驱动，可能需要主代理外层执行器收割进程；此时报告/cleanup 必须按实际状态解读。

仅 fixture 的离线自检（无监听服务、不执行产品同步）：

```bash
"$NAVKIT_PYTHON" src-tauri/tests/support/sync_simulated/validate_fixtures.py
```

离线脚本通过只是模拟器基础可变 SQL/编码/严格拒绝的自检，不是任何产品同步或厂商实测结果。
