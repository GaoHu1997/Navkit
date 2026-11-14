# 进行中

`docs/active/` 只放**现在正在做**的东西。开发完成即归档：结论并入
[`../completed/PRODUCT-STATE.md`](../completed/PRODUCT-STATE.md)，过程文档删除。

## 当前内容

| 文档 | 内容 | 状态 |
|---|---|---|
| [性能方案](<navkit-data-pipeline-performance.md>) / [实施状态](<navkit-data-pipeline-performance-status.md>) / [用户审阅对比表](<navkit-performance-comparison.md>) | 七类数据链路、负优化识别与逐项保留决策 | 审阅准备：本地原生路径、COPY/一致备份显式模式、SQLite分段与准入已开发；diff负收益不默认，实库/全负载验证边界明确，待最终统一回归和用户决定 |
| [可用内存预算差异比较](<navkit-adaptive-diff-memory-budget.md>) / [实施与结论](<navkit-adaptive-diff-memory-results.md>) / [速度内存实测表](<navkit-adaptive-diff-memory-measurements.md>) | 自动/手动共享预算、预算内内存索引、必要溢写和82次独立进程对照 | 一期已开发并验证；窄表足额时间相近、小/宽表仍有回退，待用户决定保留；不默认开启auto、不提交 |
| [execution-log-console-ui.md](execution-log-console-ui.md) | 导入 / 数据同步 / 运行 SQL 文件共用的执行日志台：布局、条目折叠/展开、双段进度、落地改动 | 设计完成，待实施 |
| [全功能桌面验收报告与用户手册-20261112.md](全功能桌面验收报告与用户手册-20261112.md) | 真实桌面全功能验收（79 用例 / 186 截图）：报告结论 + 用户手册逐功能演示；2 处产品缺陷修复（含修改前后截图） | 已完成，结论已并入 `../completed/PRODUCT-STATE.md`（docx 版同目录） |

## 证据目录

- `evidence/manual-20261112/`：第 9 轮（最终）186 张逐步截图 + `report.json` + 运行日志；`bugfix/` 子目录为产品缺陷 #1 修改前截图。
- `evidence/manual-20261112-round8-backup/`：第 8 轮证据（保留原因是其中 `126-S09-DATAGEN-written.png` 为产品缺陷 #2 的修改前现场）。
- 报告引用的全部截图经脚本校验 0 缺失；其余未被引用的 `.log` 已随运行器清理。

## 归档规则

1. **开发完成即归档**，不等真机验收。代码落地 + 测试存在，就把结论写进
   `PRODUCT-STATE.md` 的能力行与「实现归档」表，然后删除过程文档。
2. 归档时标清验收状态：`✅` 有真实证据 / `⏳` 缺现场验收 / `🚧` 仍有代码缺口。
   标 `🚧` 的条目要在 `PRODUCT-STATE.md` §六「已知限制」里留一条待办，不能悄悄消失。
3. 只保留当前轮次与常驻契约。旧轮次报告在结论并入后删除，证据日志同理。
4. 需要留证据日志时才创建 `evidence/`，且只放被本目录文档按行号引用的日志。交付前扫描一次：

```bash
grep -rho 'evidence/[A-Za-z0-9._-]*\.log' docs/active/*.md docs/active/**/*.md | sort -u
```

不在结果里的 `.log` 一律删除。

## 新增一项工作

在 `docs/planned/` 写计划（范围、非目标、验收标准、代码触点）→ 开工时整份移入本目录 →
完成后按上面第 1 条归档。不在本目录留「已完成待归档」的中间态。
