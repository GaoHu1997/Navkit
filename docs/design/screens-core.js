/** 设计系统标本 + 应用骨架屏 */

import { icon, ICON_COLORS } from "./assets/icons.js";
import { pictIcon, PICT_ICONS } from "./assets/pict.js";
import { dbLogo } from "./assets/db-logos.js";
import { navkitLogo, navkitLogoMono } from "./assets/logo.js";
import {
  esc,
  cmdBtn,
  cmdSep,
  commandbar,
  dialog,
  field,
  footer,
  grid,
  gridFooter,
  infopanel,
  input,
  menu,
  pagetoolbar,
  screen,
  select,
  statusbar,
  tabstrip,
  titlebar,
  tree,
  defaultTree,
  check,
  chip,
  button,
  note,
  toast,
  emptyState,
  highlightSql,
} from "./assets/ui.js";

/* ============================================================ 设计系统 */

export function tokensScreen() {
  const colors = [
    ["--bg-app", "#f7f8fa"],
    ["--bg-chrome", "#f7f8fa"],
    ["--bg-subtle", "#f7f8fa"],
    ["--bg-hover", "#f1f3f6"],
    ["--bg-selected", "#e3edfd"],
    ["--accent-500", "#3574f0"],
    ["--accent-600", "#2b62d9"],
    ["--border", "#e4e7ec"],
    ["--border-strong", "#d3d8e0"],
    ["--text", "#23272f"],
    ["--text-secondary", "#5c6673"],
    ["--text-tertiary", "#7d8794"],
    ["--ok-500", "#1f9254"],
    ["--warn-500", "#b54708"],
    ["--danger-500", "#d92d20"],
    ["--n-25", "#fbfcfd"],
  ];
  return screen(
    "设计令牌",
    "单一强调色 + 中性灰阶 + 五级字号 + 4px 栅格",
    `<div class="window" style="padding:24px;gap:20px;overflow:auto;background:var(--bg-app)">
      <div class="specimen">
        <div class="specimen-label">色彩</div>
        <div class="specimen-row">${colors
          .map(([n, v]) => `<div class="swatch"><i style="background:${v}"></i>${n}<span style="color:var(--text-tertiary)">${v}</span></div>`)
          .join("")}</div>
      </div>
      <div class="specimen">
        <div class="specimen-label">字号阶梯</div>
        <div style="display:flex;flex-direction:column;gap:10px">
          ${[
            ["--fs-18 · 18px", "对话框标题 / 空态标题", "18px", "600"],
            ["--fs-15 · 15px", "信息面板对象名 / 区块标题", "15px", "600"],
            ["--fs-13 · 13px", "正文：树 / 表单 / 按钮 / 菜单", "13px", "400"],
            ["--fs-12 · 12px", "表头 / 工具栏 / 信息字段", "12px", "400"],
            ["--fs-11 · 11px", "状态栏 / 角标 / 快捷键", "11px", "400"],
          ]
            .map(
              ([t, u, s, w]) =>
                `<div style="display:flex;align-items:baseline;gap:16px"><code style="width:120px;font-size:11px;color:var(--text-tertiary);font-family:var(--font-mono)">${t}</code><span style="font-size:${s};font-weight:${w}">NavKit 数据库管理</span><span style="font-size:11px;color:var(--text-tertiary)">${u}</span></div>`,
            )
            .join("")}
        </div>
      </div>
      <div class="specimen">
        <div class="specimen-label">命令条具象图标 · 24px 填充色块（原创构成，每个 2–3 色）</div>
        <div style="display:flex;gap:12px;flex-wrap:wrap">
          ${[["连接","连接"],["新建查询","新建查询"],["表","表"],["视图","视图"],["函数","函数"],["用户","用户"],["其它","其它"],["查询工具","查询"],["备份","备份"],["自动运行","自动任务"],["图表","图表"],["模型","关系图"]]
            .map(
              ([k, l]) =>
                `<div style="display:flex;flex-direction:column;align-items:center;gap:5px;width:64px">${pictIcon(k, 26)}<span style="font-size:10px;color:var(--text-tertiary)">${l}</span></div>`,
            )
            .join("")}
        </div>
        <div style="margin-top:10px;font-size:11px;color:var(--text-tertiary)">构成：链接节点+加号 / 文档+加号 / 数据表 / 视窗 / 函数符 / 人物 / 对象矩阵 / 终端 / 数据圆柱+上传 / 闹钟 / ER 节点 / 柱状图 —— 禁止描摹商业软件图标。</div>
      </div>
      <div class="specimen">
        <div class="specimen-label">功能图标族 · 24 网格 / 1.75 描边 / 按语义着色</div>
        <div style="display:grid;grid-template-columns:repeat(12,1fr);gap:12px">
          ${(
            [
              "connection","query","table","view","function","user","more","backup","schedule","model","chart","search",
              "folder","folder-open","database","schema","key","index","link","trash","plus","minus","check","cross",
              "refresh","save","play","stop","settings","info","warning","error","copy","filter","sort","columns",
              "import","export","code","brush","grid","list","details","panel-left","panel-right","star","clock","history",
              "lock","eye","eye-off","page-first","page-prev","page-next","page-last","transfer","dump","wizard","edit","open",
            ]
          )
            .map(
              (n) =>
                `<div style="display:flex;flex-direction:column;align-items:center;gap:4px;color:${ICON_COLORS[n] || "var(--text-secondary)"}">${icon(n, { size: 22 })}<span style="font-size:10px;color:var(--text-tertiary);font-family:var(--font-mono)">${n}</span></div>`,
            )
            .join("")}
        </div>
      </div>
      <div class="specimen">
        <div class="specimen-label">数据源品牌 logo · 官方素材（连接树 / 类型选择器用）</div>
        <div style="display:flex;gap:14px;flex-wrap:wrap;align-items:flex-end">
          ${["mysql","postgresql","sqlserver","oracle","mariadb","sqlite","redis","mongodb","clickhouse","snowflake","bigquery","h2","druid","odbc"]
            .map(
              (k) =>
                `<div style="display:flex;flex-direction:column;align-items:center;gap:4px">${dbLogo(k, 24)}<span style="font-size:10px;color:var(--text-tertiary);font-family:var(--font-mono)">${k}</span></div>`,
            )
            .join("")}
          <div style="display:flex;flex-direction:column;align-items:center;gap:4px"><span style="color:var(--text-tertiary)">${icon("database", { size: 24 })}</span><span style="font-size:10px;color:var(--text-tertiary);font-family:var(--font-mono)">odbc(无品牌)</span></div>
        </div>
        <div style="margin-top:10px;display:flex;gap:14px;align-items:center">
          <span style="font-size:11px;color:var(--text-tertiary)">未连接 / 规划中：</span>
          ${dbLogo("sqlserver", 20, { gray: true })}${dbLogo("mysql", 20, { gray: true })}
          <span style="font-size:11px;color:var(--text-tertiary);margin-left:16px">注意：odbc 为通用连接类型，按产品规则不使用品牌图形。</span>
        </div>
      </div>
      <div class="triple">
        <div class="specimen">
          <div class="specimen-label">按钮</div>
          <div class="specimen-row">${button("确定", { kind: "primary" })}${button("取消")}${button("删除", { kind: "danger" })}</div>
          <div class="specimen-row">${button("禁用", { disabled: true })}${button("小号", { size: "sm" })}${button("文字按钮", { kind: "ghost" })}</div>
        </div>
        <div class="specimen">
          <div class="specimen-label">选择控件</div>
          <div class="specimen-row">${check("已勾选", true)}${check("未勾选")}${check("禁用", false, { disabled: true })}</div>
          <div class="specimen-row">${select("MySQL", { width: 140 })}${input("dbswitch", { w: "w-md" })}</div>
        </div>
        <div class="specimen">
          <div class="specimen-label">徽标与提示</div>
          <div class="specimen-row">${chip("默认")}${chip("已连接", "ok")}${chip("规划中", "warn")}${chip("12 表", "accent")}</div>
          <div style="display:flex;flex-direction:column;gap:8px">${note("info", "连接成功后自动展开数据库列表。")}${note("ok", "备份完成：1,024 行数据。")}</div>
        </div>
      </div>
      <div class="triple">
        <div class="specimen">
          <div class="specimen-label">表格行状态</div>
          <div class="grid-wrap" style="border:1px solid var(--border);border-radius:6px;overflow:hidden">
            <table class="grid">
              <thead><tr><th>状态</th><th>说明</th></tr></thead>
              <tbody>
                <tr><td>默认行</td><td class="cell-muted">白 / 斑马 #FBFCFD</td></tr>
                <tr><td>悬停行</td><td class="cell-muted" style="background:var(--bg-hover)">--bg-hover</td></tr>
                <tr class="selected"><td>选中行</td><td>--bg-selected + 强调文字</td></tr>
                <tr class="row-editing"><td>编辑中</td><td class="cell-muted">--accent-50</td></tr>
              </tbody>
            </table>
          </div>
        </div>
        <div class="specimen">
          <div class="specimen-label">反馈</div>
          <div style="display:flex;flex-direction:column;gap:8px">
            ${toast("ok", "执行完成，用时 251 毫秒")}
            ${toast("error", "连接失败：拒绝访问")}
          </div>
        </div>
        <div class="specimen">
          <div class="specimen-label">菜单</div>
          <div style="position:relative;height:212px">${menu([{ label: "打开表", icon: "table" }, { label: "设计表", icon: "edit" }, "-", { label: "复制", key: "Ctrl+C" }, { label: "已勾选项", checked: true }, { label: "禁用项", disabled: true }], { x: 0, y: 0, width: 190 })}</div>
        </div>
      </div>
    </div>`,
    { w: 1440, h: 1020 },
  );
}

/* ============================================================ 骨架：对象页 */

function shell(opts) {
  return `<div class="window">
    ${titlebar({ doc: opts.doc, menu: opts.menu, openMenu: opts.openMenu })}
    ${commandbar({ active: opts.active, noConn: opts.noConn })}
    <div class="workarea">
      <aside class="sidebar">${tree(opts.treeNodes)}</aside>
      <div class="splitter"></div>
      <main class="main">
        ${tabstrip(opts.tabs, opts.activeTab ?? 0)}
        ${opts.pagetoolbar || ""}
        ${opts.content}
      </main>
      <div class="splitter"></div>
      ${infopanel(opts.info)}
    </div>
    ${statusbar({ left: opts.statusLeft })}
    ${opts.overlay || ""}
  </div>`;
}

export function objectsScreen() {
  const cols = [
    { label: "名称", key: "name", width: 232 },
    { label: "行数", key: "rows", width: 78, align: "right", sort: "desc" },
    { label: "引擎", key: "engine", width: 92 },
    { label: "排序规则", key: "coll", width: 168 },
    { label: "修改日期", key: "date", width: 148 },
    { label: "注释", key: "note" },
  ];
  const nameCell = (n) =>
    ({ html: `<span class="cell-name"><span class="cell-ico">${icon("table", { size: 14 })}</span>${n}</span>` });
  const rows = [
    ["dbswitch_account", "0", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-15 14:57:08", "账号表"],
    ["dbswitch_assignment_config", "1", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-15 14:57:08", "任务配置表"],
    ["dbswitch_calendars", "0", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-12 09:14:22", "日历表"],
    ["dbswitch_cron_triggers", "0", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-15 14:57:08", "CRON 触发器"],
    ["dbswitch_job_details", "3", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-15 14:57:08", "作业明细"],
    ["dbswitch_job_logback", "0", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-15 14:57:08", "作业日志回写"],
    ["dbswitch_locks", "0", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-12 09:14:22", "锁表"],
    ["dbswitch_paused_trigger_grps", "0", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-15 14:57:08", "暂停触发器组"],
    ["dbswitch_schema_history", "5", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-12 09:14:22", "Schema 变更历史"],
    ["dbswitch_simple_triggers", "0", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-15 14:57:08", "简单触发器"],
    ["dbswitch_system_log", "2", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-15 14:57:08", "系统日志"],
    ["dbswitch_system_user", "1", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-15 14:57:08", "系统用户表"],
    ["dbswitch_triggers", "0", "InnoDB", "utf8mb4_0900_ai_ci", "2026-05-15 14:57:08", "触发器"],
  ].map((r) => ({
    name: nameCell(r[0]),
    rows: r[1],
    engine: r[2],
    coll: r[3],
    date: r[4],
    note: r[5],
  }));

  const content = `<div class="content" style="flex-direction:column">
      <div class="grid-topbar">${gridToolbar()}</div>
      ${grid(cols, rows, { selectedRow: 2 })}
      ${gridFooter({ left: `<span>19 个表</span><span class="sb-sep"></span><span>已选中 1 行</span>` })}
    </div>`;

  return screen(
    "对象页 · 主视图",
    "四栏结构：标题栏 36 / 命令条 44 / 标签条 32 / 页工具栏 32；网格斑马底 26px 行高",
    shell({
      doc: "dbswitch — localhost",
      active: "表",
      treeNodes: defaultTree("dbswitch"),
      tabs: [
        { label: "对象", icon: "table" },
        { label: "dbswitch_calendars @dbswitch", icon: "table" },
        { label: "无标题 — 查询", icon: "code" },
      ],
      activeTab: 0,
      pagetoolbar: pagetoolbar([
        { label: "打开表", icon: "open" },
        { label: "设计表", icon: "edit" },
        "-",
        { label: "新建表", icon: "plus" },
        { label: "删除表", icon: "trash", disabled: true },
        "-",
        { label: "导入", icon: "import" },
        { label: "导出", icon: "export" },
        "|",
        { label: "筛选", icon: "filter", disabled: true },
        { label: "排序", icon: "sort", disabled: true },
      ]),
      content,
      info: {
        icon: "database",
        title: "dbswitch",
        subtitle: "数据库",
        refs: [{ logo: "mysql", text: "localhost · MySQL 8.0.36" }],
        groups: [
          { title: "概要", rows: [["表", "19"], ["视图", "3"], ["函数", "2"], ["行总数", "1,284"]] },
          { title: "属性", rows: [["字符集", "utf8mb4"], ["排序规则", "utf8mb4_0900_ai_ci"], ["大小", "12.6 MB"], ["创建日期", "2026-05-02 11:20:41"]] },
        ],
      },
      statusLeft: [
        `<span class="sb-item"><span class="sb-strong">19 个表</span></span>`,
        '<span class="sb-sep"></span>',
        `<span class="sb-item">${icon("connection", { size: 12 })}localhost</span>`,
        `<span class="sb-item">${icon("database", { size: 12 })}dbswitch</span>`,
      ],
    }),
    { w: 1440, h: 900 },
  );
}

function gridToolbar() {
  return `<div style="display:flex;align-items:center;gap:8px;height:32px;padding:0 12px;border-bottom:1px solid var(--border-subtle)">
    <span style="font-size:12px;font-weight:600;color:var(--text)">对象</span>
    <span style="font-size:11px;color:var(--text-tertiary)">19 个表 · 3 个视图</span>
    <div style="flex:1"></div>
    <div class="search" style="margin:0;width:200px;height:24px">${icon("search", { size: 13 })}<input placeholder="筛选对象" readonly></div>
  </div>`;
}

/* ============================================================ 骨架：空态 */

export function emptyScreen() {
  return screen(
    "对象页 · 空态",
    "无连接、无标签时的引导：主区中央给出下一步动作，而不是一句干瘪提示",
    shell({
      doc: "未连接",
      noConn: true,
      treeNodes: [
        { label: "本地连接", icon: "folder-open", open: true, depth: 0, strong: true },
        { label: "暂无连接", icon: "connection", depth: 0 },
      ],
      tabs: [],
      content: emptyState({
        icon: "connection",
        title: "还没有打开的连接",
        text: `新建一个连接开始工作。NavKit 支持 102 种数据源，包括 MySQL、PostgreSQL、SQL Server 与国产数据库。`,
        actions: `${button("新建连接", { kind: "primary", icon: "plus" })}${button("打开现有连接", { icon: "open" })}`,
      }),
      info: { empty: true },
      statusLeft: [`<span class="sb-item">就绪</span>`],
    }),
    { w: 1440, h: 900 },
  );
}

/* ============================================================ 查询页 */

export function queryScreen() {
  const cols = [
    { label: "id", key: "id", width: 64, align: "right" },
    { label: "created_at", key: "created", width: 168 },
    { label: "name", key: "name", width: 180 },
    { label: "quick_link_id", key: "qid", width: 116, align: "right" },
    { label: "sort_order", key: "sort", width: 100, align: "right" },
    { label: "updated_at", key: "updated", width: 160 },
    { label: "user_id", key: "uid", width: 90, align: "right" },
  ];
  const rows = [
    { id: "1", created: "2026-05-12 09:14:22", name: "默认", qid: "705", sort: "3", updated: "2026-05-12 09:14:22", uid: "1" },
    { id: "2", created: "2026-05-12 09:15:03", name: "代码", qid: "705", sort: "1", updated: "2026-05-12 09:15:03", uid: "1" },
    { id: "3", created: "2026-05-12 09:16:41", name: "教程", qid: "705", sort: "0", updated: "2026-05-12 09:16:41", uid: "1" },
    { id: "33", created: "2026-05-12 10:02:11", name: "测试", qid: "705", sort: "2", updated: "2026-05-12 10:02:11", uid: "1" },
  ];

  const sql = `SELECT * FROM note_category`;

  const content = `<div class="content">
    <div class="querybar">
      <div class="select" style="width:170px;height:26px">${icon("connection", { size: 13 })}<span>localhost</span>${icon("chevron-down", { size: 12 })}</div>
      <div class="select" style="width:180px;height:26px">${icon("database", { size: 13 })}<span>password_manager</span>${icon("chevron-down", { size: 12 })}</div>
      <div class="ptool-sep"></div>
      <button class="ptool primary" style="height:26px">${icon("play", { size: 13 })}<span>运行</span><span class="caret">${icon("chevron-down", { size: 11, stroke: 2 })}</span></button>
      <button class="ptool" style="height:26px">${icon("stop", { size: 13 })}<span>停止</span></button>
      <button class="ptool" style="height:26px">${icon("info", { size: 13 })}<span>解释</span></button>
      <div class="ptoolbar-spacer"></div>
      <span class="chip ok">已连接</span>
    </div>
    <div class="editor">
      <div class="gutter"><span>1</span><span>2</span></div>
      <div class="codearea"><div class="ln">${highlightSql(sql)}</div><div class="ln">${highlightSql("-- 查询笔记分类，按排序号显示")}</div></div>
    </div>
    <div class="results">
      <div class="resulttabs">
        <div class="resulttab">消息</div>
        <div class="resulttab">摘要</div>
        <div class="resulttab active">结果 1</div>
        <div class="resulttab">剖析</div>
        <div class="resulttab">状态</div>
        <div class="ptoolbar-spacer"></div>
        <button class="iconbtn" title="导出结果">${icon("export", { size: 13 })}</button>
        <button class="iconbtn" title="创建图表">${icon("chart", { size: 13 })}</button>
        <button class="iconbtn" title="更多">${icon("more", { size: 13 })}</button>
      </div>
      ${grid(cols, rows, { selectedRow: 0 })}
      ${gridFooter({ left: `<span>4 行 · 用时 251 ms</span>`, count: "共 4 行" })}
    </div>
  </div>`;

  return screen(
    "查询页",
    "编辑器 / 结果区 2:1 分割；连接与库选择器合并为 capsule；结果页签用底部指示条取代白底",
    shell({
      doc: "无标题 — 查询",
      active: "查询工具栏",
      treeNodes: defaultTree("password_manager"),
      tabs: [
        { label: "对象", icon: "table" },
        { label: "无标题 — 查询", icon: "code", dirty: true },
        { label: "dbswitch_calendars @dbswitch", icon: "table" },
      ],
      activeTab: 1,
      pagetoolbar: pagetoolbar([
        { label: "保存", icon: "save" },
        "-",
        { label: "美化 SQL", icon: "brush" },
        { label: "代码段", icon: "code" },
        { label: "文本", icon: "code", dropdown: true },
        "-",
        { label: "导出结果", icon: "export" },
        { label: "创建图表", icon: "chart", disabled: true },
        "|",
        { label: "查询创建工具", icon: "wizard", disabled: true },
      ]),
      content,
      info: {
        icon: "code",
        title: "代码段",
        subtitle: "全部标签",
        raw: `<div style="display:flex;flex-direction:column;gap:10px">
          ${[
            ["CASE", "流程控制", "创建条件分支结构"],
            ["INSERT Syntax", "DML", "向已有表插入新行"],
            ["SELECT Syntax", "DML", "从一个或多个表中检索行"],
            ["WHILE", "流程控制", "当条件为真时重复执行语句列表"],
            ["Runtime Parameter", "运行参数", "执行时弹出对话框要求输入"],
          ]
            .map(
              ([n, tag, d]) => `<div style="padding:8px 10px;border:1px solid var(--border-subtle);border-radius:6px">
              <div style="display:flex;align-items:center;gap:6px">
                <span style="font-size:12px;font-weight:600;color:var(--text)">${n}</span>
                ${chip(tag, tag === "DML" ? "ok" : tag === "运行参数" ? "warn" : "accent")}
              </div>
              <div style="margin-top:3px;font-size:11px;line-height:1.5;color:var(--text-tertiary)">${d}</div>
            </div>`,
            )
            .join("")}
        </div>`,
      },
      statusLeft: [
        `<span class="sb-mono">SELECT * FROM note_category</span>`,
        '<span class="sb-sep"></span>',
        `<span class="sb-item">运行时间 <span class="sb-strong">251 ms</span></span>`,
        `<span class="sb-item">第 1 条记录（共 4 条）</span>`,
      ],
    }),
    { w: 1440, h: 900 },
  );
}

/* ============================================================ 表数据页 */

export function tableScreen() {
  const cols = [
    { label: "#", key: "n", width: 36, align: "right" },
    { label: "id", key: "id", width: 52, align: "right" },
    { label: "name", key: "name", width: 108 },
    { label: "category", key: "cat", width: 92 },
    { label: "created_at", key: "created", width: 144, sort: "desc" },
    { label: "description", key: "desc", width: 150 },
    { label: "icon", key: "icon", width: 62 },
    { label: "updated_at", key: "updated", width: 150 },
    { label: "user_id", key: "uid", width: 62, align: "right" },
  ];
  const rows = [
    { n: "1", id: "1", name: "默认", cat: "系统", created: "2026-05-12 09:14:22", desc: { html: '<span class="cell-null">NULL</span>' }, icon: { html: '<span class="cell-null">NULL</span>' }, updated: "2026-05-12 09:14:22", uid: "1" },
    { n: "2", id: "2", name: "代码", cat: "开发", created: "2026-05-12 09:15:03", desc: "代码片段分类", icon: { html: `<span class="cell-ico" style="color:var(--text-tertiary)">${icon("code", { size: 13 })}</span>` }, updated: "2026-05-12 09:15:03", uid: "1" },
    { n: "3", id: "3", name: "教程", cat: "文档", created: "2026-05-12 09:16:41", desc: { html: '<span class="cell-null">NULL</span>' }, icon: { html: '<span class="cell-null">NULL</span>' }, updated: "2026-05-12 09:16:41", uid: "1" },
    { n: "4", id: "33", name: "测试", cat: { html: '<span style="display:inline-flex;align-items:center;gap:4px;padding:1px 6px;border-radius:999px;background:var(--accent-50);color:var(--accent-600);font-size:11px">测试</span>' }, created: "2026-05-12 10:02:11", desc: "自动化测试数据", icon: { html: `<span class="cell-ico" style="color:var(--text-tertiary)">${icon("wizard", { size: 13 })}</span>` }, updated: "2026-05-12 10:02:11", uid: "1" },
    { n: "5", id: "34", name: "归档", cat: "系统", created: "2026-05-13 08:22:04", desc: { html: '<span class="cell-null">NULL</span>' }, icon: { html: '<span class="cell-null">NULL</span>' }, updated: "2026-05-13 08:22:04", uid: "2" },
    { n: "6", id: "35", name: "草稿", cat: "开发", created: "2026-05-13 11:41:57", desc: "未发布的笔记", icon: { html: `<span class="cell-ico" style="color:var(--text-tertiary)">${icon("edit", { size: 13 })}</span>` }, updated: "2026-05-14 02:10:33", uid: "2" },
    { n: "7", id: "36", name: "分享", cat: "文档", created: "2026-05-14 07:03:19", desc: { html: '<span class="cell-null">NULL</span>' }, icon: { html: '<span class="cell-null">NULL</span>' }, updated: "2026-05-14 07:03:19", uid: "2" },
  ];

  const content = `<div class="content">
    ${grid(cols, rows, { selectedRow: 1, pkKey: "id" })}
    <div class="grid-footer">
      <button class="iconbtn" title="新增行">${icon("plus", { size: 13 })}</button>
      <button class="iconbtn" title="删除行">${icon("minus", { size: 13 })}</button>
      <span class="sb-sep"></span>
      <button class="iconbtn" title="提交改动" style="color:var(--ok-500)">${icon("check", { size: 13 })}</button>
      <button class="iconbtn" title="回滚改动" style="color:var(--danger-500)">${icon("cross", { size: 13 })}</button>
      <span class="sb-sep"></span>
      <span>已修改 1 行 · 未提交</span>
      <div class="spacer"></div>
      <div class="pagebox">
        <button class="iconbtn">${icon("page-first", { size: 12 })}</button>
        <button class="iconbtn">${icon("page-prev", { size: 12 })}</button>
        <input class="pageno" value="1" readonly>
        <button class="iconbtn">${icon("page-next", { size: 12 })}</button>
        <button class="iconbtn">${icon("page-last", { size: 12 })}</button>
      </div>
      <span>共 15 行</span>
      <button class="iconbtn">${icon("refresh", { size: 13 })}</button>
      <button class="iconbtn">${icon("more", { size: 13 })}</button>
    </div>
  </div>`;

  return screen(
    "表数据页",
    "主键列强调色；NULL 降级斜体；行编辑状态用底部工具条集中提交/回滚",
    shell({
      doc: "note_category — password_manager",
      active: "表",
      treeNodes: defaultTree("dbswitch"),
      tabs: [
        { label: "对象", icon: "table" },
        { label: "note_category @password_manager", icon: "table" },
        { label: "无标题 — 查询", icon: "code" },
      ],
      activeTab: 1,
      pagetoolbar: pagetoolbar([
        { label: "文本", icon: "code", dropdown: true },
        "-",
        { label: "筛选", icon: "filter" },
        { label: "排序", icon: "sort" },
        { label: "列", icon: "columns" },
        "-",
        { label: "导入", icon: "import" },
        { label: "导出", icon: "export" },
        "-",
        { label: "数据生成", icon: "wizard", disabled: true },
        { label: "创建图表", icon: "chart", disabled: true },
        "|",
        { label: "开始事务", icon: "history", disabled: true },
      ]),
      content,
      info: {
        icon: "table",
        title: "note_category",
        subtitle: "表",
        refs: [{ logo: "mysql", text: "localhost / password_manager" }],
        groups: [
          { title: "统计", rows: [["行", "15"], ["字段数", "9"], ["索引", "2"], ["外键", "0"]] },
          { title: "属性", rows: [["引擎", "InnoDB"], ["排序规则", "utf8mb4_0900_ai_ci"], ["自动递增", "105"], ["数据长度", "368.0 KB"], ["修改日期", "2026-05-15 14:57:08"], ["注释", "笔记分类"]] },
        ],
      },
      statusLeft: [
        `<span class="sb-item"><span class="sb-strong">15 行</span></span>`,
        '<span class="sb-sep"></span>',
        `<span class="sb-mono">SELECT * FROM note_category LIMIT 1000</span>`,
      ],
    }),
    { w: 1440, h: 900 },
  );
}

/* ============================================================ Logo 规范 */

export function logoSpecScreen() {
  // 三色取自品牌色卡：Pastel Rainbow · 冰青 / Celestial Sea · 青瓷、雾蓝
  const LOGO_LAYERS = [
    { cy: 21.9, top: "#9db1d5", bot: "#778eba", stroke: "#677ca6" },
    { cy: 16.0, top: "#cae6dd", bot: "#a9c9c3", stroke: "#91aead" },
    { cy: 10.1, top: "#baf8fe", bot: "#97dce7", stroke: "#82becc" },
  ];
  const gLayers = LOGO_LAYERS.map(
    (l, i) =>
      `<path d="M16 ${l.cy - 6.05}L28.8 ${l.cy}L16 ${l.cy + 6.05}L3.2 ${l.cy}Z" fill="url(#lgGrad${i})" stroke="${l.stroke}" stroke-width="0.9" stroke-linejoin="round" filter="url(#lgShadow)"/>`,
  ).join("");
  const gDefs = LOGO_LAYERS.map((l, i) => {
    return `<linearGradient id="lgGrad${i}" x1="16" y1="${l.cy - 6.05}" x2="16" y2="${l.cy + 6.05}" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="${l.top}"/><stop offset="1" stop-color="${l.bot}"/></linearGradient>`;
  }).join("");
  const grid16 = `<svg width="192" height="192" viewBox="0 0 32 32" style="background:var(--bg-sunken);border-radius:8px">
    <defs>${gDefs}<filter id="lgShadow" x="-30%" y="-30%" width="160%" height="175%" color-interpolation-filters="sRGB"><feDropShadow dx="0" dy="0.9" stdDeviation="0.9" flood-color="#2a3759" flood-opacity="0.22"/></filter></defs>
    ${Array.from({ length: 31 }, (_, i) => `<line x1="${i + 1}" y1="0" x2="${i + 1}" y2="32" stroke="var(--n-200)" stroke-width="0.03"/>`).join("")}
    ${Array.from({ length: 31 }, (_, i) => `<line x1="0" y1="${i + 1}" x2="32" y2="${i + 1}" stroke="var(--n-200)" stroke-width="0.03"/>`).join("")}
    ${gLayers}
  </svg>`;

  return screen(
    "Logo 规范 · 等距数据层",
    "抽象图形（无字母）：三层等距菱形透明悬浮堆叠（冰青顶 / 青瓷中 / 雾蓝底），无背景容器，每层带同族加深细描边保证任意底色上的轮廓",
    `<div class="window" style="padding:24px;gap:20px;overflow:auto;background:var(--bg-app)">
      <div class="triple">
        <div class="specimen" style="text-align:center">
          <div class="specimen-label">主版</div>
          <div style="display:flex;align-items:flex-end;justify-content:center;gap:16px;padding:12px 0">
            ${navkitLogo(56)}${navkitLogo(32)}${navkitLogo(24)}${navkitLogo(20)}${navkitLogo(16)}
          </div>
          <div style="font-size:11px;color:var(--text-tertiary)">56 / 32 / 24 / 20 / 16 px</div>
        </div>
        <div class="specimen" style="text-align:center">
          <div class="specimen-label">单色版（水印 / 深色底 / 低对比场景）</div>
          <div style="display:flex;align-items:center;justify-content:center;gap:16px;padding:12px 0">
            <span style="color:var(--text)">${navkitLogoMono(40)}</span>
            <span style="color:var(--text-secondary)">${navkitLogoMono(28)}</span>
            <span style="color:#fff;background:var(--accent-500);padding:10px;border-radius:8px">${navkitLogoMono(32)}</span>
            <span style="color:var(--n-300)">${navkitLogoMono(40)}</span>
          </div>
          <div style="font-size:11px;color:var(--text-tertiary)">currentColor 跟随文字颜色</div>
        </div>
        <div class="specimen" style="text-align:center">
          <div class="specimen-label">网格构造 · 32 网格 / 三层等距层（透明无容器）</div>
          <div style="display:flex;justify-content:center;padding:12px 0">${grid16}</div>
        </div>
      </div>
      <div style="display:grid;grid-template-columns:1.6fr 1fr;gap:16px">
        <div class="specimen">
          <div class="specimen-label">实际用法 · 应用标题栏</div>
          ${titlebar({ doc: "dbswitch — localhost" })}
        </div>
        <div class="specimen">
          <div class="specimen-label">实际用法 · 关于对话框头部</div>
          <div style="display:flex;flex-direction:column;align-items:center;gap:6px;padding:12px 0">
            ${navkitLogo(56)}
            <div style="font-size:15px;font-weight:600">NavKit</div>
            <div style="font-size:12px;color:var(--text-tertiary)">版本 1.0.0</div>
          </div>
        </div>
      </div>
      <div class="specimen">
        <div class="specimen-label">配色与禁忌</div>
          <div style="display:flex;flex-direction:column;gap:8px;font-size:12px;color:var(--text-secondary);line-height:1.6">
            <div>· 无背景容器：透明底，直接叠在任意底色 / 壁纸 / 标题栏上，不引入色块</div>
            <div>· 图形三色：顶层冰青 <code style="font-family:var(--font-mono)">#baf8fe → #97dce7</code>（基色 #a9f6fe） / 中层青瓷 <code style="font-family:var(--font-mono)">#cae6dd → #a9c9c3</code>（基色 #bde0d5） / 底层雾蓝 <code style="font-family:var(--font-mono)">#9db1d5 → #778eba</code>（基色 #849dcb）</div>
            <div>· 同族描边（无容器必需）：顶层 <code style="font-family:var(--font-mono)">#82becc</code> / 中层 <code style="font-family:var(--font-mono)">#91aead</code> / 底层 <code style="font-family:var(--font-mono)">#677ca6</code>，保证浅色中层在浅底、冰青顶层在深底都有轮廓</div>
            <div>· 无字母依赖；单色场景用单色版（三层轮廓），禁止给主版整体改色</div>
            <div>· 最小尺寸 16px；≤24px 描边与投影自动加粗加深，以保住层间分隔</div>
            <div>· 禁止拉伸、旋转、加回背景板或改描边为异色；深色底一律用单色版而非改色主版</div>
          </div>
        </div>
      </div>
    </div>`,
    { w: 1280, h: 900 },
  );
}
