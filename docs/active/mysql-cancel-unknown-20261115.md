# 本机 MySQL 取消终态回归未闭环

## 范围与现象

基线 `3b67b6120b1a0dc539bef5655ea06a031a9f9508`，release 测试二进制，真实 MySQL 8.0.44。性能任务仅发现并报告此失败，没有授权修改运行时。

[execution_tests.rs](../../src-tauri/src/execution_tests.rs#L553) 的 c1_mysql_real_cancel_environment 用 `SELECT SLEEP(30), 1`、100ms 应用预算验证定向取消；期望 TimedOut，实际 Unknown。

- [regression-live-mysql-1.log](../../artifacts/stream-preview-eval-20261115/regression-live-mysql-1.log#L17)：首次失败，0.46秒，基准自身拥有的 UUID 库。
- [regression-live-mysql-cancel-recheck.log](../../artifacts/stream-preview-eval-20261115/regression-live-mysql-cancel-recheck.log#L17)：独立 UUID 重跑同一断言再次失败；[regression-live-mysql-cancel-recheck.json](../../artifacts/stream-preview-eval-20261115/regression-live-mysql-cancel-recheck.json#L1) 核验测试进程退出后该库剩余厂商会话为0、库删除。
- [regression-live-mysql-2.log](../../artifacts/stream-preview-eval-20261115/regression-live-mysql-2.log#L1)：真实 SELECT1 流式终态1项通过。专项离线回归通过不覆盖本项失败。

这是实库回归缺口，尚未确认具体原因；Unknown 本身是产品的保守回执，不能直接推断数据库仍在执行，也不能降低断言把失败改写成通过。测试进程退出后零会话不能替代回执时刻的 resources_pending 证据。

## 建议诊断方案（另行任务，当前不改代码）

1. 在环境门控诊断测试中记录 result、confirmed、outcome.error、deadline.stop_confirmed 与 resources_pending，排查是控制回执、中断识别还是物理关闭导致 Unknown；禁止输出密码或 DSN。
2. 核对 [execution.rs](../../src-tauri/src/execution.rs#L491) 的 mysql_run 控制连接与固定 CONNECTION_ID，以及 drive / is_interrupted 实际返回。不能仅凭 elapsed 或成功发送 KILL 推断执行已停止。
3. 若确认产品缺陷，修复根因并保留未知结果防重放、资源排空和预览撤销；不通过放宽预算、修改共享服务器或降低断言掩盖问题。

## 验收标准

仅在另行明确授权的隔离 UUID 夹具实施修复。真实 MySQL 超时与用户取消均检查最终回执、停止确认、当时资源状态、预览/缓存撤销和后续 SELECT42；之后核验该库无残留会话并精确删除拥有的库。已有 SQLite、MySQL 模拟协议、流式服务与 query_spool 回归保持通过。根因未确认前不宣称可靠取消已完成厂商验收。

## 补充验收边界

主代理默认环境独立复跑出现11项失败，其中多项明确伴随缓存目录私有权限设置被Windows拒绝；三个范围合计47通过/11失败/4 ignored。这与上述真实MySQL取消Unknown不是同一已确认根因。被拒绝目录位于独立工作树测试产物下，未修改权限或重试读取被拒绝目录；审批不可用，权限机制未进一步诊断。见[独立复核](../../artifacts/stream-preview-eval-20261115/lead-review.json#L1)。修复/发布前需分别消除或确认这两类验收缺口，不降级缓存私有权限保护或Unknown策略。
