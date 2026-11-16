# NavKit 1.0.2

版本号 `1.0.2`。本次发布由 GitHub Actions 自动构建，覆盖 Windows（x64 / ARM64）、macOS（Apple Silicon / Intel）与 Ubuntu（x64）。

`1.0.2` 集中在 **查询页服务器分页**（本次最大的变化）、**表数据 Keyset 深分页**、**列侧栏字段排序** 与一批对话框布局修复。
完整能力边界见[产品状态说明](<docs/completed/PRODUCT-STATE.md>)。

---

## 一、命名与下载

所有产物统一命名为：**`NavKit-<版本>-<系统>-<架构>-<类型>.<扩展名>`**

| 系统 | 架构 | 安装包 | 免安装便携版 |
|---|---|---|---|
| Windows | x64 | `NavKit-1.0.2-windows-x64-installer.exe` | `NavKit-1.0.2-windows-x64-portable.zip`（附单文件 `...-portable.exe`） |
| Windows | arm64 | `NavKit-1.0.2-windows-arm64-installer.exe` | `NavKit-1.0.2-windows-arm64-portable.zip`（附单文件 `...-portable.exe`） |
| macOS | arm64 | `NavKit-1.0.2-macos-arm64-installer.dmg` | `NavKit-1.0.2-macos-arm64-portable.zip` |
| macOS | x64 | `NavKit-1.0.2-macos-x64-installer.dmg` | `NavKit-1.0.2-macos-x64-portable.zip` |
| Linux | x64 | `NavKit-1.0.2-linux-x64-installer.deb` | `NavKit-1.0.2-linux-x64-portable.AppImage` |

> 版本号与架构以实际文件名与构建产物为准。免安装版解压即用，不写注册表、不写系统目录。

### 从 1.0.1 升级

1. **可直接覆盖升级**：解压新版本目录，用新 `NavKit`（或 `.app` / `.AppImage`）覆盖旧版本即可。
2. **务必保留 `data/` 目录**：连接、密码密钥、收藏、任务与备份都存放在程序同目录的 `data/` 中，升级**不会**删除它们。
   切勿删除 `data/key.bin`，否则已保存的连接密码将无法解密。
3. **数据格式兼容**：1.0.2 未引入破坏性数据格式变更，1.0.0 / 1.0.1 创建的 `data/` 可直接被 1.0.2 读取。

---

## 二、主要改动

### 1. 查询页服务器分页（行为变化，请留意）

- **每页只执行当前页 SQL**：查询 SQL 分页改为服务器端分页，翻页时由后端生成当前页 SQL（含 LIMIT/OFFSET 包装），
  不再把全量结果先读入缓存；单页补齐受 32 MiB 内存预算约束，超限、无完整 spool 或单行读取返回 0 时明确报错并提示减小每页行数，
  不把截断当尾页、不跳过不可见行。
- **导出 = 重新查询全量**：只有点导出才付全量代价，且仅对"可安全重跑"的只读 SELECT/WITH 语句（递归只读判定，
  拒绝写 CTE / 嵌套写 CTE / `SELECT INTO`）；导出重查遇到活跃手动事务返回 `EXPORT_REQUERY_TRANSACTION_ACTIVE` 而不是重跑。
  导出会重新执行查询，结果可能与页面所见不同，界面已明确提示。
- **页数统计**：新增 `count_query_page` 能力，支持的分页查询展示总页数 / 总行数，不支持时显示原因。
- **安全加固**：分页与导出共用只读判定；Keyset 两接口绑定当前 namespace 并每次重新验证单列唯一主键与真实列类型；
  用户写了 LIMIT 的查询分页范围不超过该 LIMIT。
- **协议覆盖**：SQLite / MySQL / PostgreSQL / ClickHouse 启用服务器分页；SQL Server / ODBC / JDBC 暂不前置服务器分页，
  自动回退原全量路径并在界面显示原因。

### 2. 表数据 Keyset 深分页

- 表数据浏览连续翻页改用 Keyset（锚点续页），任意跳页保留 OFFSET；仅当存在单列唯一主键时启用，
  复合排序或无锚点自动回退 OFFSET，类型感知、带注入转义。

### 3. 列侧栏字段排序

- 查询/设计器列侧栏支持**拖动调整字段顺序**（仅当前标签页），并提供 Alt + ↑ / ↓ 键盘微调；
  拖拽状态在关闭弹层时清理，避免残留样式。

### 4. 对话框与控件布局修复

- **数据字典**：对话框加宽到 980px，「对象」步双栏（树 + 已选清单）改用 `minmax(0,1fr)`，
  长表名 / 长批注走省略号，不再把内容区撑出横向滚动条；已选清单支持滚动定位。
- SearchableSelect、popover、表格设计器、同步对话框等一批交互与样式打磨。
- 查询结果的复制、筛选、编辑语义随服务器分页重新界定：默认只对当前页生效。

---

## 三、工程改进

- 新增流式预览基准 / 取消证据测试（Rust 基准隔离门控），MySQL 取消 Unknown 诊断文档。
- `build.ps1` 资产清单正则兼容带点号的 chunk 文件名，避免校验误报。
- 查询分页后端 49 项测试（`query_pagination` / `requery` / `page` / `keyset` / `stream_preview_bench`），
  前端 `sql-completion-execution` 76 项（含 20 项分页专项）全通过。

---

## 四、系统要求与校验

与 1.0.1 相同：Windows 10/11（需 WebView2）、macOS 11+、Ubuntu 22.04+；
安装包与便携版不内置数据库驱动，首次连接按需下载。
每个产物均为 CI 构建生成，可自行计算 SHA-256 比对；更新清单 `navkit-win64.json` 内也带有 SHA-256。
已知限制与产品边界请查阅 [PRODUCT-STATE.md](<docs/completed/PRODUCT-STATE.md>)。

---

如果发现问题，欢迎在 [Issues](https://github.com/GaoHu1997/Navkit/issues) 反馈。
