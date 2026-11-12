# NavKit 1.0.0-pre 预览版

这是一个 **预览版（Pre-release）**，版本号 `1.0.0-pre`，用于提前体验与验证。
本次发布由 GitHub Actions 自动构建，覆盖 Windows（x64 / ARM64）、macOS（Apple Silicon / Intel）与 Ubuntu（x64）。

## 命名规则

所有产物统一命名为：**`NavKit-<版本>-<系统>-<架构>-<类型>.<扩展名>`**

- 系统：`windows` / `macos` / `linux`
- 架构：`x64` / `arm64`
- 类型：`installer`（安装包） / `portable`（免安装便携版）

## 下载说明

| 系统 | 架构 | 安装包 | 免安装便携版 |
|---|---|---|---|
| Windows | x64 | `NavKit-1.0.0-pre-windows-x64-installer.exe` | `NavKit-1.0.0-pre-windows-x64-portable.zip`（附单文件 `...-portable.exe`） |
| Windows | arm64 | `NavKit-1.0.0-pre-windows-arm64-installer.exe` | `NavKit-1.0.0-pre-windows-arm64-portable.zip`（附单文件 `...-portable.exe`） |
| macOS | arm64 | `NavKit-1.0.0-pre-macos-arm64-installer.dmg` | `NavKit-1.0.0-pre-macos-arm64-portable.zip` |
| macOS | x64 | `NavKit-1.0.0-pre-macos-x64-installer.dmg` | `NavKit-1.0.0-pre-macos-x64-portable.zip` |
| Linux | x64 | `NavKit-1.0.0-pre-linux-x64-installer.deb` | `NavKit-1.0.0-pre-linux-x64-portable.AppImage` |

> 版本号与架构以实际文件名与构建产物为准。免安装版解压即用，不写注册表、不写系统目录。

## 主要功能

- **多数据源管理**：MySQL / MariaDB、PostgreSQL、SQLite、SQL Server、Oracle、ClickHouse、Redis、MongoDB 等原生连接，以及 ODBC / JDBC / H2 通用连接。
- **数据库对象与表设计**：对象树浏览、表结构设计、字段/索引/外键编辑、结构同步。
- **SQL 与查询**：SQL 编辑器与补全、查询结果表格、筛选/排序/分页、结果导出。
- **数据导入导出与同步**：导入导出向导、数据同步、批量执行与执行日志。
- **辅助能力**：ER 图、数据字典、调度任务、服务器监控、历史记录。
- **便携模式**：配置、密钥、收藏与备份均保存在程序目录，连接凭据使用 AES-256-GCM 加密存储。

## 系统要求与运行依赖

- **Windows**：Windows 10/11（x64 或 ARM64，按所选包）。需要 WebView2 运行时：安装包内嵌引导程序，便携版 zip 内附 WebView2 离线引导程序。ODBC 使用系统自带驱动管理器。
- **macOS**：macOS 11 及以上（Apple Silicon 用 arm64 包，Intel 用 x64 包）。包内已内置 unixODBC 动态库，无需再安装 Homebrew 依赖。应用未做 Apple 公证，首次打开如被 Gatekeeper 拦截，请右键「打开」，或执行 `xattr -dr com.apple.quarantine /Applications/NavKit.app`。
- **Ubuntu**：Ubuntu 22.04 及以上 x64。`.deb` 已声明 `libwebkit2gtk-4.1-0`、`libgtk-3-0`、`libodbc2`、`libodbcinst2`，`sudo apt install ./NavKit-*.deb` 会自动装齐；AppImage 已内置 unixODBC 等运行库。
- JDBC / H2 连接需要 Java 11+（完整 JDK，含 `jdk.compiler`）；程序内可一键检测并安装托管运行时。

## 安装与运行

- **安装包**：Windows 运行 `.exe`，macOS 打开 `.dmg` 拖入「应用程序」，Ubuntu 使用 `sudo apt install ./NavKit-*.deb`。
- **便携版**：Windows 解压后运行 `NavKit.exe`（或直接用单文件 `...-portable.exe`）；macOS 解压后打开 `NavKit.app`；Linux 直接运行 `.AppImage`。数据保存在程序同目录 `data/`。

## 驱动与运行时说明

- 安装包与便携版均 **不内置任何数据库驱动**（避免安装包体积膨胀与驱动授权问题）。连接数据库时由应用在首次使用时 **按需下载** 对应驱动到本地数据目录（来源为本项目自建的 Navkit-drivers 镜像）。
- 各平台已确保 **ODBC 运行时** 可用：Windows 使用系统 ODBC；`.deb` 自动声明 `libodbc2` / `libodbcinst2`；AppImage 与 macOS 包内置 unixODBC。

## 校验

每个产物均为 CI 构建生成，可下载后自行计算 SHA-256 校验值比对（GitHub Release 页面会显示大小与上传时间）。

## 已知限制

- 这是预览版，接口与数据格式仍可能调整，请勿用于生产关键数据，升级前请备份。
- 部分数据库类型依赖本机驱动或厂商服务，需要在联网且具备相应驱动的环境中使用。
- Windows ARM64、macOS（Intel/Apple Silicon）与 Linux 版本均来自自动化构建，如遇平台相关问题请提交 Issue 并附系统版本与日志。

---

如果发现问题，欢迎在 [Issues](https://github.com/GaoHu1997/Navkit/issues) 反馈。
