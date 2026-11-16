/** 交互状态图：菜单栏下拉、右键菜单、控件状态矩阵 */

import { icon } from "./assets/icons.js";
import { pictIcon } from "./assets/pict.js";
import { dbLogo } from "./assets/db-logos.js";
import {
  esc,
  button,
  check,
  chip,
  commandbar,
  cmdBtn,
  cmdSep,
  grid,
  infopanel,
  input,
  menu,
  note,
  pagetoolbar,
  screen,
  select,
  statusbar,
  tabstrip,
  titlebar,
  tree,
  defaultTree,
  toast,
} from "./assets/ui.js";

/* 一个用于承载浮层的画框 */
function frameWithOverlay(opts) {
  return `<div class="window">
    ${titlebar({ doc: "navkit — localhost", openMenu: opts.openMenu })}
    ${commandbar({ active: "表" })}
    <div class="workarea">
      <aside class="sidebar">${tree(defaultTree("navkit"))}</aside>
      <div class="splitter"></div>
      <main class="main">
        ${tabstrip([{ label: "对象", icon: "table" }, { label: "note_category @password_manager", icon: "table" }], 0)}
        ${opts.pagetoolbar || ""}
        <div class="content" style="position:relative">${opts.content || ""}</div>
      </main>
      <div class="splitter"></div>
      ${infopanel({ empty: true })}
    </div>
    ${statusbar({ left: [`<span class="sb-item">19 个表</span>`] })}
    ${opts.overlay || ""}
  </div>`;
}

/* ================================================================ 菜单栏 */

const MENU_FILE = [
  { label: "新建连接", key: "Ctrl+N", icon: "plus" },
  { label: "新建查询", key: "Ctrl+Q", icon: "query" },
  "-",
  { label: "打开连接", icon: "open" },
  { label: "关闭连接", icon: "close" },
  { label: "连接属性", icon: "settings" },
  { label: "移除连接", icon: "trash" },
  "-",
  { label: "导入", icon: "import" },
  { label: "导出", icon: "export" },
  { label: "SQL 转储", icon: "dump" },
  { label: "数据传输", icon: "transfer" },
  "-",
  { label: "保存", key: "Ctrl+S", icon: "save" },
  { label: "另存为...", key: "Ctrl+Shift+S" },
  "-",
  { label: "打印", key: "Ctrl+P" },
  "-",
  { label: "退出", key: "Alt+F4" },
];

const MENU_VIEW = [
  { label: "侧边栏", checked: true },
  { label: "信息面板", checked: true },
  { label: "状态栏", checked: true },
  { label: "工具栏", checked: true },
  "-",
  { label: "显示网格", checked: true },
  { label: "跳过 Null" },
  "-",
  { label: "刷新", key: "F5", icon: "refresh" },
  { label: "展开全部" },
  { label: "折叠全部" },
  "-",
  { label: "专注模式", key: "F11" },
];

const MENU_TABLE = [
  { label: "打开表", icon: "open" },
  { label: "设计表", icon: "edit" },
  { label: "新建表", icon: "plus" },
  "-",
  { label: "删除表", icon: "trash" },
  { label: "截断表" },
  "-",
  { label: "导入向导...", icon: "wizard" },
  { label: "导出向导...", icon: "wizard" },
  { label: "转储 SQL 文件...", icon: "dump" },
  "-",
  { label: "刷新", key: "F5", icon: "refresh" },
];

const MENU_QUERY = [
  { label: "运行", key: "Ctrl+R", icon: "play" },
  { label: "运行已选择的", key: "Ctrl+Shift+R", icon: "play" },
  { label: "解释已选择的", icon: "info" },
  "-",
  { label: "美化 SQL", key: "F6", icon: "brush" },
  { label: "代码段", icon: "code" },
];

const MENU_NEW_CONN = [
  { label: "新建连接...", icon: "plus" },
  "-",
  { label: "MySQL...", icon: "database" },
  { label: "PostgreSQL...", icon: "database" },
  { label: "SQL Server...", icon: "database" },
  { label: "SQLite...", icon: "database" },
  { label: "Redis...", icon: "database" },
  { label: "MongoDB...", icon: "database" },
  { label: "ClickHouse...", icon: "database" },
];

const MENU_TEXT_FORMAT = [
  { label: "文本", checked: true },
  { label: "十六进制" },
  { label: "图像" },
  { label: "网页" },
];

function menuScreen(title, subtitle, overlays, opts = {}) {
  const frame = `<div class="window">
    ${titlebar({ doc: "navkit — localhost", menu: opts.menu, openMenu: opts.openMenu, menuPopup: overlays })}
    ${commandbar({ active: "表" })}
    <div class="workarea">
      <aside class="sidebar">${tree(defaultTree("navkit"))}</aside>
      <div class="splitter"></div>
      <main class="main">
        ${tabstrip([{ label: "对象", icon: "table" }, { label: "note_category @password_manager", icon: "table" }], 0)}
        <div class="content" style="position:relative">${opts.content || ""}</div>
      </main>
      <div class="splitter"></div>
      ${infopanel({ empty: true })}
    </div>
    ${statusbar({ left: [`<span class="sb-item">19 个表</span>`] })}
  </div>`;
  return screen(title, subtitle, frame, { w: 1440, h: 900 });
}

export function menusGroup() {
  const blank = `<div style="flex:1;background:var(--bg-surface)"></div>`;
  return `<div style="display:flex;flex-direction:column;gap:32px">
    ${menuScreen(
      "菜单栏 · 文件",
      "菜单项 26px 高、圆角 4px；悬停整行浅蓝底 + 强调文字；快捷键 11px 右对齐",
      `${menu(MENU_FILE, { width: 236 })}`,
      { openMenu: "文件", content: blank },
    )}
    ${menuScreen(
      "菜单栏 · 查看",
      "勾选项在图标列显示对勾；视图开关与网格显示属于同一组状态",
      `${menu(MENU_VIEW, { width: 208 })}`,
      { openMenu: "查看", content: blank },
    )}
    ${menuScreen(
      "菜单栏 · 表（上下文菜单项）",
      "仅在当前标签为表 / 设计器 / 对象时插入「表」菜单，位置固定在「查看」之后",
      `${menu(MENU_TABLE, { width: 208 })}`,
      { content: blank, menu: ["文件", "编辑", "查看", "工具", "表", "设置"], openMenu: "表" },
    )}
  </div>`;
}

/* =============================================================== 右键菜单 */

const CTX_TREE_CONN = [
  { label: "打开连接", icon: "open" },
  { label: "编辑连接...", icon: "edit" },
  { label: "复制连接...", icon: "copy" },
  "-",
  { label: "新建查询", icon: "query" },
  { label: "服务器信息", icon: "info" },
  "-",
  { label: "移除连接", icon: "trash" },
  { label: "关闭连接", icon: "close" },
];

const CTX_TREE_DB = [
  { label: "打开数据库", icon: "open" },
  { label: "关闭数据库", icon: "close" },
  { label: "编辑数据库...", icon: "edit" },
  { label: "新建数据库...", icon: "plus" },
  { label: "删除数据库", icon: "trash" },
  "-",
  { label: "新建查询", icon: "query" },
  { label: "导入向导...", icon: "wizard" },
  { label: "导出向导...", icon: "wizard" },
  "-",
  { label: "转储 SQL 文件...", icon: "dump" },
  { label: "服务器信息", icon: "info" },
  "-",
  { label: "刷新", key: "F5", icon: "refresh" },
];

const CTX_GRID = [
  { label: "复制", key: "Ctrl+C", icon: "copy" },
  { label: "复制行", icon: "copy" },
  { label: "复制单元格", icon: "copy" },
  "-",
  { label: "插入记录", key: "Ctrl+I", icon: "plus" },
  { label: "删除记录", key: "Ctrl+D", icon: "trash" },
  "-",
  { label: "提交改动", icon: "check" },
  { label: "回滚改动", icon: "cross" },
  "-",
  { label: "筛选...", icon: "filter" },
  { label: "排序...", icon: "sort" },
  "-",
  { label: "刷新", key: "F5", icon: "refresh" },
];

const CTX_TAB = [
  { label: "关闭", icon: "cross" },
  { label: "关闭其他" },
  { label: "关闭全部" },
  "-",
  { label: "刷新", icon: "refresh" },
  { label: "复制", icon: "copy" },
];

const CTX_BLANK = [
  { label: "新建连接", icon: "plus", children: true },
  { label: "新建组", icon: "folder" },
  { label: "刷新", icon: "refresh" },
];

function ctxScreen(title, subtitle, overlay, contentHtml) {
  const frame = `<div class="window">
    ${titlebar({ doc: "navkit — localhost" })}
    ${commandbar({ active: "表" })}
    <div class="workarea">
      <aside class="sidebar">${tree(defaultTree("navkit"))}</aside>
      <div class="splitter"></div>
      <main class="main">
        ${tabstrip([{ label: "对象", icon: "table" }, { label: "note_category @password_manager", icon: "table" }], 0)}
        <div class="content" style="position:relative">${contentHtml}</div>
      </main>
      <div class="splitter"></div>
      ${infopanel({ empty: true })}
    </div>
    ${statusbar({ left: [`<span class="sb-item">19 个表</span>`] })}
    ${overlay}
  </div>`;
  return screen(title, subtitle, frame, { w: 1440, h: 900 });
}

export function contextMenusGroup() {
  const blank = `<div style="flex:1;background:var(--bg-surface)"></div>`;
  return `<div style="display:flex;flex-direction:column;gap:32px">
    ${ctxScreen(
      "右键菜单 · 连接节点",
      "连接右键包含连接级操作；危险项（移除连接）与连接操作之间用分隔线隔开",
      `${menu(CTX_TREE_CONN, { x: 160, y: 176, width: 210 })}`,
      blank,
    )}
    ${ctxScreen(
      "右键菜单 · 数据库节点",
      "数据库右键最长（14 项），按 打开/编辑 → 查询与导入导出 → 转储与模型 → 刷新 四段分组",
      `${menu(CTX_TREE_DB, { x: 160, y: 200, width: 232 })}`,
      blank,
    )}
    ${ctxScreen(
      "右键菜单 · 数据网格",
      "网格右键分为 复制 / 行编辑 / 提交回滚 / 筛选排序 / 刷新 五段，快捷键右对齐",
      `${menu(CTX_GRID, { x: 470, y: 250, width: 218 })}`,
      `<div style="flex:1;background:var(--bg-surface);padding:24px"><div style="color:var(--text-tertiary);font-size:12px">右键点击数据区域</div></div>`,
    )}
    ${ctxScreen(
      "右键菜单 · 标签 / 空白区",
      "标签右键关闭类操作；树空白区右键只有新建连接（含二级菜单）、新建组、刷新",
      `${menu(CTX_TAB, { x: 200, y: 150, width: 190 })}
       ${menu(CTX_BLANK, { x: 470, y: 210, width: 190 })}
       ${menu(MENU_NEW_CONN, { x: 664, y: 236, width: 200 })}`,
      blank,
    )}
  </div>`;
}

/* =============================================================== 状态矩阵 */

export function statesGroup() {
  const miniBar = (opts = {}) => `<div class="commandbar" style="height:44px;overflow:hidden;padding:0 4px">
    ${cmdBtn("连接", "connection", { dropdown: true, disabled: opts.noConn })}
    ${cmdBtn("新建查询", "query")}
    ${cmdSep()}
    ${cmdBtn("表", "table", { active: opts.active === "表" })}
    ${cmdBtn("视图", "view", { active: opts.active === "视图" })}
  </div>`;

  const cmdStates = `<div class="state-grid">
    <div class="state-card"><div class="state-name">默认</div>${miniBar()}</div>
    <div class="state-card"><div class="state-name">激活（表）</div>${miniBar({ active: "表" })}</div>
    <div class="state-card"><div class="state-name">无连接时禁用</div>${miniBar({ noConn: true })}</div>
    <div class="state-card"><div class="state-name">悬停（表）</div><div class="commandbar" style="height:44px;overflow:hidden;padding:0 4px">
      ${cmdBtn("连接", "connection", { dropdown: true })}
      ${cmdBtn("新建查询", "query")}
      <div class="cmd-sep"></div>
      <button class="cmd-btn" style="background:var(--bg-hover)">${icon("table", { size: 20 })}<span class="cmd-label" style="color:var(--text)">表</span></button>
      ${cmdBtn("视图", "view")}
    </div></div>
  </div>`;

  const treeStates = `<div class="state-grid">
    ${[
      ["默认 / 悬停 / 选中", `<div style="border:1px solid var(--border);border-radius:6px;overflow:hidden">
        ${treeRowSample("navkit_calendars", "")}
        ${treeRowSample("navkit_cron_triggers", "hover")}
        ${treeRowSample("navkit_job_details", "selected")}
      </div>`],
      ["连接状态", `<div style="border:1px solid var(--border);border-radius:6px;overflow:hidden">
        ${treeRowSample("localhost", "connected")}
        ${treeRowSample("dev-mysql", "connected")}
        ${treeRowSample("dev-sqlserver", "offline")}
      </div>`],
      ["分组与计数", `<div style="border:1px solid var(--border);border-radius:6px;overflow:hidden">
        ${treeRowSample("表", "group", "19")}
        ${treeRowSample("视图", "group", "3")}
        ${treeRowSample("备份", "group", "")}
      </div>`],
      ["展开 / 折叠", `<div style="border:1px solid var(--border);border-radius:6px;overflow:hidden">
        ${treeRowSample("已展开", "open")}
        ${treeRowSample("已折叠", "closed")}
      </div>`],
    ].map(([name, body]) => `<div class="state-card"><div class="state-name">${name}</div>${body}</div>`).join("")}
  </div>`;

  const btnStates = `<div class="state-grid">
    <div class="state-card"><div class="state-name">主按钮：默认 / 悬停 / 按下 / 禁用</div>
      <div style="display:flex;flex-wrap:wrap;gap:8px">${button("确定", { kind: "primary" })}${button("确定", { kind: "primary", pressed: true })}${button("确定", { disabled: true })}</div></div>
    <div class="state-card"><div class="state-name">次按钮</div>
      <div style="display:flex;flex-wrap:wrap;gap:8px">${button("取消")}${button("取消", { pressed: true })}${button("取消", { disabled: true })}</div></div>
    <div class="state-card"><div class="state-name">危险按钮</div>
      <div style="display:flex;flex-wrap:wrap;gap:8px">${button("删除", { kind: "danger" })}${button("删除", { kind: "danger", disabled: true })}</div></div>
    <div class="state-card"><div class="state-name">输入框：默认 / 焦点 / 禁用 / 错误</div>
      <div style="display:flex;flex-direction:column;gap:8px">
        ${input("localhost", { w: "lg" })}
        <input class="input w-lg" value="localhost" style="border-color:var(--accent-500);box-shadow:0 0 0 3px var(--accent-50)">
        <input class="input w-lg" value="localhost" style="background:var(--bg-subtle);color:var(--text-disabled);border-color:var(--border)">
        <input class="input w-lg" value="localhost" style="border-color:var(--danger-500);box-shadow:0 0 0 3px var(--danger-50)">
      </div></div>
  </div>`;

  const feedback = `<div class="state-grid">
    <div class="state-card"><div class="state-name">提示条</div>
      <div style="display:flex;flex-direction:column;gap:8px">${note("info", "已连接到 MySQL 8.0.36。")}${note("ok", "备份完成：1,024 行数据。")}${note("warn", "该数据源标记为「规划中」，暂不可用。")}${note("error", "连接失败：拒绝访问 (10061)。")}</div></div>
    <div class="state-card"><div class="state-name">Toast</div>
      <div style="display:flex;flex-direction:column;gap:8px">${toast("ok", "执行完成，用时 251 毫秒")}${toast("error", "导出失败：磁盘空间不足")}${toast("info", "已复制 4 行到剪贴板")}</div></div>
    <div class="state-card"><div class="state-name">加载中</div>
      <div style="display:flex;flex-direction:column;align-items:center;gap:10px;padding:20px 0">
        <div style="width:22px;height:22px;border:2px solid var(--accent-100);border-top-color:var(--accent-500);border-radius:50%"></div>
        <span style="font-size:12px;color:var(--text-tertiary)">正在加载对象列表…</span>
      </div></div>
    <div class="state-card"><div class="state-name">骨架屏（表格加载）</div>
      <div style="display:flex;flex-direction:column;gap:6px">
        ${[1, 2, 3, 4].map((i) => `<div style="height:10px;border-radius:3px;background:var(--n-150);width:${100 - i * 8}%"></div>`).join("")}
      </div></div>
  </div>`;

  return screen(
    "交互状态矩阵",
    "每个可交互元素都必须有不依赖颜色的状态差异；焦点环 2px 强调色 + 2px 偏移",
    `<div class="window" style="padding:24px;gap:20px;overflow:auto;background:var(--bg-app)">
      <div class="specimen"><div class="specimen-label">命令条</div>${cmdStates}</div>
      <div class="specimen"><div class="specimen-label">树行</div>${treeStates}</div>
      <div class="specimen"><div class="specimen-label">按钮与输入框</div>${btnStates}</div>
      <div class="specimen"><div class="specimen-label">反馈</div>${feedback}</div>
      <div class="specimen"><div class="specimen-label">焦点可见性</div>
        <div style="display:flex;gap:12px;align-items:center;flex-wrap:wrap">
          <button class="ptool" style="outline:2px solid var(--accent-500);outline-offset:2px">${icon("save", { size: 14 })}<span>保存</span></button>
          <div class="tab active" style="outline:2px solid var(--accent-500);outline-offset:-2px">${icon("table", { size: 14 })}<span class="tab-label">对象</span></div>
          <div class="tree-row selected" style="outline:2px solid var(--accent-500);outline-offset:-2px;width:180px;padding-left:8px"><span class="tree-arrow">${icon("chevron-down", { size: 12 })}</span><span class="tree-ico">${icon("database", { size: 15 })}</span><span class="tree-name">navkit</span></div>
          <input class="input" value="焦点在输入框" style="width:180px;border-color:var(--accent-500);box-shadow:0 0 0 3px var(--accent-50)">
        </div>
      </div>
    </div>`,
    { w: 1280, h: 1180 },
  );
}

function hoverCmd(label) {
  return `<div class="commandbar" style="height:44px">
    <button class="cmd-btn" style="background:var(--bg-hover)">${pictIcon("表", 24)}<span class="cmd-label" style="color:var(--text)">${label}</span></button>
  </div>`;
}

function treeRowSample(label, state, count = "") {
  const isGroup = state === "group";
  const isConn = state === "connected" || state === "offline";
  const ico =
    state === "connected"
      ? dbLogo("mysql", 16)
      : state === "offline"
        ? dbLogo("sqlserver", 16, { gray: true })
        : `<span class="tree-ico" style="color:var(--accent-500)">${icon(isGroup ? "table" : "table", { size: 15 })}</span>`;
  const cls = ["tree-row", state === "selected" ? "selected" : "", isGroup ? "strong" : ""].filter(Boolean).join(" ");
  const bg = state === "hover" ? "background:var(--bg-hover);" : "";
  const arrow =
    state === "open"
      ? `<span class="tree-arrow">${icon("chevron-down", { size: 12, stroke: 2 })}</span>`
      : state === "closed"
        ? `<span class="tree-arrow">${icon("chevron-right", { size: 12, stroke: 2 })}</span>`
        : `<span class="tree-arrow empty"></span>`;
  return `<div class="${cls}" style="${bg}padding-left:8px">${arrow}<span class="tree-ico">${ico}</span><span class="tree-name">${label}</span>${
    count ? `<span class="tree-badge">${count}</span>` : state === "connected" ? '<span class="tree-dot"></span>' : ""
  }</div>`;
}
