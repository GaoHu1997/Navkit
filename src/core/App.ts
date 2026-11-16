/**
 * 应用主控制器：组装菜单栏 / 工具栏 / 连接树 / 标签页 / 状态栏，
 * 并按实际连接能力和当前上下文分发菜单命令；不以空操作冒充成功。
 */

import { api, datasourceSyncApi, DB_META, type DriverDefinition, type MutationDraft, type MutationReceipt, type DbValue } from "./api";
import { toDbValue } from "./mutation";
import { ConnectionHealthMonitor } from "./connection-health-client";
import type { RuntimeProfileBindings } from "./runtime-profile";
import { loadObjectCatalog } from "./object-catalog-loader";
import { CATALOG_KINDS, TREE_CATALOG_KINDS, catalogTreeGroups, catalogTreeItem, catalogNotice, catalogInfo, catalogReadReason, catalogCallReason, type CatalogListing } from "./object-navigation";
import { objectKey, objectLabel as catalogObjectLabel, type DatabaseObjectId, type ObjectDescriptor } from "./object-catalog";
import { RoutineDocumentView } from "../ui/RoutineDocumentView";
import { RoutineCallDialog } from "../dialogs/RoutineCallDialog";
import { DatasourceSync, invalidatedDatasourceIds, type DatasourceSyncResult } from "./datasourceSync";
import { ManualTransactions, type TransactionContext } from "./ManualTransactions";
import { ExecutionController, type ExecutionState, type Capabilities } from "./execution";
import type { DraftSnapshot } from "./editing-state";
import { chooseLeave, type LeavePromptItem } from "../dialogs/LeaveGuardDialog";
import { showDraftText } from "../dialogs/TextDialog";
import { encodeGridCopy, type CopyMode } from "./grid-clipboard";
import { MANUAL_TRANSACTION_WRITE_COMMANDS, manualTransactionUnavailableReason, type ManualTransactionStatus } from "./manualTransaction";
import { confirmTransactionAction } from "../dialogs/TransactionDialog";
import { TableDesigner } from "../ui/TableDesigner";
import { openTableOperationDialog } from "../dialogs/TableOperationDialog";
import { loadObjectDetails } from "./objectDetails";
import { hasSavedQueryScope, savedQueriesFor, queryFavorite, savedQueryConflict } from "./savedQueries";
import { objectTools, objectGrid, objectLabel, objectProtocol, objectCapability, newObjectSql, dropObjectSql, type BrowserObject } from "./objectBrowser";
import { formatSql } from "./sqlFormat";
import { openObjectAction } from "../dialogs/ObjectActionDialog";
import { openObjectMutationDialog } from "../dialogs/ObjectMutationDialog";
import { openHistoryLogDialog } from "../dialogs/HistoryLogDialog";
import { openServerMonitorDialog } from "../dialogs/ServerMonitorDialog";
import { openDataGeneratorDialog } from "../dialogs/DataGeneratorDialog";
import { openBatchExecutionDialog, openSqlFileExecutionDialog } from "../ui/BatchExecutionPanel";
import { openQueryBuilderDialog } from "../dialogs/QueryBuilderDialog";
import { queryBuilderUnavailableReason } from "./queryBuilder";
import { explainQuerySql } from "./queryExecution";
import { QueryResultPages } from "./queryResultPages";
import { queryResultApi, type QueryResultIpcContext } from "./query-result-api";
import { QueryResultPanel } from "../ui/QueryResultPanel";
import { ColumnSidebar } from "../ui/ColumnSidebar";
import { filterJoinControl, filterValueControl } from "../ui/TableFilterControls";
import "../ui/data-grid-controls.css";
import { openDriverManager } from "../dialogs/DriverManagerDialog";
import { openSyncDialog, taskFeedback } from "../dialogs/SyncDialog";
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
import { SqlEditor } from "../ui/SqlEditor";
import { TabStrip } from "../ui/TabStrip";
import { Ribbon, RIBBON_MAIN } from "../ui/Ribbon";
import { navkitLogo } from "../ui/logo";
import { InfoPanel } from "../ui/InfoPanel";
import { TabToolbar, type TabToolItem } from "../ui/TabToolbar";
import { TreePanel } from "../ui/TreePanel";
import { Dialog, field, radioGroup, textInput } from "../ui/Dialog";
import {
  ICON_WIN_MIN,
  ICON_WIN_MAX,
  ICON_WIN_CLOSE,
  ICON_PANEL_LEFT,
  ICON_PANEL_RIGHT,
  ICON_MSG_INFO,
  ICON_MSG_ERROR,
  ICON_PAGE_FIRST,
  ICON_PAGE_PREV,
  ICON_PAGE_NEXT,
  ICON_PAGE_LAST,
  ICON_GRID_VIEW,
  ICON_FORM_VIEW,
  ICON_PLUS_LINE,
  ICON_MINUS_LINE,
  ICON_CHECK,
  ICON_CROSS,
  ICON_TITLEBAR_TASKS,
  ICON_EMPTY_CONNECT,
  svg,
  iconSvgByName,
  type IconDef,
} from "../ui/icons";
import { TaskCenterPanel, openTaskDetailDialog } from "../ui/TaskCenterPanel";
import { taskCenter } from "./backgroundTasks";
import { openDataDictionaryDialog } from "../dialogs/DataDictionaryDialog";
import { openErDiagramDialog } from "../dialogs/ErDiagramDialog";
import { openUpdateDialog } from "../dialogs/UpdateDialog";
import { UpdateBadge } from "../ui/UpdateBadge";
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
import { openJobHistoryDialog, openScheduleJobDialog } from "../dialogs/ScheduleDialog";
import { addSnippet, querySnippets, removeSnippet } from "./snippets";
import { defaultGeneratorFields, generateDataPreview, MAX_GENERATED_ROWS } from "./dataGenerator";
import { openChangePasswordDialog, openCreateUserDialog, openEditUserDialog } from "../dialogs/UserDialog";
import {
  openAboutDialog,
  openFavoritesDialog,
  openServerInfoDialog,
  openSettingsDialog,
} from "../dialogs/SettingsDialog";
import { openBatchExportWizard } from "../dialogs/BatchExportDialog";
import { bindExportPath } from "./export-path";
import { namespaceLabel, namespaceName, namespaceType } from "./namespace-label";
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
  private taskPanel!: TaskCenterPanel;
  private taskBadge!: HTMLElement;
  private updateBadge!: UpdateBadge;

  private tabs: TabState[] = [];
  private copyOwner: { tabId: string; kind: "grid" | "editor"; grid?: DataGrid } | null = null;
  private copyContext: { tabId: string; grid: DataGrid } | null = null;
  private draftPreparing = false;
  private draftSyncApproved = false;
  private deferredTabRemovals: Set<string> | null = null;
  private activeTabId: string | null = null;
  private treeNodes: TreeItem[] = [];
  private treeLoads = new WeakMap<TreeItem, object>();
  private connections: ConnectionConfig[] = [];
  private groups: string[] = [];
  private settings!: AppSettings;
  private favorites: Favorite[] = [];
  private favoritesError = "";
  private recent: RecentItem[] = [];
  private openConnections = new Set<string>();
  private keepAliveMonitor?: ConnectionHealthMonitor;
  private readonly runtimeProfiles: RuntimeProfileBindings = {
    load: id => api.runtimeProfileView(id),
    test: (request, id, submission, preserve) => api.testRuntimeConnection(request, id, submission, preserve),
  };
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
    removeTab: id => { if (this.deferredTabRemovals) this.deferredTabRemovals.add(id); else this.removeTabNow(id); },
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
  /** 工具栏读取当前数据页的格式；预览状态由各数据视图持有，不能跨页共享。 */
  private get textFormat(): "text" | "hex" | "image" | "web" {
    const view = this.activeView();
    return view instanceof DataTabView ? view.previewFormat : "text";
  }
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
  private objectDetailsRequest: { key: string; request: number; pending: boolean } | null = null;
  private newConnectionGroup = "";

  constructor(root: HTMLElement) {
    this.root = root;
    this.buildSkeleton();
  }

  // ------------------------------------------------------------ 骨架

  private buildSkeleton(): void {
    this.root.innerHTML = "";
    this.shell = document.createElement("div");
    this.shell.className = "app-shell";

    // 标题栏（v2 · 38px）：logo + 产品名 + 菜单并入 + 右侧工具 + 窗口按钮
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
    // 右侧工具：后台任务中心（点击任务图标显示 / 再次点击关闭）
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
    // 更新徽标：位于后台任务图标左侧（发现新版本时才出现）
    this.updateBadge = new UpdateBadge((kind, text) => this.toast(kind, text));
    tools.appendChild(this.updateBadge.el);
    this.taskPanel = new TaskCenterPanel();
    this.taskPanel.onOpenTask = (task) => openTaskDetailDialog(task, (p) => {
      void api.openPathInExplorer(p).catch((e) => this.toast("error", String(e)));
    });
    const taskBtn = tbIcon(svg(ICON_TITLEBAR_TASKS, 15), "后台任务", () => {
      this.taskPanel.toggle(taskBtn);
    });
    this.taskBadge = document.createElement("span");
    this.taskBadge.className = "task-badge";
    this.taskBadge.hidden = true;
    taskBtn.appendChild(this.taskBadge);
    taskCenter.subscribe(() => this.refreshTaskBadge());
    tools.appendChild(taskBtn);
    const spacer = document.createElement("div");
    spacer.className = "titlebar-spacer";
    spacer.setAttribute("data-tauri-drag-region", "");
    titlebar.appendChild(logo);
    titlebar.appendChild(titleText);
    titlebar.appendChild(titleSep);
    titlebar.appendChild(spacer);
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
    try { this.favorites = await api.listFavorites(); }
    catch (e) { this.favoritesError = `收藏不可读取，原文件已保留；修复文件后请重启：${String(e)}`; }
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
    this.setupDraftSyncGuard();

    // 查看菜单勾选态持久化：恢复上次会话的界面显示偏好
    this.loadViewPrefs();
    this.statusBar.style.display = this.statusBarVisible ? "" : "none";

    this.refreshTree();
    this.renderTabs();
    this.renderStatus();
    // 自动任务：载入任务并启动调度（应用运行期间按间隔到期执行）
    window.addEventListener("beforeunload", e => {
      if (!this.exitApproved && (this.draftPreparing || this.manualTransactionController.hasActive() || this.tabs.some(tab => this.views.get(tab.id)?.draftDirty()))) {
        e.preventDefault(); e.returnValue = "";
      }
    });
    void this.reloadScheduleJobs();
    this.setupScheduleTimer();
    await this.datasourceSync.start();
    window.addEventListener("beforeunload", () => this.datasourceSync.stop());
    // 更新徽标：探测已有安装包 + 定时检查新版本
    this.updateBadge.start();
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
          if (this.exitApproved) return;
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
        const accepted = await this.guardLeave(this.tabs.map(t => t.id), "退出应用", async () => {
          await api.closeAllConnections();
          this.keepAliveMonitor?.stopAll();
          this.exitApproved = true;
          const internals = (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
          if (internals) {
            const { getCurrentWindow } = await import("@tauri-apps/api/window");
            await getCurrentWindow().close();
          } else window.close();
        });
        if (!accepted) { this.exitApproved = false; this.exitPending = false; return; }
      } catch (error) {
        this.exitPending = false;
        this.toast("error", `关闭连接或回滚失败，未退出应用：${String(error)}`);
        return;
      }
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
          node.loaded = old.loading ? false : old.loaded;
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
            .filter((f) => f.connection_id === connId && !!f.schema && f.schema === (m[2] ?? "main"))
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
    const namespaceKind = namespaceType(this.connectionById(node.id.slice(5).split("/")[0]));
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
      return namespaceKind;
    }
    if (node.id.includes("/table/")) return "table";
    if (node.id.includes("/view/")) return "view";
    if (node.id.includes("/proc/")) return "procedure";
    if (node.id.includes("/trigger/")) return "leaf";
    return namespaceKind;
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
    if (node.objectDescriptor) {
      const descriptor = node.objectDescriptor, readOnly = !!this.connectionById(descriptor.object.connection_id)?.read_only;
      const readReason = catalogReadReason(descriptor, readOnly), callReason = catalogCallReason(descriptor, readOnly);
      return [
        { id: "tree-design-routine", label: "打开定义", disabled: !!readReason, disabledReason: readReason ?? undefined },
        ...(["function", "procedure"].includes(descriptor.object.kind) ? [{ id: "tree-call-routine", label: "调用例程…", disabled: !!callReason, disabledReason: callReason ?? undefined }] : []),
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
    const connection = /^conn:[^/]+$/.test(node.id);
    if (node.expanded) {
      node.expanded = false;
      if (connection) {
        // A later expand must discover databases created or removed outside this tree.
        node.loaded = false;
        this.treeLoads.delete(node);
        node.loading = false;
      }
      this.tree.render();
      return;
    }
    if (node.loading) return;
    node.expanded = true;
    if (!node.loaded && (connection || node.children.length === 0)) {
      const generation = {};
      this.treeLoads.set(node, generation);
      const current = () => this.treeLoads.get(node) === generation && this.tree.find(node.id) === node;
      node.loading = true;
      this.tree.render();
      try {
        await this.loadChildren(node, current);
        if (!current()) return;
        node.loaded = true;
      } catch {
        if (!current()) return;
        node.expanded = false;
        node.loaded = false;
      } finally {
        if (current()) {
          this.treeLoads.delete(node);
          node.loading = false;
          this.syncConnIcons();
          this.tree.render();
        }
      }
      return;
    }
    this.syncConnIcons();
    this.tree.render();
  }

  /** 目录失败保留明确unknown状态，不能退回按名称猜测身份。 */
  async navigationCatalog(connectionId: string, schema: string, kinds = CATALOG_KINDS): Promise<CatalogListing> {
    try { return await loadObjectCatalog({ connection_id: connectionId, catalog: null, schema, kinds }, { list: (scope, cursor) => api.listObjectCatalog(scope, cursor) }); }
    catch (error) { return { objects: [], revision: "", sections: Object.fromEntries(kinds.map(kind => [kind, { state: "unknown" as const, reason: String(error) }])) }; }
  }

  /** 加载节点的子节点。 */
  private async loadChildren(node: TreeItem, current: () => boolean = () => true): Promise<void> {
    // 分组节点统一拦截：查询分组懒加载收藏的保存查询、备份分组懒加载备份文件，
    // 其余分组（字段 / 索引 / 外键 / 触发器）的内容已由父节点（表 / 视图）在
    // 加载时构建完毕 —— 必须在下方表分支之前 return，否则 id 中的
    // "table/<表名>/grp-xx" 会落进表正则、被当成表名 "<表名>/grp-xx" 再生成
    // 一层「字段/索引/外键/触发器」分组，造成无限套叠（字段也跟着对不上）。
    if (/\/grp-(column|index|fk|trigger|query|backup|table|view|function|event|sequence|materialized)$/.test(node.id)) {
      if (/\/grp-query$/.test(node.id)) {
        this.refreshQueryGroups();
        return;
      }
      if (/\/grp-backup$/.test(node.id)) {
        const m = /^conn:([^/]+)\/db\/([^/]+)\/grp-backup$/.exec(node.id);
        if (m) {
          try {
            const backups = await api.listBackups(m[1], m[2]);
            if (!current()) return;
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
            if (current()) node.children = [];
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
        await this.ensureConnection(connId, current);
        if (!current()) return;
        const dbs = await api.listDatabases(connId);
        if (!current()) return;
        if (cfg.kind === "sqlite") {
          // SQLite 直接把表列在连接下
          const tables = await api.listTables(connId, "");
          if (!current()) return;
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
          node.children.push({
            id: `${node.id}/grp-query`, label: "查询", type: "folder", icon: "pict_query",
            children: savedQueriesFor(this.favorites, connId, "main").map(favorite => ({
              id: `${node.id}/query/${favorite.id}`, label: favorite.name, type: "query", icon: "info_pane_query",
              children: [], expandable: false, has_children: false, expanded: false,
            })), expandable: true, has_children: true, expanded: false,
          });
          return;
        }
        const previous = new Map(node.children.map(child => [child.id, child]));
        node.children = dbs.map(([name]) => previous.get(`${node.id}/db/${name}`) ?? ({
          id: `${node.id}/db/${name}`,
          label: name,
          type: namespaceType(cfg),
          // 未打开的命名空间不亮起、不带展开箭头（打开后由 syncConnIcons 补上）
          icon: "info_pane_schema",
          children: [],
          expandable: this.openDbs.has(`${connId}/${name}`),
          has_children: true,
          expanded: false,
        }));
      } catch (e) {
        if (!current()) return;
        this.toast("error", `无法列出${namespaceName(cfg)}：${String(e)}`);
        throw e;
      }
      return;
    }
    // 库节点：表 / 视图 / 函数 / 查询 / 备份 分组
    if (/^conn:[^/]+\/db\/[^/]+$/.test(node.id)) {
      const connId = node.id.slice(5).split("/db/")[0];
      const dbName = node.id.split("/db/")[1];
      try {
        const tables = await api.listTables(connId, dbName);
        const catalog = await this.navigationCatalog(connId, dbName, TREE_CATALOG_KINDS);
        if (!current()) return;
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
        // 查询分组：列出收藏夹中保存的查询（connection 匹配或通用）
        const savedQueries: TreeItem[] = this.favorites
          .filter((f) => f.connection_id === connId && !!f.schema && f.schema === (node.id.split("/db/")[1] ?? "main"))
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
          ...catalogTreeGroups(node.id, catalog),
          group(`grp-query`, "查询", "pict_query", savedQueries),
          group(`grp-backup`, "备份", "pict_backup", []),
        ];
      } catch (e) {
        if (current()) this.toast("error", `无法列出对象：${String(e)}`);
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
          // 主键字段用琥珀钥匙图标与普通字段区分。
          icon: c.primary_key ? "info_pane_pk" : "info_pane_column",
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
        const trs: TreeItem[] = await this.loadTableTriggers(connId, schema || (kind === "sqlite" ? "main" : ""), table, node.id);
        if (!current()) return;
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
        if (current()) node.children = [];
      }
    }
  }

  /** 读取表上的触发器（按数据库类型）。baseId 为所属表节点 id。 */
  private async loadTableTriggers(
    connId: string,
    schema: string,
    table: string,
    baseId: string,
  ): Promise<TreeItem[]> {
    const catalog = await this.navigationCatalog(connId, schema, ["trigger"]);
    const notice = catalogNotice(catalog.sections);
    if (notice) this.toast("info", notice);
    return catalog.objects.filter(descriptor => descriptor.object.parent?.schema === schema && descriptor.object.parent.name === table)
      .map(descriptor => catalogTreeItem(baseId, descriptor));
  }

  /** 打开数据库：亮起图标、显示展开箭头并加载子节点（双击 / 「打开数据库」）。 */
  private async openDatabaseNode(node: TreeItem, connId: string, dbName: string): Promise<boolean> {
    if (node.loading) return false;
    // 已加载且仍处于打开状态：只需重新展开，无需再次打开连接/库
    if (node.loaded && this.openDbs.has(`${connId}/${dbName}`)) {
      node.expandable = true;
      node.expanded = true;
      this.syncConnIcons();
      this.tree.render();
      return true;
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
      return true;
    } catch (e) {
      this.openDbs.delete(`${connId}/${dbName}`);
      node.loaded = false;
      this.toast("error", `打开${namespaceName(this.connectionById(connId))}失败：${String(e)}`);
      this.syncConnIcons();
      return false;
    } finally {
      node.loading = false;
    }
  }

  private async ensureConnection(connId: string, current: () => boolean = () => true): Promise<void> {
    if (this.openConnections.has(connId)) return;
    const result = await api.ensureConnection(connId);
    if (!current()) return;
    if (!result.ok) throw new Error(result.message);
    this.openConnections.add(connId);
    this.startKeepAlive(connId);
    this.syncConnIcons();
  }

  /** PL-27：按后端代次和到期时间串行观察；禁用/不支持的连接不安排 SQL 探测。 */
  private startKeepAlive(connId: string) {
    this.keepAliveMonitor ??= new ConnectionHealthMonitor({
      view: id => api.connectionHealth(id), check: id => api.checkConnectionHealth(id),
      offline: (id, message) => this.toast("error", `连接 ${this.connectionById(id)?.name ?? id} 保活探测失败：${message}`),
      schedule: (callback, delay) => window.setTimeout(callback, delay),
      cancel: timer => window.clearTimeout(timer),
    });
    this.keepAliveMonitor.start(connId);
  }

  private stopKeepAlive(connId: string) { this.keepAliveMonitor?.stop(connId); }

  /** 关闭数据库：图标熄灭、收起并隐藏展开箭头。 */
  private async closeDatabaseNode(connId: string, dbName: string): Promise<boolean> {
    const namespace = namespaceName(this.connectionById(connId));
    return this.guardLeave(this.tabs.filter(tab => tab.connectionId === connId && tab.schema === dbName).map(tab => tab.id), `关闭${namespace}页面`, async () => {
      const transaction = this.manualTransactionFor(connId);
      if (transaction?.schema === dbName) throw new Error("请先在事务所属页提交或回滚");
      this.suspendDatasourceTabs(new Set([connId]), `${namespace}已关闭，旧页面已暂停。SQL 草稿仍保留，请明确重新选择连接。`, dbName);
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
    });
  }

  /** 选中树节点：仅移动选中光标；库节点单击不打开、不亮起（双击/「打开数据库」才打开）。 */
  private async selectNode(node: TreeItem): Promise<void> {
    // 会打开对象页的选择交给标签页统一更新详情，避免先显示节点详情再立即覆盖。
    if (/grp-backup$/.test(node.id)) {
      const connId = node.id.slice(5).split("/db/")[0];
      const dbName = node.id.split("/db/")[1].replace(/\/grp-backup$/, "");
      this.openSpecialTab("backup", connId, dbName);
      return;
    }
    if (/grp-(table|view|function|query|trigger|event|sequence|materialized)$/.test(node.id) && !/\/(table|view)\//.test(node.id)) {
      const group = /^conn:([^/]+)(?:\/db\/([^/]+))?\/grp-(table|view|function|query|trigger|event|sequence|materialized)$/.exec(node.id);
      if (!group) return;
      const connId = group[1], dbName = group[2] ?? "main";
      this.syncDbIcons(node.id);
      // 「表」分组只展示表，「视图」只展示视图，「函数」只展示函数 / 存储过程
      const filter: ObjectType = group[3] === "materialized" ? "materialized_view" : group[3] as ObjectType;
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
    this.updateInfoPanelForNode(node);
  }

  /** 打开备份 / 用户等特殊标签页。 */
  openSpecialTab(kind: "backup" | "users", connId: string, schema: string): void {
    const cfg = this.connections.find((c) => c.id === connId);
    if (!cfg) return;
    if (kind === "backup") {
      // 备份是对象浏览的一个类别，复用该连接固定在左侧的对象页。
      this.openObjectsTab(connId, schema, "folder", false, "backup");
      return;
    }
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
      title: "用户",
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
      (t) => t.kind === "viewdesign" && t.connectionId === connId && t.schema === schema && t.object === view,
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
  async openRoutineDesignTab(connId: string, schema: string, name: string, routineType: string, exact?: DatabaseObjectId): Promise<void> {
    if (!this.connections.some(c => c.id === connId)) return;
    try {
      let object = exact;
      if (!object) {
        const catalog = await loadObjectCatalog({ connection_id: connId, catalog: null, schema, kinds: [routineType] }, { list: (scope, cursor) => api.listObjectCatalog(scope, cursor) });
        const matches = catalog.objects.filter(item => item.object.name === name && item.object.kind === routineType);
        if (matches.length !== 1) throw new Error(matches.length ? "存在多个重载，请在完整对象目录中选择具体签名" : "没有取得此例程的完整可见身份；权限未知不等于不存在");
        object = matches[0].object;
      }
      if (object.connection_id !== connId || object.schema !== schema || object.name !== name || object.kind !== routineType || object.parent) throw new Error("例程身份与当前连接 / 作用域不一致");
      const key = objectKey(object), existing = this.tabs.find(t => t.kind === "routinedesign" && t.routineObject && objectKey(t.routineObject) === key);
      if (existing) { this.activeTabId = existing.id; this.renderTabs(); return; }
      const snapshot = await api.readRoutineDesignObject(object);
      const tab = this.makeTab("routinedesign", catalogObjectLabel(object), connId, schema, name, routineType as ObjectType, snapshot.definition);
      tab.routineObject = structuredClone(object); tab.routineSnapshot = snapshot;
      this.tabs.push(tab); this.activeTabId = tab.id; this.renderTabs();
    } catch (error) { this.toast("error", `读取精确例程失败：${String(error)}`); }
  }

  async openCatalogObject(descriptor: ObjectDescriptor, call = false): Promise<void> {
    const object = descriptor.object;
    const readOnly = !!this.connectionById(object.connection_id)?.read_only;
    const reason = call ? catalogCallReason(descriptor, readOnly) : catalogReadReason(descriptor, readOnly);
    if (reason) throw new Error(reason);
    if (call) {
      if (!["function", "procedure"].includes(object.kind)) throw new Error("所选对象不是可调用例程");
      new RoutineCallDialog(object, `routine-call-${crypto.randomUUID()}`, {
        metadata: value => api.routineCallMetadata(value), prepare: (draft, owner) => api.previewRoutineCall(draft, owner),
        execute: (plan, request) => api.executeRoutineCall(plan, request), issueGrant: request => api.issueGrant(request),
        cancelExecution: context => api.cancelExecution(context), executionStatus: context => api.executionStatus(context),
      });
      return;
    }
    if (["function", "procedure"].includes(object.kind)) { await this.openRoutineDesignTab(object.connection_id, object.schema, object.name, object.kind, object); return; }
    const definition = await api.objectDefinition(object);
    if (objectKey(definition.object) !== objectKey(object)) throw new Error("对象定义身份不一致，拒绝展示错误对象");
    showDraftText(`定义 · ${descriptor.display_name}`, definition.definition, `只读定义；完整性：${definition.completeness}。未验证的启停 / 删除 / 修改不发送SQL。`);
  }

  /** 打开（或聚焦）唯一的对象标签页。
   *  所有连接 / 数据库全局共用一个对象页：它永远是标签条的第一个页签（其它页签之前），
   *  单击树上不同连接 / 分组 / 数据库只就地切换内容，不再新开页签，也绝不出现第二个对象页。
   *  filter：对象来源（树上选中「表」分组只展示表、「视图」分组只展示视图、库节点展示全部）。 */
  openObjectsTab(connId: string, schema: string, filter: ObjectType = "folder", markOpened = true, browserKind: "objects" | "backup" = "objects"): void {
    const cfg = this.connections.find((c) => c.id === connId);
    if (!cfg) return;
    // 记录「已打开的库」，树中该库图标显示为绿色高亮。
    // 单击库节点仅浏览对象（markOpened=false），不改库图标，避免 mousedown
    // 期间重建图标节点而破坏后续 click/dblclick 的目标。
    if (markOpened) this.openDbs.add(`${connId}/${schema}`);
    this.openConnections.add(connId);
    this.syncConnIcons();
    const title = "对象";
    // 全局唯一对象页：不按连接区分，找到任意一个对象 / 备份页即复用。
    const existing = this.tabs.find((t) => t.kind === "objects" || t.kind === "backup");
    if (existing) {
      // 共用对象页：切换连接 + 库 + 过滤并聚焦，不新开页签。
      if (existing.kind !== browserKind) {
        this.views.get(existing.id)?.destroy();
        this.views.delete(existing.id);
        existing.kind = browserKind;
      }
      // 复用的对象页若曾因连接断开/变更被标记为数据源失效，切换目标即视为重新验证：
      // 清除失效态并丢弃旧视图（含其中的失效提示），随后重建。
      if (existing.datasourceStale) {
        this.views.get(existing.id)?.destroy();
        this.views.delete(existing.id);
        delete existing.datasourceStale;
      }
      existing.selectedRows.clear();
      existing.connectionId = connId;
      existing.connectionName = cfg.name;
      existing.schema = schema;
      existing.objectType = filter;
      existing.title = title;
      // 对象页必须永远是第一个页签。
      const index = this.tabs.indexOf(existing);
      if (index > 0) {
        this.tabs.splice(index, 1);
        this.tabs.unshift(existing);
      }
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    const tab: TabState = {
      id: `o_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`,
      kind: browserKind,
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
    // 唯一对象页永远固定在标签条最左侧。
    this.tabs.unshift(tab);
    this.activeTabId = tab.id;
    this.renderTabs();
  }

  /** 双击树节点。 */
  private async openNode(node: TreeItem): Promise<void> {
    if (node.objectDescriptor) { await this.openCatalogObject(node.objectDescriptor); return; }
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
        await this.openSavedQuery(fav.id);
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
    this.tabToolbar.setItems(tab ? this.guardedTabToolbarItems(tab) : []);
    this.syncObjectSearchInput();
  }

  /** 对象页搜索框展开状态（工具栏搜索图标切换）。 */
  private objectSearchOpen = false;
  private objectSearchInput: HTMLInputElement | null = null;

  /** 展开时聚焦一次；普通工具栏刷新不重新创建或聚焦输入。 */
  private cmdObjectSearch(): void {
    this.objectSearchOpen = !this.objectSearchOpen;
    this.notifyToolbar();
    if (this.objectSearchOpen) this.objectSearchInput?.focus();
  }

  private syncObjectSearchInput(): void {
    const tab = this.activeTab;
    const view = tab?.kind === "objects" ? this.views.get(tab.id) : undefined;
    const searchButton = this.tabToolbar.el.querySelector<HTMLButtonElement>('[data-command="object-search"]');
    searchButton?.setAttribute("aria-expanded", String(this.objectSearchOpen && view instanceof ObjectsTabView));
    if (!this.objectSearchOpen || !(view instanceof ObjectsTabView)) {
      this.objectSearchInput?.remove();
      this.objectSearchInput = null;
      return;
    }
    if (this.objectSearchInput?.dataset.tabId === tab!.id) {
      // 不改动活动输入的 value/selection，保留 IME 与光标。视图换作用域时才同步值。
      if (document.activeElement !== this.objectSearchInput) this.objectSearchInput.value = view.searchValue();
      return;
    }
    this.objectSearchInput?.remove();
    const input = document.createElement("input");
    input.type = "search";
    input.className = "tab-tool-search";
    input.dataset.tabId = tab!.id;
    input.placeholder = "筛选对象名称";
    input.setAttribute("aria-label", "筛选对象名称（Esc 清空并收起）");
    input.value = view.searchValue();
    let composing = false;
    input.addEventListener("compositionstart", () => { composing = true; });
    input.addEventListener("compositionend", () => { composing = false; view.setSearchValue(input.value); });
    input.addEventListener("input", () => { if (!composing) view.setSearchValue(input.value); });
    input.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && !e.isComposing && !composing) {
        e.preventDefault(); e.stopPropagation();
        input.value = ""; view.setSearchValue("");
        this.objectSearchOpen = false;
        this.notifyToolbar();
        this.tabToolbar.el.querySelector<HTMLButtonElement>('[data-command="object-search"]')?.focus();
      }
    });
    this.tabToolbar.stableSlot.appendChild(input);
    this.objectSearchInput = input;
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
    if (this.draftPreparing) return;
    if (this.activeTabId === id) return;
    this.activeTabId = id;
    this.renderTabs();
  }

  private async closeTab(id: string): Promise<boolean> {
    return this.closeTabs([id]);
  }

  private async closeTabs(ids: string[]): Promise<boolean> {
    return this.guardLeave(ids, "关闭页面", async () => {
      this.deferredTabRemovals = new Set();
      try {
        for (const id of ids) {
          const tab = this.tabs.find(t => t.id === id);
          if (tab && this.manualTransactionFor(tab.connectionId)?.owner === id && !await this.closeManualTransactionTab(tab)) {
            throw new Error("事务关闭已取消或失败，所有页面保留");
          }
        }
      } finally { this.deferredTabRemovals = null; }
      for (const id of ids) this.removeTabNow(id);
    });
  }

  /** 离页身份快照：保存或丢弃前确认页面仍指向同一连接、命名空间和对象。 */
  private leaveIdentity(id: string): string {
    const tab = this.tabs.find(t => t.id === id);
    // 查询页保存后名称和对象会变化，不把可变标题计入上下文身份。
    return JSON.stringify(tab ? [tab.kind, tab.connectionId, tab.schema, tab.kind === "query" ? null : tab.object, this.connectionById(tab.connectionId)] : null);
  }

  /** 离页前的保存提醒：查询 SQL 未保存时询问是否保存；其它编辑按用户选择丢弃或取消。 */
  async guardLeave(ids: string[], cause: string, action: () => Promise<void>): Promise<boolean> {
    if (this.draftPreparing) return false;
    this.draftPreparing = true;
    const previousFocus = document.activeElement as HTMLElement | null;
    const inert = this.contentHost.inert;
    try {
      const views = ids.map(id => this.views.get(id)).filter((view): view is TabView => !!view);
      views.forEach(view => view.flushDraft());
      if (views.some(view => view.draftBusy())) throw new Error("查询、结构或事务操作进行中，请完成后重试");
      this.contentHost.inert = true;
      this.contentHost.setAttribute("aria-busy", "true");
      const identities = ids.map(id => this.leaveIdentity(id));
      const dirty = ids
        .map(id => ({ id, view: this.views.get(id) }))
        .filter((item): item is { id: string; view: TabView } => !!item.view && item.view.draftDirty());
      if (dirty.length) {
        const items: LeavePromptItem[] = dirty.map(({ id, view }) => {
          const tab = this.tabs.find(candidate => candidate.id === id);
          const savable = view instanceof QueryTabView && view.hasUnsavedSql() && !!tab
            && !!this.connectionById(tab.connectionId) && !!tab.schema.trim();
          return { title: tab?.title ?? id, kind: savable ? "query" : "other" };
        });
        const choice = await chooseLeave(items, cause);
        if (choice === "cancel") return false;
        if (choice === "save") {
          for (const [index, { id, view }] of dirty.entries()) {
            if (items[index].kind !== "query") continue;
            const tab = this.tabs.find(candidate => candidate.id === id);
            if (tab && view instanceof QueryTabView) await this.saveQueryAtLeave(tab, view);
          }
        }
        if (ids.some((id, i) => identities[i] !== this.leaveIdentity(id))) throw new Error("编辑或上下文已变化，离页已取消");
      }
      await action();
      dirty.forEach(({ id }) => {
        const view = this.views.get(id);
        if (view && !view.draftBusy()) view.discardDraft();
      });
      return true;
    } catch (e) { this.toast("error", `未离开当前页面：${String(e)}`); return false; }
    finally {
      this.contentHost.inert = inert; this.draftPreparing = false;
      // 离页结束必须重算工具栏：draftPreparing 期间按钮会被判为不可用，
      // 否则「数据生成」等入口会一直停在置灰状态，直到用户手动刷新。
      this.notifyToolbar();
      this.ribbon.updateDisabled();
      this.contentHost.removeAttribute("aria-busy");
      if (previousFocus?.isConnected && !document.querySelector(".dialog-backdrop")) previousFocus.focus();
    }
  }

  /** 关闭时保存查询：已绑定查询直接覆盖，否则询问名称；取消时抛出以阻止离页。 */
  private async saveQueryAtLeave(tab: TabState, view: QueryTabView): Promise<void> {
    const sql = view.editor.getSql();
    if (!sql.trim()) throw new Error("查询内容为空，无法保存");
    const binding = { connectionId: tab.connectionId, schema: tab.schema };
    if (!this.connectionById(binding.connectionId) || !binding.schema.trim()) throw new Error("保存查询前请选择连接和数据库");
    const existing = tab.savedQueryId ? this.favorites.find(favorite => favorite.id === tab.savedQueryId) : undefined;
    const save = async (name: string) => {
      if (tab.connectionId !== binding.connectionId || tab.schema !== binding.schema) throw new Error("编辑器上下文已变化，请重新保存");
      if (!this.connectionById(binding.connectionId)) throw new Error("连接已不存在，请重新选择连接和数据库");
      const source = view.editor.getSql();
      const favorite = queryFavorite(this.favorites, { id: existing?.id, name, sql: source, ...binding });
      const items = await api.addFavorite(favorite);
      tab.savedQueryId = favorite.id;
      view.markClean(source);
      this.publishFavorites(items, favorite.id);
      this.toast("success", `查询「${name}」已保存`);
    };
    if (existing?.name) { await save(existing.name); return; }
    const saved = await new Promise<boolean>(resolve => {
      let settled = false;
      const dialog = openObjectAction({
        title: "保存查询", label: "查询名称", value: tab.title, context: `${tab.connectionName} / ${tab.schema}`, confirm: "保存",
        submit: async name => { await save(name); settled = true; resolve(true); },
      });
      dialog.onClose(() => { if (!settled) resolve(false); });
    });
    if (!saved) throw new Error("保存查询已取消");
  }
  private setupDraftSyncGuard(): void {
    const commit = this.datasourceSync.commit.bind(this.datasourceSync);
    this.datasourceSync.commit = async (id, choices) => {
      let result: DatasourceSyncResult | undefined;
      const accepted = await this.guardLeave(this.tabs.map(tab => tab.id), "应用数据源同步", async () => {
        this.draftSyncApproved = true;
        try { result = await commit(id, choices); } finally { this.draftSyncApproved = false; }
      });
      if (!accepted || !result) throw new Error("数据源同步未应用，编辑或活动门禁已取消 / 失败");
      return result;
    };
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
      title.textContent = "尚未打开工作页";
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
      this.notifyToolbar();
      this.updateInfoPanel();
      this.ribbon.setActive(null);
      return;
    }
    let view = this.views.get(tab.id);
    if (!view) {
      view = this.createView(tab);
      this.views.set(tab.id, view);
      view.el.addEventListener("focusin", e => {
        const grid = (view as TabView & { grid?: DataGrid }).grid;
        if (grid?.el.contains(e.target as Node) && !(e.target instanceof HTMLInputElement)) {
          this.copyOwner = { tabId: tab.id, kind: "grid", grid };
          this.copyContext = null;
        } else if ((e.target as HTMLElement).closest?.(".cm-editor")) {
          this.copyOwner = { tabId: tab.id, kind: "editor" }; this.copyContext = null;
        }
      });
    }
    this.contentHost.innerHTML = "";
    this.contentHost.appendChild(view.el);
    if (!tab.datasourceStale) view.activate();
    this.ribbon.updateDisabled();
    this.notifyToolbar();
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
            : tab.kind === "users"
                  ? "open-user"
                  : tab.kind === "objects" && tab.objectType === "query"
                    ? "open-query"
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
      case "viewdesign":
        return new SqlDesignTabView(this, tab, "view");
      case "routinedesign":
        return new RoutineTabView(this, tab);
      default:
        return new DataTabView(this, tab);
    }
  }

  /** 当前标签页对应的二级工具栏按钮。 */
  private guardedTabToolbarItems(tab: TabState): TabToolItem[] {
    return this.tabToolbarItems(tab).map(item => {
      if (!item.id.startsWith("object-") && item.id !== "beautify-sql" && !MANUAL_TRANSACTION_WRITE_COMMANDS.has(item.id) && !(item.id === "save" && tab.kind !== "query")) return item;
      const reason = this.commandUnavailableReason(item.id);
      return reason ? { ...item, disabled: true, disabledReason: reason } : item;
    });
  }

  /** 「文本▼」按钮上的标题 = 当前选中的显示格式（文本 / 十六进制 / 图像 / 网页）。 */
  private textFormatLabel(format = this.textFormat): string {
    const id = `text-format-${format === "text" ? "plain" : format}`;
    return TEXT_FORMAT_MENU.find((it) => it.id === id)?.label ?? "文本";
  }

  /** 「文本▼」按钮图标：跟随当前显示格式（文本 / 十六进制 / 图像 / 网页）。 */
  private textFormatIcon(): string {
    return `text_format_${this.textFormat}`;
  }

  /** 仅所属数据页显式打开预览后显示；查询和其它页面没有此开关。 */
  textPreviewActive(tabId = this.activeTabId): boolean {
    const view = tabId ? this.views.get(tabId) : undefined;
    return view instanceof DataTabView && view.previewPinned;
  }

  /** 关闭预览（预览面板的 × 按钮）：回到默认文本显示并刷新工具栏。 */
  closeTextPreview(tabId = this.activeTabId): void {
    const view = tabId ? this.views.get(tabId) : undefined;
    if (!(view instanceof DataTabView) || !view.previewPinned) return;
    view.previewFormat = "text";
    view.previewPinned = false;
    if (tabId === this.activeTabId) {
      this.notifyToolbar();
      this.refreshCellPreviews();
    } else view.updateCellPreview();
  }

  /** 预览面板标题用的格式名（视图层读取）。 */
  textFormatLabelPublic(): string {
    return this.textFormatLabel();
  }

  /** 设置当前数据页的预览格式和开关；仍需有焦点/选中内容才占用高度。 */
  setTextFormat(format: "text" | "hex" | "image" | "web"): void {
    const view = this.activeView();
    if (!(view instanceof DataTabView)) return;
    if (view.previewFormat === format && view.previewPinned) return;
    view.previewFormat = format;
    view.previewPinned = true;
    this.notifyToolbar();
    this.refreshCellPreviews();
  }

  /** 「文本」分体按钮：左侧点击切换预览开关（选中 / 取消选中）。 */
  toggleTextPreview(): void {
    const view = this.activeView();
    if (!(view instanceof DataTabView)) return;
    if (view.previewPinned) this.closeTextPreview();
    else this.setTextFormat(view.previewFormat);
  }

  /** 渲染所属数据页的单元格预览；关闭按钮绑定原页面而非点击时的活动页。 */
  renderPreviewPanel(host: HTMLElement, value: string | null, tabId = this.activeTabId): void {
    const view = tabId ? this.views.get(tabId) : undefined;
    if (!(view instanceof DataTabView) || tabId !== this.activeTabId || !view.previewPinned) {
      host.style.display = "none";
      host.replaceChildren();
      return;
    }
    host.classList.toggle("pinned", this.textPreviewActive(tabId));
    host.replaceChildren();
    const head = document.createElement("div");
    head.className = "cell-preview-head";
    const title = document.createElement("span");
    title.textContent = `预览 · ${this.textFormatLabel(view.previewFormat)}`;
    head.appendChild(title);
    if (this.textPreviewActive(tabId)) {
      const close = document.createElement("button");
      close.type = "button"; close.className = "cell-preview-close"; close.textContent = "×";
      close.title = "关闭预览"; close.setAttribute("aria-label", "关闭单元格内容预览");
      close.addEventListener("click", () => this.closeTextPreview(tabId));
      head.appendChild(close);
    }
    const body = document.createElement("div");
    body.className = "cell-preview-body";
    renderCellPreview(body, value, view.previewFormat);
    host.append(head, body);
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
          { id: "edit-user", label: "编辑用户", icon: "design_table", disabled: !(isMysql && rowSelected) },
          { id: "change-password", label: "修改密码", icon: "editor", disabled: !(isMysql && rowSelected) },
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
      case "viewdesign":
      case "routinedesign":
        return [
          { id: "save", label: "保存", icon: "dbn_post", primary: true },
          { id: "beautify-sql", label: "美化 SQL", icon: "editor" },
          { id: "refresh", label: "重新载入定义", icon: "dbn_refresh", disabled: !!tab.designCreating, disabledReason: tab.designCreating ? "新建对象尚未保存，没有可重新载入的定义" : undefined },
        ];
      case "objects": {
        const view = this.views.get(tab.id);
        return objectTools(tab.objectType, view instanceof ObjectsTabView ? view.selectedObject() : null, this.connectionById(tab.connectionId));
      }
      case "query":
        // 事务入口在查询页：开始后此页执行的 SELECT / UPDATE / DELETE 都在该事务中，提交后才生效。
        return [
          ...this.manualTransactionTools(tab),
          { id: "save", label: "保存", icon: "dbn_post", primary: true },
          { id: "query-builder", label: "查询创建工具", icon: "query_builder" },
          { id: "beautify-sql", label: "美化 SQL", icon: "editor" },
          { id: "code-snippet", label: "代码段", icon: "query_pin_ent_preview_image" },
          { id: "export", label: "导出结果", icon: "data_sync_ent_preview_image" },
        ];
      case "structure": {
        const structure = this.views.get(tab.id);
        const view = structure instanceof StructureTabView ? structure : null;
        const busy = view?.isSavingStructure() ?? false;
        const save: TabToolItem = { id: "save", label: "保存", icon: "dbn_post", primary: true, disabled: busy };
        const section = view?.designerTabName() ?? "字段";
        if (section === "索引") return [save,
          { id: "add-index", label: "添加索引", icon: "dbn_insert", disabled: busy },
          { id: "delete-index", label: "删除索引", icon: "dbn_delete", disabled: busy }];
        if (section === "外键") return [save,
          { id: "add-foreign-key", label: "添加外键", icon: "dbn_insert", disabled: busy },
          { id: "delete-foreign-key", label: "删除外键", icon: "dbn_delete", disabled: busy }];
        if (section === "检查") return [save,
          { id: "add-check", label: "添加检查约束", icon: "dbn_insert", disabled: busy },
          { id: "delete-check", label: "删除检查约束", icon: "dbn_delete", disabled: busy }];
        if (section === "触发器") return [save,
          { id: "add-trigger", label: "添加触发器", icon: "dbn_insert", disabled: busy },
          { id: "delete-trigger", label: "删除触发器", icon: "dbn_delete", disabled: busy }];
        if (section !== "字段") return [save];
        const upReason = view ? view.fieldMoveReason(-1) : "请先等待结构加载完成";
        const downReason = view ? view.fieldMoveReason(1) : "请先等待结构加载完成";
        return [save,
          { id: "add-field", label: "添加字段", icon: "dbn_insert", disabled: busy },
          { id: "insert-field", label: "插入字段", icon: "insert_field", disabled: busy },
          { id: "delete-field", label: "删除字段", icon: "dbn_delete", disabled: busy },
          { id: "move-up", label: "上移", icon: "dbn_prior", disabled: !!upReason, disabledReason: upReason ?? undefined },
          { id: "move-down", label: "下移", icon: "dbn_next", disabled: !!downReason, disabledReason: downReason ?? undefined },
        ];
      }
      default: {
        const view = this.views.get(tab.id);
        const hasRows = view instanceof DataTabView && view.rowCount > 0;
        // 表数据页不设事务入口：表格改动由底部「提交改动」（对勾）直接提交到数据库。
        const tools: TabToolItem[] = [
          { id: "text", label: this.textFormatLabel(), icon: this.textFormatIcon(), active: view instanceof DataTabView && view.previewPinned, split: true },
          { id: "filter", label: "筛选", icon: "data_filter" },
          { id: "sort", label: "排序", icon: "tab_sort" },
          { id: "import", label: "导入", icon: "data_transfer_ent_preview_image" },
          { id: "export", label: "导出", icon: "data_sync_ent_preview_image", disabled: !hasRows },
          {
            id: "data-generation",
            label: "数据生成",
            icon: "data_generation_ent_preview_image",
            disabled: this.connectionById(tab.connectionId)?.read_only ?? false,
          },
          // 最右侧字段选择按钮控制内嵌侧栏；空结果仍可选择字段。
          {
            id: "columns",
            label: "选择显示字段",
            icon: "columns",
            iconOnly: true,
            alignRight: true,
            disabled: view instanceof DataTabView ? view.columnToolDisabled() : true,
          },
        ];
        return tools;
      }
    }
  }

  /** 点击树或对象清单后只更新详情，不隐式连接、不替换树节点。 */
  private updateInfoPanelForNode(node: TreeItem): void {
    if (node.objectDescriptor) { this.showCatalogDetails(node.objectDescriptor); return; }
    if (node.catalogNotice) { this.infoRequest++; this.objectDetailsRequest = null; this.infoPanel.setState({ icon: node.icon, title: node.label, fields: [{ label: "目录读取状态", value: node.catalogNotice }] }); return; }
    if (node.id.startsWith("group:")) {
      this.infoRequest++; this.objectDetailsRequest = null;
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
    const object = node.type === "connection" || node.type === "database" || node.type === "schema" || node.id.endsWith("grp-backup") ? "" : node.label;
    const tableIndex = parts.indexOf("table");
    const viewIndex = parts.indexOf("view");
    const parentTable = tableIndex >= 0 ? parts[tableIndex + 1] : viewIndex >= 0 ? parts[viewIndex + 1] : undefined;
    this.showObjectDetails(cfg, schema, object, node.type, backup
      ? { category: "backup", backupName: node.id.endsWith("grp-backup") ? undefined : node.label }
      : { parentTable });
  }

  showCatalogDetails(descriptor: ObjectDescriptor): void {
    this.objectDetailsRequest = null;
    const request = ++this.infoRequest, state = catalogInfo(descriptor, this.connectionName(descriptor.object.connection_id));
    this.infoPanel.setState(state);
    if (catalogReadReason(descriptor)) return;
    void api.objectDefinition(descriptor.object).then(result => {
      if (request !== this.infoRequest) return;
      if (objectKey(result.object) !== objectKey(descriptor.object)) throw new Error("对象定义回执身份不一致");
      this.infoPanel.setState({ ...state, fields: [...state.fields, { label: "定义完整性", value: result.completeness }, { label: "定义", value: result.definition }] });
    }).catch(error => { if (request === this.infoRequest) this.infoPanel.setState({ ...state, fields: [...state.fields, { label: "定义读取失败", value: String(error) }] }); });
  }

  showObjectDetails(cfg: ConnectionConfig, schema: string, object: string, type: ObjectType,
    options?: import("./objectDetails").ObjectDetailsOptions): void {
    const key = JSON.stringify([cfg, schema, object, type, options?.category ?? "object", options?.backupName ?? null, options?.parentTable ?? null]);
    const previous = this.objectDetailsRequest;
    const sameObject = previous?.key === key && previous.request === this.infoRequest;
    if (sameObject && previous.pending) return;
    const request = ++this.infoRequest;
    const current = { key, request, pending: true };
    this.objectDetailsRequest = current;
    // 同一对象的刷新保留已显示的内容；仅真正切换对象时显示加载态。
    if (!sameObject) this.infoPanel.setState({ icon: type === "connection" ? connIconOf(cfg) : `info_pane_${type}`,
      title: object || schema || cfg.name, subtitle: "读取详细信息", fields: [] });
    void loadObjectDetails(cfg, schema, object, type, options).then((state) => {
      if (request === this.infoRequest) this.infoPanel.setState(state);
    }).catch((e) => {
      if (request === this.infoRequest) this.infoPanel.setState({ icon: "info_pane_database", title: object || schema || cfg.name,
        fields: [{ label: "读取失败", value: String(e) }] });
    }).finally(() => { current.pending = false; });
  }

  /** 右侧信息面板内容。 */
  private updateInfoPanel(): void {
    const tab = this.activeTab;
    if (!tab || !["objects", "backup", "users"].includes(tab.kind) || tab.datasourceStale) {
      this.infoRequest++;
      this.objectDetailsRequest = null;
    }
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
      // 查询页右侧为代码段面板：与「代码段」对话框共用同一数据源（内置 + 自定义），
      // 支持在面板内新增 / 删除自定义代码段。
      const view = this.activeView();
      const refresh = () => this.updateInfoPanel();
      this.infoPanel.setSnippets(querySnippets(), (item) => {
        if (view instanceof QueryTabView) {
          view.editor.insertText(item.sql ?? "");
          view.editor.focus();
        }
      }, { onAdd: () => this.cmdCodeSnippet(refresh), onDelete: (name) => { removeSnippet(name); refresh(); } });
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
        for (const field of info) if (field.label === "数据长度") field.label = "数据库文件大小";
        // 整库指标不是单表大小：说明写在备注行，不塞进字段名的括号里。
        info.push({ label: "统计说明", value: "数据库文件大小是整个数据库文件的大小（单位：字节），不是单表大小。" });
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
        identity: JSON.stringify([tab.connectionId, tab.schema, tab.object]),
        title: tab.object,
        refs: this.connectionRef(tab.connectionId),
        info,
        ddl: c?.ddl ?? "",
        column,
        mode: modeOverride ?? (column ? "column" : undefined),
      });
    };
    build();
    // 当前页先返回：目录、DDL 等次要请求不与首屏争抢串行数据库连接。
    if (!tab.page) return;
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
          entry.schema = await api.getTableSchema(tab.connectionId, tab.schema, tab.object, false);
        } catch {
          /* 结构获取失败留空 */
        }
        this.tableMetaCache.set(key, entry);
        const view = this.views?.get(tab.id);
        if (entry.schema && view instanceof DataTabView && !tab.datasourceStale) view.setColumnMetadata(entry.schema.columns);
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
    // SQLite 平铺树节点（无 /db/ 段）schema 为空：归一为 main，使手动事务绑定、
    // 受控执行命名空间与网格操作使用一致的命名空间（MySQL/PG 空 schema 仍要求显式选择）。
    const effectiveSchema = schema || (cfg.kind === "sqlite" ? "main" : "");
    const existing = this.tabs.find(
      (t) =>
        t.kind === "table" &&
        t.connectionId === connId &&
        t.schema === effectiveSchema &&
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
      schema: effectiveSchema,
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
        const view = this.views.get(tab.id);
        if (view instanceof QueryTabView && !view.editor.getSql()) { view.editor.setSql(def); view.markClean(def); }
        this.renderTabs();
      } catch (e) {
        this.toast("error", `读取定义失败：${String(e)}`);
      }
    }
  }

  /** 打开一个查询标签页（schema 为选中的数据库，下拉框会定位到它）。 */
  async openQueryTab(connId?: string, sql = "", schema = "", connect = true, favorite?: Favorite): Promise<TabState> {
    let cfg: ConnectionConfig | undefined;
    if (connId) cfg = this.connections.find((c) => c.id === connId);
    if (cfg && connect) await this.ensureConnection(cfg.id);
    const tab: TabState = {
      id: `q_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`,
      kind: "query",
      title: favorite?.name ?? (cfg ? `查询 - ${cfg.name}` : "查询 1"),
      connectionId: cfg?.id ?? favorite?.connection_id ?? "",
      connectionName: cfg?.name ?? favorite?.connection_id ?? "",
      schema,
      object: favorite?.name ?? "",
      objectType: "query",
      savedQueryId: favorite?.id,
      datasourceStale: favorite?.connection_id && !cfg ? "保存查询的原连接已不存在，SQL 已保留；请明确选择新连接" : undefined,
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

  /** 打开备份 / 自动任务的空列表页（与参考程序空态一致）。
   *  备份属于对象页的一个类别，复用唯一的对象页，绝不新开第二个对象页。 */
  openEmptyListTab(kind: "backup" | "schedule"): void {
    const title = kind === "backup" ? "备份" : "自动任务";
    if (kind === "backup") {
      const objectTab = this.tabs.find((t) => t.kind === "objects" || t.kind === "backup");
      if (objectTab) {
        if (objectTab.kind !== "backup") {
          this.views.get(objectTab.id)?.destroy();
          this.views.delete(objectTab.id);
          objectTab.kind = "backup";
        }
        if (objectTab.datasourceStale) {
          this.views.get(objectTab.id)?.destroy();
          this.views.delete(objectTab.id);
          delete objectTab.datasourceStale;
        }
        objectTab.selectedRows.clear();
        objectTab.connectionId = "";
        objectTab.connectionName = "";
        objectTab.schema = "";
        objectTab.objectType = "folder";
        objectTab.title = title;
        const index = this.tabs.indexOf(objectTab);
        if (index > 0) {
          this.tabs.splice(index, 1);
          this.tabs.unshift(objectTab);
        }
        this.activeTabId = objectTab.id;
        this.renderTabs();
        return;
      }
    }
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
    // 备份页属于对象页类别，固定在标签条最左；自动任务为独立页，追加到末尾。
    if (kind === "backup") this.tabs.unshift(tab);
    else this.tabs.push(tab);
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
    entries = entries.map(entry => {
      const reason = entry.id?.startsWith("object-") ? this.commandUnavailableReason(entry.id) : null;
      return reason ? { ...entry, disabled: true } : entry;
    });
    if (entries.some(entry => entry.id === "grid-copy")) {
      const grid = (this.activeView() as (TabView & { grid?: DataGrid }) | undefined)?.grid;
      if (grid && this.activeTabId) {
        this.copyContext = { tabId: this.activeTabId, grid };
        this.copyOwner = { tabId: this.activeTabId, kind: "grid", grid };
      }
      entries = [...entries.slice(0, 3), { id: "grid-copy-headers", label: "复制含列名" }, { id: "grid-copy-safe", label: "安全粘贴到表格（转义公式）" }, ...entries.slice(3)];
    }
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
    const view = this.views.get(tab.id);
    await this.guardLeave([tab.id], "重新载入", async () => {
      if (tab.kind === "table" || tab.kind === "structure") this.tableMetaCache.delete(`${tab.connectionId}/${tab.schema}/${tab.object}`);
      if (view instanceof DataTabView || view instanceof StructureTabView) await view.reloadAfterLeave();
      else await view?.reload();
    });
    this.updateInfoPanel();
    this.renderStatus();
  }

  // ------------------------------------------------------------ 状态栏

  /** 实时计时只更新当前数据页的文字，不重建状态栏以免打断按钮焦点。 */
  updateTableQueryElapsed(tabId: string, text: string): void {
    if (this.activeTab?.id !== tabId) return;
    const cell = this.statusBar.querySelector<HTMLElement>(".statusbar-query-elapsed");
    if (cell) cell.textContent = text;
  }

  renderStatus(): void {
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
        mkCell(`${view.filteredCount()} 个${objectLabel(tab.objectType)}`);
        mkCell(cfg?.name ?? "", connIconOf(cfg));
        mkCell(tab.schema, "info_pane_database");
      } else if (view instanceof DataTabView) {
        // 耗时位于右下记录统计之前；查询开始即可展示。
        const info = view.statusInfo();
        mkSpacer();
        const elapsed = mkCell(info.elapsed);
        elapsed.classList.add("statusbar-query-elapsed");
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
        // 查询统计归属结果页底部左侧；应用状态栏不再重复。
        mkSpacer();
      } else {
        mkCell(tab.title, cfg ? connIconOf(cfg) : undefined);
      }
    } else {
      mkCell("就绪");
    }

    mkSpacer();

    // 右：面板开关 2 个
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
    mkToggle(svg(ICON_PANEL_LEFT), "显示/隐藏左侧栏", this.sidebarVisible, () => {
      this.sidebarVisible = !this.sidebarVisible;
      // 与菜单 / 专注模式 / 启动恢复共用同一套显隐机制（display），避免 class 与 display 双轨。
      this.sidebar.classList.remove("hidden");
      this.sidebar.style.display = this.sidebarVisible ? "" : "none";
      this.splitter.style.display = this.sidebarVisible ? "" : "none";
      this.renderStatus();
      this.saveViewPrefs();
    });
    mkToggle(svg(ICON_PANEL_RIGHT), "显示/隐藏信息面板", this.infoPanelVisible, () => {
      this.infoPanelVisible = !this.infoPanelVisible;
      this.infoPanel.setVisible(this.infoPanelVisible);
      this.infoSplitter.style.display = this.infoPanelVisible ? "" : "none";
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

  /** 标题栏任务图标徽标：显示运行中的后台任务数。 */
  private refreshTaskBadge(): void {
    if (!this.taskBadge) return;
    const n = taskCenter.runningCount();
    this.taskBadge.textContent = n > 0 ? String(n) : "";
    this.taskBadge.hidden = n === 0;
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
    if (this.draftPreparing) return "正在处理离页操作，请等待完成";
    // 独立工具先开窗口，再选择自己的连接；不借用当前（可能已失效）的数据库标签。
    if (id === "server-monitor" || id === "history-log") return null;
    const tab = this.activeTab;
    const cfg = this.tree ? this.connectionById(this.currentConnectionId()) : undefined;
    const view = tab ? this.views.get(tab.id) : undefined;
    const query = view instanceof QueryTabView;
    if (query && view.resultBusy() && ["run", "run-selected", "explain", "export"].includes(id)) return "查询执行、缓存读取或导出进行中，请等待完成";
    if (id.startsWith("object-")) {
      if (!tab || !(view instanceof ObjectsTabView)) return "请先打开对象列表";
      const item = objectTools(tab.objectType, view.selectedObject(), this.connectionById(tab.connectionId)).find(i => i.id === id);
      if (!item) return "当前类别不支持此操作";
      if (item.disabled) return item.disabledReason ?? "此操作不可用";
      if (tab.objectType === "query") return null;
      if (tab.datasourceStale) return tab.datasourceStale;
      if (["object-new", "object-new-procedure", "object-delete", "object-import"].includes(id)) {
        if (this.manualTransactionFor(tab.connectionId) || this.manualTransactionIsBusy(tab.connectionId)) return "请先完成此连接的手动事务";
      }
      return null;
    }
    if (tab?.datasourceStale) {
      const localCommands = new Set(["options", "about", "official-doc", "shortcut-help", "driver-manager", "new-connection", "exit", "next-window", "close-window", "close-others", "close-all", "tab-close", "tab-close-others", "tab-close-left", "tab-close-right", "tab-close-all", "toggle-sidebar", "toggle-statusbar", "toggle-toolbar", "toggle-info-panel", "focus-mode", "manage-favorites"]);
      const editorCommands = new Set(["save", "save-as", "undo", "redo", "cut", "copy", "paste", "find", "find-next", "replace", "select-all", "beautify-sql", "code-snippet", "add-favorite"]);
      const designEditor = tab?.kind === "viewdesign" || tab?.kind === "routinedesign";
      const localEditor = id === "beautify-sql" ? query || designEditor : query;
      if (!localCommands.has(id) && !(localEditor && editorCommands.has(id))) return tab.datasourceStale;
    }
    const data = view instanceof DataTabView;
    const selectedCommands = ["data-sync", "data-generation", "import", "tree-import", "transfer", "data-transfer", "new-table", "tree-new-table", "delete-table", "truncate-table", "restore-backup", "connection-properties", "remove-connection", "open-connection"];
    const writeConnection = selectedCommands.includes(id) ? cfg?.id || tab?.connectionId || "" : tab?.connectionId || cfg?.id || "";
      if (MANUAL_TRANSACTION_WRITE_COMMANDS.has(id) || (id === "save" && !query)) {
        if (this.manualTransactionFor(writeConnection)) return "此连接有手动事务，请先在所属页面提交或回滚，再使用自动提交写入工具";
        if (this.manualTransactionIsBusy(writeConnection)) return "此连接的事务操作正在进行";
      }
      const transaction = this.manualTransactionFor(tab?.connectionId || "");
      if (query && ["run", "run-selected", "explain"].includes(id)) {
        if (this.manualTransactionIsBusy(tab?.connectionId || "")) return "事务操作正在进行，请等待完成";
        if (transaction?.failed) return "事务执行失败，必须回滚后才能继续执行";
        if (transaction) {
          const ownedByThisPage = transaction.owner === tab?.id && transaction.schema === tab?.schema;
          if (!ownedByThisPage) return "此连接的事务由其它页面持有，请在该页面提交或回滚后再执行";
          // EXPLAIN 不属于手动事务允许的查询/DML 语句，事务期间禁用解释。
          if (id === "explain") return "手动事务期间不能解释 SQL，请先提交或回滚事务";
        }
      }
    switch (id) {
      case "begin-transaction": {
        if (!query || !tab) return "请在查询页面开始事务";
        const reason = manualTransactionUnavailableReason(this.connectionById(tab.connectionId));
        if (reason) return reason;
        if (this.manualTransactionIsBusy(tab.connectionId)) return "事务操作正在进行";
        if (transaction) return "此连接已有手动事务，请先在所属页面提交或回滚";
        if (view instanceof QueryTabView && view.isExecuting()) return "SQL正在执行，请等待完成";
        return null;
      }
      case "commit-transaction":
      case "rollback-transaction": {
        if (!(query || data) || !tab) return "请返回事务所属页面";
        if (this.manualTransactionIsBusy(tab.connectionId) || (view instanceof DataTabView && view.committing) || (view instanceof QueryTabView && view.isExecuting())) return "SQL或事务操作正在进行";
        if (!transaction) return "此连接没有手动事务";
        if (transaction.owner !== tab.id || transaction.schema !== tab.schema) return "此事务由另一个页面或模式持有";
        return id === "commit-transaction" && transaction.failed ? "事务执行失败，必须回滚，不能提交" : null;
      }
      // data-sync / data-generation 是全局工具：不因当前选中的库不支持而禁用
      // 按钮（runCommand 里回落到受支持连接；对话框内的数据源下拉会过滤掉
      // 不支持的连接），否则用户换一个要同步的库都进不去工具。
      case "open-user":
      case "open-connection":
      case "close-connection":
      case "connection-properties":
      case "remove-connection":
        return cfg ? null : "请先选择一个连接";
      case "save":
        if (query) return null;
        if (!(data || view instanceof StructureTabView || tab?.kind === "viewdesign" || tab?.kind === "routinedesign")) return "当前页面没有可保存的内容";
        if (tab && (tab.kind === "viewdesign" || tab.kind === "routinedesign")) return objectCapability(this.connectionById(tab.connectionId), tab.objectType, tab.designCreating ? "create" : "update");
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
      case "code-snippet":
      case "add-favorite":
        return query ? null : "此命令仅适用于查询编辑器";
      case "beautify-sql":
        // 不在例程执行 / 重读期间改写草稿；其它页面的美化只做本地排版。
        if (view instanceof RoutineTabView && view.draftBusy()) return "例程正在执行或重新载入，请稍后再美化 SQL";
        return query || tab?.kind === "viewdesign" || tab?.kind === "routinedesign"
          ? null
          : "此命令仅适用于查询编辑器或视图 / 例程设计器";
      case "delete-row":
      case "grid-delete":
      case "grid-insert":
      case "grid-commit":
        if (!data || tab?.objectType !== "table") return "此命令需要可编辑的数据表页面（不能写入视图或查询结果）";
        return this.connectionById(tab.connectionId)?.read_only ? "此连接只读，不能修改记录" : null;
      case "grid-rollback":
        return data ? null : "当前页面没有数据表待提交改动";
      case "grid-copy":
      case "grid-copy-row":
      case "grid-copy-cell":
      case "grid-copy-headers":
      case "grid-copy-safe": {
        const grid = (view as (TabView & { grid?: DataGrid }) | undefined)?.grid;
        if (!grid) return "当前页面没有结果网格";
        try {
          const context = this.copyContext;
          if (context && context.tabId !== tab?.id) return "菜单所属页面已变化，请重新选择";
          const snapshot = grid.freezeCopy(!!context);
          return id === "grid-copy-cell" ? snapshot.cell === undefined ? "没有可复制的单元格" : null : snapshot.rows.length && snapshot.columns.length ? null : "请先选择已加载的行";
        } catch (e) { return String(e); }
      }
      case "copy":
      case "select-all":
        return query || data || view instanceof ObjectsTabView || view instanceof SqlDesignTabView ? null : "当前页面没有可复制或选择的内容";
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
      if (fav) void this.openSavedQuery(fav.id);
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
      if (id.startsWith("object-") && id !== "object-search") { await this.runObjectAction(id); return; }
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
          await this.cmdNewQuery();
          break;
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
          openServerMonitorDialog(this.connections, cfg, api, id => this.ensureConnection(id));
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
        case "data-dictionary": {
          const cfg = this.connectionById(this.currentConnectionId());
          const schema = this.currentDbTarget()?.schema ?? "";
          openDataDictionaryDialog(this.connections, cfg, schema, (path) => {
            this.toast("success", `数据字典已导出：${path}`);
          });
          break;
        }
        case "er-diagram": {
          const cfg = this.connectionById(this.currentConnectionId());
          const schema = this.currentDbTarget()?.schema ?? "";
          openErDiagramDialog(this.connections, cfg, schema);
          break;
        }
        case "check-update":
          openUpdateDialog();
          break;
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
        case "header-pin-toggle": {
          const col = this.headerSortColumn;
          this.headerSortColumn = null;
          const view = this.activeView();
          if (col && view instanceof DataTabView) view.togglePinnedColumn(col);
          break;
        }
        case "header-sort-asc":
        case "header-sort-desc":
        case "header-sort-remove": {
          const col = this.headerSortColumn;
          this.headerSortColumn = null;
          const view = this.activeView();
          if (col && view instanceof DataTabView) {
            view.applyHeaderSort(col, id === "header-sort-asc" ? "asc" : id === "header-sort-desc" ? "desc" : "remove");
          }
          break;
        }
        case "columns":
          this.cmdColumns();
          break;
        case "object-search":
          this.cmdObjectSearch();
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
          this.cmdDump(id === "tree-dump");
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
        case "grid-copy-headers":
        case "grid-copy-safe":
          await this.cmdCopy(id === "grid-copy-row" ? "rows" : id === "grid-copy-cell" ? "cell" : id === "grid-copy-headers" ? "rows_with_headers" : id === "grid-copy-safe" ? "spreadsheet_safe" : "default", id !== "copy");
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
          // 查询页事务开始后刷新工具栏（出现提交 / 回滚按钮）。
          this.notifyToolbar();
          break;
        case "commit-transaction":
          await this.manualTransactionController.finish(true);
          await this.refreshDataTabs(this.activeTab?.connectionId ?? "");
          break;
        case "rollback-transaction":
          await this.manualTransactionController.finish(false);
          await this.refreshDataTabs(this.activeTab?.connectionId ?? "");
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
          if (this.activeTabId) await this.closeTab(this.activeTabId);
          break;
        case "close-others":
        case "tab-close-others":
          await this.cmdCloseOthers();
          break;
        case "tab-close-left":
          await this.cmdCloseSide("left");
          break;
        case "tab-close-right":
          await this.cmdCloseSide("right");
          break;
        case "close-all":
        case "tab-close-all":
          await this.cmdCloseAll();
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
          const namespace = namespaceName(this.connectionById(connId));
          if (!connId || !db) {
            this.toast("error", `请先在左侧树中选择一个${namespace}`);
            break;
          }
          const dbNode = this.tree.find(`conn:${connId}/db/${db}`);
          if (dbNode) {
            if (!await this.openDatabaseNode(dbNode, connId, db)) break;
          } else {
            this.openDbs.add(`${connId}/${db}`);
            this.openConnections.add(connId);
            this.syncConnIcons();
            this.openObjectsTab(connId, db);
          }
          this.toast("success", `${namespace}「${db}」已打开`);
          break;
        }
        case "db-close": {
          const db = this.currentDbName();
          const connId = this.currentConnectionId();
          const namespace = namespaceName(this.connectionById(connId));
          if (!connId || !db) {
            this.toast("error", `请先在左侧树中选择一个${namespace}`);
            break;
          }
          if (await this.closeDatabaseNode(connId, db)) this.toast("info", `${namespace}「${db}」已关闭`);
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
        case "copy-table-structure":
          this.cmdCopyTable("copy_structure");
          break;
        case "copy-table-data":
          this.cmdCopyTable("copy_data");
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
          if (this.activeTabId) await this.closeTab(this.activeTabId);
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
        case "add-index":
          this.cmdAddIndex();
          break;
        case "delete-index":
          this.cmdDeleteIndex();
          break;
        case "add-foreign-key":
          this.cmdAddForeignKey();
          break;
        case "delete-foreign-key":
          this.cmdDeleteForeignKey();
          break;
        case "add-check":
          this.cmdAddCheck();
          break;
        case "delete-check":
          this.cmdDeleteCheck();
          break;
        case "add-trigger":
          this.cmdAddTrigger();
          break;
        case "delete-trigger":
          this.cmdDeleteTrigger();
          break;
        case "move-up":
        case "move-down": {
          const view = this.activeView();
          if (view instanceof StructureTabView) view.moveField(id === "move-up" ? -1 : 1);
          else this.toast("error", "请先打开表结构并选择字段");
          break;
        }
        case "text": {
          this.toggleTextPreview();
          break;
        }
        case "text-format-plain":
        case "text-format-hex":
        case "text-format-image":
        case "text-format-web": {
          const next = ({
            "text-format-plain": "text",
            "text-format-hex": "hex",
            "text-format-image": "image",
            "text-format-web": "web",
          } as const)[id];
          // 再点一次当前格式（含钉住的文本预览）= 关闭预览面板，回到默认不占高度。
          if (this.textPreviewActive() && next === this.textFormat) this.closeTextPreview();
          else this.setTextFormat(next);
          break;
        }
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
        case "rename-backup":
          this.cmdRenameBackup();
          break;
        case "batch-execution-history":
          openBatchExecutionDialog();
          break;
        case "tree-run-sql-file":
          await this.cmdRunSqlFile();
          break;
        // 视图 / 函数设计器：保存 = 重建对象
        case "tree-design-view":
          await this.cmdTreeDesignView();
          break;
        case "tree-call-routine": {
          const descriptor = this.tree.find(this.tree.selectedNodeId ?? "")?.objectDescriptor;
          if (descriptor) await this.openCatalogObject(descriptor, true);
          break;
        }
        case "tree-design-routine":
          await this.cmdTreeDesignRoutine();
          break;
        // 自动任务
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
          this.cmdEditUser();
          break;
        case "new-user":
          this.cmdNewUserReal();
          break;
        case "change-password":
          this.cmdEditUserPassword();
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
    // 统一刷新按钮并保留稳定的对象搜索插槽。
    this.notifyToolbar();
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
    openConnectionDialog(null, this.groups, async ({ config, password, runtimeProfile }) => {
      config.group = group;
      this.connections = await api.saveConnection(config, password, runtimeProfile);
      this.groups = await api.listGroups();
      this.refreshTree();
      this.toast("success", `连接「${config.name}」已创建`);
    }, kind, driver, this.runtimeProfiles);
  }

  private cmdEditConnection(): void {
    const cfg = this.connectionById(this.currentConnectionId());
    if (!cfg) {
      this.toast("error", "请先在左侧树中选择一个连接");
      return;
    }
    openConnectionDialog(cfg, this.groups, async ({ config, password, runtimeProfile }) => {
      const changed = invalidatedDatasourceIds([cfg], [config]).has(cfg.id) || !!password || !!runtimeProfile;
      const save = async () => {
        this.connections = await api.saveConnection(config, password, runtimeProfile);
        if (changed) {
          this.stopKeepAlive(cfg.id); this.openConnections.delete(cfg.id);
          this.suspendDatasourceTabs(new Set([cfg.id]), "连接端点或安全配置已修改，旧页暂停。请重新验证对象后再打开；SQL 稿可保存本机。");
        }
        this.groups = await api.listGroups(); this.refreshTree(); this.toast("success", "连接属性已保存");
      };
      if (changed) {
        const ids = this.tabs.filter(tab => tab.connectionId === cfg.id).map(tab => tab.id);
        if (!await this.guardLeave(ids, "修改连接配置", save)) throw new Error("连接修改已取消，原配置和编辑保留");
      } else await save();
    }, cfg.kind, undefined, this.runtimeProfiles);
  }

  /** 复制连接：在原配置基础上打开「新建连接」对话框。 */
  private cmdDuplicateConnection(): void {
    const cfg = this.connectionById(this.currentConnectionId());
    if (!cfg) {
      this.toast("error", "请先在左侧树中选择一个连接");
      return;
    }
    const copy: ConnectionConfig = { ...cfg, id: "", name: `${cfg.name} - 副本` };
    openConnectionDialog(copy, this.groups, async ({ config, password, runtimeProfile }) => {
      this.connections = await api.saveConnection(config, password, runtimeProfile);
      this.groups = await api.listGroups();
      this.refreshTree();
      this.toast("success", `连接「${config.name}」已创建`);
    }, cfg.kind, undefined, this.runtimeProfiles);
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
          onClick: async () => {
            const ids = this.tabs.filter(tab => tab.connectionId === cfg.id).map(tab => tab.id);
            return await this.guardLeave(ids, "移除连接", async () => {
              this.connections = await api.deleteConnection(cfg.id);
              this.openConnections.delete(cfg.id);
              this.stopKeepAlive(cfg.id);
              this.suspendDatasourceTabs(new Set([cfg.id]), "连接已移除，页面已暂停；不能自动绑定同名连接。");
              this.refreshTree();
              this.toast("success", `连接「${cfg.name}」已移除`);
            });
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
        this.startKeepAlive(connId);
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
    const ids = this.tabs.filter(tab => tab.connectionId === connId).map(tab => tab.id);
    if (!await this.guardLeave(ids, "断开连接", async () => {
      if (!await this.manualTransactionController.closeConnection(connId)) throw new Error("断连或事务回滚已取消 / 失败");
      this.suspendDatasourceTabs(new Set([connId]), "连接已断开，旧页暂停。SQL 编辑仍保留；重新选择连接才会重连。");
    })) return;
    this.openConnections.delete(connId);
    this.stopKeepAlive(connId);
    for (const key of [...this.openDbs]) {
      if (key.startsWith(`${connId}/`)) this.openDbs.delete(key);
    }
    // 收起并清空该连接已加载的库列表：后端连接已真实关闭，
    // 树中残留的库节点既不真实也不可交互（双击会静默重连）。
    const node = this.tree.find(`conn:${connId}`);
    if (node) {
      this.treeLoads.delete(node);
      node.loading = false;
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
  /** 表头右键排序的当前列（弹出菜单前设置，命令执行后清除）。 */
  headerSortColumn: string | null = null;
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

  /** 展开数据网格右侧字段操作栏。 */
  private cmdColumns(): void {
    const tab = this.activeTab;
    const view = this.activeView();
    if (!(view instanceof DataTabView) || !tab) {
      this.toast("error", "只有表数据页支持列设置");
      return;
    }
    const trigger = this.tabToolbar.el.querySelector<HTMLButtonElement>('[data-command="columns"]')
      ?? document.elementFromPoint(this.lastMouse.x, this.lastMouse.y)?.closest<HTMLButtonElement>('[data-command="columns"]');
    view.toggleColumnSidebar(trigger ?? undefined);
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
    input.type = "text";
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
    if (!t?.schema) {
      this.toast("error", "请先在左侧树中选择一个数据库");
      return;
    }
    switch (grp) {
      case "query":
        this.openObjectsTab(t.connId, t.schema, "query");
        break;
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
    const input = textInput();
    input.style.width = "100%";
    input.placeholder = "例如：开发环境";
    input.setAttribute("aria-label", "分组名称");
    new Dialog({
      title: "新建组",
      width: 380,
      render: (body) => {
        const row = field("分组名:", input);
        // 标签列自适应文字宽度：单字段弹窗左右留白都等于内容区 padding，
        // 不再出现“左侧空隙大、输入框贴右”的不对称。
        row.style.setProperty("--label-w", "auto");
        row.style.marginBottom = "0";
        body.appendChild(row);
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

  /** 打开日志窗口（数据源筛选取自已保存连接）。 */
  private cmdShowLog(): void {
    openHistoryLogDialog(api, this.connections);
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

  /** 命名保存当前查询：保存为当前数据库下的查询对象。 */
  private cmdSaveQuery(asNew = false): void {
    const tab = this.activeTab;
    const view = this.activeView();
    if (!(tab && view instanceof QueryTabView)) return;
    const sql = view.editor.getSql();
    if (!sql.trim()) {
      this.toast("error", "查询内容为空");
      return;
    }
    if (!this.connectionById(tab.connectionId) || !tab.schema.trim()) {
      this.toast("error", "保存查询前请选择连接和数据库");
      return;
    }
    const existing = asNew ? undefined : this.favorites.find(f => f.id === tab.savedQueryId);
    const binding = { connectionId: tab.connectionId, schema: tab.schema };
    openObjectAction({
      title: asNew ? "查询另存为" : "保存查询", label: "查询名称", value: existing?.name ?? tab.object,
      context: `${tab.connectionName} / ${tab.schema}`,
      confirm: "保存", submit: async name => {
        if (tab.connectionId !== binding.connectionId || tab.schema !== binding.schema) throw new Error("编辑器上下文已变化，请关闭后重新保存");
        if (!this.connectionById(binding.connectionId)) throw new Error("连接已不存在，请重新选择连接和数据库");
        const source = view.editor.getSql();
        const favorite = queryFavorite(this.favorites, { id: asNew ? undefined : tab.savedQueryId, name, sql: source, ...binding });
        const items = await api.addFavorite(favorite);
        tab.savedQueryId = favorite.id;
        view.markClean(source);
        this.publishFavorites(items, favorite.id);
        this.toast("success", `查询「${name}」已保存`);
      },
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

  private async cmdCopy(mode: CopyMode = "default", explicitGrid = false): Promise<void> {
    const tab = this.activeTab;
    if (!tab) return;
    const view = this.views.get(tab.id);
    const context = this.copyContext;
    if (view instanceof QueryTabView) {
      const controlled = view.controlledCopyText(mode, explicitGrid);
      if (controlled !== null) { await this.writeClipboard(controlled, "已复制当前可见的类型化结果；隐藏列未包含"); return; }
    }
    if (explicitGrid && context && context.tabId !== tab.id) throw new Error("复制菜单所属页面已变化，请重新选择");
    const owner = this.copyOwner;
    if (explicitGrid || (owner?.tabId === tab.id && owner.kind === "grid") || (!(view instanceof QueryTabView || view instanceof SqlDesignTabView) && (view as TabView & { grid?: DataGrid })?.grid)) {
      const grid = explicitGrid && context ? context.grid : owner?.tabId === tab.id ? owner.grid : (view as TabView & { grid?: DataGrid })?.grid;
      if (!grid) { this.toast("info", "没有可复制的网格"); return; }
      const snapshot = grid.freezeCopy(explicitGrid && !!context);
      await this.writeClipboard(encodeGridCopy(snapshot, mode), mode === "cell" ? "已复制单元格" : `已复制 ${snapshot.rows.length} 行 / ${snapshot.columns.length} 列（仅已加载范围）`);
      return;
    }
    const editor = view instanceof QueryTabView ? view.editor : view instanceof SqlDesignTabView ? view.copyEditor() : undefined;
    if (!editor?.hasSelection()) { this.toast("info", "编辑器没有选中文本，请选择内容后复制"); return; }
    await this.writeClipboard(editor.getSelection());
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

  private async writeClipboard(text: string, success = "已复制到剪贴板"): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
      this.toast("success", success);
    } catch {
      showDraftText("剪贴板写入失败", text, "系统剪贴板未写入。请在下方选中文本并用 Ctrl+C 手动复制；不会保存复制历史。TSV 的 NULL 为 \\N，文本 \\N 带引号；单格纯文本不能保证类型可逆。");
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
    this.cmdSaveQuery(asNew);
  }

  savedQueries(connectionId: string, schema: string): Favorite[] {
    if (this.favoritesError) throw new Error(this.favoritesError);
    return savedQueriesFor(this.favorites, connectionId, schema);
  }

  async openSavedQuery(id: string): Promise<void> {
    const favorite = this.favorites.find(f => f.id === id);
    if (!favorite) throw new Error("此查询已被删除，请刷新列表");
    if (!hasSavedQueryScope(favorite)) {
      const tab = await this.openQueryTab(favorite.connection_id ?? undefined, favorite.sql, "", false);
      tab.object = favorite.name; tab.title = `${favorite.name}（未保存）`;
      tab.dirty = favorite.sql.length > 0;
      const view = this.views.get(tab.id);
      if (view instanceof QueryTabView) view.detachSavedQuery();
      this.renderTabs();
      this.toast("info", "旧版 SQL 已作为未保存查询打开，请选择数据库后保存");
      return;
    }
    const existing = this.tabs.find(t => t.kind === "query" && t.savedQueryId === id);
    if (existing) { this.activeTabId = existing.id; this.renderTabs(); return; }
    const tab = await this.openQueryTab(favorite.connection_id ?? undefined, favorite.sql, favorite.schema ?? "", false, favorite);
    tab.savedQueryId = favorite.id; tab.title = favorite.name; tab.object = favorite.name;
    this.renderTabs();
  }

  private publishFavorites(items: Favorite[], changedId?: string): void {
    this.favoritesError = "";
    this.favorites = items;
    for (const tab of this.tabs) {
      if (!tab.savedQueryId) continue;
      const favorite = items.find(f => f.id === tab.savedQueryId);
      if (!favorite || !hasSavedQueryScope(favorite)) {
        delete tab.savedQueryId;
        const view = this.views.get(tab.id);
        if (view instanceof QueryTabView) view.detachSavedQuery();
      } else if (favorite.id === changedId) { tab.title = favorite.name; tab.object = favorite.name; }
    }
    this.refreshQueryGroups();
    for (const view of this.views.values()) if (view instanceof ObjectsTabView) void view.reload();
    this.renderTabs();
  }

  renameSavedQuery(id: string): void {
    const favorite = this.favorites.find(f => f.id === id);
    if (!favorite || !hasSavedQueryScope(favorite)) return;
    openObjectAction({ title: "重命名查询", context: `${favorite.name} · ${favorite.schema}`, label: "查询名称", value: favorite.name, confirm: "重命名", submit: async name => {
      const current = this.favorites.find(f => f.id === id);
      if (!current) throw new Error("查询已被删除");
      const next = { ...current, name, timestamp: Date.now() };
      if (savedQueryConflict(this.favorites, next)) throw new Error("同一归属中已有同名查询");
      this.publishFavorites(await api.addFavorite(next), id);
    } });
  }

  deleteSavedQuery(id: string, after?: () => void): void {
    const favorite = this.favorites.find(f => f.id === id);
    if (!favorite) return;
    openObjectAction({ title: hasSavedQueryScope(favorite) ? "删除查询" : "删除未保存 SQL", context: `删除本地 SQL「${favorite.name}」（${hasSavedQueryScope(favorite) ? favorite.schema : "未保存 SQL"}）？已打开的 SQL 文本将保留。`, confirm: "删除", submit: async () => {
      this.publishFavorites(await api.removeFavorite(id)); after?.();
    } });
  }

  /** 打开收藏的查询：弹出收藏清单，点击即打开。 */
  private cmdOpenFavorite(): void {
    if (this.favoritesError) { this.toast("error", this.favoritesError); return; }
    if (!this.favorites.length) {
      this.toast("info", "收藏夹为空");
      return;
    }
    const items: MenuEntry[] = this.favorites.map((f) => ({
      id: `fav-open:${f.id}`,
      label: hasSavedQueryScope(f) ? f.name : `${f.name}（未保存 SQL）`,
      icon: "object_star",
    }));
    items.push({ separator: true });
    items.push({ id: "manage-favorites", label: "管理收藏夹..." });
    this.menu.showContext(items, 80, this.menuHost.getBoundingClientRect().bottom + 2);
  }

  private cmdManageFavorites(): void {
    const dialog = openFavoritesDialog(
      this.favorites.map(f => hasSavedQueryScope(f) ? f : { ...f, name: `${f.name}（未保存 SQL）` }),
      (id) => {
        this.deleteSavedQuery(id, () => { dialog.close(); this.cmdManageFavorites(); });
      },
      (id) => {
        const fav = this.favorites.find((f) => f.id === id);
        if (fav) void this.openSavedQuery(fav.id);
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
        if (view instanceof DataTabView) { view.prepareForTransaction(); return; }
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
  private async closeManualTransactionTab(tab: TabState): Promise<boolean> {
    const context = this.transactionContext(tab.id);
    return context ? this.manualTransactionController.closeOwner(context) : false;
  }
  private manualTransactionTools(tab: TabState): TabToolItem[] {
    const beginReason = this.commandUnavailableReason("begin-transaction");
    const status = this.manualTransactionFor(tab.connectionId);
    const items: TabToolItem[] = [{
      id: "begin-transaction", label: "开始事务", icon: "dbn_applyupdates", disabled: !!beginReason,
      disabledReason: beginReason ?? undefined,
    }];
    // 事务由本查询页持有时，就地提供提交 / 回滚（此后执行的 SQL 都在该事务连接上生效）。
    if (status?.active && status.owner === tab.id && status.schema === tab.schema) {
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
    if (!schema) { this.toast("error", `请先在查询编辑器选择${namespaceName(connection)}，等待元数据加载完成`); return; }
    openQueryBuilderDialog(connection, schema, sql => {
      if (this.activeTab?.id !== tab.id || this.views.get(tab.id) !== view || tab.connectionId !== connection.id || tab.schema !== schema) {
        throw new Error(`目标查询页、连接或${namespaceName(connection)}已改变，请关闭并重新打开查询创建工具。`);
      }
      view.editor.replaceSelection(`\n${sql}\n`);
      queueMicrotask(() => view.editor.focus());
    });
  }

  /** 美化当前 SQL：查询页与视图 / 例程设计器共用同一格式化器。 */
  private async cmdBeautifySql(): Promise<void> {
    const tab = this.activeTab;
    const editor = this.activeSqlEditor();
    if (!tab || !editor) {
      this.toast("error", "请先打开查询页或视图 / 例程设计器");
      return;
    }
    const sql = editor.getSql();
    if (!sql.trim()) {
      this.toast("error", "当前编辑器没有可美化的 SQL");
      return;
    }
    const protocol = objectProtocol(this.connectionById(tab.connectionId));
    const dialect = protocol === "mysql" || protocol === "postgres" || protocol === "sqlite" ? protocol : undefined;
    const beautified = formatSql(sql, { dialect });
    if (beautified === sql) {
      this.toast("info", "SQL 已是美化后的排版");
      editor.focus();
      return;
    }
    editor.setSql(beautified);
    editor.focus();
    this.toast("success", "SQL 已美化");
  }

  /** 当前页面可美化的 SQL 编辑器（查询页 / 视图设计 / 例程设计）。 */
  private activeSqlEditor(): SqlEditor | undefined {
    const tab = this.activeTab;
    if (!tab) return undefined;
    const view = this.views.get(tab.id);
    if (view instanceof QueryTabView) return view.editor;
    if (view instanceof SqlDesignTabView) return view.copyEditor();
    if (view instanceof RoutineTabView) return view.copyEditor();
    return undefined;
  }

  /** 代码段对话框：与右侧信息面板共用同一数据源（内置 + 自定义），可新增定义。 */
  private cmdCodeSnippet(afterChange?: () => void): void {
    const tab = this.activeTab;
    const view = tab ? this.views.get(tab.id) : undefined;
    if (!(view instanceof QueryTabView)) {
      this.toast("error", "请先打开查询标签页");
      return;
    }
    let selected = querySnippets()[0]?.sql ?? "";
    new Dialog({
      title: "代码段",
      width: 620,
      render: (body) => {
        const row = document.createElement("div");
        row.style.display = "flex";
        row.style.gap = "10px";
        const side = document.createElement("div");
        side.style.cssText = "width:190px;display:flex;flex-direction:column;gap:6px;";
        const list = document.createElement("div");
        list.style.cssText = "flex:1;border:1px solid var(--border);background:var(--bg-surface);overflow:auto;height:250px;";
        const ta = document.createElement("textarea");
        ta.style.cssText = "flex:1;height:250px;font-family:var(--font-mono);";
        ta.value = selected;
        ta.setAttribute("aria-label", "代码段 SQL 内容");
        ta.addEventListener("input", () => { selected = ta.value; });
        let draftInput: HTMLInputElement | null = null;
        const refreshList = () => {
          const draft = draftInput?.closest(".snippet-name-draft");
          list.replaceChildren();
          for (const snippet of querySnippets()) {
            const item = document.createElement("div");
            item.style.cssText = "padding:4px 8px;cursor:default;display:flex;align-items:center;gap:4px;";
            const name = document.createElement("button"); name.type = "button";
            name.textContent = snippet.name;
            name.style.cssText = "flex:1;min-width:0;padding:0;text-align:left;border:none;background:transparent;color:inherit;font:inherit;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;";
            name.addEventListener("click", () => { ta.value = snippet.sql; selected = snippet.sql; });
            item.appendChild(name);
            if (snippet.custom) {
              const del = document.createElement("button"); del.type = "button";
              del.setAttribute("aria-label", `删除代码段 ${snippet.name}`);
              del.textContent = "×";
              del.title = "删除此自定义代码段";
              del.style.cssText = "cursor:pointer;color:var(--fg-muted);padding:0 3px;border:none;background:transparent;font:inherit;border-radius:3px;";
              del.addEventListener("mousedown", e => e.stopPropagation());
              del.addEventListener("click", (e) => {
                e.stopPropagation();
                removeSnippet(snippet.name);
                refreshList();
                afterChange?.();
              });
              item.appendChild(del);
            }
            item.addEventListener("mousedown", () => {
              list.querySelectorAll("div").forEach((x) => (x.style.background = ""));
              item.style.background = "var(--bg-selected-soft)";
              ta.value = snippet.sql;
              selected = snippet.sql;
            });
            list.appendChild(item);
          }
          if (draft) list.appendChild(draft);
        };
        refreshList();
        // 新增自定义代码段：与右侧信息面板一致（共用 snippets 存储）。
        const addBtn = document.createElement("button");
        addBtn.type = "button";
        addBtn.className = "btn";
        addBtn.textContent = "＋ 新增代码段";
        addBtn.addEventListener("click", () => {
          if (draftInput) { draftInput.focus(); return; }
          const draft = document.createElement("div");
          draft.className = "snippet-name-draft";
          draft.style.cssText = "padding:6px 8px;background:var(--bg-selected-soft);";
          const input = document.createElement("input");
          draftInput = input;
          input.type = "text"; input.placeholder = "代码段名称"; input.required = true;
          input.setAttribute("aria-label", "新增代码段名称");
          input.style.cssText = "width:100%;min-width:0;box-sizing:border-box;";
          const error = document.createElement("div");
          error.id = "snippet-name-error-" + Date.now().toString(36);
          error.setAttribute("role", "status"); error.setAttribute("aria-live", "polite");
          error.style.cssText = "color:var(--danger-500);font-size:var(--fs-11);overflow-wrap:anywhere;";
          input.setAttribute("aria-describedby", error.id);
          const actions = document.createElement("div");
          actions.style.cssText = "display:flex;gap:4px;margin-top:4px;";
          const confirm = document.createElement("button"); confirm.type = "button";
          confirm.className = "btn"; confirm.textContent = "保存";
          const cancel = document.createElement("button"); cancel.type = "button";
          cancel.className = "btn"; cancel.textContent = "取消";
          const finish = () => { draftInput = null; draft.remove(); addBtn.focus(); };
          const save = () => {
            try {
              const snippet = addSnippet({ name: input.value, tag: "自定义", desc: "自定义代码段", sql: ta.value.trim() });
              selected = snippet.sql;
              draftInput = null; refreshList(); afterChange?.(); addBtn.focus();
              this.toast("success", "代码段「" + snippet.name + "」已添加");
            } catch (e) {
              error.textContent = String(e); input.setAttribute("aria-invalid", "true"); input.focus();
            }
          };
          input.addEventListener("input", () => { error.textContent = ""; input.removeAttribute("aria-invalid"); });
          input.addEventListener("keydown", e => {
            e.stopPropagation();
            if (e.isComposing || e.keyCode === 229) return;
            if (e.key === "Enter") { e.preventDefault(); save(); }
            if (e.key === "Escape") { e.preventDefault(); finish(); }
          });
          confirm.addEventListener("click", save); cancel.addEventListener("click", finish);
          actions.append(confirm, cancel); draft.append(input, error, actions); list.append(draft);
          input.focus(); input.scrollIntoView({ block: "nearest" });
        });
        side.append(list, addBtn);
        row.append(side, ta);
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
      return ((!this.draftSyncApproved && this.draftPreparing) ||
        (!this.draftSyncApproved && (view?.draftDirty?.() ?? ((view instanceof DataTabView && view.hasPendingChanges()) || (tab.kind !== "query" && tab.dirty)))) ||
        (view?.draftBusy?.() ?? (view instanceof QueryTabView && view.isExecuting())));
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

  private suspendDatasourceTabs(invalidated: Set<string>, message: string, schema?: string): void {
    for (const tab of this.tabs) {
      if (!invalidated.has(tab.connectionId) || (schema !== undefined && tab.schema !== schema)) continue;
      if (tab.datasourceStale) { tab.datasourceStale = message; continue; }
      tab.datasourceStale = message;
      const view = this.views.get(tab.id);
      if (view instanceof QueryTabView) view.invalidateDatasource();
      else if (view) {
        const old = document.createElement("div"); old.className = "datasource-stale-content"; old.inert = true;
        while (view.el.firstChild) old.append(view.el.firstChild);
        const warning = document.createElement("p"); warning.className = "datasource-stale-warning"; warning.setAttribute("role", "status"); warning.textContent = tab.datasourceStale;
        view.el.append(warning, old);
      }
    }
    this.renderTabs();
  }

  private cmdOptions(initialTab?: "general" | "font" | "sql" | "sync"): void {
    openSettingsDialog(this.settings, async (s) => {
      await api.setSettings(s);
      this.settings = s;
      this.applySettings();
      this.toast("success", "设置已保存");
    }, this.datasourceSync, initialTab);
  }

  private cmdCycleTab(delta: number): void {
    if (!this.tabs.length) return;
    const i = this.tabs.findIndex((t) => t.id === this.activeTabId);
    const n = (i + delta + this.tabs.length) % this.tabs.length;
    this.activateTab(this.tabs[n].id);
  }

  private async cmdCloseOthers(): Promise<void> {
    const keep = this.activeTabId;
    await this.closeTabs(this.tabs.filter(t => t.id !== keep).map(t => t.id));
  }

  /** 关闭当前标签左侧 / 右侧的全部标签（基准为右键点中的标签）。 */
  private async cmdCloseSide(side: "left" | "right"): Promise<void> {
    const anchor = this.contextTabId ?? this.activeTabId;
    const idx = this.tabs.findIndex((t) => t.id === anchor);
    if (idx < 0) return;
    const doomed = side === "left" ? this.tabs.slice(0, idx) : this.tabs.slice(idx + 1);
    await this.closeTabs(doomed.map(t => t.id));
  }

  /** 在标签条右键时记录基准标签（关闭左侧/右侧以此为准）。 */
  private contextTabId: string | null = null;

  private async cmdCloseAll(): Promise<void> {
    await this.closeTabs(this.tabs.map(t => t.id));
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

  private assertObjectWrite(tab: TabState): ConnectionConfig {
    if (tab.datasourceStale) throw new Error(tab.datasourceStale);
    const cfg = this.connectionById(tab.connectionId);
    if (!cfg) throw new Error("连接已失效");
    if (cfg.read_only) throw new Error("此连接只读，不能修改数据库对象");
    if (this.manualTransactionFor(tab.connectionId) || this.manualTransactionIsBusy(tab.connectionId)) throw new Error("请先完成此连接的手动事务");
    return cfg;
  }

  async runObjectAction(id: string): Promise<void> {
    const reason = this.commandUnavailableReason(id);
    if (reason) throw new Error(reason);
    const tab = this.activeTab!, view = this.views.get(tab.id) as ObjectsTabView;
    const object = view.selectedObject();
    if (id === "object-new" || id === "object-new-procedure") {
      const type = id === "object-new-procedure" ? "procedure" : tab.objectType === "folder" ? "table" : tab.objectType;
      if (type === "query") { await this.openQueryTab(tab.connectionId, "", tab.schema, false); return; }
      const scope = { connectionId: tab.connectionId, schema: tab.schema, datasourceStale: tab.datasourceStale };
      openObjectAction({ title: `新建${objectLabel(type)}`, context: `${tab.connectionName} / ${scope.schema}`, label: `${objectLabel(type)}名称`, confirm: type === "table" ? "创建" : "进入设计器", submit: async name => {
        if (tab.connectionId !== scope.connectionId || tab.schema !== scope.schema) throw new Error("对象页上下文已变化，请重新打开操作");
        const cfg = this.assertObjectWrite(tab), sql = newObjectSql(cfg, scope.schema, name, type);
        if (type === "table") {
          await api.executeDdl(scope.connectionId, sql, scope.schema);
          await this.refreshObjectScope(scope.connectionId, scope.schema);
        } else {
          const design = this.makeTab(type === "view" ? "viewdesign" : "routinedesign", `新建${objectLabel(type)} · ${name}`, scope.connectionId, scope.schema, name, type as ObjectType, sql);
          design.designCreating = true; design.dirty = true;
          if (type === "function" || type === "procedure") {
            const target = { connection_id: scope.connectionId, schema: scope.schema, name, kind: type, identity_arguments: "" };
            const baseline = await api.readRoutineCreationBaseline(target, design.id);
            if (tab.connectionId !== scope.connectionId || tab.schema !== scope.schema || tab.datasourceStale !== scope.datasourceStale) throw new Error("获取创建基线期间对象页上下文已变化，未打开创建页面");
            if (baseline.object.connection_id !== target.connection_id || baseline.object.schema !== target.schema || baseline.object.name !== target.name || baseline.object.kind !== target.kind || baseline.object.identity_arguments !== "") throw new Error("创建基线对象身份不匹配，没有发送SQL");
            this.assertObjectWrite(tab);
            design.routineCreation = baseline;
          }
          this.tabs.push(design); this.activeTabId = design.id; this.renderTabs();
        }
      } });
      return;
    }
    if (id === "object-export") { this.cmdExport(false); return; }
    if (!object) return;
    if (object.favorite) {
      if (id === "object-open") await this.openSavedQuery(object.favorite.id);
      if (id === "object-rename") this.renameSavedQuery(object.favorite.id);
      if (id === "object-delete") this.deleteSavedQuery(object.favorite.id);
      return;
    }
    if (object.descriptor) {
      if (id === "object-call") await this.openCatalogObject(object.descriptor, true);
      else if (id === "object-open" || id === "object-design") await this.openCatalogObject(object.descriptor);
      else if (id === "object-delete") {
        const descriptor = object.descriptor;
        const cfg = this.assertObjectWrite(tab);
        if (objectProtocol(cfg) !== "sqlite" || descriptor.actions.drop?.level === "unsupported" || !descriptor.actions.drop) throw new Error(descriptor.actions.drop?.reason || "没有受控删除能力");
        const connectionId = tab.connectionId, schema = tab.schema;
        openObjectMutationDialog(descriptor.object, tab.id, () => {
          this.assertObjectWrite(tab);
          if (!this.tabs.includes(tab) || tab.connectionId !== connectionId || tab.schema !== schema) throw new Error("对象页归属已改变；没有发送删除 SQL");
        }, async () => {
          for (const opened of this.tabs) if (opened.kind !== "objects" && opened.connectionId === connectionId && opened.schema === schema && opened.object === descriptor.object.name && opened.objectType === "trigger") opened.datasourceStale = "触发器已删除；当前定义保留供核对，禁止再次保存";
          await this.refreshObjectScope(connectionId, schema);
        });
      }
      else throw new Error("当前对象操作尚未接入受控计划，没有发送SQL");
      return;
    }
    const { connectionId, schema } = tab;
    if (id === "object-open" || id === "object-design") {
      if (object.type === "function" || object.type === "procedure") await this.openRoutineDesignTab(connectionId, schema, object.name, object.type);
      else if (id === "object-open") await this.openTableTab(connectionId, schema, object.name, object.type as ObjectType);
      else if (object.type === "view") await this.openViewDesignTab(connectionId, schema, object.name);
      else await this.openStructureTab(connectionId, schema, object.name);
    } else if (id === "object-import") this.cmdImport(false);
    else if (id === "object-delete") {
      const name = object.name, type = object.type;
      openObjectAction({ title: `删除${objectLabel(type)}`, context: `确定删除 ${tab.connectionName} / ${schema} 中的${objectLabel(type)}「${name}」？此操作不可恢复。`, confirm: "删除", submit: async () => {
        if (tab.connectionId !== connectionId || tab.schema !== schema) throw new Error("对象页上下文已变化，请重新选择");
        const cfg = this.assertObjectWrite(tab), sql = dropObjectSql(cfg, schema, object);
        const affected = this.tabs.filter(t => t.kind !== "objects" && t.connectionId === connectionId && t.schema === schema && t.object === name && t.objectType === type);
        if (!await this.guardLeave(affected.map(t => t.id), "删除数据库对象", async () => {
          this.assertObjectWrite(tab);
          await api.executeDdl(connectionId, sql, schema);
          affected.forEach(t => { t.datasourceStale = "数据库对象已删除，当前文本或数据仍保留，请重新打开对象"; });
          await this.refreshObjectScope(connectionId, schema);
        })) throw new Error("删除已取消或未完成，原页面保留");
      } });
    }
  }

  private async refreshObjectScope(connectionId: string, schema: string): Promise<void> {
    const cfg = this.connectionById(connectionId);
    const base = cfg?.kind === "sqlite" ? `conn:${connectionId}` : `conn:${connectionId}/db/${schema}`;
    const node = this.tree.find(base);
    if (node) { node.loaded = false; await this.loadChildren(node); node.loaded = true; this.tree.render(); }
    for (const view of this.views.values()) if (view instanceof ObjectsTabView && view.isScope(connectionId, schema)) await view.reload();
    this.notifyToolbar();
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
  private cmdTruncateTable(): void { this.openTableOperation("clear"); }

  private cmdCopyTable(kind: "copy_structure" | "copy_data"): void { this.openTableOperation(kind); }

  private openTableOperation(kind: "clear" | "copy_structure" | "copy_data"): void {
    const selected = this.currentTableRef();
    if (!selected) { this.toast("error", "请先选择一个表"); return; }
    const ref = Object.freeze({ ...selected });
    const connectionIdentity = JSON.stringify(this.connectionById(ref.connId));
    const ownerId = crypto.randomUUID();
    const writeReason = (): string | null => {
      const connection = this.connectionById(ref.connId);
      if (!connection) return "连接已不存在";
      if (JSON.stringify(connection) !== connectionIdentity) return "连接配置已变化，请重新打开表操作";
      if (connection.read_only) return "此连接只读，不能修改表";
      const stale = this.tabs.find(tab => tab.connectionId === ref.connId && tab.schema === ref.schema && tab.object === ref.table && tab.datasourceStale)?.datasourceStale;
      if (stale) return stale;
      if (this.manualTransactionFor(ref.connId) || this.manualTransactionIsBusy(ref.connId)) return "请先完成此连接的手动事务";
      return null;
    };
    const reason = writeReason();
    if (reason) { this.toast("info", reason); return; }
    openTableOperationDialog({ connectionId: ref.connId, schema: ref.schema, table: ref.table, kind, ownerId, writeReason,
      toast: (level, text) => this.toast(level, text),
      applied: async (receipt, schema) => {
        const table = kind === "clear" ? receipt.source_table : receipt.target_table;
        if (!table) throw new Error("操作回执缺少精确刷新目标");
        await this.refreshTableOperationTarget(ref.connId, schema, table);
      },
    });
  }

  private async refreshTableOperationTarget(connectionId: string, schema: string, table: string): Promise<void> {
    this.tableMetaCache.delete(`${connectionId}/${schema}/${table}`);
    const ids = this.tabs.filter(tab => (tab.kind === "table" || tab.kind === "structure") && tab.connectionId === connectionId && tab.schema === schema && tab.object === table).map(tab => tab.id);
    if (ids.length) {
      const accepted = await this.guardLeave(ids, "刷新已完成表操作的目标", async () => {
        for (const id of ids) {
          const tab = this.tabs.find(tab => tab.id === id);
          if (!tab || tab.connectionId !== connectionId || tab.schema !== schema || tab.object !== table) continue;
          const view = this.views.get(id);
          if (view instanceof DataTabView || view instanceof StructureTabView) await view.reloadAfterLeave();
        }
      });
      if (!accepted) this.toast("info", "表操作已完成；目标页面草稿已保留，请稍后重新载入核对");
    }
    await this.refreshObjectScope(connectionId, schema);
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

  private cmdAddIndex(): void { const view = this.activeView(); if (view instanceof StructureTabView) view.addIndex(); }
  private cmdDeleteIndex(): void { const view = this.activeView(); if (view instanceof StructureTabView) view.deleteIndex(); }
  private cmdAddForeignKey(): void { const view = this.activeView(); if (view instanceof StructureTabView) view.addForeignKey(); }
  private cmdDeleteForeignKey(): void { const view = this.activeView(); if (view instanceof StructureTabView) view.deleteForeignKey(); }
  private cmdAddCheck(): void { const view = this.activeView(); if (view instanceof StructureTabView) view.addCheck(); }
  private cmdDeleteCheck(): void { const view = this.activeView(); if (view instanceof StructureTabView) view.deleteCheck(); }
  private cmdAddTrigger(): void { const view = this.activeView(); if (view instanceof StructureTabView) view.addTrigger(); }
  private cmdDeleteTrigger(): void { const view = this.activeView(); if (view instanceof StructureTabView) view.deleteTrigger(); }

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
    let ref = this.importExportTarget(false);
    if (fromTree) {
      // 树的数据库节点绝不借用活动标签页的表；对象身份在打开向导前固定。
      const node = this.tree.find(this.tree.selectedNodeId ?? "");
      const parts = node?.id.split("/") ?? [];
      const connId = parts[0]?.startsWith("conn:") ? parts[0].slice(5) : "";
      const dbIndex = parts.indexOf("db");
      const objectIndex = parts.findIndex(part => part === "table" || part === "view");
      ref = connId ? { connId, schema: dbIndex >= 0 ? parts[dbIndex + 1] ?? "" : this.connectionById(connId)?.kind === "sqlite" ? "main" : "", table: objectIndex >= 0 ? parts[objectIndex + 1] ?? "" : "" } : null;
    }
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
      if (view.resultBusy()) { this.toast("info", "查询执行、缓存读取或导出进行中，请等待完成"); return; }
      const snapshot = view.captureResultExport();
      openResultExportDialog(cfg, opts => view.exportResultSnapshot(opts, snapshot), snapshot.scope
        ? `结果 ${snapshot.index + 1}：导出本次执行全部 ${set.total} 行（来自完整缓存，不重新执行 SQL），不受当前页限制。`
        : `结果 ${snapshot.index + 1} 没有完整缓存，只能导出已加载的 ${set.rows.length} 行（共 ${set.total} 行）；不会重新执行 SQL。`);
      return;
    }
    const schema = ref?.schema ?? this.currentDbName();
    if (!fromTree && view instanceof ObjectsTabView && tab?.kind === "objects") {
      const selected = view.selectedObjects().filter(object => object.type === "table" || object.type === "view")
        .map(object => ({ name: object.name, object_type: object.type as "table" | "view" }));
      openBatchExportWizard(cfg, schema, selected, result => {
        const completed = result.items.filter(item => item.status === "completed").length;
        this.toast(completed === result.items.length ? "success" : "info", `批量导出：${completed} / ${result.items.length} 个对象完成，请查看逐对象结果`);
      });
      return;
    }
    if (!ref.table && schema) {
      openBatchExportWizard(cfg, schema, []);
      return;
    }
    const table = ref?.table ?? (tab?.kind === "table" ? tab.object : "");
    if (!table) {
      this.toast("error", "请先选择一个表");
      return;
    }
    openExportWizard(cfg, schema, table, (n) => {
      this.toast("success", `已导出 ${n} 行`);
    });
  }

  private cmdDump(fromTree = false, frozenTarget?: Readonly<{ connId: string; schema: string; table: string }>): void {
    let target = frozenTarget ?? this.importExportTarget(false);
    if (fromTree && !frozenTarget) {
      const node = this.tree.find(this.tree.selectedNodeId ?? "");
      const parts = node?.id.split("/") ?? [];
      const connId = parts[0]?.startsWith("conn:") ? parts[0].slice(5) : "";
      const dbIndex = parts.indexOf("db");
      const objectIndex = parts.findIndex(part => part === "table" || part === "view");
      target = connId ? { connId, schema: dbIndex >= 0 ? parts[dbIndex + 1] ?? "" : this.connectionById(connId)?.kind === "sqlite" ? "main" : "", table: objectIndex >= 0 ? parts[objectIndex + 1] ?? "" : "" } : null;
    }
    const cfg = this.connectionById(target?.connId ?? "");
    if (!cfg || !target) {
      this.toast("error", "请先打开一个连接");
      return;
    }
    const { schema, table } = { ...target };
    if (!table) {
      // 数据库级转储：选择「数据和结构 / 仅结构」→ 直接弹保存位置对话框。
      this.cmdDumpDatabase(cfg, schema);
      return;
    }
    openDumpDialog(cfg, schema, table, async (opts) => {
      const n = await api.runDump(opts);
      this.toast("success", `转储完成，共 ${n} 行`);
      return n;
    });
  }

  /** 数据库级转储 SQL 文件：内容和范围 → 保存位置（可修改文件名）→ 执行。 */
  private cmdDumpDatabase(cfg: import("./types").ConnectionConfig, schema: string): void {
    if (!schema) {
      this.toast("error", "请先选择要转储的数据库");
      return;
    }
    let structureOnly = false;
    const file = textInput("", 320);
    file.setAttribute("aria-label", "数据库转储完整保存路径");
    const destination = bindExportPath(file, `${schema}.sql`);
    const status = document.createElement("div");
    status.style.cssText = "margin-top:8px;min-height:18px;color:var(--fg-muted);";
    let running = false;
    const dlg = new Dialog({
      title: "转储 SQL 文件",
      width: 460,
      render: (body) => {
        const connection = textInput(cfg.name, 220), namespace = textInput(schema, 220);
        connection.readOnly = namespace.readOnly = true;
        connection.title = cfg.name; namespace.title = schema;
        body.appendChild(field("连接:", connection));
        body.appendChild(field(`${namespaceLabel(cfg)}:`, namespace));
        body.appendChild(field("保存完整路径:", file));
        const scope = radioGroup("dump-scope", [
          { value: "both", label: "数据和结构" },
          { value: "structure", label: "仅结构" },
        ], "both");
        body.appendChild(field("转储内容:", scope));
        scope.querySelector<HTMLInputElement>('input[value="structure"]')?.addEventListener("change", () => {
          structureOnly = scope.querySelector<HTMLInputElement>('input[value="structure"]')?.checked ?? false;
        });
        scope.querySelector<HTMLInputElement>('input[value="both"]')?.addEventListener("change", () => {
          structureOnly = !scope.querySelector<HTMLInputElement>('input[value="both"]')?.checked;
        });
        body.appendChild(status);
      },
      buttons: [
        {
          label: "选择保存位置",
          primary: true,
          onClick: async () => {
            if (running) return false;
            await destination.ready;
            const pathProblem = destination.problem();
            if (pathProblem) { dlg.showError(pathProblem); return false; }
            let path: string | null = null;
            try {
              const { save } = await import("@tauri-apps/plugin-dialog");
              path = await save({
                title: "保存 SQL 文件",
                defaultPath: file.value.trim(),
                filters: [{ name: "SQL 文件", extensions: ["sql"] }],
              });
            } catch {
              path = null;
            }
            if (!path) return false;
            file.value = path;
            destination.refresh();
            const selectedProblem = destination.problem();
            if (selectedProblem) { dlg.showError(selectedProblem); return false; }
            running = true;
            status.textContent = "正在生成转储...";
            const handle = taskCenter.register("dump", `SQL 转储 · ${schema}`);
            try {
              const [rows, bytes] = await api.dumpDatabaseToFile(cfg.id, schema, path, !structureOnly, !structureOnly);
              handle.finish(`${rows} 行数据（${(bytes / 1024).toFixed(1)} KB）`, path);
              status.textContent = `转储完成：${rows} 行数据，文件 ${(bytes / 1024).toFixed(1)} KB。`;
              this.toast("success", `已转储到 ${path}`);
            } catch (e) {
              handle.fail(String(e));
              status.textContent = `转储失败：${String(e)}`;
              dlg.showError(String(e));
              running = false;
              return false;
            }
            running = false;
            return true;
          },
        },
        { label: "取消" },
      ],
    });
    dlg.el.classList.add("form-stacked");
    status.setAttribute("role", "status");
  }

  /** 数据库右键「运行 SQL 文件」：选文件 → 执行选项（遇错终止 / 跳过继续）→ 逐条执行并报告。 */
  private async cmdRunSqlFile(): Promise<void> {
    const target = this.currentDbTarget();
    const connId = target?.connId ?? this.currentConnectionId();
    const cfg = this.connectionById(connId);
    if (!cfg) {
      this.toast("error", "请先选择一个连接");
      return;
    }
    const schema = target?.schema ?? this.currentDbName();
    let path: string | null = null;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      path = await open({
        title: "选择 SQL 文件",
        multiple: false,
        filters: [{ name: "SQL 文件", extensions: ["sql"] }, { name: "所有文件", extensions: ["*"] }],
      });
    } catch {
      path = null;
    }
    if (typeof path !== "string" || !path) return;
    openSqlFileExecutionDialog(cfg, schema, path);
  }

  /** 备份列表右键「重命名」：修改备份文件名。 */
  private cmdRenameBackup(): void {
    const t = this.backupTarget();
    const backup = t?.view?.selectedBackup();
    if (!t || !backup) {
      this.toast("error", "请先在备份列表中选择一个备份");
      return;
    }
    const input = textInput(backup.name.replace(/\.sql$/i, ""), 300);
    const dlg = new Dialog({
      title: "重命名备份",
      width: 420,
      render: (body) => {
        body.appendChild(field("新名称:", input));
        const note = document.createElement("div");
        note.className = "form-note";
        note.textContent = "保留扩展名 .sql；重命名后备份列表会自动刷新。";
        body.appendChild(note);
      },
      buttons: [
        {
          label: "重命名",
          primary: true,
          onClick: async () => {
            const name = input.value.trim();
            if (!name) {
              dlg.showError("请填写新名称");
              return false;
            }
            if (/[\\/:*?"<>|]/.test(name)) {
              dlg.showError("名称不能包含 / \\ : * ? \" < > | 字符");
              return false;
            }
            try {
              await api.renameBackup(t.connId, t.schema, backup.name, name.endsWith(".sql") ? name : `${name}.sql`);
              this.toast("success", "备份已重命名");
              this.refreshBackupTabs(t.connId, t.schema);
            } catch (e) {
              dlg.showError(String(e));
              return false;
            }
            return true;
          },
        },
        { label: "取消" },
      ],
    });
    dlg.el.classList.add("form-stacked");
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
    if (node.objectDescriptor) { void this.openCatalogObject(node.objectDescriptor); return; }
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
    const source = view.getSql(), sql = source.trim();
    if (!sql) {
      this.toast("error", "视图定义不能为空");
      return;
    }
    try {
      const cfg = this.assertObjectWrite(tab);
      const reason = objectCapability(cfg, "view", tab.designCreating ? "create" : "update");
      if (reason) throw new Error(reason);
      if (cfg.kind === "sqlite" && !tab.designCreating) {
        await api.replaceSqliteView(tab.connectionId, tab.schema, tab.object, sql);
      } else {
        const definition = !tab.designCreating ? sql.replace(/^CREATE\s+(?!OR\s+REPLACE\b)/i, "CREATE OR REPLACE ") : sql;
        await api.executeDdl(tab.connectionId, definition.replace(/;\s*$/, ""), tab.schema);
      }
      this.toast("success", `视图「${tab.object}」已更新`);
      view.markClean(source);
      tab.designCreating = false; tab.title = tab.object; view.showError("");
      await this.refreshObjectScope(tab.connectionId, tab.schema);
    } catch (e) {
      view.showError(`保存失败，当前 SQL 已保留：${String(e)}`);
      this.toast("error", String(e));
    }
  }

  /** 保存例程：沿用打开时基线，经后端预览与唯一C1门禁执行。 */
  private async cmdSaveRoutineDesign(): Promise<void> {
    const tab = this.activeTab;
    if (tab?.kind !== "routinedesign") return;
    const view = this.views.get(tab.id) as RoutineTabView | undefined;
    if (!view) return;
    try {
      this.assertObjectWrite(tab);
      if (await view.saveRoutine()) {
        this.toast("success", `「${tab.object}」已提交并读回验证`);
        await this.refreshObjectScope(tab.connectionId, tab.schema);
      }
    } catch (error) { view.showError(`保存未完成，当前SQL保留：${String(error)}`); this.toast("error", String(error)); }
  }

  // ------------------------------------------------------ 自动任务

  openScheduleTab(): void {
    const existing = this.tabs.find((t) => t.kind === "schedule");
    if (existing) {
      this.activeTabId = existing.id;
      this.renderTabs();
      return;
    }
    const tab = this.makeTab("schedule", "自动任务", this.activeTab?.connectionId ?? "", "", "");
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
      if (j.enabled && (j.interval_secs > 0 || j.daily_at || j.cron) && !this.jobNextRun.has(j.id)) {
        this.jobNextRun.set(j.id, this.nextJobRun(j));
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
      this.toast("success", "任务已创建");
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
      await this.executeJobTask(job);
      this.jobNextRun.set(job.id, this.nextJobRun(job));
      await this.reloadScheduleJobs();
      this.toast("success", `任务「${job.name}」执行完成`);
    } catch (e) {
      this.toast("error", `任务「${job.name}」失败：${String(e)}`);
      await this.reloadScheduleJobs();
    }
  }

  /**
   * 任务派发：任务型任务（备份 / 导出 / 导入 / 数据同步 / 数据生成 / 数据字典）由前端调用
   * 对应工具接口执行并写入运行历史；旧版 SQL 任务仍走后端 run_job_now。
   */
  private async executeJobTask(job: import("./types").ScheduleJob): Promise<void> {
    const task = job.task;
    if (!task || task.kind === undefined) {
      // 旧版 SQL 任务
      const [elapsed, detail] = await api.runJobNow(job.id);
      this.toast("info", `任务「${job.name}」完成（${(elapsed / 1000).toFixed(2)} s）：${detail}`);
      return;
    }
    const started = Date.now();
    const options = (task.options ?? {}) as Record<string, unknown>;
    let ok = true;
    let detail = "";
    try {
      switch (task.kind) {
        case "backup": {
          const schema = String(options.schema ?? job.database ?? "");
          const [rows, bytes] = await api.createBackup({
            connection_id: job.connection_id,
            schema,
            comment: String(options.comment ?? ""),
            tables: Array.isArray(options.tables) ? options.tables.map(String) : [],
            routines: Array.isArray(options.routines) ? options.routines.map(String) : [],
            file_name: null,
            include_structure: options.include_structure !== false,
            include_data: options.include_data !== false,
          }).then((backup) => [backup.rows, backup.bytes] as const);
          detail = `备份完成：${rows} 行数据，文件 ${(bytes / 1024).toFixed(1)} KB`;
          this.refreshBackupTabs(job.connection_id, schema);
          break;
        }
        case "export": {
          const n = await api.runExport({ ...(options as Partial<import("./types").ExportOptions>), connection_id: job.connection_id } as import("./types").ExportOptions);
          detail = `导出完成，共 ${n} 行`;
          break;
        }
        case "import": {
          const n = await api.runImport({ ...(options as Partial<import("./types").ImportOptions>), connection_id: job.connection_id } as import("./types").ImportOptions);
          detail = `导入完成，共 ${n} 行`;
          break;
        }
        case "data_sync": {
          const n = await api.runTransfer({ ...(options as Partial<import("./types").TransferOptions>), source_connection: job.connection_id } as import("./types").TransferOptions);
          detail = `数据同步完成，共 ${n} 行`;
          break;
        }
        case "data_generation": {
          const schema = String(options.schema ?? job.database ?? "");
          const table = String(options.table ?? "");
          const meta = await api.getTableSchema(job.connection_id, schema, table);
          const count = Math.max(1, Math.min(Number(options.count ?? 100) || 100, MAX_GENERATED_ROWS));
          const plan = generateDataPreview(
            meta,
            defaultGeneratorFields(meta),
            count,
            options.seed != null && String(options.seed) !== "" ? String(options.seed) : `job_${job.id.slice(0, 8)}`,
          );
          await api.insertGeneratedRows({ connection_id: job.connection_id, schema, table, columns: plan.columns, rows: plan.rows });
          detail = `数据生成完成，写入 ${plan.rows.length} 行`;
          break;
        }
        case "data_dictionary": {
          const schema = String(options.schema ?? job.database ?? "");
          const format = String(options.format ?? "markdown");
          const file = String(options.file ?? "").trim();
          if (!file) throw new Error("数据字典任务缺少输出文件路径，请编辑任务重新保存");
          const meta = await api.dictionaryMetadata(job.connection_id, schema);
          const dictOpts: import("./types").DictionaryExportOptions = {
            format,
            excel_sheet_mode: String(options.excel_sheet_mode ?? "single"),
            include_tables: true,
            include_views: format === "pdf" || format === "markdown",
            include_routines: format === "pdf" || format === "markdown",
            created_at: new Date().toLocaleString("zh-CN", { hour12: false }),
          };
          const bytes = await api.dictionaryExport(meta, dictOpts, file);
          detail = `数据字典已导出：${file}（${(bytes / 1024).toFixed(1)} KB）`;
          break;
        }
        default:
          throw new Error(`未支持的任务类型：${task.kind}`);
      }
    } catch (e) {
      ok = false;
      detail = String(e);
    }
    // record_job_history 在后端统一写入运行历史并回写列表的上次运行 / 状态列。
    await api.recordJobHistory(job.id, job.name, ok, Date.now() - started, detail).catch(() => undefined);
    if (!ok) throw new Error(detail);
    this.toast("info", `任务「${job.name}」：${detail}`);
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

  /** 唯一后端 SchedulerHost 负责持久调度；浏览器不再领取或重放旧任务。 */
  private setupScheduleTimer(): void {
    // 自动任务统一到期执行引擎：应用运行期间每 5 秒检查一次到期任务。
    // 任务型任务（备份 / 导出 / 导入 / 数据同步 / 数据生成 / 数据字典）由前端
    // executeJobTask 派发到对应工具接口；旧版 SQL 任务仍走后端 run_job_now。
    window.setInterval(() => {
      const now = Date.now();
      for (const job of this.scheduleJobs) {
        if (!job.enabled) continue;
        if (!(job.interval_secs > 0 || job.daily_at || job.cron)) continue;
        const next = this.jobNextRun.get(job.id);
        if (next === undefined || now < next) continue;
        // 先推进计划时间再执行，失败也不在同一时刻重复触发。
        this.jobNextRun.set(job.id, this.nextJobRun(job));
        void this.executeJobTask(job)
          .then(() => this.reloadScheduleJobs())
          .catch(() => this.reloadScheduleJobs());
      }
    }, 5000);
  }

  /** 计算下一次运行时间：cron 优先，其次每日固定时刻，否则按间隔。 */
  private nextJobRun(job: import("./types").ScheduleJob): number {
    if (job.cron) {
      const next = cronNextOccurrence(job.cron);
      if (next !== null) return next;
    }
    if (job.daily_at) {
      const [h, m] = job.daily_at.split(":").map((x) => Number(x));
      const next = new Date();
      next.setHours(h, m, 0, 0);
      if (next.getTime() <= Date.now()) next.setDate(next.getDate() + 1);
      return next.getTime();
    }
    return Date.now() + job.interval_secs * 1000;
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

  private cmdEditUser(): void {
    const ref = this.currentUserRef();
    if (!ref) {
      this.toast("error", "请先在用户列表中选择一个用户");
      return;
    }
    openEditUserDialog(ref.connId, ref.user, ref.host, () => {
      this.toast("success", `用户 ${ref.user}@${ref.host} 已更新`);
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

  /** 事务提交 / 回滚后刷新该连接的表数据页：清除事务内可视化状态并重读数据。 */
  private async refreshDataTabs(connId: string): Promise<void> {
    if (!connId) return;
    for (const t of this.tabs) {
      if (t.connectionId !== connId) continue;
      const view = this.views.get(t.id);
      if (view instanceof DataTabView) {
        view.clearTransactionState();
        try { await view.reload(); } catch { /* 失败已由 reload 提示 */ }
      }
    }
    this.notifyToolbar();
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
            // 关闭只停止显示，后台任务继续；另有“停止后续语句”按钮，等待当前操作回执。
            return true;
          },
        },
      ],
    });
    const restoreFeedback = taskFeedback(dlg, statusText, undefined, undefined, {
      cancelOnClose: false, unit: "条语句", cancelLabel: "停止后续语句",
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
      const effectLabel = ({ acked: "已回执", committed: "已提交", rolled_back: "已回滚", skipped: "已跳过", failed: "失败", unknown: "结果未知" } as const)[step.effect];
      row.dataset.text =
        `#${step.index} ${effectLabel} ${step.sql}` +
        (step.error ? `\n    → ${step.error}` : "") +
        (step.affected != null ? `\n    → 已回执影响 ${step.affected} 行（${effectLabel}）` : "");

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
        ok.textContent = `已回执影响 ${step.affected} 行 · ${effectLabel}`;
        main.appendChild(ok);
      } else {
        const ok = document.createElement("div");
        ok.className = "restore-step-meta";
        ok.textContent = effectLabel;
        main.appendChild(ok);
      }
      row.append(num, icon, main);
      list.appendChild(row);
      list.scrollTop = list.scrollHeight;
    };

    try {
      const restoreTaskId = restoreFeedback.begin();
      statusText.textContent = "正在打开连接…";
      await api.openConnection(connId);
      statusText.textContent = "正在完整预扫描；执行中可停止后续语句，等待当前操作回执与清理…";
      const report = await api.restoreBackupDetailed(connId, schema, file, restoreTaskId);
      restoreFeedback.finish();
      for (const step of report.steps) appendStep(step);
      finished = true;
      spinner.remove();
      setProgress(report.attempted, report.total);
      // 执行结束后回到顶部：用户从第 1 条开始读，而非停在末尾
      list.scrollTop = 0;
      const ok = report.failed === 0 && !report.effects_unknown && !report.error && !report.cleanup_error;
      const failureAt = report.first_failed_index != null ? `，第 ${report.first_failed_index} 条失败` : "";
      const aggregate = `总计 ${report.total} 条 · 已回执 ${report.acked} · 已提交 ${report.committed} · 已回滚 ${report.rolled_back} · 跳过 ${report.skipped} · 未知 ${report.unknown} · 失败 ${report.failed}`;
      const notices = [
        report.omitted_steps > 0 ? `报告有界：仅展示 ${report.steps.length} 条，省略 ${report.omitted_steps} 条；上述汇总覆盖全部语句，语句编号保持原序。` : "",
        report.error ?? "", report.cleanup_error ?? "", ...report.warnings,
      ].filter(Boolean);
      if (notices.length) {
        const note = document.createElement("div");
        note.className = "restore-step skipped";
        note.textContent = notices.join("\n");
        note.dataset.text = `${aggregate}\n${notices.join("\n")}`;
        list.appendChild(note);
      }
      statusRow.classList.add(ok ? "ok" : "error");
      statusText.textContent = ok
        ? `还原完成：已确认提交 ${report.committed} 条语句`
        : `还原未完整完成：已提交 ${report.committed} 条${failureAt}${report.effects_unknown ? "，存在未知结果，禁止自动重放" : ""}`;
      summary.textContent = aggregate;
      summary.classList.add(ok ? "ok" : "error");
      if (copyBtn) copyBtn.disabled = false;
      if (ok) this.toast("success", `还原完成，已确认提交 ${report.committed} 条语句`);
      else this.toast("error", `还原未完整完成${failureAt}${report.effects_unknown ? "；请核对目标，不要自动重放" : ""}`);
      this.refreshBackupTabs(connId, schema);
    } catch (e) {
      restoreFeedback.finish();
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
    if (node.objectDescriptor) {
      const object = node.objectDescriptor.object;
      void this.refreshObjectScope(object.connection_id, object.schema).catch(error => this.toast("error", `刷新目录失败：${String(error)}`));
      return;
    }
    this.treeLoads.delete(node);
    node.loading = false;
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
    const namespace = namespaceName(cfg);
    const nameInput = document.createElement("input");
    nameInput.type = "text";
    nameInput.style.width = "220px";
    new Dialog({
      title: `新建${namespace}`,
      width: 380,
      render: (body) => {
        const row = document.createElement("div");
        row.className = "field";
        const lab = document.createElement("div");
        lab.className = "field-label";
        lab.textContent = `${namespace}名:`;
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
                this.toast("success", `${namespace}「${name}」已创建`);
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
    const namespace = namespaceName(cfg);
    if (!cfg || !db) {
      this.toast("error", `请先在左侧树中选择一个${namespace}`);
      return;
    }
    new Dialog({
      title: `删除${namespace}`,
      width: 400,
      render: (body) => {
        body.textContent = `确定要删除${namespace}「${db}」吗？该操作不可恢复，${namespace}中全部对象将被删除。`;
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
                this.toast("success", `${namespace}「${db}」已删除`);
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
    const protocol = kindMeta(kind)?.protocol ?? (kind === "mssql" ? "mssql" : undefined);
    const isTick = protocol === "mysql" || protocol === "clickhouse";
    const q = (s: string) => protocol === "mssql"
      ? `[${s.replace(/\]/g, "]]")}]`
      : isTick ? `\`${s.replace(/`/g, "``")}\`` : `"${s.replace(/"/g, '""')}"`;
    const pgFamily = protocol === "postgres";
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

  /** 预览开关或格式变化只刷新当前数据页，不改写其它页面的面板。 */
  private refreshCellPreviews(): void {
    const view = this.activeView();
    if (view instanceof DataTabView) view.updateCellPreview();
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
  flushDraft(): void {
    const active = document.activeElement;
    if (active instanceof HTMLElement && this.el.contains(active)) active.blur();
  }
  draftDirty(): boolean { return !!this.tab.dirty; }
  draftBusy(): boolean { return false; }
  discardDraft(): void { this.tab.dirty = false; }
  captureDraft(): Pick<DraftSnapshot, "kind" | "baseline" | "payload"> {
    if (this.draftDirty()) throw new Error("此资产页尚未接入完整草稿载荷，已拒绝关闭以保留编辑");
    return { kind: "table_structure", baseline: null, payload: { requires_validation: true } };
  }
}

/** 对象列表视图（「对象」标签页：库名下的表/视图清单；顶部信息条已去掉，直接从表头开始）。 */
class ObjectsTabView extends TabView {
  grid: DataGrid;
  private objects: BrowserObject[] = [];
  /** 过滤后的可见行索引。 */
  private visible: number[] = [];
  private generation = 0;
  private disposed = false;
  private scope = "";
  private metadataNotice = "";
  private metadataKnown = false;
  /** 保留供核对的旧清单不能作为本次操作的对象证明。 */
  private staleResults = false;
  /** 搜索框由工具栏搜索图标就地展开（App 托管输入框元素，视图只保存值）。 */
  private searchValue_ = "";
  loading = false;

  constructor(app: App, tab: TabState) {
    super(app, tab);
    this.el.classList.add("object-browser");
    this.grid = new DataGrid({
      onSort: () => {
        /* 对象清单固定按名称排序 */
      },
      onSelectionChange: (rows) => {
        if (this.interactionBlocked()) { this.clearSelection(); this.updateFooter(); return; }
        this.tab.selectedRows = new Set(rows.filter(row => Number.isInteger(row) && !!this.objects[this.visible[row]]));
        app.notifyToolbar();
        this.updateFooter();
        const selected = this.selectedObject();
        const cfg = app.connectionById(tab.connectionId);
        if (selected?.descriptor) app.showCatalogDetails(selected.descriptor);
        else if (selected && cfg && selected.type !== "query") app.showObjectDetails(cfg, tab.schema, selected.name, selected.type as ObjectType);
      },
      onRowDoubleClick: (row) => {
        if (this.interactionBlocked() || !this.objects[this.visible[row]]) return;
        tab.selectedRows = new Set([row]); this.grid.setState({ selectedRows: tab.selectedRows });
        void app.runCommand("object-open");
      },
      onContextMenu: (x, y, row) => {
        if (this.interactionBlocked()) { this.clearSelection(); this.updateFooter(); return; }
        if (row < 0 || !this.objects[this.visible[row]]) tab.selectedRows = new Set();
        else if (!tab.selectedRows.has(row)) tab.selectedRows = new Set([row]);
        this.grid.setState({ selectedRows: tab.selectedRows }); app.notifyToolbar();
        app.showContextMenu(objectTools(tab.objectType, this.selectedObject(), app.connectionById(tab.connectionId)).map(item => ({ ...item, disabledReason: item.disabledReason })), x, y);
      },
    });
    this.el.appendChild(this.grid.el);
    // 网格页脚：左统计（设计图「19 个表 · 已选中 1 行」）
    this.grid.el.addEventListener("keydown", event => {
      if (this.interactionBlocked() || (event.target as HTMLElement).matches("input, textarea, select")) return;
      if (event.key === "Enter" && this.selectedObject()) { event.preventDefault(); void app.runCommand("object-open"); }
      if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
        event.preventDefault();
        const rect = this.grid.el.getBoundingClientRect();
        app.showContextMenu(objectTools(tab.objectType, this.selectedObject(), app.connectionById(tab.connectionId)), rect.left, rect.top);
      }
    });
    this.footer = document.createElement("div");
    this.footer.className = "grid-footer";
    this.footer.id = `object-feedback-${tab.id}`;
    this.footer.setAttribute("role", "status");
    this.footer.setAttribute("aria-live", "polite");
    this.footer.setAttribute("aria-atomic", "true");
    // 保留原页脚布局；长错误在窄窗中换行，不能被固定高度裁掉。
    this.footer.style.height = "auto";
    this.footer.style.whiteSpace = "normal";
    this.footer.style.overflowWrap = "anywhere";
    this.footer.style.padding = "6px var(--sp-2)";
    this.grid.el.setAttribute("aria-describedby", this.footer.id);
    this.el.appendChild(this.footer);
    this.updateFooter();
  }

  private footer: HTMLElement;

  /** 工具栏搜索框值（App 托管的输入框读写）。 */
  searchValue(): string { return this.searchValue_; }
  setSearchValue(value: string): void {
    if (value === this.searchValue_) return;
    this.searchValue_ = value;
    this.clearSelection();
    this.applySearch();
  }

  /** 工具栏「打开表 / 设计表」取选中行对象。 */
  selectedObject(): BrowserObject | null {
    if (this.interactionBlocked()) return null;
    const r = [...this.tab.selectedRows][0];
    if (r === undefined) return null;
    return this.objects[this.visible[r]] ?? null;
  }

  /** 导出使用整组选中对象，不能退化为第一行或以空集合代表全库。 */
  selectedObjects(): BrowserObject[] {
    if (this.interactionBlocked()) return [];
    return [...this.tab.selectedRows].map(row => this.objects[this.visible[row]]).filter((object): object is BrowserObject => !!object);
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

  isScope(connectionId: string, schema: string): boolean { return this.tab.connectionId === connectionId && this.tab.schema === schema; }
  private scopeKey(): string { return JSON.stringify([this.tab.connectionId, this.tab.schema, this.tab.objectType]); }
  private interactionBlocked(): boolean { return this.disposed || this.loading || this.staleResults || this.scope !== this.scopeKey(); }
  private clearSelection(): void {
    this.tab.selectedRows.clear(); this.grid?.setState({ selectedRows: this.tab.selectedRows }); this.app.notifyToolbar();
  }

  async reload(): Promise<void> {
    if (this.disposed) return;
    const generation = ++this.generation, scope = this.scopeKey();
    const { connectionId, schema, objectType: filter } = this.tab;
    if (scope !== this.scope) { this.searchValue_ = ""; this.objects = []; this.staleResults = false; }
    this.scope = scope; this.metadataNotice = ""; this.metadataKnown = false; this.loading = true;
    this.el.setAttribute("aria-busy", "true");
    this.clearSelection(); this.applySearch();
    const current = () => !this.disposed && generation === this.generation && scope === this.scopeKey();
    try {
      let objects: BrowserObject[], notice = "", stale = false;
      if (filter === "query") {
        objects = this.app.savedQueries(connectionId, schema).map(favorite => ({ name: favorite.name, type: "query", favorite }));
      } else if (CATALOG_KINDS.includes(filter)) {
        const kinds = filter === "function" ? ["function", "procedure"] : [filter];
        const catalog = await this.app.navigationCatalog(connectionId, schema, kinds);
        if (!current()) return;
        notice = catalogNotice(Object.fromEntries(kinds.map(kind => [kind, catalog.sections[kind] ?? { state: "unknown", reason: "后端未提供此类别的读取状态" }])));
        objects = catalog.objects.map(descriptor => ({ name: descriptor.object.name, type: descriptor.object.kind as ObjectType, descriptor }));
        if (!objects.length && notice && this.objects.length) { objects = this.objects; stale = true; }
      } else {
        const list = await api.listObjects(connectionId, schema);
        objects = filter === "table" || filter === "view" ? list.filter(object => object.type === filter) : list;
      }
      if (!current()) return;
      this.objects = objects; this.metadataNotice = notice; this.metadataKnown = !notice; this.staleResults = stale;
    } catch (e) {
      if (!current()) return;
      this.metadataNotice = `加载失败：${String(e)}。${this.objects.length ? "当前保留上次已加载清单，仅供核对；" : "尚未取得对象清单；"}请刷新后核对。`;
      this.metadataKnown = false; this.staleResults = true;
      this.app.toast("error", `对象列表加载失败：${String(e)}`);
    } finally {
      if (current()) {
        this.loading = false; this.el.setAttribute("aria-busy", "false");
        this.applySearch(); this.app.renderStatus(); this.app.notifyToolbar();
      }
    }
  }

  private applySearch(): void {
    const search = this.searchValue_.trim().toLocaleLowerCase();
    this.visible = this.objects.flatMap((object, index) => (object.descriptor?.display_name ?? object.name).toLocaleLowerCase().includes(search) ? [index] : []);
    this.renderData();
  }

  private renderData(): void {
    this.grid.setData({
      ...objectGrid(this.tab.objectType, this.visible.map(i => this.objects[i]), false),
      selectedRows: this.tab.selectedRows,
      rightAlign: ["数据大小", "行数", "自增值"],
      rowIcons: this.visible.map((i) =>
        this.objects[i].type === "query" ? "info_pane_query" : this.objects[i].type === "view"
          ? "info_pane_view"
          : this.objects[i].type === "function" || this.objects[i].type === "procedure"
            ? "info_pane_function"
            : "info_pane_table",
      ),
      showRowNumbers: false,
      readOnly: true,
    });
    // 不同类别各自配置列宽；表的可选自增值列由网格补齐默认宽度。
    this.grid.setColWidths(CATALOG_KINDS.includes(this.tab.objectType) || this.visible.some(i => !!this.objects[i].descriptor) ? [280, 120, 180, 320, 140] : this.tab.objectType === "query" ? [240, 180, 520] : this.tab.objectType === "view" ? [280, 180, 320] : [240, 160, 110, 100, 90, 240]);
    this.updateFooter();
  }

  /** 页脚始终呈现加载、完整性、空结果和筛选状态。 */
  private updateFooter(): void {
    const kind = objectLabel(this.tab.objectType);
    const count = this.visible.length, total = this.objects.length;
    const selection = !this.interactionBlocked() && this.tab.selectedRows.size ? ` · 已选中 ${this.tab.selectedRows.size} 行` : "";
    const summary = this.searchValue_.trim() ? `显示 ${count} / ${total} 个${kind}` : `${count} 个${kind}`;
    if (this.loading) {
      this.footer.textContent = `正在加载${kind}清单…${total ? ` · ${summary}（上次已加载，仅供核对）` : ""}`;
    } else if (this.metadataNotice) {
      this.footer.textContent = `${this.metadataNotice}${total ? ` · ${summary}${this.staleResults ? "（上次已加载，仅供核对）" : "（当前可见清单，可能不完整）"}` : " · 尚不能确认当前类别为空"}${selection}`;
    } else if (!this.metadataKnown) {
      this.footer.textContent = "尚未加载对象清单";
    } else if (!total) {
      this.footer.textContent = this.tab.objectType === "query" ? "当前数据库没有已保存查询" : `当前数据库没有${kind}`;
    } else if (!count) {
      this.footer.textContent = `没有匹配的${kind} · 共 ${total} 个${kind}`;
    } else {
      this.footer.textContent = `${summary}${selection}`;
    }
    this.footer.setAttribute("aria-live", !this.loading && this.metadataNotice ? "assertive" : "polite");
    this.footer.style.color = this.metadataNotice ? "var(--text-secondary)" : "";
  }

  destroy(): void {
    this.disposed = true; ++this.generation;
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
  /** 手动事务内已执行但未提交的更改（提交事务前一直显示新值，但不再标记待提交）。 */
  private txAppliedUpdates = new Map<number, Map<string, string | null>>();
  private txAppliedDeletes = new Set<number>();
  private txAppliedInserts: (string | null)[][] = [];
  private rowDraftTokens = new WeakMap<object, string>();
  private reloadingAfterLeave = false;
  /** 表字段以真实 name 标识，隐藏列不移除行草稿。 */
  private hiddenCols = new Set<string>();
  private pinnedCols = new Set<string>();
  private columnComments = new Map<string, string>();
  private columnMetadataLoaded = false;
  private columnMetadataLoading = false;
  /** 表单视图容器与当前记录索引（仅表单模式使用）。 */
  private formHost!: HTMLElement;
  private formIndex = 0;
  private columnSidebar: ColumnSidebar;
  private editingCol = "";
  /** 内联筛选行（草稿）：一行一个括号组。行内多条条件按 cond.logic 连接
   *  （默认 OR），行与行按 line.logic 并列连接（默认 AND），
   *  直接对应 a = 1 AND (b = 2 OR c = 3) AND d = 4。 */
  private filterLines: {
    logic?: import("./types").FilterLogic;
    conds: { col: string; op: string; val: string; touched?: boolean; logic?: import("./types").FilterLogic }[];
  }[] = [];
  private sortRows: import("./types").SortRule[] = [];
  /** 编辑草稿与已应用状态分离；隐藏面板不丢失草稿。 */
  private barMode: "filter" | "sort" | null = null;
  private sortRowsEl!: HTMLElement;
  private sortCountEl: HTMLElement | null = null;
  /** 拖拽排序时记录的源行下标（null 表示不在拖拽中）。 */
  private dragSortFrom: number | null = null;
  private requestVersion = 0;
  private queryAction: "reload" | "filter" | "sort" | "page" = "reload";
  private loadedPageRequest: import("./types").PageRequest | null = null;
  private currentRecord: number | null = null;
  private queryStartedAt: number | null = null;
  private queryElapsedMs: number | null = null;
  private queryTimer: ReturnType<typeof setInterval> | null = null;
  private countingTotal = false;
  /** 计数与页请求分别编号：普通翻页可复用，刷新、筛选和已提交写入必须失效。 */
  private totalVersion = 0;
  private totalSnapshot: { key: string; total: number; at: number } | null = null;
  private totalError = "";
  private totalCountFrame: number | null = null;
  private disposed = false;
  private loadStatus!: HTMLElement;
  private loadingOverlay!: HTMLElement;
  private loadingLabelEl!: HTMLElement;
  private loadingTimerEl!: HTMLElement;
  private filterBarEl!: HTMLElement;
  private filterRowsEl!: HTMLElement;
  private filterPlaceholderEl!: HTMLElement;
  private filterCountEl: HTMLElement | null = null;
  /** 预览开关和格式跟随本数据页生命周期；切页保留，销毁视图自然释放。 */
  previewFormat: "text" | "hex" | "image" | "web" = "text";
  previewPinned = false;
  /** 单元格内容面板（按「文本▼」格式展示焦点单元格）。 */
  private cellPreviewEl!: HTMLElement;
  /** 载入中 / 提交中（工具栏禁用与事务门禁读取）。 */
  loading = false;

  constructor(app: App, tab: TabState) {
    super(app, tab);

    this.el.classList.add("data-grid-controls");
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
          touched: true,
          logic: j === 0 ? undefined : c.logic ?? "OR",
        })),
      }));
      this.renderFilterLines();
    }

    this.grid = new DataGrid({
      onEditStart: (row, col) => {
        if (this.loading || this.committing) {
          this.app.toast("info", "表格正在载入或提交，请稍候");
          return;
        }
        // 使用已按可见列映射、包含草稿的网格值，不能用显示列索引读取完整行。
        const value = this.grid.rows[row]?.[col] ?? null;
        this.editingCol = this.grid.columns[col]?.name ?? "";
        this.grid.beginEdit(row, col, value);
      },
      onEditBegin: () => { this.updateCellPreview(); },
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
      onHeaderContextMenu: (name, _index, x, y) => {
        // 表头右键：升序 / 降序 / 移除排序，与顶部排序面板同一数据源、立即联动。
        if (!name) return;
        this.app.headerSortColumn = name;
        const cur = this.sortRows.find((r) => r.column === name);
        this.app.showContextMenu([
          { id: "header-pin-toggle", label: this.pinnedCols.has(name) ? "取消固定列" : "固定列到最左侧", checked: this.pinnedCols.has(name), disabled: this.loading || this.committing },
          { separator: true },
          { id: "header-sort-asc", label: "ASC 升序排序", checked: cur?.ascending === true },
          { id: "header-sort-desc", label: "DESC 降序排序", checked: cur?.ascending === false },
          { id: "header-sort-remove", label: "移除排序", disabled: !cur },
          { separator: true },
          { id: "grid-filter", label: "筛选..." },
          { id: "grid-sort", label: "排序..." },
        ], x, y);
      },
      onCellFocus: (row) => {
        if (this.loading) return;
        this.currentRecord = row;
        this.updateCellPreview();
        this.app.renderStatus();
      },
      onContextMenu: (x, y) => {
        this.app.showContextMenu(GRID_CONTEXT_MENU, x, y);
      },
    });
    this.loadStatus = document.createElement("div");
    this.loadStatus.className = "data-load-status";
    this.loadStatus.setAttribute("role", "alert");
    this.loadStatus.hidden = true;
    this.el.appendChild(this.loadStatus);
    const body = document.createElement("div"); body.className = "data-grid-body";
    const surface = document.createElement("div"); surface.className = "data-grid-surface";
    const loadingOverlay = document.createElement("div"); loadingOverlay.className = "grid-loading-overlay data-loading-overlay";
    loadingOverlay.setAttribute("role", "status"); loadingOverlay.setAttribute("aria-live", "polite"); loadingOverlay.hidden = true;
    this.loadingOverlay = loadingOverlay;
    this.loadingLabelEl = document.createElement("span");
    this.loadingTimerEl = document.createElement("span"); this.loadingTimerEl.className = "data-loading-timer";
    this.loadingTimerEl.setAttribute("aria-hidden", "true");
    loadingOverlay.append(this.loadingLabelEl, this.loadingTimerEl);
    this.columnSidebar = new ColumnSidebar(`data-columns-${crypto.randomUUID()}`, () => this.allColumns().map(name => ({ id: name, name, comment: this.columnComments.get(name) })), () => this.hiddenColumns(), ids => this.setHiddenColumns(ids));
    this.formHost = document.createElement("div");
    this.formHost.className = "data-form-view-host";
    this.formHost.hidden = true;
    surface.append(this.grid.el, this.formHost, loadingOverlay);
    body.append(surface, this.columnSidebar.el); this.el.appendChild(body);

    // 单元格内容面板
    this.cellPreviewEl = document.createElement("div");
    this.cellPreviewEl.className = "cell-preview";
    // 未选中任何内容时不该占高度，否则表下方会长期挂着一条空面板
    this.cellPreviewEl.style.display = "none";
    this.el.appendChild(this.cellPreviewEl);
    // 「文本」预览只在选中内容时出现（编辑单元格或拖选单元格文本）。
    document.addEventListener("selectionchange", this.selectionListener);

    this.footer = document.createElement("div");
    this.footer.className = "grid-footer";
    this.el.appendChild(this.footer);
    if (tab.page) {
      this.loadedPageRequest = this.pageRequest();
      this.rememberTotal(tab.page, this.loadedPageRequest);
    }
  }

  get total(): number {
    return this.tab.page?.total ?? 0;
  }

  /** 原表行数（不含待插入行）。 */
  private originalRowCount(): number {
    return this.tab.page?.rows.length ?? 0;
  }

  private readOnlyReason(): string {
    if (this.tab.datasourceStale) return "数据源已变化，请重新载入后再编辑";
    if (this.tab.objectType === "view") return "视图数据只读";
    if (this.app.connectionById?.(this.tab.connectionId)?.read_only) return "此连接只读";
    // 查询页持有的手动事务未提交时，禁止同一连接的表数据页以自动提交方式混写。
    const transaction = this.app.manualTransactionFor?.(this.tab.connectionId);
    if (transaction?.active && transaction.owner !== this.tab.id) return "此连接有手动事务，请先在事务页面提交或回滚，再编辑表数据";
    if (this.app.manualTransactionIsBusy?.(this.tab.connectionId)) return "事务操作正在进行，请稍候";
    return "";
  }

  /** 网格当前显示的行（原行 + 追加行，含未提交修改与事务内已应用更改）。 */
  private get displayRows(): (string | null)[][] {
    const page = this.tab.page;
    if (!page) return [];
    const applyEdits = (row: (string | null)[], edits: Map<string, string | null> | undefined) => {
      const out = [...row];
      if (edits) {
        page.columns.forEach((c, ci) => {
          if (edits.has(c.name)) out[ci] = edits.get(c.name) ?? null;
        });
      }
      return out;
    };
    // 事务内已应用的插入行在前（按创建顺序），其后是尚未执行的待插入行。
    const inserted = [
      ...this.txAppliedInserts,
      ...this.pendingInserts.map((row, i) => applyEdits(row, this.pendingInsertEdits.get(i))),
    ];
    const original = page.rows.map((row, r) =>
      applyEdits(row, this.pendingUpdates.get(r) ?? this.txAppliedUpdates.get(r)));
    return [...original, ...inserted];
  }

  hasPendingChanges(): boolean { return this.hasPending(); }
  draftDirty(): boolean { return this.hasPending(); }
  discardDraft(): void {
    this.pendingUpdates.clear(); this.pendingDeletes.clear(); this.pendingInserts = []; this.pendingInsertEdits.clear();
    this.tab.dirty = false; this.applyData();
  }
  draftBusy(): boolean { return this.committing; }
  flushDraft(): void {
    this.grid.flushEditing();
  }
  captureDraft(): Pick<DraftSnapshot, "kind" | "baseline" | "payload"> {
    const page = this.tab.page;
    const rows = [...new Set([...this.pendingUpdates.keys(), ...this.pendingDeletes])].map(index => {
      const original = page?.rows[index];
      if (!original) throw new Error("待改行缺少原始快照");
      let token = this.rowDraftTokens.get(original);
      if (!token) { token = crypto.randomUUID(); this.rowDraftTokens.set(original, token); }
      const keys = page!.primary_keys.map(name => [name, original[page!.columns.findIndex(c => c.name === name)]]);
      const stableKey = keys.length > 0 && keys.every(([, value]) => value !== null && value !== undefined) && page!.rows.filter(row => page!.primary_keys.every(name => {
        const col = page!.columns.findIndex(c => c.name === name); return col >= 0 && row[col] === original[col];
      })).length === 1;
      return { token, identity: { keys, proven_unique: stableKey, requires_validation: true }, original: [...original], changes: [...(this.pendingUpdates.get(index) ?? [])], deleted: this.pendingDeletes.has(index) };
    });
    const inserts = this.pendingInserts.map((row, index) => {
      let token = this.rowDraftTokens.get(row);
      if (!token) { token = crypto.randomUUID(); this.rowDraftTokens.set(row, token); }
      return { token, values: [...row], changes: [...(this.pendingInsertEdits.get(index) ?? [])] };
    });
    return { kind: "row_changes", baseline: { columns: page?.columns ?? [], primary_keys: page?.primary_keys ?? [], rows: rows.map(row => ({ token: row.token, identity: row.identity, original: row.original })) }, payload: { rows, inserts, requires_validation: true, execution_effects_unknown: this.mutationEffectsUnknown } };
  }
  async reloadAfterLeave(): Promise<void> {
    this.reloadingAfterLeave = true;
    try { await this.reload(); } finally { this.reloadingAfterLeave = false; }
  }

  private hasPending(): boolean {
    return (
      this.pendingUpdates.size > 0 ||
      this.pendingDeletes.size > 0 ||
      this.pendingInserts.length > 0
    );
  }

  /** 供工具栏判断行数。 */
  get rowCount(): number {
    return this.originalRowCount() + this.pendingInserts.length;
  }

  /** 行标记（新增 / 删除 / 修改）；事务内已应用的行不再标记（其状态由事务按钮表达）。 */
  private rowFlags(): Record<number, "insert" | "delete" | "dirty"> {
    const flags: Record<number, "insert" | "delete" | "dirty"> = {};
    const base = this.originalRowCount();
    this.txAppliedInserts.forEach((_, i) => {
      flags[base + i] = "insert";
    });
    this.pendingInserts.forEach((_, i) => {
      flags[base + this.txAppliedInserts.length + i] = "insert";
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

  /** 仅调整显示顺序；底层页数据和所有待提交草稿仍按字段名映射。 */
  togglePinnedColumn(name: string): void {
    if (this.loading || this.committing || !this.allColumns().includes(name)) return;
    const selectedName = this.grid.columns[this.grid.selectedColumn]?.name;
    this.grid.flushEditing();
    if (this.pinnedCols.has(name)) this.pinnedCols.delete(name); else this.pinnedCols.add(name);
    this.applyData();
    this.grid.selectedColumn = selectedName === undefined ? -1 : this.grid.columns.findIndex(column => column.name === selectedName);
    this.app.handleColumnSelected(this.tab, this.grid.columns[this.grid.selectedColumn]?.name ?? "");
    this.app.renderStatus();
  }

  setColumnMetadata(columns: import("./types").ColumnInfo[]): void {
    if (this.disposed) return;
    this.columnComments = new Map(columns.flatMap(column => column.comment?.trim() ? [[column.name, column.comment] as [string, string]] : []));
    this.columnMetadataLoaded = true;
    this.columnSidebar.render();
  }

  private async loadColumnMetadata(): Promise<void> {
    if (this.columnMetadataLoaded || this.columnMetadataLoading || this.disposed) return;
    const { connectionId, schema, object } = this.tab;
    this.columnMetadataLoading = true;
    try {
      const metadata = await api.getTableSchema(connectionId, schema, object, false);
      if (!this.disposed && !this.tab.datasourceStale && connectionId === this.tab.connectionId && schema === this.tab.schema && object === this.tab.object) this.setColumnMetadata(metadata.columns);
    } catch { /* 备注不可用时保留字段名；下次打开侧栏可重试，不影响数据页加载。 */ }
    finally { this.columnMetadataLoading = false; }
  }

  /** 切换列显示。 */
  toggleColumn(name: string): void {
    if (!this.allColumns().includes(name)) return;
    const next = this.hiddenColumns();
    if (next.has(name)) next.delete(name); else next.add(name);
    this.setHiddenColumns([...next]);
  }

  private setHiddenColumns(names: string[]): void {
    const all = this.allColumns(), next = new Set(names.filter(name => all.includes(name)));
    if (all.length && all.every(name => next.has(name))) return;
    // 表数据列名唯一：保存列身份，不能把旧显示索引沿用到新的可见列。
    const selectedName = this.grid.columns[this.grid.selectedColumn]?.name;
    // 先提交当前单元格输入到本地草稿，再重建可见列映射。
    this.grid.flushEditing();
    this.hiddenCols = next;
    this.applyData();
    this.grid.selectedColumn = selectedName === undefined ? -1 : this.grid.columns.findIndex(column => column.name === selectedName);
    this.app.handleColumnSelected(this.tab, this.grid.columns[this.grid.selectedColumn]?.name ?? "");
    this.app.renderStatus();
    this.columnSidebar.render();
  }

  columnToolDisabled(): boolean {
    // 工具栏会重建按钮，每次渲染后重新连接控件状态和焦点返回目标。
    queueMicrotask(() => {
      const trigger = document.querySelector<HTMLElement>('.tab-toolbar [data-command="columns"]');
      if (trigger && this.el.isConnected) this.columnSidebar.bind(trigger);
    });
    return !this.allColumns().length;
  }

  toggleColumnSidebar(trigger?: HTMLElement): void {
    if (!this.allColumns().length) return;
    this.grid.flushEditing();
    this.columnSidebar.toggle(trigger);
    if (!this.columnSidebar.el.hidden) void this.loadColumnMetadata();
  }

  /** 原生按钮保留既有紧凑样式，同时支持键盘和清晰可访问名称。 */
  private panelButton(label: string, action: () => void, title = label): HTMLButtonElement {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "fb-btn fb-act";
    b.textContent = label;
    b.title = title;
    b.setAttribute("aria-label", title);
    if (/^(应用筛选|应用排序|清除筛选|清除排序)$/.test(title)) {
      b.classList.add("fb-query-action");
      b.dataset.label = label;
      b.dataset.action = /筛选/.test(title) ? "filter" : "sort";
    }
    b.addEventListener("click", () => { if (!b.disabled) action(); });
    return b;
  }

  private appliedSortRules(): import("./types").SortRule[] {
    return this.tab.order_rules ?? (this.tab.orderBy
      ? [{ column: this.tab.orderBy, ascending: this.tab.ascending }]
      : []);
  }

  /** 顶部内联面板：头部（标题/计数/说明/主次操作）+ 条件或排序行 + 添加按钮。 */
  private buildFilterBar(): HTMLElement {
    const bar = document.createElement("div");
    bar.className = "filter-bar";

    const filterPane = document.createElement("div");
    filterPane.className = "fb-pane fb-filter";
    this.filterCountEl = document.createElement("span");
    this.filterCountEl.className = "fb-count";
    this.filterCountEl.textContent = "0";
    const filterHead = this.buildPanelHead("data_filter", "筛选", this.filterCountEl, "组内 AND · 组间 OR（可切换）", [
      this.panelButton("清除", () => {
        if (!this.allowQueryChange()) return;
        if (this.hasPending()) {
          this.app.toast("info", "存在未提交的修改，请先提交或回滚");
          return;
        }
        this.filterLines = [];
        this.renderFilterLines();
        this.applyFilters();
      }, "清除筛选"),
      this.panelButton("应用筛选", () => this.applyFilters()),
    ]);
    const tools = document.createElement("div");
    tools.className = "filter-tools fb-subhint";
    this.filterPlaceholderEl = document.createElement("div");
    this.filterPlaceholderEl.className = "filter-placeholder";
    this.filterPlaceholderEl.textContent = "一行条件为一组：行内 AND / OR 连接条件，组间 AND / OR 连接各行；＋ 增加下一行组，()＋ 在本行加条件。文本匹配区分大小写；删除全部行后应用可清除筛选。";
    tools.appendChild(this.filterPlaceholderEl);
    this.filterRowsEl = document.createElement("div");
    this.filterRowsEl.className = "filter-rows";
    filterPane.append(filterHead, tools, this.filterRowsEl);

    const sortPane = document.createElement("div");
    sortPane.className = "fb-pane fb-sort";
    this.sortCountEl = document.createElement("span");
    this.sortCountEl.className = "fb-count";
    this.sortCountEl.textContent = "0";
    const sortHead = this.buildPanelHead("tab_sort", "排序", this.sortCountEl, "按从上到下的优先级依次生效", [
      this.panelButton("清除", () => {
        if (!this.allowQueryChange()) return;
        if (this.hasPending()) {
          this.app.toast("info", "存在未提交的修改，请先提交或回滚");
          return;
        }
        this.sortRows = [];
        this.renderSortRows();
        this.applySort();
      }, "清除排序"),
      this.panelButton("应用排序", () => this.applySort()),
    ]);
    const sortTools = document.createElement("div");
    sortTools.className = "filter-actions sort-add-row";
    sortTools.appendChild(this.panelButton("＋ 添加排序字段", () => this.addSortRow()));
    this.sortRowsEl = document.createElement("div");
    this.sortRowsEl.className = "filter-rows sort-rows";
    sortPane.append(sortHead, this.sortRowsEl, sortTools);

    bar.append(filterPane, sortPane);
    return bar;
  }

  /** 排序 / 筛选共用的面板头：图标 + 标题 + 计数 + 说明 + 右侧操作按钮。 */
  private buildPanelHead(icon: string, title: string, countEl: HTMLElement, hint: string, actions: HTMLButtonElement[]): HTMLElement {
    const head = document.createElement("div");
    head.className = "fb-head";
    const left = document.createElement("div");
    left.className = "fb-head-left";
    const iconEl = document.createElement("span");
    iconEl.className = "fb-head-icon";
    iconEl.innerHTML = iconSvgByName(icon);
    const titleEl = document.createElement("span");
    titleEl.className = "fb-head-title";
    titleEl.textContent = title;
    const hintEl = document.createElement("span");
    hintEl.className = "fb-head-hint";
    hintEl.textContent = hint;
    left.append(iconEl, titleEl, countEl, hintEl);
    const right = document.createElement("div");
    right.className = "fb-head-right";
    actions.forEach((button, index) => {
      button.classList.add(index === actions.length - 1 ? "fb-primary" : "fb-ghost");
      right.appendChild(button);
    });
    head.append(left, right);
    return head;
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
    if (this.sortCountEl) this.sortCountEl.textContent = String(this.sortRows.length);
    if (!this.sortRows.length) {
      const empty = document.createElement("div");
      empty.className = "fb-empty";
      const emptyIcon = document.createElement("span");
      emptyIcon.className = "fb-empty-icon";
      emptyIcon.innerHTML = iconSvgByName("tab_sort");
      const emptyText = document.createElement("div");
      emptyText.className = "fb-empty-text";
      const title = document.createElement("div");
      title.className = "fb-empty-title";
      title.textContent = "尚未添加排序字段";
      const sub = document.createElement("div");
      sub.className = "fb-empty-sub";
      sub.textContent = "添加字段后，结果将按从上到下的优先级依次排序。";
      emptyText.append(title, sub);
      empty.append(emptyIcon, emptyText);
      this.sortRowsEl.appendChild(empty);
      return;
    }
    this.sortRows.forEach((row, i) => {
      const el = document.createElement("div");
      el.className = "filter-line sort-line";
      el.setAttribute("data-sort-index", String(i));

      const grip = document.createElement("span");
      grip.className = "fb-grip";
      grip.title = "拖拽调整优先级";
      grip.setAttribute("aria-label", "排序优先级 " + (i + 1) + "，拖拽可调整顺序");
      grip.setAttribute("draggable", "true");
      grip.innerHTML = '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><circle cx="6" cy="4" r="1.3"/><circle cx="10" cy="4" r="1.3"/><circle cx="6" cy="8" r="1.3"/><circle cx="10" cy="8" r="1.3"/><circle cx="6" cy="12" r="1.3"/><circle cx="10" cy="12" r="1.3"/></svg>';
      grip.addEventListener("dragstart", (event: DragEvent) => {
        this.dragSortFrom = i;
        el.classList.add("dragging");
        try {
          if (event.dataTransfer) {
            event.dataTransfer.setData("text/plain", String(i));
            event.dataTransfer.effectAllowed = "move";
          }
        } catch { /* 某些环境没有 DataTransfer */ }
      });
      grip.addEventListener("dragend", () => {
        this.dragSortFrom = null;
        el.classList.remove("dragging");
        this.sortRowsEl.querySelectorAll(".sort-line").forEach((node) => node.classList.remove("drag-over"));
      });
      el.addEventListener("dragover", (event) => {
        if (this.dragSortFrom === null) return;
        event.preventDefault();
        el.classList.add("drag-over");
      });
      el.addEventListener("dragleave", () => el.classList.remove("drag-over"));
      el.addEventListener("drop", (event) => {
        event.preventDefault();
        el.classList.remove("drag-over");
        const from = this.dragSortFrom;
        if (from === null || from === i) return;
        const moved = this.sortRows.splice(from, 1)[0];
        if (!moved) return;
        this.sortRows.splice(i, 0, moved);
        this.dragSortFrom = null;
        this.renderSortRows();
      });

      const priority = document.createElement("span");
      priority.className = "fb-priority";
      priority.textContent = String(i + 1);

      const col = document.createElement("select");
      col.className = "fb-txt";
      for (const name of this.allColumns().filter(name => name === row.column || !this.sortRows.some(rule => rule.column === name))) {
        const option = document.createElement("option"); option.value = name; option.textContent = name; col.appendChild(option);
      }
      col.value = row.column;
      col.setAttribute("aria-label", "排序字段 " + (i + 1));
      col.title = "点击修改排序字段";
      col.addEventListener("change", () => {
        if (this.sortRows.some((rule, index) => index !== i && rule.column === col.value)) { col.value = row.column; return; }
        row.column = col.value;
        this.renderSortRows();
      });

      const dir = document.createElement("button"); dir.type = "button";
      dir.className = "fb-txt fb-dir";
      dir.textContent = row.ascending ? "ASC" : "DESC";
      dir.dataset.dir = row.ascending ? "asc" : "desc";
      dir.setAttribute("aria-label", "排序方向 " + (i + 1) + "：" + (row.ascending ? "ASC 升序" : "DESC 降序") + "，点击切换");
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
      const up = this.panelButton("", () => move(-1), "上移"); up.innerHTML = iconSvgByName("dbn_prior");
      up.disabled = i === 0;
      const down = this.panelButton("", () => move(1), "下移"); down.innerHTML = iconSvgByName("dbn_next");
      down.disabled = i === this.sortRows.length - 1;
      const remove = this.panelButton("×", () => {
        this.sortRows.splice(i, 1);
        this.renderSortRows();
      }, "删除排序字段");
      remove.classList.add("fb-del");
      const grow = document.createElement("span");
      grow.className = "fb-grow";
      el.append(grip, priority, col, dir, up, down, grow, remove);
      this.sortRowsEl.appendChild(el);
    });
  }

  /** 新增一行（单个条件）。同一行内可继续追加条件，整行括为一组。 */
  private addFilterLine(after = this.filterLines.length - 1): void {
    const cols = this.allColumns();
    if (!cols.length) {
      this.app.toast("info", "当前表还没有列");
      return;
    }
    const selected = this.grid.columns[this.grid.selectedColumn]?.name;
    // 新行沿用上一行的连接方式，保持组语义自然延续
    const last = this.filterLines[after];
    this.filterLines.splice(after + 1, 0, {
      logic: this.filterLines.length ? last?.logic ?? "AND" : undefined,
      conds: [{ col: selected && cols.includes(selected) ? selected : cols[0], op: "=", val: "", touched: false }],
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
      touched: false,
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

  /** 筛选运算符：内部值（小写，后端兼容）→ SQL 显示文本。IN/NOT IN 值为逗号分隔列表。 */
  private static readonly FILTER_OPS: [string, string][] = [
    ["=", "="], ["<>", "<>"], [">", ">"], [">=", ">="], ["<", "<"], ["<=", "<="],
    ["like", "LIKE"], ["not like", "NOT LIKE"], ["in", "IN"], ["not in", "NOT IN"],
    ["is null", "IS NULL"], ["is not null", "IS NOT NULL"],
  ];

  /** AND / OR 内联文本，点击就地切换（不重绘整行）。 */
  private joinText(
    value: import("./types").FilterLogic,
    on: (v: import("./types").FilterLogic) => void,
  ): HTMLElement {
    return filterJoinControl(value, on);
  }

  /** 单条条件 chip：列名 / 操作符 / 值均为无边框文本，点击修改。 */
  private buildCondChip(
    c: { col: string; op: string; val: string; touched?: boolean; logic?: import("./types").FilterLogic },
    k: number,
    j: number,
  ): HTMLElement {
    const chip = document.createElement("div");
    chip.className = "cond-chip";
    const col = document.createElement("select");
    col.className = "fb-txt";
    for (const name of this.allColumns()) { const option = document.createElement("option"); option.value = name; option.textContent = name; col.appendChild(option); }
    col.value = c.col;
    col.setAttribute("aria-label", `筛选字段 ${k + 1}.${j + 1}`);
    col.title = "点击修改字段";
    col.addEventListener("change", () => {
      c.col = col.value;
    });
    const op = document.createElement("select");
    op.className = "fb-txt fb-op-txt";
    for (const [value, label] of DataTabView.FILTER_OPS) { const option = document.createElement("option"); option.value = value; option.textContent = label; op.appendChild(option); }
    op.value = c.op;
    op.setAttribute("aria-label", `筛选运算符 ${k + 1}.${j + 1}`);
    op.title = "点击修改操作符";
    op.addEventListener("change", () => {
      c.op = op.value;
      chip.querySelector(".fb-val")?.replaceWith(this.buildValText(c, k, j));
    });
    chip.append(col, op, this.buildValText(c, k, j));
    const del = this.panelButton("×", () => this.removeCond(k, j), "删除该条件");
    del.classList.add("fb-cond-del");
    chip.appendChild(del);
    return chip;
  }

  /** 值文本：点击就地出现输入框，Enter 提交并应用、失焦提交、Esc 取消。 */
  private buildValText(
    c: { op: string; val: string; touched?: boolean },
    k: number,
    j: number,
  ): HTMLElement {
    return filterValueControl(c, `筛选值 ${k + 1}.${j + 1}`, enter => { if (enter) void this.applyFilters(); });
  }

  /** 重建筛选行 DOM：条件组（左侧组标签/轨道 + 条件行 + 组内加条件），组间虚线分隔。 */
  private renderFilterLines(): void {
    this.filterRowsEl.innerHTML = "";
    const totalConds = this.filterLines.reduce((count, line) => count + line.conds.length, 0);
    if (this.filterCountEl) this.filterCountEl.textContent = this.filterLines.length + " 组 · " + totalConds + " 条件";
    this.filterPlaceholderEl.style.display = "";
    if (!this.filterLines.length) {
      const emptyAdd = this.panelButton("＋ 添加条件组", () => this.addFilterLine(), "添加条件组");
      emptyAdd.classList.add("fb-add-group");
      this.filterRowsEl.appendChild(emptyAdd);
      return;
    }
    this.filterLines.forEach((line, k) => {
      const el = document.createElement("div");
      el.className = "filter-line fb-group";
      el.setAttribute("data-group-index", String(k));

      const main = document.createElement("div");
      main.className = "fb-group-main";

      const side = document.createElement("div");
      side.className = "fb-group-side";
      if (k > 0) {
        // 组间连接方式：点击胶囊在 AND / OR 间切换
        side.appendChild(this.joinText(line.logic ?? "AND", (v) => (line.logic = v)));
      } else {
        const tag = document.createElement("span");
        tag.className = "fb-group-tag";
        tag.textContent = "WHERE";
        side.appendChild(tag);
      }
      const rail = document.createElement("span");
      rail.className = "fb-group-rail";
      side.appendChild(rail);

      const group = document.createElement("div");
      group.className = "cond-group";
      // 括号语义保留在 DOM（多条件组），视觉上由左侧轨道表达，不显示括号字符。
      if (line.conds.length > 1) {
        const brOpen = document.createElement("span");
        brOpen.className = "fb-bracket";
        brOpen.textContent = "（";
        group.appendChild(brOpen);
      }
      line.conds.forEach((c, j) => {
        const condRow = document.createElement("div");
        condRow.className = "fb-cond-row";
        if (j === 0) {
          // 该组第一条条件默认与组起点 AND，用静态 A 标记（与设计一致）
          const firstTag = document.createElement("span");
          firstTag.className = "fb-cond-tag";
          firstTag.textContent = "A";
          condRow.appendChild(firstTag);
        } else {
          // 行内连接：可见 A / O 方形徽标；真实连接控件保留（离屏）以维持交互与测试契约
          const conn = document.createElement("button");
          conn.type = "button";
          conn.className = "fb-cond-conn";
          conn.textContent = (c.logic ?? "OR") === "OR" ? "O" : "A";
          conn.title = "点击切换行内 AND / OR";
          conn.setAttribute("aria-label", "行内条件连接：" + ((c.logic ?? "OR") === "OR" ? "OR" : "AND") + "，点击切换");
          conn.addEventListener("click", () => {
            c.logic = (c.logic ?? "OR") === "OR" ? "AND" : "OR";
            this.renderFilterLines();
          });
          const join = this.joinText(c.logic ?? "OR", (v) => (c.logic = v));
          join.classList.add("fb-join-hidden");
          condRow.append(conn, join);
        }
        condRow.appendChild(this.buildCondChip(c, k, j));
        group.appendChild(condRow);
      });
      if (line.conds.length > 1) {
        const brClose = document.createElement("span");
        brClose.className = "fb-bracket";
        brClose.textContent = "）";
        group.appendChild(brClose);
      }
      const addCond = this.panelButton("＋ 在本组加条件", () => this.addLineCond(k), "在本组追加条件");
      addCond.classList.add("fb-add-cond");
      group.appendChild(addCond);

      main.append(side, group);
      // 兼容既有交互契约：保留 +（下一组）/ ()+（本组加条件）/ ✕（删除整组），前两者离屏隐藏。
      const addNext = this.panelButton("＋", () => this.addFilterLine(k), "在本行后增加下一行条件组");
      const addCondLegacy = this.panelButton("()＋", () => this.addLineCond(k), "在本行追加条件（整行括为一组）");
      [addNext, addCondLegacy].forEach((button) => button.classList.add("fb-legacy-hidden"));
      const delLine = this.panelButton("✕", () => {
        this.filterLines.splice(k, 1);
        this.renderFilterLines();
      }, "删除整行");
      delLine.classList.add("fb-group-del");
      el.append(main, addNext, addCondLegacy, delLine);
      this.filterRowsEl.appendChild(el);
    });
    const addGroup = this.panelButton("＋ 添加条件组", () => this.addFilterLine(this.filterLines.length - 1), "添加条件组");
    addGroup.classList.add("fb-add-group");
    this.filterRowsEl.appendChild(addGroup);
  }

  /** 打开筛选只显示草稿；仅用户点击加号才新增条件。 */
  focusFilter(): void {
    this.barMode = this.barMode === "filter" ? null : "filter";
    this.renderBar();
    if (this.barMode === "filter") {
      this.renderFilterLines();
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

  /** 表头右键排序：与排序面板共用 sortRows，立即应用并同步面板显示。 */
  applyHeaderSort(column: string, mode: "asc" | "desc" | "remove"): void {
    if (!this.allColumns().includes(column) || !this.allowQueryChange()) return;
    if (this.hasPending()) {
      this.app.toast("info", "存在未提交的修改，请先提交或回滚");
      return;
    }
    if (this.transactionOwnedByThisTab()) {
      this.app.toast("info", "手动事务进行中：请先提交或回滚事务，再修改排序");
      return;
    }
    const rules = this.sortRows.map(rule => ({ ...rule })), index = rules.findIndex(rule => rule.column === column);
    if (mode === "remove") { if (index >= 0) rules.splice(index, 1); }
    else if (index >= 0) rules[index].ascending = mode === "asc";
    else rules.push({ column, ascending: mode === "asc" });
    this.sortRows = rules;
    this.renderSortRows();
    this.applySort();
  }

  /** 应用筛选：一行一括号组（行内多条自动括起），行间按行连接方式并列，
   *  直接对应 a=1 AND (b=2 OR c=3) AND d=4；空串是有效值，不悄悄丢弃条件。 */
  private applyFilters(): void {
    if (!this.allowQueryChange()) return;
    if (this.hasPending()) {
      this.app.toast("info", "存在未提交的修改，请先提交或回滚");
      return;
    }
    const incomplete = this.filterLines.some(line => line.conds.some(c => !/^(is null|is not null)$/i.test(c.op.trim()) && !c.val && c.touched !== true));
    if (incomplete) { this.app.toast("info", "筛选条件尚未填写：点击 <?> 输入值，空字符串请在输入框中明确确认"); return; }
    // filter_logic 仅作旧字段回退；实际连接方式已逐行 / 行内保存
    this.tab.filter_logic = "AND";
    this.tab.filters = this.filterLines.map((l, k) => ({
      logic: k > 0 ? l.logic ?? "AND" : undefined,
      conds: l.conds.map((c, j) => ({
        column: c.col,
        operator: c.op,
        value: c.val,
        case_sensitive: true,
        logic: j > 0 ? c.logic ?? "OR" : undefined,
      })),
    }));
    this.tab.pageOffset = 0;
    void this.reload("filter");
    this.app.renderStatus();
  }

  /** 多字段排序数组顺序即优先级；删除全部后应用清除排序。 */
  private applySort(): void {
    if (!this.allowQueryChange()) return;
    if (this.hasPending()) {
      this.app.toast("info", "存在未提交的修改，请先提交或回滚");
      return;
    }
    this.tab.order_rules = this.sortRows.map((r) => ({ ...r }));
    this.tab.orderBy = this.sortRows[0]?.column ?? null;
    this.tab.ascending = this.sortRows[0]?.ascending ?? true;
    this.tab.pageOffset = 0;
    void this.reload("sort");
    this.app.renderStatus();
  }

  /** 改变查询前先把网格输入落为本地草稿，所有门禁在改已应用状态之前执行。 */
  private allowQueryChange(): boolean {
    if (this.loading || this.committing) { this.app.toast("info", "表格正在载入或提交，请稍候"); return false; }
    this.grid.flushEditing();
    if (this.hasPending()) { this.app.toast("info", "存在未提交的修改，请先提交或回滚"); return false; }
    if (this.transactionOwnedByThisTab()) { this.app.toast("info", "手动事务进行中：请先提交或回滚事务，再筛选或排序"); return false; }
    return true;
  }

  /** selectionchange → 预览面板只在有选中内容时出现。 */
  private selectionListener = (): void => {
    this.updateCellPreview();
  };

  /** 网格内当前拖选的文本（无选中或选区不在本网格时为空串）。 */
  private gridSelectionText(): string {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || !sel.anchorNode) return "";
    if (!this.grid.el.contains(sel.anchorNode)) return "";
    return sel.toString();
  }

  /**
   * 单元格内容面板：仅当前数据页打开预览开关后，由焦点/选中内容决定显隐。
   * 未打开开关、没有选中内容或已切到其它页时不占用高度；不回退首格。
   * 格式和开关保留在所属数据视图内，返回该页可继续预览。
   * 在面板内选择文本时不重渲染，避免打断选区。
   */
  updateCellPreview(): void {
    if (this.app.activeTab?.id !== this.tab.id || !this.app.textPreviewActive(this.tab.id)) {
      this.cellPreviewEl.style.display = "none";
      this.cellPreviewEl.innerHTML = "";
      return;
    }
    const activeSel = window.getSelection();
    if (activeSel && activeSel.anchorNode && this.cellPreviewEl.contains(activeSel.anchorNode)) return;
    const selected = this.gridSelectionText();
    const cell = this.grid.focusedCell;
    if (!this.grid.editingActive && !selected && !cell) {
      this.cellPreviewEl.style.display = "none";
      this.cellPreviewEl.innerHTML = "";
      return;
    }
    const value = selected || (cell ? this.grid.rows[cell.row]?.[cell.col] ?? null : null);
    this.cellPreviewEl.style.display = "";
    this.renderCellPreviewWithClose(value);
    this.app.renderStatus();
  }

  /** 渲染预览面板；格式预览可关闭，关闭即回到文本显示。 */
  private renderCellPreviewWithClose(value: string | null): void {
    this.app.renderPreviewPanel(this.cellPreviewEl, value, this.tab.id);
  }

  private totalIsExact(page = this.tab.page): boolean {
    return !!page && page.total_exact !== false;
  }

  private hasNextPage(page = this.tab.page): boolean {
    if (!page) return false;
    // 当前页的多取一行证据优先于计数快照，外部新插入不能被旧总数挡住。
    return page.has_more ?? (this.totalIsExact(page)
      ? this.tab.pageOffset + this.tab.pageLimit < page.total
      : page.rows.length >= this.tab.pageLimit);
  }

  private pageRequest(includeTotal = false): import("./types").PageRequest {
    return {
      connection_id: this.tab.connectionId,
      schema: this.tab.schema,
      table: this.tab.object,
      offset: this.tab.pageOffset,
      limit: this.tab.pageLimit,
      order_by: this.tab.orderBy,
      ascending: this.tab.ascending,
      filters: this.tab.filters.map(line => ({ ...line, conds: line.conds.map(condition => ({ ...condition, case_sensitive: true })) })),
      filter_logic: this.tab.filter_logic ?? "AND",
      order_rules: this.appliedSortRules(),
      include_total: includeTotal,
    };
  }

  /** 总数只取决于表和已应用筛选，不取决于页码、页大小或排序。 */
  private totalKey(request = this.loadedPageRequest ?? this.pageRequest()): string {
    return JSON.stringify([request.connection_id, request.schema, request.table, request.filters, request.filter_logic ?? "AND"]);
  }

  private rememberTotal(page: import("./types").PageResult, request: import("./types").PageRequest): void {
    if (!this.totalIsExact(page)) return;
    ++this.totalVersion; // 新的精确页证据优先于更早启动、尚未返回的 COUNT。
    this.totalSnapshot = { key: this.totalKey(request), total: page.total, at: Date.now() };
    this.totalError = "";
  }

  /** 数据库已变更时失效，不通过草稿数加减伪造过滤后总数。也用于部分成功和未知回执。 */
  invalidateTotal(): void {
    ++this.totalVersion;
    this.totalSnapshot = null;
    this.totalError = "";
    if (this.totalCountFrame !== null) cancelAnimationFrame(this.totalCountFrame);
    this.totalCountFrame = null;
    if (this.tab.page) this.tab.page.total_exact = false;
  }

  private totalBlocked(): boolean {
    return this.disposed || !!this.tab.datasourceStale || this.loading || this.committing || !!this.mutationEffectsUnknown
      || !!this.app.manualTransactionFor?.(this.tab.connectionId)?.active || !!this.app.manualTransactionIsBusy?.(this.tab.connectionId);
  }

  /** 先让首屏绘制；没有计数轮询，每个视图最多一笔真实统计请求。 */
  private scheduleTotalCount(): void {
    if (!this.tab.page || this.totalIsExact() || this.totalBlocked() || this.countingTotal || this.totalError || this.totalCountFrame !== null) return;
    this.totalCountFrame = requestAnimationFrame(() => {
      // rAF 在绘制前执行；再等一帧，确保网格先有一次实际绘制机会。
      this.totalCountFrame = requestAnimationFrame(() => {
        this.totalCountFrame = null;
        void this.countTotal(true);
      });
    });
  }

  private totalText(): string {
    if (this.totalIsExact()) return `共 ${this.tab.page!.total} 条`;
    if (this.mutationEffectsUnknown) return "总数待核对";
    if (this.app.manualTransactionFor?.(this.tab.connectionId)?.active) return "总数待事务结束后统计";
    if (this.totalError) return "总数统计失败，可重试";
    return this.countingTotal || this.totalCountFrame !== null ? "总数统计中…" : "总数未统计";
  }

  /** 独立后台统计：可翻页、可编辑；只更新数字，不重置网格、焦点或草稿。 */
  private async countTotal(automatic = false): Promise<void> {
    if (!this.tab.page || this.totalBlocked() || this.countingTotal || (automatic && this.totalIsExact())) return;
    if (this.totalCountFrame !== null) cancelAnimationFrame(this.totalCountFrame);
    this.totalCountFrame = null;
    const request = { ...(this.loadedPageRequest ?? this.pageRequest()), include_total: true, offset: 0, limit: 0 };
    const key = this.totalKey(request), version = this.totalVersion;
    const current = () => !this.disposed && !this.tab.datasourceStale && version === this.totalVersion
      && key === this.totalKey() && key === this.totalKey(this.pageRequest()) && !this.mutationEffectsUnknown && !this.committing
      && !this.app.manualTransactionFor?.(this.tab.connectionId)?.active && !this.app.manualTransactionIsBusy?.(this.tab.connectionId);
    this.totalError = "";
    this.countingTotal = true;
    this.renderFooter(this.tab.page);
    this.app.renderStatus();
    try {
      const result = await api.pageTable(request);
      if (!current()) return;
      if (this.totalIsExact(result)) {
        const page = this.tab.page;
        const minimum = page?.rows.length ? (this.loadedPageRequest?.offset ?? this.tab.pageOffset) + page.rows.length + (page.has_more ? 1 : 0) : 0;
        if (result.total < minimum) {
          this.totalSnapshot = null;
          if (page) page.total_exact = false;
          this.totalError = "统计快照已过期，请重新统计";
          return;
        }
        this.rememberTotal(result, request);
        if (this.tab.page) {
          this.tab.page.total = result.total;
          this.tab.page.total_exact = true;
          // 保留当前页 has_more 的证据，不能用异步计数改写。
        }
      } else {
        this.totalError = "当前数据源不支持精确总数统计";
        if (!automatic) this.app.toast("info", this.totalError);
      }
    } catch (e) {
      if (current()) {
        this.totalError = `统计失败：${String(e)}`;
        if (!automatic) this.app.toast("error", this.totalError);
      }
    } finally {
      this.countingTotal = false;
      if (!this.disposed && this.tab.page) {
        // 旧请求结束后只补最新版本；错误不自动无限重试。
        if (version !== this.totalVersion || key !== this.totalKey()) this.scheduleTotalCount();
        this.renderFooter(this.tab.page);
        this.app.renderStatus();
      }
    }
  }

  private queryElapsedText(): string {
    const startedAt = this.queryStartedAt;
    const elapsed = startedAt !== null ? performance.now() - startedAt : this.queryElapsedMs ?? this.tab.page?.elapsed_ms;
    if (elapsed == null) return "";
    return `查询耗时: ${(Math.max(0, elapsed) / 1000).toFixed(3)}s${startedAt !== null ? "（查询中）" : ""}`;
  }

  private clearQueryTimer(): void {
    if (this.queryTimer !== null) clearInterval(this.queryTimer);
    this.queryTimer = null;
  }

  private startQueryTiming(): void {
    this.clearQueryTimer();
    this.queryElapsedMs = 0;
    this.queryStartedAt = performance.now();
    this.queryTimer = setInterval(() => {
      if (this.disposed || !this.loading) return;
      this.loadingTimerEl.textContent = this.queryLoadingElapsed();
      this.app.updateTableQueryElapsed(this.tab.id, this.queryElapsedText());
    }, 50);
  }

  private stopQueryTiming(version: number): void {
    if (version !== this.requestVersion || this.queryStartedAt === null) return;
    // 保留从发起请求到返回的实际耗时，不能改用后端耗时而导致读数倒退。
    this.queryElapsedMs = performance.now() - this.queryStartedAt;
    this.queryStartedAt = null;
    this.clearQueryTimer();
    this.app.updateTableQueryElapsed(this.tab.id, this.queryElapsedText());
  }

  /** 发起后同步反馈并锁定旧数据与查询草稿；不依赖后端首次响应。 */
  private updateQueryBusy(): void {
    this.el.classList.toggle("data-query-busy", this.loading);
    this.loadingOverlay.hidden = !this.loading;
    this.loadingLabelEl.textContent = this.loading ? "正在查询… " : "";
    this.loadingTimerEl.textContent = this.loading ? this.queryLoadingElapsed() : "";
    this.columnSidebar.el.inert = this.loading;
    this.grid.el.inert = this.loading;
    this.formHost.inert = this.loading;
    this.filterBarEl.inert = this.loading;
    this.filterRowsEl.inert = this.loading;
    this.sortRowsEl.inert = this.loading;
    this.filterBarEl.querySelectorAll<HTMLButtonElement>(".fb-query-action").forEach(button => {
      button.disabled = this.loading;
      const label = button.dataset.label ?? "";
      button.textContent = this.loading && button.dataset.action === this.queryAction && label.startsWith("应用")
        ? `${label}中…` : label;
      button.setAttribute("aria-busy", String(this.loading && button.dataset.action === this.queryAction));
    });
  }

  private queryLoadingElapsed(): string {
    const elapsed = this.queryStartedAt === null ? this.queryElapsedMs ?? 0 : performance.now() - this.queryStartedAt;
    return `${(Math.max(0, elapsed) / 1000).toFixed(3)} s`;
  }

  /** 状态栏信息。 */
  statusInfo(): { sql: string; pageText: string; elapsed: string } {
    const page = this.tab.page;
    const full = this.tab.schema
      ? `${this.tab.schema}.${this.tab.object}`
      : this.tab.object;
    const pageNo = Math.floor(this.tab.pageOffset / this.tab.pageLimit) + 1;
    const totalText = this.totalText();
    const index = this.viewMode() === "form" ? this.formIndex : this.currentRecord ?? 0;
    const recordText = page?.rows.length ? `第 ${this.tab.pageOffset + Math.min(Math.max(0, index), page.rows.length - 1) + 1} 条记录` : "当前页无记录";
    return {
      sql: `SELECT * FROM ${full} LIMIT ${this.tab.pageOffset},${this.tab.pageLimit}`,
      elapsed: this.queryElapsedText(),
      pageText: this.loading ? "正在查询…" : page
        ? `${recordText}（${totalText}），第 ${pageNo} 页`
        : "",
    };
  }

  activate(): void {
    if (!this.tab.page) {
      if (!this.loading) void this.reload();
    } else {
      this.applyData();
      this.scheduleTotalCount();
    }
  }

  async reload(action: "reload" | "filter" | "sort" | "page" = "reload"): Promise<void> {
    if (this.disposed) return;
    if (this.hasPending() && !this.reloadingAfterLeave) {
      this.app.toast("info", "存在未提交的修改，请先提交（✓）或回滚（✗）");
      return;
    }
    // 事务进行中不允许重读：分页 / 筛选 / 排序都会重置行序，使事务内已应用更改的行定位失效。
    if (this.transactionOwnedByThisTab()) {
      this.app.toast("info", "手动事务进行中：请先「提交事务 / 回滚事务」，再筛选、排序或刷新");
      return;
    }
    const version = ++this.requestVersion;
    const request = this.pageRequest();
    if (action === "reload" || action === "filter" || (this.loadedPageRequest && this.totalKey(request) !== this.totalKey())) this.invalidateTotal();
    let received = false;
    this.queryAction = action;
    this.loading = true;
    this.el.setAttribute("aria-busy", "true");
    this.startQueryTiming();
    this.updateQueryBusy();
    this.loadStatus.hidden = true;
    this.loadStatus.textContent = "";
    if (this.tab.page) this.renderFooter(this.tab.page);
    this.app.notifyToolbar();
    this.app.renderStatus();
    try {
      const res = await api.pageTable(request);
      this.stopQueryTiming(version);
      // 快速连续应用时旧响应不得覆盖新条件的数据，也不得覆盖编辑中的网格。
      if (version !== this.requestVersion || (this.hasPending() && !this.reloadingAfterLeave)) return;
      if (this.reloadingAfterLeave) {
        this.pendingUpdates.clear(); this.pendingDeletes.clear(); this.pendingInserts = []; this.pendingInsertEdits.clear();
      }
      const snapshot = this.totalSnapshot;
      if (this.totalIsExact(res)) {
        // 短页已证明总数，无需额外 COUNT；也处理外部删除后的尾页。
        this.rememberTotal(res, request);
      } else if (snapshot?.key === this.totalKey(request)) {
        // 当前页证据超出快照表示有外部新插入，不得沿用旧总页数。
        if (res.rows.length && request.offset + res.rows.length + (res.has_more ? 1 : 0) > snapshot.total) this.invalidateTotal();
        else { res.total = snapshot.total; res.total_exact = true; }
      }
      this.tab.page = res;
      this.loadedPageRequest = request;
      received = true;
      this.currentRecord = null;
      if (action !== "reload") this.formIndex = 0;
      this.mutationEffectsUnknown = null;
      this.tab.selectedRows.clear();
      // 请求已完成，解除载入态后再渲染网格页脚操作。
      this.loading = false;
      this.loadStatus.hidden = true;
      this.loadStatus.textContent = "";
      this.applyData();
      this.renderFilterLines();
      this.renderSortRows();
      this.app.updateInfoPanelPublic();
      this.app.notifyToolbar();
      this.app.renderStatus();
    } catch (e) {
      this.stopQueryTiming(version);
      if (version === this.requestVersion) {
        // 失败仍保留上次结果，必须同步恢复它的页码与已应用条件，草稿可修改后重试。
        const previous = this.loadedPageRequest;
        if (this.tab.page && previous && !this.reloadingAfterLeave) {
          this.tab.pageOffset = previous.offset;
          this.tab.orderBy = previous.order_by;
          this.tab.ascending = previous.ascending;
          this.tab.order_rules = previous.order_rules?.map(rule => ({ ...rule }));
          this.tab.filters = previous.filters.map(line => ({ ...line, conds: line.conds.map(condition => ({ ...condition })) }));
          this.tab.filter_logic = previous.filter_logic;
        }
        this.loadStatus.hidden = false;
        this.loadStatus.textContent = `当前页加载失败，可再次应用或刷新重试${this.tab.page ? "（仍显示上次结果）" : ""}：${String(e)}`;
        if (!this.reloadingAfterLeave) this.app.toast("error", String(e));
      }
      if (this.reloadingAfterLeave) throw e;
    } finally {
      if (version === this.requestVersion) {
        this.loading = false;
        this.el.setAttribute("aria-busy", "false");
        this.updateQueryBusy();
        if (received) this.scheduleTotalCount();
        if (this.tab.page) this.renderFooter(this.tab.page);
        this.app.notifyToolbar();
        this.app.renderStatus();
      }
    }
  }

  private applyData(): void {
    const page = this.tab.page;
    if (!page) return;
    this.hiddenCols = new Set([...this.hiddenCols].filter(name => page.columns.some(column => column.name === name)));
    if (page.columns.length && page.columns.every(column => this.hiddenCols.has(column.name))) this.hiddenCols.delete(page.columns[0].name);
    this.pinnedCols = new Set([...this.pinnedCols].filter(name => page.columns.some(column => column.name === name)));
    const visible = page.columns.filter(column => !this.hiddenCols.has(column.name));
    const cols = [
      ...[...this.pinnedCols].flatMap(name => visible.filter(column => column.name === name)),
      ...visible.filter(column => !this.pinnedCols.has(column.name)),
    ];
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
        if (di !== undefined) dirtyCells.add(`${this.originalRowCount() + this.txAppliedInserts.length + r}:${di}`);
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
      // 表数据不再显示左侧 rowid/行号列。
      showRowNumbers: false,
      pinnedColumns: [...this.pinnedCols],
    });
    const formMode = this.tab.viewMode === "form" && !!this.formHost;
    if (formMode) {
      if (this.grid.el) this.grid.el.style.display = "none";
      this.formHost.hidden = false;
      this.renderForm();
    } else {
      if (this.grid.el) this.grid.el.style.display = "";
      if (this.formHost) { this.formHost.hidden = true; this.formHost.replaceChildren(); }
    }
    this.renderFooter(page);
    this.columnSidebar?.render();
    // 重载/翻页/切换列后网格没有焦点单元格：预览面板随之收起，不残留上一页的内容。
    const preview = this.cellPreviewEl;
    if (preview && !this.grid.editingActive && !this.grid.focusedCell) { preview.style.display = "none"; preview.innerHTML = ""; }
  }

  private renderFooter(page: import("../core/types").PageResult): void {
    this.footer.innerHTML = "";
    const total = page.total;
    const exact = this.totalIsExact(page);
    const pages = Math.max(1, Math.ceil(total / this.tab.pageLimit));
    const pageNo = Math.floor(this.tab.pageOffset / this.tab.pageLimit) + 1;
    const hasPending = this.hasPending();
    const writeBlocked = this.committing || this.loading || !!this.mutationEffectsUnknown || !!this.readOnlyReason();

    const svgBtn = (icon: IconDef, title: string) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "grid-footer-btn";
      b.title = title;
      b.setAttribute("aria-label", title);
      b.innerHTML = svg(icon);
      return b;
    };

    // 左：增 / 删 / 提交 / 回滚（网格底部工具）
    const addBtn = svgBtn(ICON_PLUS_LINE, "插入记录");
    addBtn.disabled = writeBlocked;
    addBtn.addEventListener("click", () => void this.insertRow());
    this.footer.appendChild(addBtn);

    const delBtn = svgBtn(ICON_MINUS_LINE, "删除记录");
    delBtn.disabled = writeBlocked;
    delBtn.addEventListener("click", () => void this.deleteSelectedRows());
    this.footer.appendChild(delBtn);

    const commitBtn = svgBtn(ICON_CHECK, "提交改动");
    commitBtn.disabled = !hasPending || writeBlocked;
    commitBtn.style.opacity = commitBtn.disabled ? "0.45" : "1";
    commitBtn.addEventListener("click", () => void this.commitEdits());
    this.footer.appendChild(commitBtn);

    const rollbackBtn = svgBtn(ICON_CROSS, "清除本机未提交草稿（不执行数据库回滚）");
    rollbackBtn.disabled = !hasPending || this.committing || this.loading;
    rollbackBtn.style.opacity = hasPending ? "1" : "0.45";
    rollbackBtn.addEventListener("click", () => this.rollbackEdits());
    this.footer.appendChild(rollbackBtn);

    const pager = document.createElement("nav");
    pager.className = "grid-pager";
    pager.setAttribute("aria-label", "表格分页");

    const mkPage = (icon: IconDef, title: string, disabled: boolean, onClick: () => void) => {
      const b = svgBtn(icon, title);
      b.disabled = disabled || this.loading || this.transactionOwnedByThisTab();
      b.addEventListener("click", () => {
        if (b.disabled || !this.allowQueryChange()) return;
        if (this.hasPending()) {
          this.app.toast("info", "存在未提交的修改，请先提交或回滚");
          return;
        }
        onClick();
      });
      pager.appendChild(b);
    };

    mkPage(ICON_PAGE_FIRST, "第一页", pageNo <= 1, () => {
      this.tab.pageOffset = 0;
      void this.reload("page");
    });
    mkPage(ICON_PAGE_PREV, "上一页", pageNo <= 1, () => {
      this.tab.pageOffset = Math.max(0, this.tab.pageOffset - this.tab.pageLimit);
      void this.reload("page");
    });

    const pagePosition = document.createElement("form");
    pagePosition.className = "grid-pager-position";
    const pageBox = document.createElement("input");
    pageBox.type = "text";
    pageBox.inputMode = "numeric";
    pageBox.pattern = "[0-9]*";
    pageBox.value = String(pageNo);
    pageBox.setAttribute("aria-label", "当前页码");
    pageBox.disabled = this.loading || this.transactionOwnedByThisTab();
    pageBox.title = exact ? `输入页码跳转（共 ${pages} 页）` : "输入页码跳转（总数未统计，深页可能较慢）";
    const goToPage = () => {
      if (pageBox.disabled || this.loading || this.committing) return;
      const n = Number(pageBox.value);
      if (!Number.isSafeInteger(n) || n < 1 || !Number.isSafeInteger((n - 1) * this.tab.pageLimit)) {
        pageBox.value = String(pageNo);
        return;
      }
      if (this.hasPending()) {
        this.app.toast("info", "存在未提交的修改，请先提交或回滚");
        pageBox.value = String(pageNo);
        return;
      }
      const nextPage = exact ? Math.min(n, pages) : n;
      pageBox.value = String(nextPage);
      if (nextPage === pageNo || !this.allowQueryChange()) return;
      this.tab.pageOffset = (nextPage - 1) * this.tab.pageLimit;
      void this.reload("page");
    };
    pageBox.addEventListener("change", goToPage);
    pagePosition.addEventListener("submit", (e) => {
      e.preventDefault();
      goToPage();
    });
    const pageCount = document.createElement("span");
    pageCount.className = "grid-page-count";
    pageCount.textContent = exact ? `/ ${pages} 页` : `页 · ${this.totalText()}`;
    pagePosition.append(pageBox, pageCount);
    pager.appendChild(pagePosition);

    mkPage(ICON_PAGE_NEXT, "下一页", !this.hasNextPage(page), () => {
      this.tab.pageOffset += this.tab.pageLimit;
      void this.reload("page");
    });
    mkPage(ICON_PAGE_LAST, exact ? "最后一页" : "最后一页（请先统计总数）", !exact || pageNo >= pages, () => {
      this.tab.pageOffset = (pages - 1) * this.tab.pageLimit;
      void this.reload("page");
    });

    // 筛选 / 排序生效指示（点击可再次打开对应对话框）
    const activeFilters = this.tab.filters.length;
    if (activeFilters || this.tab.orderBy) {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "grid-filter-summary";
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
      chip.title = chip.textContent;
      chip.setAttribute("aria-label", `修改筛选或排序：${chip.textContent}`);
      chip.addEventListener("click", () => void this.app.runCommand(activeFilters ? "filter" : "sort"));
      this.footer.appendChild(chip);
    }

    const spacer = document.createElement("div");
    spacer.className = "grid-footer-spacer";
    this.footer.appendChild(spacer);

    const totalLabel = document.createElement("span");
    totalLabel.className = "grid-total-count";
    totalLabel.textContent = this.totalText();
    totalLabel.setAttribute("role", "status");
    totalLabel.title = this.totalError || (this.totalSnapshot
      ? `统计快照：${new Date(this.totalSnapshot.at).toLocaleTimeString()}；外部写入请刷新，未提交草稿不计入总数`
      : "先显示当前页，再后台统计；未提交草稿不计入总数");
    this.footer.appendChild(totalLabel);
    const countButton = document.createElement("button");
    countButton.type = "button";
    countButton.className = "grid-footer-btn grid-count-total";
    countButton.textContent = this.countingTotal ? "统计中…" : this.totalError ? "重试统计" : "重新统计";
    countButton.title = "独立刷新总行数和总页数；大表或复杂筛选可能耗时，不影响已显示的数据";
    countButton.disabled = this.totalBlocked() || this.countingTotal;
    countButton.addEventListener("click", () => { if (!countButton.disabled) void this.countTotal(); });
    this.footer.appendChild(countButton);
    if (this.viewMode() === "form") {
      this.footer.appendChild(this.buildRecordNav(page));
    } else {
      this.footer.appendChild(pager);
    }
    const separator = document.createElement("span");
    separator.className = "grid-footer-divider";
    this.footer.appendChild(separator);
    this.footer.appendChild(this.buildViewSwitch());
  }

  /** 当前视图模式（缺省网格）。 */
  private viewMode(): "grid" | "form" {
    return this.tab.viewMode === "form" ? "form" : "grid";
  }

  /** 切换网格 / 表单视图：复用当前页，不重新拉取数据。 */
  private setViewMode(mode: "grid" | "form"): void {
    if (this.viewMode() === mode) return;
    if (mode === "grid") this.currentRecord = this.formIndex;
    this.tab.viewMode = mode;
    if (mode === "form") {
      const selected = [...(this.tab.selectedRows ?? [])];
      if (selected.length === 1) this.formIndex = Math.min(selected[0], Math.max(0, this.displayRows.length - 1));
    }
    this.applyData();
    this.app.renderStatus();
  }

  /** 视图切换：网格 / 表单，仅图标。 */
  private buildViewSwitch(): HTMLElement {
    const wrap = document.createElement("div");
    wrap.className = "view-switch";
    wrap.setAttribute("role", "tablist");
    wrap.setAttribute("aria-label", "视图切换");
    const mk = (mode: "grid" | "form", ico: IconDef, title: string): HTMLButtonElement => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "grid-footer-btn view-switch-btn" + (this.viewMode() === mode ? " active" : "");
      b.title = title;
      b.setAttribute("aria-label", title);
      b.setAttribute("aria-selected", String(this.viewMode() === mode));
      b.innerHTML = svg(ico);
      b.addEventListener("click", () => this.setViewMode(mode));
      return b;
    };
    wrap.append(mk("grid", ICON_GRID_VIEW, "网格视图"), mk("form", ICON_FORM_VIEW, "表单视图"));
    return wrap;
  }

  /** 表单视图页脚：分页图标旋转 90° = 首条 / 上一条 / 当前条 / 下一条 / 末条。 */
  private buildRecordNav(page: import("../core/types").PageResult): HTMLElement {
    const nav = document.createElement("nav");
    nav.className = "grid-record-nav";
    nav.setAttribute("aria-label", "逐条切换记录");
    const total = page.total;
    const exact = this.totalIsExact(page);
    const globalIndex = this.tab.pageOffset + this.formIndex;
    const clampIndex = Math.max(0, exact ? Math.min(globalIndex, Math.max(0, total - 1)) : globalIndex);
    const btn = (ico: IconDef, title: string, disabled: boolean, action: () => void): HTMLButtonElement => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "grid-footer-btn";
      b.title = title;
      b.setAttribute("aria-label", title);
      b.disabled = disabled || this.loading || this.committing || this.transactionOwnedByThisTab();
      b.innerHTML = '<span class="rot90">' + svg(ico) + '</span>';
      b.addEventListener("click", () => { if (!b.disabled) action(); });
      return b;
    };
    const atFirst = clampIndex <= 0;
    const atLast = exact ? total <= 0 || clampIndex >= total - 1 : page.rows.length === 0 || (this.formIndex >= page.rows.length - 1 && !this.hasNextPage(page));
    nav.append(
      btn(ICON_PAGE_FIRST, "第一条", atFirst, () => this.gotoRecord(0)),
      btn(ICON_PAGE_PREV, "上一条", atFirst, () => this.gotoRecord(clampIndex - 1)),
    );
    const input = document.createElement("input");
    input.type = "text";
    input.inputMode = "numeric";
    input.className = "grid-record-input";
    input.value = String(total > 0 ? clampIndex + 1 : 0);
    input.setAttribute("aria-label", "当前条");
    input.disabled = this.loading || this.transactionOwnedByThisTab();
    input.title = exact ? "输入条号跳转（共 " + total + " 条）" : "输入条号跳转（总数未统计，深页可能较慢）";
    const submit = () => {
      if (input.disabled) return;
      const n = Number(input.value);
      if (!Number.isSafeInteger(n) || n < 1 || (exact && n > total)) { input.value = String(total > 0 ? clampIndex + 1 : 0); return; }
      if (this.hasPending()) { this.app.toast("info", "存在未提交的修改，请先提交或回滚"); input.value = String(total > 0 ? clampIndex + 1 : 0); return; }
      this.gotoRecord(n - 1);
    };
    input.addEventListener("change", submit);
    input.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); submit(); } });
    nav.append(input);
    nav.append(
      btn(ICON_PAGE_NEXT, "下一条", atLast, () => this.gotoRecord(clampIndex + 1)),
      btn(ICON_PAGE_LAST, exact ? "最后一条" : "最后一条（请先统计总数）", !exact || atLast, () => this.gotoRecord(total - 1)),
    );
    return nav;
  }

  /** 跳到全表第 n 条（0 基），跨页时重新拉取该条所在页。 */
  private gotoRecord(n: number): void {
    const page = this.tab.page;
    if (!page) return;
    const total = page.total;
    if (!Number.isSafeInteger(n) || n < 0 || (this.totalIsExact(page) && n >= total) || this.loading) return;
    if (this.hasPending()) { this.app.toast("info", "存在未提交的修改，请先提交或回滚"); return; }
    if (!this.allowQueryChange()) return;
    if (this.transactionOwnedByThisTab()) { this.app.toast("info", "手动事务进行中：请先提交或回滚事务，再切换记录"); return; }
    const limit = this.tab.pageLimit;
    const offset = Math.floor(n / limit) * limit;
    const idx = n - offset;
    if (offset === this.tab.pageOffset) { this.formIndex = idx; this.applyData(); this.app.renderStatus(); return; }
    this.tab.pageOffset = offset;
    void this.reload("page").then(() => {
      if (this.disposed || this.loading || this.loadedPageRequest?.offset !== offset) return;
      this.formIndex = idx; this.applyData(); this.app.renderStatus();
    });
  }

  /** 表单视图：单列、每行一个可编辑框，复用网格草稿机制。 */
  private renderForm(): void {
    const page = this.tab.page;
    if (!page) { this.formHost.replaceChildren(); return; }
    const rows = this.displayRows;
    if (rows.length === 0) { this.formHost.replaceChildren(this.formEmpty()); return; }
    if (this.formIndex >= rows.length) this.formIndex = rows.length - 1;
    if (this.formIndex < 0) this.formIndex = 0;
    const row = rows[this.formIndex];
    const editable = !this.readOnlyReason() && !this.committing && !this.loading;
    const grid = document.createElement("div");
    grid.className = "data-form-grid";
    page.columns.forEach((col, ci) => {
      const field = document.createElement("div");
      field.className = "data-form-field";
      const label = document.createElement("label");
      label.textContent = col.name;
      label.title = col.name + " · " + col.type;
      const controlWrap = document.createElement("div");
      controlWrap.className = "data-form-control";
      const raw = row[ci] ?? null;
      const long = /text|json|blob|bytea/i.test(col.type || "");
      let control: HTMLInputElement | HTMLTextAreaElement;
      if (long) {
        const ta = document.createElement("textarea");
        ta.rows = 4;
        control = ta;
      } else {
        control = document.createElement("input");
        (control as HTMLInputElement).type = "text";
      }
      control.className = "data-form-input" + (long ? " data-form-textarea" : "") + (raw === null ? " is-null" : "");
      control.value = raw ?? "";
      if (raw === null) control.placeholder = "NULL";
      control.readOnly = !editable;
      control.addEventListener("change", () => {
        if (!editable) return;
        const v = control.value === "" ? null : control.value;
        this.applyEdit(this.formIndex, col.name, v);
      });
      controlWrap.appendChild(control);
      field.append(label, controlWrap);
      grid.appendChild(field);
    });
    const wrap = document.createElement("div");
    wrap.className = "data-form-view";
    wrap.appendChild(grid);
    this.formHost.replaceChildren(wrap);
  }

  private formEmpty(): HTMLElement {
    const p = document.createElement("p");
    p.className = "data-form-empty";
    p.textContent = "当前页没有可显示的记录";
    return p;
  }

  /** 应用单元格编辑（进入待提交状态，不直接写库）。 */
  private applyEdit(row: number, colName: string, value: string | null): void {
    if (this.committing || this.mutationEffectsUnknown) return;
    const page = this.tab.page;
    if (!page || !colName) return;
    if (this.pendingDeletes.has(row) || this.txAppliedDeletes.has(row)) {
      this.app.toast("info", "该行已标记删除，请先提交或回滚再编辑");
      return;
    }
    if (row >= this.originalRowCount()) {
      // 待插入行的编辑（前段是事务内已应用的插入行，不属于可编辑草稿区）
      const idx = row - this.originalRowCount() - this.txAppliedInserts.length;
      if (idx < 0) return;
      const edits = this.pendingInsertEdits.get(idx) ?? new Map<string, string | null>();
      edits.set(colName, value);
      this.pendingInsertEdits.set(idx, edits);
    } else {
      const ci = page.columns.findIndex((c) => c.name === colName);
      const old = page.rows[row]?.[ci] ?? null;
      const edits = this.pendingUpdates.get(row) ?? new Map<string, string | null>();
      if (old === value) {
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
    if (this.committing || this.mutationEffectsUnknown) return;
    const page = this.tab.page;
    if (!page) return;
    this.pendingInserts.push(page.columns.map(() => null));
    if (this.tab.viewMode === "form") this.formIndex = this.displayRows.length - 1;
    this.applyData();
    this.app.renderStatus();
  }

  /** 删除选中的行（进入待删除状态）。 */
  async deleteSelectedRows(): Promise<void> {
    if (this.committing || this.mutationEffectsUnknown) return;
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
      if (r < base && (this.pendingDeletes.has(r) || this.txAppliedDeletes.has(r))) {
        this.app.toast("info", "选中的行已处于待删除状态");
        return;
      }
    }
    for (const r of rows) {
      if (r >= base) {
        const offset = r - base;
        if (offset < this.txAppliedInserts.length) {
          // 事务内已应用的插入行：直接从显示列表移除（尚未落库，无需后端操作）。
          this.txAppliedInserts.splice(offset, 1);
        } else {
          // 待插入行：同时移除其编辑缓存，并把更大下标的编辑键整体前移（防错位）。
          const idx = offset - this.txAppliedInserts.length;
          this.pendingInserts.splice(idx, 1);
          this.pendingInsertEdits.delete(idx);
          const remapped = new Map<number, Map<string, string | null>>();
          for (const [k, v] of this.pendingInsertEdits) {
            remapped.set(k > idx ? k - 1 : k, v);
          }
          this.pendingInsertEdits = remapped;
        }
      } else {
        this.pendingDeletes.add(r);
      }
      this.tab.selectedRows.delete(r);
    }
    this.applyData();
    this.app.renderStatus();
  }

  /** 提交是否正在进行（防重入：双击/快捷键会重复提交同一批修改）。 */
  committing = false;
  /** 不确定的提交不得重新发送；仅显式清空草稿并成功重新载入后解锁。 */
  private mutationEffectsUnknown: string | null = null;

  /** 表数据页事务前置检查：事务将绑定本页面，编辑在事务连接上执行。 */
  prepareForTransaction(): void {
    if (this.committing || this.loading) throw new Error("表格正在载入或提交，请稍候再开始事务");
    if (this.mutationEffectsUnknown) throw new Error("上一笔写入结果未知，请先核对数据库并重新载入");
  }

  /** 事务是否属于本页面（工具栏与提交路径判断）。 */
  transactionOwnedByThisTab(): boolean {
    const status = this.app.manualTransactionFor?.(this.tab.connectionId);
    return !!status?.active && status.owner === this.tab.id && status.schema === this.tab.schema;
  }

  /** 提交全部待提交修改（删除 / 更新 / 插入）。 */
  async commitEdits(): Promise<void> {
    if (this.mutationEffectsUnknown) {
      this.app.toast("error", "上一笔写入结果未知，禁止重试；请先核对数据库状态并重新载入");
      return;
    }
    const page = this.tab.page;
    if (!page) return;
    if (this.committing) return; // 防重入
    if (!this.hasPending()) {
      this.app.toast("info", "没有需要提交的修改");
      return;
    }
    // 手动事务属于本表数据页：待提交修改在事务连接上执行（提交事务前不落库）。
    if (this.transactionOwnedByThisTab()) {
      await this.commitEditsInTransaction(page);
      return;
    }
    const pk = page.primary_keys;
    if (!pk.length && !page.columns.length) {
      this.app.toast("error", "没有可用的列信息，无法保存修改");
      return;
    }
    this.committing = true;
    this.renderFooter(page);
    let done = 0;
    let failMsg = "";
    /** 无主键表的整行旧值定位条件。 */
    const oldRow = (r: number): [string, string | null][] =>
      page.columns.map((c, ci) => [c.name, page.rows[r]?.[ci] ?? null] as [string, string | null]);
    try {
      // 1) 删除（WHERE 用数据库里的旧值；DELETE 未命中才是真错误）
      const delList = [...this.pendingDeletes].sort((a, b) => a - b);
      for (const r of delList) {
        const keyPairs = (pk.length ? this.pkValues(page, r, new Map()) : oldRow(r)).map(
          ([c, v]) => [c, toDbValue(v, this.columnType(page, c))] as [string, DbValue],
        );
        await this.runMutation({
          connection_id: this.tab.connectionId,
          schema: this.tab.schema,
          table: this.tab.object,
          kind: "delete",
          keys: keyPairs,
          original_values: oldRow(r).map(([c, v]) => [c, toDbValue(v, this.columnType(page, c))] as [string, DbValue]),
        });
        this.pendingDeletes.delete(r);
        done++;
      }
      // 2) 更新（WHERE 用旧主键值；UPDATE 命中 0 行 = 值无变化，不算错误）
      for (const [r, edits] of [...this.pendingUpdates.entries()]) {
        const keyPairs = pk.length
          ? this.pkValues(page, r, new Map()).map(
              ([c, v]) => [c, toDbValue(v, this.columnType(page, c))] as [string, DbValue],
            )
          : [];
        const changes = [...edits.entries()].map(
          ([c, v]) => [c, toDbValue(v, this.columnType(page, c))] as [string, DbValue],
        );
        // 整行原值 CAS：包括未修改列，不能悄悄覆盖外部已经更新的行。
        const originalPairs = oldRow(r).map(([c, v]) => [c, toDbValue(v, this.columnType(page, c))] as [string, DbValue]);
        await this.runMutation({
          connection_id: this.tab.connectionId,
          schema: this.tab.schema,
          table: this.tab.object,
          kind: "update",
          changes,
          keys: keyPairs,
          original_values: originalPairs,
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
          // 未编辑的空格表示省略该列，保留服务端默认值；显式 NULL 则独立传递。
          if (!edits?.has(c.name) && this.pendingInserts[i][ci] == null) return;
          const raw = edits?.has(c.name)
            ? edits.get(c.name) ?? null
            : this.pendingInserts[i][ci] ?? null;
          if (raw === null && c.auto_increment) return;
          cols.push(c.name);
          values.push(raw);
        });
        // 空 changes 仍提交 DEFAULT VALUES；后端仅在能读回唯一身份时允许。
        const insertChanges: [string, DbValue][] = cols.map(
          (c, ci) => [c, toDbValue(values[ci], this.columnType(page, c))] as [string, DbValue],
        );
        await this.runMutation({
          connection_id: this.tab.connectionId,
          schema: this.tab.schema,
          table: this.tab.object,
          kind: "insert",
          changes: insertChanges,
        });
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
      failMsg = String(e);
      // 已确认提交的项已出队；未知项保留证据但必须阻止任何重放。
      // 注意不能在此 reload：行基数据变化会让剩余 pending 的行号定位失效。
      const remaining =
        this.pendingDeletes.size +
        this.pendingUpdates.size +
        this.pendingInserts.length;
      if (remaining > 0) {
        const recovery = this.mutationEffectsUnknown
          ? "结果未知，禁止重试；本机草稿保留，建议先另存证据，核对后清除本机草稿并重新载入"
          : "草稿保留；冲突请核对原值，明确回滚的失败可重新预览";
        this.app.toast("error", `${failMsg}（已确认 ${done} 项，待核对 ${remaining} 项；${recovery}）`);
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
      this.scheduleTotalCount();
      this.renderFooter(this.tab.page ?? page);
      this.app.renderStatus();
    }
  }

  /** 取某列声明的类型名；用于把网格字符串转换成类型化绑定值。 */
  private columnType(page: import("../core/types").PageResult, name: string): string {
    return page.columns.find((c) => c.name === name)?.type ?? "";
  }

  /**
   * 事务内提交：把全部待提交修改打包为结构化变更，在事务连接上执行。
   * 与普通提交不同：不落库、不重读（读池只见已提交数据），更改保留显示
   * 直到工具栏「提交事务 / 回滚事务」结束事务。
   */
  private async commitEditsInTransaction(page: import("../core/types").PageResult): Promise<void> {
    const ops: import("./types").ManualMutationOp[] = [];
    const oldRow = (r: number): [string, string | null][] =>
      page.columns.map((c, ci) => [c.name, page.rows[r]?.[ci] ?? null] as [string, string | null]);
    // 1) 删除
    for (const r of [...this.pendingDeletes].sort((a, b) => a - b)) {
      ops.push({
        kind: "delete",
        table: this.tab.object,
        keys: this.pkValues(page, r, new Map()),
        old_values: oldRow(r),
      });
    }
    // 2) 更新
    for (const [r, edits] of [...this.pendingUpdates.entries()]) {
      ops.push({
        kind: "update",
        table: this.tab.object,
        changes: [...edits.entries()],
        keys: this.pkValues(page, r, new Map()),
        old_values: oldRow(r),
      });
    }
    // 3) 插入（自增主键未填值时省略，与普通提交一致）
    for (const [i, row] of this.pendingInserts.entries()) {
      const edits = this.pendingInsertEdits.get(i);
      const cols: string[] = [];
      const values: (string | null)[] = [];
      page.columns.forEach((c, ci) => {
        if (!edits?.has(c.name) && row[ci] == null) return;
        const raw = edits?.has(c.name) ? edits.get(c.name) ?? null : row[ci] ?? null;
        if (raw === null && c.auto_increment) return;
        cols.push(c.name);
        values.push(raw);
      });
      ops.push({ kind: "insert", table: this.tab.object, columns: cols, values });
    }
    if (!ops.length) {
      this.app.toast("info", "没有需要提交的修改");
      return;
    }
    this.committing = true;
    this.invalidateTotal();
    this.renderFooter(page);
    try {
      const result = await api.manualTransactionMutate(this.tab.connectionId, this.tab.id, this.tab.schema, ops);
      const affected = result.sets.reduce((a, s) => a + s.affected, 0);
      // 成功：转入「事务内已应用」状态，显示新值但保持未提交标记语义。
      for (const r of [...this.pendingDeletes].sort((a, b) => a - b)) {
        this.txAppliedDeletes.add(r);
        this.pendingDeletes.delete(r);
      }
      for (const [r, edits] of [...this.pendingUpdates.entries()]) {
        const merged = new Map<string, string | null>(this.txAppliedUpdates.get(r) ?? []);
        for (const [c, v] of edits) merged.set(c, v);
        this.txAppliedUpdates.set(r, merged);
        this.pendingUpdates.delete(r);
      }
      for (const row of this.pendingInserts) this.txAppliedInserts.push(row);
      this.pendingInserts = [];
      this.pendingInsertEdits.clear();
      this.app.toast("success", `已在事务中应用 ${ops.length} 项更改（${affected} 行受影响）；点击「提交事务」落库或「回滚事务」撤销`);
    } catch (e) {
      // 事务语句失败：后端将事务标记为 failed，只能回滚；本地草稿保留供核对。
      this.app.toast("error", `${String(e)}（事务中执行失败；如事务已标记失败，请回滚后重试）`);
    } finally {
      this.committing = false;
      this.renderFooter(this.tab.page ?? page);
      this.applyData();
      this.app.renderStatus();
      this.app.notifyToolbar();
    }
  }

  /** 清除事务可视化状态（提交 / 回滚事务后由 App 调用，随后 reload）。 */
  clearTransactionState(): void {
    this.invalidateTotal();
    this.txAppliedUpdates.clear();
    this.txAppliedDeletes.clear();
    this.txAppliedInserts = [];
    this.pendingUpdates.clear();
    this.pendingDeletes.clear();
    this.pendingInserts = [];
    this.pendingInsertEdits.clear();
    this.tab.dirty = false;
    this.applyData();
  }

  /** 预览并执行一次图形化写入；冲突、失败或读回未通过统一抛错。 */
  private async runMutation(draft: MutationDraft): Promise<MutationReceipt> {
    if (this.mutationEffectsUnknown) throw new Error("上一笔写入结果未知，禁止重新预览或执行；请核对数据库后重新载入");
    const owner = this.tab.id;
    const context = [this.tab.connectionId, this.tab.schema, this.tab.object].join("\0");
    const plan = await api.previewRowMutation({ ...draft, owner_id: owner });
    if (!plan || typeof plan.plan_id !== "string" || !plan.plan_id || plan.plan_id.length > 256) {
      throw new Error("预览未返回有效的服务端计划，未发送写入");
    }
    if (this.tab.id !== owner || [this.tab.connectionId, this.tab.schema, this.tab.object].join("\0") !== context || this.tab.datasourceStale) {
      throw new Error("预览期间页面目标已变化，写入计划未执行；草稿保留");
    }
    // 在真正发送写入前废弃计数，包含部分成功、明确失败和未知回执。
    this.invalidateTotal();
    let receipt: MutationReceipt;
    try { receipt = await api.executeRowMutation(plan.plan_id, owner); }
    catch (error) {
      const code = typeof error === "object" && error !== null && "code" in error ? String(error.code ?? "") : "";
      if (!["PLAN_EXPIRED", "PLAN_OWNER_MISMATCH", "READ_ONLY_VIOLATION", "MUTATION_CONTEXT_CHANGED", "CONNECTION_NOT_OPEN", "UNSUPPORTED_MUTATION"].includes(code)) {
        this.mutationEffectsUnknown = String(error);
        throw new Error(`写入回执未确认，结果未知；禁止自动或手动重放，须重新载入核对：${String(error)}`);
      }
      throw error;
    }
    if (!receipt || receipt.plan_id !== plan.plan_id || !["applied", "unchanged", "conflict", "failed"].includes(receipt.status)
      || !["committed", "rolled_back", "none", "unknown"].includes(receipt.transaction) || typeof receipt.verified !== "boolean"
      || !Number.isSafeInteger(receipt.affected) || receipt.affected < 0 || !Array.isArray(receipt.mismatches)
      || receipt.mismatches.some(message => typeof message !== "string")
      || (receipt.status === "applied" && (receipt.transaction !== "committed" || !receipt.verified || receipt.affected !== 1))) {
      this.mutationEffectsUnknown = "写入回执缺失、无效或与计划不一致";
      throw new Error("写入回执未通过协议校验，结果未知，禁止重放；请重新载入核对");
    }
    if (receipt.transaction === "unknown") {
      this.mutationEffectsUnknown = "提交回执未确认";
      throw new Error("写入结果未知，禁止重放；建议另存本机草稿证据，并重新载入核对");
    }
    if (receipt.status === "conflict") {
      const detail = receipt.mismatches.length ? "：" + receipt.mismatches.join("；") : "";
      throw new Error(`表结构或数据已变化，保存被拒绝${detail}`);
    }
    if (receipt.status === "failed") {
      const detail = receipt.mismatches.length ? "：" + receipt.mismatches.join("；") : "";
      throw new Error(`写入失败${detail}`);
    }
    if (!receipt.verified) {
      const detail = receipt.mismatches.length ? "：" + receipt.mismatches.join("；") : "";
      throw new Error(`写入结果未通过读回校验${detail}`);
    }
    return receipt;
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
    this.app.toast("info", this.mutationEffectsUnknown
      ? "本机草稿已清除；这不代表数据库已回滚。请重新载入核对未知写入后再编辑"
      : "本机未提交草稿已清除，没有向数据库发送回滚或写入");
  }

  destroy(): void {
    this.disposed = true;
    ++this.requestVersion;
    this.invalidateTotal();
    this.clearQueryTimer();
    this.queryStartedAt = null;
    document.removeEventListener("selectionchange", this.selectionListener);
    this.grid.dispose();
  }
}

/** 查询视图（运行条 + 多结果页签 + 消息）。 */
class QueryTabView extends TabView {
  editor: SqlEditor;
  private sqlBaseline = "";
  private connSelect: HTMLSelectElement;
  private dbSelect: HTMLSelectElement;
  private runBtn: HTMLButtonElement;
  private stopBtn: HTMLButtonElement;
  private explainBtn: HTMLButtonElement;
  /** 「运行 / 解释」按钮文字节点：有选区时改成「运行已选择的 / 解释已选择的」。 */
  private runBtnLabel!: HTMLElement;
  private explainBtnLabel!: HTMLElement;
  private runbar: HTMLElement;
  private contentWrap: HTMLElement;
  private resultSplitter: HTMLElement;
  private resultTabsEl: HTMLElement;
  private resultBody: HTMLElement;
  /** 结果区高度占内容区比例（拖动范围 1/3 ~ 2/3）。 */
  private resultRatio = 1 / 3;
  grid: DataGrid;
  private gridPreviewNoteEl: HTMLElement;
  private resultLoadingEl: HTMLElement | null = null;
  private resultLoadingLabelEl: HTMLElement | null = null;
  private resultLoadingTimerEl: HTMLElement | null = null;
  private resultPager: HTMLElement;
  private pageNumberInput: HTMLInputElement;
  private pageControls: HTMLElement;
  private pageCountEl: HTMLElement;
  private pageSizeSelect: HTMLSelectElement;
  private footerStatusEl: HTMLElement;
  private pageErrorEl: HTMLElement;
  private pageButtons: HTMLButtonElement[] = [];
  private readonly resultPages = new QueryResultPages();
  private controlledResult: QueryResultPanel | null = null;
  private controlledResultContext: QueryResultIpcContext | null = null;
  private resultPanelHost: HTMLElement;
  private resultRecoveryTools: HTMLElement;
  private resultEvidenceLoading = false;
  private resultEvidenceRequest = 0;
  private resultEvidenceKey = "";
  private resultEvidenceReason = "";
  private resultExecution: { context: Omit<QueryResultIpcContext, "result_index">; result: MultiResult } | null = null;
  private exporting = false;
  private executionRevision = 0;
  private messagesEl: HTMLElement;
  private cellPreviewEl: HTMLElement;
  /** -1 = 消息页；>=0 = 结果 N。显示、导出和状态栏共用标签页上的唯一索引。 */
  private get activeResultTab(): number { return this.tab.activeResult; }
  private set activeResultTab(index: number) { this.tab.activeResult = index; }
  private running = false;
  private execution: ExecutionController;
  private executionCapabilities: Capabilities | null = null;
  private executionState: ExecutionState = "succeeded";
  private executionNeedsReview = false;
  private executionStatusEl: HTMLElement;
  private executionLabelEl: HTMLElement;
  private executionTimerEl: HTMLElement;
  private executionMessageLabelEl: HTMLElement | null = null;
  private executionMessageTimerEl: HTMLElement | null = null;
  /** 本次执行的墙钟起点；null 表示没有正在进行的计时。 */
  private executionClockStartedAt: number | null = null;
  private executionClockTimer: ReturnType<typeof setInterval> | null = null;
  private lastMessages: { text: string; error: boolean }[] = [];
  private completionLoaded = false;
  private completionRequest = 0;
  private disposed = false;

  isExecuting(): boolean { return this.running; }
  resultBusy(): boolean { return this.running || this.exporting || this.resultPages.busy || this.resultEvidenceLoading || this.controlledResult?.busy === true; }
  draftBusy(): boolean { return this.resultBusy(); }
  draftDirty(): boolean { return this.editor.getSql() !== this.sqlBaseline || this.controlledResult?.dirty === true; }
  /** 仅 SQL 文本相对已保存内容有改动；结果行草稿不算。 */
  hasUnsavedSql(): boolean { return this.editor.getSql() !== this.sqlBaseline; }
  captureDraft(): Pick<DraftSnapshot, "kind" | "baseline" | "payload"> {
    return { kind: "query", baseline: this.sqlBaseline, payload: { sql: this.editor.getSql(),
      ...(this.controlledResult ? { result_editing: { ipc_context: this.controlledResultContext, draft: this.controlledResult.captureDraft(), replay_allowed: false } } : {}) } };
  }
  markClean(sql = this.editor.getSql()): void { this.sqlBaseline = sql; this.tab.dirty = this.draftDirty(); }
  detachSavedQuery(): void {
    delete this.tab.savedQueryId;
    this.sqlBaseline = ""; this.tab.dirty = this.draftDirty();
  }
  discardDraft(): void {
    if (this.controlledResult?.busy) throw new Error("结果写入、预览或未知回执仍需保留，不能丢弃");
    this.disposeControlledResult();
    this.editor.setSql(this.sqlBaseline); this.tab.dirty = false;
  }
  async prepareForTransaction(): Promise<void> {
    if (this.executionNeedsReview) throw new Error("上一执行结果未知，请先验证数据库状态并重新选择连接");
    if (this.running || this.disposed) throw new Error("当前查询未就绪");
    await this.refreshCompletionData();
    if (!this.completionLoaded || this.disposed) throw new Error("真实数据库/模式元数据未加载完成，不能开始事务");
  }
  syncManualTransactionControls(): void {
    const status = this.app.manualTransactionFor?.(this.tab.connectionId);
    const pending = this.app.manualTransactionIsBusy?.(this.tab.connectionId) ?? false;
    // 查询页可以承载手动事务：本页持有时允许执行 SQL（在该事务连接上生效），提交后才落库。
    const own = status?.owner === this.tab.id && status?.schema === this.tab.schema;
    const base = this.tab.datasourceStale || (this.executionNeedsReview ? "上一执行结果未知，请重新验证连接后再执行" : pending ? "事务操作进行中，请等待完成" : "");
    const otherOwner = status && !own ? "此连接的事务由其它页面持有，请在该页面提交或回滚" : "";
    const runReason = base || otherOwner || (status?.failed && own ? "事务执行失败，必须回滚后才能继续执行" : "");
    // EXPLAIN 不属于手动事务允许的查询/DML 语句，事务期间禁用解释。
    const explainReason = base || otherOwner || (status ? "手动事务期间不能解释 SQL，请先提交或回滚事务" : "");
    const busy = this.resultBusy();
    this.runBtn.disabled = busy || !!runReason;
    this.explainBtn.disabled = busy || !!explainReason;
    this.runBtn.title = runReason || "运行全文（Ctrl+Enter）；运行选区用 Ctrl+Shift+Enter";
    if (this.controlledResult?.dirty) {
      this.runBtn.title = "结果草稿尚未处理；请先撤销或释放编辑结果";
    }
    this.runBtn.setAttribute("aria-description", this.runBtn.title);
    this.explainBtn.title = explainReason || "解释一条 SQL；多语句请先选中其中一条";
    this.explainBtn.setAttribute("aria-description", this.explainBtn.title);
    this.connSelect.disabled = busy || pending || !!status;
    this.dbSelect.disabled = busy || pending || !!status || !!this.tab.datasourceStale;
    this.syncNamespaceLabels();
  }

  private syncNamespaceLabels(): void {
    const status = this.app.manualTransactionFor?.(this.tab.connectionId);
    const pending = this.app.manualTransactionIsBusy?.(this.tab.connectionId) ?? false;
    const namespace = namespaceName(this.app.connectionsValue().find(connection => connection.id === this.tab.connectionId));
    const scopeReason = status ? `事务绑定当前连接和${namespace}，请先提交或回滚再切换` : pending ? "事务操作进行中" : "";
    this.connSelect.title = scopeReason || "查询连接";
    this.dbSelect.title = scopeReason || `查询${namespace}`;
    this.dbSelect.setAttribute("aria-label", `查询${namespace}`);
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
    this.dbSelect.setAttribute("aria-label", "查询命名空间");
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
    // 运行 / 停止 / 解释并排一组；执行状态和实时计时显示在中央结果遮罩。
    this.runBtn = mkRunBtn("run_sel", "运行");
    this.runBtn.classList.add("primary");
    this.stopBtn = mkRunBtn("stop_sel", "停止");
    this.stopBtn.classList.add("disabled");
    this.stopBtn.disabled = true;
    this.stopBtn.title = "没有正在执行的查询；取消能力将在连接就绪后从后端读取";
    this.stopBtn.setAttribute("aria-label", "停止当前查询");
    this.stopBtn.addEventListener("click", () => {
      void this.execution.cancel().catch(error => this.app.toast("error", `停止请求失败，查询仍等待确认：${String(error)}`));
    });
    this.explainBtn = mkRunBtn("explain_sel", "解释");
    // 有选区时按钮显示「解释已选择的」并只解释选区；无选区时解释全文。
    this.explainBtn.addEventListener("click", () => void this.explainSelection(this.editor.hasSelection()));
    this.runBtnLabel = this.runBtn.lastElementChild as HTMLElement;
    this.explainBtnLabel = this.explainBtn.lastElementChild as HTMLElement;
    this.runBtn.addEventListener("click", () => void this.executeSelection(this.editor.hasSelection()));
    this.executionStatusEl = document.createElement("span");
    this.executionStatusEl.className = "query-transaction-status";
    this.executionStatusEl.style.marginLeft = "auto";
    this.executionStatusEl.style.paddingRight = "10px";
    this.executionStatusEl.setAttribute("role", "status");
    this.executionStatusEl.setAttribute("aria-live", "polite");
    this.executionStatusEl.setAttribute("aria-label", "查询执行状态");
    this.executionStatusEl.setAttribute("data-state", "idle");
    // 状态点与毫秒计时都是视觉辅助：状态文字本身已经完整表达含义。
    const statusDot = document.createElement("span");
    statusDot.className = "query-status-dot";
    statusDot.setAttribute("aria-hidden", "true");
    this.executionLabelEl = document.createElement("span");
    this.executionLabelEl.className = "query-status-label";
    this.executionTimerEl = document.createElement("span");
    this.executionTimerEl.className = "query-status-timer";
    this.executionTimerEl.setAttribute("aria-hidden", "true");
    // 执行状态保留用于消息和错误文案；可见计时只显示在中央查询遮罩。
    this.executionStatusEl.append(statusDot, this.executionLabelEl, this.executionTimerEl);
    this.execution = new ExecutionController({ ...api,
      cancelExecution: context => api.cancelExecution(context),
      executionStatus: context => api.executionStatus(context),
      executeSqlV2: async request => {
      const revision = this.executionRevision;
      const outcome = await api.executeSqlV2(request);
      // 控制器销毁后会吞掉迟到响应，在传输边界先回收其快照。
      if (this.disposed || revision !== this.executionRevision) {
        this.releaseSpool(outcome.result?.spool_id
          ? { spoolId: outcome.result.spool_id, connectionId: request.connection_id, ownerId: request.owner_id, namespace: request.namespace } : null);
        this.releaseQueryEvidence({ execution_id: outcome.execution_id, connection_id: request.connection_id, owner_id: request.owner_id, namespace: request.namespace, result_index: 0 });
      }
      return outcome;
    } }, state => {
      this.executionState = state;
      // 前端提交即执行，后端没有可观察的排队阶段：queued 与 running 对用户是同一件事。
      const labels: Record<ExecutionState, string> = { queued: "查询中", running: "查询中", cancel_requested: "停止中…", succeeded: "执行完成", failed: "执行失败", cancelled: "已停止（不保证回滚）", timed_out: "已超时（不保证回滚）", unknown: "结果未知，请验证写入状态" };
      this.executionLabelEl.textContent = labels[state];
      this.executionStatusEl.setAttribute("data-state", state);
      if (state === "queued" || state === "running" || state === "cancel_requested") {
        this.startExecutionClock();
        this.tickExecutionClock();
      } else this.freezeExecutionClock();
      const supported = this.executionCapabilities?.cancel.level !== "unsupported" && !!this.executionCapabilities;
      this.stopBtn.disabled = !this.running || !supported || state === "cancel_requested" || !["queued", "running"].includes(state);
      this.stopBtn.title = !supported ? this.executionCapabilities?.cancel.reason ?? "取消能力尚未读取" : state === "cancel_requested" ? "正在等待数据库停止确认，请勿重复提交" : "请求停止本次执行；不会自动提交或保证回滚";
      this.stopBtn.setAttribute("aria-description", this.stopBtn.title);
    });
    this.connSelect.addEventListener("change", () => {
      void this.switchConnection(this.connSelect.value);
    });
    this.dbSelect.addEventListener("change", () => {
      const namespace = namespaceName(this.app.connectionsValue().find(connection => connection.id === this.tab.connectionId));
      if (this.resultBusy() || this.app.manualTransactionFor?.(this.tab.connectionId) || this.app.manualTransactionIsBusy?.(this.tab.connectionId)) {
        this.dbSelect.value = this.tab.schema;
        this.app.toast("info", `事务或查询进行中，请先完成再切换${namespace}`);
        return;
      }
      if (!this.allowResultReplacement(`切换${namespace}`)) { this.dbSelect.value = this.tab.schema; return; }
      const desired = this.dbSelect.value, previous = this.tab.schema;
      const draftIds = previous || this.tab.savedQueryId ? [this.tab.id] : [];
      void this.app.guardLeave(draftIds, `切换查询${namespace}`, async () => {
        if (desired !== previous && this.tab.savedQueryId) this.detachSavedQuery();
        if (desired !== previous) this.clearResultSnapshot();
        this.tab.schema = desired; this.completionLoaded = false; await this.loadCompletionData();
      }).then(accepted => { if (!accepted && !this.disposed) this.dbSelect.value = previous; });
    });
    // 查询页承载手动事务入口；工具栏按事务状态显示开始 / 提交 / 回滚。
    this.syncManualTransactionControls();
    this.el.appendChild(this.runbar);

    // ---- 编辑器
    this.sqlBaseline = tab.sql;
    this.editor = new SqlEditor({
      value: tab.sql,
      autoComplete: app.settingsValue.auto_complete !== false,
      completionMode: "query",
      fontFamily: app.settingsValue.sql_font_family,
      fontSize: app.settingsValue.sql_font_size,
      onChange: (v) => {
        this.tab.sql = v;
        this.tab.dirty = this.draftDirty();
      },
      onExecute: (_sql, selectionOnly) => {
        void this.executeSelection(selectionOnly);
      },
      onExplain: (selectionOnly) => { void this.explainSelection(selectionOnly); },
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
    this.resultTabsEl.setAttribute("aria-label", "查询输出页");
    this.resultTabsEl.style.display = "none";
    this.resultBody = document.createElement("div");
    this.resultBody.className = "query-result-body";
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
    this.gridPreviewNoteEl = document.createElement("div");
    this.gridPreviewNoteEl.className = "result-preview-note";
    this.gridPreviewNoteEl.setAttribute("role", "status");
    this.gridPreviewNoteEl.style.display = "none";
    this.resultPager = document.createElement("nav");
    this.resultPager.className = "query-result-pager";
    this.resultPager.setAttribute("aria-label", "当前结果缓存分页");
    this.pageNumberInput = document.createElement("input");
    this.pageNumberInput.type = "text"; this.pageNumberInput.inputMode = "numeric";
    this.pageNumberInput.className = "query-page-number";
    this.pageNumberInput.setAttribute("aria-label", "当前页码");
    this.pageNumberInput.title = "输入页码，按 Enter 或移开焦点跳转（不重新执行 SQL）";
    this.footerStatusEl = document.createElement("span");
    this.footerStatusEl.className = "query-footer-status";
    this.footerStatusEl.setAttribute("role", "status");
    this.footerStatusEl.setAttribute("aria-live", "polite");
    this.resultPager.appendChild(this.footerStatusEl);
    const pageSizeLabel = document.createElement("label");
    pageSizeLabel.className = "query-page-size-label"; pageSizeLabel.textContent = "每页";
    this.pageSizeSelect = document.createElement("select");
    this.pageSizeSelect.className = "query-page-size";
    this.pageSizeSelect.setAttribute("aria-label", "每页条数");
    this.pageSizeSelect.title = "切换每页条数并回到首页（读取本次查询缓存，不重新执行 SQL）";
    for (const size of [200, 500, 1000]) {
      const option = document.createElement("option"); option.value = String(size); option.textContent = String(size);
      this.pageSizeSelect.appendChild(option);
    }
    this.pageSizeSelect.addEventListener("change", () => {
      const size = Number(this.pageSizeSelect.value);
      if (![200, 500, 1000].includes(size) || this.pageSizeSelect.disabled || this.resultBusy() || this.disposed) { this.renderResultPager(); return; }
      if (size !== this.resultPages.pageLimit(this.activeResultTab)) void this.loadResultPage("first", size);
    });
    pageSizeLabel.appendChild(this.pageSizeSelect);
    this.resultPager.appendChild(pageSizeLabel);
    this.pageErrorEl = document.createElement("span");
    this.pageErrorEl.className = "query-page-error"; this.pageErrorEl.hidden = true;
    this.pageErrorEl.id = `query-page-error-${tab.id}`;
    this.pageErrorEl.setAttribute("role", "alert");
    this.pageNumberInput.setAttribute("aria-describedby", this.pageErrorEl.id);
    this.pageNumberInput.addEventListener("keydown", event => {
      if (event.key === "Enter") { event.preventDefault(); void this.jumpResultPage(); }
      else if (event.key === "Escape") { event.preventDefault(); this.renderResultPager(); }
    });
    this.pageNumberInput.addEventListener("blur", () => { void this.jumpResultPage(); });
    this.pageNumberInput.addEventListener("input", () => {
      this.pageNumberInput.removeAttribute("aria-invalid"); this.pageErrorEl.hidden = true;
    });
    // 页码与总页数使用表数据页相同的布局和控件尺寸。
    this.pageControls = document.createElement("div");
    this.pageControls.className = "grid-pager";
    this.pageCountEl = document.createElement("span");
    this.pageCountEl.className = "grid-page-count";
    const pagePosition = document.createElement("div");
    pagePosition.className = "grid-pager-position";
    pagePosition.append(this.pageNumberInput, this.pageCountEl);
    const pageIcons = { first: ICON_PAGE_FIRST, previous: ICON_PAGE_PREV, next: ICON_PAGE_NEXT, last: ICON_PAGE_LAST } as const;
    for (const [action, label] of [["first", "首页"], ["previous", "上一页"], ["next", "下一页"], ["last", "末页"]] as const) {
      const button = document.createElement("button");
      button.type = "button"; button.className = "grid-footer-btn";
      button.innerHTML = svg(pageIcons[action]);
      button.title = `${label}（读取本次执行缓存，不重新执行 SQL）`;
      button.setAttribute("aria-label", button.title);
      button.addEventListener("click", () => void this.loadResultPage(action));
      this.pageButtons.push(button); this.pageControls.appendChild(button);
      if (action === "previous") this.pageControls.appendChild(pagePosition);
    }
    this.resultPager.append(this.pageControls, this.pageErrorEl);
    this.messagesEl = document.createElement("div");
    this.messagesEl.setAttribute("role", "log");
    this.messagesEl.setAttribute("aria-live", "polite");
    this.messagesEl.className = "result-messages";
    this.cellPreviewEl = document.createElement("div");
    this.cellPreviewEl.className = "cell-preview";
    this.resultPanelHost = document.createElement("div");
    this.resultPanelHost.className = "query-result-panel-host"; this.resultPanelHost.style.display = "none";
    this.resultRecoveryTools = document.createElement("nav");
    this.resultRecoveryTools.className = "query-result-pager"; this.resultRecoveryTools.style.display = "none";
    this.resultRecoveryTools.setAttribute("aria-label", "查询结果恢复证据");
    for (const [label, action] of [["查看 / 复制行草稿证据", () => this.showResultRecovery()],
      ["释放编辑结果", () => { void this.releaseResultAfterConfirm(); }]] as const) {
      const button = document.createElement("button"); button.type = "button"; button.className = "btn"; button.textContent = label;
      button.addEventListener("click", action); this.resultRecoveryTools.appendChild(button);
    }
    this.resultBody.appendChild(this.gridPreviewNoteEl);
    this.resultBody.appendChild(this.resultRecoveryTools);
    this.resultBody.appendChild(this.resultPanelHost);
    this.resultBody.appendChild(this.grid.el);
    this.resultBody.appendChild(this.messagesEl);
    this.resultBody.appendChild(this.resultPager);
    this.resultBody.appendChild(this.cellPreviewEl);
    this.cellPreviewEl.style.display = "none";
    // 「文本」预览只在选中文本时出现（结果网格只读，用拖选查看长文本）。
    document.addEventListener("selectionchange", this.selectionListener);
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

  /** selectionchange → 预览面板只在有选中内容时出现（结果网格只读，拖选单元格文本即可查看）。 */
  private selectionListener = (): void => {
    this.updateCellPreview();
    this.syncSelectionControls();
  };

  /** 编辑器有选区时，运行 / 解释按钮显示「运行已选择的 / 解释已选择的」。 */
  private syncSelectionControls(): void {
    if (this.disposed || !this.editor) return;
    const selected = this.editor.hasSelection();
    const run = selected ? "运行已选择的" : "运行";
    const explain = selected ? "解释已选择的" : "解释";
    if (this.runBtnLabel && this.runBtnLabel.textContent !== run) this.runBtnLabel.textContent = run;
    if (this.explainBtnLabel && this.explainBtnLabel.textContent !== explain) this.explainBtnLabel.textContent = explain;
    if (this.runBtnLabel) this.runBtn.title = selected ? "运行选中的 SQL（Ctrl+Shift+Enter）" : "运行全文（Ctrl+Enter）；运行选区用 Ctrl+Shift+Enter";
    if (this.explainBtnLabel) this.explainBtn.title = selected ? "解释选中的 SQL" : "解释一条 SQL；多语句请先选中其中一条";
  }

  /** 网格内当前拖选的文本（无选中或选区不在本网格时为空串）。 */
  private gridSelectionText(): string {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || !sel.anchorNode) return "";
    if (!this.grid.el.contains(sel.anchorNode)) return "";
    return sel.toString();
  }

  /**
   * 单元格内容面板：由选中决定 —— 结果网格焦点单元格、受控结果面板焦点单元格、编辑中或拖选文本时出现；
   * 没有选中就不出现、不占用高度。切到「消息」页时结果网格隐藏，预览一并收起。
   * 「文本▼」的格式（文本 / 十六进制 / 图像 / 网页）只决定渲染方式，不再凭格式常驻显示或回退首格。
   * 在面板内选择文本时不重渲染，避免打断选区。
   */
  updateCellPreview(): void {
    if (this.app.activeTab?.id !== this.tab.id || !this.app.textPreviewActive(this.tab.id)) {
      this.cellPreviewEl.style.display = "none";
      this.cellPreviewEl.innerHTML = "";
      return;
    }
    const activeSel = window.getSelection();
    if (activeSel && activeSel.anchorNode && this.cellPreviewEl.contains(activeSel.anchorNode)) return;
    const panel = this.controlledResult;
    const gridVisible = this.grid.el.style.display !== "none";
    const panelVisible = !!panel && this.resultPanelHost.style.display !== "none";
    if (!gridVisible && !panelVisible) {
      this.cellPreviewEl.style.display = "none";
      this.cellPreviewEl.innerHTML = "";
      return;
    }
    const selected = this.gridSelectionText();
    const cell = gridVisible ? this.grid.focusedCell : null;
    const fromGrid = cell ? (this.grid.rows[cell.row]?.[cell.col] ?? null) : null;
    const panelFocused = panelVisible && panel!.hasFocusedCell();
    const fromPanel = panelFocused ? panel!.focusedCellValue() : null;
    if (!this.grid.editingActive && !selected && !cell && !panelFocused) {
      this.cellPreviewEl.style.display = "none";
      this.cellPreviewEl.innerHTML = "";
      return;
    }
    const value = selected ?? fromPanel ?? fromGrid;
    this.cellPreviewEl.style.display = "";
    this.app.renderPreviewPanel(this.cellPreviewEl, value, this.tab.id);
  }

  controlledCopyText(mode: CopyMode, explicitGrid = false): string | null {
    const panel = this.controlledResult;
    if (!panel || this.activeResultTab < 0 || this.resultPanelHost.style.display === "none") return null;
    if (explicitGrid || document.activeElement && panel.el.contains(document.activeElement)) return panel.copyText(mode);
    return null;
  }
  private allowResultReplacement(cause: string): boolean {
    if (this.controlledResult?.busy) {
      this.app.toast("error", `${cause}已阻止：结果预览、写入或未知回执尚未处理。可查看 / 复制证据，不能重放或覆盖。`); return false;
    }
    if (this.controlledResult?.dirty) {
      this.app.toast("info", `${cause}已阻止：结果含未提交输入或行草稿。请先撤销此行草稿，或选择“释放编辑结果”。`); return false;
    }
    return true;
  }
  private disposeControlledResult(): void {
    ++this.resultEvidenceRequest; this.resultEvidenceLoading = false;
    this.controlledResult?.dispose(); this.controlledResult = null; this.controlledResultContext = null;
    this.resultPanelHost?.replaceChildren(); if (this.resultPanelHost) this.resultPanelHost.style.display = "none";
    if (this.resultRecoveryTools) this.resultRecoveryTools.style.display = "none";
  }
  private releaseQueryEvidence(context: QueryResultIpcContext): void {
    void queryResultApi.release(context).catch(error => {
      if (!["EXECUTION_NOT_FOUND", "QUERY_RESULT_EXPIRED", "QUERY_RESULT_READ_ONLY"].includes((error as { code?: string }).code ?? ""))
        this.app.toast("info", `查询来源证据释放未确认：${String(error)}。不会重新执行 SQL。`);
    });
  }
  private releaseResultEditing(): void {
    const execution = this.resultExecution; this.resultExecution = null;
    this.disposeControlledResult(); this.resultEvidenceKey = ""; this.resultEvidenceReason = "";
    if (execution) this.releaseQueryEvidence({ ...execution.context, result_index: 0 });
  }
  private showResultRecovery(): void {
    if (!this.controlledResult) return;
    showDraftText("查询 SQL、行草稿与写入证据", JSON.stringify(this.captureDraft(), null, 2), "可选中并复制完整类型化草稿、原执行身份和已发送计划回执。这里不会重新取得行授权或重放 SQL；结果未知时请先核对数据库状态。");
  }
  /** 丢弃结果内的行草稿并释放编辑；已执行 SQL 的缓存保留。 */
  private async releaseResultAfterConfirm(): Promise<void> {
    const panel = this.controlledResult;
    if (!panel || this.disposed) return;
    if (panel.busy || this.resultBusy()) { this.app.toast("error", "写入、预览或未知回执尚未处理；不能释放此结果"); return; }
    const accepted = await confirmTransactionAction("释放编辑结果",
      "当前结果含未提交的行草稿或输入。释放会丢弃这些本地草稿并关闭编辑状态，已执行 SQL 的结果缓存保留；不会重新执行或提交数据库操作。确定释放吗？", "丢弃草稿并释放");
    if (!accepted || this.disposed || this.controlledResult !== panel) return;
    this.releaseResultEditing(); this.tab.dirty = this.draftDirty(); this.renderResults(); this.app.notifyToolbar?.();
  }
  private resultContextCurrent(context: QueryResultIpcContext, key: string): boolean {
    const execution = this.resultExecution;
    return !this.disposed && !this.tab.datasourceStale && !this.running && execution?.result === this.tab.results
      && execution.context.execution_id === context.execution_id && this.tab.id === context.owner_id
      && this.tab.connectionId === context.connection_id && this.tab.schema === context.namespace
      && this.activeResultTab === context.result_index && this.resultEvidenceKey === key;
  }
  private async requestControlledResult(): Promise<void> {
    const execution = this.resultExecution, index = this.activeResultTab, set = this.currentSet(), page = this.resultPages.page(index);
    if (!execution || execution.result !== this.tab.results || !set?.columns.length || !page || this.running || this.exporting || page.loading || this.disposed || this.tab.datasourceStale) return;
    const context: QueryResultIpcContext = { ...execution.context, result_index: index };
    const key = JSON.stringify([context, page.offset, page.rows.length, page.request, page.total]);
    if (key === this.resultEvidenceKey) return;
    if (!this.allowResultReplacement("替换结果证据")) return;
    this.disposeControlledResult(); this.resultEvidenceKey = key; this.resultEvidenceReason = "正在核对本次执行的来源证据；当前结果只读。";
    const request = ++this.resultEvidenceRequest; this.resultEvidenceLoading = true; this.syncManualTransactionControls(); this.renderResultPager();
    try {
      const snapshot = await queryResultApi.snapshot(context, { columns: set.columns, offset: page.offset, loaded_rows: page.rows.length, total_rows: page.total });
      if (request !== this.resultEvidenceRequest || !this.resultContextCurrent(context, key)) return;
      const requireCurrent = () => { if (!this.resultContextCurrent(context, key) || this.exporting || this.resultPages.busy || this.tab.datasourceStale || this.app.connectionById(this.tab.connectionId)?.read_only) throw new Error("查询结果或连接上下文已改变，没有发送写入"); };
      const panel = new QueryResultPanel(snapshot, {
        preview: change => { requireCurrent(); return queryResultApi.preview(context, change); },
        execute: planId => { requireCurrent(); return api.executeRowMutation(planId, context.owner_id); },
        onChanged: () => { if (this.disposed) return; this.tab.dirty = this.draftDirty(); this.syncManualTransactionControls(); this.renderResultPager(); this.app.notifyToolbar?.(); },
        onPreviewChange: () => this.updateCellPreview(),
        toast: (kind, text) => this.app.toast(kind, text),
      });
      this.controlledResult = panel; this.controlledResultContext = context; this.resultEvidenceReason = "";
      this.resultPanelHost.replaceChildren(panel.el);
    } catch (error) {
      if (request === this.resultEvidenceRequest && this.resultContextCurrent(context, key)) this.resultEvidenceReason = `此页保持只读：${error instanceof Error ? error.message : String(error)}`;
    } finally {
      if (request === this.resultEvidenceRequest) { this.resultEvidenceLoading = false; if (!this.disposed) { this.syncManualTransactionControls(); this.renderResults(); this.app.notifyToolbar?.(); } }
    }
  }

  /** 结果页底部统计。 */
  statusInfo(): { sql: string; pageText: string; elapsed: string } {
    const set = this.currentSet(), page = this.resultPages.page(this.activeResultTab);
    const result = this.tab.results;
    const receipts = result?.sets.filter(item => !item.columns.length) ?? [];
    const progress = page?.loading ? " · 正在读取缓存…" : this.exporting ? " · 正在导出完整缓存…" : "";
    const affected = receipts.reduce((sum, item) => sum + item.affected, 0);
    const affectedKnown = receipts.every(item => Number.isSafeInteger(item.affected) && item.affected >= 0)
      && Number.isSafeInteger(affected);
    const rows = set ? `共 ${page?.total ?? set.total} 行`
      : receipts.length ? affectedKnown ? `影响 ${affected} 行` : "影响行数未知" : "";
    return {
      sql: this.editor.getSelection() || this.editor.getSql(),
      pageText: rows + progress,
      elapsed: rows && result ? `运行耗时 ${(result.elapsed_ms / 1000).toFixed(3)}s` : "",
    };
  }

  /** 当前激活的结果集。 */
  currentSet() {
    return this.tab.results?.sets[this.tab.activeResult] ?? null;
  }

  private releaseSpool(scope: import("./types").QuerySpoolScope | null): void {
    if (scope) void api.dropQuerySpool(scope).catch(error => this.app.toast("error", `释放查询缓存失败：${String(error)}`));
  }

  private clearResultSnapshot(): void {
    if (!this.allowResultReplacement("清理查询结果")) throw new Error("查询结果草稿或写入证据需要先处理");
    this.releaseResultEditing();
    ++this.executionRevision;
    this.releaseSpool(this.resultPages.replace(null));
    this.tab.results = null; this.activeResultTab = -1; this.lastMessages = [];
    this.renderResults();
  }

  /** 执行开始时作废旧结果：网格、消息与导出证据都不再代表当前 SQL 的输出。 */
  private expireResultForNewExecution(): void {
    ++this.executionRevision;
    this.releaseSpool(this.resultPages.replace(null));
    this.tab.results = null;
    this.resultExecution = null;
    this.resultEvidenceKey = "";
    this.resultEvidenceReason = "";
    this.activeResultTab = -1;
    this.lastMessages = [];
    this.tab.selectedRows.clear();
    this.grid.setData({ columns: [], rows: [], readOnly: true });
    this.cellPreviewEl.replaceChildren();
    this.cellPreviewEl.style.display = "none";
  }

  /** 前端提交即进入「查询中」；后端状态回调可能仍在读取能力，不能等它才反馈。 */
  private markExecutionRunning(): void {
    this.executionState = "queued";
    this.executionLabelEl.textContent = "查询中";
    this.executionStatusEl.setAttribute("data-state", "running");
  }

  /** 执行状态与消息页共用的文案；终态时带定格耗时。 */
  private executionStatusText(): string {
    const label = this.executionLabelEl.textContent ?? "";
    const elapsed = this.executionTimerEl.textContent ?? "";
    return elapsed ? `${label} · ${elapsed}` : label;
  }

  /** 消息页第一行；省略号只补一次。 */
  private runningMessageText(elapsed: string): string {
    const label = (this.executionLabelEl.textContent || "查询中").replace(/…+$/, "");
    return `${label}… ${elapsed}`;
  }

  /** 毫秒计时：起点固定在本次提交，状态轮询不会重置它。 */
  /** 高精度时钟；无计时能力的宿主退回墙钟。 */
  private executionNow(): number {
    return typeof performance === "object" && typeof performance.now === "function" ? performance.now() : Date.now();
  }

  /** 固定三位小数的秒；等宽数字排版不会随刷新跳动。 */
  private formatExecutionElapsed(ms: number): string {
    return `${(Math.max(0, ms) / 1000).toFixed(3)} s`;
  }

  private startExecutionClock(): void {
    if (this.executionClockStartedAt !== null) return;
    this.executionClockStartedAt = this.executionNow();
    this.executionTimerEl.textContent = "0.000 s";
    // 无定时器的宿主里只是不再续刷，起点与定格值仍然正确。
    if (typeof setInterval === "function") this.executionClockTimer = setInterval(() => this.tickExecutionClock(), 50);
  }

  private tickExecutionClock(): void {
    const startedAt = this.executionClockStartedAt;
    if (startedAt === null) return;
    const elapsed = this.formatExecutionElapsed(this.executionNow() - startedAt);
    this.executionTimerEl.textContent = elapsed;
    if (this.resultLoadingTimerEl && this.running && !this.resultLoadingEl?.hidden) {
      this.resultLoadingTimerEl.textContent = elapsed;
      const label = this.executionState === "cancel_requested" ? "正在停止查询… " : "正在查询… ";
      if (this.resultLoadingLabelEl && this.resultLoadingLabelEl.textContent !== label) this.resultLoadingLabelEl.textContent = label;
    }
    if (this.running && this.lastMessages[0] && !this.lastMessages[0].error) {
      this.lastMessages[0].text = this.runningMessageText(elapsed);
    }
    if (this.executionMessageLabelEl) {
      const label = this.runningMessageText("").trimEnd();
      if (this.executionMessageLabelEl.textContent !== label) this.executionMessageLabelEl.textContent = label;
    }
    if (this.executionMessageTimerEl) this.executionMessageTimerEl.textContent = elapsed;
  }

  /** 终态定格最终耗时并停表；保留消息证据与结果页脚读数，不归零。 */
  private freezeExecutionClock(): void {
    const startedAt = this.executionClockStartedAt;
    if (startedAt !== null) {
      this.executionTimerEl.textContent = this.formatExecutionElapsed(this.executionNow() - startedAt);
      if (this.resultLoadingTimerEl && this.running) this.resultLoadingTimerEl.textContent = this.executionTimerEl.textContent;
    }
    this.stopExecutionClock();
  }

  private stopExecutionClock(): void {
    if (this.executionClockTimer !== null && typeof clearInterval === "function") clearInterval(this.executionClockTimer);
    this.executionClockTimer = null;
    this.executionClockStartedAt = null;
  }

  captureResultExport() {
    return { result: this.tab.results, scope: this.resultPages.scope, index: this.activeResultTab,
      connectionId: this.tab.connectionId, namespace: this.tab.schema };
  }

  async exportResultSnapshot(opts: import("./types").ExportOptions, snapshot: ReturnType<QueryTabView["captureResultExport"]>): Promise<number> {
    if (this.disposed || snapshot.result !== this.tab.results || snapshot.connectionId !== this.tab.connectionId || snapshot.namespace !== this.tab.schema)
      throw new Error("查询结果已经替换或页面已关闭，请重新打开导出；不会重新执行 SQL。");
    if (this.resultBusy()) throw new Error("查询执行、缓存读取或导出进行中，请等待完成");
    const set = snapshot.result?.sets[snapshot.index];
    if (!set?.columns.length) throw new Error("当前没有可导出的结果集");
    this.exporting = true; this.syncManualTransactionControls(); this.renderResultPager(); this.app.notifyToolbar?.();
    try {
      const options = { ...opts, connection_id: snapshot.connectionId, schema: snapshot.namespace };
      return snapshot.scope
        ? await api.exportQuerySpooled(options, snapshot.scope, snapshot.index)
        : await api.exportQueryResult(options, set.columns.map(column => column.name), set.rows);
    } finally {
      this.exporting = false;
      if (!this.disposed) { this.syncManualTransactionControls(); this.renderResultPager(); this.app.notifyToolbar?.(); }
    }
  }

  private async jumpResultPage(): Promise<void> {
    if (this.pageNumberInput.disabled || this.resultBusy() || this.disposed) return;
    const text = this.pageNumberInput.value.trim(), total = this.resultPages.pageCount(this.activeResultTab);
    const number = Number(text);
    if (!/^[0-9]+$/.test(text) || !Number.isSafeInteger(number) || number < 1 || number > total) {
      this.pageErrorEl.textContent = `请输入 1–${total} 的整数页码`; this.pageErrorEl.hidden = false;
      this.pageNumberInput.setAttribute("aria-invalid", "true"); return;
    }
    await this.loadResultPage(number);
  }

  private async loadResultPage(action: "first" | "previous" | "next" | "last" | number, limit = this.resultPages.pageLimit(this.activeResultTab)): Promise<void> {
    if (!this.allowResultReplacement("翻页") || this.resultBusy() || this.disposed) return;
    const index = this.activeResultTab;
    const offset = typeof action === "number" ? this.resultPages.pageOffset(index, action) : this.resultPages.offset(index, action);
    if (offset === null || typeof action === "number" && offset === this.resultPages.page(index)?.offset) { this.renderResultPager(); return; }
    this.disposeControlledResult(); this.resultEvidenceKey = "";
    const loading = this.resultPages.fetch(index, offset, (scope, resultIndex, offset, limit) => api.fetchQueryRows(scope, resultIndex, offset, limit), limit);
    this.syncManualTransactionControls(); this.renderResultPager(); this.app.notifyToolbar?.();
    try { await loading; }
    catch (error) {
      if (!this.disposed) this.app.toast("error", `读取结果缓存失败：${String(error)}`);
    } finally {
      if (!this.disposed) {
        this.syncManualTransactionControls(); this.renderResults(); this.app.renderStatus?.(); this.app.notifyToolbar?.();
      }
    }
  }

  private updateResultLoading(): void {
    const page = this.resultPages.page(this.activeResultTab);
    const loading = this.running || !!page?.loading;
    if (!this.resultLoadingEl) {
      this.resultLoadingEl = document.createElement("div");
      this.resultLoadingEl.className = "grid-loading-overlay query-result-loading";
      this.resultLoadingEl.setAttribute("role", "status");
      this.resultLoadingEl.setAttribute("aria-live", "polite");
      this.resultLoadingLabelEl = document.createElement("span");
      this.resultLoadingLabelEl.className = "query-loading-label";
      this.resultLoadingTimerEl = document.createElement("span");
      this.resultLoadingTimerEl.className = "query-loading-timer";
      // 数字每 50ms 更新，仅作视觉辅助，避免读屏反复播报。
      this.resultLoadingTimerEl.setAttribute("aria-hidden", "true");
      this.resultLoadingEl.append(this.resultLoadingLabelEl, this.resultLoadingTimerEl);
      this.resultBody.appendChild(this.resultLoadingEl);
    }
    this.resultLoadingEl.hidden = !loading;
    const queryLabel = this.executionState === "cancel_requested" ? "正在停止查询…" : "正在查询…";
    this.resultLoadingLabelEl!.textContent = loading ? this.running ? `${queryLabel} ` : "正在加载结果页…" : "";
    this.resultLoadingTimerEl!.hidden = !loading || !this.running;
    this.resultLoadingTimerEl!.textContent = loading && this.running ? this.executionTimerEl.textContent || "0.000 s" : "";
    this.resultBody.setAttribute("aria-busy", String(loading));
    this.grid.el.inert = loading;
    this.resultPanelHost.inert = loading;
    this.messagesEl.inert = loading;
    this.messagesEl.style.visibility = this.running ? "hidden" : "";
  }

  private renderResultPager(): void {
    this.updateResultLoading();
    const page = this.resultPages.page(this.activeResultTab), scope = this.resultPages.scope;
    const visible = !!this.currentSet()?.columns.length;
    const info = this.statusInfo();
    this.resultPager.style.display = visible || !!info.elapsed ? "flex" : "none";
    this.pageControls.style.display = visible ? "flex" : "none";
    this.pageSizeSelect.parentElement!.style.display = visible ? "inline-flex" : "none";
    this.resultPager.setAttribute("aria-busy", String(!!page?.loading));
    const blocked = this.resultBusy() || this.controlledResult?.dirty === true || !scope || !page;
    this.pageNumberInput.value = String(this.resultPages.pageNumber(this.activeResultTab));
    this.pageNumberInput.disabled = blocked || !page?.total;
    this.pageSizeSelect.value = String(this.resultPages.pageLimit(this.activeResultTab));
    this.pageSizeSelect.disabled = blocked || !page?.total;
    this.pageNumberInput.removeAttribute("aria-invalid"); this.pageErrorEl.hidden = true;
    const count = this.resultPages.pageCount(this.activeResultTab);
    this.pageCountEl.textContent = `/ ${count} 页`;
    this.pageNumberInput.setAttribute("aria-description", `共 ${count} 页；输入页码后按 Enter 或移开焦点跳转`);
    this.footerStatusEl.textContent = [info.elapsed, info.pageText].filter(Boolean).join(" | ");
    this.pageButtons.forEach((button, index) => {
      // 首屏0行时允许首页读取重试（超大单行也可直接导出）。
      button.disabled = blocked || page!.total === 0 || (index <= 1 ? page!.offset === 0 && page!.rows.length > 0 : index === 2
        ? page!.rows.length === 0 || page!.offset + page!.rows.length >= page!.total : page!.total === 0 || page!.offset + page!.rows.length >= page!.total);
      button.title = this.controlledResult?.dirty ? "结果含未提交输入或行草稿；请先撤销或释放编辑结果" : !scope ? "此历史结果无完整缓存；不能通过重新执行 SQL 获取后续行" : this.resultBusy() ? "当前操作进行中，请等待完成" : "读取本次执行缓存，不重新执行 SQL";
    });
  }

  /**
   * 运行（runBtn / F5 / 菜单「运行」）始终执行编辑器全文；
   * 只有「运行已选择的」才读取选区，且无选区时必须提示而不是退回全文。
   * 旧实现忽略 selectionOnly，任何残留选区都会把全文截成片段执行，
   * 例如选中 `* FROM t_big` 后运行会发送缺少 SELECT 的语句并触发 MySQL 1064。
   */
  async executeSelection(selectionOnly: boolean): Promise<void> {
    if (!selectionOnly) {
      await this.execute(this.editor.getSql(), false);
      return;
    }
    const sql = this.editor.hasSelection() ? this.editor.getSelection() : "";
    if (!sql.trim()) {
      this.app.toast("info", "请先选中要执行的 SQL");
      return;
    }
    await this.execute(sql, true);
  }

  /** 供 App 菜单调用：解释当前 SQL 全文。 */
  async explainPublic(): Promise<void> {
    await this.explainSelection(false);
  }

  /** 解释 SQL：默认解释编辑器全文；只有右键「解释已选择的」传入 true 才解释选区。 */
  private async explainSelection(selectionOnly = false): Promise<void> {
    if (this.app.manualTransactionFor?.(this.tab.connectionId)) {
      this.app.toast("info", "手动事务期间不能解释 SQL，请先提交或回滚事务");
      return;
    }
    const sql = (selectionOnly
      ? (this.editor.hasSelection() ? this.editor.getSelection() : "")
      : this.editor.getSql()).trim();
    if (!sql) {
      this.app.toast("info", selectionOnly ? "请先选中要解释的 SQL" : "SQL 为空");
      return;
    }
    try {
      await this.execute(explainQuerySql(sql), false);
    } catch (e) {
      this.app.toast("info", e instanceof Error ? e.message : String(e));
    }
  }

  invalidateDatasource(): void {
    if (this.controlledResult?.dirty || this.controlledResult?.busy) {
      ++this.resultEvidenceRequest; this.resultEvidenceLoading = false;
      this.controlledResult.invalidate(this.tab.datasourceStale || "数据源已改变，原输入和写入证据已保留；请核对数据库状态");
    } else this.clearResultSnapshot();
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
    if (this.editor.getSql().trim()) await this.executeSelection(false);
  }

  /** 切换运行条中的连接：不得沿用旧连接的模式或补全。 */
  private async switchConnection(connId: string): Promise<void> {
    if (!this.allowResultReplacement("切换连接")) { this.connSelect.value = this.tab.connectionId; return; }
    if (this.resultBusy() || this.app.manualTransactionFor?.(this.tab.connectionId) || this.app.manualTransactionIsBusy?.(this.tab.connectionId)) {
      this.connSelect.value = this.tab.connectionId;
      this.app.toast("info", "事务或查询进行中，请先完成再切换连接");
      return;
    }
    if (!connId || !this.app.connectionById(connId)) return;
    const oldConnection = this.tab.connectionId;
    const draftIds = this.tab.schema || this.tab.savedQueryId ? [this.tab.id] : [];
    const accepted = await this.app.guardLeave(draftIds, "切换查询连接", async () => { await this.applyConnectionChoice(connId); });
    if (!accepted && !this.disposed) this.connSelect.value = oldConnection;
  }
  private async applyConnectionChoice(connId: string): Promise<void> {
    this.clearResultSnapshot();
    this.executionNeedsReview = false;
    this.executionCapabilities = null;
    this.lastMessages = [];
    if (connId !== this.tab.connectionId && this.tab.savedQueryId) this.detachSavedQuery();
    if (this.tab.datasourceStale) {
      delete this.tab.datasourceStale;
      this.el.querySelector(".datasource-stale-warning")?.remove();
      this.dbSelect.disabled = false;
    }
    this.tab.connectionId = connId;
    this.syncNamespaceLabels();
    this.tab.schema = "";
    this.tab.results = null;
    this.renderResults();
    this.completionLoaded = false;
    const cfg = this.app.connectionById(connId);
    if (cfg) { this.tab.connectionName = cfg.name; this.tab.title = this.tab.savedQueryId ? this.tab.title : `查询 - ${cfg.name}`; }
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
      const capabilities = await api.executionCapabilities(connId);
      if (this.disposed || request !== this.completionRequest || connId !== this.tab.connectionId) return;
      this.executionCapabilities = capabilities;
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
      if (!keep) {
        const unknown = document.createElement("option"); unknown.value = this.tab.savedQueryId ? this.tab.schema : "";
        unknown.textContent = this.tab.savedQueryId && this.tab.schema ? `${this.tab.schema}（当前不可用）` : "请选择数据库";
        this.dbSelect.appendChild(unknown);
        if (!this.tab.savedQueryId) this.tab.schema = "";
      }
      this.dbSelect.value = this.tab.schema;
    } catch (e) {
      if (this.disposed || request !== this.completionRequest || connId !== this.tab.connectionId) return;
      this.dbSelect.innerHTML = "";
      if (!this.tab.savedQueryId) this.tab.schema = "";
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
    if (!this.allowResultReplacement("重新执行查询")) return;
    if (this.tab.datasourceStale) { this.app.toast("info", this.tab.datasourceStale); return; }
    if (this.executionNeedsReview) { this.app.toast("error", "上一执行结果未知，禁止重试。请验证数据库状态并重新选择连接；活跃事务必须明确回滚。"); return; }
    const connId = this.tab.connectionId;
    if (!connId) {
      this.app.toast("error", "请先在运行条中选择一个连接");
      return;
    }
    if (!sql.trim()) {
      this.app.toast("error", "SQL 为空");
      return;
    }
    if (this.resultBusy() || this.disposed) return; // 执行、读取和导出互斥
    const namespace = this.tab.schema;
    const transaction = this.app.manualTransactionFor?.(connId);
    if (this.app.manualTransactionIsBusy?.(connId) || (transaction && (transaction.owner !== this.tab.id || transaction.schema !== this.tab.schema)) || transaction?.failed) {
      this.app.toast("error", transaction?.failed ? "事务执行失败，必须回滚" : "事务由其它页面持有或正在操作，不能执行此 SQL");
      return;
    }
    this.releaseResultEditing();
    // 本次执行从提交这一刻开始：先整体作废上一次的结果，
    // 否则等待返回期间旧结果仍然在屏，会被读成本次查询的输出。
    this.expireResultForNewExecution();
    const revision = this.executionRevision;
    this.running = true;
    this.runBtn.disabled = true;
    this.explainBtn.disabled = true;
    this.connSelect.disabled = true;
    this.dbSelect.disabled = true;
    this.runbar.setAttribute("aria-busy", "true");
    this.markExecutionRunning();
    this.startExecutionClock();
    this.lastMessages = [{ text: this.runningMessageText("0.000 s"), error: false }];
    this.renderResults();
    this.app.notifyToolbar?.();
    try {
      this.executionCapabilities = await api.executionCapabilities(connId);
      if (this.disposed || revision !== this.executionRevision) return;
      const outcome = await this.execution.run({ execution_id: crypto.randomUUID(), connection_id: connId, namespace,
        owner_id: this.tab.id, sql, transaction_owner: transaction?.active ? this.tab.id : null, origin: "query" });
      if (this.disposed || revision !== this.executionRevision || !outcome) return;
      if (outcome.state !== "succeeded" || !outcome.result) {
        this.executionNeedsReview = outcome.state === "unknown";
        this.lastMessages = [{ text: `${this.executionStatusText()}。${outcome.error?.message ?? ""} 本次执行没有可用结果。`, error: true }];
        this.activeResultTab = -1;
        this.renderResults(); this.app.toast(outcome.state === "unknown" ? "error" : "info", this.lastMessages[0].text); return;
      }
      const res: MultiResult = outcome.result;
      const previousSpool = this.resultPages.replace(res, { connectionId: connId, ownerId: this.tab.id, namespace });
      this.tab.results = res;
      this.resultExecution = { context: { execution_id: outcome.execution_id, connection_id: connId, owner_id: this.tab.id, namespace }, result: res };
      this.releaseSpool(previousSpool);
      // 无返回列的 DML/DDL 回执只展示消息；保留原 sets 索引供分页与导出。
      this.activeResultTab = res.sets.findIndex(set => set.columns.length > 0);
      // 消息页与「执行记录」窗口共用同一格式：[时间][连接][库][驱动] + SQL + Time。
      const cfg = this.app.connectionById(connId);
      const record = [
        `[${logStamp()}][${cfg?.name ?? connId}][${namespace || "—"}][${(cfg?.kind ?? "").toUpperCase()}]`,
        sql.trim(),
        ...res.sets.filter(set => !set.columns.length).map(set => `${Number.isSafeInteger(set.affected) && set.affected >= 0 ? `影响 ${set.affected} 行` : "影响行数未知"}${set.message ? ` · ${set.message}` : ""}`),
        `Time: ${(res.elapsed_ms / 1000).toFixed(3)}s`,
      ].join("\n");
      this.lastMessages = [
        { text: record, error: false },
        ...res.messages.map((m) => ({ text: m, error: false })),
      ];
      this.renderResults();
      // 正常完成不弹提示；耗时/行数保留在状态栏，执行记录保留在消息页。

    } catch (e) {
      if (this.disposed || revision !== this.executionRevision) return;
      // 能力读取失败发生在控制器启动之前，也必须解除“查询中”并停止计时。
      if (this.executionClockStartedAt !== null) {
        this.executionState = "failed";
        this.executionLabelEl.textContent = "执行失败";
        this.executionStatusEl.setAttribute("data-state", "failed");
        this.freezeExecutionClock();
      }
      this.lastMessages = [{ text: `${String(e)}。本次执行没有可用结果。`, error: true }];
      this.executionNeedsReview = this.executionState === "unknown";
      this.activeResultTab = -1;
      this.renderResults();
      this.app.toast("error", String(e));
    } finally {
      if (!this.disposed && revision === this.executionRevision) this.freezeExecutionClock();
      await this.app.refreshManualTransactionStatus?.(connId)?.catch(error => this.app.toast("error", `事务状态确认失败：${String(error)}`));
      this.running = false;
      this.stopBtn.disabled = true;
      this.syncManualTransactionControls();
      if (!this.disposed) { this.renderResults(); this.app.renderStatus?.(); }
      this.app.notifyToolbar?.();
      this.runbar.setAttribute("aria-busy", "false");
    }
  }

  /** 渲染结果页签与内容。无输出时结果区整体隐藏，编辑器占满内容区。 */
  private renderResults(): void {
    const res = this.tab.results;
    if (res !== this.resultPages.result && (this.controlledResult?.dirty || this.controlledResult?.busy)) {
      this.controlledResult.invalidate("结果已在外部改变；原行输入和写入证据保留，请核对状态"); return;
    }
    if (res !== this.resultPages.result) this.releaseSpool(this.resultPages.replace(res, { connectionId: this.tab.connectionId, ownerId: this.tab.id, namespace: this.tab.schema }));
    this.resultTabsEl.innerHTML = "";
    const receiptSets = res?.sets.filter(set => !set.columns.length) ?? [];
    const resultIndexes = res?.sets.flatMap((set, index) => set.columns.length ? [index] : []) ?? [];
    if (this.activeResultTab >= 0 && !res?.sets[this.activeResultTab]?.columns.length) this.activeResultTab = -1;
    const hasMessages = this.lastMessages.length > 0 || receiptSets.length > 0;
    const setCount = resultIndexes.length;
    const hasOutput = this.running || hasMessages || setCount > 0;
    if (!hasOutput) {
      this.resultSplitter.style.display = "none";
      this.resultBody.style.display = "none";
      this.resultTabsEl.style.display = "none";
      this.grid.setData({ columns: [], rows: [] });
      this.resultPager.style.display = "none";
      this.gridPreviewNoteEl.style.display = "none";
      this.gridPreviewNoteEl.textContent = "";
      this.messagesEl.style.display = "none";
      this.grid.el.style.display = "";
      // 没有输出就没有可预览的单元格：面板一并清空，不留上次的内容。
      this.cellPreviewEl.style.display = "none";
      this.cellPreviewEl.innerHTML = "";
      return;
    }
    this.resultSplitter.style.display = "";
    this.resultBody.style.display = "flex";
    this.resultBody.style.height = `${this.resultRatio * 100}%`;
    this.resultTabsEl.style.display = "";
    const mkTab = (label: string, index: number) => {
      const t = document.createElement("button");
      t.type = "button";
      t.className = "result-tab" + (index === this.activeResultTab ? " active" : "");
      t.setAttribute("aria-pressed", String(index === this.activeResultTab));
      t.textContent = label;
      t.addEventListener("click", () => {
        if (index !== this.activeResultTab) {
          if (!this.allowResultReplacement("切换结果集") || this.resultBusy()) return;
          this.disposeControlledResult(); this.resultEvidenceKey = "";
        }
        this.activeResultTab = index;
        this.renderResults();
        this.app.renderStatus();
        this.app.notifyToolbar?.();
        const position = index < 0 ? 1 : resultIndexes.indexOf(index) + (hasMessages ? 2 : 1);
        this.resultTabsEl.querySelector<HTMLButtonElement>(`.result-tab:nth-child(${position})`)?.focus();
      });
      this.resultTabsEl.appendChild(t);
    };
    if (hasMessages) mkTab("消息", -1);
    resultIndexes.forEach((index, position) => mkTab(`结果 ${position + 1}`, index));
    // 返回列而非行数决定结果集：空 SELECT 和 DML RETURNING 仍保留结果页。
    const showGrid = this.activeResultTab >= 0 && !!res?.sets[this.activeResultTab]?.columns.length;
    const showPanel = showGrid && !!this.controlledResult;
    this.resultPanelHost.style.display = showPanel ? "flex" : "none";
    this.resultRecoveryTools.style.display = showPanel ? "flex" : "none";
    this.grid.el.style.display = showGrid && !showPanel ? "" : "none";
    this.messagesEl.style.display = showGrid ? "none" : "";
    this.renderResultPager();
    if (!showGrid) {
      this.gridPreviewNoteEl.style.display = "none";
      this.gridPreviewNoteEl.textContent = "";
      // 「消息」页不展示单元格内容面板，否则底部会留一条空的 42px 面板
      this.cellPreviewEl.style.display = "none";
      this.cellPreviewEl.innerHTML = "";
    }
    if (showGrid && res) {
      const set = res.sets[this.activeResultTab];
      const page = this.resultPages.page(this.activeResultTab);
      // 不再显示通用说明文字；只保留与当前页真实状态相关的提示：
      // 无完整缓存（仅已加载行）、字节预算截断、读取缓存失败。
      // 完整缓存的常规完成消息留在「消息」页，不再占据网格上方；异常警告仍展示。
      const message = page?.message ?? "";
      const completionNotice = /^查询已完成[，,]/.test(message) && /(?:内存预览|分页与导出使用本次缓存)/.test(message) && !/不完整|失败|超出|截断|过期/.test(message);
      const note = this.resultPages.scope
        ? (completionNotice ? "" : message)
        : `无完整缓存，仅已加载 ${set.rows.length} / ${set.total} 行；复制、筛选与导出只含已加载行，不会重新执行 SQL。`;
      this.gridPreviewNoteEl.textContent = note;
      this.gridPreviewNoteEl.title = note || this.resultEvidenceReason;
      this.gridPreviewNoteEl.style.display = note ? "block" : "none";
      if (this.controlledResult) { this.updateCellPreview(); return; }
      this.grid.setData({
        columns: set.columns,
        rows: set.rows,
        rowNumberOffset: page?.offset ?? 0,
        readOnly: true,
        skipNull: this.app.skipNullState(),
        lined: this.app.showGridLinesState(),
      });
      // 结果页重绘后没有选中单元格：面板一并收起，避免残留上一次的焦点内容。
      const preview = this.cellPreviewEl;
      if (preview && !this.grid.editingActive && !this.grid.focusedCell) { preview.style.display = "none"; preview.innerHTML = ""; }
      void this.requestControlledResult();
    } else {
      this.renderMessages();
    }
  }

  /** 渲染消息页内容。 */
  private renderMessages(): void {
    this.messagesEl.innerHTML = "";
    this.executionMessageLabelEl = null;
    this.executionMessageTimerEl = null;
    const messages = this.lastMessages.length ? this.lastMessages
      : this.tab.results?.sets.filter(set => !set.columns.length).map(set => ({
        text: `${Number.isSafeInteger(set.affected) && set.affected >= 0 ? `影响 ${set.affected} 行` : "影响行数未知"}${set.message ? ` · ${set.message}` : ""}`, error: false,
      })) ?? [];
    messages.forEach((m, index) => {
      const line = document.createElement("div");
      line.className = m.error ? "msg-error" : "msg-ok";
      // 活计时只属于尚未结束的执行；完成日志不能被轮询改写。
      if (!m.error && this.executionClockStartedAt !== null && index === 0) {
        line.classList.add("msg-running");
        this.executionMessageLabelEl = document.createElement("span");
        this.executionMessageLabelEl.textContent = this.runningMessageText("").trimEnd();
        this.executionMessageTimerEl = document.createElement("span");
        this.executionMessageTimerEl.className = "query-message-timer";
        this.executionMessageTimerEl.setAttribute("aria-hidden", "true");
        this.executionMessageTimerEl.textContent = this.executionTimerEl.textContent;
        line.append(this.executionMessageLabelEl, this.executionMessageTimerEl);
      } else line.textContent = m.text;
      this.messagesEl.appendChild(line);
    });
  }

  destroy(): void {
    if (this.controlledResult?.busy) throw new Error("查询结果写入或未知证据仍未确认，禁止销毁页面");
    this.releaseResultEditing();
    this.stopExecutionClock();
    this.disposed = true;
    ++this.executionRevision;
    this.releaseSpool(this.resultPages.replace(null));
    this.execution.dispose();
    ++this.completionRequest;
    document.removeEventListener("selectionchange", this.selectionListener);
    this.editor.destroy();
    this.grid.dispose();
  }
}

/** 表结构视图（可编辑 + 保存 DDL）。 */
/** 备份视图（参考截图 14：网格 名/修改日期/文件大小/注释，数据来自真实备份目录）。 */
class BackupTabView extends TabView {
  grid: DataGrid;
  private backups: import("./types").BackupInfo[] = [];
  private generation = 0;
  private disposed = false;

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
    if (this.disposed) return;
    const generation = ++this.generation;
    const { connectionId, schema } = this.tab;
    const current = () => !this.disposed && generation === this.generation && connectionId === this.tab.connectionId && schema === this.tab.schema;
    this.backups = [];
    this.tab.selectedRows.clear();
    this.grid.setData({ columns: [], rows: [], readOnly: true });
    this.footer.textContent = "正在读取备份…";
    try {
      const backups = await api.listBackups(connectionId, schema);
      if (!current()) return;
      this.backups = backups;
    } catch (e) {
      if (!current()) return;
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
    this.disposed = true;
    ++this.generation;
    this.grid.dispose();
  }
}

/**
 * SQL 对象设计器（视图 / 函数 / 存储过程共用）：SQL 编辑器 + 保存重建。
 * mode=view 保存走 CREATE OR REPLACE（SQLite 为 DROP+CREATE），
 * mode=routine 保存整体重建（单语句执行，支持 BEGIN..END 体）。
 */
class RoutineTabView extends TabView {
  private readonly document: RoutineDocumentView;
  constructor(app: App, tab: TabState) { super(app, tab); this.document = new RoutineDocumentView(app, tab); this.el.append(this.document.el); }
  activate(): void { this.document.activate(); }
  reload(): Promise<void> { return this.document.reload(); }
  getSql(): string { return this.document.getSql(); }
  copyEditor(): SqlEditor { return this.document.copyEditor(); }
  saveRoutine(): Promise<boolean> { return this.document.saveRoutine(); }
  showError(message: string): void { this.document.showError(message); }
  draftDirty(): boolean { return this.document.draftDirty(); }
  draftBusy(): boolean { return this.document.draftBusy(); }
  captureDraft(): Pick<DraftSnapshot, "kind" | "baseline" | "payload"> { return this.document.captureDraft(); }
  discardDraft(): void { this.document.discardDraft(); }
  destroy(): void { this.document.dispose(); }
}

class SqlDesignTabView extends TabView {
  private editor: SqlEditor;
  private mode: "view" | "routine";
  private sqlBaseline: string;
  private loadGeneration = 0;
  private disposed = false;
  private error: HTMLElement;

  constructor(app: App, tab: TabState, mode: "view" | "routine") {
    super(app, tab);
    this.mode = mode;
    this.sqlBaseline = tab.designCreating ? "" : tab.sql || "";
    this.error = document.createElement("div"); this.error.className = "object-action-error";
    this.error.setAttribute("aria-live", "assertive"); this.el.append(this.error);
    const unavailable = objectCapability(app.connectionById(tab.connectionId), tab.objectType, tab.designCreating ? "create" : "update");
    if (unavailable) this.showError(unavailable);
    this.editor = new SqlEditor({ value: tab.sql || "", onChange: sql => {
      this.tab.sql = sql; this.tab.dirty = sql !== this.sqlBaseline;    },
      onFormat: () => { void app.runCommand("beautify-sql"); },
      dialect: objectProtocol(app.connectionById(tab.connectionId)) as "mysql" | "postgres" | "sqlite" });
    this.el.appendChild(this.editor.el);
  }

  activate(): void {
    this.editor.focus();
    this.app.renderStatus();
  }

  getSql(): string {
    return this.editor.getSql();
  }
  showError(message: string): void { this.error.textContent = message; this.error.hidden = !message; }
  copyEditor(): SqlEditor { return this.editor; }
  draftDirty(): boolean { return this.editor.getSql() !== this.sqlBaseline; }
  captureDraft(): Pick<DraftSnapshot, "kind" | "baseline" | "payload"> { return { kind: this.mode, baseline: this.sqlBaseline, payload: { sql: this.editor.getSql(), requires_validation: true } }; }
  markClean(sql = this.editor.getSql()): void { this.sqlBaseline = sql; this.tab.dirty = this.draftDirty(); }
  discardDraft(): void { this.editor.setSql(this.sqlBaseline); this.tab.dirty = false; }

  async reload(): Promise<void> {
    if (this.tab.designCreating) return;
    const generation = ++this.loadGeneration, before = this.editor.getSql();
    try {
      let sql: string;
      if (this.mode === "view") {
        sql = await api.getViewDefinition(
          this.tab.connectionId,
          this.tab.schema,
          this.tab.object,
        );
      } else {
        sql = await api.getRoutineDefinition(
          this.tab.connectionId,
          this.tab.schema,
          this.tab.object,
          this.tab.objectType === "procedure" ? "procedure" : "function",
        );
      }
      if (this.disposed || generation !== this.loadGeneration || this.editor.getSql() !== before) throw new Error("读取期间编辑已变化，保留当前 SQL");
      this.editor.setSql(sql || "");
      this.markClean(sql || "");
    } catch (e) {
      throw new Error(`读取定义失败，当前草稿已保留：${String(e)}`);
    }
  }

  destroy(): void {
    this.disposed = true; ++this.loadGeneration;
    this.editor.destroy();
  }
}

/** 自动任务视图：真实任务列表 + 执行 + 历史。 */
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
      this.app.toast("error", `读取任务失败，保留上次已加载列表（当前状态未确认）：${String(e)}`);
      return;
    }
const connName = (id: string) => this.app.connectionName(id);
    const TASK_LABELS: Record<string, string> = { backup: "备份", export: "导出", import: "导入", data_sync: "数据同步", data_generation: "数据生成", data_dictionary: "数据字典" };
    const scheduleText = (j: import("./types").ScheduleJob): string => {
      if (j.cron) return `Cron ${j.cron}`;
      if (j.daily_at) return `每天 ${j.daily_at}`;
      if (j.interval_secs > 0) return formatInterval(j.interval_secs);
      return "手动";
    };
    this.grid.setData({
      columns: [
        { name: "任务名", type: "text", nullable: false, primary_key: false, auto_increment: false },
        { name: "任务类型", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "定时", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "连接", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "数据库", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "启用", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "上次运行", type: "text", nullable: true, primary_key: false, auto_increment: false },
        { name: "状态", type: "text", nullable: true, primary_key: false, auto_increment: false },
      ],
      rows: this.jobs.map((j) => [
        j.name,
        j.task ? TASK_LABELS[j.task.kind] ?? j.task.kind : "SQL（旧版）",
        scheduleText(j),
        connName(j.connection_id),
        j.database || "",
        j.enabled ? "是" : "否",
        j.last_run ?? "",
        j.last_status ?? "",
      ]),
      rowIcons: this.jobs.map(() => "info_pane_query"),
      showRowNumbers: false,
      fillWidth: true,
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

/** 解析单个 cron 字段为取值集合（支持 * , - /）；非法返回 null。 */
function parseCronField(field: string, min: number, max: number): Set<number> | null {
  const values = new Set<number>();
  for (const item of field.split(",")) {
    const match = /^(\*|\d{1,3}(?:-\d{1,3})?)(?:\/(\d{1,3}))?$/.exec(item);
    if (!match) return null;
    const step = match[2] === undefined ? 1 : Number(match[2]);
    if (step <= 0) return null;
    let start = min;
    let end = max;
    if (match[1] !== "*") {
      const parts = match[1].split("-").map(Number);
      start = parts[0];
      end = parts[1] ?? parts[0];
      if (start < min || end > max || start > end) return null;
    }
    for (let v = start; v <= end; v += step) values.add(v);
  }
  return values;
}

/** 计算五字段 cron（分 时 日 月 星期）的下一次触发时间（本地时区，向后逐分钟扫描，上限 366 天）。 */
function cronNextOccurrence(cron: string, from = new Date()): number | null {
  const parts = cron.trim().split(/\s+/);
  if (parts.length !== 5) return null;
  const ranges: [number, number][] = [[0, 59], [0, 23], [1, 31], [1, 12], [0, 7]];
  const sets = parts.map((field, i) => parseCronField(field, ranges[i][0], ranges[i][1]));
  if (sets.some((s) => s === null)) return null;
  const [minutes, hours, days, months, weekdays] = sets as Set<number>[];
  // 星期：cron 用 0/7 表示周日，JS getDay() 0 = 周日。
  const wd = new Set([...weekdays].map((v) => v % 7));
  const dayRestricted = parts[2] !== "*";
  const wdRestricted = parts[4] !== "*";
  const t = new Date(from.getTime());
  t.setSeconds(0, 0);
  t.setMinutes(t.getMinutes() + 1);
  const limit = new Date(from.getTime() + 366 * 86400000);
  while (t <= limit) {
    if (!months.has(t.getMonth() + 1)) {
      t.setMonth(t.getMonth() + 1, 1);
      t.setHours(0, 0, 0, 0);
      continue;
    }
    // 日与星期同时受限时按标准 cron 的 OR 语义。
    const dayOk = dayRestricted && wdRestricted
      ? days.has(t.getDate()) || wd.has(t.getDay())
      : dayRestricted ? days.has(t.getDate())
        : wdRestricted ? wd.has(t.getDay()) : true;
    if (!dayOk) {
      t.setDate(t.getDate() + 1);
      t.setHours(0, 0, 0, 0);
      continue;
    }
    if (!hours.has(t.getHours())) {
      t.setHours(t.getHours() + 1, 0, 0, 0);
      continue;
    }
    if (!minutes.has(t.getMinutes())) {
      t.setMinutes(t.getMinutes() + 1, 0, 0);
      continue;
    }
    return t.getTime();
  }
  return null;
}

/** 执行记录时间戳：本地 "YYYY-MM-DD HH:mm:ss.SSS"。 */
function logStamp(date = new Date()): string {
  const pad = (value: number, size = 2) => String(value).padStart(size, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${pad(date.getMilliseconds(), 3)}`;
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
  { id: "rename-backup", label: "重命名..." },
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
  private readonly designer: TableDesigner;

  constructor(app: App, tab: TabState) {
    super(app, tab);
    this.designer = new TableDesigner(tab, {
      connection: () => app.connectionById(tab.connectionId),
      writeReason: () => tab.datasourceStale || (app.connectionById(tab.connectionId)?.read_only ? "此连接只读，不能修改表结构" : !app.connectionById(tab.connectionId) ? "连接不存在" : app.manualTransactionFor(tab.connectionId) || app.manualTransactionIsBusy(tab.connectionId) ? "请先完成此连接的手动事务" : null),
      changed: () => { app.notifyToolbar(); },
      status: () => app.renderStatus(),
      toast: (kind, text) => app.toast(kind, text),
    }, this.el);
  }

  activate(): void { this.designer.activate(); }
  flushDraft(): void { this.designer.flushDraft(); }
  draftDirty(): boolean { return this.designer.draftDirty(); }
  discardDraft(): void { this.designer.discardDraft(); }
  draftBusy(): boolean { return this.designer.draftBusy(); }
  captureDraft(): Pick<DraftSnapshot, "kind" | "baseline" | "payload"> { return this.designer.captureDraft(); }
  async reloadAfterLeave(): Promise<void> { await this.designer.reloadAfterLeave(); }
  async reload(): Promise<void> { await this.designer.reload(); }
  addField(): void { this.designer.addField(); }
  insertField(): void { this.designer.insertField(); }
  deleteField(): void { this.designer.deleteField(); }
  togglePrimaryKey(): void { this.designer.togglePrimaryKey(); }
  designerTabName(): string { return this.designer.activeTabName; }
  addIndex(): void { this.designer.addIndex(); }
  deleteIndex(): void { this.designer.deleteIndex(); }
  addForeignKey(): void { this.designer.addForeignKey(); }
  deleteForeignKey(): void { this.designer.deleteForeignKey(); }
  addCheck(): void { this.designer.addCheck(); }
  deleteCheck(): void { this.designer.deleteCheck(); }
  addTrigger(): void { this.designer.addTrigger(); }
  deleteTrigger(): void { this.designer.deleteTrigger(); }
  isSavingStructure(): boolean { return this.designer.isSavingStructure(); }
  fieldMoveReason(direction: -1 | 1): string | null { return this.designer.fieldMoveReason(direction); }
  moveField(direction: -1 | 1): void { this.designer.moveField(direction); }
  async saveChanges(): Promise<void> { await this.designer.saveChanges(); }
  destroy(): void { this.designer.destroy(); }
}


