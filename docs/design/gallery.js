/** 设计画廊入口：按分组装配全部设计图 */

import { tokensScreen, objectsScreen, emptyScreen, queryScreen, tableScreen, logoSpecScreen } from "./screens-core.js";
import { navkitLogo } from "./assets/logo.js";
import { designerScreen, backupScreen, usersScreen, scheduleScreen, modelScreen, chartScreen } from "./screens-workspace.js";
import { menusGroup, statesGroup, contextMenusGroup } from "./screens-interactions.js";
import {
  connectionPickerScreen,
  connectionDialogScreen,
  importWizardScreen,
  exportWizardScreen,
  transferScreen,
  dumpScreen,
  backupDialogScreen,
  chartDialogScreen,
  scheduleDialogScreen,
  settingsScreen,
  aboutScreen,
  serverInfoScreen,
  favoritesScreen,
  userDialogScreen,
  historyScreen,
  confirmDialogScreen,
} from "./screens-dialogs.js";
import {
  driverManagerScreen,
  driverManualScreen,
  driverStatesScreen,
  customDriverScreen,
  driverRationaleScreen,
} from "./screens-driver.js";

const GROUPS = [
  {
    id: "system",
    title: "01 · 设计系统",
    desc: "色彩、字号、图标、组件与状态的全部定义。改代码前先对照这一屏，任何界面元素都必须能在这里找到出处。",
    screens: [{ id: "tokens", label: "令牌与组件", html: tokensScreen() }, { id: "logo", label: "Logo 规范", html: logoSpecScreen() }],
  },
  {
    id: "core",
    title: "02 · 主框架与核心页",
    desc: "从旧版多栏压缩为四条：标题栏（含菜单）/ 命令条 44px / 标签条 32px / 页工具栏 32px；图标统一为双色扁平族。",
    screens: [
      { id: "objects", label: "对象页", html: objectsScreen() },
      { id: "table", label: "表数据页", html: tableScreen() },
      { id: "query", label: "查询页", html: queryScreen() },
      { id: "empty", label: "空态", html: emptyScreen() },
    ],
  },
  {
    id: "workspace",
    title: "03 · 工作页面",
    desc: "设计器、备份、用户、调度、模型与图表。重点是把这些页面从「空网格占位」变成有明确工具栏分组和空态引导的工作区。",
    screens: [
      { id: "designer", label: "表设计器", html: designerScreen() },
      { id: "backup", label: "备份页", html: backupScreen() },
      { id: "users", label: "用户页", html: usersScreen() },
      { id: "schedule", label: "自动运行", html: scheduleScreen() },
      { id: "model", label: "模型", html: modelScreen() },
      { id: "chart", label: "图表", html: chartScreen() },
    ],
  },
  {
    id: "interactions",
    title: "04 · 菜单与交互",
    desc: "菜单栏下拉、右键菜单、下拉列表、悬停/选中/禁用矩阵。所有浮层统一 26px 项高、勾选列与快捷键列。",
    screens: [
      { id: "menus", label: "菜单栏展开", html: menusGroup() },
      { id: "context", label: "右键菜单", html: contextMenusGroup() },
      { id: "states", label: "状态矩阵", html: statesGroup() },
    ],
  },
  {
    id: "dialogs",
    title: "05 · 对话框",
    desc: "对话框骨架：标题 36px / 页脚 48px / 标签列 112px；向导统一步骤条；空值与错误提示统一组件。",
    screens: [
      { id: "picker", label: "新建连接（类型选择）", html: connectionPickerScreen() },
      { id: "conn", label: "连接属性", html: connectionDialogScreen() },
      { id: "import", label: "导入向导", html: importWizardScreen() },
      { id: "export", label: "导出向导", html: exportWizardScreen() },
      { id: "transfer", label: "数据传输", html: transferScreen() },
      { id: "dump", label: "SQL 转储", html: dumpScreen() },
      { id: "backupdlg", label: "新建备份", html: backupDialogScreen() },
      { id: "chartdlg", label: "创建图表", html: chartDialogScreen() },
      { id: "scheddlg", label: "新建任务", html: scheduleDialogScreen() },
      { id: "history", label: "运行历史", html: historyScreen() },
      { id: "settings", label: "选项", html: settingsScreen() },
      { id: "userdlg", label: "新建用户", html: userDialogScreen() },
      { id: "favorites", label: "管理收藏夹", html: favoritesScreen() },
      { id: "about", label: "关于", html: aboutScreen() },
      { id: "serverinfo", label: "服务器信息", html: serverInfoScreen() },
      { id: "confirm", label: "确认对话框", html: confirmDialogScreen() },
    ],
  },
  {
    id: "driver",
    title: "06 · 驱动管理（改版）",
    desc: "旧版按实现分页签、三步向导、状态文案混着来源与许可。新版把「我缺什么」和「这个怎么装」拆成左清单与右详情，自定义驱动压成单页表单。",
    screens: [
      { id: "drvmain", label: "驱动管理主界面", html: driverManagerScreen() },
      { id: "drvmanual", label: "需手动获取", html: driverManualScreen() },
      { id: "drvstates", label: "状态与文案矩阵", html: driverStatesScreen() },
      { id: "drvcustom", label: "自定义驱动单页", html: customDriverScreen() },
      { id: "drvwhy", label: "改版依据", html: driverRationaleScreen() },
    ],
  },
];

function render() {
  const nav = GROUPS.map((g, i) => `<button data-goto="${g.id}"${i === 0 ? ' class="active"' : ""}>${g.title}</button>`).join("");
  const main = GROUPS.map(
    (g) => `<section class="group" id="${g.id}">
      <div class="group-head"><h1>${g.title}</h1><p>${g.desc}</p></div>
      <div style="display:flex;flex-direction:column;gap:32px">${g.screens.map((s) => s.html).join("")}</div>
    </section>`,
  ).join("");

  document.getElementById("app").innerHTML = `<header class="gallery-top">
    <div class="gallery-brand"><span style="display:grid;place-items:center">${navkitLogo(22)}</span>NavKit 设计图 <small>v3 · 与实现同步</small></div>
    <nav class="gallery-nav">${nav}</nav>
    <div class="gallery-tools"><span>${GROUPS.reduce((n, g) => n + g.screens.length, 0)} 张设计图</span></div>
  </header>
  <main class="gallery-main">${main}</main>`;

  // 缩放控制
  const applyZoom = (z) => document.documentElement.style.setProperty("--frame-zoom", z);
  let zoom = 0.72;
  applyZoom(zoom);
  window.addEventListener("keydown", (e) => {
    if (e.key === "-" || e.key === "_") { zoom = Math.max(0.4, zoom - 0.06); applyZoom(zoom); }
    if (e.key === "+" || e.key === "=") { zoom = Math.min(1, zoom + 0.06); applyZoom(zoom); }
    if (e.key === "0") { zoom = 0.72; applyZoom(zoom); }
  });

  // 导航
  document.querySelectorAll(".gallery-nav button").forEach((b) => {
    b.addEventListener("click", () => {
      document.querySelectorAll(".gallery-nav button").forEach((x) => x.classList.remove("active"));
      b.classList.add("active");
      document.getElementById(b.dataset.goto)?.scrollIntoView({ behavior: "smooth", block: "start" });
    });
  });
}

render();

// 供截图工具调用：?export=1 全尺寸平铺
if (new URLSearchParams(location.search).has("export")) {
  document.body.classList.add("export");
}
