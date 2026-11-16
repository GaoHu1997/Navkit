/** 工作页面：设计器 / 备份 / 用户 / 自动运行 / 模型 / 图表 */

import { icon } from "./assets/icons.js";
import {
  esc,
  button,
  check,
  chip,
  emptyState,
  grid,
  gridFooter,
  infopanel,
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
  commandbar,
  dialog,
  field,
  input,
  footer,
  highlightSql,
} from "./assets/ui.js";

function shell(opts) {
  return `<div class="window">
    ${titlebar({ doc: opts.doc, menu: opts.menu, openMenu: opts.openMenu })}
    ${commandbar({ active: opts.active })}
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

/* =============================================================== 表设计器 */

export function designerScreen() {
  const cols = [
    { label: "#", key: "n", width: 34, align: "right" },
    { label: "名称", key: "name", width: 168 },
    { label: "类型", key: "type", width: 104 },
    { label: "长度", key: "len", width: 62, align: "right" },
    { label: "小数点", key: "scale", width: 58, align: "right" },
    { label: "不是 null", key: "nn", width: 72 },
    { label: "虚拟", key: "v", width: 50 },
    { label: "键", key: "key", width: 56 },
    { label: "默认值", key: "def", width: 104 },
    { label: "注释", key: "cmt", width: 96 },
  ];
  const keyCell = (n) =>
    ({ html: `<span style="display:inline-flex;align-items:center;gap:2px;color:#b5760a">${icon("key", { size: 13 })}<span style="font-size:11px;color:var(--text-tertiary)">${n}</span></span>` });
  const rows = [
    ["1", "id", "bigint", "20", "", true, false, keyCell("1"), "", "主键"],
    ["2", "account_name", "varchar", "120", "", true, false, "", "", "账号名"],
    ["3", "category", "varchar", "64", "", false, false, "", "默认", "分类"],
    ["4", "created_at", "datetime", "", "", false, false, "", "CURRENT_TIMESTAMP", "创建时间"],
    ["5", "description", "longtext", "", "", false, false, "", { html: '<span class="cell-null">NULL</span>' }, "描述"],
    ["6", "icon", "longtext", "", "", false, false, "", { html: '<span class="cell-null">NULL</span>' }, "图标"],
    ["7", "password", "varchar", "255", "", true, false, "", "", "密码"],
    ["8", "password_library_id", "bigint", "20", "", false, false, "", { html: '<span class="cell-null">NULL</span>' }, "所属密码库"],
    ["9", "updated_at", "datetime", "", "", false, false, "", "CURRENT_TIMESTAMP", "更新时间"],
    ["10", "user_id", "bigint", "20", "", true, false, "", "", "用户"],
    ["11", "username", "varchar", "80", "", true, false, "", "", "用户名"],
    ["12", "website", "varchar", "255", "", false, false, "", { html: '<span class="cell-null">NULL</span>' }, "网站"],
  ].map((r) => {
    const [n, name, type, len, scale, nn, v, key, def, cmt] = r;
    return {
      n,
      name: { html: `<span style="color:var(--text);font-weight:500">${name}</span>` },
      type: { html: `<span style="font-family:var(--font-mono);font-size:11px;color:#7a5cc4">${type}</span>` },
      len,
      scale,
      nn: { html: nn ? `<span style="color:var(--accent-600)">${icon("check", { size: 14, stroke: 2.4 })}</span>` : "" },
      v: { html: v ? icon("check", { size: 14 }) : "" },
      key,
      def: def && def.html ? def : { html: def ? `<span style="font-family:var(--font-mono);font-size:11px;color:var(--text-tertiary)">${def}</span>` : "" },
      cmt: { html: `<span style="color:var(--text-tertiary)">${cmt}</span>` },
    };
  });

  const content = `<div class="content">
    <div class="dtabs" style="background:var(--bg-subtle)">
      <div class="dtab active">字段</div>
      <div class="dtab">索引</div>
      <div class="dtab">外键</div>
      <div class="dtab">检查</div>
      <div class="dtab">触发器</div>
      <div class="dtab">选项</div>
      <div class="dtab">注释</div>
      <div class="dtab">SQL 预览</div>
    </div>
    <div class="designer-grid">
      ${grid(cols, rows, { selectedRow: 0, pkKey: "key" })}
    </div>
    <div class="designer-foot">
      <span style="font-size:11px;color:var(--text-tertiary)">默认</span>
      ${select("CURRENT_TIMESTAMP", { width: 176 })}
      <span class="sb-sep"></span>
      ${check("自动递增", true)}
      ${check("无符号")}
      ${check("填充率")}
      <div style="flex:1"></div>
      <span style="font-size:11px;color:var(--text-tertiary)">字段数：12</span>
    </div>
  </div>`;

  return screen(
    "表设计器",
    "设计器选项卡下划线指示；字段网格斑马 + 主键金色钥匙；页脚选项按语义分组",
    shell({
      doc: "account @password_manager",
      active: "表",
      treeNodes: defaultTree("password_manager"),
      tabs: [
        { label: "对象", icon: "table" },
        { label: "account @password_manager", icon: "columns", dirty: true },
        { label: "无标题 — 查询", icon: "code" },
      ],
      activeTab: 1,
      pagetoolbar: pagetoolbar([
        { label: "保存", icon: "save" },
        "-",
        { label: "添加字段", icon: "plus" },
        { label: "插入字段", icon: "plus" },
        { label: "删除字段", icon: "trash" },
        "-",
        { label: "主键", icon: "key" },
        { label: "上移", icon: "chevron-up", disabled: true },
        { label: "下移", icon: "chevron-down", disabled: true },
        "|",
        { label: "重新载入定义", icon: "refresh" },
      ]),
      content,
      info: {
        icon: "table",
        title: "account",
        subtitle: "表 · 设计模式",
        refs: [{ logo: "mysql", text: "localhost / password_manager" }],
        groups: [
          { title: "改动", rows: [["新增字段", "0"], ["修改字段", "1"], ["删除字段", "0"], ["状态", "未保存"]] },
          { title: "属性", rows: [["引擎", "InnoDB"], ["排序规则", "utf8mb4_0900_ai_ci"], ["行格式", "Dynamic"], ["数据长度", "368.0 KB"]] },
        ],
      },
      statusLeft: [
        `<span class="sb-item">字段数：<span class="sb-strong">12</span></span>`,
        '<span class="sb-sep"></span>',
        `<span class="sb-item" style="color:var(--warn-600)">${icon("warning", { size: 12 })}有未保存的改动</span>`,
      ],
    }),
    { w: 1440, h: 900 },
  );
}

/* ================================================================= 备份页 */

export function backupScreen() {
  const cols = [
    { label: "名称", key: "name", width: 280 },
    { label: "修改日期", key: "date", width: 168 },
    { label: "文件大小", key: "size", width: 118, align: "right" },
    { label: "注释", key: "note" },
  ];
  const rows = [
    ["20260515_password_manager", "2026-05-15 14:57:08", "1.24 MB", "每日全量备份"],
    ["20260514_password_manager", "2026-05-14 02:00:01", "1.18 MB", "每日全量备份"],
    ["20260513_password_manager", "2026-05-13 02:00:02", "1.12 MB", ""],
  ].map((r) => ({
    name: { html: `<span class="cell-name"><span class="cell-ico">${icon("backup", { size: 14 })}</span>${r[0]}</span>` },
    date: r[1],
    size: r[2],
    note: r[3] ? r[3] : { html: '<span class="cell-null">NULL</span>' },
  }));

  const content = `<div class="content">
    ${grid(cols, rows, { selectedRow: 0 })}
    ${gridFooter({ left: `<span>3 个备份 · 共 3.54 MB</span>`, count: "共 3 行" })}
  </div>`;

  return screen(
    "备份页",
    "备份条目用备份图标标识；主操作「新建备份」在工具栏最左，危险操作靠后",
    shell({
      doc: "password_manager — 备份",
      active: "备份",
      treeNodes: defaultTree("password_manager"),
      tabs: [
        { label: "对象", icon: "table" },
        { label: "password_manager — 备份", icon: "backup" },
      ],
      activeTab: 1,
      pagetoolbar: pagetoolbar([
        { label: "新建备份", icon: "plus" },
        "-",
        { label: "还原备份", icon: "history" },
        { label: "打开所在文件夹", icon: "open" },
        "-",
        { label: "删除备份", icon: "trash" },
        "|",
        { label: "刷新", icon: "refresh" },
      ]),
      content,
      info: {
        icon: "backup",
        title: "password_manager",
        subtitle: "备份集",
        refs: [{ logo: "mysql", text: "localhost / MySQL" }],
        groups: [
          { title: "概要", rows: [["备份数", "3"], ["总大小", "3.54 MB"], ["最近备份", "2026-05-15 14:57"], ["保留策略", "7 天"]] },
        ],
      },
      statusLeft: [`<span class="sb-item"><span class="sb-strong">3 个备份</span></span>`],
    }),
    { w: 1440, h: 900 },
  );
}

/* ================================================================= 用户页 */

export function usersScreen() {
  const cols = [
    { label: "用户", key: "name", width: 240 },
    { label: "主机", key: "host", width: 150 },
    { label: "SSL 类型", key: "ssl", width: 120 },
    { label: "每小时查询", key: "q", width: 110, align: "right" },
    { label: "每小时更新", key: "u", width: 110, align: "right" },
    { label: "每小时连接", key: "c", width: 110, align: "right" },
    { label: "最大并发", key: "mc", width: 104, align: "right" },
    { label: "超级用户", key: "sa", width: 100 },
  ];
  const rows = [
    ["root", "localhost", "ANY", "不限", "不限", "不限", "0", { html: chip("超级用户", "accent") }],
    ["mysql.sys", "localhost", "ANY", "不限", "不限", "不限", "0", ""],
    ["navkit", "%", "ANY", "0", "0", "0", "20", ""],
    ["readonly", "%", "REQUIRED", "3600", "0", "7200", "10", ""],
  ].map((r) => ({
    name: { html: `<span class="cell-name"><span class="cell-ico" style="color:#b5760a">${icon("user", { size: 14 })}</span>${r[0]}</span>` },
    host: { html: `<span style="font-family:var(--font-mono);font-size:11px">${r[1]}</span>` },
    ssl: r[2] === "REQUIRED" ? { html: chip("REQUIRED", "ok") } : { html: '<span class="cell-muted">ANY</span>' },
    q: r[3],
    u: r[4],
    c: r[5],
    mc: r[6],
    sa: r[7],
  }));

  const content = `<div class="content">
    ${grid(cols, rows, { selectedRow: 0 })}
    ${gridFooter({ left: `<span>4 个用户</span>`, count: "共 4 行" })}
  </div>`;

  return screen(
    "用户页",
    "权限类数值右对齐 tabular-nums；SSL 类型用语义 chip；新增/改密按连接类型启用",
    shell({
      doc: "password_manager — 用户",
      active: "用户",
      treeNodes: defaultTree("password_manager"),
      tabs: [
        { label: "对象", icon: "table" },
        { label: "password_manager — 用户", icon: "user" },
      ],
      activeTab: 1,
      pagetoolbar: pagetoolbar([
        { label: "新建用户", icon: "plus" },
        "-",
        { label: "修改密码", icon: "lock", disabled: true },
        { label: "删除用户", icon: "trash", disabled: true },
        { label: "权限管理员", icon: "lock" },
        "|",
        { label: "刷新", icon: "refresh" },
      ]),
      content,
      info: {
        icon: "user",
        title: "password_manager",
        subtitle: "用户与权限",
        refs: [{ logo: "mysql", text: "localhost / MySQL 8.0.36" }],
        groups: [
          { title: "概要", rows: [["用户数", "4"], ["超级用户", "1"], ["SSL 强制", "1"]] },
        ],
      },
      statusLeft: [`<span class="sb-item"><span class="sb-strong">4 个用户</span></span>`],
    }),
    { w: 1440, h: 900 },
  );
}

/* =============================================================== 自动运行 */

export function scheduleScreen() {
  const cols = [
    { label: "任务名", key: "name", width: 240 },
    { label: "数据库", key: "db", width: 168 },
    { label: "执行间隔", key: "every", width: 130 },
    { label: "下次运行", key: "next", width: 168 },
    { label: "上次结果", key: "last", width: 120 },
    { label: "SQL", key: "sql" },
  ];
  const rows = [
    ["清理过期会话", "password_manager", "每 1 小时", "2026-05-16 09:00:00", { html: chip("成功", "ok") }, { html: '<span style="font-family:var(--font-mono);font-size:11px;color:var(--text-tertiary)">DELETE FROM session WHERE ...</span>' }],
    ["每日统计汇总", "navkit", "每天", "2026-05-17 00:00:00", { html: chip("成功", "ok") }, { html: '<span style="font-family:var(--font-mono);font-size:11px;color:var(--text-tertiary)">INSERT INTO stat_daily SELECT ...</span>' }],
    ["同步作业日志", "navkit", "每 5 分钟", "2026-05-16 08:35:00", { html: chip("失败", "danger") }, { html: '<span style="font-family:var(--font-mono);font-size:11px;color:var(--text-tertiary)">UPDATE job_logback SET ...</span>' }],
    ["归档旧备份", "password_manager", "仅手动执行", "—", { html: '<span class="cell-muted">未运行</span>' }, { html: '<span style="font-family:var(--font-mono);font-size:11px;color:var(--text-tertiary)">DELETE FROM backup WHERE ...</span>' }],
  ].map((r) => ({ name: { html: `<span class="cell-name"><span class="cell-ico">${icon("schedule", { size: 14 })}</span>${r[0]}</span>` }, db: r[1], every: r[2], next: r[3], last: r[4], sql: r[5] }));

  const content = `<div class="content">
    ${grid(cols, rows, { selectedRow: 0 })}
    ${gridFooter({ left: `<span>4 个任务 · 3 个已启用</span>`, count: "共 4 行" })}
  </div>`;

  return screen(
    "自动运行（调度）",
    "任务状态用语义 chip；下次运行时间等宽数字；失败任务可直接从行内进入运行历史",
    shell({
      doc: "自动运行",
      active: "自动运行",
      treeNodes: defaultTree("navkit"),
      tabs: [
        { label: "对象", icon: "table" },
        { label: "自动运行", icon: "schedule" },
      ],
      activeTab: 1,
      pagetoolbar: pagetoolbar([
        { label: "新建任务", icon: "plus" },
        "-",
        { label: "编辑任务", icon: "edit" },
        { label: "立即运行", icon: "play" },
        { label: "删除任务", icon: "trash" },
        "-",
        { label: "运行历史", icon: "history" },
        "|",
        { label: "刷新", icon: "refresh" },
      ]),
      content,
      info: {
        icon: "schedule",
        title: "自动运行",
        subtitle: "4 个任务",
        groups: [
          { title: "状态", rows: [["已启用", "3"], ["已暂停", "1"], ["最近失败", "1"]] },
          { title: "调度", rows: [["最小间隔", "5 分钟"], ["下次检查", "08:35:00"]] },
        ],
      },
      statusLeft: [`<span class="sb-item"><span class="sb-strong">4 个任务</span></span>`, '<span class="sb-sep"></span>', `<span class="sb-item" style="color:var(--danger-600)">${icon("error", { size: 12 })}1 个任务失败</span>`],
    }),
    { w: 1440, h: 900 },
  );
}

/* =================================================================== 模型 */

export function modelScreen() {
  const table = (x, y, name, rows, accent) => `<div class="er-table" style="left:${x}px;top:${y}px">
    <div class="er-head" style="${accent ? "" : "background:var(--n-600)"}">${icon("table", { size: 13 })}<span>${name}</span></div>
    ${rows
      .map(
        ([k, t, type]) =>
          `<div class="er-row">${k ? `<span class="er-key">${icon("key", { size: 10 })}</span>` : '<span style="width:10px"></span>'}<span>${t}</span><span class="er-type">${type}</span></div>`,
      )
      .join("")}
  </div>`;

  const content = `<div class="content">
    <div class="canvas">
      <svg style="position:absolute;inset:0;width:100%;height:100%;pointer-events:none">
        <!-- note.category_id → note_category.id -->
        <path d="M300 157 C 254 157, 252 169, 206 169" fill="none" stroke="var(--n-400)" stroke-width="1.5" stroke-dasharray="4 3"/>
        <!-- note_tag.note_id → note.id -->
        <path d="M300 391 C 398 391, 380 135, 476 135" fill="none" stroke="var(--n-400)" stroke-width="1.5" stroke-dasharray="4 3"/>
        <!-- note_tag.tag_id → tag.id -->
        <path d="M476 413 C 532 413, 530 169, 586 169" fill="none" stroke="var(--n-400)" stroke-width="1.5" stroke-dasharray="4 3"/>
        <!-- note.user_id → sys_user.id -->
        <path d="M476 245 C 536 245, 528 389, 586 389" fill="none" stroke="var(--n-400)" stroke-width="1.5" stroke-dasharray="4 3"/>
        <circle cx="300" cy="157" r="3" fill="var(--accent-500)"/>
        <circle cx="206" cy="169" r="3" fill="var(--accent-500)"/>
        <circle cx="300" cy="391" r="3" fill="var(--accent-500)"/>
        <circle cx="476" cy="135" r="3" fill="var(--accent-500)"/>
        <circle cx="476" cy="413" r="3" fill="var(--accent-500)"/>
        <circle cx="586" cy="169" r="3" fill="var(--accent-500)"/>
        <circle cx="476" cy="245" r="3" fill="var(--accent-500)"/>
        <circle cx="586" cy="389" r="3" fill="var(--accent-500)"/>
      </svg>
      ${table(30, 130, "note_category", [
        ["1", "id", "bigint"],
        ["", "name", "varchar"],
        ["", "sort_order", "int"],
        ["", "user_id", "bigint"],
      ], true)}
      ${table(300, 96, "note", [
        ["1", "id", "bigint"],
        ["", "category_id", "bigint"],
        ["", "title", "varchar"],
        ["", "content", "longtext"],
        ["", "url", "varchar"],
        ["", "user_id", "bigint"],
        ["", "created_at", "datetime"],
      ], true)}
      ${table(300, 330, "note_tag", [
        ["1", "id", "bigint"],
        ["", "note_id", "bigint"],
        ["", "tag_id", "bigint"],
      ], true)}
      ${table(586, 130, "tag", [
        ["1", "id", "bigint"],
        ["", "name", "varchar"],
        ["", "color", "varchar"],
      ], false)}
      ${table(586, 350, "sys_user", [
        ["1", "id", "bigint"],
        ["", "username", "varchar"],
        ["", "email", "varchar"],
      ], false)}
      <div style="position:absolute;right:16px;bottom:16px;display:flex;gap:4px;padding:4px;border:1px solid var(--border);border-radius:8px;background:var(--bg-surface);box-shadow:var(--shadow-2)">
        <button class="iconbtn" title="缩小">${icon("minus", { size: 13 })}</button>
        <span style="display:grid;place-items:center;min-width:44px;font-size:11px;color:var(--text-secondary);font-variant-numeric:tabular-nums">100%</span>
        <button class="iconbtn" title="放大">${icon("plus", { size: 13 })}</button>
        <div class="sb-sep"></div>
        <button class="iconbtn" title="适应窗口">${icon("grid", { size: 13 })}</button>
      </div>
      <div style="position:absolute;left:16px;top:14px;display:flex;gap:6px">
        ${chip("5 张表", "accent")}${chip("4 条关系")}
      </div>
    </div>
  </div>`;

  return screen(
    "模型（ER 图）",
    "画布改为点阵网格底；表卡片单色强调头部 + 等宽字段；连接线用虚线表示非强制外键",
    shell({
      doc: "password_manager — 模型",
      active: "模型",
      treeNodes: defaultTree("password_manager"),
      tabs: [
        { label: "对象", icon: "table" },
        { label: "password_manager — 模型", icon: "model" },
      ],
      activeTab: 1,
      pagetoolbar: pagetoolbar([
        { label: "适应窗口", icon: "grid" },
        { label: "放大", icon: "plus" },
        { label: "缩小", icon: "minus" },
        "-",
        { label: "刷新", icon: "refresh" },
        "|",
        { label: "导出图片", icon: "export" },
      ]),
      content,
      info: {
        icon: "model",
        title: "note / tag",
        subtitle: "关系",
        groups: [
          { title: "关系", rows: [["note.category_id → note_category.id", "多对一"], ["note_tag.note_id → note.id", "多对一"], ["note_tag.tag_id → tag.id", "多对一"]] },
        ],
      },
      statusLeft: [`<span class="sb-item">5 张表 · 4 条关系</span>`, '<span class="sb-sep"></span>', `<span class="sb-item">缩放 100%</span>`],
    }),
    { w: 1440, h: 900 },
  );
}

/* =================================================================== 图表 */

export function chartScreen() {
  const bars = [
    ["默认", 12], ["代码", 28], ["教程", 19], ["测试", 7], ["归档", 15], ["草稿", 9], ["分享", 22],
  ];
  const max = 32;
  const chart = `<svg viewBox="0 0 720 320" style="width:100%;height:320px">
    ${[0, 1, 2, 3, 4].map((i) => {
      const y = 260 - i * 54;
      return `<line x1="56" y1="${y}" x2="700" y2="${y}" stroke="var(--n-150)" stroke-width="1"/>
        <text x="46" y="${y + 4}" text-anchor="end" font-size="10" fill="var(--text-tertiary)" font-family="var(--font-mono)">${i * 8}</text>`;
    }).join("")}
    ${bars
      .map(([label, v], i) => {
        const h = (v / max) * 216;
        const x = 76 + i * 90;
        return `<rect x="${x}" y="${260 - h}" width="46" height="${h}" rx="3" fill="var(--accent-500)"/>
          <text x="${x + 23}" y="${252 - h}" text-anchor="middle" font-size="10" fill="var(--text-secondary)" font-family="var(--font-mono)">${v}</text>
          <text x="${x + 23}" y="278" text-anchor="middle" font-size="11" fill="var(--text-tertiary)">${label}</text>`;
      })
      .join("")}
    <line x1="56" y1="260" x2="700" y2="260" stroke="var(--n-300)" stroke-width="1"/>
  </svg>`;

  const content = `<div class="content">
    <div class="chartcard">
      <div style="display:flex;align-items:center;gap:12px;margin-bottom:16px">
        <h3 style="margin:0;font-size:15px;font-weight:600">各分类笔记数</h3>
        <div style="flex:1"></div>
        <div class="select" style="width:120px;height:26px"><span>柱状图</span>${icon("chevron-down", { size: 12 })}</div>
        <div class="select" style="width:150px;height:26px"><span>category</span>${icon("chevron-down", { size: 12 })}</div>
        <div class="select" style="width:150px;height:26px"><span>count</span>${icon("chevron-down", { size: 12 })}</div>
      </div>
      <div style="border:1px solid var(--border-subtle);border-radius:8px;padding:20px">${chart}</div>
      <div style="display:flex;gap:8px;margin-top:12px">
        ${chip("数据点 7")}${chip("来源 note_category")}${chip("已刷新 08:32", "ok")}
      </div>
    </div>
  </div>`;

  return screen(
    "图表页",
    "图表控件移到标题行右侧；图表容器用浅边框卡片承载；数据来源与刷新状态用 chip 说明",
    shell({
      doc: "各分类笔记数 — 图表",
      active: "图表",
      treeNodes: defaultTree("password_manager"),
      tabs: [
        { label: "对象", icon: "table" },
        { label: "各分类笔记数 — 图表", icon: "chart" },
      ],
      activeTab: 1,
      pagetoolbar: pagetoolbar([
        { label: "刷新", icon: "refresh" },
        "-",
        { label: "导出图片", icon: "export" },
        { label: "导出数据", icon: "export" },
      ]),
      content,
      info: {
        icon: "chart",
        title: "各分类笔记数",
        subtitle: "柱状图",
        groups: [
          { title: "数据", rows: [["X 轴", "category"], ["数值列", "count"], ["数据点", "7"], ["最大", "28"]], mono: true },
        ],
      },
      statusLeft: [`<span class="sb-item">图表 · 7 个数据点</span>`],
    }),
    { w: 1440, h: 900 },
  );
}
