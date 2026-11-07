/**
 * 应用主控制器：组装菜单栏 / 工具栏 / 连接树 / 标签页 / 状态栏，
 * 并按实际连接能力和当前上下文分发菜单命令；不以空操作冒充成功。
 */

import { api, datasourceSyncApi, DB_META, type DriverDefinition } from "./api";
import { DatasourceSync, invalidatedDatasourceIds, type DatasourceSyncResult } from "./datasourceSync";
import { ManualTransactions, type TransactionContext } from "./ManualTransactions";
import { MANUAL_TRANSACTION_WRITE_COMMANDS, manualTransactionStatusText, manualTransactionUnavailableReason, type ManualTransactionStatus } from "./manualTransaction";
import { confirmTransactionAction } from "../dialogs/TransactionDialog";
import { columnOrderCapability, mysqlColumnDefinitions, mysqlColumnOrderSql } from "./columnOrder";
import { loadObjectDetails } from "./objectDetails";
import { openHistoryLogDialog } from "../dialogs/HistoryLogDialog";
import { openServerMonitorDialog } from "../dialogs/ServerMonitorDialog";
import { openDataGeneratorDialog } from "../dialogs/DataGeneratorDialog";
import { openQueryBuilderDialog } from "../dialogs/QueryBuilderDialog";
import { queryBuilderUnavailableReason } from "./queryBuilder";
import { openDriverManager } from "../dialogs/DriverManagerDialog";
import { openSyncDialog } from "../dialogs/SyncDialog";
import { DB_KINDS, kindMeta } from "./dbcatalog";
import type {
  AppSettings,
  ConnectionConfig,
  Favorite,
  MultiResult,
  ObjectType,
  RecentItem,
  TabState,
  TreeItem,
} from "./types";
import { DataGrid } from "../ui/DataGrid";
import { MenuRenderer } from "../ui/MenuBar";
import { ModelCanvas, type ErTable } from "../ui/ModelCanvas";
import { SqlEditor } from "../ui/SqlEditor";
import { TabStrip } from "../ui/TabStrip";
import { Ribbon, RIBBON_MAIN } from "../ui/Ribbon";
import { navkitLogo } from "../ui/logo";
import { InfoPanel } from "../ui/InfoPanel";
import { TabToolbar, type TabToolItem } from "../ui/TabToolbar";
import { TreePanel } from "../ui/TreePanel";
import { Dialog } from "../ui/Dialog";
import {
  ICON_WIN_MIN,
  ICON_WIN_MAX,
  ICON_WIN_CLOSE,
  ICON_VIEW_DOTS,
  ICON_VIEW_LIST,
  ICON_VIEW_GRID,
  ICON_PANEL_LEFT,
  ICON_PANEL_RIGHT,
  ICON_MSG_INFO,
  ICON_MSG_ERROR,
  ICON_PAGE_FIRST,
  ICON_PAGE_PREV,
  ICON_PAGE_NEXT,
  ICON_PAGE_LAST,
  ICON_MORE_DOTS,
  ICON_COLUMNS_GRID,
  ICON_PLUS_LINE,
  ICON_MINUS_LINE,
  ICON_CHECK,
  ICON_CROSS,
  ICON_KEY_GOLD,
  ICON_TITLEBAR_SEARCH,
  ICON_EMPTY_CONNECT,
  svg,
  iconSvgByName,
  type IconDef,
} from "../ui/icons";
import {
  GRID_CONTEXT_MENU,
  OTHER_OBJECTS_MENU,
  QUERY_MENU,
  TAB_CONTEXT_MENU,
  TEXT_FORMAT_MENU,
  TREE_BLANK_MENU,
  treeContextMenu,
  type MenuEntry,
} from "../ui/menu";
import { openConnectionDialog } from "../dialogs/ConnectionDialog";
import { openConnectionPicker } from "../dialogs/ConnectionPickerDialog";
import { openBackupDialog } from "../dialogs/BackupDialog";
import { ChartPanel } from "../ui/ChartPanel";
import { chartSourceError, createChartState, type ChartSource } from "./chart";
import { openJobHistoryDialog, openScheduleJobDialog } from "../dialogs/ScheduleDialog";
import { openChangePasswordDialog, openCreateUserDialog } from "../dialogs/UserDialog";
import {
  openAboutDialog,
  openFavoritesDialog,
  openServerInfoDialog,
  openSettingsDialog,
} from "../dialogs/SettingsDialog";
import {
  openDumpDialog,
  openExportWizard,
  openImportWizard,
  openResultExportDialog,
  openTransferDialog,
} from "../dialogs/ImportExportDialog";

/** 提示消息。 */
interface Toast {
  kind: "info" | "error" | "success";
  text: string;
}

/** 查询页右侧的代码段清单（参考截图 5）。 */
const QUERY_SNIPPETS: { name: string; tag: string; desc: string; sql: string }[] = [
  { name: "SELECT 查询", tag: "查询", desc: "检索表中的记录", sql: "SELECT *\nFROM table_name\nWHERE condition;" },
  { name: "INSERT 插入", tag: "DML", desc: "插入一条记录", sql: "INSERT INTO table_name (column_name)\nVALUES ('value');" },
  { name: "UPDATE 更新", tag: "DML", desc: "按条件更新，执行前检查 WHERE", sql: "UPDATE table_name\nSET column_name = 'value'\nWHERE condition;" },
  { name: "CASE 分支", tag: "流程控制", desc: "在查询中使用条件表达式", sql: "CASE\n  WHEN condition THEN 'yes'\n  ELSE 'no'\nEND" },
  { name: "SQL 注释", tag: "注释", desc: "插入说明注释", sql: "-- 在这里填写说明\n" },
  { name: "多字段排序", tag: "查询", desc: "依次使用多个字段排序", sql: "SELECT * FROM table_name\nORDER BY first_column ASC, second_column DESC;" },
];

export class App {
  private root: HTMLElement;
  private shell: HTMLElement;
  private menuHost: HTMLElement;
  private ribbonHost: HTMLElement;
  private sidebar: HTMLElement;
  private splitter: HTMLElement;
  private mainPanel: HTMLElement;
  private tabStrip: TabStrip;
  private contentHost: HTMLElement;
  private statusBar: HTMLElement;
  private toastHost: HTMLElement;

  private menu!: MenuRenderer;
  private ribbon!: Ribbon;
  private tree!: TreePanel;
  private infoPanel!: InfoPanel;
  private tabToolbar!: TabToolbar;
  private infoSplitter!: HTMLElement;

  private tabs: TabState[] = [];
  private activeTabId: string | null = null;
  private treeNodes: TreeItem[] = [];
  private connections: ConnectionConfig[] = [];
  private groups: string[] = [];
  private settings!: AppSettings;
  private favorites: Favorite[] = [];
  private recent: RecentItem[] = [];
  private openConnections = new Set<string>();
  /** 已打开对象页的库（connId/db），树中图标保持绿色高亮。 */
  private openDbs = new Set<string>();
  private datasourceSyncStatus = "";
  private datasourceSync = new DatasourceSync(datasourceSyncApi, {
    ready: () => this.datasourceSyncReady(),
    applied: result => this.applyDatasourceSync(result),
    status: (message, error) => {
      this.datasourceSyncStatus = `${error ? "⚠ " : ""}${message}`;
      this.renderStatus();
    },
  });
  /** 后端事务状态仅用于显示；生命周期操作交给独立控制器。 */
  private manualTransactionController = new ManualTransactions(api, {
    context: id => this.transactionContext(id),
    connection: id => this.connectionById(id),
    createQuery: async (id, schema) => {
      const tab = await this.openQueryTab(id, "", schema);
      const context = this.transactionContext(tab.id);
      if (!context) throw new Error("创建事务查询页面失败");
      return context;
    },
    removeTab: id => this.removeTabNow(id),
    focus: id => { const view = this.views.get(id); if (view instanceof QueryTabView) view.editor.focus(); },
    changed: () => {
      for (const view of this.views.values()) if (view instanceof QueryTabView) view.syncManualTransactionControls();
      this.notifyToolbar();
    },
    toast: (kind, message) => this.toast(kind, message),
  });

  /** 查看选项（查看菜单真实生效的状态）。 */
  private showGridLines = true;
  private skipNull = false;
  private focusMode = false;
  /** 状态栏数据显示模式（明细 / 列表 / 网格，互斥高亮）。 */
  private viewMode: "detail" | "list" | "grid" = "grid";
  /** 单元格内容面板的显示格式（文本▼下拉）。 */
  private textFormat: "text" | "hex" | "image" | "web" = "text";
  /** 表页信息面板数据缓存（conn/schema/table → 表信息 + DDL + 表结构）。 */
  private tableMetaCache = new Map<
    string,
    { status: [string, string][]; ddl: string; schema: import("./types").TableSchema | null }
  >();

  /** 每个标签页对应的视图组件。 */
  private views = new Map<string, TabView>();
  private sidebarVisible = true;
  private statusBarVisible = true;
  private toolbarVisible = true;
  private infoPanelVisible = true;
  private exitApproved = false;
  private exitPending = false;
  private exitDialog: Dialog | null = null;
  private infoRequest = 0;
  private newConnectionGroup = "";
  /** 标题栏右侧文档标题（当前连接上下文）。 */
  private titleDoc!: HTMLElement;

  constructor(root: HTMLElement) {
    this.root = root;
    this.buildSkeleton();
  }

  // ------------------------------------------------------------ 骨架

  private buildSkeleton(): void {
    this.root.innerHTML = "";
    this.shell = document.createElement("div");
    this.shell.className = "app-shell";

    // 标题栏（v2 · 38px）：logo + 产品名 + 菜单并入 + 右侧文档标题/工具 + 窗口按钮
    const titlebar = document.createElement("div");
    titlebar.className = "titlebar";
    const logo = document.createElement("div");
    logo.className = "titlebar-logo";
    logo.innerHTML = navkitLogo(20);
    const titleText = document.createElement("div");
    titleText.className = "titlebar-name";
    titleText.textContent = "NavKit";
    const titleSep = document.createElement("div");
    titleSep.className = "titlebar-sep";
    const btns = document.createElement("div");
    btns.className = "titlebar-btns";
    const mkBtn = (icon: IconDef, cls: string, title: string) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = `titlebar-btn ${cls}`;
      b.title = title;
      b.setAttribute("aria-label", title);
      b.innerHTML = svg(icon);
      b.addEventListener("mousedown", (e) => e.stopPropagation());
      return b;
    };
    const minBtn = mkBtn(ICON_WIN_MIN, "", "最小化");
    const maxBtn = mkBtn(ICON_WIN_MAX, "", "最大化");
    const closeBtn = mkBtn(ICON_WIN_CLOSE, "close", "关闭");
    btns.appendChild(minBtn);
    btns.appendChild(maxBtn);
    btns.appendChild(closeBtn);
    // 右侧文档标题 + 工具（搜索聚焦树 / 打开选项）
    this.titleDoc = document.createElement("div");
    this.titleDoc.className = "titlebar-doc";
    this.titleDoc.textContent = "未连接";
    const tools = document.createElement("div");
    tools.className = "titlebar-tools";
    const tbIcon = (body: string, title: string, onClick: () => void) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "tb-icon";
      b.title = title;
      b.setAttribute("aria-label", title);
      b.innerHTML = body;
      b.addEventListener("mousedown", (e) => e.stopPropagation());
      b.addEventListener("click", onClick);
      return b;
    };
    tools.appendChild(
      tbIcon(svg(ICON_TITLEBAR_SEARCH, 15), "搜索", () => this.tree?.focusSearch()),
    );
    const spacer = document.createElement("div");
    spacer.className = "titlebar-spacer";
    spacer.setAttribute("data-tauri-drag-region", "");
    titlebar.appendChild(logo);
    titlebar.appendChild(titleText);
    titlebar.appendChild(titleSep);
    titlebar.appendChild(spacer);
    titlebar.appendChild(this.titleDoc);
    titlebar.appendChild(tools);
    titlebar.appendChild(btns);
    // 拖动只绑定标题区域，不能把窗口按钮包含在拖动区域里。
    titleText.setAttribute("data-tauri-drag-region", "");
    logo.setAttribute("data-tauri-drag-region", "");
    const win = async () => {
      if ((window as unknown as { __PREVIEW_MOCK__?: unknown }).__PREVIEW_MOCK__) return null;
      if (!(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) return null;
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      return getCurrentWindow();
    };
    minBtn.addEventListener("click", () => {
      void win().then((w) => w?.minimize()).catch((e) => this.toast("error", `最小化失败：${String(e)}`));
    });
    maxBtn.addEventListener("click", () => {
      void win().then((w) => w?.toggleMaximize()).catch((e) => this.toast("error", `最大化失败：${String(e)}`));
    });
    titleText.addEventListener("dblclick", () => {
      void win().then((w) => w?.toggleMaximize()).catch((e) => this.toast("error", String(e)));
    });
    closeBtn.addEventListener("click", () => this.confirmExit());
    this.shell.appendChild(titlebar);

    // 菜单栏（v2：并入标题栏内部，紧跟产品名分隔线之后）
    this.menuHost = document.createElement("div");
    this.menuHost.className = "menubar";
    titlebar.insertBefore(this.menuHost, spacer);

    // Ribbon 工具栏（v2 命令条）
    this.ribbonHost = document.createElement("div");
    this.ribbonHost.className = "ribbon";
    this.shell.appendChild(this.ribbonHost);

    // 工作区
    const work = document.createElement("div");
    work.className = "workarea";

    this.sidebar = document.createElement("div");
    this.sidebar.className = "sidebar";
    this.splitter = document.createElement("div");
    this.splitter.className = "splitter";
    this.mainPanel = document.createElement("div");
    this.mainPanel.className = "main-panel";

    this.infoSplitter = document.createElement("div");
    this.infoSplitter.className = "splitter";

    work.appendChild(this.sidebar);
    work.appendChild(this.splitter);
    work.appendChild(this.mainPanel);
    work.appendChild(this.infoSplitter);
    this.shell.appendChild(work);

    // 标签条
    this.tabStrip = new TabStrip({
      onSelect: (tab) => this.activateTab(tab.id),
      onClose: (tab) => this.closeTab(tab.id),
      onContextMenu: (tab, x, y) => {
        // 右键时同步激活该标签并记录基准，供「关闭左侧/右侧窗口」使用
        this.contextTabId = tab.id;
        this.activateTab(tab.id);
        this.menu.showContext(TAB_CONTEXT_MENU, x, y);
      },
    });
    this.mainPanel.appendChild(this.tabStrip.el);

    // 标签页二级工具栏
    this.tabToolbar = new TabToolbar({
      onCommand: (id) => void this.runCommand(id),
      onDropdown: (id, x, y) => this.ribbonDropdown(id, x, y),
    });
    this.mainPanel.appendChild(this.tabToolbar.el);

    // 内容区
    this.contentHost = document.createElement("div");
    this.contentHost.className = "content-host";
    this.mainPanel.appendChild(this.contentHost);

    // 右侧信息面板
    this.infoPanel = new InfoPanel();
    work.appendChild(this.infoPanel.el);

    // 状态栏
    this.statusBar = document.createElement("div");
    this.statusBar.className = "statusbar";
    this.shell.appendChild(this.statusBar);

    // 提示
    this.toastHost = document.createElement("div");
    this.toastHost.className = "toast-host";
    document.body.appendChild(this.toastHost);

    // 记录鼠标位置（排序 / 列选择等弹出菜单跟随鼠标出现）
    document.addEventListener("mousemove", (e) => {
      this.lastMouse = { x: e.clientX, y: e.clientY };
    });

    this.root.appendChild(this.shell);
  }

  // ------------------------------------------------------------ 初始化

  async init(): Promise<void> {
    this.settings = await api.getSettings();
    this.applySettings();

    this.connections = await api.listConnections();
    this.groups = await api.listGroups();
    this.favorites = await api.listFavorites();
    this.recent = await api.listRecent();

    this.menu = new MenuRenderer(this.menuHost, (id) => this.dispatchCommand(id), (id) => this.ribbonDisabled(id));
    // 「设置」以文字项形式置于菜单栏末尾（替代原先的齿轮图标，语义更直白）；WebDAV 同步配置在设置的对应选项卡内。
    this.menu.setTailActions([{ label: "设置", command: "options" }]);
    this.menu.renderBar(this.menuHost);

    this.ribbon = new Ribbon(RIBBON_MAIN, {
      onCommand: (id) => void this.runCommand(id),
      isDisabled: (id) => this.ribbonDisabled(id),
      onDropdown: (id, x, y) => this.ribbonDropdown(id, x, y),
    });
    this.ribbonHost.replaceWith(this.ribbon.el);
    this.ribbon.el.className = "ribbon";
    this.ribbonHost = this.ribbon.el;

    this.tree = new TreePanel({
      onToggle: (node) => void this.toggleNode(node),
      onSelect: (node) => void this.selectNode(node),
      onOpen: (node) => void this.openNode(node),
      onContextMenu: (node, x, y) =>
        this.menu.showContext(this.treeMenuFor(node), x, y),
      onBlankContextMenu: (x, y) => this.menu.showContext(TREE_BLANK_MENU, x, y),
      onMoveConnection: (id, group, targetId, position) => this.moveConnectionToGroup(id, group, targetId, position),
    });
    this.sidebar.appendChild(this.tree.el);

    this.setupSplitter();
    this.setupInfoSplitter();
    this.setupKeyboard();
    this.setupCloseGuard();

    // 查看菜单勾选态持久化：恢复上次会话的界面显示偏好
    this.loadViewPrefs();
    this.statusBar.style.display = this.statusBarVisible ? "" : "none";

    this.refreshTree();
    this.renderTabs();
    this.renderStatus();
    // 自动运行：载入任务并启动调度（应用运行期间按间隔到期执行）
    void this.reloadScheduleJobs();
    this.setupScheduleTimer();
    await this.datasourceSync.start();
    window.addEventListener("beforeunload", () => this.datasourceSync.stop());
    // 调试钩子（仅开发预览用）
    (window as unknown as { __APP__?: App }).__APP__ = this;
  }

  /** 查看菜单显示偏好持久化（重启后保留勾选态）。 */
  private saveViewPrefs(): void {
    try {
      localStorage.setItem(
        "navkit.viewprefs.v1",
        JSON.stringify({
          sidebarVisible: this.sidebarVisible,
          statusBarVisible: this.statusBarVisible,
          toolbarVisible: this.toolbarVisible,
          infoPanelVisible: this.infoPanelVisible,
          showGridLines: this.showGridLines,
          skipNull: this.skipNull,
        }),
      );
    } catch { /* 存储不可用时静默忽略 */ }
  }

  private loadViewPrefs(): void {
    try {
      const raw = localStorage.getItem("navkit.viewprefs.v1");
      if (!raw) return;
      const p = JSON.parse(raw) as Record<string, unknown>;
      if (p["sidebarVisible"] === false) {
        this.sidebarVisible = false;
        this.sidebar.style.display = "none";
        this.splitter.style.display = "none";
      }
      if (p["statusBarVisible"] === false) this.statusBarVisible = false;
      if (p["toolbarVisible"] === false) {
        this.toolbarVisible = false;
        this.ribbon.el.style.display = "none";
      }
      if (p["infoPanelVisible"] === false) {
        this.infoPanelVisible = false;
        this.infoPanel.setVisible(false);
        this.infoSplitter.style.display = "none";
      }
      if (p["showGridLines"] === false) this.showGridLines = false;
      if (p["skipNull"] === true) this.skipNull = true;
    } catch { /* 损坏数据忽略，使用默认值 */ }
  }

  private applySettings(): void {    document.documentElement.style.setProperty(
      "--font-ui",
      `"${this.settings.font_family}", "Microsoft YaHei UI", "Segoe UI", Tahoma, sans-serif`,
    );
    document.documentElement.style.fontSize = `${this.settings.font_size}px`;
  }

  private setupSplitter(): void {
    let dragging = false;
    let startX = 0;
    let startW = 0;
    this.splitter.addEventListener("mousedown", (e) => {
      dragging = true;
      startX = e.clientX;
      startW = this.sidebar.offsetWidth;
      this.splitter.classList.add("dragging");
      document.body.style.cursor = "col-resize";
      e.preventDefault();
    });
    window.addEventListener("mousemove", (e) => {
      if (!dragging) return;
      const w = Math.max(160, Math.min(640, startW + e.clientX - startX));
      this.sidebar.style.width = `${w}px`;
    });
    window.addEventListener("mouseup", () => {
      if (!dragging) return;
      dragging = false;
      this.splitter.classList.remove("dragging");
      document.body.style.cursor = "";
    });
  }

  private setupInfoSplitter(): void {
    let dragging = false;
    let startX = 0;
    let startW = 0;
    this.infoSplitter.addEventListener("mousedown", (e) => {
      dragging = true;
      startX = e.clientX;
      startW = this.infoPanel.el.offsetWidth;
      this.infoSplitter.classList.add("dragging");
      document.body.style.cursor = "col-resize";
      e.preventDefault();
    });
    window.addEventListener("mousemove", (e) => {
      if (!dragging) return;
      const w = Math.max(140, Math.min(420, startW - (e.clientX - startX)));
      this.infoPanel.el.style.width = `${w}px`;
    });
    window.addEventListener("mouseup", () => {
      if (!dragging) return;
      dragging = false;
      this.infoSplitter.classList.remove("dragging");
      document.body.style.cursor = "";
    });
  }

  /** 全局快捷键（编辑器内部按键由 CodeMirror 先处理并 preventDefault）。 */
  private setupKeyboard(): void {
    document.addEventListener("keydown", (e) => {
      if (e.defaultPrevented) return;
      const ctrl = e.ctrlKey || e.metaKey;
      const inEditor = !!(e.target as HTMLElement | null)?.closest?.(".cm-editor");
      // 输入框 / 文本域 / 下拉框内的编辑组合键必须交还给原生控件处理，
      // 否则对话框里 Ctrl+A 全选失效（表现为端口等字段无法正常改值）。
      const inEditable = !!(e.target as HTMLElement | null)?.matches?.(
        'input[type="text"], input[type="password"], input[type="number"], input[type="search"], input:not([type]), textarea, select, [contenteditable="true"]',
      );
      if (!ctrl) {
        if (e.key === "F5") {
          e.preventDefault();
          void this.runCommand("refresh");
        } else if (e.key === "F6") {
          e.preventDefault();
          void this.runCommand("beautify-sql");
        } else if (e.key === "F11") {
          e.preventDefault();
          void this.runCommand("focus-mode");
        } else if (e.key === "F3") {
          e.preventDefault();
          void this.runCommand("find-next");
        } else if (e.key === "Delete" && !inEditor && !inEditable) {
          const view = this.activeView();
          if (view instanceof DataTabView) {
            e.preventDefault();
            void this.cmdDeleteRow();
          }
        }
        return;
      }
      // 可编辑控件内的 Ctrl+A/C/X/V/Z/F/H/P 一律放行（原生全选 / 剪贴板行为）
      if (inEditable && ["a", "c", "x", "v", "z", "y", "f", "h"].includes(e.key.toLowerCase())) {
        return;
      }
      const key = e.key.toLowerCase();
      if (e.shiftKey) {
        if (key === "s") {
          e.preventDefault();
          void this.runCommand("save-as");
        } else if (key === "w") {
          e.preventDefault();
          this.cmdCloseAll();
        } else if (key === "f") {
          e.preventDefault();
          void this.runCommand("add-favorite");
        } else if (key === "r") {
          e.preventDefault();
          void this.runCommand("run-selected");
        }
        return;
      }
      if (e.key === "Tab") {
        e.preventDefault();
        this.cmdCycleTab(1);
        return;
      }
      const map: Record<string, string> = {
        n: "new-connection",
        q: "new-query",
        s: "save",
        w: "close-window",
        f: "find",
        r: "run",
        p: "print",
        l: "history-log",
        z: "undo",
        y: "redo",
        x: "cut",
        c: "copy",
        v: "paste",
        a: "select-all",
        h: "replace",
        i: "grid-insert",
        d: "delete-row",
      };
      // 焦点在普通输入控件时不再分发应用级快捷键（对话框里 Ctrl+S 误触发保存等）
      if (inEditable) return;
      const cmd = map[key];
      if (cmd) {
        if (inEditor && ["a", "f", "h", "z", "y", "c", "x", "v"].includes(key)) {
          // 编辑器内部：Ctrl+C/Z/F/H 等由 CodeMirror 原生处理
          return;
        }
        e.preventDefault();
        void this.runCommand(cmd);
      }
    });
  }

  /** 关闭守卫：设置里开启「关闭前确认」时弹出确认对话框。 */
  private setupCloseGuard(): void {
    try {
      const internals = (window as unknown as { __TAURI_INTERNALS__?: unknown })
        .__TAURI_INTERNALS__;
      if (!internals) return;
      void import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
        const w = getCurrentWindow();
        void w.onCloseRequested(async (evt) => {
          if (this.exitApproved || (!this.settings.confirm_on_close && !this.manualTransactionController.hasActive() && !this.exitPending)) return;
          evt.preventDefault();
          this.confirmExit();
        });
      });
    } catch {
      /* 浏览器预览中无 Tauri 窗口 */
    }
  }

  /** 退出前确认（Alt+F4 / 关闭按钮 / 文件-退出 共用）。 */
  private confirmExit(): void {
    if (this.exitApproved || this.exitDialog || this.exitPending) return;
    if (this.manualTransactionController.hasActive()) {
      this.exitPending = true;
      void confirmTransactionAction("退出并回滚", "存在尚未提交的手动事务。退出会回滚这些事务中的全部未提交更改；取消可返回查询页进行提交或回滚。确定退出吗？", "回滚并退出").then(accepted => {
        this.exitPending = false;
        if (accepted) this.doExit();
      });
      return;
    }
    if (!this.settings.confirm_on_close) {
      this.doExit();
      return;
    }
    this.exitDialog = new Dialog({
      title: "确认",
      width: 360,
      render: (body) => {
        body.textContent = "确定要退出 NavKit 吗？";
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "确定",
          primary: true,
          onClick: () => {
            this.doExit();
          },
        },
        { label: "取消" },
      ],
    });
    this.exitDialog.onClose(() => { this.exitDialog = null; });
  }

  private doExit(): void {
    if (this.exitPending) return;
    this.exitPending = true;
    this.exitDialog = null;
    void (async () => {
      try {
        await api.closeAllConnections();
      } catch (error) {
        this.exitPending = false;
        this.toast("error", `关闭连接或回滚失败，未退出应用：${String(error)}`);
        return;
      }
      this.exitApproved = true;
      try {
        const internals = (window as unknown as { __TAURI_INTERNALS__?: unknown })
          .__TAURI_INTERNALS__;
        if (internals) {
          const { getCurrentWindow } = await import("@tauri-apps/api/window");
          await getCurrentWindow().close();
          return;
        }
      } catch {
        /* ignore */
      }
      window.close();
    })();
  }

  // ------------------------------------------------------------ 树

  private refreshTree(): void {
    const previous = new Map<string, TreeItem>();
    const remember = (nodes: TreeItem[]) => nodes.forEach((node) => { previous.set(node.id, node); remember(node.children); });
    remember(this.treeNodes);
    this.treeNodes = TreePanel.buildGroups(this.connections, this.groups);
    const restore = (nodes: TreeItem[]) => nodes.forEach((node) => {
      const old = previous.get(node.id);
      if (old) {
        node.expanded = old.expanded;
        if (node.type === "connection") {
          node.children = old.children;
          node.loaded = old.loaded;
        }
      }
      restore(node.children);
    });
    restore(this.treeNodes);
    this.tree.setNodes(this.treeNodes);
    this.refreshQueryGroups();
    this.syncConnIcons();
  }

  /** 依据连接打开状态同步连接节点图标：已连接=彩色官方 logo，未连接=灰色。 */
  private syncConnIcons(): void {
    const walk = (nodes: TreeItem[]) => {
      for (const n of nodes) {
        const m = /^conn:([^/]+)$/.exec(n.id);
        if (m) {
          const cfg = this.connections.find((c) => c.id === m[1]);
          if (cfg) {
            const base = DB_META[cfg.kind]?.icon ?? "info_pane_conn_postgresql";
            n.connected = this.openConnections.has(m[1]);
            // 打开：官方 logo 原图 + 自带外框染绿（_open）；关闭：灰阶（_off）
            n.icon = n.connected ? `${base}_open` : `${base}_off`;
          }
        }
        // 库节点：已打开 = 绿色数据库图标 + 左侧展开箭头；
        // 未打开（仅点击选中）不亮起、不显示箭头
        const dbm = /^conn:([^/]+)\/db\/([^/]+)$/.exec(n.id);
        if (dbm) {
          const opened = this.openDbs.has(`${dbm[1]}/${dbm[2]}`);
          n.icon = opened ? "info_pane_database" : "info_pane_schema";
          n.expandable = opened;
        }
        walk(n.children || []);
      }
    };
    walk(this.treeNodes);
    this.tree.updateNodes();
  }

  /** 重建树中「查询」分组的内容（收藏的查询即保存的查询）。 */
  private refreshQueryGroups(): void {
    const walk = (nodes: TreeItem[]) => {
      for (const n of nodes) {
        const m = /^(conn:[^/]+)(?:\/db\/([^/]+))?\/grp-query$/.exec(n.id);
        if (m) {
          const connId = m[1].slice(5);
          n.children = this.favorites
            .filter((f) => !f.connection_id || f.connection_id === connId)
            .map((f) => ({
              id: `${m[1]}${m[2] ? `/db/${m[2]}` : ""}/query/${f.id}`,
              label: f.name,
              type: "query" as ObjectType,
              icon: "info_pane_query",
              children: [],
              expandable: false,
              has_children: false,
              expanded: false,
            }));
          n.has_children = n.children.length > 0;
        }
        walk(n.children || []);
      }
    };
    walk(this.treeNodes);
    this.tree.setNodes(this.treeNodes);
  }

  /** 节点的语义分类（用于右键菜单）。 */
  private nodeKind(node: TreeItem): string {
    // 注意：所有节点 id 都以 "conn:" 开头，连接根节点是唯一没有 "/" 的
    if (node.id.startsWith("conn:") && !node.id.slice(5).includes("/")) return "connection";
    if (node.id.startsWith("group:")) return "folder";
    if (node.id.includes("/obj/")) {
      if (node.id.includes("/obj/procedure/")) return "procedure";
      if (node.id.includes("/obj/function/")) return "function";
      if (node.id.includes("/obj/trigger/")) return "leaf";
      if (node.id.includes("/obj/event/")) return "leaf";
      return "function";
    }
    if (node.id.includes("/db/")) {
      // 表 / 视图路径下的全部深层节点（分组、字段、索引、外键）沿用所属
      // 对象的右键菜单，而不是归到数据库菜单
      if (/\/view\//.test(node.id)) return "view";
      if (/\/table\//.test(node.id)) return "table";
      return "database";
    }
    if (node.id.includes("/table/")) return "table";
    if (node.id.includes("/view/")) return "view";
    if (node.id.includes("/proc/")) return "procedure";
    if (node.id.includes("/trigger/")) return "leaf";
    return "database";
  }

  /** 树节点右键菜单（备份分组节点有专属菜单）。 */
  private treeMenuFor(node: TreeItem): MenuEntry[] {
    if (/grp-backup$/.test(node.id)) {
      return [
        { id: "tree-new-backup", label: "新建备份..." },
        { id: "tree-open-backup-folder", label: "打开所在文件夹..." },
        { separator: true },
        { id: "tree-refresh", label: "刷新" },
      ];
    }
    return treeContextMenu(this.nodeKind(node));
  }

  /** 从树选中的节点解析 (connId, schema, table)。 */
  private currentTableRef(): { connId: string; schema: string; table: string } | null {
    const node = this.tree.find(this.tree.selectedNodeId ?? "");
    if (node) {
      const parts = node.id.split("/");
      const connId = parts[0].startsWith("conn:") ? parts[0].slice(5) : "";
      const tblIdx = parts.indexOf("table");
      const viewIdx = parts.indexOf("view");
      if (connId && (tblIdx >= 0 || viewIdx >= 0)) {
        const dbIdx = parts.indexOf("db");
        return {
          connId,
          schema: dbIdx >= 0 ? parts[dbIdx + 1] : "",
          table: tblIdx >= 0 ? parts[tblIdx + 1] : parts[viewIdx + 1],
        };
      }
    }
    const tab = this.activeTab;
    if (tab && (tab.kind === "table" || tab.kind === "structure") && tab.object) {
      return { connId: tab.connectionId, schema: tab.schema, table: tab.object };
    }
    if (tab && tab.kind === "objects" && tab.selectedRows.size) {
      const view = this.views.get(tab.id);
      if (view instanceof ObjectsTabView) {
        const info = view.selectedObject();
        if (info && info.type === "table") {
          return { connId: tab.connectionId, schema: tab.schema, table: info.name };
        }
      }
    }
    return null;
  }

  /** 当前上下文连接 id：右键选中的树节点 → 当前标签页。 */
  private currentConnectionId(): string {
    const node = this.tree.find(this.tree.selectedNodeId ?? "");
    if (node) {
      const parts = node.id.split("/");
      if (parts[0].startsWith("conn:")) return parts[0].slice(5);
    }
    return this.activeTab?.connectionId ?? "";
  }

  /** 当前选中的数据库名（树节点 → /db/<name>；SQLite 连接即库）。 */
  private currentDbName(): string {
    const node = this.tree.find(this.tree.selectedNodeId ?? "");
    if (node) {
      const m = node.id.match(/^conn:[^/]+(?:\/db\/([^/]+))?/);
      if (m?.[1]) return m[1];
      const cfg = this.connections.find((c) => c.id === this.currentConnectionId());
      if (cfg?.kind === "sqlite") return "main";
    }
    return this.activeTab?.schema ?? "";
  }

  /** 展开节点，加载子节点。 */
  private async toggleNode(node: TreeItem): Promise<void> {
    if (!node.expandable) return;
    if (node.expanded) {
      node.expanded = false;
      this.tree.render();
      return;
    }
    if (node.children.length === 0 && !node.loaded) {
      node.loading = true;
      this.tree.render();
      await this.loadChildren(node);
      node.loading = false;
      node.loaded = true;
    }
    node.expanded = true;
    this.syncConnIcons();
    this.tree.render();
  }

  /** 加载节点的子节点。 */
  private async loadChildren(node: TreeItem): Promise<void> {
    // 分组节点统一拦截：查询分组懒加载收藏的保存查询、备份分组懒加载备份文件，
    // 其余分组（字段 / 索引 / 外键 / 触发器）的内容已由父节点（表 / 视图）在
    // 加载时构建完毕 —— 必须在下方表分支之前 return，否则 id 中的
    // "table/<表名>/grp-xx" 会落进表正则、被当成表名 "<表名>/grp-xx" 再生成
    // 一层「字段/索引/外键/触发器」分组，造成无限套叠（字段也跟着对不上）。
    if (/\/grp-(column|index|fk|trigger|query|backup|table|view|function)$/.test(node.id)) {
      if (/\/grp-query$/.test(node.id)) {
        this.refreshQueryGroups();
        return;
      }
      if (/\/grp-backup$/.test(node.id)) {
        const m = /^conn:([^/]+)\/db\/([^/]+)\/grp-backup$/.exec(node.id);
        if (m) {
          try {
            const backups = await api.listBackups(m[1], m[2]);
            node.children = backups.map((b) => ({
              id: `${node.id}/file/${b.name}`,
              label: b.name,
              type: "folder" as ObjectType,
              icon: "dbn_backup",
              children: [],
              expandable: false,
              has_children: false,
              expanded: false,
            }));
            node.has_children = node.children.length > 0;
          } catch {
            node.children = [];
          }
        }
      }
      // 其余分组：内容已就绪（空分组展开为空白即可）
      return;
    }
    if (/^conn:[^/]+$/.test(node.id)) {
      const connId = node.id.slice(5);
      const cfg = this.connections.find((c) => c.id === connId);
      if (!cfg) return;
      try {
        await this.ensureConnection(connId);
        const dbs = await api.listDatabases(connId);
        if (cfg.kind === "sqlite") {
          // SQLite 直接把表列在连接下
          const tables = await api.listTables(connId, "");
          node.children = tables.map(([t, type]) => ({
            id: `${node.id}/table/${t}`,
            label: t,
            type,
            icon: type === "view" ? "info_pane_view" : "info_pane_table",
            children: [],
            expandable: true,
            has_children: true,
            expanded: false,
          }));
          return;
        }
        node.children = dbs.map(([name]) => ({
          id: `${node.id}/db/${name}`,
          label: name,
          type: "database" as ObjectType,
          // 未打开的库不亮起、不带展开箭头（打开后由 syncConnIcons 补上）
          icon: "info_pane_schema",
          children: [],
          expandable: this.openDbs.has(`${connId}/${name}`),
          has_children: true,
          expanded: false,
        }));
      } catch (e) {
        this.toast("error", `无法列出数据库：${String(e)}`);
      }
      return;
    }
    // 库节点：表 / 视图 / 函数 / 查询 / 备份 分组
    if (/^conn:[^/]+\/db\/[^/]+$/.test(node.id)) {
      const connId = node.id.slice(5).split("/db/")[0];
      const dbName = node.id.split("/db/")[1];
      try {
        const tables = await api.listTables(connId, dbName);
        const routines = await api.listRoutines(connId, dbName);
        const mkObj = (t: string, type: string, icon: string): TreeItem => ({
          id: `${node.id}/obj/${type}/${t}`,
          label: t,
          type: type as TreeItem["type"],
          icon,
          children: [],
          expandable: false,
          has_children: false,
          expanded: false,
        });
        const tbl = tables
          .filter(([, ty]) => ty === "table")
          .map(([t]) => ({
            id: `${node.id}/table/${t}`,
            label: t,
            type: "table" as ObjectType,
            icon: "info_pane_table",
            children: [],
            expandable: true,
            has_children: true,
            expanded: false,
          }));
        const views = tables
          .filter(([, ty]) => ty === "view")
          .map(([t]) => ({
            id: `${node.id}/view/${t}`,
            label: t,
            type: "view" as ObjectType,
            icon: "info_pane_view",
            children: [],
            expandable: true,
            has_children: true,
            expanded: false,
          }));
        const funcs = routines
          .filter(([ty]) => ty === "function" || ty === "procedure")
          .map(([ty, n]) => mkObj(n, ty, "info_pane_function"));
        // 查询分组：列出收藏夹中保存的查询（connection 匹配或通用）
        const savedQueries: TreeItem[] = this.favorites
          .filter((f) => !f.connection_id || f.connection_id === connId)
          .map((f) => ({
            id: `${node.id}/query/${f.id}`,
            label: f.name,
            type: "query" as ObjectType,
            icon: "info_pane_query",
            children: [],
            expandable: false,
            has_children: false,
            expanded: false,
          }));
        const group = (suffix: string, label: string, icon: string, kids: TreeItem[]): TreeItem => ({
          id: `${node.id}/${suffix}`,
          label,
          type: "folder",
          icon,
          children: kids,
          expandable: true,
          has_children: kids.length > 0,
          expanded: false,
        });
        node.children = [
          // 分组导航入口与命令条同名按钮同源（pict_* → pict.ts 具象图标），保证联动视觉一致
          group(`grp-table`, "表", "pict_table", tbl),
          group(`grp-view`, "视图", "pict_view", views),
          group(`grp-function`, "函数", "pict_function", funcs),
          group(`grp-query`, "查询", "pict_query", savedQueries),
          group(`grp-backup`, "备份", "pict_backup", []),
        ];
      } catch (e) {
        this.toast("error", `无法列出对象：${String(e)}`);
      }
      return;
    }
    // 表 / 视图节点：字段 / 索引 / 外键 / 触发器 分组
    // 表名必须是 id 的最后一段（不允许再带 /xxx 深层），防止分组 / 字段
    // 叶子的 id 被误当表名进入本分支。
    const tblMatch = node.id.match(
      /^conn:([^/]+)(?:\/db\/([^/]+))?\/(table|view)\/([^/]+)$/,
    );
    if (tblMatch) {
      const [, connId, schema, , table] = tblMatch;
      try {
        const s = await api.getTableSchema(connId, schema ?? "", table);
        const kind = this.connections.find((c) => c.id === connId)?.kind ?? "mysql";
        const cols: TreeItem[] = s.columns.map((c) => ({
          id: `${node.id}/column/${c.name}`,
          label: c.name,
          type: "column" as ObjectType,
          icon: "info_pane_column",
          children: [],
          expandable: false,
          has_children: false,
          expanded: false,
        }));
        const idx: TreeItem[] = s.indexes.map((i) => ({
          id: `${node.id}/index/${i.name}`,
          label: `${i.name} (${i.columns.join(", ")})`,
          type: "index" as ObjectType,
          icon: "info_pane_index",
          children: [],
          expandable: false,
          has_children: false,
          expanded: false,
        }));
        const fks: TreeItem[] = s.foreign_keys.map((f) => ({
          id: `${node.id}/fk/${f.name}`,
          label: `${f.name} → ${f.ref_table}`,
          type: "index" as ObjectType,
          icon: "info_pane_fk",
          children: [],
          expandable: false,
          has_children: false,
          expanded: false,
        }));
        const trs: TreeItem[] = await this.loadTableTriggers(
          connId,
          kind,
          schema ?? "",
          table,
          node.id,
        );
        const group = (suffix: string, label: string, icon: string, kids: TreeItem[]): TreeItem => ({
          id: `${node.id}/${suffix}`,
          label,
          type: "folder",
          icon,
          children: kids,
          expandable: true,
          has_children: kids.length > 0,
          expanded: false,
        });
        node.children = [
          group("grp-column", "字段", "info_pane_column", cols),
          group("grp-index", "索引", "info_pane_index", idx),
          group("grp-fk", "外键", "info_pane_fk", fks),
          group("grp-trigger", "触发器", "trigger", trs),
        ];
      } catch {
        node.children = [];
      }
    }
  }

  /** 读取表上的触发器（按数据库类型）。baseId 为所属表节点 id。 */
  private async loadTableTriggers(
    connId: string,
    kind: string,
    schema: string,
    table: string,
    baseId: string,
  ): Promise<TreeItem[]> {
    const esc = (s: string) => s.replace(/'/g, "''");
    let sql = "";
    if (kind === "mysql") {
      sql =
        "SELECT TRIGGER_NAME FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA = " +
        `'${esc(schema)}' AND EVENT_OBJECT_TABLE = '${esc(table)}'`;
    } else if (kind === "sqlite") {
      sql = `SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = '${esc(table)}'`;
    } else {
      sql = `SELECT tgname FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid WHERE c.relname = '${esc(table)}' AND NOT tgisinternal`;
    }
    try {
      const res = await api.executeSql(connId, sql);
      const set = res.sets[0];
      if (!set) return [];
      const nameCol = set.columns.findIndex((c) => /name/i.test(c.name));
      return set.rows
        .map((r) => (nameCol >= 0 ? r[nameCol] : r[0]))
        .filter((n): n is string => !!n)
        .map((n) => ({
          // 挂在所属表的路径下（baseId = 表节点 id），保证右键 / 双击
          // 能解析到正确的表
          id: `${baseId}/trigger/${n}`,
          label: n,
          type: "trigger" as ObjectType,
          icon: "trigger",
          children: [],
          expandable: false,
          has_children: false,
          expanded: false,
        }));
    } catch {
      return [];
    }
  }

  /** 打开数据库：亮起图标、显示展开箭头并加载子节点（双击 / 「打开数据库」）。 */
  private async openDatabaseNode(node: TreeItem, connId: string, dbName: string): Promise<void> {
    if (node.loading) return;
    // 已加载且仍处于打开状态：只需重新展开，无需再次打开连接/库
    if (node.loaded && this.openDbs.has(`${connId}/${dbName}`)) {
      node.expandable = true;
      node.expanded = true;
      this.syncConnIcons();
      this.tree.render();
      return;
    }
    node.loading = true;
    try {
      await this.ensureConnection(connId);
      const opened = await api.openDatabase(connId, dbName);
      if (!opened.ok) throw new Error(opened.message);
      if (!node.loaded) {
        await this.loadChildren(node);
        node.loaded = true;
      }
      this.openDbs.add(`${connId}/${dbName}`);
      node.expandable = true;
      node.expanded = true;
      this.syncConnIcons();
      this.tree.render();
      this.openObjectsTab(connId, dbName);
    } catch (e) {
      this.openDbs.delete(`${connId}/${dbName}`);
      node.loaded = false;
      this.toast("error", `打开数据库失败：${String(e)}`);
      this.syncConnIcons();
    } finally {
      node.loading = false;
    }
  }

  private async ensureConnection(connId: string): Promise<void> {
    if (this.openConnections.has(connId)) return;
    const result = await api.ensureConnection(connId);
    if (!result.ok) throw new Error(result.message);
    this.openConnections.add(connId);
    this.syncConnIcons();
  }

  /** 关闭数据库：图标熄灭、收起并隐藏展开箭头。 */
  private closeDatabaseNode(connId: string, dbName: string): void {
    this.openDbs.delete(`${connId}/${dbName}`);
    const node = this.tree.find(`conn:${connId}/db/${dbName}`);
    if (node) {
      node.expanded = false;
      node.expandable = false;
      node.children = [];
      node.loaded = false;
    }
    this.syncConnIcons();
    this.tree.render();
  }

  /** 选中树节点：仅移动选中光标；库节点单击不打开、不亮起（双击/「打开数据库」才打开）。 */
  private async selectNode(node: TreeItem): Promise<void> {
    this.infoRequest++;
    this.updateInfoPanelForNode(node);
    if (/grp-backup$/.test(node.id)) {
      const connId = node.id.slice(5).split("/db/")[0];
      const dbName = node.id.split("/db/")[1].replace(/\/grp-backup$/, "");
      this.openSpecialTab("backup", connId, dbName);
      return;
    }
    if (/grp-(table|view|function)$/.test(node.id)) {
      const connId = node.id.slice(5).split("/db/")[0];
      const dbName = node.id.split("/db/")[1].replace(/\/(grp-\w+)$/, "");
      this.syncDbIcons(node.id);
      // 「表」分组只展示表，「视图」只展示视图，「函数」只展示函数 / 存储过程
      const filter: ObjectType = /grp-table$/.test(node.id)
        ? "table"
        : /grp-view$/.test(node.id)
          ? "view"
          : "function";
      this.openObjectsTab(connId, dbName, filter);
      return;
    }
    // 纯库节点：单击显示对象页（不打开/展开库；展开状态由双击维护）
    if (/^conn:[^/]+\/db\/[^/]+$/.test(node.id)) {
      const connId = node.id.slice(5).split("/db/")[0];
      const dbName = node.id.split("/db/")[1];
      this.openObjectsTab(connId, dbName, "folder", false);
      return;
    }
    // 单击连接仅选中；双击才打开，避免单击时重建节点破坏双击目标。
  }

  /** 打开备份 / 用户等特殊标签页。 */
  openSpecialTab(kind: "backup" | "users", connId: string, schema: string): void {
    const cfg = this.connections.find((c) => c.id === connId);
    if (!cfg) return;
    const existing = this.tabs.find(
      (t) => t.kind === kind && t.connectionId === connId && t.schema === schema,
    );
    if (existing) {
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    const tab: TabState = {
      id: `${kind[0]}_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`,
      kind,
      title: kind === "backup" ? "备份" : "用户",
      connectionId: connId,
      connectionName: cfg.name,
      schema,
      object: "",
      objectType: "table",
      sql: "",
      results: null,
      activeResult: 0,
      page: null,
      pageOffset: 0,
      pageLimit: this.settings.rows_per_page,
      orderBy: null,
      ascending: true,
      filters: [],
      schemaInfo: null,
      dirty: false,
      selectedRows: new Set(),
      editing: null,
    };
    this.tabs.push(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
  }

  /** 通用标签页工厂：减少各 open* 方法的重复样板。 */
  private makeTab(
    kind: TabState["kind"],
    title: string,
    connId: string,
    schema: string,
    object: string,
    objectType: ObjectType = "table",
    sql = "",
  ): TabState {
    const cfg = this.connections.find((c) => c.id === connId);
    return {
      id: `${kind[0]}_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`,
      kind,
      title,
      connectionId: connId,
      connectionName: cfg?.name ?? "",
      schema,
      object,
      objectType,
      sql,
      results: null,
      activeResult: 0,
      page: null,
      pageOffset: 0,
      pageLimit: this.settings.rows_per_page,
      orderBy: null,
      ascending: true,
      filters: [],
      schemaInfo: null,
      dirty: false,
      selectedRows: new Set(),
      editing: null,
    };
  }

  /** 打开视图设计器（读取定义 → 可编辑 → 重建视图）。 */
  async openViewDesignTab(connId: string, schema: string, view: string): Promise<void> {
    const cfg = this.connections.find((c) => c.id === connId);
    if (!cfg) return;
    const existing = this.tabs.find(
      (t) => t.kind === "viewdesign" && t.connectionId === connId && t.object === view,
    );
    if (existing) {
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    let sql = "";
    try {
      sql = await api.getViewDefinition(connId, schema, view);
    } catch (e) {
      this.toast("error", `读取视图定义失败：${String(e)}`);
      return;
    }
    const tab = this.makeTab("viewdesign", view, connId, schema, view, "view", sql);
    this.tabs.push(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
  }

  /** 打开函数 / 存储过程源码（可编辑重建）。 */
  async openRoutineDesignTab(
    connId: string,
    schema: string,
    name: string,
    routineType: string,
  ): Promise<void> {
    const cfg = this.connections.find((c) => c.id === connId);
    if (!cfg) return;
    const existing = this.tabs.find(
      (t) =>
        t.kind === "routinedesign" && t.connectionId === connId && t.object === name,
    );
    if (existing) {
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    let sql = "";
    try {
      sql = await api.getRoutineDefinition(connId, schema, name, routineType);
    } catch (e) {
      this.toast("error", `读取定义失败：${String(e)}`);
      return;
    }
    const tab = this.makeTab(
      "routinedesign",
      name,
      connId,
      schema,
      name,
      routineType as ObjectType,
      sql,
    );
    this.tabs.push(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
  }

  /** 打开模型视图（当前库的 ER 关系图）。 */
  openModelTab(connId: string, schema: string): void {
    const cfg = this.connections.find((c) => c.id === connId);
    if (!cfg) return;
    const existing = this.tabs.find(
      (t) => t.kind === "model" && t.connectionId === connId && t.schema === schema,
    );
    if (existing) {
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    const tab = this.makeTab("model", `模型 - ${schema}`, connId, schema, "");
    this.tabs.push(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
  }

  /** 只读取当前活动视图已经取得的数据，不从连接树或其它页借用结果。 */
  private currentChartSource(): ChartSource | null {
    const tab = this.activeTab;
    if (!tab) return null;
    const view = this.views.get(tab.id);
    if (tab.kind === "query" && view instanceof QueryTabView) return view.chartSource();
    if (tab.kind === "table" && view instanceof DataTabView && tab.page) {
      return {
        title: tab.object || tab.title,
        description: `${tab.connectionName || tab.connectionId} / ${tab.schema} · 当前页已加载数据库数据（不含未提交编辑）`,
        columns: tab.page.columns.map(column => column.name), rows: tab.page.rows,
      };
    }
    return null;
  }

  private cmdCreateChart(): void {
    const sourceTab = this.activeTab;
    const source = this.currentChartSource();
    const error = chartSourceError(source);
    if (!sourceTab || !source || error) { this.toast("error", error ?? "当前没有可绘制的数据。"); return; }
    const tab = this.makeTab("chart", `图表 - ${source.title}`, sourceTab.connectionId, sourceTab.schema, sourceTab.object);
    tab.connectionName = sourceTab.connectionName;
    tab.chart = createChartState(source);
    this.tabs.push(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
  }

  /** 打开（或聚焦）某连接的对象标签页。
   *  对象页每个连接共用一个：固定在标签条左侧（其它页签之前），
   *  单击树上不同分组 / 不同库只就地切换内容，不再新开页签。
   *  filter：对象来源（树上选中「表」分组只展示表、「视图」分组只展示视图、库节点展示全部）。 */
  openObjectsTab(connId: string, schema: string, filter: ObjectType = "folder", markOpened = true): void {
    const cfg = this.connections.find((c) => c.id === connId);
    if (!cfg) return;
    // 记录「已打开的库」，树中该库图标显示为绿色高亮。
    // 单击库节点仅浏览对象（markOpened=false），不改库图标，避免 mousedown
    // 期间重建图标节点而破坏后续 click/dblclick 的目标。
    if (markOpened) this.openDbs.add(`${connId}/${schema}`);
    this.openConnections.add(connId);
    this.syncConnIcons();
    const title =
      filter === "table" ? "表" : filter === "view" ? "视图" : filter === "function" ? "函数" : "对象";
    const existing = this.tabs.find(
      (t) => t.kind === "objects" && t.connectionId === connId,
    );
    if (existing) {
      // 共用对象页：切换内容（库 + 过滤）并聚焦，不新开页签
      existing.schema = schema;
      existing.objectType = filter;
      existing.title = title;
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    const tab: TabState = {
      id: `o_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`,
      kind: "objects",
      title,
      connectionId: connId,
      connectionName: cfg.name,
      schema,
      object: "",
      objectType: filter,
      sql: "",
      results: null,
      activeResult: 0,
      page: null,
      pageOffset: 0,
      pageLimit: this.settings.rows_per_page,
      orderBy: null,
      ascending: true,
      filters: [],
      schemaInfo: null,
      dirty: false,
      selectedRows: new Set(),
      editing: null,
    };
    // 固定左侧：插到已有对象页之后、其余页签之前
    const insertAt = this.tabs.filter((t) => t.kind === "objects").length;
    this.tabs.splice(insertAt, 0, tab);
    this.activeTabId = tab.id;
    this.renderTabs();
  }

  /** 双击树节点。 */
  private async openNode(node: TreeItem): Promise<void> {
    // 节点 id 形如：
    //   conn:<id>                     仅连接本身
    //   conn:<id>/table/<name>        表 / 视图（SQLite）
    //   conn:<id>/db/<schema>/table/<name>
    //   conn:<id>/db/<schema>/obj/procedure/<name>
    const parts = node.id.split("/");
    const connId = parts[0].slice(5);

    if (parts.length === 1) {
      if (node.id.startsWith("group:")) { this.tree.toggle(node.id); return; }
      await this.ensureConnection(connId);
      await this.toggleNode(node);
      const cfg = this.connectionById(connId);
      if (cfg?.kind === "sqlite") this.openObjectsTab(connId, "main");
      return;
    }

    // 库节点：双击 = 已展开则收起（不关闭、不清空子节点），未展开则打开
    const dbNodeMatch = /^conn:([^/]+)\/db\/([^/]+)$/.exec(node.id);
    if (dbNodeMatch) {
      if (node.expanded) {
        node.expanded = false;
        this.syncConnIcons();
        this.tree.render();
        return;
      }
      await this.openDatabaseNode(node, dbNodeMatch[1], dbNodeMatch[2]);
      return;
    }

    // 分组节点（表/视图/函数/查询/备份、字段/索引/外键/触发器）：
    // 双击 = 展开 / 折叠，与点击左侧展开箭头一致
    // （打开对象页 / 备份页由单击 selectNode 负责，双击不再重复打开页签）。
    if (parts[parts.length - 1]?.startsWith("grp-")) {
      await this.toggleNode(node);
      return;
    }

    // 保存的查询节点：用其 SQL 打开查询页
    const qIdx = parts.indexOf("query");
    if (qIdx > 0 && parts.length > qIdx + 1) {
      const fav = this.favorites.find((f) => f.id === parts[qIdx + 1]);
      if (fav) {
        const dq = parts.indexOf("db");
        await this.openQueryTab(fav.connection_id ?? connId, fav.sql, dq >= 0 ? parts[dq + 1] : "");
        return;
      }
    }

    const dbIdx = parts.indexOf("db");
    const schema = dbIdx >= 0 ? parts[dbIdx + 1] : "";

    // 字段 / 索引 / 外键 / 触发器叶子：打开所属表的结构页
    // （必须先于下方 table 分支判断，否则这些叶子的 id 中含有 table 段，
    //  双击会错开成表数据页）。
    if (
      parts.includes("column") ||
      parts.includes("index") ||
      parts.includes("fk") ||
      parts.includes("trigger")
    ) {
      const tblIdx = parts.indexOf("table");
      const viewIdx = parts.indexOf("view");
      const t = tblIdx >= 0 ? parts[tblIdx + 1] : viewIdx >= 0 ? parts[viewIdx + 1] : "";
      if (t) void this.openStructureTab(connId, schema, t);
      return;
    }

    for (const kind of ["table", "view", "procedure", "function", "trigger", "column", "index"]) {
      const idx = parts.indexOf(kind);
      if (idx < 0) continue;
      const objName = parts[idx + 1];
      if (!objName) continue;
      if (kind === "table" || kind === "view") {
        await this.openTableTab(connId, schema, objName, node.type);
      } else if (kind === "column" || kind === "index") {
        // 字段 / 索引节点：打开所属表的结构
        const tblIdx = parts.indexOf("table");
        const viewIdx = parts.indexOf("view");
        const t = tblIdx >= 0 ? parts[tblIdx + 1] : viewIdx >= 0 ? parts[viewIdx + 1] : "";
        if (t) void this.openStructureTab(connId, schema, t);
      } else {
        await this.openRoutineTab(connId, schema, objName, node.type);
      }
      return;
    }
  }

  // ------------------------------------------------------------ 标签页

  private renderTabs(): void {
    this.tabStrip.setTabs(this.tabs, this.activeTabId);
    this.renderActiveView();
    // 菜单栏上下文项（表 / 查询）随标签页类型变化；「查看」勾选态实时同步
    this.menu.renderBar(this.menuHost, this.contextMenus(), this.viewMenu());
  }

  /** 供视图内部调用：整体重渲染标签页。 */
  renderTabsPublic(): void {
    this.renderTabs();
  }

  /** 供视图内部调用：刷新信息面板。 */
  updateInfoPanelPublic(): void {
    this.updateInfoPanel();
  }

  /** 供视图内部调用：刷新二级工具栏启用态。 */
  notifyToolbar(): void {
    const tab = this.activeTab;
    if (tab) this.tabToolbar.setItems(this.guardedTabToolbarItems(tab));
  }

  /** 连接 id → 显示名（未找到时原样返回）。 */
  connectionName(id: string): string {
    return this.connections.find((c) => c.id === id)?.name ?? id;
  }

  /** 当前标签页对应的菜单栏上下文菜单。 */
  private contextMenus(): import("../ui/menu").MenuBarEntry[] {
    const tab = this.activeTab;
    if (!tab) return [];
    return tab.kind === "query" ? [QUERY_MENU] : [];
  }

  /** 动态「查看」菜单（勾选态随面板开关实时同步）。 */
  private viewMenu(): import("../ui/menu").MenuBarEntry {
    return {
      label: "查看",
      children: [
        { id: "toggle-sidebar", label: "侧边栏", checked: this.sidebarVisible },
        { id: "toggle-info-panel", label: "信息面板", checked: this.infoPanelVisible },
        { id: "toggle-statusbar", label: "状态栏", checked: this.statusBarVisible },
        { id: "toggle-toolbar", label: "工具栏", checked: this.toolbarVisible },
        { separator: true },
        { id: "show-grid", label: "显示网格", checked: this.showGridLines },
        { id: "show-null", label: "跳过 Null", checked: this.skipNull },
        { separator: true },
        { id: "refresh", label: "刷新", shortcut: "F5" },
        { id: "expand-all", label: "展开全部" },
        { id: "collapse-all", label: "折叠全部" },
        { separator: true },
        { id: "focus-mode", label: "专注模式", shortcut: "F11" },
      ],
    };
  }

  private activateTab(id: string): void {
    if (this.activeTabId === id) return;
    this.activeTabId = id;
    this.renderTabs();
  }

  private closeTab(id: string): void {
    const tab = this.tabs.find(t => t.id === id);
    if (!tab) return;
    if (this.manualTransactionIsBusy(tab.connectionId)) {
      this.toast("info", "事务操作进行中，请完成后再关闭页面");
      return;
    }
    const transaction = this.manualTransactionFor(tab.connectionId);
    if (transaction?.owner === id) {
      void this.closeManualTransactionTab(tab);
      return;
    }
    this.removeTabNow(id);
  }

  private removeTabNow(id: string): void {
    const i = this.tabs.findIndex((t) => t.id === id);
    if (i < 0) return;
    this.views.get(id)?.destroy();
    this.views.delete(id);
    this.tabs.splice(i, 1);
    // 库的打开状态由「打开 / 关闭数据库」显式维护，关闭标签页不熄灭库图标
    if (this.activeTabId === id) {
      this.activeTabId = this.tabs.length
        ? this.tabs[Math.min(i, this.tabs.length - 1)].id
        : null;
    }
    this.renderTabs();
  }

  /** 当前激活视图。 */
  private activeView(): TabView | undefined {
    return this.activeTabId ? this.views.get(this.activeTabId) : undefined;
  }

  /** 渲染当前激活标签页的视图。 */
  private renderActiveView(): void {
    // 移除旧视图
    for (const [id, view] of this.views) {
      if (id !== this.activeTabId) {
        view.el.remove();
      }
    }
    const tab = this.tabs.find((t) => t.id === this.activeTabId);
    if (!tab) {
      // v2 引导式空态（设计图「对象页 · 空态」）：中央图标 + 下一步动作按钮
      this.contentHost.innerHTML = "";
      const empty = document.createElement("div");
      empty.className = "empty-state";
      const glyph = document.createElement("div");
      glyph.className = "empty-state-glyph";
      glyph.innerHTML = svg(ICON_EMPTY_CONNECT, 30);
      const title = document.createElement("div");
      title.className = "empty-state-title";
      title.textContent = "还没有打开的连接";
      const text = document.createElement("div");
      text.className = "empty-state-text";
      text.textContent = `新建一个连接开始工作。NavKit 支持 ${DB_KINDS.length} 种数据源，包括 MySQL、PostgreSQL、SQL Server 与国产数据库。`;
      const actions = document.createElement("div");
      actions.className = "empty-state-actions";
      const mkAction = (label: string, primary: boolean, cmd: string) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = primary ? "btn primary" : "btn";
        b.textContent = label;
        b.addEventListener("click", () => void this.runCommand(cmd));
        return b;
      };
      actions.appendChild(mkAction("＋ 新建连接", true, "new-connection"));
      actions.appendChild(mkAction("打开现有连接", false, "open-connection"));
      empty.append(glyph, title, text, actions);
      this.contentHost.appendChild(empty);
      this.tabToolbar.setItems([]);
      this.updateInfoPanel();
      this.ribbon.setActive(null);
      return;
    }
    let view = this.views.get(tab.id);
    if (!view) {
      view = this.createView(tab);
      this.views.set(tab.id, view);
    }
    this.contentHost.innerHTML = "";
    this.contentHost.appendChild(view.el);
    if (!tab.datasourceStale) view.activate();
    this.ribbon.updateDisabled();
    this.tabToolbar.setItems(this.guardedTabToolbarItems(tab));
    this.renderStatus();
    // Ribbon 变体：SQL Server 连接时「用户」带下拉并在「其它」后插入「SQL Server 备份」
    {
      const cfgR = this.connections.find((c) => c.id === tab.connectionId);
      if (cfgR?.kind === "sqlserver") {
        const items = RIBBON_MAIN.map((it) =>
          it.id === "open-user" ? { ...it, dropdown: true } : { ...it },
        );
        const idx = items.findIndex((it) => it.id === "backup");
        items.splice(idx + 1, 0, {
          id: "sqlserver-backup",
          icon: "ribbon_backup",
          label: "SQL Server 备份",
        });
        this.ribbon.setItems(items);
      } else {
        this.ribbon.setItems(RIBBON_MAIN);
      }
    }
    this.updateInfoPanel();
    // Ribbon 高亮跟随标签页类型（对象页按过滤类型高亮「表 / 视图 / 函数」）
    this.ribbon.setActive(
      tab.kind === "query"
        ? "open-query"
        : tab.kind === "backup"
          ? "backup"
          : tab.kind === "schedule"
            ? "schedule"
            : tab.kind === "model"
              ? "model"
              : tab.kind === "chart"
                ? "chart"
                : tab.kind === "users"
                  ? "open-user"
                  : tab.kind === "objects" && tab.objectType === "view"
                    ? "open-view"
                    : tab.kind === "objects" && tab.objectType === "function"
                      ? "open-function"
                      : "open-table",
    );
  }

  /** 为标签页创建视图。 */
  private createView(tab: TabState): TabView {
    switch (tab.kind) {
      case "objects":
        return new ObjectsTabView(this, tab);
      case "query":
        return new QueryTabView(this, tab);
      case "structure":
        return new StructureTabView(this, tab);
      case "backup":
        return new BackupTabView(this, tab);
      case "users":
        return new UsersTabView(this, tab);
      case "schedule":
        return new ScheduleTabView(this, tab);
      case "model":
        return new ModelTabView(this, tab);
      case "viewdesign":
        return new SqlDesignTabView(this, tab, "view");
      case "routinedesign":
        return new SqlDesignTabView(this, tab, "routine");
      case "chart":
        return new ChartTabView(this, tab);
      default:
        return new DataTabView(this, tab);
    }
  }

  /** 当前标签页对应的二级工具栏按钮。 */
  private guardedTabToolbarItems(tab: TabState): TabToolItem[] {
    return this.tabToolbarItems(tab).map(item => {
      if (!MANUAL_TRANSACTION_WRITE_COMMANDS.has(item.id) && !(item.id === "save" && tab.kind !== "query")) return item;
      const reason = this.commandUnavailableReason(item.id);
      return reason ? { ...item, disabled: true, disabledReason: reason } : item;
    });
  }

  private tabToolbarItems(tab: TabState): TabToolItem[] {
    if (!tab) return [];
    switch (tab.kind) {
      case "backup": {
        const view = this.views.get(tab.id);
        const rowSelected =
          view instanceof BackupTabView && (tab.selectedRows?.size ?? 0) > 0;
        return [
          { id: "new-backup", label: "新建备份", icon: "new_table" },
          { id: "restore-backup", label: "还原备份", icon: "open_table", disabled: !rowSelected },
          { id: "open-backup-folder", label: "打开所在文件夹", icon: "open_table" },
          { id: "delete-backup", label: "删除备份", icon: "delete_table", disabled: !rowSelected },
          { id: "refresh", label: "刷新", icon: "dbn_refresh", alignRight: true },
        ];
      }
      case "users": {
        const cfg = this.connections.find((c) => c.id === tab.connectionId);
        const isMysql =
          cfg?.kind === "mysql" || cfg?.kind === "tidb" || cfg?.kind === "oceanbase"
          || cfg?.kind === "polardb" || cfg?.kind === "tdsql" || cfg?.kind === "doris";
        const view = this.views.get(tab.id);
        const rowSelected = view instanceof UsersTabView && (tab.selectedRows?.size ?? 0) > 0;
        return [
          { id: "edit-user", label: "修改密码", icon: "design_table", disabled: !(isMysql && rowSelected) },
          { id: "new-user", label: "新建用户", icon: "new_table", disabled: !isMysql },
          { id: "delete-user", label: "删除用户", icon: "delete_table", disabled: !(isMysql && rowSelected) },
          { id: "refresh", label: "刷新", icon: "dbn_refresh" },
        ];
      }
      case "schedule":
        return [
          { id: "new-job", label: "新建任务", icon: "new_table" },
          { id: "edit-job", label: "编辑任务", icon: "design_table", disabled: (tab.selectedRows?.size ?? 0) === 0 },
          { id: "run-job", label: "立即运行", icon: "run_sel", disabled: (tab.selectedRows?.size ?? 0) === 0 },
          { id: "delete-job", label: "删除任务", icon: "delete_table", disabled: (tab.selectedRows?.size ?? 0) === 0 },
          { id: "job-history", label: "运行历史", icon: "editor" },
          { id: "refresh", label: "刷新", icon: "dbn_refresh" },
        ];
      case "model":
        return [
          { id: "model-fit", label: "适应窗口", icon: "full_screen" },
          { id: "model-zoom-in", label: "放大", icon: "bb_zoom_in" },
          { id: "model-zoom-out", label: "缩小", icon: "bb_zoom_in" },
          { id: "refresh", label: "刷新", icon: "dbn_refresh" },
        ];
      case "viewdesign":
      case "routinedesign":
        return [
          { id: "save", label: "保存", icon: "dbn_post" },
          { id: "refresh", label: "重新载入定义", icon: "dbn_refresh" },
        ];
      case "objects": {
        const view = this.views.get(tab.id);
        const rowSelected =
          view instanceof ObjectsTabView &&
          view.grid.rows.length > 0 &&
          (tab.selectedRows?.size ?? 0) > 0;
        const selIsTable =
          view instanceof ObjectsTabView && view.selectedObjectType() === "table";
        return [
          { id: "open-table", label: "打开表", icon: "open_table", disabled: !rowSelected },
          { id: "design-table", label: "设计表", icon: "design_table", disabled: !selIsTable },
          { id: "new-table", label: "新建表", icon: "new_table" },
          { id: "delete-table", label: "删除表", icon: "delete_table", disabled: !selIsTable },
          { id: "import", label: "导入向导", icon: "import_wizard" },
          { id: "export", label: "导出向导", icon: "export_wizard" },
        ];
      }
      case "query":
        return [
          { id: "save", label: "保存", icon: "dbn_post" },
          { id: "query-builder", label: "查询创建工具", icon: "query_builder" },
          { id: "beautify-sql", label: "美化 SQL", icon: "editor" },
          { id: "code-snippet", label: "代码段", icon: "query_pin_ent_preview_image" },
          { id: "export", label: "导出结果", icon: "data_sync_ent_preview_image" },
          {
            id: "chart",
            label: "创建图表",
            icon: "data_profiling_ent_preview_image",
          },
          ...this.manualTransactionTools(tab),
        ];
      case "structure": {
        const structure = this.views.get(tab.id);
        const view = structure instanceof StructureTabView ? structure : null;
        const upReason = view ? view.fieldMoveReason(-1) : "请先等待结构加载完成";
        const downReason = view ? view.fieldMoveReason(1) : "请先等待结构加载完成";
        const busy = view?.isSavingStructure() ?? false;
        return [
          { id: "save", label: "保存", icon: "dbn_post", disabled: busy },
          { id: "add-field", label: "添加字段", icon: "dbn_insert", disabled: busy },
          { id: "insert-field", label: "插入字段", icon: "dbn_edit", disabled: busy },
          { id: "delete-field", label: "删除字段", icon: "dbn_delete", disabled: busy },
          { id: "primary-key", label: "主键", icon: "dbn_edit", disabled: busy },
          { id: "move-up", label: "上移", icon: "dbn_prior", disabled: !!upReason, disabledReason: upReason ?? undefined },
          { id: "move-down", label: "下移", icon: "dbn_next", disabled: !!downReason, disabledReason: downReason ?? undefined },
        ];
      }
      case "chart":
        return [{ id: "refresh", label: "刷新", icon: "dbn_refresh" }];
      default: {
        const view = this.views.get(tab.id);
        const hasRows = view instanceof DataTabView && view.rowCount > 0;
        return [
          { id: "text", label: "文本", icon: "editor", dropdown: true },
          { id: "filter", label: "筛选", icon: "data_filter" },
          { id: "sort", label: "排序", icon: "tab_sort" },
          { id: "columns", label: "列", icon: "columns", disabled: !hasRows },
          { id: "import", label: "导入", icon: "data_transfer_ent_preview_image" },
          { id: "export", label: "导出", icon: "data_sync_ent_preview_image", disabled: !hasRows },
          {
            id: "data-generation",
            label: "数据生成",
            icon: "data_generation_ent_preview_image",
            disabled: this.connectionById(tab.connectionId)?.read_only ?? false,
          },
          {
            id: "chart",
            label: "创建图表",
            icon: "data_profiling_ent_preview_image",
          },
          ...this.manualTransactionTools(tab).map(item => ({ ...item, alignRight: true })),
        ];
      }
    }
  }

  /** 点击树或对象清单后只更新详情，不隐式连接、不替换树节点。 */
  private updateInfoPanelForNode(node: TreeItem): void {
    if (node.id.startsWith("group:")) {
      const group = node.id.slice(6);
      this.infoPanel.setState({ icon: node.icon, title: group, subtitle: "连接组", fields: [
        { label: "连接数量", value: String(this.connections.filter((c) => c.group === group).length) },
        { label: "操作", value: "拖动连接到此组，或右键新建连接" },
      ] });
      return;
    }
    const parts = node.id.split("/");
    const connId = parts[0].slice(5);
    const cfg = this.connectionById(connId);
    if (!cfg) return;
    const dbIdx = parts.indexOf("db");
    const schema = dbIdx >= 0 ? parts[dbIdx + 1] : "";
    const backup = node.id.includes("grp-backup");
    const object = node.type === "connection" || node.type === "database" || node.id.endsWith("grp-backup") ? "" : node.label;
    const tableIndex = parts.indexOf("table");
    const viewIndex = parts.indexOf("view");
    const parentTable = tableIndex >= 0 ? parts[tableIndex + 1] : viewIndex >= 0 ? parts[viewIndex + 1] : undefined;
    this.showObjectDetails(cfg, schema, object, node.type, backup
      ? { category: "backup", backupName: node.id.endsWith("grp-backup") ? undefined : node.label }
      : { parentTable });
  }

  showObjectDetails(cfg: ConnectionConfig, schema: string, object: string, type: ObjectType,
    options?: import("./objectDetails").ObjectDetailsOptions): void {
    const request = ++this.infoRequest;
    this.infoPanel.setState({ icon: type === "connection" ? connIconOf(cfg) : "info_pane_database",
      title: object || schema || cfg.name, subtitle: "读取详细信息", fields: [] });
    void loadObjectDetails(cfg, schema, object, type, options).then((state) => {
      if (request === this.infoRequest) this.infoPanel.setState(state);
    }).catch((e) => {
      if (request === this.infoRequest) this.infoPanel.setState({ icon: "info_pane_database", title: object || schema || cfg.name,
        fields: [{ label: "读取失败", value: String(e) }] });
    });
  }

  /** 右侧信息面板内容。 */
  private updateInfoPanel(): void {
    const tab = this.activeTab;
    if (tab?.datasourceStale) {
      this.infoPanel.setState({ icon: "info_pane_database", title: "数据源已变化", subtitle: tab.datasourceStale, fields: [] });
      return;
    }
    if (!tab) {
      const first = this.connections[0];
      this.infoPanel.setState(
        first
          ? {
              icon: connIconOf(first),
              title: first.name,
              subtitle: DB_META[first.kind]?.label ?? first.kind,
              fields: [
                { label: "主机", value: first.host || "localhost" },
                ...(first.port ? [{ label: "端口", value: String(first.port) }] : []),
                ...(first.database ? [{ label: "数据库", value: first.database }] : []),
              ],
            }
          : null,
      );
      return;
    }
    if (tab.kind === "table") {
      // 表数据页：右侧为「表信息 / DDL / 列信息」三视图
      this.renderTableInfoPanel(tab);
      return;
    }
    if (tab.kind === "query") {
      // 参考截图 5：查询页右侧为代码段面板
      const view = this.activeView();
      this.infoPanel.setSnippets(QUERY_SNIPPETS, (item) => {
        if (view instanceof QueryTabView) {
          view.editor.insertText(item.sql ?? "");
          view.editor.focus();
        }
      });
      return;
    }
    if (tab.kind === "structure" && tab.schemaInfo) {
      const s = tab.schemaInfo;
      this.infoPanel.setState({
        icon: "info_pane_table",
        title: s.table,
        subtitle: "表",
        refs: this.connectionRef(tab.connectionId),
        fields: [
          { label: "数据库", value: s.database || tab.schema },
          { label: "字段数", value: String(s.columns.length) },
          { label: "索引数", value: String(s.indexes.length) },
          { label: "外键数", value: String(s.foreign_keys.length) },
          ...(s.engine ? [{ label: "引擎", value: s.engine }] : []),
          ...(s.collation ? [{ label: "排序规则", value: s.collation }] : []),
          ...(s.comment ? [{ label: "注释", value: s.comment }] : []),
        ],
      });
      return;
    }
    if (tab.kind === "objects" || tab.kind === "backup" || tab.kind === "users") {
      const cfg = this.connectionById(tab.connectionId);
      if (cfg) {
        const view = this.views.get(tab.id);
        const selected = view instanceof ObjectsTabView ? view.selectedObject() : null;
        this.showObjectDetails(cfg, tab.schema, selected?.name ?? "", selected?.type as ObjectType ?? "database",
          tab.kind === "backup" ? { category: "backup" } : undefined);
      }
      return;
    }
    const cfg = this.connections.find((c) => c.id === tab.connectionId);
    this.infoPanel.setState({
      icon: cfg ? connIconOf(cfg) : "info_pane_database",
      title: tab.object || cfg?.name || "对象",
      subtitle: tab.objectType === "table" ? "表"
        : tab.objectType === "view" ? "视图"
        : tab.objectType === "query" ? "查询" : "",
      refs: this.connectionRef(tab.connectionId),
      fields: [
        ...(tab.schema ? [{ label: "数据库", value: tab.schema }] : []),
        ...(cfg?.host ? [{ label: "主机", value: cfg.host }] : []),
        ...(cfg?.port ? [{ label: "端口", value: String(cfg.port) }] : []),
      ],
    });
  }

  /** 连接引用行数据。 */
  private connectionRef(connId: string): { icon: string; text: string }[] {
    const cfg = this.connections.find((c) => c.id === connId);
    return cfg ? [{ icon: connIconOf(cfg), text: cfg.name }] : [];
  }

  /** 表数据页选中列：右侧信息面板切到「列信息」；取消选中则清空列信息。 */
  handleColumnSelected(tab: TabState, column: string): void {
    if (tab.kind === "table") {
      this.renderTableInfoPanel(tab, column ? "column" : undefined);
    }
  }

  /** 渲染表数据页右侧三视图（表信息 / DDL / 列信息）。 */
  private renderTableInfoPanel(
    tab: TabState,
    modeOverride?: "info" | "ddl" | "column",
  ): void {
    const key = `${tab.connectionId}/${tab.schema}/${tab.object}`;
    const request = ++this.infoRequest;
    const build = () => {
      const c = this.tableMetaCache.get(key);
      const cfg = this.connectionById(tab.connectionId);
      const info = c ? mapTableStatus(c.status).filter((field) => cfg?.kind !== "sqlite" || field.label !== "排序规则")
        : [{ label: "加载中", value: "..." }];
      if (cfg?.kind === "sqlite") {
        for (const field of info) if (field.label === "数据长度") field.label = "数据库页大小（非单表）";
      }
      if (c?.schema) {
        info.unshift(
          { label: "数据库", value: tab.schema || cfg?.database || "main" },
          { label: "列数量", value: String(c.schema.columns.length) },
          { label: "主键", value: c.schema.columns.filter((col) => col.primary_key).map((col) => col.name).join(", ") || "无" },
          { label: "索引数量", value: String(c.schema.indexes.length) },
          { label: "外键数量", value: String(c.schema.foreign_keys.length) },
        );
      }
      // 选中列的列信息（结构缓存里取类型 / 默认值 / 注释）
      const view = this.views.get(tab.id);
      let column: import("../ui/InfoPanel").TableColumnInfo | null = null;
      if (view instanceof DataTabView && view.grid.selectedColumn >= 0) {
        const name = view.grid.columns[view.grid.selectedColumn]?.name;
        const ci = c?.schema?.columns.find((x) => x.name === name);
        if (name) {
          column = {
            name,
            type: ci?.type ?? view.grid.columns[view.grid.selectedColumn]?.type ?? "",
            nullable: ci ? (ci.nullable ? "否" : "是") : "--",
            defaultValue: ci?.default ?? "(NULL)",
            comment: ci?.comment ?? "--",
          };
        }
      }
      this.infoPanel.setTableState({
        title: tab.object,
        refs: this.connectionRef(tab.connectionId),
        info,
        ddl: c?.ddl ?? "",
        column,
        mode: modeOverride ?? (column ? "column" : undefined),
      });
    };
    build();
    if (!this.tableMetaCache.has(key)) {
      void (async () => {
        const entry = {
          status: [] as [string, string][],
          ddl: "",
          schema: null as import("./types").TableSchema | null,
        };
        try {
          entry.status = await api.tableStatus(tab.connectionId, tab.schema, tab.object);
        } catch (e) {
          entry.status.push(["读取表信息失败", String(e)]);
        }
        try {
          entry.ddl = await api.getTableDdl(tab.connectionId, tab.schema, tab.object);
        } catch (e) {
          entry.ddl = `-- 获取定义失败：${String(e)}`;
        }
        try {
          entry.schema = await api.getTableSchema(tab.connectionId, tab.schema, tab.object);
        } catch {
          /* 结构获取失败留空 */
        }
        this.tableMetaCache.set(key, entry);
        if (this.activeTab?.id === tab.id && request === this.infoRequest) build();
      })();
    }
  }

  /** 库节点图标同步：已打开（db-open / 打开对象页）= 绿色亮图标，关闭 = 灰色。 */
  private syncDbIcons(_selectedId: string | null): void {
    this.syncConnIcons();
  }

  /** 打开一个表数据标签页。 */
  async openTableTab(
    connId: string,
    schema: string,
    table: string,
    type: ObjectType,
  ): Promise<void> {
    const cfg = this.connections.find((c) => c.id === connId);
    if (!cfg) return;
    const existing = this.tabs.find(
      (t) =>
        t.kind === "table" &&
        t.connectionId === connId &&
        t.schema === schema &&
        t.object === table,
    );
    if (existing) {
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    const tab: TabState = {
      id: `t_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`,
      kind: "table",
      title: table,
      connectionId: connId,
      connectionName: cfg.name,
      schema,
      object: table,
      objectType: type,
      sql: "",
      results: null,
      activeResult: 0,
      page: null,
      pageOffset: 0,
      pageLimit: this.settings.rows_per_page,
      orderBy: null,
      ascending: true,
      filters: [],
      schemaInfo: null,
      dirty: false,
      selectedRows: new Set(),
      editing: null,
    };
    this.tabs.push(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
    await this.pushRecent(connId, cfg.name, table, "table");
  }

  /** 打开一个存储过程 / 函数 / 触发器标签页（读取定义填入编辑器）。 */
  async openRoutineTab(
    connId: string,
    schema: string,
    name: string,
    type: ObjectType,
  ): Promise<void> {
    const cfg = this.connections.find((c) => c.id === connId);
    if (!cfg) return;
    const existing = this.tabs.find(
      (t) =>
        t.kind === "query" &&
        t.connectionId === connId &&
        t.object === name,
    );
    if (existing) {
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    const tab: TabState = {
      id: `r_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`,
      kind: "query",
      title: name,
      connectionId: connId,
      connectionName: cfg.name,
      schema,
      object: name,
      objectType: type,
      sql: "",
      results: null,
      activeResult: 0,
      page: null,
      pageOffset: 0,
      pageLimit: this.settings.rows_per_page,
      orderBy: null,
      ascending: true,
      filters: [],
      schemaInfo: null,
      dirty: false,
      selectedRows: new Set(),
      editing: null,
    };
    this.tabs.push(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
    // 读出源码填入编辑器（失败时保留空编辑器，不弹致命错误）
    if (type === "procedure" || type === "function") {
      try {
        const def = await api.getRoutineDefinition(
          connId,
          schema,
          name,
          type === "procedure" ? "procedure" : "function",
        );
        tab.sql = def;
        const view = this.views.get(tab.id);
        if (view instanceof QueryTabView) view.editor.setSql(def);
        this.renderTabs();
      } catch (e) {
        this.toast("error", `读取定义失败：${String(e)}`);
      }
    }
  }

  /** 打开一个查询标签页（schema 为选中的数据库，下拉框会定位到它）。 */
  async openQueryTab(connId?: string, sql = "", schema = ""): Promise<TabState> {
    let cfg: ConnectionConfig | undefined;
    if (connId) cfg = this.connections.find((c) => c.id === connId);
    if (cfg) await this.ensureConnection(cfg.id);
    const tab: TabState = {
      id: `q_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`,
      kind: "query",
      title: cfg ? `查询 - ${cfg.name}` : "查询 1",
      connectionId: cfg?.id ?? "",
      connectionName: cfg?.name ?? "",
      schema,
      object: "",
      objectType: "query",
      sql,
      results: null,
      activeResult: 0,
      page: null,
      pageOffset: 0,
      pageLimit: this.settings.rows_per_page,
      orderBy: null,
      ascending: true,
      filters: [],
      schemaInfo: null,
      dirty: false,
      selectedRows: new Set(),
      editing: null,
    };
    this.tabs.push(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
    return tab;
  }

  /** 打开一个表结构标签页。 */
  async openStructureTab(
    connId: string,
    schema: string,
    table: string,
  ): Promise<void> {
    const cfg = this.connections.find((c) => c.id === connId);
    if (!cfg) return;
    const existing = this.tabs.find(
      (t) =>
        t.kind === "structure" &&
        t.connectionId === connId &&
        t.schema === schema &&
        t.object === table,
    );
    if (existing) {
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    const tab: TabState = {
      id: `s_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`,
      kind: "structure",
      title: `${table} - 表结构`,
      connectionId: connId,
      connectionName: cfg.name,
      schema,
      object: table,
      objectType: "table",
      sql: "",
      results: null,
      activeResult: 0,
      page: null,
      pageOffset: 0,
      pageLimit: this.settings.rows_per_page,
      orderBy: null,
      ascending: true,
      filters: [],
      schemaInfo: null,
      dirty: false,
      selectedRows: new Set(),
      editing: null,
    };
    this.tabs.push(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
  }

  /** 打开备份 / 自动运行 / 模型 / 图表的空列表页（与参考程序空态一致）。 */
  openEmptyListTab(kind: "backup" | "schedule" | "model" | "chart"): void {
    const title = kind === "backup" ? "备份"
      : kind === "schedule" ? "自动运行"
      : kind === "model" ? "模型" : "图表";
    const existing = this.tabs.find((t) => t.kind === kind);
    if (existing) {
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    const tab: TabState = {
      id: `e_${kind}_${Date.now().toString(36)}`,
      kind,
      title,
      connectionId: this.activeTab?.connectionId ?? "",
      connectionName: "",
      schema: "",
      object: "",
      objectType: "folder",
      sql: "",
      results: null,
      activeResult: 0,
      page: null,
      pageOffset: 0,
      pageLimit: this.settings.rows_per_page,
      orderBy: null,
      ascending: true,
      filters: [],
      schemaInfo: null,
      dirty: false,
      selectedRows: new Set(),
      editing: null,
    };
    this.tabs.push(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
  }

  /** 供视图读取的设置。 */
  get settingsValue(): AppSettings {
    return this.settings;
  }

  /** 连接列表（查询页运行条用）。 */
  connectionsValue(): ConnectionConfig[] {
    return this.connections;
  }

  /** 按 id 找连接。 */
  datasourceConnectionChoices(): { id: string; name: string }[] {
    return this.connections.map(({ id, name }) => ({ id, name }));
  }

  connectionById(id: string): ConnectionConfig | undefined {
    return this.connections.find((c) => c.id === id);
  }

  /** 查看选项（视图读取）。 */
  showGridLinesState(): boolean {
    return this.showGridLines;
  }

  skipNullState(): boolean {
    return this.skipNull;
  }

  /** 弹出右键菜单。 */
  showContextMenu(entries: MenuEntry[], x: number, y: number): void {
    this.menu.showContext(entries, x, y);
  }

  /** 当前激活标签页。 */
  get activeTab(): TabState | null {
    return this.tabs.find((t) => t.id === this.activeTabId) ?? null;
  }

  /** 刷新当前标签页数据。 */
  async refreshActive(): Promise<void> {
    const tab = this.activeTab;
    if (!tab) return;
    if (tab.kind === "table" || tab.kind === "structure") this.tableMetaCache.delete(`${tab.connectionId}/${tab.schema}/${tab.object}`);
    const view = this.views.get(tab.id);
    await view?.reload();
    this.updateInfoPanel();
    this.renderStatus();
  }

  // ------------------------------------------------------------ 状态栏

  /** 标题栏右侧文档标题（对齐设计稿：对象页「库 — 连接」，备份/用户等「连接 — 类型」，查询页显示页签标题）。 */
  private renderTitleDoc(): void {
    const tab = this.activeTab;
    const cfg = tab ? this.connections.find((c) => c.id === tab.connectionId) : undefined;
    let doc = "未连接";
    if (tab) {
      if (tab.kind === "objects" && tab.schema) {
        doc = cfg ? `${tab.schema} — ${cfg.name}` : tab.schema;
      } else if (
        tab.kind === "backup" ||
        tab.kind === "schedule" ||
        tab.kind === "model" ||
        tab.kind === "chart" ||
        tab.kind === "users"
      ) {
        doc = cfg ? `${tab.schema || cfg.name} — ${tab.title.replace(/ — .*/, "")}` : tab.title;
      } else if (tab.kind === "query" || tab.kind === "result") {
        doc = tab.title;
      } else if (cfg) {
        doc = `${tab.title.replace(/ @.*$/, "") || tab.title} — ${cfg.name}`;
      } else {
        doc = tab.title;
      }
    }
    this.titleDoc.textContent = doc;
  }

  renderStatus(): void {
    this.renderTitleDoc();
    this.statusBar.innerHTML = "";
    const tab = this.activeTab;
    const view = tab ? this.views.get(tab.id) : undefined;
    const cfg = tab ? this.connections.find((c) => c.id === tab.connectionId) : undefined;
    let hasSpacer = false;

    const mkCell = (text?: string, icon?: string) => {
      const c = document.createElement("div");
      c.className = "statusbar-cell";
      if (icon) {
        const ic = document.createElement("span");
        ic.innerHTML = iconSvgByName(icon);
        ic.style.lineHeight = "0";
        ic.style.display = "inline-flex";
        c.appendChild(ic);
      }
      if (text) c.appendChild(document.createTextNode(text));
      this.statusBar.appendChild(c);
      return c;
    };
    const mkSpacer = () => {
      if (hasSpacer) return;
      const s = document.createElement("div");
      s.className = "statusbar-cell grow";
      this.statusBar.appendChild(s);
      hasSpacer = true;
    };

    if (tab) {
      if (tab.kind === "objects" && view instanceof ObjectsTabView) {
        // 参考图 1：`N 表` + 连接图标 + 连接名 + 库图标 + 库名
        mkCell(`${view.filteredCount()} 表`);
        mkCell(cfg?.name ?? "", connIconOf(cfg));
        mkCell(tab.schema, "info_pane_database");
      } else if (view instanceof DataTabView) {
        // 参考图 6：左 = SELECT 语句（选中列时显示"已选择 1 列"）；
        // 右 = 第 N 条记录（共 M 条）于第 K 页
        const info = view.statusInfo();
        if (view.grid.selectedColumn >= 0) {
          mkCell("已选择 1 列");
        } else {
          mkCell(info.sql);
        }
        mkSpacer();
        mkCell(info.pageText);
      } else if (tab.kind === "backup" && view instanceof BackupTabView) {
        mkCell(`${view.grid.rows.length} 备份`);
        mkCell(cfg?.name ?? "", connIconOf(cfg));
        mkCell(tab.schema, "info_pane_database");
      } else if (tab.kind === "users" && view instanceof UsersTabView) {
        mkCell(`${view.grid.rows.length} 用户`);
        mkCell(cfg?.name ?? "", connIconOf(cfg));
        mkCell(tab.schema, "info_pane_database");
      } else if (tab.kind === "structure") {
        mkCell(`字段数: ${tab.schemaInfo?.columns.length ?? 0}`);
      } else if (view instanceof QueryTabView) {
        // 参考截图 5：状态栏左侧显示 SQL（等宽字体），右侧显示运行时间与记录数
        const info = view.statusInfo();
        mkCell(info.sql);
        mkSpacer();
        if (info.elapsed) mkCell(info.elapsed);
        mkCell(info.pageText);
      } else {
        mkCell(tab.title, cfg ? connIconOf(cfg) : undefined);
      }
    } else {
      mkCell("就绪");
    }

    mkSpacer();

    // 右：视图切换 3 个 + 面板开关 2 个
    const toggles = document.createElement("div");
    toggles.className = "statusbar-toggle";
    const mkToggle = (
      iconSvgStr: string,
      title: string,
      active: boolean,
      onClick: () => void,
    ) => {
      const b = document.createElement("div");
      b.className = `statusbar-toggle-btn${active ? " active" : ""}`;
      b.title = title;
      b.innerHTML = iconSvgStr;
      b.addEventListener("click", onClick);
      toggles.appendChild(b);
    };
    // 数据显示模式（明细 / 列表 / 网格）互斥高亮
    const cycle = () => {
      this.viewMode = this.viewMode === "grid" ? "detail" : this.viewMode === "detail" ? "list" : "grid";
      this.renderStatus();
    };
    mkToggle(svg(ICON_VIEW_DOTS), "明细视图", this.viewMode === "detail", cycle);
    mkToggle(svg(ICON_VIEW_LIST), "列表视图", this.viewMode === "list", cycle);
    mkToggle(svg(ICON_VIEW_GRID), "网格视图", this.viewMode === "grid", cycle);
    mkToggle(svg(ICON_PANEL_LEFT), "显示/隐藏左侧栏", this.sidebarVisible, () => {
      this.sidebarVisible = !this.sidebarVisible;
      this.sidebar.classList.toggle("hidden", !this.sidebarVisible);
      this.renderStatus();
      this.saveViewPrefs();
    });
    mkToggle(svg(ICON_PANEL_RIGHT), "显示/隐藏信息面板", this.infoPanelVisible, () => {
      this.infoPanelVisible = !this.infoPanelVisible;
      this.infoPanel.setVisible(this.infoPanelVisible);
      this.renderStatus();
      this.saveViewPrefs();
    });
    this.statusBar.appendChild(toggles);
    if (this.datasourceSyncStatus) {
      const status = document.createElement("span"); status.className = "datasource-sync-status";
      status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite"); status.textContent = this.datasourceSyncStatus;
      this.statusBar.append(status);
    }
    this.statusBar.style.display = this.statusBarVisible ? "" : "none";
  }

  // ------------------------------------------------------------ 提示

  toast(kind: Toast["kind"], text: string): void {
    const el = document.createElement("div");
    el.className = `toast ${kind}`;
    const icon = document.createElement("span");
    icon.className = "toast-icon";
    icon.style.lineHeight = "0";
    icon.innerHTML = svg(kind === "error" ? ICON_MSG_ERROR : ICON_MSG_INFO);
    const txt = document.createElement("div");
    txt.className = "toast-text";
    txt.textContent = text;
    el.appendChild(icon);
    el.appendChild(txt);
    this.toastHost.appendChild(el);
    setTimeout(() => el.remove(), 4200);
  }

  // ------------------------------------------------------------ 最近 / 收藏

  private async pushRecent(
    connId: string,
    connName: string,
    label: string,
    kind: string,
  ): Promise<void> {
    const item: RecentItem = {
      id: `${connId}:${label}`,
      label: connName ? `${connName} / ${label}` : label,
      kind,
      timestamp: Date.now(),
    };
    this.recent = await api.pushRecent(item);
  }

  // ------------------------------------------------------------ 命令

  private commandUnavailableReason(id: string): string | null {
    const tab = this.activeTab;
    const cfg = this.tree ? this.connectionById(this.currentConnectionId()) : undefined;
    const view = tab ? this.views.get(tab.id) : undefined;
    const query = view instanceof QueryTabView;
    if (tab?.datasourceStale) {
      const localCommands = new Set(["options", "about", "official-doc", "shortcut-help", "driver-manager", "new-connection", "exit", "next-window", "close-window", "close-others", "close-all", "tab-close", "tab-close-others", "tab-close-left", "tab-close-right", "tab-close-all", "toggle-sidebar", "toggle-statusbar", "toggle-toolbar", "toggle-info-panel", "focus-mode", "manage-favorites"]);
      const editorCommands = new Set(["save", "save-as", "undo", "redo", "cut", "copy", "paste", "find", "find-next", "replace", "select-all", "beautify-sql", "code-snippet", "add-favorite"]);
      if (!localCommands.has(id) && !(query && editorCommands.has(id))) return tab.datasourceStale;
    }
    const data = view instanceof DataTabView;
    const selectedCommands = ["data-sync", "data-generation", "import", "tree-import", "transfer", "data-transfer", "new-table", "tree-new-table", "delete-table", "truncate-table", "restore-backup", "connection-properties", "remove-connection", "open-connection"];
    const writeConnection = selectedCommands.includes(id) ? cfg?.id || tab?.connectionId || "" : tab?.connectionId || cfg?.id || "";
    if (MANUAL_TRANSACTION_WRITE_COMMANDS.has(id) || (id === "save" && !query)) {
      if (this.manualTransactionFor(writeConnection)) return "此连接有手动事务，请先在所属查询页提交或回滚，再使用自动提交写入工具";
      if (this.manualTransactionIsBusy(writeConnection)) return "此连接的事务操作正在进行";
    }
    const transaction = this.manualTransactionFor(tab?.connectionId || "");
    if (query && ["run", "run-selected", "explain"].includes(id)) {
      if (this.manualTransactionIsBusy(tab?.connectionId || "")) return "事务操作正在进行，请等待完成";
      if (transaction && (transaction.owner !== tab?.id || transaction.schema !== tab?.schema)) return "此连接事务由另一个查询页或模式持有，请返回所属查询页执行";
      if (transaction?.failed) return "事务执行失败，必须回滚后才能继续执行";
      if (transaction && id === "explain") return "手动事务仅支持 SELECT/WITH 和 DML；解释计划请先提交或回滚";
    }
    switch (id) {
      case "begin-transaction": {
        if (!(query || data) || !tab) return "请在查询或表数据页面开始事务";
        const reason = manualTransactionUnavailableReason(this.connectionById(tab.connectionId));
        if (reason) return reason;
        if (this.manualTransactionIsBusy(tab.connectionId)) return "事务操作正在进行";
        if (transaction) return "此连接已有手动事务，请先在所属查询页提交或回滚";
        if (view instanceof QueryTabView && view.isExecuting()) return "SQL正在执行，请等待完成";
        if (view instanceof DataTabView && view.hasPendingChanges()) return "请先保存或取消表格中待提交的改动";
        return null;
      }
      case "commit-transaction":
      case "rollback-transaction": {
        if (!query || !tab) return "请返回事务所属查询页面";
        if (this.manualTransactionIsBusy(tab.connectionId) || (view instanceof QueryTabView && view.isExecuting())) return "SQL或事务操作正在进行";
        if (!transaction) return "此连接没有手动事务";
        if (transaction.owner !== tab.id || transaction.schema !== tab.schema) return "此事务由另一个查询页或模式持有";
        return id === "commit-transaction" && transaction.failed ? "事务执行失败，必须回滚，不能提交" : null;
      }
      case "chart":
        return chartSourceError(this.currentChartSource());
      // data-sync / data-generation 是全局工具：不因当前选中的库不支持而禁用
      // 按钮（runCommand 里回落到受支持连接；对话框内的数据源下拉会过滤掉
      // 不支持的连接），否则用户换一个要同步的库都进不去工具。
      case "server-monitor":
      case "open-user":
      case "open-connection":
      case "close-connection":
      case "connection-properties":
      case "remove-connection":
        return cfg ? null : "请先选择一个连接";
      case "save":
        if (query) return null;
        if (!(data || view instanceof StructureTabView || tab?.kind === "viewdesign" || tab?.kind === "routinedesign")) return "当前页面没有可保存的内容";
        return this.connectionById(tab?.connectionId ?? "")?.read_only ? "此连接只读，不能保存数据库改动" : null;
      case "save-as":
        return query || data ? null : "另存为适用于查询或表数据页";
      case "query-builder": {
        if (!query) return "此命令仅适用于查询编辑器";
        const target = this.connectionById(tab?.connectionId ?? "");
        return target ? queryBuilderUnavailableReason(target.kind) : "请先在查询编辑器选择连接";
      }
      case "undo":
      case "redo":
      case "cut":
      case "paste":
      case "find":
      case "find-next":
      case "run":
      case "run-selected":
      case "explain":
      case "beautify-sql":
      case "code-snippet":
      case "add-favorite":
        return query ? null : "此命令仅适用于查询编辑器";
      case "delete-row":
      case "grid-delete":
      case "grid-insert":
      case "grid-commit":
        if (!data || tab?.objectType !== "table") return "此命令需要可编辑的数据表页面（不能写入视图或查询结果）";
        return this.connectionById(tab.connectionId)?.read_only ? "此连接只读，不能修改记录" : null;
      case "grid-rollback":
        return data ? null : "当前页面没有数据表待提交改动";
      case "copy":
      case "select-all":
        return query || data || view instanceof ObjectsTabView ? null : "当前页面没有可复制或选择的内容";
      case "structure":
        return this.currentTableRef() ? null : "请先选择一个数据表";
      case "filter":
      case "sort":
        return data || query ? null : "此命令需要表数据或查询结果页面";
      default:
        return null;
    }
  }

  private ribbonDisabled(id: string): boolean {
    return this.commandUnavailableReason(id) !== null;
  }

  /** Ribbon / 二级工具栏下拉按钮（连接▼ / 其它▼ / 文本▼ / 用户▼）。 */
  private ribbonDropdown(id: string, x: number, y: number): void {
    if (id === "new-connection") {
      // 点击「连接」直接进入新建连接（类型选择 → 连接属性），不再中转下拉菜单
      void this.runCommand("new-connection");
    } else if (id === "open-other") {
      this.menu.showContext(OTHER_OBJECTS_MENU, x, y);
    } else if (id === "text") {
      // 「文本」下拉：勾选当前显示格式（文本 / 十六进制 / 图像 / 网页）
      const cur = this.textFormat;
      const items = TEXT_FORMAT_MENU.map((it) => ({
        ...it,
        checked: it.id === `text-format-${cur === "text" ? "plain" : cur}`,
      }));
      this.menu.showContext(items, x, y);
    } else if (id === "open-user") {
      // SQL Server 连接的「用户▼」：下拉里给出用户管理入口
      this.menu.showContext(
        [
          { id: "open-user", label: "打开用户页..." },
          { id: "new-user", label: "新建用户..." },
        ],
        x,
        y,
      );
    } else {
      void this.runCommand(id);
    }
  }

  /** 命令统一入口（含一次性菜单分发）。 */
  private dispatchCommand(id: string): void {
    if (this.sortMenuOnce) {
      const h = this.sortMenuOnce;
      this.sortMenuOnce = null;
      h(id);
      return;
    }
    if (this.colMenuOnce) {
      const h = this.colMenuOnce;
      this.colMenuOnce = null;
      h(id);
      return;
    }
    if (id.startsWith("fav-open:")) {
      const fav = this.favorites.find((f) => f.id === id.slice(9));
      if (fav) void this.openQueryTab(fav.connection_id ?? undefined, fav.sql);
      return;
    }
    if (id.startsWith("recent-open:")) {
      const rid = id.slice(12);
      const item = this.recent.find((r) => r.id === rid);
      if (item) {
        const [connId, ...rest] = item.id.split(":");
        void this.openTableTab(connId, "", rest.join(":"), "table");
      }
      return;
    }
    if (id === "recent-clear") {
      void api.clearRecent().then((list) => {
        this.recent = list;
        this.toast("success", "最近打开已清空");
      });
      return;
    }
    void this.runCommand(id);
  }

  /** 执行菜单 / 工具栏命令。 */
  async runCommand(id: string): Promise<void> {
    const unavailable = this.commandUnavailableReason(id);
    if (unavailable) { this.toast("error", unavailable); return; }
    try {
      switch (id) {
        case "new-connection":
          this.cmdNewConnection();
          break;
        case "new-connection-mysql":
          this.cmdNewConnection("mysql");
          break;
        case "new-connection-postgresql":
          this.cmdNewConnection("postgresql");
          break;
        case "new-connection-sqlite":
          this.cmdNewConnection("sqlite");
          break;
        case "new-connection-tidb":
          this.cmdNewConnection("tidb");
          break;
        case "new-connection-oceanbase":
          this.cmdNewConnection("oceanbase");
          break;
        case "new-connection-polardb":
          this.cmdNewConnection("polardb");
          break;
        case "new-connection-tdsql":
          this.cmdNewConnection("tdsql");
          break;
        case "new-connection-doris":
          this.cmdNewConnection("doris");
          break;
        case "new-connection-clickhouse":
          this.cmdNewConnection("clickhouse");
          break;
        case "new-connection-dameng":
          this.cmdNewConnection("dameng");
          break;
        case "new-connection-hive":
          this.cmdNewConnection("hive");
          break;
        case "new-connection-opengauss":
          this.cmdNewConnection("opengauss");
          break;
        case "new-connection-kingbase":
          this.cmdNewConnection("kingbase");
          break;
        case "new-connection-highgo":
          this.cmdNewConnection("highgo");
          break;
        case "new-connection-sqlserver":
          this.cmdNewConnection("sqlserver");
          break;
        case "new-connection-redis":
          this.cmdNewConnection("redis");
          break;
        case "new-connection-mongodb":
          this.cmdNewConnection("mongodb");
          break;
        case "new-query":
        case "open-query":
          // 与树中选中 / 鼠标点选的数据库联动
          await this.focusDbGroup("query");
          break;
        case "open-table":
          await this.focusDbGroup("table");
          break;
        case "open-view":
          await this.focusDbGroup("view");
          break;
        case "open-function":
          await this.focusDbGroup("function");
          break;
        case "backup": {
          // Ribbon「备份」：对当前选中的库打开备份标签页；未选库时打开空列表页
          const bt = this.currentDbTarget();
          if (bt?.schema) {
            this.openSpecialTab("backup", bt.connId, bt.schema);
            this.selectDbTreeNode({ connId: bt.connId, schema: bt.schema }, "backup");
          } else {
            this.openEmptyListTab("backup");
          }
          break;
        }
        case "open-user": {
          // 用户页：对当前选中的连接打开（MySQL 读 mysql.user 真实数据）；
          // 用户是全局对象，未选中库也能打开
          const connId = this.currentConnectionId();
          const cfg = connId ? this.connectionById(connId) : undefined;
          const schema =
            (cfg?.kind === "sqlite" ? "main" : this.currentDbName()) || "";
          if (connId) {
            this.openSpecialTab("users", connId, schema);
          } else {
            this.toast("error", "请先在左侧树中选择一个连接");
          }
          break;
        }
        case "schedule":
          this.openScheduleTab();
          break;
        case "model": {
          const mt = this.backupTarget();
          if (mt) {
            this.openModelTab(mt.connId, mt.schema);
          } else {
            this.toast("error", "请先在左侧树中选择一个数据库");
          }
          break;
        }
        case "chart":
          this.cmdCreateChart();
          break;
        case "data-sync": {
          const cfg = this.connectionById(this.currentConnectionId());
          if (!cfg) { this.toast("error", "请先选择源连接"); break; }
          const schema = this.currentDbTarget()?.schema ?? "", ref = this.currentTableRef();
          // 全局工具始终可点：当前连接不支持同步时原样传入，对话框不预选任何
          // 数据源（下拉里只列受支持连接且保持空选），由用户自行选择。
          openSyncDialog(this.connections, cfg, {
            schema, table: ref?.connId === cfg.id && ref.schema === schema ? ref.table : "",
          }, () => { void this.refreshActive(); });
          break;
        }
        case "driver-manager":
          openDriverManager();
          break;
        case "server-monitor": {
          const cfg = this.connectionById(this.currentConnectionId());
          if (cfg) openServerMonitorDialog(cfg);
          else this.toast("error", "请先选择一个连接");
          break;
        }
        case "data-generation": {
          const cfg = this.connectionById(this.currentConnectionId());
          if (!cfg) { this.toast("error", "请先选择一个连接"); break; }
          const schema = this.currentDbTarget()?.schema ?? "", ref = this.currentTableRef();
          // 全局工具始终可点：连接在对话框内选择，当前连接不支持时下拉保持
          // 空选（不默认选中第一个），由用户自行挑一个可写的连接。
          openDataGeneratorDialog(cfg, schema, ref?.connId === cfg.id && ref.schema === schema ? ref.table : "",
            () => { void this.refreshActive(); }, undefined, this.connections);
          break;
        }
        case "group-new-connection": {
          const node = this.tree.find(this.tree.selectedNodeId ?? "");
          this.newConnectionGroup = node?.id.startsWith("group:") ? node.id.slice(6) : "";
          this.cmdNewConnection();
          break;
        }
        case "group-rename":
          this.cmdRenameGroup();
          break;
        case "open-sql-file": {
          const { open } = await import("@tauri-apps/plugin-dialog");
          const path = await open({ multiple: false, filters: [{ name: "SQL", extensions: ["sql"] }] });
          if (typeof path === "string") {
            const sql = await api.readSqlFile(path);
            const target = this.currentDbTarget();
            await this.openQueryTab(target?.connId, sql, target?.schema ?? "");
          }
          break;
        }
        case "history-log":
          this.cmdShowLog();
          break;
        case "run":
          await this.cmdRun(false);
          break;
        case "run-selected":
          await this.cmdRun(true);
          break;
        case "explain":
          await this.cmdExplain();
          break;
        case "structure":
          this.cmdStructure();
          break;
        case "filter":
        case "grid-filter":
          this.cmdFilter();
          break;
        case "sort":
        case "grid-sort":
          this.cmdSort();
          break;
        case "columns":
          this.cmdColumns();
          break;
        case "open-connection":
          await this.cmdOpenConnection();
          break;
        case "close-connection":
          await this.cmdCloseConnection();
          break;
        case "connection-properties":
          this.cmdEditConnection();
          break;
        case "remove-connection":
          this.cmdRemoveConnection();
          break;
        case "import":
          this.cmdImport(false);
          break;
        case "tree-import":
          this.cmdImport(true);
          break;
        case "export":
        case "grid-export":
          this.cmdExport(false);
          break;
        case "tree-export":
          this.cmdExport(true);
          break;
        case "dump-sql":
        case "sql-dump":
        case "tree-dump":
          this.cmdDump();
          break;
        case "transfer":
        case "data-transfer":
          this.cmdTransfer();
          break;
        case "save":
          if (this.activeTab?.kind === "viewdesign") {
            await this.cmdSaveViewDesign();
          } else if (this.activeTab?.kind === "routinedesign") {
            await this.cmdSaveRoutineDesign();
          } else {
            await this.cmdSave();
          }
          break;
        case "save-as":
          await this.cmdSaveAs();
          break;
        case "print":
          window.print();
          break;
        case "exit":
          this.confirmExit();
          break;
        case "undo":
          this.cmdUndo();
          break;
        case "redo":
          this.cmdRedo();
          break;
        case "cut":
          await this.cmdCut();
          break;
        case "copy":
        case "grid-copy":
        case "grid-copy-row":
        case "grid-copy-cell":
          await this.cmdCopy();
          break;
        case "paste":
          await this.cmdPaste();
          break;
        case "delete-row":
        case "grid-delete":
          await this.cmdDeleteRow();
          break;
        case "find":
          this.cmdFind();
          break;
        case "find-next":
          this.cmdFindNext();
          break;
        case "replace":
          this.cmdFind();
          break;
        case "select-all":
          this.cmdSelectAll();
          break;
        case "toggle-sidebar":
          this.sidebarVisible = !this.sidebarVisible;
          this.sidebar.style.display = this.sidebarVisible ? "" : "none";
          this.splitter.style.display = this.sidebarVisible ? "" : "none";
          this.saveViewPrefs();
          break;
        case "toggle-statusbar":
          this.statusBarVisible = !this.statusBarVisible;
          this.renderStatus();
          this.saveViewPrefs();
          break;
        case "toggle-toolbar":
          this.toolbarVisible = !this.toolbarVisible;
          this.ribbon.el.style.display = this.toolbarVisible ? "" : "none";
          this.saveViewPrefs();
          break;
        case "toggle-info-panel":
          this.infoPanelVisible = !this.infoPanelVisible;
          this.infoPanel.setVisible(this.infoPanelVisible);
          this.infoSplitter.style.display = this.infoPanelVisible ? "" : "none";
          this.saveViewPrefs();
          break;
        case "show-grid":
          this.showGridLines = !this.showGridLines;
          this.applyViewOptions();
          this.saveViewPrefs();
          break;
        case "show-null":
          this.skipNull = !this.skipNull;
          this.applyViewOptions();
          this.saveViewPrefs();
          break;
        case "refresh":
        case "grid-refresh":
        case "tab-refresh":
          await this.refreshActive();
          break;
        case "expand-all":
          this.tree.expandAll();
          break;
        case "collapse-all":
          this.tree.collapseAll();
          break;
        case "focus-mode":
          this.toggleFocusMode();
          break;
        case "add-favorite":
          await this.cmdAddFavorite();
          break;
        case "open-favorite":
          this.cmdOpenFavorite();
          break;
        case "manage-favorites":
          this.cmdManageFavorites();
          break;
        case "recent":
          this.cmdRecent();
          break;
        case "server-info":
        case "tree-server-info":
          await this.cmdServerInfo();
          break;
        case "begin-transaction":
          await this.manualTransactionController.begin();
          break;
        case "commit-transaction":
          await this.manualTransactionController.finish(true);
          break;
        case "rollback-transaction":
          await this.manualTransactionController.finish(false);
          break;
        case "query-builder":
          this.cmdQueryBuilder();
          break;
        case "beautify-sql":
          await this.cmdBeautifySql();
          break;
        case "code-snippet":
          this.cmdCodeSnippet();
          break;
        case "options":
          this.cmdOptions();
          break;
        case "next-window":
          this.cmdCycleTab(1);
          break;
        case "close-window":
          if (this.activeTabId) this.closeTab(this.activeTabId);
          break;
        case "close-others":
        case "tab-close-others":
          this.cmdCloseOthers();
          break;
        case "tab-close-left":
          this.cmdCloseSide("left");
          break;
        case "tab-close-right":
          this.cmdCloseSide("right");
          break;
        case "close-all":
        case "tab-close-all":
          this.cmdCloseAll();
          break;
        case "official-doc":
          this.cmdOfficialDoc();
          break;
        case "shortcut-help":
          this.cmdShortcutHelp();
          break;
        case "about":
          this.cmdAbout();
          break;
        // Ribbon「其它▼」：表空间 / 事件（按协议生成系统查询并在查询页执行）
        case "open-other-tablespace":
          await this.cmdOpenOtherObject("tablespace");
          break;
        case "open-other-event":
          await this.cmdOpenOtherObject("event");
          break;
        // 树右键命令
        case "tree-open":
          this.cmdTreeOpen();
          break;
        case "tree-edit":
          this.cmdEditConnection();
          break;
        case "tree-duplicate":
          this.cmdDuplicateConnection();
          break;
        case "tree-remove":
          this.cmdRemoveConnection();
          break;
        case "tree-close":
          await this.cmdCloseConnection();
          break;
        case "tree-refresh":
          this.cmdTreeRefresh();
          break;
        case "tree-new-group":
          this.cmdNewGroup();
          break;
        // 数据库节点命令（打开 / 关闭后图标亮灭与展开箭头同步）
        case "db-open": {
          const db = this.currentDbName();
          const connId = this.currentConnectionId();
          if (!connId || !db) {
            this.toast("error", "请先在左侧树中选择一个数据库");
            break;
          }
          const dbNode = this.tree.find(`conn:${connId}/db/${db}`);
          if (dbNode) {
            await this.openDatabaseNode(dbNode, connId, db);
          } else {
            this.openDbs.add(`${connId}/${db}`);
            this.openConnections.add(connId);
            this.syncConnIcons();
            this.openObjectsTab(connId, db);
          }
          this.toast("success", `数据库「${db}」已打开`);
          break;
        }
        case "db-close": {
          const db = this.currentDbName();
          const connId = this.currentConnectionId();
          if (!connId || !db) {
            this.toast("error", "请先在左侧树中选择一个数据库");
            break;
          }
          this.closeDatabaseNode(connId, db);
          this.toast("info", `数据库「${db}」已关闭`);
          break;
        }
        case "db-new":
          this.cmdNewDatabase();
          break;
        case "db-drop":
          this.cmdDropDatabase();
          break;
        case "tree-edit-db":
          this.cmdEditConnection();
          break;
        case "tree-model": {
          const mt2 = this.backupTarget();
          if (mt2) {
            this.openModelTab(mt2.connId, mt2.schema);
          } else {
            this.toast("error", "请先在左侧树中选择一个数据库");
          }
          break;
        }
        case "tree-new-query":
          await this.cmdNewQuery();
          break;
        case "tree-open-table":
        case "open-selected-table":
          this.cmdTreeOpen();
          break;
        case "tree-structure":
        case "design-table":
          this.cmdTreeStructure();
          break;
        case "new-table":
          this.cmdNewTable();
          break;
        case "delete-table":
          this.cmdDropTable();
          break;
        case "truncate-table":
          this.cmdTruncateTable();
          break;
        case "grid-insert":
          await this.cmdInsertRow();
          break;
        case "grid-commit":
          await this.cmdCommit();
          break;
        case "grid-rollback":
          this.cmdRollback();
          break;
        case "tab-close":
          if (this.activeTabId) this.closeTab(this.activeTabId);
          break;
        case "tab-duplicate":
          await this.cmdDuplicateTab();
          break;
        case "add-field":
          this.cmdAddField();
          break;
        case "insert-field":
          this.cmdInsertField();
          break;
        case "delete-field":
          this.cmdDeleteField();
          break;
        case "primary-key":
          this.cmdTogglePrimaryKey();
          break;
        case "move-up":
        case "move-down": {
          const view = this.activeView();
          if (view instanceof StructureTabView) view.moveField(id === "move-up" ? -1 : 1);
          else this.toast("error", "请先打开表结构并选择字段");
          break;
        }
        case "text-format-plain":
        case "text-format-hex":
        case "text-format-image":
        case "text-format-web":
          this.textFormat = ({
            "text-format-plain": "text",
            "text-format-hex": "hex",
            "text-format-image": "image",
            "text-format-web": "web",
          } as const)[id];
          this.refreshCellPreviews();
          break;
        // 备份命令
        case "new-backup":
        case "tree-new-backup":
          this.cmdNewBackup();
          break;
        case "restore-backup":
          await this.cmdRestoreBackup();
          break;
        case "delete-backup":
          await this.cmdDeleteBackup();
          break;
        // 视图 / 函数设计器：保存 = 重建对象
        case "tree-design-view":
          await this.cmdTreeDesignView();
          break;
        case "tree-design-routine":
          await this.cmdTreeDesignRoutine();
          break;
        case "model-fit":
        case "model-zoom-in":
        case "model-zoom-out": {
          const tab = this.activeTab;
          const view = tab ? this.views.get(tab.id) : undefined;
          if (view instanceof ModelTabView) {
            if (id === "model-fit") view.fit();
            else if (id === "model-zoom-in") view.zoomIn();
            else view.zoomOut();
          }
          break;
        }
        // 自动运行
        case "new-job":
          this.cmdNewJob();
          break;
        case "edit-job":
          this.cmdEditJob();
          break;
        case "run-job":
          await this.cmdRunJob();
          break;
        case "delete-job":
          await this.cmdDeleteJob();
          break;
        case "job-history":
          this.cmdJobHistory();
          break;
        // 用户管理
        case "edit-user":
          this.cmdEditUserPassword();
          break;
        case "new-user":
          this.cmdNewUserReal();
          break;
        case "delete-user":
          await this.cmdDropUser();
          break;
        case "open-backup-folder":
        case "tree-open-backup-folder":
          this.cmdOpenBackupFolder();
          break;
        default:
          this.toast("error", `命令「${id}」尚未实现或当前版本不支持`);
          break;
      }
    } catch (e) {
      this.toast("error", String(e));
    }
    // 工具栏的启用态可能随命令变化
    const tab = this.activeTab;
    if (tab) this.tabToolbar.setItems(this.guardedTabToolbarItems(tab));
    this.renderStatus();
    // 「查看」等菜单的勾选态随命令实时刷新
    this.menu.renderBar(this.menuHost, this.contextMenus(), this.viewMenu());
  }

  // ------------------------------------------------------ 各项命令实现

  /**
   * 新建连接：不传类型时先弹出「数据源类型选择器」（分类 + 卡片 + 搜索），
   * 用户选定类型后再进入连接属性对话框；快捷菜单仍可直接指定类型。
   */
  private cmdNewConnection(kind?: string, driver?: DriverDefinition): void {
    if (!kind) {
      openConnectionPicker((picked, custom) => this.cmdNewConnection(picked, custom), "mysql");
      return;
    }
    const group = this.newConnectionGroup;
    this.newConnectionGroup = "";
    openConnectionDialog(null, this.groups, ({ config, password }) => {
      config.group = group;
      void (async () => {
        try {
          this.connections = await api.saveConnection(config, password);
          this.groups = await api.listGroups();
          this.refreshTree();
          this.toast("success", `连接「${config.name}」已创建`);
        } catch (e) {
          this.toast("error", String(e));
        }
      })();
    }, kind, driver);
  }

  private cmdEditConnection(): void {
    const cfg = this.connectionById(this.currentConnectionId());
    if (!cfg) {
      this.toast("error", "请先在左侧树中选择一个连接");
      return;
    }
    openConnectionDialog(cfg, this.groups, ({ config, password }) => {
      void (async () => {
        try {
          this.connections = await api.saveConnection(config, password);
          this.groups = await api.listGroups();
          this.refreshTree();
          this.toast("success", "连接属性已保存");
        } catch (e) {
          this.toast("error", String(e));
        }
      })();
    });
  }

  /** 复制连接：在原配置基础上打开「新建连接」对话框。 */
  private cmdDuplicateConnection(): void {
    const cfg = this.connectionById(this.currentConnectionId());
    if (!cfg) {
      this.toast("error", "请先在左侧树中选择一个连接");
      return;
    }
    const copy: ConnectionConfig = { ...cfg, id: "", name: `${cfg.name} - 副本` };
    openConnectionDialog(copy, this.groups, ({ config, password }) => {
      void (async () => {
        try {
          this.connections = await api.saveConnection(config, password);
          this.groups = await api.listGroups();
          this.refreshTree();
          this.toast("success", `连接「${config.name}」已创建`);
        } catch (e) {
          this.toast("error", String(e));
        }
      })();
    });
  }

  private cmdRemoveConnection(): void {
    const cfg = this.connectionById(this.currentConnectionId());
    if (!cfg) {
      this.toast("error", "请先在左侧树中选择一个连接");
      return;
    }
    new Dialog({
      title: "移除连接",
      width: 380,
      render: (body) => {
        body.textContent = `确定要移除连接「${cfg.name}」吗？`;
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "确定",
          primary: true,
          onClick: () => {
            void (async () => {
              this.connections = await api.deleteConnection(cfg.id);
              this.openConnections.delete(cfg.id);
              this.refreshTree();
              this.toast("success", `连接「${cfg.name}」已移除`);
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  private async cmdOpenConnection(): Promise<void> {
    const connId = this.currentConnectionId();
    if (!connId) {
      this.toast("error", "请先在左侧选择一个连接");
      return;
    }
    try {
      const r = await api.openConnection(connId);
      if (r.ok) {
        this.openConnections.add(connId);
        this.syncConnIcons();
        this.toast("success", "连接已打开");
      } else {
        this.toast("error", r.message);
      }
    } catch (e) {
      this.toast("error", String(e));
    }
  }

  private async cmdCloseConnection(): Promise<void> {
    const connId = this.currentConnectionId();
    if (!connId) return;
    if (!await this.manualTransactionController.closeConnection(connId)) return;
    this.openConnections.delete(connId);
    for (const key of [...this.openDbs]) {
      if (key.startsWith(`${connId}/`)) this.openDbs.delete(key);
    }
    // 收起并清空该连接已加载的库列表：后端连接已真实关闭，
    // 树中残留的库节点既不真实也不可交互（双击会静默重连）。
    const node = this.tree.find(`conn:${connId}`);
    if (node) {
      node.expanded = false;
      node.loaded = false;
      node.children = [];
      node.has_children = true;
    }
    this.syncConnIcons();
    this.tree.render();
    this.toast("info", "连接已关闭");
  }

  /** 运行当前查询（可选仅运行选区）。 */
  private async cmdRun(selectionOnly: boolean): Promise<void> {
    const tab = this.activeTab;
    if (!tab) return;
    const view = this.views.get(tab.id);
    if (view instanceof QueryTabView) {
      await view.executeSelection(selectionOnly);
      return;
    }
    if (tab.kind === "table") {
      // 表数据页：把当前表转成 SELECT 执行
      const full = tab.schema ? `${tab.schema}.${tab.object}` : tab.object;
      await this.runSqlInNewTab(tab.connectionId, `SELECT * FROM ${full};`);
    }
  }

  /** 解释当前查询（EXPLAIN 选区或全文）。 */
  private async cmdExplain(): Promise<void> {
    const tab = this.activeTab;
    if (!tab?.connectionId) {
      this.toast("error", "请先打开一个连接");
      return;
    }
    const view = this.views.get(tab.id);
    if (view instanceof QueryTabView) {
      await view.explainPublic();
      return;
    }
    this.toast("error", "解释仅适用于查询标签页");
  }

  /** 打开当前对象的表结构页。 */
  private cmdStructure(): void {
    const ref = this.currentTableRef();
    if (!ref) {
      this.toast("error", "当前对象没有表结构");
      return;
    }
    void this.openStructureTab(ref.connId, ref.schema, ref.table);
  }

  /** 筛选：定位到数据表页顶部的内联筛选栏（点"＋"添加筛选准则）。 */
  private cmdFilter(): void {
    const view = this.activeView();
    if (view instanceof DataTabView) {
      view.focusFilter();
      return;
    }
    this.toast("error", "只有表数据页支持筛选");
  }

  /** 一次性排序菜单分发器（非 null 时 sort:* 命令交由它处理）。 */
  private sortMenuOnce: ((id: string) => void) | null = null;
  /** 一次性列菜单分发器。 */
  private colMenuOnce: ((id: string) => void) | null = null;
  /** 最近鼠标位置（弹出菜单跟随）。 */
  private lastMouse = { x: 0, y: 0 };

  /** 排序：打开 / 关闭排序面板（排序列 + ↑↓方向 + 应用）。 */
  private cmdSort(): void {
    const view = this.activeView();
    if (view instanceof DataTabView) {
      view.focusSort();
      return;
    }
    this.toast("error", "只有表数据页支持排序");
  }

  /** 列显示选择菜单。 */
  private cmdColumns(): void {
    const tab = this.activeTab;
    const view = this.activeView();
    if (!(view instanceof DataTabView) || !tab) {
      this.toast("error", "只有表数据页支持列设置");
      return;
    }
    const all = view.allColumns();
    if (!all.length) return;
    const hidden = view.hiddenColumns();
    const items: MenuEntry[] = all.map((c) => ({
      id: `col-toggle:${c}`,
      label: c,
      checked: !hidden.has(c),
    }));
    this.menu.showContext(items, this.lastMouse.x || 80, this.lastMouse.y || 120);
    this.colMenuOnce = (cmd: string) => {
      if (cmd.startsWith("col-toggle:")) {
        view.toggleColumn(cmd.slice(11));
        this.notifyToolbar();
      } else {
        void this.runCommand(cmd);
        return;
      }
      this.colMenuOnce = null;
    };
  }

  /** 在新查询标签页中执行一段 SQL。 */
  private async runSqlInNewTab(connId: string, sql: string): Promise<void> {
    await this.openQueryTab(connId, sql);
    const tab = this.activeTab;
    if (!tab) return;
    const view = this.views.get(tab.id);
    if (view instanceof QueryTabView) {
      await view.execute(sql, false);
    }
  }

  /** 移动/重排只改变连接树，保留打开的连接池、数据库及已加载子树。 */
  private async moveConnectionToGroup(id: string, group: string, targetId?: string, position?: import("./types").ConnectionMovePosition): Promise<void> {
    try {
      const source = this.connections.find((c) => c.id === id);
      const target = targetId ? this.connections.find((c) => c.id === targetId) : null;
      if (!source) throw new Error("连接不存在");
      if (group && !this.groups.includes(group) && !this.connections.some((c) => c.group === group)) throw new Error("目标分组不存在");
      if ((targetId === undefined) !== (position === undefined)) throw new Error("排序目标与位置必须同时指定");
      if (targetId && (!target || target.id === id || (target.group || "") !== group)) throw new Error("无效的排序目标");
      if (position && position !== "before" && position !== "after") throw new Error("无效的排序位置");
      this.connections = await api.moveConnection(id, group, targetId, position);
      this.refreshTree();
      const folder = this.tree.find(`group:${group}`);
      if (folder) { folder.expanded = true; this.tree.render(); }
      this.tree.select(`conn:${id}`);
      this.toast("success", targetId ? "连接顺序已保存" : group ? `已移入分组「${group}」` : "已移出分组");
    } catch (e) {
      this.toast("error", String(e));
      throw e; // TreePanel 保留并播报失败状态，不能把失败当成移动成功。
    }
  }

  private cmdRenameGroup(): void {
    const node = this.tree.find(this.tree.selectedNodeId ?? "");
    if (!node?.id.startsWith("group:")) return;
    const input = document.createElement("input");
    input.value = node.label;
    input.setAttribute("aria-label", "分组名称");
    new Dialog({ title: "重命名组", width: 380, render: (body) => body.appendChild(input), buttons: [
      { label: "确定", primary: true, onClick: async (dlg) => {
        if (!input.value.trim()) { dlg.showError("请输入分组名称"); return false; }
        this.connections = await api.renameGroup(node.label, input.value.trim());
        this.groups = await api.listGroups(); this.refreshTree();
      } }, { label: "取消" },
    ] });
  }

  /** 当前数据库上下文：树中选中 / 鼠标点选的节点 → 当前标签页。 */
  private currentDbTarget(): { connId: string; schema: string } | null {
    const node = this.tree?.find(this.tree.selectedNodeId ?? "");
    if (node) {
      const parts = node.id.split("/");
      if (parts[0].startsWith("conn:")) {
        const connId = parts[0].slice(5);
        const dbIdx = parts.indexOf("db");
        if (dbIdx >= 0 && parts[dbIdx + 1]) {
          return { connId, schema: parts[dbIdx + 1] };
        }
        const cfg = this.connections.find((c) => c.id === connId);
        if (cfg?.kind === "sqlite") return { connId, schema: "main" };
        return { connId, schema: "" };
      }
    }
    const tab = this.activeTab;
    if (tab?.connectionId) return { connId: tab.connectionId, schema: tab.schema ?? "" };
    return null;
  }

  /**
   * Ribbon「查询 / 表 / 视图 / 函数 / 备份」联动：打开当前数据库的对应内容，
   * 并把树上的选中光标移到对应分组节点（未展开时落在库节点）。
   */
  private async focusDbGroup(grp: "table" | "view" | "function" | "backup" | "query"): Promise<void> {
    const t = this.currentDbTarget();
    if (grp === "query") {
      await this.cmdNewQuery();
      if (t) this.selectDbTreeNode(t, "query");
      return;
    }
    if (!t?.schema) {
      this.toast("error", "请先在左侧树中选择一个数据库");
      return;
    }
    switch (grp) {
      case "table":
        this.openObjectsTab(t.connId, t.schema, "table");
        break;
      case "view":
        this.openObjectsTab(t.connId, t.schema, "view");
        break;
      case "function":
        this.openObjectsTab(t.connId, t.schema, "function");
        break;
      case "backup":
        this.openSpecialTab("backup", t.connId, t.schema);
        break;
    }
    this.selectDbTreeNode(t, grp);
  }

  /** 树选中光标跟随到对应分组节点（分组未加载时落在库节点）。 */
  private selectDbTreeNode(t: { connId: string; schema: string }, grp: string): void {
    const base = t.schema ? `conn:${t.connId}/db/${t.schema}` : `conn:${t.connId}`;
    const node = this.tree.find(`${base}/grp-${grp}`) ?? this.tree.find(base);
    if (node) this.tree.select(node.id);
  }

  /** 「其它▼」表空间 / 事件：按协议生成系统查询，在查询页执行。 */
  private async cmdOpenOtherObject(kind: "tablespace" | "event"): Promise<void> {
    const label = kind === "tablespace" ? "表空间" : "事件";
    const t = this.currentDbTarget();
    if (!t?.connId) {
      this.toast("error", "请先在左侧树中选择一个连接");
      return;
    }
    const cfg = this.connectionById(t.connId);
    const proto = cfg ? (kindMeta(cfg.kind)?.protocol ?? "") : "";
    let sql = "";
    if (kind === "tablespace") {
      if (proto === "mysql") {
        sql =
          "SELECT TABLESPACE_NAME, FILE_NAME, ENGINE, TOTAL_EXTENTS, EXTENT_SIZE\nFROM information_schema.FILES";
      } else if (proto === "postgres") {
        sql =
          "SELECT oid, spcname, pg_get_userbyid(spcowner) AS owner, pg_tablespace_location(oid) AS location\nFROM pg_catalog.pg_tablespace";
      } else if (proto === "mssql") {
        sql =
          "SELECT name AS 表空间名, physical_name AS 文件路径, type_desc AS 类型, size AS 大小\nFROM sys.database_files";
      } else if (proto === "odbc") {
        sql = "SELECT * FROM user_tablespaces";
      }
    } else {
      if (proto === "mysql") {
        sql =
          "SELECT EVENT_SCHEMA, EVENT_NAME, EVENT_TYPE, STATUS, EXECUTE_AT, INTERVAL_VALUE, LAST_EXECUTED\nFROM information_schema.EVENTS";
      } else if (proto === "postgres") {
        sql =
          "SELECT evtname AS 事件名, evtevent AS 触发时机, evtenabled AS 状态\nFROM pg_catalog.pg_event_trigger";
      } else if (proto === "mssql") {
        sql =
          "SELECT name AS 事件会话名, startup_state AS 自启动\nFROM sys.server_event_sessions";
      }
    }
    if (!sql) {
      this.toast("error", `当前数据源不支持${label}`);
      return;
    }
    await this.openQueryTab(t.connId, sql, t.schema);
    await this.cmdRun(false);
    this.toast("info", `已在查询页执行「${label}」查询`);
  }

  /** 新建分组：弹窗输入分组名，持久化后显示在树中（空分组也保留）。 */
  private cmdNewGroup(): void {
    const input = document.createElement("input");
    input.type = "text";
    input.style.width = "240px";
    input.placeholder = "例如：开发环境";
    new Dialog({
      title: "新建组",
      width: 380,
      render: (body) => {
        const row = document.createElement("div");
        row.className = "field";
        const lab = document.createElement("div");
        lab.className = "field-label";
        lab.textContent = "分组名:";
        row.appendChild(lab);
        row.appendChild(input);
        body.appendChild(row);
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "确定",
          primary: true,
          onClick: () => {
            const name = input.value.trim();
            if (!name) return false;
            void (async () => {
              try {
                this.groups = await api.addGroup(name);
                this.refreshTree();
                this.toast("success", `分组「${name}」已创建`);
              } catch (e) {
                this.toast("error", String(e));
              }
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  /** 打开日志窗口。 */
  private cmdShowLog(): void {
    openHistoryLogDialog();
  }

  private async cmdNewQuery(): Promise<void> {
    // 跟随当前选中 / 鼠标点选的数据库，而不是默认第一个库
    const t = this.currentDbTarget();
    const connId = t?.connId ?? this.activeTab?.connectionId ?? "";
    await this.openQueryTab(connId, "", t?.schema ?? "");
  }

  private async cmdSave(): Promise<void> {
    const tab = this.activeTab;
    if (!tab) return;
    const view = this.views.get(tab.id);
    if (view instanceof QueryTabView) {
      // 查询页保存 = 命名保存到「查询」分组（树中可见、可再次打开）
      this.cmdSaveQuery();
      return;
    }
    if (view instanceof DataTabView) {
      await view.commitEdits();
      return;
    }
    if (view instanceof StructureTabView) {
      await view.saveChanges();
    }
  }

  /** 命名保存当前查询：写入收藏夹并同步树中的「查询」分组。 */
  private cmdSaveQuery(): void {
    const tab = this.activeTab;
    const view = this.activeView();
    if (!(tab && view instanceof QueryTabView)) return;
    const sql = view.editor.getSql();
    if (!sql.trim()) {
      this.toast("error", "查询内容为空");
      return;
    }
    // 若该查询此前保存过（对象名 = 收藏名），沿用原名覆盖
    const existingFav = this.favorites.find((f) => f.name === tab.object);
    const nameInput = document.createElement("input");
    nameInput.type = "text";
    nameInput.style.width = "220px";
    nameInput.value = existingFav?.name ?? tab.object ?? "";
    new Dialog({
      title: "保存查询",
      width: 380,
      render: (body) => {
        const row = document.createElement("div");
        row.className = "field";
        const lab = document.createElement("div");
        lab.className = "field-label";
        lab.textContent = "查询名:";
        row.appendChild(lab);
        row.appendChild(nameInput);
        body.appendChild(row);
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "保存",
          primary: true,
          onClick: () => {
            const name = nameInput.value.trim();
            if (!name) return false;
            void (async () => {
              const fav: Favorite = {
                id: existingFav?.id ?? `f_${Date.now().toString(36)}`,
                name,
                sql,
                connection_id: tab.connectionId || null,
                timestamp: Date.now(),
              };
              this.favorites = await api.addFavorite(fav);
              tab.object = name;
              tab.title = name;
              this.refreshQueryGroups();
              this.renderTabs();
              this.toast("success", `查询「${name}」已保存`);
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  private async cmdSaveAs(): Promise<void> {
    const tab = this.activeTab;
    if (!tab) return;
    const view = this.views.get(tab.id);
    if (view instanceof QueryTabView) {
      // 另存为：以新名称加入收藏夹
      await this.cmdAddFavorite(true);
      return;
    }
    if (view instanceof DataTabView) {
      this.cmdExport(false);
    }
  }

  private async cmdCopy(): Promise<void> {
    const tab = this.activeTab;
    if (!tab) return;
    const view = this.views.get(tab.id);
    if (view instanceof DataTabView || view instanceof ObjectsTabView) {
      const rows = [...tab.selectedRows];
      if (!rows.length) {
        this.toast("info", "请先选择要复制的行");
        return;
      }
      const text = (view as DataTabView | ObjectsTabView).grid.getText(rows, true);
      await this.writeClipboard(text);
      return;
    }
    if (view instanceof QueryTabView) {
      const text = view.editor.getSelection();
      if (!text.trim()) {
        this.toast("info", "编辑器没有选中的内容");
        return;
      }
      await this.writeClipboard(text);
    }
  }

  private async cmdCut(): Promise<void> {
    const view = this.activeView();
    if (view instanceof QueryTabView) {
      const sel = view.editor.getSelection();
      if (!sel) return;
      try {
        await navigator.clipboard.writeText(sel);
        view.editor.replaceSelection("");
      } catch {
        this.toast("error", "无法写入剪贴板");
      }
    } else {
      await this.cmdCopy();
    }
  }

  private async cmdPaste(): Promise<void> {
    const view = this.activeView();
    if (view instanceof QueryTabView) {
      try {
        const text = await navigator.clipboard.readText();
        if (text) view.editor.replaceSelection(text);
      } catch {
        this.toast("error", "无法读取剪贴板");
      }
    } else {
      this.toast("info", "请在单元格编辑或查询编辑器中使用粘贴");
    }
  }

  private cmdUndo(): void {
    const view = this.activeView();
    if (view instanceof QueryTabView) view.editor.undo();
  }

  private cmdRedo(): void {
    const view = this.activeView();
    if (view instanceof QueryTabView) view.editor.redo();
  }

  private cmdFind(): void {
    const view = this.activeView();
    if (view instanceof QueryTabView) {
      view.editor.openSearch();
    } else {
      this.toast("info", "查找适用于查询编辑器（Ctrl+F）");
    }
  }

  private cmdFindNext(): void {
    const view = this.activeView();
    if (view instanceof QueryTabView) view.editor.findNext();
  }

  private async writeClipboard(text: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
      this.toast("success", "已复制到剪贴板");
    } catch {
      this.toast("error", "无法写入剪贴板");
    }
  }

  private cmdSelectAll(): void {
    const tab = this.activeTab;
    if (!tab) return;
    const view = this.views.get(tab.id);
    if (view instanceof DataTabView || view instanceof ObjectsTabView) {
      tab.selectedRows = new Set(view.grid.rows.map((_, i) => i));
      view.grid.setState({ selectedRows: tab.selectedRows });
    } else if (view instanceof QueryTabView) {
      view.editor.selectAll();
    }
  }

  private async cmdDeleteRow(): Promise<void> {
    const tab = this.activeTab;
    if (!tab) return;
    const view = this.views.get(tab.id);
    if (view instanceof DataTabView) await view.deleteSelectedRows();
  }

  private async cmdInsertRow(): Promise<void> {
    const tab = this.activeTab;
    if (!tab) return;
    const view = this.views.get(tab.id);
    if (view instanceof DataTabView) await view.insertRow();
  }

  private async cmdCommit(): Promise<void> {
    const tab = this.activeTab;
    if (!tab) return;
    const view = this.views.get(tab.id);
    if (view instanceof DataTabView) await view.commitEdits();
  }

  private cmdRollback(): void {
    const tab = this.activeTab;
    if (!tab) return;
    const view = this.views.get(tab.id);
    if (view instanceof DataTabView) view.rollbackEdits();
  }

  private async cmdAddFavorite(asNew = false): Promise<void> {
    const tab = this.activeTab;
    if (!tab || tab.kind !== "query") {
      this.toast("error", "只有查询标签页可以加入收藏");
      return;
    }
    const view = this.views.get(tab.id);
    if (!(view instanceof QueryTabView)) return;
    const sql = view.editor.getSql();
    if (!sql.trim()) {
      this.toast("error", "查询内容为空");
      return;
    }
    const name = asNew
      ? `查询 ${Date.now().toString(36).slice(-4)}`
      : `查询 ${this.favorites.length + 1}`;
    const fav: Favorite = {
      id: `f_${Date.now().toString(36)}`,
      name,
      sql,
      connection_id: tab.connectionId || null,
      timestamp: Date.now(),
    };
    this.favorites = await api.addFavorite(fav);
    this.refreshQueryGroups();
    this.toast("success", `「${name}」已添加到收藏夹`);
  }

  /** 打开收藏的查询：弹出收藏清单，点击即打开。 */
  private cmdOpenFavorite(): void {
    if (!this.favorites.length) {
      this.toast("info", "收藏夹为空");
      return;
    }
    const items: MenuEntry[] = this.favorites.map((f) => ({
      id: `fav-open:${f.id}`,
      label: f.name,
      icon: "object_star",
    }));
    items.push({ separator: true });
    items.push({ id: "manage-favorites", label: "管理收藏夹..." });
    this.menu.showContext(items, 80, this.menuHost.getBoundingClientRect().bottom + 2);
  }

  private cmdManageFavorites(): void {
    openFavoritesDialog(
      this.favorites,
      (id) => {
        void (async () => {
          this.favorites = await api.removeFavorite(id);
          this.refreshQueryGroups();
        })();
      },
      (id) => {
        const fav = this.favorites.find((f) => f.id === id);
        if (fav) void this.openQueryTab(fav.connection_id ?? undefined, fav.sql);
      },
    );
  }

  /** 最近打开：弹出最近记录清单。 */
  private cmdRecent(): void {
    if (!this.recent.length) {
      this.toast("info", "没有最近打开的记录");
      return;
    }
    const items: MenuEntry[] = this.recent.map((r) => ({
      id: `recent-open:${r.id}`,
      label: r.label,
    }));
    items.push({ separator: true });
    items.push({ id: "recent-clear", label: "清空最近打开" });
    this.menu.showContext(items, 80, this.menuHost.getBoundingClientRect().bottom + 2);
  }

  private async cmdServerInfo(): Promise<void> {
    const connId = this.currentConnectionId() || this.activeTab?.connectionId;
    if (!connId) {
      this.toast("error", "请先打开一个连接");
      return;
    }
    try {
      await api.openConnection(connId);
      const info = await api.serverInfo(connId);
      openServerInfoDialog(info);
    } catch (e) {
      this.toast("error", String(e));
    }
  }

  private transactionContext(id?: string): TransactionContext | undefined {
    const tab = id ? this.tabs.find(item => item.id === id) : this.activeTab;
    if (!tab) return undefined;
    const view = this.views.get(tab.id);
    return {
      id: tab.id, connectionId: tab.connectionId, schema: tab.schema, kind: tab.kind,
      running: view instanceof QueryTabView && view.isExecuting(),
      pendingEdits: view instanceof DataTabView && view.hasPendingChanges(),
      prepare: async () => {
        if (!(view instanceof QueryTabView)) throw new Error("当前页面不是查询编辑器");
        await view.prepareForTransaction();
      },
    };
  }

  manualTransactionFor(id: string): ManualTransactionStatus | undefined {
    return this.manualTransactionController?.status(id);
  }
  manualTransactionIsBusy(id: string): boolean { return this.manualTransactionController?.busy(id) ?? false; }
  async refreshManualTransactionStatus(id: string): Promise<ManualTransactionStatus | undefined> {
    return this.manualTransactionController?.refresh(id);
  }
  private async closeManualTransactionTab(tab: TabState): Promise<void> {
    const context = this.transactionContext(tab.id);
    if (context) await this.manualTransactionController.closeOwner(context);
  }
  private manualTransactionTools(tab: TabState): TabToolItem[] {
    const beginReason = this.commandUnavailableReason("begin-transaction");
    const status = this.manualTransactionFor(tab.connectionId);
    const items: TabToolItem[] = [{
      id: "begin-transaction", label: "开始事务", icon: "dbn_post", disabled: !!beginReason,
      disabledReason: beginReason ?? undefined,
    }];
    if (tab.kind === "query" && status?.active) {
      const commitReason = this.commandUnavailableReason("commit-transaction");
      const rollbackReason = this.commandUnavailableReason("rollback-transaction");
      items.push(
        { id: "commit-transaction", label: "提交事务", icon: "dbn_post", disabled: !!commitReason, disabledReason: commitReason ?? undefined },
        { id: "rollback-transaction", label: "回滚事务", icon: "dbn_cancel", disabled: !!rollbackReason, disabledReason: rollbackReason ?? undefined },
      );
    }
    return items;
  }

  /** 查询创建工具仅生成 SELECT，确认后替换选区或在光标插入。 */
  private cmdQueryBuilder(): void {
    const tab = this.activeTab;
    const view = tab ? this.views.get(tab.id) : undefined;
    if (!tab || !(view instanceof QueryTabView)) { this.toast("error", "请先打开查询标签页"); return; }
    const connection = this.connectionById(tab.connectionId ?? "");
    if (!connection) { this.toast("error", "请先在查询编辑器选择连接"); return; }
    const reason = queryBuilderUnavailableReason(connection.kind);
    if (reason) { this.toast("error", reason); return; }
    const schema = tab.schema ?? "";
    if (!schema) { this.toast("error", "请先在查询编辑器选择数据库 / 模式，等待元数据加载完成"); return; }
    openQueryBuilderDialog(connection, schema, sql => {
      if (this.activeTab?.id !== tab.id || this.views.get(tab.id) !== view || tab.connectionId !== connection.id || tab.schema !== schema) {
        throw new Error("目标查询页、连接或模式已改变，请关闭并重新打开查询创建工具。");
      }
      view.editor.replaceSelection(`\n${sql}\n`);
      queueMicrotask(() => view.editor.focus());
    });
  }

  private async cmdBeautifySql(): Promise<void> {
    const tab = this.activeTab;
    if (!tab || tab.kind !== "query") {
      this.toast("error", "请先打开查询标签页");
      return;
    }
    const view = this.views.get(tab.id);
    if (!(view instanceof QueryTabView)) return;
    const sql = view.editor.getSql();
    // 分段美化：字符串字面量（含 '' 转义）与反引号标识符原样保留，仅对代码段做大写与子句换行
    const beautifySegment = (seg: string): string =>
      seg
        .replace(/\s+/g, " ")
        .replace(/\s*,\s*/g, ", ")
        .replace(
          /\b(?:select|distinct|insert into|values|update|set|delete from|union all|union|all|and|or|not|in|is null|is not null|like|between|exists|as|asc|desc|from|where|group by|order by|having|limit|left join|right join|inner join|on)\b/gi,
          (m) => m.toUpperCase(),
        )
        .replace(
          /\b(?:FROM|WHERE|GROUP BY|ORDER BY|HAVING|LIMIT|LEFT JOIN|RIGHT JOIN|INNER JOIN)\b/g,
          "\n$&",
        )
        .replace(/\bON\b/g, "\n  ON");
    const beautified = sql
      .split(/('(?:[^']|'')*'|"(?:[^"]|"")*"|`[^`]*`)/g)
      .map((part, i) => (i % 2 === 1 ? part : beautifySegment(part)))
      .join("")
      .trim();
    view.editor.setSql(beautified);
    this.toast("success", "SQL 已美化");
  }

  /** 代码段对话框：内置常用 SQL 模板，插入当前查询编辑器。 */
  private cmdCodeSnippet(): void {
    const tab = this.activeTab;
    const view = tab ? this.views.get(tab.id) : undefined;
    if (!(view instanceof QueryTabView)) {
      this.toast("error", "请先打开查询标签页");
      return;
    }
    const snippets: [string, string][] = [
      ["SELECT 全表", "SELECT * FROM table_name LIMIT 100;"],
      ["条件查询", "SELECT * FROM table_name\nWHERE column = 'value'\nORDER BY id\nLIMIT 100;"],
      ["多表连接", "SELECT a.*, b.*\nFROM table_a a\nINNER JOIN table_b b ON b.a_id = a.id\nLIMIT 100;"],
      ["分组统计", "SELECT column, COUNT(*) AS cnt\nFROM table_name\nGROUP BY column\nORDER BY cnt DESC;"],
      ["插入数据", "INSERT INTO table_name (col1, col2) VALUES ('v1', 'v2');"],
      ["更新数据", "UPDATE table_name SET col1 = 'v1' WHERE id = 1;"],
      ["删除数据", "DELETE FROM table_name WHERE id = 1;"],
      [
        "建表",
        "CREATE TABLE table_name (\n  id BIGINT PRIMARY KEY AUTO_INCREMENT,\n  name VARCHAR(100) NOT NULL,\n  created_at DATETIME\n);",
      ],
    ];
    let selected = snippets[0][1];
    new Dialog({
      title: "代码段",
      width: 560,
      render: (body) => {
        const row = document.createElement("div");
        row.style.display = "flex";
        row.style.gap = "10px";
        const list = document.createElement("div");
        list.style.width = "150px";
        list.style.border = "1px solid var(--line)";
        list.style.background = "var(--bg-surface)";
        list.style.overflow = "auto";
        list.style.height = "240px";
        const ta = document.createElement("textarea");
        ta.style.flex = "1";
        ta.style.height = "240px";
        ta.value = selected;
        for (const [name, snippet] of snippets) {
          const item = document.createElement("div");
          item.textContent = name;
          item.style.padding = "4px 8px";
          item.style.cursor = "default";
          item.addEventListener("mousedown", () => {
            list.querySelectorAll("div").forEach((x) => (x.style.background = ""));
            item.style.background = "var(--bg-selected-soft)";
            ta.value = snippet;
            selected = snippet;
          });
          list.appendChild(item);
        }
        row.appendChild(list);
        row.appendChild(ta);
        body.appendChild(row);
      },
      buttons: [
        {
          label: "插入",
          primary: true,
          onClick: () => {
            view.editor.insertText(`\n${selected}`);
            this.toast("success", "代码段已插入");
          },
        },
        { label: "取消" },
      ],
    });
  }

  private datasourceSyncReady(): boolean {
    const loading = (nodes: TreeItem[]): boolean => nodes.some(node => node.loading || loading(node.children));
    if (loading(this.treeNodes) || this.manualTransactionController.hasActive()) return false;
    return !this.tabs.some(tab => {
      const view = this.views.get(tab.id);
      return (view instanceof QueryTabView && view.isExecuting()) || (view instanceof DataTabView && view.hasPendingChanges()) || (tab.kind !== "query" && tab.dirty);
    });
  }

  private applyDatasourceSync(result: DatasourceSyncResult): void {
    const invalidated = invalidatedDatasourceIds(this.connections, result.connections);
    this.connections = result.connections;
    this.groups = result.groups;
    this.infoRequest++;
    for (const id of invalidated) {
      this.openConnections.delete(id);
      for (const db of this.openDbs) if (db.startsWith(`${id}/`)) this.openDbs.delete(db);
      for (const key of this.tableMetaCache.keys()) if (key.startsWith(`${id}/`)) this.tableMetaCache.delete(key);
    }
    const invalidateTree = (nodes: TreeItem[]) => {
      for (const node of nodes) {
        if (node.type === "connection" && invalidated.has(node.id.slice(5))) {
          node.children = []; node.loaded = false; node.connected = false; node.expanded = false;
        } else invalidateTree(node.children);
      }
    };
    invalidateTree(this.treeNodes);
    for (const tab of this.tabs) {
      if (!invalidated.has(tab.connectionId)) continue;
      tab.datasourceStale = "数据源同步已修改或删除此连接。旧页面已暂停，请关闭后重新打开；SQL 编辑内容仍保留，查询页可明确重新选择连接。";
      const view = this.views.get(tab.id);
      if (view instanceof QueryTabView) view.invalidateDatasource();
      else if (view) {
        const old = document.createElement("div"); old.className = "datasource-stale-content"; old.inert = true;
        while (view.el.firstChild) old.append(view.el.firstChild);
        const warning = document.createElement("p"); warning.className = "datasource-stale-warning"; warning.setAttribute("role", "status"); warning.textContent = tab.datasourceStale;
        view.el.append(warning, old);
      }
    }
    this.refreshTree();
    this.renderTabs();
  }

  private cmdOptions(initialTab?: "general" | "font" | "sql" | "sync"): void {
    openSettingsDialog(this.settings, (s) => {
      this.settings = s;
      this.applySettings();
      void api.setSettings(s);
      this.toast("success", "设置已保存");
    }, this.datasourceSync, initialTab);
  }

  private cmdCycleTab(delta: number): void {
    if (!this.tabs.length) return;
    const i = this.tabs.findIndex((t) => t.id === this.activeTabId);
    const n = (i + delta + this.tabs.length) % this.tabs.length;
    this.activateTab(this.tabs[n].id);
  }

  private cmdCloseOthers(): void {
    const keep = this.activeTabId;
    for (const t of [...this.tabs]) {
      if (t.id !== keep) this.closeTab(t.id);
    }
  }

  /** 关闭当前标签左侧 / 右侧的全部标签（基准为右键点中的标签）。 */
  private cmdCloseSide(side: "left" | "right"): void {
    const anchor = this.contextTabId ?? this.activeTabId;
    const idx = this.tabs.findIndex((t) => t.id === anchor);
    if (idx < 0) return;
    const doomed = side === "left" ? this.tabs.slice(0, idx) : this.tabs.slice(idx + 1);
    for (const t of doomed) this.closeTab(t.id);
  }

  /** 在标签条右键时记录基准标签（关闭左侧/右侧以此为准）。 */
  private contextTabId: string | null = null;

  private cmdCloseAll(): void {
    for (const t of [...this.tabs]) this.closeTab(t.id);
  }

  private async cmdDuplicateTab(): Promise<void> {
    const tab = this.activeTab;
    if (!tab) return;
    if (tab.kind === "query") {
      const view = this.views.get(tab.id);
      if (view instanceof QueryTabView) {
        await this.openQueryTab(tab.connectionId || undefined, view.editor.getSql());
      }
    }
  }

  private cmdTreeOpen(): void {
    const ref = this.currentTableRef();
    if (ref) {
      void this.openTableTab(ref.connId, ref.schema, ref.table, "table");
      return;
    }
    const node = this.tree.find(this.tree.selectedNodeId ?? "");
    if (node) void this.openNode(node);
  }

  private cmdTreeStructure(): void {
    const ref = this.currentTableRef();
    if (!ref) {
      this.toast("error", "请先在左侧树中选择一个表");
      return;
    }
    void this.openStructureTab(ref.connId, ref.schema, ref.table);
  }

  /** 新建表：输入表名 → 建立含自增主键 id 的最小表。 */
  private cmdNewTable(): void {
    const tab = this.activeTab;
    const connId = tab?.connectionId ?? this.currentConnectionId();
    const cfg = this.connectionById(connId);
    if (!cfg) {
      this.toast("error", "请先打开一个连接");
      return;
    }
    const schema = tab?.kind === "objects" ? tab.schema : tab?.schema ?? "";
    const nameInput = document.createElement("input");
    nameInput.type = "text";
    nameInput.style.width = "220px";
    new Dialog({
      title: "新建表",
      width: 380,
      render: (body) => {
        const row = document.createElement("div");
        row.className = "field";
        const lab = document.createElement("div");
        lab.className = "field-label";
        lab.textContent = "表名:";
        row.appendChild(lab);
        row.appendChild(nameInput);
        body.appendChild(row);
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "确定",
          primary: true,
          onClick: () => {
            const name = nameInput.value.trim();
            if (!name) return false;
            void (async () => {
              try {
                // 引号风格按协议族：MySQL/ClickHouse 反引号，PG 族 / SQLite / 其它双引号
                const isTick = cfg.kind === "mysql" || cfg.kind === "clickhouse";
                const q = (s: string) =>
                  isTick
                    ? `\`${s.replace(/`/g, "``")}\``
                    : `"${s.replace(/"/g, '""')}"`;
                const full = schema ? `${q(schema)}.${q(name)}` : q(name);
                let ddl = "";
                if (cfg.kind === "sqlite") {
                  ddl = `CREATE TABLE ${full} (id INTEGER PRIMARY KEY AUTOINCREMENT)`;
                } else if (cfg.kind === "postgresql") {
                  ddl = `CREATE TABLE ${full} (id BIGSERIAL PRIMARY KEY)`;
                } else if (cfg.kind === "clickhouse") {
                  ddl = `CREATE TABLE ${full} (id UInt64) ENGINE = MergeTree ORDER BY id`;
                } else if (cfg.kind === "redis" || cfg.kind === "mongodb") {
                  throw new Error("该数据源类型不支持关系表");
                } else {
                  ddl = `CREATE TABLE ${full} (id BIGINT PRIMARY KEY AUTO_INCREMENT)`;
                }
                await api.executeDdl(connId, ddl);
                this.toast("success", `表「${name}」已创建`);
                await this.refreshActive();
                this.cmdTreeRefresh();
              } catch (e) {
                this.toast("error", String(e));
              }
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  /** 删除表（DROP TABLE，含确认）。 */
  private cmdDropTable(): void {
    const ref = this.currentTableRef();
    if (!ref) {
      this.toast("error", "请先选择一个表");
      return;
    }
    new Dialog({
      title: "删除表",
      width: 380,
      render: (body) => {
        body.textContent = `确定要删除表「${ref.table}」吗？该操作不可恢复。`;
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "删除",
          primary: true,
          onClick: () => {
            void (async () => {
              try {
                const cfg = this.connectionById(ref.connId);
                const isTick = cfg?.kind === "mysql" || cfg?.kind === "clickhouse";
                const q = (s: string) =>
                  isTick
                    ? `\`${s.replace(/`/g, "``")}\``
                    : `"${s.replace(/"/g, '""')}"`;
                const full = ref.schema ? `${q(ref.schema)}.${q(ref.table)}` : q(ref.table);
                await api.executeDdl(ref.connId, `DROP TABLE IF EXISTS ${full}`);
                this.toast("success", `表「${ref.table}」已删除`);
                for (const t of [...this.tabs]) {
                  if (t.kind === "table" && t.object === ref.table) this.closeTab(t.id);
                }
                await this.refreshActive();
                this.cmdTreeRefresh();
              } catch (e) {
                this.toast("error", String(e));
              }
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  /** 截断表（TRUNCATE / DELETE，含确认）。 */
  private cmdTruncateTable(): void {
    const ref = this.currentTableRef();
    if (!ref) {
      this.toast("error", "请先选择一个表");
      return;
    }
    new Dialog({
      title: "截断表",
      width: 380,
      render: (body) => {
        body.textContent = `确定要截断表「${ref.table}」的全部数据吗？`;
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "确定",
          primary: true,
          onClick: () => {
            void (async () => {
              try {
                const cfg = this.connectionById(ref.connId);
                const isTick = cfg?.kind === "mysql" || cfg?.kind === "clickhouse";
                const q = (s: string) =>
                  isTick
                    ? `\`${s.replace(/`/g, "``")}\``
                    : `"${s.replace(/"/g, '""')}"`;
                const full = ref.schema ? `${q(ref.schema)}.${q(ref.table)}` : q(ref.table);
                const sql = cfg?.kind === "mysql"
                  ? `TRUNCATE TABLE ${full}`
                  : `DELETE FROM ${full}`;
                await api.executeDdl(ref.connId, sql);
                this.toast("success", `表「${ref.table}」已截断`);
                await this.refreshActive();
              } catch (e) {
                this.toast("error", String(e));
              }
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  /** 表设计器：添加字段（末尾）。 */
  private cmdAddField(): void {
    const view = this.activeView();
    if (view instanceof StructureTabView) view.addField();
  }

  /** 表设计器：在选中行上方插入字段。 */
  private cmdInsertField(): void {
    const view = this.activeView();
    if (view instanceof StructureTabView) view.insertField();
  }

  /** 表设计器：删除选中字段。 */
  private cmdDeleteField(): void {
    const view = this.activeView();
    if (view instanceof StructureTabView) view.deleteField();
  }

  /** 表设计器：切换选中字段的主键标记。 */
  private cmdTogglePrimaryKey(): void {
    const view = this.activeView();
    if (view instanceof StructureTabView) view.togglePrimaryKey();
  }

  private importExportTarget(fromTree: boolean): { connId: string; schema: string; table: string } | null {
    if (fromTree) {
      const ref = this.currentTableRef();
      if (ref) return ref;
      const target = this.currentDbTarget();
      return target ? { ...target, table: "" } : null;
    }
    const tab = this.activeTab;
    if (!tab?.connectionId) return null;
    if (tab.kind === "table" || tab.kind === "structure") {
      return { connId: tab.connectionId, schema: tab.schema, table: tab.object };
    }
    if (tab.kind === "objects") {
      const view = this.views.get(tab.id);
      const object = view instanceof ObjectsTabView ? view.selectedObject() : null;
      return { connId: tab.connectionId, schema: tab.schema, table: object?.name ?? "" };
    }
    return { connId: tab.connectionId, schema: tab.schema, table: "" };
  }

  private cmdImport(fromTree = false): void {
    const ref = this.importExportTarget(fromTree);
    const cfg = this.connectionById(ref?.connId ?? "");
    if (!cfg || !ref) {
      this.toast("error", "请先选择要导入的连接、数据库或表");
      return;
    }
    const schema = ref.schema;
    const table = ref.table;
    openImportWizard(cfg, schema, table, (n) => {
      this.toast("success", `已导入 ${n} 行`);
      void this.refreshActive();
    });
  }

  private cmdExport(fromTree = false): void {
    const tab = this.activeTab;
    const ref = this.importExportTarget(fromTree);
    const cfg = this.connectionById(ref?.connId ?? "");
    if (!cfg || !ref) {
      this.toast("error", "请先打开一个连接");
      return;
    }
    const view = tab ? this.views.get(tab.id) : undefined;
    // 查询页：导出当前结果集
    if (!fromTree && view instanceof QueryTabView && tab?.kind === "query") {
      const set = view.currentSet();
      if (!set || !set.columns.length) {
        this.toast("info", "当前没有可导出的结果集");
        return;
      }
      openResultExportDialog(cfg, async (opts) => {
        const names = set.columns.map((c) => c.name);
        return await api.exportQueryResult(opts, names, set.rows);
      });
      return;
    }
    const schema = ref?.schema ?? this.currentDbName();
    const table = ref?.table ?? (tab?.kind === "table" ? tab.object : "");
    if (!table) {
      this.toast("error", "请先选择一个表");
      return;
    }
    openExportWizard(cfg, schema, table, (n) => {
      this.toast("success", `已导出 ${n} 行`);
    });
  }

  private cmdDump(): void {
    const tab = this.activeTab;
    const connId = tab?.connectionId ?? this.currentConnectionId();
    const cfg = this.connectionById(connId);
    if (!cfg) {
      this.toast("error", "请先打开一个连接");
      return;
    }
    const ref = this.currentTableRef();
    const schema = ref?.schema ?? (tab?.kind === "objects" ? tab.schema : tab?.schema ?? "");
    const table = ref?.table ?? (tab?.kind === "table" ? tab.object : "");
    openDumpDialog(cfg, schema, table, async (opts) => {
      const n = await api.runDump(opts);
      this.toast("success", `转储完成，共 ${n} 行`);
      return n;
    });
  }

  // ------------------------------------------------------ 视图 / 函数设计

  private cmdTreeDesignView(): void {
    const node = this.tree.find(this.tree.selectedNodeId ?? "");
    if (!node) return;
    const m = node.id.match(/^conn:([^/]+)(?:\/db\/([^/]+))?\/view\/(.+)$/);
    if (!m) {
      this.toast("error", "请先在左侧树中选择一个视图");
      return;
    }
    void this.openViewDesignTab(m[1], m[2] ?? "", m[3]);
  }

  private cmdTreeDesignRoutine(): void {
    const node = this.tree.find(this.tree.selectedNodeId ?? "");
    if (!node) return;
    const m = node.id.match(/^conn:([^/]+)(?:\/db\/([^/]+))?\/obj\/(\w+)\/(.+)$/);
    if (!m) {
      this.toast("error", "请先在左侧树中选择一个函数或存储过程");
      return;
    }
    const type = m[3] === "procedure" ? "procedure" : "function";
    void this.openRoutineDesignTab(m[1], m[2] ?? "", m[4], type);
  }

  /** 保存视图定义：MySQL/PG 用 CREATE OR REPLACE，SQLite 用 DROP+CREATE。 */
  private async cmdSaveViewDesign(): Promise<void> {
    const tab = this.activeTab;
    if (tab?.kind !== "viewdesign") return;
    const view = this.views.get(tab.id) as SqlDesignTabView | undefined;
    if (!view) return;
    const sql = view.getSql().trim();
    if (!sql) {
      this.toast("error", "视图定义不能为空");
      return;
    }
    try {
      const cfg = this.connectionById(tab.connectionId);
      if (cfg?.kind === "sqlite") {
        const name = tab.object;
        const q = (s: string) => `"${s.replace(/"/g, '""')}"`;
        await api.executeSql(
          tab.connectionId,
          `DROP VIEW IF EXISTS ${q(name)};\n${sql.replace(/;\s*$/, "")};`,
        );
      } else {
        await api.executeDdl(tab.connectionId, sql.replace(/;\s*$/, ""));
      }
      this.toast("success", `视图「${tab.object}」已更新`);
      this.refreshTreeSilently();
    } catch (e) {
      this.toast("error", String(e));
    }
  }

  /** 保存函数 / 存储过程（整体重建，单语句执行）。 */
  private async cmdSaveRoutineDesign(): Promise<void> {
    const tab = this.activeTab;
    if (tab?.kind !== "routinedesign") return;
    const view = this.views.get(tab.id) as SqlDesignTabView | undefined;
    if (!view) return;
    const sql = view.getSql().trim();
    if (!sql) {
      this.toast("error", "定义不能为空");
      return;
    }
    try {
      await api.executeDdl(tab.connectionId, sql.replace(/;\s*$/, ""));
      this.toast("success", `「${tab.object}」已重建`);
      this.refreshTreeSilently();
    } catch (e) {
      this.toast("error", String(e));
    }
  }

  /** 静默刷新树（重建对象后同步）。 */
  private refreshTreeSilently(): void {
    const node = this.tree.find(this.tree.selectedNodeId ?? "");
    if (node && /\/db\//.test(node.id)) {
      node.children = [];
      node.loaded = false;
      void this.toggleNode(node).then(() => {
        node.expanded = true;
        this.tree.render();
      });
    }
  }

  // ------------------------------------------------------ 自动运行

  openScheduleTab(): void {
    const existing = this.tabs.find((t) => t.kind === "schedule");
    if (existing) {
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    const tab = this.makeTab("schedule", "自动运行", this.activeTab?.connectionId ?? "", "", "");
    this.tabs.push(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
  }

  private scheduleJobs: import("./types").ScheduleJob[] = [];
  /** 任务下次计划执行时间（应用运行期内存态）。 */
  private jobNextRun = new Map<string, number>();

  private async reloadScheduleJobs(): Promise<void> {
    this.scheduleJobs = await api.listJobs();
    for (const j of this.scheduleJobs) {
      if (j.enabled && j.interval_secs > 0 && !this.jobNextRun.has(j.id)) {
        this.jobNextRun.set(j.id, Date.now() + j.interval_secs * 1000);
      }
    }
    const tab = this.tabs.find((t) => t.kind === "schedule");
    if (tab) {
      const view = this.views.get(tab.id);
      if (view instanceof ScheduleTabView) void view.reload();
    }
  }

  private cmdNewJob(): void {
    if (!this.connections.length) {
      this.toast("error", "请先创建连接");
      return;
    }
    openScheduleJobDialog(this.connections, null, () => {
      this.toast("success", "任务已保存");
      void this.reloadScheduleJobs();
    });
  }

  private cmdEditJob(): void {
    const job = this.selectedScheduleJob();
    if (!job) {
      this.toast("error", "请先选择一个任务");
      return;
    }
    openScheduleJobDialog(this.connections, job, () => {
      this.toast("success", "任务已更新");
      void this.reloadScheduleJobs();
    });
  }

  private selectedScheduleJob(): import("./types").ScheduleJob | null {
    const tab = this.activeTab;
    if (tab?.kind !== "schedule") return null;
    const row = [...tab.selectedRows][0];
    if (row === undefined) return null;
    return this.scheduleJobs[row] ?? null;
  }

  private async cmdRunJob(): Promise<void> {
    const job = this.selectedScheduleJob();
    if (!job) {
      this.toast("error", "请先选择一个任务");
      return;
    }
    this.toast("info", `正在执行任务「${job.name}」...`);
    try {
      const [elapsed, detail] = await api.runJobNow(job.id);
      this.toast("success", `任务「${job.name}」完成（${(elapsed / 1000).toFixed(2)} s）：${detail}`);
      this.jobNextRun.set(job.id, Date.now() + job.interval_secs * 1000);
      await this.reloadScheduleJobs();
    } catch (e) {
      this.toast("error", String(e));
      await this.reloadScheduleJobs();
    }
  }

  private async cmdDeleteJob(): Promise<void> {
    const job = this.selectedScheduleJob();
    if (!job) {
      this.toast("error", "请先选择一个任务");
      return;
    }
    new Dialog({
      title: "删除任务",
      width: 380,
      render: (body) => {
        body.textContent = `确定要删除任务「${job.name}」吗？`;
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "删除",
          primary: true,
          onClick: () => {
            void (async () => {
              try {
                await api.deleteJob(job.id);
                this.jobNextRun.delete(job.id);
                this.toast("success", `任务「${job.name}」已删除`);
                await this.reloadScheduleJobs();
              } catch (e) {
                this.toast("error", String(e));
              }
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  private cmdJobHistory(): void {
    void (async () => {
      const history = await api.listJobHistory();
      openJobHistoryDialog(history);
    })();
  }

  /** 自动运行调度：每 5 秒检查到期任务并执行（应用运行期间有效）。 */
  private setupScheduleTimer(): void {
    window.setInterval(() => {
      const now = Date.now();
      for (const job of this.scheduleJobs) {
        if (!job.enabled || job.interval_secs <= 0) continue;
        const next = this.jobNextRun.get(job.id);
        if (next !== undefined && now >= next) {
          this.jobNextRun.set(job.id, now + job.interval_secs * 1000);
          void api
            .runJobNow(job.id)
            .then(() => this.reloadScheduleJobs())
            .catch(() => this.reloadScheduleJobs());
        }
      }
    }, 5000);
  }

  // ------------------------------------------------------ 用户管理

  private currentUserRef(): { connId: string; user: string; host: string } | null {
    const tab = this.activeTab;
    if (tab?.kind !== "users") return null;
    const row = [...tab.selectedRows][0];
    if (row === undefined) return null;
    const view = this.views.get(tab.id);
    if (!(view instanceof UsersTabView)) return null;
    const account = view.grid.rows[row]?.[0] ?? "";
    // 账户名形如 user@host；用户名本身可能含 @，取最后一个 @ 分隔
    const at = account.lastIndexOf("@");
    if (at < 0) return null;
    return {
      connId: tab.connectionId,
      user: account.slice(0, at),
      host: account.slice(at + 1),
    };
  }

  private cmdNewUserReal(): void {
    const connId = this.activeTab?.connectionId ?? this.currentConnectionId();
    if (!connId) {
      this.toast("error", "请先打开一个用户标签页");
      return;
    }
    openCreateUserDialog(connId, () => {
      this.toast("success", "用户已创建");
      const tab = this.tabs.find((t) => t.kind === "users");
      if (tab) {
        const view = this.views.get(tab.id);
        if (view instanceof UsersTabView) void view.reload();
      }
    });
  }

  private cmdEditUserPassword(): void {
    const ref = this.currentUserRef();
    if (!ref) {
      this.toast("error", "请先在用户列表中选择一个用户");
      return;
    }
    openChangePasswordDialog(ref.connId, ref.user, ref.host, () => {
      this.toast("success", `用户 ${ref.user}@${ref.host} 的密码已修改`);
    });
  }

  private async cmdDropUser(): Promise<void> {
    const ref = this.currentUserRef();
    if (!ref) {
      this.toast("error", "请先在用户列表中选择一个用户");
      return;
    }
    new Dialog({
      title: "删除用户",
      width: 400,
      render: (body) => {
        body.textContent = `确定要删除用户 ${ref.user}@${ref.host} 吗？该操作不可恢复。`;
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "删除",
          primary: true,
          onClick: () => {
            void (async () => {
              try {
                await api.dropDbUser(ref.connId, ref.user, ref.host);
                this.toast("success", `用户 ${ref.user}@${ref.host} 已删除`);
                const tab = this.tabs.find((t) => t.kind === "users");
                if (tab) {
                  const view = this.views.get(tab.id);
                  if (view instanceof UsersTabView) void view.reload();
                }
              } catch (e) {
                this.toast("error", String(e));
              }
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  // ------------------------------------------------------ 备份 / 还原

  /** 解析备份操作目标：优先当前备份标签页，其次树中选中的数据库。 */
  private backupTarget(): { connId: string; schema: string; view?: BackupTabView } | null {
    const tab = this.activeTab;
    if (tab?.kind === "backup") {
      const view = this.views.get(tab.id);
      if (view instanceof BackupTabView) {
        return { connId: tab.connectionId, schema: tab.schema, view };
      }
      return { connId: tab.connectionId, schema: tab.schema };
    }
    const node = this.tree.find(this.tree.selectedNodeId ?? "");
    if (node) {
      const m = node.id.match(/^conn:([^/]+)(?:\/db\/([^/]+))?/);
      if (m) {
        let schema = m[2] ?? "";
        if (!schema) {
          const cfg = this.connections.find((c) => c.id === m[1]);
          if (cfg?.kind === "sqlite") schema = "main";
        }
        if (schema) return { connId: m[1], schema };
      }
    }
    return null;
  }

  /** 刷新匹配连接 / 库的备份标签页。 */
  private refreshBackupTabs(connId: string, schema: string): void {
    for (const t of this.tabs) {
      if (t.kind === "backup" && t.connectionId === connId && t.schema === schema) {
        const view = this.views.get(t.id);
        if (view instanceof BackupTabView) void view.reload();
      }
    }
  }

  private cmdNewBackup(): void {
    const t = this.backupTarget();
    if (!t) {
      this.toast("error", "请先选择一个数据库（或打开备份标签页）");
      return;
    }
    const cfg = this.connectionById(t.connId);
    if (!cfg) {
      this.toast("error", "请先打开一个连接");
      return;
    }
    openBackupDialog(cfg, t.schema, () => {
      this.toast("success", "备份完成");
      this.refreshBackupTabs(t.connId, t.schema);
    });
  }

  private async cmdRestoreBackup(): Promise<void> {
    const t = this.backupTarget();
    const backup = t?.view?.selectedBackup();
    if (!t || !backup) {
      this.toast("error", "请先在备份列表中选择一个备份");
      return;
    }
    const cfg = this.connectionById(t.connId);
    if (!cfg) {
      this.toast("error", "连接不存在");
      return;
    }
    const target = `${cfg.name} / ${t.schema}`;
    new Dialog({
      title: "还原备份",
      width: 460,
      render: (body) => {
        body.style.padding = "18px 16px";
        const p = document.createElement("div");
        p.className = "restore-confirm";
        p.textContent = `确定要还原备份「${backup.name}」到 ${target} 吗？`;
        const hint = document.createElement("div");
        hint.className = "restore-confirm-hint";
        hint.textContent = "将逐条执行备份文件中的 SQL 语句；遇到错误会立即停止。执行过程与每条语句的结果会在下一步展示。";
        body.append(p, hint);
      },
      buttons: [
        {
          label: "开始还原",
          primary: true,
          onClick: () => {
            // 确认后立即关闭本框，改由执行进度框接管（避免两层遮罩）
            window.setTimeout(() => void this.runRestoreWithReport(t.connId, t.schema, backup.name, target), 0);
          },
        },
        { label: "取消" },
      ],
    });
  }

  /**
   * 还原备份并展示执行过程：进度条 + 逐条语句 + 每条结果。
   *
   * 先确保连接可用，再调用后端详细版还原接口；过程中实时渲染
   * 「第 N / 共 M 条」与语句摘要，结束后汇总成功 / 失败并允许复制报告。
   */
  private async runRestoreWithReport(
    connId: string,
    schema: string,
    file: string,
    target: string,
  ): Promise<void> {
    const host = document.createElement("div");
    host.className = "restore-report";

    const head = document.createElement("div");
    head.className = "restore-report-head";
    const title = document.createElement("div");
    title.className = "restore-report-title";
    title.textContent = file;
    const sub = document.createElement("div");
    sub.className = "restore-report-sub";
    sub.textContent = target;
    head.append(title, sub);

    const statusRow = document.createElement("div");
    statusRow.className = "restore-report-status";
    const spinner = document.createElement("span");
    spinner.className = "restore-spinner";
    const statusText = document.createElement("span");
    statusText.textContent = "正在准备…";
    statusRow.append(spinner, statusText);

    const bar = document.createElement("div");
    bar.className = "restore-progress";
    const fill = document.createElement("div");
    fill.className = "restore-progress-fill";
    bar.appendChild(fill);

    const summary = document.createElement("div");
    summary.className = "restore-summary";

    const list = document.createElement("div");
    list.className = "restore-steps";

    host.append(head, statusRow, bar, summary, list);

    let finished = false;
    const dlg = new Dialog({
      title: "还原备份 · 执行过程",
      width: 720,
      height: 560,
      render: (body) => {
        body.style.padding = "0";
        body.appendChild(host);
      },
      buttons: [
        {
          label: "复制报告",
          alignLeft: true,
          onClick: () => false,
        },
        {
          label: "关闭",
          primary: true,
          onClick: () => {
            // 执行中关闭 = 仅关闭窗口（后端语句已提交，无法中断）
            return true;
          },
        },
      ],
    });
    const copyBtn = dlg.footer.querySelector<HTMLButtonElement>(".dialog-footer-left .btn");
    if (copyBtn) {
      copyBtn.disabled = true;
      copyBtn.addEventListener("click", () => {
        const text = [...list.querySelectorAll<HTMLElement>(".restore-step")]
          .map((row) => row.dataset.text ?? "")
          .join("\n");
        void navigator.clipboard?.writeText(text).then(
          () => this.toast("success", "执行报告已复制"),
          () => this.toast("error", "复制失败"),
        );
      });
    }

    const setProgress = (done: number, total: number) => {
      const pct = total > 0 ? Math.min(100, (done / total) * 100) : 0;
      fill.style.width = `${pct}%`;
    };

    const appendStep = (step: import("./types").RestoreStep) => {
      const row = document.createElement("div");
      row.className = `restore-step${step.ok ? "" : " failed"}${step.skipped ? " skipped" : ""}`;
      row.dataset.text =
        `#${step.index} ${step.ok ? (step.skipped ? "SKIP" : "OK") : "FAIL"} ${step.sql}` +
        (step.error ? `\n    → ${step.error}` : "") +
        (step.affected != null ? `\n    → 影响 ${step.affected} 行` : "");

      const num = document.createElement("div");
      num.className = "restore-step-num";
      num.textContent = String(step.index);

      const icon = document.createElement("div");
      icon.className = "restore-step-icon";
      icon.innerHTML = svg(step.ok ? ICON_CHECK : ICON_CROSS);

      const main = document.createElement("div");
      main.className = "restore-step-main";
      const sqlEl = document.createElement("pre");
      sqlEl.className = "restore-step-sql";
      sqlEl.textContent = step.sql;
      main.appendChild(sqlEl);
      if (step.error) {
        const err = document.createElement("div");
        err.className = "restore-step-error";
        err.textContent = step.error;
        main.appendChild(err);
      } else if (step.affected != null) {
        const ok = document.createElement("div");
        ok.className = "restore-step-meta";
        ok.textContent = `影响 ${step.affected} 行`;
        main.appendChild(ok);
      } else {
        const ok = document.createElement("div");
        ok.className = "restore-step-meta";
        ok.textContent = "执行成功";
        main.appendChild(ok);
      }
      row.append(num, icon, main);
      list.appendChild(row);
      list.scrollTop = list.scrollHeight;
    };

    try {
      statusText.textContent = "正在打开连接…";
      await api.openConnection(connId);
      statusText.textContent = "正在执行 SQL 语句…";
      const report = await api.restoreBackupDetailed(connId, schema, file);
      for (const step of report.steps) appendStep(step);
      finished = true;
      spinner.remove();
      setProgress(report.total, report.total);
      // 执行结束后回到顶部：用户从第 1 条开始读，而非停在末尾
      list.scrollTop = 0;
      const ok = report.failed === 0;
      statusRow.classList.add(ok ? "ok" : "error");
      statusText.textContent = ok
        ? `还原完成：共执行 ${report.succeeded} 条语句`
        : `还原中断：成功 ${report.succeeded} 条，第 ${report.steps.length} 条失败`;
      summary.textContent = `总计 ${report.total} 条 · 成功 ${report.succeeded} · 失败 ${report.failed}`;
      summary.classList.add(ok ? "ok" : "error");
      if (copyBtn) copyBtn.disabled = false;
      if (ok) this.toast("success", `还原完成，共执行 ${report.succeeded} 条语句`);
      else this.toast("error", `还原在第 ${report.steps.length} 条语句处失败`);
      this.refreshBackupTabs(connId, schema);
    } catch (e) {
      finished = true;
      spinner.remove();
      statusRow.classList.add("error");
      const msg = e instanceof Error ? e.message : String(e);
      statusText.textContent = `还原失败：${msg}`;
      const failRow = document.createElement("div");
      failRow.className = "restore-step failed";
      failRow.dataset.text = `ERROR ${msg}`;
      const icon = document.createElement("div");
      icon.className = "restore-step-icon";
      icon.innerHTML = svg(ICON_CROSS);
      const main = document.createElement("div");
      main.className = "restore-step-main";
      const err = document.createElement("div");
      err.className = "restore-step-error";
      err.textContent = msg;
      main.appendChild(err);
      failRow.append(icon, main);
      list.appendChild(failRow);
      this.toast("error", msg);
    }
    void finished;
  }

  private async cmdDeleteBackup(): Promise<void> {
    const t = this.backupTarget();
    const backup = t?.view?.selectedBackup();
    if (!t || !backup) {
      this.toast("error", "请先在备份列表中选择一个备份");
      return;
    }
    new Dialog({
      title: "删除备份",
      width: 400,
      render: (body) => {
        body.textContent = `确定要删除备份「${backup.name}」吗？该操作不可恢复。`;
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "删除",
          primary: true,
          onClick: () => {
            void (async () => {
              try {
                await api.deleteBackup(t.connId, t.schema, backup.name);
                this.toast("success", `备份「${backup.name}」已删除`);
                this.refreshBackupTabs(t.connId, t.schema);
              } catch (e) {
                this.toast("error", String(e));
              }
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  private cmdOpenBackupFolder(): void {
    const t = this.backupTarget();
    if (!t) {
      this.toast("error", "请先选择一个数据库（或打开备份标签页）");
      return;
    }
    void (async () => {
      try {
        await api.openBackupFolder(t.connId, t.schema);
      } catch (e) {
        this.toast("error", String(e));
      }
    })();
  }

  private cmdTransfer(): void {
    const tab = this.activeTab;
    const connId = tab?.connectionId ?? this.currentConnectionId();
    const cfg = this.connectionById(connId);
    if (!cfg) {
      this.toast("error", "请先打开一个连接");
      return;
    }
    const ref = this.currentTableRef();
    // 数据传输复用同步对话框：当前连接不支持同步时原样传入，
    // 对话框保持空选（不默认选中第一个），由用户自行选择数据源。
    const prefill = {
      schema: ref?.schema || tab?.schema || this.currentDbName() || "",
      table: ref?.table ?? (tab?.kind === "table" ? tab.object : ""),
    };
    openTransferDialog(this.connections, cfg, prefill, async (opts) => {
      // 传输两端都必须已打开（get_db 只认注册表中的连接）
      for (const id of [opts.source_connection, opts.target_connection]) {
        await api.openConnection(id);
      }
      const n = await api.runTransfer(opts);
      this.toast("success", `已传输 ${n} 行`);
      return n;
    });
  }

  private cmdAbout(): void {
    void (async () => {
      const info = await api.aboutInfo();
      openAboutDialog(info);
    })();
  }

  private cmdOfficialDoc(): void {
    this.toast("info", "NavKit 暂未提供在线文档，可使用「帮助」菜单中的快捷键与关于信息。");
  }

  /** 快捷键速查表（帮助菜单）。 */
  private cmdShortcutHelp(): void {
    const rows: [string, string][] = [
      ["Ctrl+N", "新建连接"],
      ["Ctrl+Q", "新建查询"],
      ["Ctrl+R", "运行当前查询"],
      ["Ctrl+Shift+R", "运行已选择的 SQL"],
      ["Ctrl+S", "保存（查询 / 表数据 / 表结构）"],
      ["Ctrl+Shift+S", "另存为"],
      ["Ctrl+F / F3", "查找 / 查找下一个"],
      ["Ctrl+H", "替换"],
      ["Ctrl+Z / Ctrl+Y", "撤消 / 重做"],
      ["Ctrl+C / Ctrl+X / Ctrl+V", "复制 / 剪切 / 粘贴"],
      ["Ctrl+A", "全选"],
      ["Ctrl+I", "插入记录"],
      ["Ctrl+D", "删除选中行"],
      ["Ctrl+Tab", "切换到下一个标签页"],
      ["Ctrl+W", "关闭当前标签页"],
      ["Ctrl+Shift+W", "关闭全部标签页"],
      ["Ctrl+L", "历史日志"],
      ["Ctrl+P", "打印"],
      ["Ctrl+Space", "手动唤起 SQL 补全"],
      ["F5", "刷新"],
      ["F6", "美化 SQL"],
      ["F11", "专注模式"],
      ["Alt+F4", "退出"],
    ];
    new Dialog({
      title: "快捷键",
      width: 460,
      render: (body) => {
        const table = document.createElement("table");
        table.className = "mini-table";
        for (const [key, desc] of rows) {
          const tr = document.createElement("tr");
          const td1 = document.createElement("td");
          td1.style.width = "150px";
          const code = document.createElement("b");
          code.textContent = key;
          td1.appendChild(code);
          const td2 = document.createElement("td");
          td2.textContent = desc;
          tr.appendChild(td1);
          tr.appendChild(td2);
          table.appendChild(tr);
        }
        body.appendChild(table);
        body.style.padding = "12px 14px";
        body.style.maxHeight = "420px";
        body.style.overflow = "auto";
      },
      buttons: [{ label: "关闭", primary: true }],
    });
  }

  private cmdTreeRefresh(): void {
    const node = this.tree.find(this.tree.selectedNodeId ?? "");
    if (!node) {
      this.refreshTree();
      return;
    }
    node.children = [];
    node.loaded = false;
    node.expanded = false;
    void this.toggleNode(node);
  }

  /** 新建数据库（MySQL: CREATE DATABASE；PostgreSQL 族: CREATE SCHEMA）。 */
  private cmdNewDatabase(): void {
    const connId = this.currentConnectionId();
    const cfg = this.connectionById(connId);
    if (!cfg) {
      this.toast("error", "请先在左侧树中选择一个连接");
      return;
    }
    const nameInput = document.createElement("input");
    nameInput.type = "text";
    nameInput.style.width = "220px";
    new Dialog({
      title: "新建数据库",
      width: 380,
      render: (body) => {
        const row = document.createElement("div");
        row.className = "field";
        const lab = document.createElement("div");
        lab.className = "field-label";
        lab.textContent = "数据库名:";
        row.appendChild(lab);
        row.appendChild(nameInput);
        body.appendChild(row);
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "确定",
          primary: true,
          onClick: () => {
            const name = nameInput.value.trim();
            if (!name) return false;
            void (async () => {
              try {
                await api.executeDdl(connId, this.databaseDdl(cfg.kind, "create", name));
                this.toast("success", `数据库「${name}」已创建`);
                this.cmdTreeRefresh();
              } catch (e) {
                this.toast("error", String(e));
              }
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  /** 删除数据库（含确认）。 */
  private cmdDropDatabase(): void {
    const connId = this.currentConnectionId();
    const cfg = this.connectionById(connId);
    const db = this.currentDbName();
    if (!cfg || !db) {
      this.toast("error", "请先在左侧树中选择一个数据库");
      return;
    }
    new Dialog({
      title: "删除数据库",
      width: 400,
      render: (body) => {
        body.textContent = `确定要删除数据库「${db}」吗？该操作不可恢复，库中全部对象将被删除。`;
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "删除",
          primary: true,
          onClick: () => {
            void (async () => {
              try {
                await api.executeDdl(connId, this.databaseDdl(cfg.kind, "drop", db));
                this.openDbs.delete(`${connId}/${db}`);
                this.syncConnIcons();
                this.toast("success", `数据库「${db}」已删除`);
                this.cmdTreeRefresh();
              } catch (e) {
                this.toast("error", String(e));
              }
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  /** 生成数据库级 DDL（按协议族选择 CREATE DATABASE / SCHEMA）。 */
  private databaseDdl(kind: string, action: "create" | "drop", name: string): string {
    const isTick = kind === "mysql" || kind === "clickhouse";
    const q = (s: string) =>
      isTick ? `\`${s.replace(/`/g, "``")}\`` : `"${s.replace(/"/g, '""')}"`;
    const pgFamily = kindMeta(kind)?.protocol === "postgres";
    if (action === "create") {
      return pgFamily
        ? `CREATE SCHEMA ${q(name)}`
        : `CREATE DATABASE ${q(name)}`;
    }
    return pgFamily
      ? `DROP SCHEMA IF EXISTS ${q(name)} CASCADE`
      : `DROP DATABASE IF EXISTS ${q(name)}`;
  }

  private toggleFocusMode(): void {
    this.focusMode = !this.focusMode;
    this.sidebarVisible = !this.focusMode;
    this.toolbarVisible = !this.focusMode;
    this.infoPanelVisible = !this.focusMode;
    this.sidebar.style.display = this.sidebarVisible ? "" : "none";
    this.splitter.style.display = this.sidebarVisible ? "" : "none";
    this.infoSplitter.style.display = this.infoPanelVisible ? "" : "none";
    this.ribbon.el.style.display = this.toolbarVisible ? "" : "none";
    this.infoPanel.setVisible(this.infoPanelVisible);
    this.saveViewPrefs();
    this.toast("info", this.focusMode ? "专注模式已开启（F11 退出）" : "专注模式已关闭");
  }

  private applyViewOptions(): void {
    for (const view of this.views.values()) {
      if (view instanceof DataTabView) {
        view.applyViewOptions(this.showGridLines, this.skipNull);
      }
    }
  }

  /** 当前单元格显示格式（视图读取）。 */
  get textFormatValue(): "text" | "hex" | "image" | "web" {
    return this.textFormat;
  }

  /** 文本格式变化后刷新全部单元格内容面板。 */
  private refreshCellPreviews(): void {
    for (const view of this.views.values()) {
      if (view instanceof DataTabView || view instanceof QueryTabView) {
        view.updateCellPreview();
      }
    }
    this.renderStatus();
  }
}

/** 表状态信息中文映射（SHOW TABLE STATUS 键 → 信息面板「表信息」行）。 */
const TABLE_STATUS_LABELS: [string, string][] = [
  ["Rows", "行"],
  ["Engine", "引擎"],
  ["Auto_increment", "自动递增"],
  ["Row_format", "行格式"],
  ["Update_time", "修改日期"],
  ["Create_time", "创建日期"],
  ["Check_time", "检查时间"],
  ["Index_length", "索引长度"],
  ["Data_length", "数据长度"],
  ["Max_data_length", "最大数据长度"],
  ["Data_free", "数据可用空间"],
  ["Collation", "排序规则"],
  ["Create_options", "创建选项"],
  ["Comment", "注释"],
];

function mapTableStatus(status: [string, string][]): { label: string; value: string }[] {
  const map = new Map(status);
  const fields = TABLE_STATUS_LABELS.filter(([key]) => map.has(key)).map(([key, label]) => {
    let value = map.get(key) ?? "";
    // 长度字段：可读大小 + 原始字节数（参考样式 "5.52 MB (5,783,552)"）
    if (
      ["Index_length", "Data_length", "Max_data_length", "Data_free"].includes(key) &&
      /^\d+$/.test(value)
    ) {
      value = `${formatSize(Number(value))} (${Number(value).toLocaleString("en-US")})`;
    } else if (!value) {
      value = "--";
    }
    return { label, value };
  });
  for (const [label, value] of status) {
    if (!TABLE_STATUS_LABELS.some(([key]) => key === label)) fields.push({ label, value });
  }
  return fields;
}

/** 单元格内容面板渲染（按「文本▼」选择的格式展示焦点单元格值）。 */
function renderCellPreview(
  host: HTMLElement,
  value: string | null,
  format: "text" | "hex" | "image" | "web",
): void {
  host.innerHTML = "";
  const v = value ?? "NULL";
  if (format === "text") {
    host.textContent = v;
    return;
  }
  if (format === "hex") {
    const bytes = new TextEncoder().encode(v);
    host.textContent = [...bytes]
      .map((b) => b.toString(16).padStart(2, "0"))
      .join(" ");
    return;
  }
  if (format === "image") {
    const isDataUri = /^data:image\//.test(v);
    const isUrl = /^https?:\/\/\S+$/.test(v);
    const looksB64 =
      v.length > 64 && /^[A-Za-z0-9+/=\s]+$/.test(v) && v.length % 4 === 0;
    if (isDataUri || isUrl || looksB64) {
      const img = document.createElement("img");
      img.className = "cell-preview-img";
      img.src = isDataUri || isUrl ? v : `data:image/png;base64,${v.replace(/\s+/g, "")}`;
      img.onerror = () => {
        host.textContent = "（无法解码为图像）";
      };
      host.appendChild(img);
    } else {
      host.textContent = "（非图像数据）";
    }
    return;
  }
  // 网页：URL 显示链接；HTML 用沙箱 iframe 渲染
  if (/^https?:\/\/\S+$/.test(v)) {
    const a = document.createElement("a");
    a.href = v;
    a.textContent = v;
    a.target = "_blank";
    a.rel = "noopener";
    host.appendChild(a);
    return;
  }
  const frame = document.createElement("iframe");
  frame.className = "cell-preview-frame";
  frame.setAttribute("sandbox", "");
  frame.srcdoc = v;
  host.appendChild(frame);
}

/** 连接类型对应的图标资源名。 */
function connIconOf(cfg: ConnectionConfig | undefined): string {
  if (!cfg) return "info_pane_database";
  return DB_META[cfg.kind]?.icon ?? "info_pane_conn_postgresql";
}

/** 标签页视图基类。 */
abstract class TabView {
  readonly el: HTMLElement;
  protected app: App;
  protected tab: TabState;

  constructor(app: App, tab: TabState) {
    this.app = app;
    this.tab = tab;
    this.el = document.createElement("div");
    this.el.style.flex = "1";
    this.el.style.minHeight = "0";
    this.el.style.display = "flex";
    this.el.style.flexDirection = "column";
  }

  abstract activate(): void;
  abstract reload(): Promise<void>;
  abstract destroy(): void;
}

/** 对象列表视图（「对象」标签页：库名下的表/视图清单）。 */
class ObjectsTabView extends TabView {
  grid: DataGrid;
  private objects: import("./types").ObjectInfo[] = [];
  /** 过滤后的可见行索引。 */
  private visible: number[] = [];

  constructor(app: App, tab: TabState) {
    super(app, tab);
    this.grid = new DataGrid({
      onSort: () => {
        /* 对象清单固定按名称排序 */
      },
      onSelectionChange: (rows) => {
        this.tab.selectedRows = new Set(rows);
        app.notifyToolbar();
        this.updateFooter();
        const selected = this.objects[rows[0]];
        const cfg = app.connectionById(tab.connectionId);
        if (selected && cfg) app.showObjectDetails(cfg, tab.schema, selected.name, selected.type as ObjectType);
      },
      onRowDoubleClick: (row) => {
        const info = this.objects[row];
        if (!info) return;
        // 函数 / 存储过程打开例程页，表 / 视图打开数据页
        if (info.type === "function" || info.type === "procedure") {
          void app.openRoutineTab(
            tab.connectionId,
            tab.schema,
            info.name,
            info.type as ObjectType,
          );
        } else {
          void app.openTableTab(
            tab.connectionId,
            tab.schema,
            info.name,
            info.type as ObjectType,
          );
        }
      },
      onContextMenu: (x, y) => {
        app.showContextMenu(GRID_CONTEXT_MENU, x, y);
      },
    });
    this.el.appendChild(this.grid.el);
    // 网格页脚：左统计（设计图「19 个表 · 已选中 1 行」）
    this.footer = document.createElement("div");
    this.footer.className = "grid-footer";
    this.el.appendChild(this.footer);
  }

  private footer: HTMLElement;

  /** 工具栏「打开表 / 设计表」取选中行对象。 */
  selectedObject(): import("./types").ObjectInfo | null {
    const r = [...this.tab.selectedRows][0];
    if (r === undefined) return null;
    return this.objects[r] ?? null;
  }

  selectedObjectType(): string {
    return this.selectedObject()?.type ?? "";
  }

  filteredCount(): number {
    return this.visible.length;
  }

  activate(): void {
    void this.reload();
  }

  async reload(): Promise<void> {
    try {
      const filter = this.tab.objectType;
      if (filter === "function") {
        // 「函数」分组 / Ribbon「函数」：列出函数与存储过程
        const routines = await api.listRoutines(this.tab.connectionId, this.tab.schema);
        this.objects = routines.map(([ty, n]) => ({
          name: n,
          type: ty,
          autoIncrement: 0,
          updateTime: "",
          dataLength: 0,
          engine: "",
          rows: 0,
          comment: "",
        }));
      } else {
        const list = await api.listObjects(this.tab.connectionId, this.tab.schema);
        // 按来源过滤：树上从「表」分组进来只展示表，「视图」分组只展示视图
        this.objects =
          filter === "table" || filter === "view"
            ? list.filter((o) => o.type === filter)
            : list;
      }
      this.visible = this.objects.map((_, i) => i);
      this.renderData();
      this.app.renderStatus();
    } catch (e) {
      this.app.toast("error", `无法列出对象：${String(e)}`);
    }
  }

  private renderData(): void {
    const fmtSize = (n: number) =>
      n >= 1024 * 1024
        ? `${(n / 1024 / 1024).toFixed(2)} MB`
        : n >= 1024
          ? `${Math.max(1, Math.round(n / 1024))} KB`
          : n > 0
            ? `${n} B`
            : "";
    this.grid.setData({
      columns: [
        { name: "名", type: "text", nullable: false, primary_key: false, auto_increment: false },
        { name: "自动递增值", type: "bigint", nullable: true, primary_key: false, auto_increment: false },
        { name: "修改日期", type: "datetime", nullable: true, primary_key: false, auto_increment: false },
        { name: "数据长度", type: "bigint", nullable: true, primary_key: false, auto_increment: false },
        { name: "引擎", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "行", type: "bigint", nullable: true, primary_key: false, auto_increment: false },
        { name: "注释", type: "text", nullable: true, primary_key: false, auto_increment: false },
      ],
      rows: this.visible.map((i) => {
        const o = this.objects[i];
        return [
          o.name,
          o.autoIncrement > 0 ? String(o.autoIncrement) : "0",
          o.updateTime || "",
          fmtSize(o.dataLength),
          o.engine,
          String(o.rows),
          o.comment,
        ];
      }),
      rightAlign: ["自动递增值", "数据长度", "行"],
      rowIcons: this.visible.map((i) =>
        this.objects[i].type === "view"
          ? "info_pane_view"
          : this.objects[i].type === "function" || this.objects[i].type === "procedure"
            ? "info_pane_function"
            : "info_pane_table",
      ),
      showRowNumbers: false,
      readOnly: true,
    });
    // 参考截图实测列宽（物理px ÷1.25）：名245.6 自动递增88.8 修改日期137.6
    // 数据长度108 引擎88.8 行88.8 注释137.6
    this.grid.setColWidths([245.6, 88.8, 137.6, 108, 88.8, 88.8, 137.6]);
    this.updateFooter();
  }

  /** 页脚统计（设计图：左统计 + 右留白）。 */
  private updateFooter(): void {
    const kind =
      this.tab.objectType === "table"
        ? "表"
        : this.tab.objectType === "view"
          ? "视图"
          : this.tab.objectType === "function"
            ? "函数"
            : "对象";
    const sel = this.tab.selectedRows.size;
    this.footer.textContent = `${this.visible.length} 个${kind}${sel ? ` · 已选中 ${sel} 行` : ""}`;
  }

  destroy(): void {
    this.grid.dispose();
  }
}

/** 数据表视图（分页 + 增删改 + 提交 / 回滚）。 */
class DataTabView extends TabView {
  grid: DataGrid;
  private footer: HTMLElement;
  /** 未提交的单元格修改：行号 -> (列名 -> 新值)。 */
  private pendingUpdates = new Map<number, Map<string, string | null>>();
  /** 待删除的行（原表行）。 */
  private pendingDeletes = new Set<number>();
  /** 追加的待插入行。 */
  private pendingInserts: (string | null)[][] = [];
  /** 待插入行的修改。 */
  private pendingInsertEdits = new Map<number, Map<string, string | null>>();
  /** 隐藏的列名（「列」菜单）。 */
  private hiddenCols = new Set<string>();
  private editingCol = "";
  /** 内联筛选行（草稿）：一行一个括号组。行内多条条件按 cond.logic 连接
   *  （默认 OR），行与行按 line.logic 并列连接（默认 AND），
   *  直接对应 a = 1 AND (b = 2 OR c = 3) AND d = 4。 */
  private filterLines: {
    logic?: import("./types").FilterLogic;
    conds: { col: string; op: string; val: string; logic?: import("./types").FilterLogic }[];
  }[] = [];
  private caseSensitive = false;
  private sortRows: import("./types").SortRule[] = [];
  /** 编辑草稿与已应用状态分离；隐藏面板不丢失草稿。 */
  private barMode: "filter" | "sort" | null = null;
  private sortRowsEl!: HTMLElement;
  private requestVersion = 0;
  private filterBarEl!: HTMLElement;
  private filterRowsEl!: HTMLElement;
  private filterPlaceholderEl!: HTMLElement;
  /** 单元格内容面板（按「文本▼」格式展示焦点单元格）。 */
  private cellPreviewEl!: HTMLElement;

  constructor(app: App, tab: TabState) {
    super(app, tab);

    this.caseSensitive = tab.filters.some((l) => l.conds.some((c) => c.case_sensitive));
    this.sortRows = this.appliedSortRules().map((r) => ({ ...r }));
    // 顶部内联筛选栏（添加条件 / AND、OR / 大小写 / 多字段排序）
    this.filterBarEl = this.buildFilterBar();
    this.el.appendChild(this.filterBarEl);
    // 初始隐藏（点工具栏「筛选」「排序」才出现）
    this.renderBar();
    // 恢复标签页上已保存的筛选行（旧数据无逐条 / 逐行 logic 时按 AND 回退）
    if (tab.filters.length) {
      this.filterLines = tab.filters.map((l, k) => ({
        logic: k === 0 ? undefined : l.logic ?? tab.filter_logic ?? "AND",
        conds: l.conds.map((c, j) => ({
          col: c.column,
          op: c.operator,
          val: c.value,
          logic: j === 0 ? undefined : c.logic ?? "OR",
        })),
      }));
      this.renderFilterLines();
    }

    this.grid = new DataGrid({
      onEditStart: (row, col) => {
        const value = this.displayRows[row]?.[col] ?? null;
        this.editingCol = this.grid.columns[col]?.name ?? "";
        this.grid.beginEdit(row, col, value);
      },
      onEditEnd: (row, _col, value) => {
        this.applyEdit(row, this.editingCol, value);
      },
      onSelectionChange: (rows) => {
        this.tab.selectedRows = new Set(rows);
      },
      onColumnClick: (name) => {
        // 表头单击 = 选中列（整列高亮 + 右侧列信息 + 状态栏），排序走 ↑↓/应用
        this.app.handleColumnSelected(this.tab, name);
        this.app.renderStatus();
      },
      onCellFocus: () => {
        this.updateCellPreview();
      },
      onContextMenu: (x, y) => {
        this.app.showContextMenu(GRID_CONTEXT_MENU, x, y);
      },
    });
    this.el.appendChild(this.grid.el);

    // 单元格内容面板
    this.cellPreviewEl = document.createElement("div");
    this.cellPreviewEl.className = "cell-preview";
    this.el.appendChild(this.cellPreviewEl);

    this.footer = document.createElement("div");
    this.footer.className = "grid-footer";
    this.el.appendChild(this.footer);
  }

  get total(): number {
    return this.tab.page?.total ?? 0;
  }

  /** 原表行数（不含待插入行）。 */
  private originalRowCount(): number {
    return this.tab.page?.rows.length ?? 0;
  }

  /** 网格当前显示的行（原行 + 追加行，含未提交修改）。 */
  private get displayRows(): (string | null)[][] {
    const page = this.tab.page;
    if (!page) return [];
    const inserted = this.pendingInserts.map((row, i) => {
      const out = [...row];
      const edits = this.pendingInsertEdits.get(i);
      if (edits) {
        page.columns.forEach((c, ci) => {
          if (edits.has(c.name)) out[ci] = edits.get(c.name) ?? null;
        });
      }
      return out;
    });
    const original = page.rows.map((row, r) => {
      const edits = this.pendingUpdates.get(r);
      if (!edits) return [...row];
      const out = [...row];
      page.columns.forEach((c, ci) => {
        if (edits.has(c.name)) out[ci] = edits.get(c.name) ?? null;
      });
      return out;
    });
    return [...original, ...inserted];
  }

  hasPendingChanges(): boolean { return this.hasPending(); }

  private hasPending(): boolean {
    return (
      this.pendingUpdates.size > 0 ||
      this.pendingDeletes.size > 0 ||
      this.pendingInserts.length > 0
    );
  }

  /** 供工具栏判断行数。 */
  get rowCount(): number {
    return this.displayRows.length;
  }

  /** 行标记（新增 / 删除 / 修改）。 */
  private rowFlags(): Record<number, "insert" | "delete" | "dirty"> {
    const flags: Record<number, "insert" | "delete" | "dirty"> = {};
    const base = this.originalRowCount();
    this.pendingInserts.forEach((_, i) => {
      flags[base + i] = "insert";
    });
    for (const r of this.pendingDeletes) flags[r] = "delete";
    for (const r of this.pendingUpdates.keys()) {
      if (flags[r] !== "delete") flags[r] = "dirty";
    }
    return flags;
  }

  /** 应用查看选项（显示网格 / 跳过 Null）。 */
  applyViewOptions(lined: boolean, skipNull: boolean): void {
    this.grid.setState({ lined, skipNull });
  }

  /** 全部列名（含隐藏列）。 */
  allColumns(): string[] {
    return this.tab.page?.columns.map((c) => c.name) ?? [];
  }

  hiddenColumns(): Set<string> {
    return new Set(this.hiddenCols);
  }

  /** 切换列显示。 */
  toggleColumn(name: string): void {
    if (this.hiddenCols.has(name)) {
      this.hiddenCols.delete(name);
    } else {
      if (this.allColumns().length - this.hiddenCols.size <= 1) return;
      this.hiddenCols.add(name);
    }
    this.applyData();
  }

  /** 原生按钮保留既有紧凑样式，同时支持键盘和清晰可访问名称。 */
  private panelButton(label: string, action: () => void, title = label): HTMLButtonElement {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "fb-btn fb-act";
    b.textContent = label;
    b.title = title;
    b.addEventListener("click", action);
    return b;
  }

  private appliedSortRules(): import("./types").SortRule[] {
    return this.tab.order_rules ?? (this.tab.orderBy
      ? [{ column: this.tab.orderBy, ascending: this.tab.ascending }]
      : []);
  }

  /** 顶部内联面板：整组条件逻辑、大小写敏感、多字段优先级排序。 */
  private buildFilterBar(): HTMLElement {
    const bar = document.createElement("div");
    bar.className = "filter-bar";
    const filterPane = document.createElement("div");
    filterPane.className = "fb-pane";
    const tools = document.createElement("div");
    tools.className = "filter-tools";
    tools.style.height = "auto";
    tools.style.minHeight = "24px";
    tools.style.flexWrap = "wrap";
    tools.appendChild(this.panelButton("＋ 添加条件行", () => this.addFilterLine(), "新增一行条件；同一行内可放多个条件（自动括为一组），行与行并列连接"));
    const caseLabel = document.createElement("label");
    caseLabel.style.display = "inline-flex";
    caseLabel.style.alignItems = "center";
    caseLabel.style.whiteSpace = "nowrap";
    const caseInput = document.createElement("input");
    caseInput.type = "checkbox";
    caseInput.className = "fb-case";
    caseInput.checked = this.caseSensitive;
    caseInput.addEventListener("change", () => (this.caseSensitive = caseInput.checked));
    caseLabel.append(caseInput, document.createTextNode("大小写敏感"));
    tools.appendChild(caseLabel);
    this.filterPlaceholderEl = document.createElement("div");
    this.filterPlaceholderEl.className = "filter-placeholder";
    this.filterPlaceholderEl.textContent = "一行条件为一组：同一行内多个条件自动括起（行内可选 AND / OR），行与行并列连接；删除全部行后应用可清除筛选";
    tools.appendChild(this.filterPlaceholderEl);
    this.filterRowsEl = document.createElement("div");
    this.filterRowsEl.className = "filter-rows";
    const filterActions = document.createElement("div");
    filterActions.className = "filter-actions";
    filterActions.appendChild(this.panelButton("应用筛选", () => this.applyFilters()));
    filterActions.appendChild(this.panelButton("清除筛选", () => {
      if (this.hasPending()) {
        this.app.toast("info", "存在未提交的修改，请先提交或回滚");
        return;
      }
      this.filterLines = [];
      this.renderFilterLines();
      this.applyFilters();
    }));
    filterPane.append(tools, this.filterRowsEl, filterActions);

    const sortPane = document.createElement("div");
    sortPane.className = "fb-pane";
    const sortTools = document.createElement("div");
    sortTools.className = "filter-actions";
    sortTools.appendChild(this.panelButton("＋ 添加排序字段", () => this.addSortRow()));
    const hint = document.createElement("span");
    hint.className = "filter-placeholder";
    hint.textContent = "从上到下依次排序（优先级 1 最高）";
    sortTools.appendChild(hint);
    this.sortRowsEl = document.createElement("div");
    this.sortRowsEl.className = "filter-rows sort-rows";
    const sortActions = document.createElement("div");
    sortActions.className = "filter-actions";
    sortActions.appendChild(this.panelButton("应用排序", () => this.applySort()));
    sortActions.appendChild(this.panelButton("清除排序", () => {
      if (this.hasPending()) {
        this.app.toast("info", "存在未提交的修改，请先提交或回滚");
        return;
      }
      this.sortRows = [];
      this.renderSortRows();
      this.applySort();
    }));
    sortPane.append(sortTools, this.sortRowsEl, sortActions);
    bar.append(filterPane, sortPane);
    return bar;
  }

  /** 按当前模式显示 / 隐藏面板（null 全隐藏）。 */
  private renderBar(): void {
    if (!this.filterBarEl) return;
    this.filterBarEl.style.display = this.barMode ? "" : "none";
    this.filterBarEl.querySelectorAll<HTMLElement>(".fb-pane").forEach((p, i) => {
      p.style.display = this.barMode === (i === 0 ? "filter" : "sort") ? "" : "none";
    });
  }

  private addSortRow(): void {
    const selected = this.grid.columns[this.grid.selectedColumn]?.name;
    const available = this.allColumns().filter((c) => !this.sortRows.some((r) => r.column === c));
    const col = available.includes(selected ?? "") ? selected : available[0];
    if (!col) {
      this.app.toast("info", "没有更多可添加的排序字段");
      return;
    }
    this.sortRows.push({ column: col, ascending: true });
    this.renderSortRows();
  }

  private renderSortRows(): void {
    this.sortRowsEl.innerHTML = "";
    if (!this.sortRows.length) this.sortRowsEl.textContent = "未设置排序；可添加多个字段";
    this.sortRows.forEach((row, i) => {
      const el = document.createElement("div");
      el.className = "filter-line sort-line";
      const priority = document.createElement("span");
      priority.className = "fb-priority";
      priority.textContent = `${i + 1}.`;
      const col = document.createElement("span");
      col.className = "fb-txt";
      col.textContent = row.column;
      col.setAttribute("aria-label", `排序字段 ${i + 1}`);
      col.title = "点击修改排序字段";
      col.addEventListener("click", () => {
        this.showPicker(col, this.allColumns().map((name) => ({ value: name, label: name })), row.column, (v) => {
          row.column = v;
          this.renderSortRows();
        });
      });
      const dir = document.createElement("span");
      dir.className = "fb-txt fb-dir";
      dir.textContent = row.ascending ? "↑" : "↓";
      dir.setAttribute("aria-label", `排序方向 ${i + 1}`);
      dir.title = row.ascending ? "升序（点击切换为降序）" : "降序（点击切换为升序）";
      dir.addEventListener("click", () => {
        row.ascending = !row.ascending;
        this.renderSortRows();
      });
      const move = (delta: number) => {
        const target = i + delta;
        if (target < 0 || target >= this.sortRows.length) return;
        [this.sortRows[i], this.sortRows[target]] = [this.sortRows[target], this.sortRows[i]];
        this.renderSortRows();
      };
      const up = this.panelButton("▲", () => move(-1), "上移");
      up.disabled = i === 0;
      const down = this.panelButton("▼", () => move(1), "下移");
      down.disabled = i === this.sortRows.length - 1;
      el.append(priority, col, dir, up, down, this.panelButton("×", () => {
        this.sortRows.splice(i, 1);
        this.renderSortRows();
      }, "删除排序字段"));
      this.sortRowsEl.appendChild(el);
    });
  }

  /** 新增一行（单个条件）。同一行内可继续追加条件，整行括为一组。 */
  private addFilterLine(): void {
    const cols = this.allColumns();
    if (!cols.length) {
      this.app.toast("info", "当前表还没有列");
      return;
    }
    const selected = this.grid.columns[this.grid.selectedColumn]?.name;
    // 新行沿用上一行的连接方式，保持组语义自然延续
    const last = this.filterLines[this.filterLines.length - 1];
    this.filterLines.push({
      logic: this.filterLines.length ? last?.logic ?? "AND" : undefined,
      conds: [{ col: selected ?? cols[0], op: "=", val: "" }],
    });
    this.renderFilterLines();
  }

  /** 在某行末尾追加条件（同一行 = 括号一组）；行内连接方式默认 OR。 */
  private addLineCond(k: number): void {
    const line = this.filterLines[k];
    const cols = this.allColumns();
    if (!line || !cols.length) return;
    const last = line.conds[line.conds.length - 1];
    line.conds.push({
      col: this.grid.columns[this.grid.selectedColumn]?.name ?? cols[0],
      op: "=",
      val: "",
      logic: last?.logic ?? "OR",
    });
    this.renderFilterLines();
  }

  /** 删除行内一条条件；行空了整行移除。 */
  private removeCond(k: number, j: number): void {
    const line = this.filterLines[k];
    if (!line) return;
    line.conds.splice(j, 1);
    if (!line.conds.length) this.filterLines.splice(k, 1);
    this.renderFilterLines();
  }

  /** 筛选运算符：内部值（小写，后端兼容）→ SQL 显示文本。 */
  private static readonly FILTER_OPS: [string, string][] = [
    ["=", "="], ["<>", "<>"], [">", ">"], [">=", ">="], ["<", "<"], ["<=", "<="],
    ["like", "LIKE"], ["not like", "NOT LIKE"], ["is null", "IS NULL"], ["is not null", "IS NOT NULL"],
  ];

  /** 在锚点下方弹出轻量选项列表（无边框文本样式配套；点击别处 / 再点关闭）。 */
  private showPicker(
    anchor: HTMLElement,
    options: { value: string; label: string }[],
    current: string | null,
    onPick: (value: string) => void,
  ): void {
    document.querySelectorAll(".fb-picker").forEach((m) => m.remove());
    const menu = document.createElement("div");
    menu.className = "menu-popup fb-picker";
    for (const o of options) {
      const row = document.createElement("div");
      row.className = "menu-entry" + (o.value === current ? " current" : "");
      const label = document.createElement("div");
      label.className = "menu-entry-label";
      label.textContent = o.label;
      row.appendChild(label);
      row.addEventListener("mouseup", () => {
        menu.remove();
        document.removeEventListener("mousedown", close, true);
        onPick(o.value);
      });
      menu.appendChild(row);
    }
    const close = (e: MouseEvent) => {
      if (!menu.contains(e.target as Node)) {
        menu.remove();
        document.removeEventListener("mousedown", close, true);
      }
    };
    document.addEventListener("mousedown", close, true);
    document.body.appendChild(menu);
    const rect = anchor.getBoundingClientRect();
    const box = menu.getBoundingClientRect();
    let left = rect.left;
    let top = rect.bottom + 1;
    if (left + box.width > window.innerWidth - 4) left = Math.max(2, window.innerWidth - box.width - 4);
    if (top + box.height > window.innerHeight - 4) top = Math.max(2, rect.top - box.height - 1);
    menu.style.left = `${left}px`;
    menu.style.top = `${top}px`;
  }

  /** AND / OR 内联文本，点击就地切换（不重绘整行）。 */
  private joinText(
    value: import("./types").FilterLogic,
    on: (v: import("./types").FilterLogic) => void,
  ): HTMLElement {
    const s = document.createElement("span");
    s.className = "fb-join-text";
    s.textContent = value;
    s.title = "点击切换 AND / OR";
    s.addEventListener("click", () => {
      const next = s.textContent === "AND" ? "OR" : "AND";
      s.textContent = next;
      on(next);
    });
    return s;
  }

  /** 单条条件 chip：列名 / 操作符 / 值均为无边框文本，点击修改。 */
  private buildCondChip(
    c: { col: string; op: string; val: string; logic?: import("./types").FilterLogic },
    k: number,
    j: number,
  ): HTMLElement {
    const chip = document.createElement("div");
    chip.className = "cond-chip";
    const col = document.createElement("span");
    col.className = "fb-txt";
    col.textContent = c.col;
    col.setAttribute("aria-label", `筛选字段 ${k + 1}.${j + 1}`);
    col.title = "点击修改字段";
    col.addEventListener("click", () => {
      this.showPicker(col, this.allColumns().map((name) => ({ value: name, label: name })), c.col, (v) => {
        c.col = v;
        this.renderFilterLines();
      });
    });
    const opDisplay = (raw: string) =>
      DataTabView.FILTER_OPS.find(([v]) => v === raw)?.[1] ?? raw;
    const op = document.createElement("span");
    op.className = "fb-txt fb-op-txt";
    op.textContent = opDisplay(c.op);
    op.setAttribute("aria-label", `筛选运算符 ${k + 1}.${j + 1}`);
    op.title = "点击修改操作符";
    op.addEventListener("click", () => {
      this.showPicker(op, DataTabView.FILTER_OPS.map(([v, label]) => ({ value: v, label })), c.op, (v) => {
        c.op = v;
        this.renderFilterLines();
      });
    });
    chip.append(col, op, this.buildValText(c, k, j));
    const del = this.panelButton("×", () => this.removeCond(k, j), "删除该条件");
    del.classList.add("fb-cond-del");
    chip.appendChild(del);
    return chip;
  }

  /** 值文本：点击就地出现输入框，Enter 提交并应用、失焦提交、Esc 取消。 */
  private buildValText(
    c: { op: string; val: string },
    k: number,
    j: number,
  ): HTMLElement {
    const s = document.createElement("span");
    s.className = "fb-txt fb-val";
    s.textContent = c.val;
    if (!c.val) s.classList.add("empty");
    if (/null/i.test(c.op)) s.style.display = "none";
    s.tabIndex = 0;
    s.setAttribute("aria-label", `筛选值 ${k + 1}.${j + 1}`);
    s.title = "点击输入值（LIKE 可用 % 与 _ 通配；空字符串也是有效值）";
    s.addEventListener("click", () => {
      const input = document.createElement("input");
      input.className = "fb-edit";
      input.type = "text";
      input.setAttribute("aria-label", `筛选值 ${k + 1}.${j + 1}`);
      input.value = c.val;
      s.replaceWith(input);
      input.focus();
      let done = false;
      const commit = (apply: boolean, runApply = false) => {
        if (done) return;
        done = true;
        if (apply) c.val = input.value;
        this.renderFilterLines();
        if (apply && runApply && c.val) void this.applyFilters();
      };
      input.addEventListener("keydown", (e) => {
        e.stopPropagation();
        if (e.key === "Enter") {
          e.preventDefault();
          commit(true, true);
        }
        if (e.key === "Escape") {
          e.preventDefault();
          commit(false);
        }
      });
      input.addEventListener("blur", () => commit(true));
    });
    return s;
  }

  /** 重建筛选行 DOM：一行 = [行连接] （ chip OR chip ） ＋ ✕，无边框文本样式。 */
  private renderFilterLines(): void {
    this.filterRowsEl.innerHTML = "";
    this.filterPlaceholderEl.style.display = this.filterLines.length ? "none" : "";
    this.filterLines.forEach((line, k) => {
      const el = document.createElement("div");
      el.className = "filter-line";
      // 首行无连接方式；其余行点击文本切换与上一行条件组的 AND / OR
      if (k > 0) el.appendChild(this.joinText(line.logic ?? "AND", (v) => (line.logic = v)));
      const group = document.createElement("div");
      group.className = "cond-group";
      if (line.conds.length > 1) {
        const brOpen = document.createElement("span");
        brOpen.className = "fb-bracket";
        brOpen.textContent = "（";
        group.appendChild(brOpen);
      }
      line.conds.forEach((c, j) => {
        if (j > 0) group.appendChild(this.joinText(c.logic ?? "OR", (v) => (c.logic = v)));
        group.appendChild(this.buildCondChip(c, k, j));
      });
      if (line.conds.length > 1) {
        const brClose = document.createElement("span");
        brClose.className = "fb-bracket";
        brClose.textContent = "）";
        group.appendChild(brClose);
      }
      el.appendChild(group);
      el.appendChild(this.panelButton("＋", () => this.addLineCond(k), "在本行追加条件（整行括为一组）"));
      el.appendChild(this.panelButton("✕", () => {
        this.filterLines.splice(k, 1);
        this.renderFilterLines();
      }, "删除整行"));
      this.filterRowsEl.appendChild(el);
    });
  }

  /** 工具栏「筛选」：切换筛选面板（再点隐藏），首次打开自动加一行空条件。 */
  focusFilter(): void {
    this.barMode = this.barMode === "filter" ? null : "filter";
    this.renderBar();
    if (this.barMode === "filter") {
      if (!this.filterLines.length) this.addFilterLine();
      const last = this.filterRowsEl.querySelector<HTMLInputElement>(".fb-val");
      last?.focus();
    }
  }

  /** 工具栏「排序」：切换排序面板（与筛选面板不同：排序列 + 升/降 + 应用）。 */
  focusSort(): void {
    this.barMode = this.barMode === "sort" ? null : "sort";
    this.renderBar();
    if (this.barMode === "sort") {
      this.renderSortRows();
      this.sortRowsEl.querySelector<HTMLSelectElement>("select")?.focus();
    }
  }

  /** 应用筛选：一行一括号组（行内多条自动括起），行间按行连接方式并列，
   *  直接对应 a=1 AND (b=2 OR c=3) AND d=4；空串是有效值，不悄悄丢弃条件。 */
  private applyFilters(): void {
    if (this.hasPending()) {
      this.app.toast("info", "存在未提交的修改，请先提交或回滚");
      return;
    }
    // filter_logic 仅作旧字段回退；实际连接方式已逐行 / 行内保存
    this.tab.filter_logic = "AND";
    this.tab.filters = this.filterLines.map((l, k) => ({
      logic: k > 0 ? l.logic ?? "AND" : undefined,
      conds: l.conds.map((c, j) => ({
        column: c.col,
        operator: c.op,
        value: c.val,
        case_sensitive: this.caseSensitive,
        logic: j > 0 ? c.logic ?? "OR" : undefined,
      })),
    }));
    this.tab.pageOffset = 0;
    void this.reload();
    this.app.renderStatus();
  }

  /** 多字段排序数组顺序即优先级；删除全部后应用清除排序。 */
  private applySort(): void {
    if (this.hasPending()) {
      this.app.toast("info", "存在未提交的修改，请先提交或回滚");
      return;
    }
    this.tab.order_rules = this.sortRows.map((r) => ({ ...r }));
    this.tab.orderBy = this.sortRows[0]?.column ?? null;
    this.tab.ascending = this.sortRows[0]?.ascending ?? true;
    this.tab.pageOffset = 0;
    void this.reload();
    this.app.renderStatus();
  }

  /** 单元格内容面板：按「文本▼」格式展示焦点单元格。 */
  updateCellPreview(): void {
    const cell = this.grid.focusedCell;
    const value = cell ? (this.grid.rows[cell.row]?.[cell.col] ?? null) : null;
    renderCellPreview(this.cellPreviewEl, value, this.app.textFormatValue);
    this.app.renderStatus();
  }

  /** 状态栏信息。 */
  statusInfo(): { sql: string; pageText: string } {
    const page = this.tab.page;
    const full = this.tab.schema
      ? `${this.tab.schema}.${this.tab.object}`
      : this.tab.object;
    return {
      sql: `SELECT * FROM ${full} LIMIT ${this.tab.pageOffset},${this.tab.pageLimit}`,
      pageText: page
        ? `第 ${Math.min(this.tab.pageOffset + 1, Math.max(page.total, 1))} 条记录（共 ${page.total} 条）于第 ${Math.floor(this.tab.pageOffset / this.tab.pageLimit) + 1} 页`
        : "",
    };
  }

  activate(): void {
    if (!this.tab.page) {
      void this.reload();
    } else {
      this.applyData();
    }
  }

  async reload(): Promise<void> {
    if (this.hasPending()) {
      this.app.toast("info", "存在未提交的修改，请先提交（✓）或回滚（✗）");
      return;
    }
    const version = ++this.requestVersion;
    this.el.setAttribute("aria-busy", "true");
    try {
      const res = await api.pageTable({
        connection_id: this.tab.connectionId,
        schema: this.tab.schema,
        table: this.tab.object,
        offset: this.tab.pageOffset,
        limit: this.tab.pageLimit,
        order_by: this.tab.orderBy,
        ascending: this.tab.ascending,
        filters: this.tab.filters,
        filter_logic: this.tab.filter_logic ?? "AND",
        order_rules: this.appliedSortRules(),
      });
      // 快速连续应用时旧响应不得覆盖新条件的数据，也不得覆盖编辑中的网格。
      if (version !== this.requestVersion || this.hasPending()) return;
      this.tab.page = res;
      this.tab.selectedRows.clear();
      this.applyData();
      this.renderFilterLines();
      this.renderSortRows();
      this.app.notifyToolbar();
      this.app.renderStatus();
    } catch (e) {
      if (version === this.requestVersion) this.app.toast("error", String(e));
    } finally {
      if (version === this.requestVersion) this.el.setAttribute("aria-busy", "false");
    }
  }

  private applyData(): void {
    const page = this.tab.page;
    if (!page) return;
    const cols = page.columns.filter((c) => !this.hiddenCols.has(c.name));
    // 全列索引 -> 显示列索引（隐藏列后 dirty 高亮才不会标错列）
    const displayIdx = new Map<string, number>();
    cols.forEach((c, di) => displayIdx.set(c.name, di));
    const dirtyCells = new Set<string>();
    for (const [r, edits] of this.pendingUpdates) {
      for (const name of edits.keys()) {
        const di = displayIdx.get(name);
        if (di !== undefined) dirtyCells.add(`${r}:${di}`);
      }
    }
    for (const [r, edits] of this.pendingInsertEdits) {
      for (const name of edits.keys()) {
        const di = displayIdx.get(name);
        if (di !== undefined) dirtyCells.add(`${this.originalRowCount() + r}:${di}`);
      }
    }
    // 索引映射只算一次（此前每行每格重复 findIndex，大页 O(rows×cols²)）
    const idxs = cols.map((c) => page.columns.findIndex((pc) => pc.name === c.name));
    const rows = this.displayRows.map((row) => idxs.map((i) => row[i] ?? null));
    this.grid.setData({
      columns: cols,
      rows,
      orderBy: this.tab.orderBy,
      ascending: this.tab.ascending,
      orderRules: this.appliedSortRules(),
      selectedRows: this.tab.selectedRows,
      primaryKeys: page.primary_keys,
      lined: this.app.showGridLinesState(),
      skipNull: this.app.skipNullState(),
      rowFlags: this.rowFlags(),
      dirtyCells,
    });
    this.renderFooter(page);
  }

  private renderFooter(page: import("../core/types").PageResult): void {
    this.footer.innerHTML = "";
    const total = page.total;
    const pages = Math.max(1, Math.ceil(total / this.tab.pageLimit));
    const pageNo = Math.floor(this.tab.pageOffset / this.tab.pageLimit) + 1;
    const hasPending = this.hasPending();

    const svgBtn = (icon: IconDef, title: string) => {
      const b = document.createElement("div");
      b.className = "grid-footer-btn";
      b.title = title;
      b.innerHTML = svg(icon);
      return b;
    };

    // 左：增 / 删 / 提交 / 回滚（网格底部工具）
    const addBtn = svgBtn(ICON_PLUS_LINE, "插入记录");
    addBtn.addEventListener("click", () => void this.insertRow());
    this.footer.appendChild(addBtn);

    const delBtn = svgBtn(ICON_MINUS_LINE, "删除记录");
    delBtn.addEventListener("click", () => void this.deleteSelectedRows());
    this.footer.appendChild(delBtn);

    const commitBtn = svgBtn(ICON_CHECK, "提交改动");
    commitBtn.style.opacity = hasPending ? "1" : "0.45";
    commitBtn.addEventListener("click", () => void this.commitEdits());
    this.footer.appendChild(commitBtn);

    const rollbackBtn = svgBtn(ICON_CROSS, "回滚改动");
    rollbackBtn.style.opacity = hasPending ? "1" : "0.45";
    rollbackBtn.addEventListener("click", () => this.rollbackEdits());
    this.footer.appendChild(rollbackBtn);

    const pager = document.createElement("div");
    pager.className = "grid-pager";
    pager.style.marginLeft = "12px";

    const mkPage = (icon: IconDef, title: string, onClick: () => void) => {
      const b = svgBtn(icon, title);
      b.addEventListener("click", () => {
        if (this.hasPending()) {
          this.app.toast("info", "存在未提交的修改，请先提交或回滚");
          return;
        }
        onClick();
      });
      pager.appendChild(b);
    };

    mkPage(ICON_PAGE_FIRST, "第一页", () => {
      this.tab.pageOffset = 0;
      void this.reload();
    });
    mkPage(ICON_PAGE_PREV, "上一页", () => {
      this.tab.pageOffset = Math.max(0, this.tab.pageOffset - this.tab.pageLimit);
      void this.reload();
    });

    const pageBox = document.createElement("input");
    pageBox.type = "text";
    pageBox.value = String(pageNo);
    pageBox.addEventListener("change", () => {
      const n = Number(pageBox.value);
      if (!Number.isFinite(n) || n < 1) {
        pageBox.value = String(pageNo);
        return;
      }
      if (this.hasPending()) {
        this.app.toast("info", "存在未提交的修改，请先提交或回滚");
        pageBox.value = String(pageNo);
        return;
      }
      this.tab.pageOffset = (Math.min(n, pages) - 1) * this.tab.pageLimit;
      void this.reload();
    });
    pager.appendChild(pageBox);

    mkPage(ICON_PAGE_NEXT, "下一页", () => {
      this.tab.pageOffset = Math.min(
        (pages - 1) * this.tab.pageLimit,
        this.tab.pageOffset + this.tab.pageLimit,
      );
      void this.reload();
    });
    mkPage(ICON_PAGE_LAST, "最后一页", () => {
      this.tab.pageOffset = (pages - 1) * this.tab.pageLimit;
      void this.reload();
    });

    const more = svgBtn(ICON_MORE_DOTS, "更多");
    more.addEventListener("click", (e) => {
      const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
      this.app.showContextMenu(
        [
          { id: "grid-copy", label: "复制", shortcut: "Ctrl+C" },
          { id: "grid-export", label: "导出..." },
          { separator: true },
          { id: "grid-filter", label: "筛选..." },
          { id: "grid-sort", label: "排序..." },
          { id: "columns", label: "列..." },
          { separator: true },
          { id: "grid-refresh", label: "刷新", shortcut: "F5" },
        ],
        rect.left,
        rect.bottom,
      );
    });
    pager.appendChild(more);

    this.footer.appendChild(pager);

    // 筛选 / 排序生效指示（点击可再次打开对应对话框）
    const activeFilters = this.tab.filters.length;
    if (activeFilters || this.tab.orderBy) {
      const chip = document.createElement("div");
      chip.style.display = "flex";
      chip.style.alignItems = "center";
      chip.style.gap = "4px";
      chip.style.marginLeft = "8px";
      chip.style.padding = "1px 8px";
      chip.style.border = "1px solid var(--line-strong)";
      chip.style.background = "var(--bg-surface)";
      chip.style.fontSize = "10.5px";
      chip.style.color = "var(--fg-muted)";
      chip.style.whiteSpace = "nowrap";
      chip.style.maxWidth = "320px";
      chip.style.overflow = "hidden";
      chip.style.textOverflow = "ellipsis";
      const parts: string[] = [];
      if (activeFilters) {
        // 按行显示括号组，形式与 SQL 一致：a=1 AND (b~x OR c IS NULL)
        parts.push(
          `筛选: ${this.tab.filters
            .map((l, k) => {
              const body = l.conds
                .map((c, j) => {
                  const expr = /null/.test(c.operator)
                    ? `${c.column} ${c.operator}`
                    : `${c.column}${c.operator === "like" ? "~" : c.operator === "=" ? "=" : c.operator}${c.value}`;
                  return `${j > 0 ? `${c.logic ?? "OR"} ` : ""}${expr}`;
                })
                .join(" ");
              const group = l.conds.length > 1 ? `(${body})` : body;
              return `${k > 0 ? `${l.logic ?? this.tab.filter_logic ?? "AND"} ` : ""}${group}`;
            })
            .join(" ")}`,
        );
      }
      const rules = this.appliedSortRules();
      if (rules.length) {
        parts.push(`排序: ${rules.map((r, i) => `${i + 1}.${r.column} ${r.ascending ? "↑" : "↓"}`).join("，")}`);
      }
      chip.textContent = parts.join("  ");
      chip.title = "点击修改筛选";
      chip.addEventListener("click", () => void this.app.runCommand("filter"));
      this.footer.appendChild(chip);
    }

    const spacer = document.createElement("div");
    spacer.className = "grid-footer-spacer";
    this.footer.appendChild(spacer);

    // 右侧视图切换（网格 / 表单）
    const gridBtn = svgBtn(ICON_COLUMNS_GRID, "网格视图");
    gridBtn.classList.add("active");
    this.footer.appendChild(gridBtn);
    const formBtn = svgBtn(ICON_VIEW_LIST, "表单视图（当前版本未提供）");
    formBtn.style.opacity = "0.45";
    this.footer.appendChild(formBtn);
  }

  /** 应用单元格编辑（进入待提交状态，不直接写库）。 */
  private applyEdit(row: number, colName: string, value: string | null): void {
    const page = this.tab.page;
    if (!page || !colName) return;
    if (this.pendingDeletes.has(row)) {
      this.app.toast("info", "该行已标记删除，请先提交或回滚再编辑");
      return;
    }
    if (row >= this.originalRowCount()) {
      // 待插入行的编辑
      const idx = row - this.originalRowCount();
      const edits = this.pendingInsertEdits.get(idx) ?? new Map<string, string | null>();
      edits.set(colName, value);
      this.pendingInsertEdits.set(idx, edits);
    } else {
      const ci = page.columns.findIndex((c) => c.name === colName);
      const old = page.rows[row]?.[ci] ?? null;
      const edits = this.pendingUpdates.get(row) ?? new Map<string, string | null>();
      if ((old ?? "") === (value ?? "")) {
        edits.delete(colName);
        if (!edits.size) this.pendingUpdates.delete(row);
      } else {
        edits.set(colName, value);
        this.pendingUpdates.set(row, edits);
      }
    }
    this.applyData();
    this.app.renderStatus();
  }

  /** 追加一条待插入行。 */
  async insertRow(): Promise<void> {
    const page = this.tab.page;
    if (!page) return;
    this.pendingInserts.push(page.columns.map(() => null));
    this.applyData();
    this.app.renderStatus();
  }

  /** 删除选中的行（进入待删除状态）。 */
  async deleteSelectedRows(): Promise<void> {
    const page = this.tab.page;
    if (!page || !this.tab.selectedRows.size) {
      this.app.toast("info", "请先选择要删除的行");
      return;
    }
    const base = this.originalRowCount();
    const rows = [...this.tab.selectedRows].sort((a, b) => b - a);
    // 预检：任何一行有未提交修改或已标记删除，就整体拒绝（避免半途而废的部分应用）。
    for (const r of rows) {
      if (r < base && this.pendingUpdates.has(r)) {
        this.app.toast("info", "选中的行存在未提交的修改，请先提交或回滚");
        return;
      }
      if (r < base && this.pendingDeletes.has(r)) {
        this.app.toast("info", "选中的行已处于待删除状态");
        return;
      }
    }
    for (const r of rows) {
      if (r >= base) {
        // 待插入行：同时移除其编辑缓存，并把更大下标的编辑键整体前移（防错位）
        const idx = r - base;
        this.pendingInserts.splice(idx, 1);
        this.pendingInsertEdits.delete(idx);
        const remapped = new Map<number, Map<string, string | null>>();
        for (const [k, v] of this.pendingInsertEdits) {
          remapped.set(k > idx ? k - 1 : k, v);
        }
        this.pendingInsertEdits = remapped;
      } else {
        this.pendingDeletes.add(r);
      }
      this.tab.selectedRows.delete(r);
    }
    this.applyData();
    this.app.renderStatus();
  }

  /** 提交是否正在进行（防重入：双击/快捷键会重复提交同一批修改）。 */
  private committing = false;

  /** 提交全部待提交修改（删除 / 更新 / 插入）。 */
  async commitEdits(): Promise<void> {
    const page = this.tab.page;
    if (!page) return;
    if (this.committing) return; // 防重入
    if (!this.hasPending()) {
      this.app.toast("info", "没有需要提交的修改");
      return;
    }
    const pk = page.primary_keys;
    if (!pk.length && !page.columns.length) {
      this.app.toast("error", "没有可用的列信息，无法保存修改");
      return;
    }
    this.committing = true;
    let done = 0;
    let failed = false;
    let failMsg = "";
    /** 无主键表的整行旧值定位条件。 */
    const oldRow = (r: number): [string, string | null][] =>
      page.columns.map((c, ci) => [c.name, page.rows[r]?.[ci] ?? null] as [string, string | null]);
    try {
      // 1) 删除（WHERE 用数据库里的旧值；DELETE 未命中才是真错误）
      const delList = [...this.pendingDeletes].sort((a, b) => a - b);
      for (const r of delList) {
        const primaryKeys = pk.length
          ? this.pkValues(page, r, new Map())
          : oldRow(r);
        const affected = await api.deleteRow(
          this.tab.connectionId,
          this.tab.schema,
          this.tab.object,
          primaryKeys,
        );
        if (!affected) {
          throw new Error(`第 ${r + 1} 行删除未命中（数据可能已被外部修改）`);
        }
        this.pendingDeletes.delete(r);
        done++;
      }
      // 2) 更新（WHERE 用旧主键值；UPDATE 命中 0 行 = 值无变化，不算错误）
      for (const [r, edits] of [...this.pendingUpdates.entries()]) {
        const primaryKeys = pk.length
          ? this.pkValues(page, r, new Map())
          : [];
        const oldValues = pk.length ? undefined : oldRow(r);
        const changes: [string, string | null][] = [...edits.entries()];
        await api.updateRow({
          connection_id: this.tab.connectionId,
          schema: this.tab.schema,
          table: this.tab.object,
          primary_keys: primaryKeys,
          changes,
          old_values: oldValues,
        });
        this.pendingUpdates.delete(r);
        done++;
      }
      // 3) 插入（自增主键列未填值时必须省略，否则 PG serial 报 NULL 约束冲突）
      //    始终处理队首：成功即出队（连同其编辑缓存），失败即中断 ——
      //    已入库的行绝不能留在 pending 里，否则重试会重复插入。
      while (this.pendingInserts.length > 0) {
        const i = 0;
        const edits = this.pendingInsertEdits.get(i);
        const cols: string[] = [];
        const values: (string | null)[] = [];
        page.columns.forEach((c, ci) => {
          const raw = edits?.has(c.name)
            ? edits.get(c.name) ?? null
            : this.pendingInserts[i][ci] ?? null;
          if (raw === null && c.auto_increment) return; // 跳过未填的自增列
          cols.push(c.name);
          values.push(raw);
        });
        if (!cols.length) {
          // 整行空白不提交，直接出队
          this.pendingInserts.splice(i, 1);
          this.pendingInsertEdits.delete(i);
          const remapped = new Map<number, Map<string, string | null>>();
          for (const [k, v] of this.pendingInsertEdits) {
            remapped.set(k > i ? k - 1 : k, v);
          }
          this.pendingInsertEdits = remapped;
          continue;
        }
        const affected = await api.insertRow({
          connection_id: this.tab.connectionId,
          schema: this.tab.schema,
          table: this.tab.object,
          columns: cols,
          values,
        });
        if (!affected) {
          throw new Error(`第 ${this.originalRowCount() + i + 1} 行插入失败`);
        }
        this.pendingInserts.splice(i, 1);
        this.pendingInsertEdits.delete(i);
        const remapped = new Map<number, Map<string, string | null>>();
        for (const [k, v] of this.pendingInsertEdits) {
          remapped.set(k > i ? k - 1 : k, v);
        }
        this.pendingInsertEdits = remapped;
        done++;
      }
      this.pendingUpdates.clear();
      this.pendingDeletes.clear();
      this.pendingInserts = [];
      this.pendingInsertEdits.clear();
      this.app.toast("success", `已提交 ${done} 项修改`);
      await this.reload();
    } catch (e) {
      failed = true;
      failMsg = String(e);
      // 已应用的部分已从 pending 移除，未尝试的保留 —— 用户可直接重试或回滚。
      // 注意不能在此 reload：行基数据变化会让剩余 pending 的行号定位失效。
      const remaining =
        this.pendingDeletes.size +
        this.pendingUpdates.size +
        this.pendingInserts.length;
      if (remaining > 0) {
        this.app.toast("error", `${failMsg}（已提交 ${done} 项，剩余 ${remaining} 项未提交，可重试或回滚）`);
      } else {
        this.app.toast("error", `${failMsg}（其余 ${done} 项已提交）`);
        this.pendingUpdates.clear();
        this.pendingDeletes.clear();
        this.pendingInserts = [];
        this.pendingInsertEdits.clear();
        await this.reload();
      }
      this.applyData();
    } finally {
      this.committing = false;
      if (failed) this.app.renderStatus();
    }
  }

  /** 组装某行的主键值（优先取待提交的新值）。 */
  private pkValues(
    page: import("../core/types").PageResult,
    row: number,
    edits: Map<string, string | null>,
  ): [string, string | null][] {
    return page.primary_keys.map((k) => {
      const idx = page.columns.findIndex((c) => c.name === k);
      if (edits.has(k)) return [k, edits.get(k) ?? null] as [string, string | null];
      return [k, idx >= 0 ? page.rows[row][idx] ?? null : null] as [string, string | null];
    });
  }

  /** 回滚全部未提交修改。 */
  rollbackEdits(): void {
    if (this.committing) return; // 提交进行中禁止回滚（避免与提交循环交错）
    if (!this.hasPending()) return;
    this.pendingUpdates.clear();
    this.pendingDeletes.clear();
    this.pendingInserts = [];
    this.pendingInsertEdits.clear();
    this.applyData();
    this.app.renderStatus();
    this.app.toast("info", "已回滚全部未提交修改");
  }

  destroy(): void {
    ++this.requestVersion;
    this.grid.dispose();
  }
}

/** 查询视图（运行条 + 多结果页签 + 消息）。 */
class QueryTabView extends TabView {
  editor: SqlEditor;
  private connSelect: HTMLSelectElement;
  private dbSelect: HTMLSelectElement;
  private runBtn: HTMLButtonElement;
  private stopBtn: HTMLButtonElement;
  private explainBtn: HTMLButtonElement;
  private runbar: HTMLElement;
  private contentWrap: HTMLElement;
  private resultSplitter: HTMLElement;
  private resultTabsEl: HTMLElement;
  private resultBody: HTMLElement;
  /** 结果区高度占内容区比例（拖动范围 1/3 ~ 2/3）。 */
  private resultRatio = 1 / 3;
  private grid: DataGrid;
  private messagesEl: HTMLElement;
  private cellPreviewEl: HTMLElement;
  /** -1 = 消息页；>=0 = 结果 N。 */
  private activeResultTab = 0;
  private running = false;
  private lastMessages: { text: string; error: boolean }[] = [];
  private completionLoaded = false;
  private completionRequest = 0;
  private disposed = false;
  private transactionStatus!: HTMLElement;

  isExecuting(): boolean { return this.running; }
  async prepareForTransaction(): Promise<void> {
    if (this.running || this.disposed) throw new Error("当前查询未就绪");
    await this.refreshCompletionData();
    if (!this.completionLoaded || this.disposed) throw new Error("真实数据库/模式元数据未加载完成，不能开始事务");
  }
  syncManualTransactionControls(): void {
    const status = this.app.manualTransactionFor?.(this.tab.connectionId);
    const pending = this.app.manualTransactionIsBusy?.(this.tab.connectionId) ?? false;
    const ownedElsewhere = !!status && (status.owner !== this.tab.id || status.schema !== this.tab.schema);
    const reason = this.tab.datasourceStale || (pending ? "事务操作进行中，请等待完成" : ownedElsewhere ? "此连接的事务由另一个查询页持有" : status?.failed ? "事务执行失败，必须回滚" : "");
    this.runBtn.disabled = this.running || !!reason;
    this.explainBtn.disabled = this.running || !!reason || !!status;
    this.runBtn.title = reason || "运行：有选区执行选区，否则执行全文";
    this.explainBtn.title = reason || (status ? "手动事务仅支持 SELECT/WITH 和 DML；解释计划请先提交或回滚" : "解释当前选区或全文");
    this.explainBtn.setAttribute("aria-description", this.explainBtn.title);
    this.connSelect.disabled = this.running || pending || !!status;
    this.dbSelect.disabled = this.running || pending || !!status || !!this.tab.datasourceStale;
    const scopeReason = status ? "事务绑定当前连接和模式，请先提交或回滚再切换" : pending ? "事务操作进行中" : "";
    this.connSelect.title = scopeReason || "查询连接";
    this.dbSelect.title = scopeReason || "查询数据库 / 模式";
    if (this.transactionStatus) {
      this.transactionStatus.textContent = pending ? "事务操作进行中…" : manualTransactionStatusText(status, this.tab.id);
      this.transactionStatus.title = status ? "仅所属查询共享真实事务连接；导出/元数据/其它数据页读取只反映已提交状态。DDL和会话控制不在此事务中执行。" : "当前SQL使用自动提交";
    }
  }

  constructor(app: App, tab: TabState) {
    super(app, tab);

    // ---- 运行条：连接 / 数据库 / 运行已选择的 / 停止 / 解释已选择的
    this.runbar = document.createElement("div");
    this.runbar.className = "query-runbar";
    this.connSelect = document.createElement("select");
    this.connSelect.className = "run-select";
    this.connSelect.setAttribute("aria-label", "查询连接");
    for (const c of app.connectionsValue()) {
      const o = document.createElement("option");
      o.value = c.id;
      o.textContent = c.name;
      this.connSelect.appendChild(o);
    }
    this.connSelect.style.width = "150px";
    this.connSelect.value = tab.connectionId;
    this.dbSelect = document.createElement("select");
    this.dbSelect.className = "run-select";
    this.dbSelect.setAttribute("aria-label", "查询数据库 / 模式");
    this.dbSelect.style.width = "170px";
    const mkRunBtn = (icon: string, label: string) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "run-btn";
      const ic = document.createElement("span");
      ic.setAttribute("aria-hidden", "true");
      ic.innerHTML = iconSvgByName(icon);
      ic.style.lineHeight = "0";
      ic.style.display = "inline-flex";
      b.appendChild(ic);
      const t = document.createElement("span");
      t.textContent = label;
      b.appendChild(t);
      this.runbar.appendChild(b);
      return b;
    };
    this.runbar.appendChild(this.connSelect);
    this.runbar.appendChild(this.dbSelect);
    this.runBtn = mkRunBtn("run_sel", "运行已选择的");
    this.runBtn.classList.add("primary");
    this.stopBtn = mkRunBtn("stop_sel", "停止");
    this.stopBtn.classList.add("disabled");
    this.stopBtn.disabled = true;
    this.stopBtn.title = "当前数据库执行接口不支持取消；查询完成前无法停止。";
    this.stopBtn.setAttribute("aria-label", "停止（当前执行接口不支持取消）");
    this.explainBtn = mkRunBtn("explain_sel", "解释已选择的");
    this.explainBtn.addEventListener("click", () => void this.explainSelection());
    this.runBtn.addEventListener("click", () => void this.executeSelection(false));
    this.connSelect.addEventListener("change", () => {
      void this.switchConnection(this.connSelect.value);
    });
    this.dbSelect.addEventListener("change", () => {
      if (this.running || this.app.manualTransactionFor?.(this.tab.connectionId) || this.app.manualTransactionIsBusy?.(this.tab.connectionId)) {
        this.dbSelect.value = this.tab.schema;
        this.app.toast("info", "事务或查询进行中，请先完成再切换数据库 / 模式");
        return;
      }
      this.tab.schema = this.dbSelect.value;
      this.completionLoaded = false;
      void this.loadCompletionData();
    });
    this.transactionStatus = document.createElement("span");
    this.transactionStatus.className = "query-transaction-status";
    this.transactionStatus.setAttribute("role", "status");
    this.transactionStatus.setAttribute("aria-live", "polite");
    this.transactionStatus.setAttribute("aria-label", "查询事务状态");
    this.runbar.appendChild(this.transactionStatus);
    this.syncManualTransactionControls();
    this.el.appendChild(this.runbar);

    // ---- 编辑器
    this.editor = new SqlEditor({
      value: tab.sql,
      autoComplete: app.settingsValue.auto_complete !== false,
      fontFamily: app.settingsValue.sql_font_family,
      fontSize: app.settingsValue.sql_font_size,
      onChange: (v) => {
        this.tab.sql = v;
        this.tab.dirty = true;
      },
      onExecute: (sql, selectionOnly) => {
        void this.execute(sql, selectionOnly);
      },
      onExplain: () => { void this.explainSelection(); },
      identifierQuote: () => {
        const kind = app.connectionById(this.tab.connectionId)?.kind ?? "";
        return /mysql|mariadb|tidb|oceanbase/i.test(kind) ? "`" : /sqlserver|mssql/i.test(kind) ? "[" : '"';
      },
      onFormat: () => { void app.runCommand("beautify-sql"); },
      onBeautify: () => { void app.runCommand("beautify-sql"); },
    });
    // ---- 内容区：编辑器占满剩余空间；结果区按需出现并把编辑器挤上去
    this.contentWrap = document.createElement("div");
    this.contentWrap.style.flex = "1";
    this.contentWrap.style.minHeight = "0";
    this.contentWrap.style.display = "flex";
    this.contentWrap.style.flexDirection = "column";
    this.resultSplitter = document.createElement("div");
    this.resultSplitter.className = "splitter splitter-v";
    this.resultSplitter.style.display = "none";

    this.contentWrap.appendChild(this.editor.el);
    this.contentWrap.appendChild(this.resultSplitter);

    // ---- 结果区：页签（消息 / 结果 N）+ 内容
    this.resultTabsEl = document.createElement("div");
    this.resultTabsEl.className = "result-tabs";
    this.resultTabsEl.style.display = "none";
    this.resultBody = document.createElement("div");
    this.resultBody.style.flex = "0 0 auto";
    this.resultBody.style.height = `${this.resultRatio * 100}%`;
    this.resultBody.style.minHeight = "0";
    this.resultBody.style.display = "flex";
    this.resultBody.style.flexDirection = "column";
    this.grid = new DataGrid({
      onSort: () => {
        this.app.toast("info", "结果集排序请在 SQL 中使用 ORDER BY。");
      },
      onCellFocus: () => {
        this.updateCellPreview();
      },
      onContextMenu: (x, y) => {
        this.app.showContextMenu(GRID_CONTEXT_MENU, x, y);
      },
    });
    this.messagesEl = document.createElement("div");
    this.messagesEl.className = "result-messages";
    this.cellPreviewEl = document.createElement("div");
    this.cellPreviewEl.className = "cell-preview";
    this.resultBody.appendChild(this.grid.el);
    this.resultBody.appendChild(this.messagesEl);
    this.resultBody.appendChild(this.cellPreviewEl);
    this.cellPreviewEl.style.display = "none";
    this.contentWrap.appendChild(this.resultTabsEl);
    this.contentWrap.appendChild(this.resultBody);
    this.el.appendChild(this.contentWrap);
    this.setupResultSplitter();
  }

  /** 结果区分隔条拖动：结果区高度夹在内容区的 1/3 ~ 2/3。监听仅在拖动期间挂载。 */
  private setupResultSplitter(): void {
    let startY = 0;
    let startH = 0;
    const onMove = (e: MouseEvent) => {
      const avail = this.contentWrap.clientHeight;
      if (avail <= 0) return;
      const h = Math.max(Math.round(avail / 3), Math.min(Math.round((avail * 2) / 3), startH + (startY - e.clientY)));
      this.resultBody.style.height = `${h}px`;
      this.resultRatio = h / avail;
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      this.resultSplitter.classList.remove("dragging");
      document.body.style.cursor = "";
    };
    this.resultSplitter.addEventListener("mousedown", (e) => {
      if (this.resultSplitter.style.display === "none") return;
      startY = e.clientY;
      startH = this.resultBody.offsetHeight;
      this.resultSplitter.classList.add("dragging");
      document.body.style.cursor = "row-resize";
      e.preventDefault();
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    });
  }

  /** 单元格内容面板：按「文本▼」格式展示焦点单元格。首次聚焦时才显示面板。 */
  updateCellPreview(): void {
    const cell = this.grid.focusedCell;
    const value = cell ? (this.grid.rows[cell.row]?.[cell.col] ?? null) : null;
    this.cellPreviewEl.style.display = "";
    renderCellPreview(this.cellPreviewEl, value, this.app.textFormatValue);
  }

  /** 状态栏信息。 */
  statusInfo(): { sql: string; pageText: string; elapsed: string } {
    const res = this.tab.results;
    const set = res?.sets[this.tab.activeResult];
    return {
      sql: this.editor.getSelection() || this.editor.getSql(),
      pageText: set ? `第 1 条记录（共 ${set.total} 条）` : "",
      elapsed: res ? `运行时间: ${(res.elapsed_ms / 1000).toFixed(3)}s` : "",
    };
  }

  /** 当前激活的结果集。 */
  currentSet() {
    return this.tab.results?.sets[this.tab.activeResult] ?? null;
  }

  /** 图表只使用实际显示的结果页；不执行查询，也不回退另一个结果集。 */
  chartSource(): ChartSource | null {
    if (this.running || this.activeResultTab < 0) return null;
    const set = this.tab.results?.sets[this.activeResultTab];
    if (!set?.has_rows) return null;
    return {
      title: `${this.tab.object || this.tab.title} · 结果 ${this.activeResultTab + 1}`,
      description: `${this.tab.connectionName || this.tab.connectionId} / ${this.tab.schema} · 当前查询结果快照`,
      columns: set.columns.map(column => column.name), rows: set.rows,
    };
  }

  /** 运行（runBtn / 菜单共用）：有选区跑选区，否则跑全文。 */
  async executeSelection(_selectionOnly: boolean): Promise<void> {
    await this.execute(this.editor.getSelection(), this.editor.hasSelection());
  }

  /** 供 App 菜单调用：解释已选择的。 */
  async explainPublic(): Promise<void> {
    await this.explainSelection();
  }

  /** 解释当前选择的 SQL。 */
  private async explainSelection(): Promise<void> {
    if (this.app.manualTransactionFor?.(this.tab.connectionId)) {
      this.app.toast("info", "手动事务仅支持 SELECT/WITH 和 DML；解释计划请先提交或回滚");
      return;
    }
    const sql = this.editor.getSelection().trim();
    if (!sql) {
      this.app.toast("info", "请先选中要解释的 SQL");
      return;
    }
    await this.execute(`EXPLAIN ${sql}`, false);
  }

  invalidateDatasource(): void {
    this.completionRequest++;
    this.completionLoaded = false;
    this.editor.setCompletionData({});
    this.dbSelect.disabled = true;
    this.connSelect.replaceChildren();
    const placeholder = document.createElement("option"); placeholder.value = ""; placeholder.textContent = "请重新选择连接（不会自动连接）"; this.connSelect.append(placeholder);
    for (const config of this.app.datasourceConnectionChoices()) { const option = document.createElement("option"); option.value = config.id; option.textContent = config.name; this.connSelect.append(option); }
    this.connSelect.value = "";
    let warning = this.el.querySelector<HTMLElement>(".datasource-stale-warning");
    if (!warning) { warning = document.createElement("p"); warning.className = "datasource-stale-warning"; warning.setAttribute("role", "status"); this.el.prepend(warning); }
    warning.textContent = this.tab.datasourceStale ?? "数据源已变化，请重新选择连接";
    this.syncManualTransactionControls();
  }

  activate(): void {
    this.editor.focus();
    if (this.tab.datasourceStale) return;
    this.syncManualTransactionControls();
    if (this.tab.connectionId) void this.app.refreshManualTransactionStatus?.(this.tab.connectionId)?.catch(() => undefined);
    if (!this.completionLoaded && this.tab.connectionId) void this.refreshCompletionData();
    if (this.tab.results || this.lastMessages.length) this.renderResults();
  }

  async reload(): Promise<void> {
    if (this.editor.getSelection().trim()) await this.executeSelection(false);
  }

  /** 切换运行条中的连接：不得沿用旧连接的模式或补全。 */
  private async switchConnection(connId: string): Promise<void> {
    if (this.running || this.app.manualTransactionFor?.(this.tab.connectionId) || this.app.manualTransactionIsBusy?.(this.tab.connectionId)) {
      this.connSelect.value = this.tab.connectionId;
      this.app.toast("info", "事务或查询进行中，请先完成再切换连接");
      return;
    }
    if (!connId || !this.app.connectionById(connId)) return;
    if (this.tab.datasourceStale) {
      delete this.tab.datasourceStale;
      this.el.querySelector(".datasource-stale-warning")?.remove();
      this.dbSelect.disabled = false;
    }
    this.tab.connectionId = connId;
    this.tab.schema = "";
    this.tab.results = null;
    this.renderResults();
    this.completionLoaded = false;
    const cfg = this.app.connectionById(connId);
    if (cfg) this.tab.title = `查询 - ${cfg.name}`;
    await this.refreshCompletionData();
    if (!this.disposed && this.tab.connectionId === connId) this.app.renderTabsPublic();
  }

  /** 连接与数据库选择就绪后再请求补全，避免空模式竞态。 */
  private async refreshCompletionData(): Promise<void> {
    const request = ++this.completionRequest;
    const connId = this.tab.connectionId;
    this.completionLoaded = false;
    this.editor.setCompletionData({});
    if (!connId || this.disposed || this.tab.datasourceStale) return;
    try {
      await api.ensureConnection(connId);
      if (this.disposed || request !== this.completionRequest || connId !== this.tab.connectionId) return;
      await this.refreshDbSelect(request);
      if (this.disposed || request !== this.completionRequest || connId !== this.tab.connectionId) return;
      await this.loadCompletionData(request);
    } catch (e) {
      if (!this.disposed && request === this.completionRequest) this.app.toast("error", `SQL 元数据加载失败：${String(e)}`);
    }
  }

  /** 刷新数据库下拉；迟到的列库结果不能改写当前连接/模式。 */
  private async refreshDbSelect(request = this.completionRequest): Promise<void> {
    const connId = this.tab.connectionId;
    this.dbSelect.innerHTML = "";
    if (!connId) return;
    try {
      const dbs = await api.listDatabases(connId);
      if (this.disposed || request !== this.completionRequest || connId !== this.tab.connectionId) return;
      for (const [name] of dbs) {
        const o = document.createElement("option");
        o.value = name;
        o.textContent = name;
        this.dbSelect.appendChild(o);
      }
      const keep = dbs.some(([n]) => n === this.tab.schema);
      this.tab.schema = keep ? this.tab.schema : dbs[0]?.[0] ?? "";
      this.dbSelect.value = this.tab.schema;
    } catch (e) {
      if (this.disposed || request !== this.completionRequest || connId !== this.tab.connectionId) return;
      this.dbSelect.innerHTML = "";
      this.tab.schema = "";
      throw e;
    }
  }

  /** 载入真实表/视图/字段。过期响应不能覆盖新模式或已销毁编辑器。 */
  private async loadCompletionData(request = ++this.completionRequest): Promise<void> {
    const connId = this.tab.connectionId;
    const schema = this.tab.schema;
    this.completionLoaded = false;
    this.editor.setCompletionData({}, schema);
    if (!connId || this.disposed || this.tab.datasourceStale) return;
    try {
      const data = await api.completionData(connId, schema);
      if (this.disposed || request !== this.completionRequest || connId !== this.tab.connectionId || schema !== this.tab.schema) return;
      this.editor.setCompletionData(data, schema);
      this.completionLoaded = true;
    } catch (e) {
      if (!this.disposed && request === this.completionRequest && connId === this.tab.connectionId && schema === this.tab.schema) {
        this.app.toast("error", `SQL 元数据加载失败：${String(e)}`);
      }
    }
  }

  /** 执行 SQL。 */
  async execute(sql: string, _selectionOnly: boolean): Promise<void> {
    if (this.tab.datasourceStale) { this.app.toast("info", this.tab.datasourceStale); return; }
    const connId = this.tab.connectionId;
    if (!connId) {
      this.app.toast("error", "请先在运行条中选择一个连接");
      return;
    }
    if (!sql.trim()) {
      this.app.toast("error", "SQL 为空");
      return;
    }
    if (this.running) return; // 防重入：双击运行按钮不并发执行两次
    const transaction = this.app.manualTransactionFor?.(connId);
    if (this.app.manualTransactionIsBusy?.(connId) || (transaction && (transaction.owner !== this.tab.id || transaction.schema !== this.tab.schema)) || transaction?.failed) {
      this.app.toast("error", transaction?.failed ? "事务执行失败，必须回滚" : "事务由其它查询页持有或正在操作，不能执行此SQL");
      return;
    }
    this.running = true;
    this.runBtn.disabled = true;
    this.explainBtn.disabled = true;
    this.connSelect.disabled = true;
    this.dbSelect.disabled = true;
    this.runbar.setAttribute("aria-busy", "true");
    this.lastMessages = [{ text: "正在执行...（当前执行接口不支持取消）", error: false }];
    this.renderResults();
    try {
      const res: MultiResult = await api.executeSql(connId, sql, this.tab.schema || undefined, transaction?.active ? this.tab.id : undefined, "query");
      if (this.disposed) return;
      this.tab.results = res;
      this.tab.activeResult = 0;
      this.activeResultTab = res.sets.length ? 0 : -1;
      const affected = res.sets.reduce((a, s) => a + s.affected, 0);
      this.lastMessages = [
        {
          text: `执行完成，用时 ${res.elapsed_ms} 毫秒${affected ? `，${affected} 行受影响` : ""}`,
          error: false,
        },
        ...res.messages.map((m) => ({ text: m, error: false })),
      ];
      this.renderResults();
      this.app.toast(
        "success",
        `执行完成，用时 ${res.elapsed_ms} 毫秒${affected ? `，${affected} 行受影响` : ""}`,
      );
    } catch (e) {
      if (this.disposed) return;
      this.lastMessages = [{ text: String(e), error: true }];
      this.tab.results = null;
      this.activeResultTab = -1;
      this.renderResults();
      this.app.toast("error", String(e));
    } finally {
      await this.app.refreshManualTransactionStatus?.(connId)?.catch(error => this.app.toast("error", `事务状态确认失败：${String(error)}`));
      this.running = false;
      this.syncManualTransactionControls();
      this.app.notifyToolbar?.();
      this.runbar.setAttribute("aria-busy", "false");
    }
  }

  /** 渲染结果页签与内容。无输出时结果区整体隐藏，编辑器占满内容区。 */
  private renderResults(): void {
    const res = this.tab.results;
    this.resultTabsEl.innerHTML = "";
    const hasMessages = this.lastMessages.length > 0;
    const setCount = res?.sets.length ?? 0;
    const hasOutput = this.running || hasMessages || setCount > 0;
    if (!hasOutput) {
      this.resultSplitter.style.display = "none";
      this.resultBody.style.display = "none";
      this.resultTabsEl.style.display = "none";
      this.grid.setData({ columns: [], rows: [] });
      this.messagesEl.style.display = "none";
      this.grid.el.style.display = "";
      return;
    }
    this.resultSplitter.style.display = "";
    this.resultBody.style.display = "flex";
    this.resultBody.style.height = `${this.resultRatio * 100}%`;
    this.resultTabsEl.style.display = "";
    const mkTab = (label: string, index: number) => {
      const t = document.createElement("div");
      t.className = "result-tab" + (index === this.activeResultTab ? " active" : "");
      t.textContent = label;
      t.addEventListener("click", () => {
        this.activeResultTab = index;
        this.renderResults();
      });
      this.resultTabsEl.appendChild(t);
    };
    if (hasMessages) mkTab("消息", -1);
    res?.sets.forEach((_s, i) => mkTab(`结果 ${i + 1}`, i));
    // 内容切换
    const showGrid = this.activeResultTab >= 0 && !!res?.sets[this.activeResultTab];
    this.grid.el.style.display = showGrid ? "" : "none";
    this.messagesEl.style.display = showGrid ? "none" : "";
    if (showGrid && res) {
      const set = res.sets[this.activeResultTab];
      this.grid.setData({
        columns: set.columns,
        rows: set.rows,
        readOnly: true,
        skipNull: this.app.skipNullState(),
        lined: this.app.showGridLinesState(),
      });
    } else {
      this.renderMessages();
    }
  }

  /** 渲染消息页内容。 */
  private renderMessages(): void {
    this.messagesEl.innerHTML = "";
    for (const m of this.lastMessages) {
      const line = document.createElement("div");
      line.className = m.error ? "msg-error" : "msg-ok";
      line.textContent = m.text;
      this.messagesEl.appendChild(line);
    }
  }

  destroy(): void {
    this.disposed = true;
    ++this.completionRequest;
    this.editor.destroy();
    this.grid.dispose();
  }
}

/** 表结构视图（可编辑 + 保存 DDL）。 */
/** 备份视图（参考截图 14：网格 名/修改日期/文件大小/注释，数据来自真实备份目录）。 */
class BackupTabView extends TabView {
  grid: DataGrid;
  private backups: import("./types").BackupInfo[] = [];

  constructor(app: App, tab: TabState) {
    super(app, tab);
    this.grid = new DataGrid({
      onSelectionChange: (rows) => {
        this.tab.selectedRows = new Set(rows);
        app.notifyToolbar();
        const backup = this.backups[rows[0]];
        const cfg = app.connectionById(tab.connectionId);
        if (cfg) app.showObjectDetails(cfg, tab.schema, backup?.name ?? "", "folder", { category: "backup", backupName: backup?.name });
      },
      onRowDoubleClick: () => {
        // 双击备份 = 还原
        void app.runCommand("restore-backup");
      },
      onContextMenu: (x, y) => {
        app.showContextMenu(BACKUP_CONTEXT_MENU, x, y);
      },
    });
    this.el.appendChild(this.grid.el);
    // 网格页脚：左统计（设计图「3 个备份 · 共 3.54 MB」）
    this.footer = document.createElement("div");
    this.footer.className = "grid-footer";
    this.el.appendChild(this.footer);
  }

  private footer: HTMLElement;

  /** 选中行的备份信息。 */
  selectedBackup(): import("./types").BackupInfo | null {
    const r = [...this.tab.selectedRows][0];
    if (r === undefined) return null;
    return this.backups[r] ?? null;
  }

  activate(): void {
    void this.reload();
  }

  async reload(): Promise<void> {
    try {
      this.backups = await api.listBackups(this.tab.connectionId, this.tab.schema);
    } catch (e) {
      this.backups = [];
      this.app.toast("error", `读取备份列表失败：${String(e)}`);
    }
    this.grid.setData({
      columns: [
        { name: "名", type: "text", nullable: false, primary_key: false, auto_increment: false },
        { name: "修改日期", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "文件大小", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "注释", type: "text", nullable: true, primary_key: false, auto_increment: false },
      ],
      rows: this.backups.map((b) => [b.name, b.modified, formatSize(b.size), b.comment]),
      rowIcons: this.backups.map(() => "dbn_backup"),
      rightAlign: ["文件大小"],
      showRowNumbers: false,
      readOnly: true,
    });
    const totalSize = this.backups.reduce((sum, b) => sum + (Number(b.size) || 0), 0);
    this.footer.textContent = this.backups.length
      ? `${this.backups.length} 个备份 · 共 ${formatSize(totalSize)}`
      : "0 个备份";
    this.tab.selectedRows = new Set();
    this.app.renderStatus();
  }

  destroy(): void {
    this.grid.dispose();
  }
}

/**
 * SQL 对象设计器（视图 / 函数 / 存储过程共用）：SQL 编辑器 + 保存重建。
 * mode=view 保存走 CREATE OR REPLACE（SQLite 为 DROP+CREATE），
 * mode=routine 保存整体重建（单语句执行，支持 BEGIN..END 体）。
 */
class SqlDesignTabView extends TabView {
  private editor: SqlEditor;
  private mode: "view" | "routine";

  constructor(app: App, tab: TabState, mode: "view" | "routine") {
    super(app, tab);
    this.mode = mode;
    this.editor = new SqlEditor({ value: tab.sql || "" });
    this.el.appendChild(this.editor.el);
  }

  activate(): void {
    this.editor.setSql(this.tab.sql || "");
    this.editor.focus();
    this.app.renderStatus();
  }

  getSql(): string {
    return this.editor.getSql();
  }

  async reload(): Promise<void> {
    try {
      if (this.mode === "view") {
        this.tab.sql = await api.getViewDefinition(
          this.tab.connectionId,
          this.tab.schema,
          this.tab.object,
        );
      } else {
        this.tab.sql = await api.getRoutineDefinition(
          this.tab.connectionId,
          this.tab.schema,
          this.tab.object,
          this.tab.objectType === "procedure" ? "procedure" : "function",
        );
      }
      this.editor.setSql(this.tab.sql || "");
    } catch (e) {
      this.app.toast("error", `读取定义失败：${String(e)}`);
    }
  }

  destroy(): void {
    this.editor.destroy();
  }
}

/** 模型视图：活库表 / 字段 / 外键的 ER 关系图。 */
class ModelTabView extends TabView {
  private canvas: ModelCanvas;

  constructor(app: App, tab: TabState) {
    super(app, tab);
    this.canvas = new ModelCanvas();
    this.el.appendChild(this.canvas.el);
  }

  activate(): void {
    void this.reload();
    // 容器尺寸就绪后重绘（部分环境 ResizeObserver 不可靠，双保险）
    window.requestAnimationFrame(() => this.canvas.fit());
    window.setTimeout(() => this.canvas.fit(), 300);
  }

  async reload(): Promise<void> {
    try {
      const tables = await api.listTables(this.tab.connectionId, this.tab.schema);
      const erTables: ErTable[] = [];
      for (const [name, type] of tables) {
        if (type !== "table") continue;
        const s = await api.getTableSchema(this.tab.connectionId, this.tab.schema, name);
        erTables.push({
          name,
          columns: s.columns.map((c) => ({
            name: c.name,
            type: c.type,
            primary_key: c.primary_key,
          })),
          foreign_keys: s.foreign_keys.map((f) => ({
            name: f.name,
            columns: f.columns,
            ref_table: f.ref_table,
            ref_columns: f.ref_columns,
          })),
        });
      }
      this.canvas.setTables(erTables);
    } catch (e) {
      this.app.toast("error", `读取模型数据失败：${String(e)}`);
    }
  }

  fit(): void {
    this.canvas.fit();
  }

  zoomIn(): void {
    this.canvas.setScale(this.canvas.getScale() * 1.2);
  }

  zoomOut(): void {
    this.canvas.setScale(this.canvas.getScale() / 1.2);
  }

  destroy(): void {
    this.canvas.dispose();
  }
}

/** 自动运行视图：真实任务列表 + 执行 + 历史。 */
class ScheduleTabView extends TabView {
  grid: DataGrid;
  private jobs: import("./types").ScheduleJob[] = [];

  constructor(app: App, tab: TabState) {
    super(app, tab);
    this.grid = new DataGrid({
      onSelectionChange: (rows) => {
        this.tab.selectedRows = new Set(rows);
        app.notifyToolbar();
      },
      onRowDoubleClick: () => {
        void app.runCommand("run-job");
      },
    });
    this.el.appendChild(this.grid.el);
  }

  activate(): void {
    void this.reload();
  }

  async reload(): Promise<void> {
    try {
      this.jobs = await api.listJobs();
    } catch (e) {
      this.jobs = [];
      this.app.toast("error", `读取任务失败：${String(e)}`);
    }
const connName = (id: string) => this.app.connectionName(id);
    this.grid.setData({
      columns: [
        { name: "任务名", type: "text", nullable: false, primary_key: false, auto_increment: false },
        { name: "连接", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "数据库", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "间隔", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "启用", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "上次运行", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "状态", type: "text", nullable: true, primary_key: false, auto_increment: false },
      ],
      rows: this.jobs.map((j) => [
        j.name,
        connName(j.connection_id),
        j.database || "-",
        j.interval_secs > 0 ? formatInterval(j.interval_secs) : "手动",
        j.enabled ? "是" : "否",
        j.last_run ?? "-",
        j.last_status ?? "-",
      ]),
      rowIcons: this.jobs.map(() => "info_pane_query"),
      showRowNumbers: false,
      readOnly: true,
    });
    this.tab.selectedRows = new Set();
    this.app.renderStatus();
  }

  destroy(): void {
    this.grid.dispose();
  }
}

/** 间隔秒数的人类可读形式。 */
function formatInterval(secs: number): string {
  if (secs % 86400 === 0) return `每 ${secs / 86400} 天`;
  if (secs % 3600 === 0) return `每 ${secs / 3600} 小时`;
  if (secs % 60 === 0) return `每 ${secs / 60} 分钟`;
  return `每 ${secs} 秒`;
}

/** 文件大小格式化（B / KB / MB）。 */
function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(2)} MB`;
}

/** 备份网格右键菜单。 */
const BACKUP_CONTEXT_MENU: MenuEntry[] = [
  { id: "restore-backup", label: "还原备份..." },
  { id: "open-backup-folder", label: "打开所在文件夹..." },
  { separator: true },
  { id: "delete-backup", label: "删除备份" },
  { separator: true },
  { id: "refresh", label: "刷新", shortcut: "F5" },
];

/** 用户视图（参考截图 8；MySQL 协议族读 mysql.user 真实数据，可建户 / 删户 / 改密）。 */
class UsersTabView extends TabView {
  grid: DataGrid;
  /** 空态 / 错误只提示一次，避免每次激活都弹。 */
  private warned = false;

  constructor(app: App, tab: TabState) {
    super(app, tab);
    this.grid = new DataGrid({
      onSelectionChange: (rows) => {
        this.tab.selectedRows = new Set(rows);
        app.notifyToolbar();
      },
      onRowDoubleClick: () => {
        app.runCommand("edit-user");
      },
    });
    this.el.appendChild(this.grid.el);
  }

  activate(): void {
    void this.reload();
  }

  async reload(): Promise<void> {
    let rows: string[][] = [];
    try {
      rows = await api.listDbUsers(this.tab.connectionId);
    } catch (e) {
      rows = [];
      if (!this.warned) {
        this.warned = true;
        this.app.toast("error", `读取用户列表失败：${String(e)}`);
      }
    }
    if (!rows.length && !this.warned) {
      this.warned = true;
      this.app.toast(
        "info",
        "没有用户数据：仅 MySQL 协议族支持用户管理，或当前账号无权限读取 mysql.user",
      );
    }
    this.grid.setData({
      columns: [
        { name: "名", type: "text", nullable: false, primary_key: false, auto_increment: false },
        { name: "SSL 类型", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "每小时查询数", type: "number", nullable: true, primary_key: false, auto_increment: false },
        { name: "每小时更新数", type: "number", nullable: true, primary_key: false, auto_increment: false },
        { name: "每小时连接数", type: "number", nullable: true, primary_key: false, auto_increment: false },
        { name: "最大连接数", type: "number", nullable: true, primary_key: false, auto_increment: false },
        { name: "超级用户", type: "text", nullable: true, primary_key: false, auto_increment: false },
      ],
      rows,
      rightAlign: ["每小时查询数", "每小时更新数", "每小时连接数", "最大连接数"],
      rowIcons: rows.map(() => "info_pane_user"),
      showRowNumbers: false,
      readOnly: true,
    });
    this.app.renderStatus();
  }

  destroy(): void {
    this.grid.dispose();
  }
}

class StructureTabView extends TabView {
  private currentTab = "字段";
  /** 字段编辑副本。 */
  private fields: {
    name: string;
    type: string;
    length: string;
    scale: string;
    nullable: boolean;
    pk: boolean;
    isAutoInc: boolean;
    unsigned: boolean;
    zerofill: boolean;
    comment: string;
    isNew: boolean;
    isDeleted: boolean;
    origName: string;
    origType: string;
    origNullable: boolean;
    origDefault: string | null;
    origBase: string;
    origLen: string;
    origScale: string;
    origUnsigned: boolean;
    origZerofill: boolean;
    origComment: string;
  }[] = [];
  private origPk: string[] = [];
  private originalOrder: string[] = [];
  private originalFieldDrafts = new Map<string, string>();
  private originalTableDraft = "";
  private originalColumnMetadata = "";
  private originalColumnDefinitions = "";
  private savingStructure = false;
  private fieldOrderStatus = "";
  private selectedField = -1;
  private tableMeta: {
    engine: string | null;
    comment: string | null;
    collation: string | null;
  } = { engine: null, comment: null, collation: null };
  private host: HTMLElement;

  constructor(app: App, tab: TabState) {
    super(app, tab);
    this.host = document.createElement("div");
    this.host.className = "designer-body";
    this.el.appendChild(this.host);
  }

  activate(): void {
    void this.reload();
  }

  async reload(): Promise<void> {
    try {
      const s = await api.getTableSchema(
        this.tab.connectionId,
        this.tab.schema,
        this.tab.object,
      );
      this.tab.schemaInfo = s;
      if (!this.fields.length) {
        this.fields = s.columns.map((c) => ({
          name: c.name,
          type: c.type.split("(")[0] || c.type,
          length:
            c.length != null ? String(c.length) : (c.type.match(/\((\d+)/)?.[1] ?? ""),
          scale: c.scale != null ? String(c.scale) : (c.type.match(/,(\d+)\)/)?.[1] ?? ""),
          nullable: c.nullable,
          pk: c.primary_key,
          isAutoInc: c.auto_increment,
          unsigned: /unsigned/i.test(c.type),
          zerofill: /zerofill/i.test(c.type),
          comment: c.comment ?? "",
          isNew: false,
          isDeleted: false,
          origName: c.name,
          origType: c.type,
          origNullable: c.nullable,
          origDefault: c.default,
          origBase: c.type.split("(")[0] || c.type,
          origLen: c.length != null ? String(c.length) : (c.type.match(/\((\d+)/)?.[1] ?? ""),
          origScale: c.scale != null ? String(c.scale) : (c.type.match(/,(\d+)\)/)?.[1] ?? ""),
          origUnsigned: /unsigned/i.test(c.type),
          origZerofill: /zerofill/i.test(c.type),
          origComment: c.comment ?? "",
        }));
        this.originalOrder = s.columns.map(c => c.name);
        this.originalFieldDrafts = new Map(this.fields.map(f => [f.origName, JSON.stringify(f)]));
        this.originalTableDraft = JSON.stringify({ engine: s.engine, comment: s.comment, collation: s.collation });
        this.originalColumnMetadata = JSON.stringify(s.columns);
        this.originalColumnDefinitions = "";
        const cfg = this.app.connectionById(this.tab.connectionId);
        if (cfg && !columnOrderCapability(cfg.kind, cfg.params, cfg.read_only)) {
          try {
            const ddl = await api.getTableDdl(this.tab.connectionId, this.tab.schema || s.database, this.tab.object);
            this.originalColumnDefinitions = JSON.stringify([...mysqlColumnDefinitions(ddl)]);
          } catch (error) {
            this.app.toast("error", `无法读取完整字段定义，字段排序已禁用：${String(error)}`);
          }
        }
      }
      this.origPk = s.columns.filter((c) => c.primary_key).map((c) => c.name);
      this.tableMeta = { engine: s.engine, comment: s.comment, collation: s.collation };
      this.render();
      // 结构异步加载完成后刷新状态栏，避免「字段数: 0」短暂滞留
      this.app.renderStatus();
      this.app.notifyToolbar();
    } catch (e) {
      this.app.toast("error", String(e));
    }
  }

  /** 顶部选项卡：字段 / 索引 / 外键 / 检查 / 触发器 / 选项 / 注释 / SQL 预览。 */
  private render(): void {
    const s = this.tab.schemaInfo;
    this.el.innerHTML = "";
    if (!s) return;

    const tabs = document.createElement("div");
    tabs.className = "designer-tabs";
    const names = ["字段", "索引", "外键", "检查", "触发器", "选项", "注释", "SQL 预览"];
    const body = document.createElement("div");
    body.className = "designer-body";
    const footer = document.createElement("div");
    footer.className = "designer-footer";
    footer.addEventListener("change", () => { this.tab.dirty = true; this.app.notifyToolbar(); });

    const show = (name: string) => {
      this.currentTab = name;
      body.innerHTML = "";
      if (name === "字段") this.renderFields(body);
      else if (name === "索引") {
        this.appendSimpleTable(body, ["名称", "列", "唯一", "主键"],
          s.indexes.map((i) => [i.name, i.columns.join(", "), i.unique ? "是" : "", i.primary ? "是" : ""]));
      } else if (name === "外键") {
        this.appendSimpleTable(body, ["名称", "列", "引用表", "引用列"],
          s.foreign_keys.map((f) => [f.name, f.columns.join(", "), f.ref_table, f.ref_columns.join(", ")]));
      } else if (name === "SQL 预览") {
        this.renderSqlPreview(body, s);
      } else if (name === "选项") {
        this.appendSimpleTable(body, ["选项", "值"], [
          ["引擎", this.tableMeta.engine ?? ""],
          ["行格式", this.tableMeta.collation ?? ""],
          ["注释", this.tableMeta.comment ?? "--"],
        ]);
      } else if (name === "注释") {
        const ta = document.createElement("textarea");
        ta.value = this.tableMeta.comment ?? "";
        ta.style.width = "100%";
        ta.style.height = "120px";
        ta.addEventListener("change", () => {
          this.tableMeta.comment = ta.value || null;
          this.tab.dirty = true;
          this.app.notifyToolbar();
        });
        body.appendChild(ta);
      } else {
        const note = document.createElement("div");
        note.style.color = "var(--fg-muted)";
        note.style.padding = "10px";
        note.textContent = `此表没有${name}。`;
        body.appendChild(note);
      }
      this.renderFooter(footer);
      this.app.notifyToolbar();
    };

    for (const name of names) {
      const t = document.createElement("div");
      t.className = "designer-tab" + (name === this.currentTab ? " active" : "");
      t.textContent = name;
      t.addEventListener("click", () => {
        if (this.savingStructure) return;
        tabs.querySelectorAll(".designer-tab").forEach((x) => x.classList.remove("active"));
        t.classList.add("active");
        show(name);
      });
      tabs.appendChild(t);
    }

    this.el.appendChild(tabs);
    this.el.appendChild(body);
    this.el.appendChild(footer);
    show(this.currentTab);
  }

  /** 字段页（可编辑：名称 / 类型 / 长度 / 小数点 / 不是 null / 键 / 注释）。 */
  private renderFields(body: HTMLElement): void {
    const table = document.createElement("table");
    table.className = "mini-table designer-grid";
    // 参考截图 6 实测列宽（物理 ÷1.25）
    const colg = document.createElement("colgroup");
    for (const w of [257.6, 148.8, 60, 60, 60, 60, 60, 236.8]) {
      const col = document.createElement("col");
      col.style.width = `${w}px`;
      colg.appendChild(col);
    }
    table.appendChild(colg);
    const thead = document.createElement("thead");
    const tr = document.createElement("tr");
    for (const h of ["名", "类型", "长度", "小数点", "不是 null", "虚拟", "键", "注释"]) {
      const th = document.createElement("th");
      th.textContent = h;
      tr.appendChild(th);
    }
    thead.appendChild(tr);
    table.appendChild(thead);

    const tbody = document.createElement("tbody");
    this.fields.forEach((f, i) => {
      const r = document.createElement("tr");
      if (f.isDeleted) r.style.textDecoration = "line-through";
      if (f.isNew) r.style.background = "var(--ok-50)";
      if (i === this.selectedField) r.style.background = "var(--bg-selected-soft)";
      const selectRow = () => {
        if (this.savingStructure || this.selectedField === i) return;
        this.selectedField = i;
        tbody.querySelectorAll<HTMLTableRowElement>("tr").forEach((row, rowIndex) => {
          row.style.background = rowIndex === i ? "var(--bg-selected-soft)" : this.fields[rowIndex]?.isNew ? "var(--ok-50)" : "";
        });
        const footer = this.el.querySelector<HTMLElement>(".designer-footer");
        if (footer) this.renderFooter(footer);
        this.app.notifyToolbar();
      };
      r.addEventListener("mousedown", selectRow);
      r.addEventListener("focusin", selectRow);

      const mkCellText = (value: string, onChange: (v: string) => void, label: string) => {
        const td = document.createElement("td");
        const input = document.createElement("input");
        input.type = "text";
        input.value = value;
        input.setAttribute("aria-label", `${f.name || "新字段"}：${label}`);
        input.style.width = "95%";
        input.style.height = "16px";
        input.style.border = "1px solid transparent";
        input.style.background = "transparent";
        input.addEventListener("focus", () => {
          input.style.border = "1px solid var(--focus-blue)";
        });
        input.addEventListener("blur", () => {
          input.style.border = "1px solid transparent";
          onChange(input.value);
          if (input.value !== value) this.tab.dirty = true;
          this.app.notifyToolbar();
        });
        td.appendChild(input);
        return td;
      };

      r.appendChild(mkCellText(f.name, (v) => (this.fields[i].name = v.trim()), "名称"));
      r.appendChild(mkCellText(f.type, (v) => (this.fields[i].type = v.trim()), "类型"));
      r.appendChild(mkCellText(f.length, (v) => (this.fields[i].length = v.trim()), "长度"));
      r.appendChild(mkCellText(f.scale, (v) => (this.fields[i].scale = v.trim()), "小数点"));

      const nullTd = document.createElement("td");
      nullTd.style.textAlign = "center";
      const nullBox = document.createElement("input");
      nullBox.type = "checkbox";
      nullBox.checked = !f.nullable;
      nullBox.setAttribute("aria-label", `${f.name || "新字段"}：不是 null`);
      nullBox.addEventListener("change", () => {
        this.fields[i].nullable = !nullBox.checked;
        this.tab.dirty = true;
        this.app.notifyToolbar();
      });
      nullTd.appendChild(nullBox);
      r.appendChild(nullTd);

      const virtTd = document.createElement("td");
      virtTd.style.textAlign = "center";
      const virtBox = document.createElement("input");
      virtBox.type = "checkbox";
      virtBox.setAttribute("aria-label", `${f.name || "新字段"}：虚拟字段`);
      virtBox.disabled = true;
      virtTd.appendChild(virtBox);
      r.appendChild(virtTd);

      const keyTd = document.createElement("td");
      keyTd.style.textAlign = "center";
      keyTd.style.cursor = "default";
      if (f.pk) {
        keyTd.innerHTML =
          `<span style="line-height:0;display:inline-flex">${svg(ICON_KEY_GOLD, 13)}</span> 1`;
        keyTd.title = "点击取消主键";
      } else {
        keyTd.title = "点击设为主键";
      }
      keyTd.addEventListener("click", () => {
        if (this.savingStructure) return;
        this.fields[i].pk = !this.fields[i].pk;
        this.tab.dirty = true;
        this.render();
      });
      r.appendChild(keyTd);

      r.appendChild(mkCellText(f.comment, (v) => (this.fields[i].comment = v), "注释"));
      tbody.appendChild(r);
    });
    table.appendChild(tbody);
    body.appendChild(table);
  }

  private renderSqlPreview(body: HTMLElement, s: import("../core/types").TableSchema): void {
    const full = s.database ? `${s.database}.${s.table}` : s.table;
    const lines: string[] = [`CREATE TABLE ${full} (`];
    const cols = s.columns.map((c) => {
      let seg = `  ${c.name} ${c.type}`;
      if (!c.nullable) seg += " NOT NULL";
      if (c.default) seg += ` DEFAULT ${c.default}`;
      return seg;
    });
    const pks = s.columns.filter((c) => c.primary_key).map((c) => c.name);
    if (pks.length) cols.push(`  PRIMARY KEY (${pks.join(", ")})`);
    lines.push(cols.join(",\n"));
    lines.push(");");
    const ta = document.createElement("textarea");
    ta.value = lines.join("\n");
    ta.style.width = "100%";
    ta.style.height = "220px";
    body.appendChild(ta);
  }

  /** 底部：默认值 + 自动递增 / 无符号 / 填充零（作用于选中字段）。 */
  private renderFooter(footer: HTMLElement): void {
    footer.innerHTML = "";
    if (this.currentTab !== "字段") return;
    // 保存/删除后字段列表可能变短，钳制选中索引避免越界读取
    if (this.selectedField >= this.fields.length) {
      this.selectedField = this.fields.length - 1;
    }
    const row = document.createElement("div");
    row.style.display = "flex";
    row.style.alignItems = "flex-start";
    row.style.gap = "18px";

    const left = document.createElement("div");
    const label = document.createElement("div");
    label.className = "designer-footer-label";
    label.textContent = "默认:";
    left.appendChild(label);
    const combo = document.createElement("select");
    combo.style.width = "200px";
    combo.setAttribute("aria-label", "选中字段的默认值");
    const sel = this.selectedField;
    const cur = sel >= 0 ? this.fields[sel]?.origDefault ?? "" : "";
    const opts = ["", "NULL", "CURRENT_TIMESTAMP", "0", "''"];
    if (cur && !opts.includes(cur)) opts.unshift(cur);
    for (const o of opts) {
      const op = document.createElement("option");
      op.value = o;
      op.textContent = o === "" ? "（无默认值）" : o;
      combo.appendChild(op);
    }
    combo.value = cur;
    combo.addEventListener("change", () => {
      if (sel >= 0) this.fields[sel].origDefault = combo.value || null;
    });
    left.appendChild(combo);
    row.appendChild(left);

    const boxes = document.createElement("div");
    boxes.className = "designer-checkboxes";
    const mkBox = (text: string, get: () => boolean, set: (v: boolean) => void) => {
      const lab = document.createElement("label");
      lab.className = "checkbox";
      const input = document.createElement("input");
      input.type = "checkbox";
      input.checked = get();
      input.addEventListener("change", () => set(input.checked));
      const span = document.createElement("span");
      span.textContent = text;
      lab.appendChild(input);
      lab.appendChild(span);
      boxes.appendChild(lab);
    };
    mkBox("自动递增", () => sel >= 0 && this.fields[sel].isAutoInc, (v) => {
      if (sel >= 0) this.fields[sel].isAutoInc = v;
    });
    mkBox("无符号", () => sel >= 0 && this.fields[sel].unsigned, (v) => {
      if (sel >= 0) this.fields[sel].unsigned = v;
    });
    mkBox("填充零", () => sel >= 0 && this.fields[sel].zerofill, (v) => {
      if (sel >= 0) this.fields[sel].zerofill = v;
    });
    row.appendChild(boxes);

    footer.appendChild(row);
    const statusText = this.fieldOrderStatus || this.fieldOrderReason();
    if (statusText) {
      const status = document.createElement("div");
      status.className = "field-order-status";
      status.setAttribute("role", "status");
      status.setAttribute("aria-live", "polite");
      status.style.padding = "6px 0";
      status.style.color = "var(--fg-muted)";
      status.textContent = statusText;
      footer.appendChild(status);
    }
  }

  /** 添加字段（末尾追加空行）。 */
  addField(): void {
    if (this.savingStructure) return;
    this.fields.push(this.blankField());
    this.selectedField = this.fields.length - 1;
    this.render();
  }

  /** 在选中行上方插入字段。 */
  insertField(): void {
    if (this.savingStructure) return;
    const at = this.selectedField >= 0 ? this.selectedField : this.fields.length;
    this.fields.splice(at, 0, this.blankField());
    this.selectedField = at;
    this.render();
  }

  /** 删除选中字段。 */
  deleteField(): void {
    if (this.savingStructure) return;
    if (this.selectedField < 0 || this.selectedField >= this.fields.length) {
      this.app.toast("info", "请先选中一个字段");
      return;
    }
    const f = this.fields[this.selectedField];
    if (f.isNew) {
      this.fields.splice(this.selectedField, 1);
    } else {
      f.isDeleted = !f.isDeleted;
    }
    this.render();
  }

  /** 切换选中字段的主键标记。 */
  togglePrimaryKey(): void {
    if (this.savingStructure) return;
    if (this.selectedField < 0 || this.selectedField >= this.fields.length) {
      this.app.toast("info", "请先选中一个字段");
      return;
    }
    const f = this.fields[this.selectedField];
    f.pk = !f.pk;
    this.render();
  }

  private blankField() {
    return {
      name: "",
      type: "varchar",
      length: "100",
      scale: "",
      nullable: true,
      pk: false,
      isAutoInc: false,
      unsigned: false,
      zerofill: false,
      comment: "",
      isNew: true,
      isDeleted: false,
      origName: "",
      origType: "",
      origNullable: true,
      origDefault: null,
      origBase: "",
      origLen: "",
      origScale: "",
      origUnsigned: false,
      origZerofill: false,
      origComment: "",
    };
  }

  /** 排序只允许原地安全 DDL，不能夹带字段定义或约束修改。 */
  private fieldOrderReason(): string | null {
    const cfg = this.app.connectionById(this.tab.connectionId);
    if (!cfg) return "连接不存在";
    const capability = columnOrderCapability(cfg.kind, cfg.params, cfg.read_only);
    if (capability) return capability;
    if (this.savingStructure) return "正在保存表结构，请稍候";
    if (!this.tab.schemaInfo || !this.originalOrder.length) return "请先等待字段结构加载完成";
    if (!this.originalColumnDefinitions) return "完整字段定义尚未加载，不能安全调整字段顺序";
    if (!(this.tab.schema || this.tab.schemaInfo.database)) return "请先明确当前数据库，不能在未知命名空间调整字段顺序";
    if (this.fields.length !== this.originalOrder.length || this.fields.some(f =>
      f.isNew || f.isDeleted || this.originalFieldDrafts.get(f.origName) !== JSON.stringify(f)) ||
      this.originalTableDraft !== JSON.stringify(this.tableMeta)) {
      return "字段排序不能与新增、删除、重命名或其它结构修改混合；请先保存其它修改再重新打开结构";
    }
    return null;
  }

  private fieldOrderChanged(): boolean {
    const existing = this.fields.filter(f => !f.isNew).map(f => f.origName);
    return existing.length === this.originalOrder.length && existing.some((name, i) => name !== this.originalOrder[i]);
  }

  isSavingStructure(): boolean { return this.savingStructure; }

  fieldMoveReason(direction: -1 | 1): string | null {
    const reason = this.fieldOrderReason();
    if (reason) return reason;
    if (this.currentTab !== "字段") return "请切换到字段页后选择字段";
    if (this.selectedField < 0 || this.selectedField >= this.fields.length) return "请先选择要移动的字段";
    const target = this.selectedField + direction;
    return target < 0 ? "已是第一个字段" : target >= this.fields.length ? "已是最后一个字段" : null;
  }

  /** 工具栏点击前提交当前输入，避免重建 DOM 丢失未失焦文字。 */
  private flushFieldInput(): void {
    const active = document.activeElement;
    if (active instanceof HTMLElement && this.el.contains(active)) active.blur();
  }

  moveField(direction: -1 | 1): void {
    this.flushFieldInput();
    const reason = this.fieldMoveReason(direction);
    if (reason) { this.app.toast("info", reason); this.app.notifyToolbar(); return; }
    const from = this.selectedField, to = from + direction;
    [this.fields[from], this.fields[to]] = [this.fields[to], this.fields[from]];
    this.selectedField = to;
    this.tab.dirty = this.fieldOrderChanged();
    this.fieldOrderStatus = this.tab.dirty ? "字段顺序为待保存草稿；点击保存后才会调整数据库的物理字段顺序。" : "";
    this.render();
    this.app.notifyToolbar();
    this.el.querySelectorAll<HTMLTableRowElement>(".designer-grid tbody tr")[to]?.querySelector<HTMLInputElement>("input[type=text]")?.focus();
  }

  /** 复用 SHOW CREATE 的完整列定义，一次 ALTER 保存，再读取真实顺序确认。 */
  private async saveFieldOrder(): Promise<void> {
    const reason = this.fieldOrderReason();
    if (reason) { this.fieldOrderStatus = reason; this.app.toast("error", reason); this.render(); return; }
    const connectionId = this.tab.connectionId, schema = this.tab.schema || this.tab.schemaInfo?.database || "", table = this.tab.object;
    const initialTabSchema = this.tab.schema;
    const desired = this.fields.map(f => f.origName);
    this.savingStructure = true;
    this.fieldOrderStatus = "正在保存字段顺序并核对数据库实际结果…";
    this.el.setAttribute("aria-busy", "true");
    const status = this.el.querySelector<HTMLElement>(".field-order-status");
    if (status) status.textContent = this.fieldOrderStatus;
    this.app.notifyToolbar();
    this.el.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>("input, select, textarea")
      .forEach(input => { input.disabled = true; });
    let submitted = false;
    try {
      const freshSchema = await api.getTableSchema(connectionId, schema, table);
      if (JSON.stringify(freshSchema.columns) !== this.originalColumnMetadata) {
        throw new Error("数据库字段定义已发生变化，请重新打开表结构后再调整");
      }
      const ddl = await api.getTableDdl(connectionId, schema, table);
      if (JSON.stringify([...mysqlColumnDefinitions(ddl)]) !== this.originalColumnDefinitions) {
        throw new Error("数据库完整字段定义已发生变化，请重新打开表结构后再调整");
      }
      const sql = mysqlColumnOrderSql(ddl, schema, table, this.originalOrder, desired);
      const cfg = this.app.connectionById(connectionId);
      if (!cfg || columnOrderCapability(cfg.kind, cfg.params, cfg.read_only) ||
        this.tab.connectionId !== connectionId || this.tab.schema !== initialTabSchema || this.tab.object !== table) {
        throw new Error("连接、数据库或只读状态已变化，字段排序没有执行");
      }
      if (!sql) return;
      await api.executeDdl(connectionId, sql);
      submitted = true;
      const confirmed = await api.getTableSchema(connectionId, schema, table);
      const actual = confirmed.columns.map(c => c.name);
      if (actual.length !== desired.length || actual.some((name, i) => name !== desired[i])) {
        throw new Error(`数据库返回的实际字段顺序与草稿不同：${actual.join(" → ")}`);
      }
      this.fields = [];
      this.tab.dirty = false;
      await this.reload();
      this.fieldOrderStatus = "字段顺序已保存，并已核对数据库实际顺序";
      this.app.toast("success", this.fieldOrderStatus);
    } catch (error) {
      this.tab.dirty = true;
      this.fieldOrderStatus = `${submitted ? "排序 DDL 已执行，但结果尚未确认；请重新读取结构核对，不能当作保存成功。" : "字段排序未确认保存，草稿已保留。"}${String(error)}`;
      this.app.toast("error", this.fieldOrderStatus);
    } finally {
      this.savingStructure = false;
      this.el.setAttribute("aria-busy", "false");
      this.render();
      this.app.notifyToolbar();
    }
  }

  /** 保存：生成并执行 ALTER TABLE。 */
  async saveChanges(): Promise<void> {
    this.flushFieldInput();
    const cfg = this.app.connectionById(this.tab.connectionId);
    if (!cfg) return;
    if (cfg.read_only) { this.app.toast("error", "此连接只读，不能保存结构修改"); return; }
    if (this.savingStructure) return;
    if (this.fields.length && this.originalOrder.length && this.fieldOrderChanged()) {
      await this.saveFieldOrder();
      return;
    }
    const kind = cfg.kind;
    const q = (s: string) =>
      kind === "mysql"
        ? `\`${s.replace(/`/g, "``")}\``
        : `"${s.replace(/"/g, '""')}"`;
    const full = this.tab.schema
      ? `${q(this.tab.schema)}.${q(this.tab.object)}`
      : q(this.tab.object);
    const colType = (f: (typeof this.fields)[number]): string => {
      let t = f.type;
      if (f.length) t += `(${f.length}${f.scale ? `,${f.scale}` : ""})`;
      if (kind === "mysql") {
        if (f.unsigned) t += " UNSIGNED";
        if (f.zerofill) t += " ZEROFILL";
      }
      if (!f.nullable) t += " NOT NULL";
      if (f.origDefault) t += ` DEFAULT ${f.origDefault}`;
      if (f.comment && kind === "mysql") {
        t += ` COMMENT '${f.comment.replace(/'/g, "''")}'`;
      }
      return t;
    };

    const ddls: string[] = [];
    // 与加载时完全同构地重建原始类型串，避免「未修改也永远不相等」导致的空 DDL。
    const origTypeOf = (f: (typeof this.fields)[number]): string => {
      let t = f.origBase || f.origType;
      if (f.origLen) t += `(${f.origLen}${f.origScale ? `,${f.origScale}` : ""})`;
      if (kind === "mysql") {
        if (f.origUnsigned) t += " UNSIGNED";
        if (f.origZerofill) t += " ZEROFILL";
      }
      if (!f.origNullable) t += " NOT NULL";
      if (f.origDefault) t += ` DEFAULT ${f.origDefault}`;
      if (f.origComment && kind === "mysql") {
        t += ` COMMENT '${f.origComment.replace(/'/g, "''")}'`;
      }
      return t;
    };
    for (const f of this.fields) {
      if (f.isNew && !f.isDeleted && f.name) {
        let t = colType(f);
        if (f.isAutoInc && kind === "mysql") t += " AUTO_INCREMENT";
        ddls.push(`ALTER TABLE ${full} ADD COLUMN ${q(f.name)} ${t}`);
      } else if (!f.isNew && f.isDeleted) {
        ddls.push(`ALTER TABLE ${full} DROP COLUMN ${q(f.name)}`);
      } else if (!f.isNew && !f.isDeleted && f.name) {
        if (f.name !== f.origName) {
          if (kind === "mysql") {
            ddls.push(
              `ALTER TABLE ${full} CHANGE COLUMN ${q(f.origName)} ${q(f.name)} ${colType(f)}`,
            );
          } else {
            ddls.push(`ALTER TABLE ${full} RENAME COLUMN ${q(f.origName)} TO ${q(f.name)}`);
          }
        } else if (colType(f) !== origTypeOf(f) && kind === "postgresql") {
          ddls.push(`ALTER TABLE ${full} ALTER COLUMN ${q(f.name)} TYPE ${colType(f)}`);
        } else if (colType(f) !== origTypeOf(f) && kind !== "sqlite") {
          ddls.push(`ALTER TABLE ${full} MODIFY COLUMN ${q(f.name)} ${colType(f)}`);
        } else if (colType(f) !== origTypeOf(f) && kind === "sqlite") {
          this.app.toast("info", `SQLite 不支持修改列「${f.name}」的类型，已跳过`);
        }
        // MySQL 的 MODIFY COLUMN 已携带 NOT NULL 子句；SET/DROP NOT NULL 语法仅 PG 等支持。
        if (f.nullable !== f.origNullable && kind !== "sqlite" && kind !== "mysql") {
          ddls.push(
            f.nullable
              ? `ALTER TABLE ${full} ALTER COLUMN ${q(f.name)} DROP NOT NULL`
              : `ALTER TABLE ${full} ALTER COLUMN ${q(f.name)} SET NOT NULL`,
          );
        }
      }
    }
    // 主键变更（仅在主键集合真变化时发出，避免每次保存都重建主键）
    const pkCols = this.fields
      .filter((f) => f.pk && !f.isDeleted && !f.isNew && f.name)
      .map((f) => f.name);
    const pkChanged =
      pkCols.length !== this.origPk.length ||
      pkCols.some((c, i) => c !== this.origPk[i]) ||
      pkCols.some((c) => !this.origPk.includes(c));
    if (pkChanged && pkCols.length && kind === "postgresql") {
      ddls.push(
        `ALTER TABLE ${full} DROP CONSTRAINT IF EXISTS ${q(`${this.tab.object}_pkey`)}, ADD PRIMARY KEY (${pkCols.map(q).join(", ")})`,
      );
    } else if (pkChanged && pkCols.length && kind !== "sqlite") {
      ddls.push(
        `ALTER TABLE ${full} DROP PRIMARY KEY, ADD PRIMARY KEY (${pkCols.map(q).join(", ")})`,
      );
    } else if (pkCols.length && kind === "sqlite") {
      this.app.toast("info", "SQLite 不支持在现有表上调整主键，已跳过");
    }

    if (!ddls.length) {
      this.app.toast("info", "没有需要保存的修改");
      return;
    }
    try {
      for (const ddl of ddls) {
        await api.executeDdl(this.tab.connectionId, ddl);
      }
      this.app.toast("success", `已保存 ${ddls.length} 项结构修改`);
      this.fields = [];
      await this.reload();
    } catch (e) {
      this.app.toast("error", String(e));
    }
  }

  private appendSimpleTable(body: HTMLElement, headers: string[], rows: string[][]): void {
    const table = document.createElement("table");
    table.className = "mini-table";
    const thead = document.createElement("thead");
    const tr = document.createElement("tr");
    for (const h of headers) {
      const th = document.createElement("th");
      th.textContent = h;
      tr.appendChild(th);
    }
    thead.appendChild(tr);
    table.appendChild(thead);
    const tbody = document.createElement("tbody");
    for (const r of rows) {
      const row = document.createElement("tr");
      for (const c of r) {
        const td = document.createElement("td");
        td.textContent = c;
        row.appendChild(td);
      }
      tbody.appendChild(row);
    }
    table.appendChild(tbody);
    body.appendChild(table);
  }

  destroy(): void {
    /* nothing */
  }
}

/** 图表标签页仅渲染内存快照，刷新也不会重跑来源 SQL。 */
class ChartTabView extends TabView {
  private panel: ChartPanel | null = null;
  constructor(app: App, tab: TabState) {
    super(app, tab);
    if (tab.chart) {
      this.panel = new ChartPanel(tab.chart);
      this.el.appendChild(this.panel.el);
    } else {
      const empty = document.createElement("div");
      empty.className = "empty-state";
      empty.textContent = "请从表数据页或查询结果页点击“创建图表”；图表使用创建时的数据快照。";
      empty.setAttribute("role", "status");
      this.el.appendChild(empty);
    }
  }
  activate(): void { this.panel?.focus(); }
  async reload(): Promise<void> { this.panel?.redraw(); }
  destroy(): void { this.panel?.destroy(); }
}


