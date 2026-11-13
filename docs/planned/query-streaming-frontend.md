# 查询首屏流式前端接入（未开工）

## 范围与进入条件

后端已在独立 `perf/query-streaming` 分支提供 opt-in API；本任务尚未实施。进入条件：确认本次后端差异如何与主工作树并发改动合并，并获得前端接入授权。接入后才可能改善现有查询页面的首屏等待。

范围：查询页启用 `execute_sql_streaming`，显示临时只读预览，轮询 `execution_stream_snapshot` 到最终回执，最终成功后恢复原缓存分页/完整导出。

非目标：不重新执行用户 SQL；不把 preview 当成功或可写证据；不修改 DML/DDL 的安全、取消、事务与授权语义；不宣称 JDBC/ODBC/REST/隧道已获得流式首屏支持。

## 验收标准

- 同一份一次性 grant 只发送一次；禁止断网/超时后自动重放 SQL。
- 显示 provisional 首屏时状态仍为执行中，总行数尚未知；首个结果无行时允许暂时没有预览。
- 等待首帧 1 秒返回的是 pending ticket，不是数据库超时；前端继续轮询而不误报失败。
- 用户停止、关闭页、切连接、页面替换时正确处理后台任务与迟到回执；停止不声称已回滚。
- outcome 存在且最终成功后，使用 spool_id 读取当前页，不能把 metadata-only receipt 中的空 rows 当查询无结果。
- 失败/取消/Unknown 时撤销临时预览，不允许将不完整缓存导出为完整文件。
- 背景期间暂时禁用缓存翻页/导出与行编辑；成功后保留原执行身份、真实全量行数及只读/编辑证据校验。
- 查询页面及 mock/IPC/DOM 回归通过，并进行 Windows/Tauri + 专用 MySQL 的真实现场验收。报告分别记录首屏、全量完成、导出时间，核对与 DBX/DBeaver 的服务器、SQL、fetch 行数、冷热缓存与驱动参数是否一致。

## 代码触点

- [后端入口与鉴权](<../../src-tauri/src/commands.rs>)
- [后端流式回执契约](<../../src-tauri/src/execution_stream.rs>)
- [查询页](<../../src/core/App.ts>)
- [IPC API](<../../src/core/api.ts>)
- [旧执行控制器](<../../src/core/execution.ts>)
- [缓存分页模型](<../../src/core/queryResultPages.ts>)

## 已知后端边界

默认首屏 200 行，1..1000 可选，8 MiB 保守字节上限；只提前预览 result_index=0。完整导出仍受原有缓存配额（单结果 512 MiB / 全局 2 GiB）限制，配额或写盘失败必须明确拒绝“完整导出”。SQLite/PostgreSQL/MySQL 共享观察点，MySQL 已有回环协议回归；不能以模拟器通过替代用户实库性能验收。
