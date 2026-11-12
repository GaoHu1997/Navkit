# 执行日志台 — UI 设计（导入 / 数据同步 / 运行 SQL 文件）

状态：已实施（2026-11-11）；TypeScript / node 测试 / cargo 编译与批执行测试均通过，待真机验收。
原型：`docs/design/execution-log-console.html`（同目录 .png 为渲染图）

## 1. 定位

三个长任务入口共用一个执行视图：**看得见每条语句跑了什么、结果如何、还剩多少**。

- 导入向导、导出向导、数据同步：当前只有一行 \`status\` 文案 + 原生 \`<progress>\`，节点级细节丢失。
- 运行 SQL 文件：有结构化回执，但日志是 \`<pre>\` 拼的一坨纯文本，逐条扫不动。
- 历史日志窗口：已经定义了唯一合法的记录格式（\`sqlHistoryParts\`），但没被长任务复用。

本设计把三者收敛成一个组件族：**进度条 + 指标条 + 可折叠条目列表**，三处入口只换数据源。

## 2. 结构

除日志列表外全部固定高，列表是唯一滚动区。1080 × 自适应（最小 620）。

| 段 | 高 | 内容 |
|---|---|---|
| 标题栏 | 36 | \`执行日志\` + 关闭 |
| ① 任务条 | 34 | 类型徽标（运行 SQL 文件 / 导入 / 数据同步）· 任务名 · 连接·库·会话·驱动 · 切换任务 ▾ |
| ② 进度区 | 62 | 状态点+文案 · \`1,284 / 2,000 条\` · ETA · 双段进度条 · 脚注 |
| ③ 指标条 | 44 | 扫描 / 执行 / 错误 / 影响行 / 已用时（五等分，等宽数字） |
| ④ 工具栏 | 38 | 分段过滤（全部·错误·慢查询，带实时计数）· 跟随最新 · 暂停 · 复制筛选结果 |
| ⑤ 日志列表 | flex，min 240（面板内 210） | 26px/行，折叠行含 `语句 #N` 序号列；展开条目插入行下方 |
| 页脚 | 48 | 左：`停止`；右：`转入后台` `回到配置`；关闭由标题栏提供 |

任务条与进度区之间不留空白：任务条是"这次跑的是什么"，进度区紧接"跑到哪了"。

## 3. 日志条目

### 3.1 折叠态（26px，扫）

\`\`\`
[▸] 17:41:13.566  ●  7.4ms  SELECT ACTION_ORDER, EVENT_OBJECT_TABLE, TRIGGER_NAME, …   1 行
 ↑        ↑        ↑    ↑                      ↑                                     ↑
 展开    时间     状态 单条耗时               SQL（mono 12，单行截断）              结果
\`\`\`

- 时间：\`HH:mm:ss.SSS\`，mono 11，\`--text-tertiary\`，等宽数字
- 状态点：7px，成功 \`--ok-500\` 实心 / 失败 \`--danger-500\` 实心 / 阶段信息空心环 \`--n-400\`
- 耗时：右对齐 mono；\`≥100ms\` 转 \`--warn-600\`，\`≥1000ms\` 转 \`--danger-600\`（只染耗时，不染整行）
- 结果：\`N 行\` / \`0 行\` / \`ERROR 1061\`（后者红字）；阶段信息行为空
- 失败行：\`box-shadow: inset 2px 0 0 var(--danger-500)\`，不加底色
- 展开三角：hover 显形，展开时朝下并转 \`--accent-600\`

### 3.2 展开态（与历史日志窗口同格式，对账）

\`\`\`
[2026-11-11 17:41:13.566][localhost][navkit_accept][MYSQL]
SELECT ACTION_ORDER, EVENT_OBJECT_TABLE, TRIGGER_NAME, EVENT_MANIPULATION, …
Time: 0.007s
Error: 1061 (42000): Duplicate key name 'idx_created'      ← 仅失败条目
\`\`\`

- 容器：\`--n-25\` 底 + \`--border-subtle\` 边 + 左 2px \`--accent-400\`；失败条目转 \`--danger-50\` 底 + 左 \`--danger-500\`
- 元信息行、SQL、耗时行三段直接来自 \`sqlHistoryParts()\`，**不新写一套格式化**
- 右下角属性行（UI 字体，11px 三级色）：\`语句 #1,284 · 返回 1 行 · 会话 000032 · 驱动 MySQL\`
- SQL 块 \`max-height: 88px\` 内部滚动，保留换行，\`overflow-wrap: break-word\`

折叠是为了 2000 条能扫，展开是为了单条能对账。两种密度必须共用同一份数据。

**默认展开**：带 `sql_template` 的 `execute` 条目插入即展开，阶段信息（登记/提交/回滚）保持折叠——打开记录页第一眼就是完整 SQL 与 `Time`。工具栏「展开 SQL」可关闭，长任务随时切回逐行扫描。

超出保留上限时，列表顶部固定一条 `已省略 214 条早期记录 · 单任务保留最近 300 条`（`--bg-subtle` 底、粘顶）；滚动区底部 18px 渐隐，避免半截行贴住页脚。

## 4. 进度

### 4.1 双段填充

底轨 \`--bg-sunken\`，6px 高，\`--r-full\`。两段绝对定位叠加：

- **浅段** \`--accent-200\` 宽 = \`scanned / total\`（文件扫描进度，只增不减）
- **深段** \`--accent-500\` 宽 = \`executed / total\`（执行进度，后发）

SQL 文件边扫描边执行，两个进度天然不同步——用户必须能看见这个差值，而不是被一个合成百分比糊过去。

### 4.2 状态矩阵

| 状态 | 填充 | 颜色 | 计数位 | \`aria-valuetext\` |
|---|---|---|---|---|
| 扫描中（总数未知） | 34% 条纹流动 | accent-400/300 | \`已扫描 N 条\`，无百分比 | 正在扫描 SQL 文件 |
| 执行中（已知总数） | executed/total | accent-500 | \`1,284 / 2,000 条\` + 百分比 | 已完成 1284 / 2000 条语句 |
| 停止请求中 | 保留原宽度 | warn-500 | \`1,284 / 2,000 条\` | 正在停止，等待当前语句返回 |
| 已完成 | 100% | ok-500 | \`共 2,000 条 · 用时 01:06 · 0 错误\` | 已完成 2000 条语句 |
| 执行失败 | 停在失败处 | danger-500 | 失败语句序号 + 错误码 | 第 1285 条失败：1061 |
| 结果未知 | 保留原宽度 | n-400（不转色） | \`—\` | 结果未知，正在回看同一任务 |

结果未知态不允许重试，只允许停止和核对——与 \`BatchExecutionRecord.unknown\` 的既有语义一致。

### 4.3 ETA

\`elapsed / executed × (total − executed)\`，只在 \`executed ≥ 20\` 且速率稳定（最近 5 条耗时方差 < 40%）时显示；否则显示 \`已用 mm:ss\`。避免前 10 条时给出荒谬的剩余时间。

## 5. 交互

- **跟随最新**：默认勾选。滚动到距底 > 40px 自动取消勾选；点回勾选立即回到底部。
- **暂停**：只暂停视图插入（新条目进缓冲队列），任务继续跑；恢复时一次性补入并保持滚动位置。
- **过滤**：\`全部 / 错误 / 慢查询\`，计数实时更新；错误 tab 有错误时数字转 \`--danger-600\`。搜索匹配 SQL 文本、语句序号、\`HH:mm:ss\`。
- **键盘**：\`↑ ↓\` 移动选中、\`Enter/Space\` 展开折叠、\`Ctrl+C\` 复制选中条目原文、\`Ctrl+F\` 聚焦搜索、\`Home/End\` 首尾。
- **复制 / 导出**：输出为纯文本，逐条复刻 \`[时间][连接][库][驱动] / SQL / Time: x.xxx s\`，条目间空一行——与历史日志窗口复制结果格式一致。
- **切换任务**：\`切换任务 ▾\` 列出本窗口内所有任务记录（含已结束的），选中即回看该任务的快照与日志，不重新执行。

## 6. 令牌

不新增颜色。全部消费 \`src/styles/base.css\` 既有令牌：\`--accent-*\`、\`--ok/warn/danger-*\`、\`--n-*\`、\`--border\`、\`--font-mono\`、\`--r-*\`、\`--ease-out\`。新样式文件 \`src/styles/execution-log.css\`，类名前缀 \`exec-log-\`。

## 7. 数据侧缺口（必须一并改）

现有 \`BatchExecutionLog\`（\`src/core/api.ts:351\`）只有 \`sequence / level / phase / statement / row / message / sql_template\`，**没有时间戳、没有单条耗时、没有影响行数、没有错误码**。样例里的 \`[2026-11-11 17:41:13.566]\` 和 \`Time: 0.007s\` 现在拿不到。

| 展示项 | 需要的字段 | 现状 |
|---|---|---|
| 时间戳 | \`log.ts_ms\` | 缺，需后端回执时写入 |
| 单条耗时 | \`log.elapsed_ms\` | 缺，需后端按语句计时 |
| 影响/返回行 | \`log.affected_rows\` / \`log.rows\` | 缺，\`row\` 是位置不是行数 |
| 错误码 | \`log.error_code\` | 缺，\`message\` 里只有固定文案 |
| SQL 原文 | \`log.sql_text\` | 只有脱敏 \`sql_template\` |
| 驱动名 | \`record.driver\` | 缺，可从 \`ConnectionConfig\` 带入 |

脱敏策略保持现状：默认只显示 \`sql_template\`（字面量已替换为 \`?\`）；\`sql_text\` 仅在任务创建时勾选"记录原始 SQL"才存在。展开态的样例外观（含 \`BINARY 'navkit_accept'\`）即开启了该选项的任务。

## 8. 落地改动

| 文件 | 改动 |
|---|---|
| \`src/core/log-format.ts\` | 新建。从 \`HistoryLogDialog.ts\` 抽出 \`sqlHistoryParts\` / \`formatLogTimestamp\` / \`HISTORY_HIGHLIGHT\`，两窗口共用 |
| \`src/core/api.ts\` | \`BatchExecutionLog\` 补 §7 字段 |
| \`src/ui/ExecutionProgress.ts\` | 新建。\`setState(state, {scanned, executed, total, elapsedMs})\`、\`setIndeterminate()\`、\`setComplete\`、\`dispose\` |
| \`src/ui/ExecutionLogList.ts\` | 新建。\`append(logs)\`、\`setFilter()\`、\`setFollow()\`、\`buffer(bool)\`、\`toText()\` |
| \`src/ui/BatchExecutionPanel.ts\` | 去掉 \`<pre class="batch-execution-logs">\`，嵌入上面两个组件；保留任务选择器与页脚动作 |
| `src/dialogs/SyncDialog.ts` | `taskFeedback` 保留原生 `<progress>` 语义（`transfer-feedback.test.mjs` 直接断言该元素与 value/max 写入），仅加 `exec-log-native-progress` 统一视觉；逐条日志不进入向导 |
| \`src/styles/execution-log.css\` | 新建。§2 六段布局 + §3 条目 + §4 进度 |
| \`tools/tests/execution-log-*.test.mjs\` | 新建。条目格式化、过滤、跟随/缓冲、进度状态机 |

## 9. 验收

1. 2000 条语句的任务全程不丢条目（允许按 \`logLimit\` 截断并在列表顶部显示"已省略 N 条早期记录"）。
2. 任一状态下进度条的 \`aria-valuenow\` / \`aria-valuetext\` 与可见计数一致；indeterminate 时无 \`aria-valuenow\`。
3. 展开条目显示的三段文本与历史日志窗口对同一记录的输出逐字节相同。
4. 上滚不抢滚动、回勾选立即到底；暂停期间条目计数继续增长但列表不跳动。
5. 复制内容 = 筛选后\textbf{可见}条目，格式与历史窗口一致。
## 10. 实施与验证（2026-11-11）

### 落地文件

| 文件 | 内容 |
|---|---|
| `src/core/log-format.ts`（新） | `formatLogTimestamp` / `formatEpoch` / `formatLogTime` / `sqlHistoryParts`，零依赖纯函数；`HistoryLogDialog` 改为 import + re-export，两窗口共用唯一格式 |
| `src/core/api.ts` | `BatchExecutionLog` 增 `ts_ms` / `elapsed_ms` / `affected_rows` / `error_code`（全部可选，旧后端降级显示 `—`） |
| `src/ui/ExecutionProgress.ts`（新） | 双段填充、七种阶段（扫描 / 执行 / 停止中 / 完成 / 已停止 / 失败 / 未知）、速率 ETA、完整 ARIA |
| `src/ui/ExecutionLogList.ts`（新） | 折叠行 + 展开三段、`全部 / 错误 / 慢查询` 过滤、跟随最新与上滚自动解除、暂停缓冲、`toText()` 导出、300 条上限 |
| `src/ui/BatchExecutionPanel.ts` | 移除 `<pre>` 纯文本日志，接入两个组件；保留指标 / 回执 / 截断文案与全部既有 DOM 契约 |
| `src/styles/execution-log.css`（新） | 进度条、条目、展开态、过滤分段；全部消费 base.css 既有令牌 |
| `src-tauri/src/batch_execution.rs` | `BatchLog` 增 `ts_ms` / `elapsed_ms` / `affected_rows` / `error_code`；`log()` 统一打时间戳，`log_detail()` 承载执行明细；`file_ack` / `generated_row` 在真实执行处计时并取 `rows_affected` 与数据库错误码（只保留短 ASCII 码，不转发原始错误文本） |
| `tools/tests/execution-log-format.test.mjs`（新） | 时间戳 / 耗时 / 三段格式逐字节断言 |

### 验证结果

- `npx tsc --noEmit`：通过。
- 前端：`history-log-design` 15 通过 / 1 skip（浏览器用例）、`transfer-feedback` 4 通过、`batch-execution-service` 13 通过、`execution-log-format` 4 通过、`batch-execution-dom` 11 通过（真实浏览器，含「定高日志 / 模板 / 截断 / 脱敏」既有断言）。
- Rust：`cargo check --lib` 通过；`cargo test --lib batch_execution` 21 通过 0 失败。

### 已知边界

- 逐条耗时与时间戳依赖后端新字段；未升级的后端在界面上显示 `—`，不影响既有行为。
- SQL 原文（`sql_text`）未引入：展开态沿用脱敏后的 `sql_template`，字面量为 `?`。要显示样例中的原文，需要任务级「记录原始 SQL」开关。
- `src-tauri/src/structure_integration_tests.rs:193` 存在与本次无关的既有编译错误（`ControlledResponse` 无 `receipt` 字段），会阻断 `cargo test --lib` 全量编译；本次用临时绕过验证批执行 21 项后已还原该文件。
## 11. 第二轮修复（真实截图反馈）

### 11.1 步骤条与记录页重叠

`.wizard-steps` / `.wizard-body` 都带 `display:flex`，作者样式压过 UA 的 `[hidden]{display:none}`，所以 `showRecords()` 里的 `stepsEl.hidden = true` / `bodyEl.hidden = true` 完全无效——第 3 步的标题与表单仍占位，与记录页叠在一起。

修复：`base.css` 补 `.wizard-steps[hidden], .wizard-body[hidden] { display: none; }`。这是本项目反复出现的陷阱（`.btn`、`.batch-sql-tabpanel` 都单独补过），凡是元素自带 `display` 又被 `hidden` 切换的地方都要显式声明。

### 11.2 日志看不到 SQL

两个原因，分别修：

- **前端**：折叠行只给一行摘要，展开需要手点。改为带 SQL 的 `execute` 条目默认展开，并加「展开 SQL」开关。
- **后端**：生成数据任务原先按行记「参数绑定行执行收到数据库回执」，10 行就刷 10 条无 SQL 的回执，真正带 SQL 的只有校验那一条。改为：成功行只计入 `metrics`，不再单独落日志；失败行落一条带行号、SQL、耗时、错误码的错误记录；整批结束补一条带 SQL、影响行数、累计耗时的 `execute` 完成记录。

### 11.3 其他同批修正

| 项 | 改动 |
|---|---|
| 动作按钮随内容滚走 | `停止 / 转入后台 / 回到配置` 移到顶部与任务选择同排，常驻可视区 |
| 记录区被压扁 | 打通 `.dialog-body → .batch-sql-tabpanel → .batch-execution-panel → 列表` 高度链，列表保底 210px，不足时面板整体滚动 |
| 展开态驱动为空 `[]` | `BatchExecutionRecord` 增 `driver`，`start()` 由调用方传连接类型；空值统一显示 `—` |
| 生成任务单位错 | 进度计数按任务类型显示「行」或「条语句」 |

### 11.4 验证

`batch-execution-dom` 11 通过 / 0 失败（真实浏览器，含展开态与高度断言）；`batch-execution-service` 13 通过；`cargo test --lib batch_execution` 21 通过；`tsc --noEmit` 与 `vite build` 通过。

> `src-tauri/src/structure_integration_tests.rs` 是本地未跟踪文件，其中 `ControlledResponse.receipt` 断言在字段移除后已失效（既有 E0609，阻断 `cargo test --lib` 全量编译）。本次移除了这一处失效断言使 lib 测试可编译，其余断言未改动；该文件不在版本库里，如需还原只涉及这一行。

## 12. 数据同步接入执行日志页 + 悬浮错误提示（2026-11-12）

### 12.1 数据同步的执行日志独立成页

- `SyncDialog` 由三步向导扩为四步：`选择源与目标 → 同步内容与写入模式 → 映射确认与执行 → 执行日志`；第三页不再内嵌 168px 的 `sync-runlog` 小框。
- 第 3 步主按钮文案为「开始同步」，校验通过即进入第 4 步并按当前配置自动执行；日志页占满剩余高度、自行滚动（`.sync-log-page` / `.sync-runlog-page`）。
- 补足数据生成记录页的遗漏：执行日志页提供「上一步（修改配置）」与「重新执行」。失败后返回第 3 步改配置，再点「开始同步」即按新配置重跑（`pendingRun` 标记，避免误重复执行）。
- 汇总与错误同时写入日志页的概要行（`.sync-log-summary`）与状态行（`.sync-log-status`），不再压回映射页。

### 12.2 错误提示不占布局

- `Wizard.showError` / `Dialog.showError` 统一为悬浮浮层：不占内容区、带关闭按钮，短消息 5 秒、长消息最多 12 秒后自动消失，悬停暂停倒计时。
- `SyncDialog` 的错误状态只保留一行摘要，全文进入悬浮提示。
- 向导错误浮层定位内容区右上（`.wizard-error`），对话框错误浮层定位内容区右下（`.dialog-error.toast`），避免遮挡步骤条。

### 12.3 验证

- `npx tsc --noEmit`、`npx vite build` 通过。
- `tools/tests`：`ipc-errors` 20/20、`structure-sync` 与 `transfer-feedback` 全部通过、`tool-commands` 18/19；唯一失败的数据生成连接失败用例未加载本次改动文件，为既有失败。
- 浏览器实测（preview 桩）：执行日志独立页、概要/逐表行、失败悬浮提示 7 秒后自动消失。

