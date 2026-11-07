/**
 * NavKit 主菜单定义。
 *
 * 菜单栏标题与下拉项严格取自参考程序截图：
 *   文件 编辑 查看 收藏夹 工具 窗口 帮助
 * 另按参考程序行为，在表/查询标签页激活时插入「表」「查询」上下文菜单。
 * 只收录真实可用的命令 —— 不留任何点击无响应的菜单项。
 */

export interface MenuEntry {
  /** 命令标识；与 App 的命令表对应。 */
  id?: string;
  /** 显示文案（已含 Windows 加速键标记）。 */
  label?: string;
  /** 快捷键提示（显示在右侧）。 */
  shortcut?: string;
  /** 16px 图标名。 */
  icon?: string;
  /** 是否勾选。 */
  checked?: boolean;
  /** 是否禁用。 */
  disabled?: boolean;
  /** 是否为分隔线。 */
  separator?: boolean;
  /** 子菜单。 */
  children?: MenuEntry[];
}

export interface MenuBarEntry {
  label: string;
  children: MenuEntry[];
}

/** 主菜单栏（基础项，标签页上下文菜单由 App 动态插入）。 */
export const MENU_BAR: MenuBarEntry[] = [
  {
    label: "文件",
    children: [
      { label: "新建", children: [
        { id: "new-connection", label: "连接...", shortcut: "Ctrl+N" },
        { id: "new-query", label: "查询", shortcut: "Ctrl+Q" },
        { id: "tree-new-group", label: "组..." },
      ] },
      { id: "open-sql-file", label: "打开 SQL 文件..." },
      { separator: true },
      { id: "open-connection", label: "打开连接" },
      { id: "close-connection", label: "关闭连接" },
      { id: "connection-properties", label: "连接属性" },
      { id: "remove-connection", label: "移除连接" },
      { separator: true },
      { id: "import", label: "导入" },
      { id: "export", label: "导出" },
      { id: "dump-sql", label: "SQL 转储" },
      { id: "transfer", label: "数据传输" },
      { separator: true },
      { id: "save", label: "保存", shortcut: "Ctrl+S" },
      { id: "save-as", label: "另存为...", shortcut: "Ctrl+Shift+S" },
      { separator: true },
      { id: "print", label: "打印", shortcut: "Ctrl+P" },
      { separator: true },
      { id: "exit", label: "退出", shortcut: "Alt+F4" },
    ],
  },
  {
    label: "编辑",
    children: [
      { id: "undo", label: "撤消", shortcut: "Ctrl+Z" },
      { id: "redo", label: "重做", shortcut: "Ctrl+Y" },
      { separator: true },
      { id: "cut", label: "剪切", shortcut: "Ctrl+X" },
      { id: "copy", label: "复制", shortcut: "Ctrl+C" },
      { id: "paste", label: "粘贴", shortcut: "Ctrl+V" },
      { id: "delete-row", label: "删除", shortcut: "Delete" },
      { separator: true },
      { id: "find", label: "查找", shortcut: "Ctrl+F" },
      { id: "find-next", label: "查找下一个", shortcut: "F3" },
      { id: "replace", label: "替换", shortcut: "Ctrl+H" },
      { separator: true },
      { id: "select-all", label: "全选", shortcut: "Ctrl+A" },
    ],
  },
  {
    label: "查看",
    children: [
      { id: "toggle-sidebar", label: "侧边栏", checked: true },
      { id: "toggle-info-panel", label: "信息面板", checked: true },
      { id: "toggle-statusbar", label: "状态栏", checked: true },
      { id: "toggle-toolbar", label: "工具栏", checked: true },
      { separator: true },
      { id: "show-grid", label: "显示网格", checked: true },
      { id: "show-null", label: "跳过 Null", checked: false },
      { separator: true },
      { id: "refresh", label: "刷新", shortcut: "F5" },
      { id: "expand-all", label: "展开全部" },
      { id: "collapse-all", label: "折叠全部" },
      { separator: true },
      { id: "focus-mode", label: "专注模式", shortcut: "F11" },
    ],
  },
  {
    label: "收藏夹",
    children: [
      { id: "add-favorite", label: "添加到收藏夹", shortcut: "Ctrl+Shift+F" },
      { id: "open-favorite", label: "打开收藏的查询" },
      { id: "manage-favorites", label: "管理收藏夹" },
      { separator: true },
      { id: "recent", label: "最近打开的" },
    ],
  },
  {
    label: "工具",
    children: [
      { id: "data-sync", label: "数据同步..." },
      { id: "data-generation", label: "数据生成..." },
      { id: "driver-manager", label: "驱动管理..." },
      { separator: true },
      { id: "server-monitor", label: "数据库监控..." },
      { id: "history-log", label: "历史日志...", shortcut: "Ctrl+L" },
    ],
  },
];

/** 表标签页的上下文菜单栏项（参考程序在表页显示「表」菜单）。 */
export const TABLE_MENU: MenuBarEntry = {
  label: "表",
  children: [
    { id: "tree-open-table", label: "打开表" },
    { id: "tree-structure", label: "设计表" },
    { id: "new-table", label: "新建表" },
    { separator: true },
    { id: "delete-table", label: "删除表" },
    { id: "truncate-table", label: "截断表" },
    { separator: true },
    { id: "import", label: "导入向导..." },
    { id: "export", label: "导出向导..." },
    { id: "dump-sql", label: "转储 SQL 文件..." },
    { separator: true },
    { id: "refresh", label: "刷新", shortcut: "F5" },
  ],
};

/** 查询标签页的上下文菜单栏项（参考程序在查询页显示「查询」「格式」菜单）。 */
export const QUERY_MENU: MenuBarEntry = {
  label: "查询",
  children: [
    { id: "run", label: "运行", shortcut: "Ctrl+R" },
    { id: "run-selected", label: "运行已选择的", shortcut: "Ctrl+Shift+R" },
    { id: "explain", label: "解释已选择的" },
    { separator: true },
    { id: "beautify-sql", label: "美化 SQL", shortcut: "F6" },
    { id: "code-snippet", label: "代码段" },
  ],
};

/** 标签页右键菜单。 */
export const TAB_CONTEXT_MENU: MenuEntry[] = [
  { id: "tab-close", label: "关闭" },
  { id: "tab-close-others", label: "关闭其他" },
  { id: "tab-close-left", label: "关闭左侧窗口" },
  { id: "tab-close-right", label: "关闭右侧窗口" },
  { id: "tab-close-all", label: "关闭全部" },
  { separator: true },
  { id: "tab-refresh", label: "刷新" },
  { id: "tab-duplicate", label: "复制" },
];

/** 树节点右键菜单（按对象类型；全部项均已实现）。 */
export function treeContextMenu(type: string): MenuEntry[] {
  switch (type) {
    case "connection":
      return [
        { id: "tree-open", label: "打开连接" },
        { id: "tree-edit", label: "编辑连接..." },
        { id: "tree-duplicate", label: "复制连接..." },
        { separator: true },
        { id: "tree-new-query", label: "新建查询" },
        { id: "tree-server-info", label: "服务器信息" },
        { separator: true },
        { id: "tree-remove", label: "移除连接" },
        { id: "tree-close", label: "关闭连接" },
      ];
    case "database":
    case "schema":
      return [
        { id: "db-open", label: "打开数据库" },
        { id: "db-close", label: "关闭数据库" },
        { id: "tree-edit-db", label: "编辑数据库..." },
        { id: "db-new", label: "新建数据库..." },
        { id: "db-drop", label: "删除数据库" },
        { separator: true },
        { id: "tree-new-query", label: "新建查询" },
        { id: "tree-import", label: "导入向导..." },
        { id: "tree-export", label: "导出向导..." },
        { separator: true },
        { id: "tree-dump", label: "转储 SQL 文件..." },
        { id: "tree-model", label: "逆向数据库到模型..." },
        { id: "tree-server-info", label: "服务器信息" },
        { separator: true },
        { id: "tree-refresh", label: "刷新" },
      ];
    case "table":
      return [
        { id: "tree-open-table", label: "打开表" },
        { id: "tree-structure", label: "设计表" },
        { separator: true },
        { id: "delete-table", label: "删除表" },
        { id: "truncate-table", label: "截断表" },
        { separator: true },
        { id: "tree-import", label: "导入向导..." },
        { id: "tree-export", label: "导出向导..." },
        { separator: true },
        { id: "tree-dump", label: "转储 SQL 文件..." },
        { id: "tree-refresh", label: "刷新" },
      ];
    case "view":
      return [
        { id: "tree-open-table", label: "打开视图" },
        { id: "tree-design-view", label: "设计视图..." },
        { separator: true },
        { id: "tree-dump", label: "转储 SQL 文件..." },
        { id: "tree-refresh", label: "刷新" },
      ];
    case "procedure":
    case "function":
      return [
        { id: "tree-design-routine", label: type === "procedure" ? "设计存储过程..." : "设计函数..." },
        { separator: true },
        { id: "tree-refresh", label: "刷新" },
      ];
    case "folder":
      return [
        { id: "group-new-connection", label: "新建连接..." },
        { id: "tree-new-group", label: "新建组..." },
        { id: "group-rename", label: "重命名组..." },
        { separator: true },
        { id: "tree-refresh", label: "刷新" },
      ];
    default:
      return [
        { id: "tree-refresh", label: "刷新" },
      ];
  }
}

/** 结果网格右键菜单。 */
export const GRID_CONTEXT_MENU: MenuEntry[] = [
  { id: "grid-copy", label: "复制", shortcut: "Ctrl+C" },
  { id: "grid-copy-row", label: "复制行" },
  { id: "grid-copy-cell", label: "复制单元格" },
  { separator: true },
  { id: "grid-insert", label: "插入记录", shortcut: "Ctrl+I" },
  { id: "grid-delete", label: "删除记录", shortcut: "Ctrl+D" },
  { separator: true },
  { id: "grid-commit", label: "提交改动" },
  { id: "grid-rollback", label: "回滚改动" },
  { separator: true },
  { id: "grid-filter", label: "筛选..." },
  { id: "grid-sort", label: "排序..." },
  { separator: true },
  { id: "grid-refresh", label: "刷新", shortcut: "F5" },
];

/** 新建连接下拉（参考程序「连接▼」按钮；兼容协议数据库真实可用）。 */
/** 「连接▼」下拉：类型过多，选择全部数据源走「新建连接...」选择器；
 *  这里仅保留最常用类型的快捷入口。 */
export const NEW_CONNECTION_MENU: MenuEntry[] = [
  { id: "new-connection", label: "新建连接..." },
  { separator: true },
  { id: "new-connection-mysql", label: "MySQL..." },
  { id: "new-connection-postgresql", label: "PostgreSQL..." },
  { id: "new-connection-sqlserver", label: "SQL Server..." },
  { id: "new-connection-sqlite", label: "SQLite..." },
  { id: "new-connection-redis", label: "Redis..." },
  { id: "new-connection-mongodb", label: "MongoDB..." },
  { id: "new-connection-clickhouse", label: "ClickHouse..." },
];

/** Ribbon「其它▼」下拉（表空间 / 事件）。 */
export const OTHER_OBJECTS_MENU: MenuEntry[] = [
  { id: "open-other-tablespace", label: "表空间" },
  { id: "open-other-event", label: "事件" },
];

/** 连接树空白处右键菜单（新建连接 ▶ / 新建组 / 刷新）。 */
export const TREE_BLANK_MENU: MenuEntry[] = [
  { label: "新建连接", children: NEW_CONNECTION_MENU },
  { id: "tree-new-group", label: "新建组" },
  { id: "tree-refresh", label: "刷新" },
];

/** 「文本」显示格式下拉（文本 / 十六进制 / 图像 / 网页；勾选态由 App 按当前格式注入）。 */
export const TEXT_FORMAT_MENU: MenuEntry[] = [
  { id: "text-format-plain", label: "文本" },
  { id: "text-format-hex", label: "十六进制" },
  { id: "text-format-image", label: "图像" },
  { id: "text-format-web", label: "网页" },
];
