# NavKit 1.0.1

版本号 `1.0.1`。本次发布由 GitHub Actions 自动构建，覆盖 Windows（x64 / ARM64）、macOS（Apple Silicon / Intel）与 Ubuntu（x64）。

`1.0.1` 是 `1.0.0` 之后的第一个补丁版本，集中在 **ER 关系图重做**、**软件更新弹窗**、**表格设计器交互**三个方向的改进，
以及 openGauss / TiDB 兼容性修复。完整能力边界见[产品状态说明](<docs/completed/PRODUCT-STATE.md>)。

---

## 一、命名与下载

所有产物统一命名为：**`NavKit-<版本>-<系统>-<架构>-<类型>.<扩展名>`**

| 系统 | 架构 | 安装包 | 免安装便携版 |
|---|---|---|---|
| Windows | x64 | `NavKit-1.0.1-windows-x64-installer.exe` | `NavKit-1.0.1-windows-x64-portable.zip`（附单文件 `...-portable.exe`） |
| Windows | arm64 | `NavKit-1.0.1-windows-arm64-installer.exe` | `NavKit-1.0.1-windows-arm64-portable.zip`（附单文件 `...-portable.exe`） |
| macOS | arm64 | `NavKit-1.0.1-macos-arm64-installer.dmg` | `NavKit-1.0.1-macos-arm64-portable.zip` |
| macOS | x64 | `NavKit-1.0.1-macos-x64-installer.dmg` | `NavKit-1.0.1-macos-x64-portable.zip` |
| Linux | x64 | `NavKit-1.0.1-linux-x64-installer.deb` | `NavKit-1.0.1-linux-x64-portable.AppImage` |

> 版本号与架构以实际文件名与构建产物为准。免安装版解压即用，不写注册表、不写系统目录。

### 从 1.0.0 升级

1. **可直接覆盖升级**：解压新版本目录，用新 `NavKit`（或 `.app` / `.AppImage`）覆盖旧版本即可。
2. **务必保留 `data/` 目录**：连接、密码密钥、收藏、任务与备份都存放在程序同目录的 `data/` 中，升级**不会**删除它们。
   切勿删除 `data/key.bin`，否则已保存的连接密码将无法解密。
3. **数据格式兼容**：1.0.1 未引入破坏性数据格式变更，1.0.0 创建的 `data/` 可直接被 1.0.1 读取。

---

## 二、主要改动

### 1. ER 关系图核心重做（移植 参考实现 关系图能力）

- **ELK 分层布局**：移植 参考实现 ELK layered（Sugiyama）布局替代自研最长路径分层，交叉最小化 + 节点平衡 + NETWORK_SIMPLEX 分层；
  elkjs 动态 import 按需加载（独立 chunk），不拖慢启动；新增布局方向选择（LR/TB/RL/BT），
  大图（>400 节点或 >1200 边）自动退回连通分量聚簇网格布局；布局结果持久化，刷新不跳回旧位置。
- **虚拟外键自动匹配**：无物理外键时按命名约定（`user_id` / `userId` / `order_uuid`）+ 类型签名推断关系，
  单复数容错，物理外键优先、四元组去重；匹配面板支持逐条确认 / 忽略、多目标冲突标记，
  按「连接 + 库」持久化；推断边用虚线样式与物理外键区分。
- **边避障路由**：正交走廊枚举绕开中间表卡片，按障碍物边界动态生成通道；路由失败回退三段式正交折线，永远有线可画。
- **工具栏修复**：工具栏按钮禁止换行，空间不足时横向滚动。

### 2. 软件更新弹窗重做

- 打开即自动检查更新，移除手动「检查更新」按钮。
- 收敛为 checking / latest / available / downloading / ready / error 六状态机，
  界面文案、图标与按钮全部由状态推导；版本 chip 对比（已安装 → 最新版），更新说明按行渲染为列表。

### 3. 表格设计器与数据源兼容性

- 表设计器「选项」页默认值候选改为弹层选择、索引方法类型对齐、SQL 预览高亮。
- **TiDB 表设计读取修复**：物理表 ID 改读 `information_schema.TABLES.TIDB_TABLE_ID`（原 `TIDB_TABLES.TABLE_ID` 目录不存在），
  并修正 TiDB 暴露兼容 `@@server_uuid` 时被误套用原生 MySQL InnoDB 身份校验、导致表设计读取报
  `TABLE_MYSQL_PHYSICAL_IDENTITY_UNPROVEN` 的问题；ID 为空 / 歧义 / 非法时给出明确错误码。
- **TiDB 表结构保存明确拒绝**：TiDB 的 `GET_LOCK` / `LOCK TABLES` 是 noop 兼容功能，不能提供保存所需的服务端锁；
  读取、预览仍可用。同时移除旧版 `table_native` 实现。
- **openGauss 兼容修复**：openGauss 7.0（9.2.4 内核）没有 `to_jsonb(record)`（42883），
  PG 索引查询改用 `to_json(x)->>'indnkeyatts'`（PG 9.x / 10 / 11+ / openGauss 均可用），并保留 PG 10 的 `indnatts` 回退。

### 4. 传输 / 同步调整

- 协议与批量导入相关调整；命名空间错误测试适配 `loadChildren` 重抛行为。

---

## 三、工程改进

- **免安装便携包精简**：移除内置的 WebView2 在线引导程序（约 1.9 MB；安装包已通过 `embedBootstrapper` 内嵌，
  Windows 10/11 通常已自带运行时），便携包只保留 `NavKit.exe`、`data/` 与使用说明，减小下载体积。
- `README` 界面截图更新为当前版本。
- 移除一次性的表设计器截图脚本。
- `package-lock.json` 补齐 `elkjs`，与 `pnpm-lock.yaml` 保持同步。

---

## 四、系统要求与校验

与 1.0.0 相同：Windows 10/11（需 WebView2）、macOS 11+、Ubuntu 22.04+；
安装包与便携版不内置数据库驱动，首次连接按需下载。
每个产物均为 CI 构建生成，可自行计算 SHA-256 比对；更新清单 `navkit-win64.json` 内也带有 SHA-256。
已知限制与产品边界请查阅 [PRODUCT-STATE.md](<docs/completed/PRODUCT-STATE.md>)。

---

如果发现问题，欢迎在 [Issues](https://github.com/GaoHu1997/Navkit/issues) 反馈。
