# NavKit 1.0.0-pre 预览版

这是一个 **预览版（Pre-release）**，版本号 `1.0.0-pre`，用于提前体验与验证。
本次发布由 GitHub Actions 自动构建，包含 Windows、macOS 与 Ubuntu 三个平台的安装包与免安装便携版。

## 下载说明

| 平台 | 安装包 | 免安装便携版 |
|---|---|---|
| Windows x64 | `NavKit_1.0.0-pre_x64-setup.exe`（NSIS 安装包） | `NavKit-1.0.0-pre-windows-x64-portable.zip` |
| macOS（Apple Silicon / Intel，按构建机架构） | `NavKit_1.0.0-pre_<arch>.dmg` | `NavKit-1.0.0-pre-macos-<arch>-portable.zip` |
| Ubuntu / Linux x64 | `NavKit_1.0.0-pre_amd64.deb`、`NavKit_1.0.0-pre_amd64.AppImage` | `NavKit-1.0.0-pre-linux-x64-portable.tar.gz`（AppImage 亦可直接运行） |

> 文件名中的版本号以实际构建产物为准。免安装版解压即用，不写注册表、不写系统目录。

## 主要功能

- **多数据源管理**：MySQL / MariaDB、PostgreSQL、SQLite、SQL Server、Oracle、ClickHouse、Redis、MongoDB 等原生连接，以及 ODBC / JDBC / H2 通用连接。
- **数据库对象与表设计**：对象树浏览、表结构设计、字段/索引/外键编辑、结构同步。
- **SQL 与查询**：SQL 编辑器与补全、查询结果表格、筛选/排序/分页、结果导出。
- **数据导入导出与同步**：导入导出向导、数据同步、批量执行与执行日志。
- **辅助能力**：ER 图、数据字典、调度任务、服务器监控、历史记录。
- **便携模式**：配置、密钥、收藏与备份均保存在程序目录，连接凭据使用 AES-256-GCM 加密存储。

## 系统要求

- **Windows**：Windows 10/11 x64，需要 WebView2 运行时（安装包会自动处理；便携版附带 WebView2 离线引导程序）。
- **macOS**：macOS 11 及以上；安装包架构以文件名中的 `aarch64`（Apple Silicon）或 `x64`（Intel）为准。
- **Ubuntu**：Ubuntu 22.04 及以上 x64，需安装系统 WebKitGTK 等运行库（`.deb` 会自动声明依赖）。
- JDBC / H2 连接需要 Java 11+（完整 JDK，含 `jdk.compiler`）；程序内可一键检测并安装托管运行时。

## 安装与运行

- **安装包**：Windows 运行 `.exe`，macOS 打开 `.dmg` 拖入「应用程序」，Ubuntu 使用 `sudo apt install ./NavKit-*.deb` 或直接运行 `.AppImage`。
- **便携版**：解压后直接运行 `NavKit.exe` / `NavKit.app` / `navkit`，数据保存在同目录 `data/`。

## 校验

每个产物均为 CI 构建生成，可下载后自行计算 SHA-256 校验值比对（GitHub Release 页面会显示大小与上传时间）。

## 已知限制

- 这是预览版，接口与数据格式仍可能调整，请勿用于生产关键数据，升级前请备份。
- 部分数据库类型依赖本机驱动或厂商服务，需要在联网且具备相应驱动的环境中使用。
- macOS 与 Linux 版本为自动化构建产物，如遇平台相关问题请提交 Issue 并附上系统版本与日志。

---

如果发现问题，欢迎在 [Issues](https://github.com/GaoHu1997/Navkit/issues) 反馈。
