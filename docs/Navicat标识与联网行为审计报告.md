# Navicat 标识残留与联网行为审计报告

- 审计日期：2026-11-05（清理执行完成于同日）
- 审计范围：`D:\github\tauri-navicat` 全仓库（排除 `node_modules`、`src-tauri/target`）
- 审计方法：全文检索（含二进制 strings / UTF-16 扫描）、配置审查、图标资源哈希比对、Rust/TS 网络调用点逐一定位

> **清理状态（2026-11-05 更新）**：§2 所列 P0 项均已处理完毕，
> 完成后复扫全库（排除第三方克隆 `tools/acceptance/reference-transfer-*`），
> 唯一命中为项目目录名 `tauri-navicat` 本身，**无真实品牌残留**。
> 详见 §5「清理执行记录」。

---

## 一、总体结论（清理前基线）

| 项目 | 结论 |
|---|---|
| 应用主体标识（产品名/Bundle ID/窗口标题/关于框/exe 元数据） | ✅ 已全部品牌化为 **NavKit**，二进制内无 Navicat 字符串 |
| 发行物内 Navicat 残留 | ❌ **有遗留**（构建产物、便携版说明、lock 文件，共 7 处，见 §2.1） |
| 第三方专有资产 | ❌ **严重遗留**（Navicat 安装包/主程序/EULA/提取资源等 ~460MB，见 §2.3） |
| 图标知识产权 | ⚠️ **存疑**：图标脚本自述取材自 Navicat 安装包，与 README"原创绘制"矛盾（见 §2.2） |
| 联网上报 / 遥测 / 更新检查 / 激活 | ✅ **未发现**，前端被 CSP 硬性阻断外部请求（见 §3） |

---

## 二、Navicat 相关标识残留

### 2.1 发行物 / 用户可见残留（P0 — 发布前必须清理）

| # | 位置 | 内容 | 影响 |
|---|---|---|---|
| 1 | `dist/preview.html:5` | `<title>Navicat Premium 18 — 预览（模拟 IPC）</title>` | 会被 `frontendDist: ../dist` 嵌入 navkit.exe |
| 2 | `dist/preview-bootstrap.js:499-507` | mock `about_info` 返回 `product: "Navicat Premium 18"`、`copyright: "Copyright © 2026 Navicat"`、`publisher: "Navicat"`、`url/support: https://www.navicat.com…`、`dataDir: "D:/Navicat/data"` | 同上，已确认 `preview.html`/`preview-bootstrap` 路径字符串嵌入 exe 资产清单 |
| 3 | `dist/preview-bootstrap.js:393` | mock 返回本地路径 `D:/Navicat/data/backups/...` | 泄露开发机路径 |
| 4 | `public/preview.html:5`、`public/preview-bootstrap.js:500-507` | 同上内容的**源头文件**（Vite 复制进 dist） | 只清 dist 不清 public 会复发 |
| 5 | `preview.html:6`（仓库根） | `<title>Navicat Premium 18 (Preview)</title>` | 开发预览入口 |
| 6 | `dist-portable/README-portable.txt:1`、`assets/使用说明.txt:1` | 标题 **"Navicat Premium 18 复刻 — 绿色版"**；正文写 `navicat-clone.exe`（旧包名，实际产物是 `navkit.exe`） | **随绿色版分发给用户** |
| 7 | `package-lock.json:2,8` | `"name": "navicat-clone"`（`package.json` 已改名 `navkit`，lock 未同步） | npm script 输出 `navicat-clone@…` |

> 正式构建的 `dist/assets/index-*.js`（主程序 bundle）**不含**任何 navicat 字符串；残留集中在 preview 相关文件与说明文档。

### 2.2 图标（P1 — 知识产权风险，需人工定夺）

- `tools/make_icons.py:2-9` 文档字符串明确写着：
  > "Build the application icon set (src-tauri/icons/\*) **from the reference Navicat installer's own RT_GROUP_ICON / RT_ICON resources**"，且 `:119` 默认输入为 `navicat18_premium_cs_x64.exe`。
- `README.md:6` 却声称"**应用图标为原创绘制**"。两者矛盾。
- 实测核对：
  - `src-tauri/icons/*`（icon.ico / 各尺寸 PNG）的 MD5 与安装包、`reference_files/navicat.exe` 内 RT_ICON 原始资源**均不一致**；
  - 安装包自带图标实为 Inno Setup 纸箱图，非蓝色 N 标；
  - 应用图标（蓝色圆角方块 + 白色 N + 绿点）与 Navicat 品牌图标**视觉高度近似**。
- 结论：字节层面无法证明是直接提取，但（a）工具链脚本自述取材自 Navicat 资源，（b）图形与 Navicat 商标近似。**建议替换为有明确原创证据的新图标，并重写/删除 `make_icons.py` 的取材说明**，否则商业分发有商标侵权风险。

### 2.3 第三方专有资产残留（P0 — 法律风险，与本项目无关，应移出仓库）

| 位置 | 内容 | 规模 |
|---|---|---|
| `navicat18_premium_cs_x64.exe`（仓库根） | **Navicat Premium 18.0.5 安装包本体** | 195 MB |
| `reference_files/navicat.exe`、`navicat.pdf` | Navicat 主程序 + 官方手册 | 110 MB + PDF |
| `reference_files/eula.txt` | PremiumSoft Cybertech Ltd. 许可协议（含 "PREMIUMSOFT CYBERTECH LTD"、"Navicat Premium Lite"） | — |
| `reference_files/resource/httptunnel/ntunnel_mysql.php` 等 3 个 | Navicat 官方 HTTP 隧道脚本，含 navicat 标识 3-4 处/文件 | — |
| `reference_files/*.dll`（数十个） | Navicat 附带的运行库 | — |
| `reference_resources/delphi_strings.txt` | 从 Navicat 提取的字符串表，**navicat ×580**，含"激活码/取消激活/激活失败…请激活以继续使用 Navicat"等 DRM 文案 | — |
| `reference_resources/ui_strings.txt` | 提取的 UI 字符串，**navicat ×457**，PremiumSoft 字样 | — |
| `reference_resources/assets/` | `navicat_cloud_*.png` 等上百个 Navicat 云功能图标、`tnavicat*.bmp` 表单位图，`_index.txt` navicat ×141 | — |
| `reference_resources/EXTRACTED_MENU.txt`、`action_names.txt` 等 | 提取的菜单/命令名 | — |
| `tools/extract_*.py`、`innoparse.py`、`innoextract.py`、`dfm.py` 等 | 从 Navicat 安装包/二进制提取资源的脚本，注释中直接出现 Navicat（如 `extract_menu.py:1` "Recover the Navicat main menu tree…"） | — |
| `tools/_backup_App_concurrent.ts:1237,2059` | 备份源码含 `"代码段功能需要 Navicat Cloud 账户"`、`"官方文档：https://www.navicat.com/support"` | — |
| `tools/_tauri_dev*.log`、`_tauri_final*.log` | 开发日志含 `navicat-clone@18.0.5`、`navicat-clone.exe`（log 内 navicat ×5-54 不等） | — |
| `docs/验收交付/修改文件完整源码/src-tauri/tauri.conf.json:3,5,15` | **旧品牌配置副本**：`productName: "Navicat Premium 18"`、`identifier: "com.navicat.premium.clone"`、窗口标题 `"Navicat Premium 18"` | — |
| `docs/验收交付/修改文件完整源码/src-tauri/Cargo.toml:2,4,9` | `name = "navicat-clone"`、`description = "Navicat Premium 18 像素级复刻"` | — |
| `README.md` | navicat ×9："Navicat 风格复刻"、`参考程序：navicat18_premium_cs_x64.exe`、`D:/app/Navicat/navicat.exe` 等 | — |
| `docs/NavKit-完整任务书.md`、`验收报告.md`、`界面规格表.md`、`差异清单.md` 等 | 文档内多处 navicat 引用（×2-10/文件） | — |

### 2.4 源码注释残留（P2 — 不影响运行，建议顺手清理）

| 位置 | 内容 |
|---|---|
| `src/styles/base.css:2` | `Navicat Premium 复刻 —— 全局样式` |
| `src/core/App.ts:193` | `// 标题栏（实测 29px…logo + "Navicat Premium" + 窗口按钮）` |
| `src/core/App.ts:5566` | `// …（Navicat 网格底部工具）` |
| `src/ui/menu.ts:2` | `Navicat 主菜单定义。` |
| `src/ui/Ribbon.ts:2` | `Ribbon 工具栏：Navicat Premium 的主命令区。` |
| `src/dialogs/ConnectionDialog.ts:6` | `（Navicat / 数据库）` |

### 2.5 已确认干净的应用标识（✅ 无需改动）

| 项 | 值 | 出处 |
|---|---|---|
| productName | `NavKit` | `src-tauri/tauri.conf.json` |
| identifier（Bundle ID） | `com.navkit.app` | 同上 |
| 窗口标题 | `NavKit - 数据库管理` | `tauri.conf.json` / `index.html:6` |
| 标题栏文本 | `NavKit` | `src/core/App.ts`（titleText） |
| Cargo 包名 / lib 名 | `navkit` / `navkit_lib` | `src-tauri/Cargo.toml` |
| npm 包名 | `navkit` | `package.json`（lock 待同步） |
| 关于对话框 | product `NavKit`、`Copyright © 2026 NavKit Contributors`、publisher `NavKit`、url/support 为空 | `src-tauri/src/commands.rs:1378-1388` |
| exe 版本信息 | FileDescription `NavKit`、CompanyName `navkit` | navkit.exe VersionInfo |
| 构建产物二进制 | "Navicat / PremiumSoft / navicat.com / navicat-clone" ASCII+UTF-16 扫描均 **0 命中** | `dist-portable/navkit.exe` |
| 主 bundle JS | `dist/assets/index-*.js` 无 navicat 字符串 | rg 扫描 |

---

## 三、联网与上报行为审计

### 3.1 结论：**未发现任何主动联网上报功能**

无遥测（telemetry）、埋点统计（analytics）、崩溃上报（crash report）、更新检查（update check）、激活/许可证校验、"phone-home" 行为。全库关键词（telemetry / analytics / sentry / posthog / matomo / gtag / sendBeacon / 上报 / 埋点 / activation / license key 等）在源码中 **0 有效命中**（`analytics` 仅是数据库分类"数仓分析"的 id；激活类字符串只存在于 Navicat 提取的 `reference_resources/delphi_strings.txt`，属遗留物而非本程序代码）。

### 3.2 网络出口清单（全部为"连接用户自己的数据库"，即产品核心功能）

| 出口 | 位置 | 性质 |
|---|---|---|
| ClickHouse HTTP 协议 | `src-tauri/src/clickhouse.rs`（reqwest） | 用户配置的数据库地址 |
| HTTP 系数据库适配器 ×19（Elasticsearch / Solr / CouchDB / Meilisearch / Qdrant / Milvus / Weaviate / Chroma / Neo4j / TDengine / IoTDB / InfluxDB / VictoriaMetrics / Turso / Cloudflare D1 / Pulsar / RabbitMQ / Nacos / Consul） | `src-tauri/src/http_db.rs`（reqwest） | 全部连用户输入的 host |
| Cloudflare D1 | `http_db.rs:2240` 硬编码 `https://api.cloudflare.com/client/v4` | **唯一硬编码外网 URL**：D1 数据库托管于 Cloudflare 的官方 API 端点，且需用户提供 account_id/database_id/bearer token 才会触发，属该数据库类型的正常端点，非上报 |
| SQL Server（TDS）、Redis、MongoDB、MySQL/PG/SQLite、ODBC | `mssql.rs` / `redis_db.rs` / `mongo_db.rs` / sqlx / `odbc-api` | 数据库协议驱动 |
| 测试连通探测 | `src-tauri/tests/common/mod.rs:31`（TcpStream::connect_timeout） | 仅测试基建，不进发行物 |

前端 **零** fetch / XMLHttpRequest / WebSocket / sendBeacon / EventSource / axios 调用；前后端通信全部走 Tauri IPC。

### 3.3 防护配置（✅ 设计良好）

- **CSP**（`tauri.conf.json`）：`default-src 'self'; … connect-src 'self' ipc: http://ipc.localhost` —— 前端即使被植入代码也无法向外发请求；
- **插件最小化**：仅 `tauri-plugin-dialog` + `tauri-plugin-fs`，**无** updater / http / shell / opener 插件 —— 无自动更新检查、无外部 URL 打开能力（关于对话框的网站/支持字段是纯文本、不可点击）；
- **权限白名单**（`capabilities/default.json`）：仅窗口控制相关，无网络权限；
- **启动流程**（`lib.rs` setup）：只创建本地 webview 缓存目录、加载本地 Store、写本地日志 `navkit.log`，无网络动作。

### 3.4 需知事项（非上报，但与网络相关）

1. `tauri.conf.json` 的 `webviewInstallMode: embedBootstrapper`：将来若启用 `bundle.active` 打安装包，安装器会从**微软官方**下载 WebView2 运行时（系统组件安装，非本应用上报）；
2. `dist-portable/data/cache/webview/` 与 `tools/acceptance/sandbox/…/EBWebView/` 内的 Crashpad / BrowserMetrics 文件来自 **WebView2（Edge）运行时自带组件**，非应用代码触发的网络行为；
3. `devUrl: http://localhost:5183/preview.html` 仅开发调试用，不进发行物。

---

## 四、清理建议（按优先级）

1. **P0 移出仓库**：`navicat18_premium_cs_x64.exe`、`reference_files/`、`reference_resources/` —— 商业软件本体与提取物，任何分发渠道（含 GitHub 公开）都构成侵权风险；
2. **P0 修发行物**：删 `public/preview.html`、`public/preview-bootstrap.js`、根 `preview.html`（或改名去除 Navicat 字样与 mock 中的品牌信息、`D:/Navicat` 路径、navicat.com 链接），重建 `dist`；改写 `assets/使用说明.txt` 与 `dist-portable/README-portable.txt`（"NavKit — 绿色版"、`navkit.exe`）；
3. **P0 同步命名**：`package-lock.json` 重新生成（name → navkit）；删除 `tools/_tauri_*.log`、`tools/_backup_App_concurrent.ts`；
4. **P1 图标**：确认/更换 `src-tauri/icons/*` 与标题栏 `ICON_LOGO`，使之一眼可区分于 Navicat 商标；重写 `make_icons.py` 注释或移除该脚本；修正 README 的图标来源表述；
5. **P2 注释/文档**：清理 §2.4 的 6 处源码注释；`docs/验收交付/修改文件完整源码/` 内旧品牌副本（`com.navicat.premium.clone`、`navicat-clone`）与 README/docs 中的 Navicat 字样按需改写或删除。

完成 P0-P1 后，可认为"无 Navicat 遗留"；联网方面当前即无上报功能，无需整改。

---

## 五、清理执行记录（2026-11-05）

### 5.1 已删除

| 类别 | 内容 | 说明 |
|---|---|---|
| 商业软件本体 | `navicat18_premium_cs_x64.exe`（195 MB） | 安装包 |
| 专有资产目录 | `reference_files/`（含 `navicat.exe` 110 MB、`navicat.pdf`、PremiumSoft `eula.txt`、`httptunnel/ntunnel_*.php`、数十个 DLL） | 整体删除 |
| 提取资源目录 | `reference_resources/`（`navicat_cloud_*` 图标、`tnavicat*.bmp`、`delphi_strings.txt`、`ui_strings.txt`、`EXTRACTED_MENU.txt` 等） | 整体删除 |
| 逆向工具 | `tools/` 下 14 个脚本：`capture_window.py`、`cut_ribbon_icons.py`、`dfm.py`、`extract_assets.py`、`extract_bitmap_icons.py`、`extract_image_lists.py`、`extract_menu.py`、`extract_resources.py`、`extract_ribbon_icons.py`、`extract_strings.py`、`extract_ui_strings.py`、`innoextract.py`、`innoparse.py`、`make_icons.py` | 均以 Navicat 二进制为输入 |
| 开发日志/备份 | `tools/_tauri_dev*.log`、`tools/_tauri_final*.log`、`tools/_backup_App_concurrent.ts`、`tools/_tsc.log`、`tools/browser_shot.js` | 含旧包名 `navicat-clone` |
| 像素比对截图 | `tools/*.png`（192 个，含 37 张 Navicat 并排对比图、65 张裁切自 Navicat 的局部） | 逆向比对产物 |
| 界面截图 | `images/`（20 张，含 14 张 Navicat 界面截图） | 专有界面素材 |
| 旧品牌文档副本 | `docs/验收交付/修改文件完整源码/`（19 文件，含 `com.navicat.premium.clone`） | 过期源码副本 |
| 过期构建产物 | `dist/`、`dist-portable/`（内含未清理的 preview 副本与旧 README） | 见 §5.3 |

### 5.2 已改写

| 文件 | 改动 |
|---|---|
| `public/preview.html`、`preview.html` | 标题 → `NavKit` |
| `public/preview-bootstrap.js` | mock `about_info` → NavKit 品牌；`D:/Navicat/...` → 相对路径 |
| `assets/使用说明.txt` | 标题 → 「NavKit — 数据库管理工具 绿色版」；`navicat-clone.exe` → `navkit.exe`；补充实际支持的数据库清单 |
| `package-lock.json` | `name`/`version`/`packages[""]` → `navkit` 1.0.0 |
| `src/styles/base.css`、`src/core/App.ts`（2 处）、`src/ui/menu.ts`、`src/ui/Ribbon.ts`、`src/dialogs/ConnectionDialog.ts` | 注释中的 Navicat 字样改为中性表述 |
| `README.md` | 重写：移除参考程序与逆向提取章节；新增「图标」「隐私与网络」章节 |
| `docs/NavKit-完整任务书.md`、`docs/界面规格表.md`、`docs/差异清单.md`、`docs/design/DESIGN.md`、`docs/验收交付/验收报告.md` | 品牌词与产物名改为中性/NavKit |

### 5.3 复验结果

- 全库复扫（排除 `node_modules`、`src-tauri/target`、第三方克隆 `tools/acceptance/reference-transfer-*`）：
  仅剩 5 个文件的命中，且**全部是项目目录名 `tauri-navicat` 的子串**，无真实品牌残留；
- 文件名/目录名检索：无 `*navicat*` 项（目录名除外）；
- `dist/` 与 `dist-portable/` 已删除（原产物内含未清理的 preview 副本）。

### 5.4 未处理项与后续事项

1. **图标**：`src-tauri/icons/*` 由已删除的 `make_icons.py` 生成，来源为 Navicat 安装包图标资源。
   仓库内另有原创脚本 `tools/make_original_icon.py`（品牌蓝渐变 + 白色「N」+ 绿点，自述原创且不引用任何第三方资源），
   但当前 `icons/` 是否为该脚本产物**无法从仓库内自证**（原提取脚本已删除、无构建记录）。建议执行
   `python tools/make_original_icon.py` 重新生成图标后再发布，以彻底消除商标近似风险；
2. **格式兼容命名**：`tools/acceptance/reference-transfer-20261105-dbx/` 内 `navicatImport.ts`、
   `navicat_nb3.rs`、`navicat_mysql_tunnel.rs` 等均为**上游开源项目自带的 Navicat 导入功能**，
   属第三方代码，不应改动；
3. **`tools/acceptance/`**（约 2.1 GB：`pg/` PostgreSQL 数据目录、两个第三方仓库克隆）为测试基建，
   体积较大且不属于发行物，可按需清理或加入 `.gitignore`；
4. **构建状态**：本次复验执行 `npm run build` 失败，报 16 处 TypeScript 错误（`DataGrid.ts` 缺少
   `onHeaderLeave`/`onResizeKeyDown`/`previousBodyCursor` 等成员；`App.ts` 引用 `DataTabView.sortDir`/
   `fillSortColumns`/`sortColSel`）。经比对，这些错误**在本次清理前即已存在**（与清理改动无关，
   清理仅触及注释）。按要求未修改功能代码，构建产物未重建 —— **发布前需先修复该编译错误**。
