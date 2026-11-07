/** 全部对话框设计图 */

import { icon } from "./assets/icons.js";
import { dbLogo } from "./assets/db-logos.js";
import { navkitLogo, navkitLogoMono } from "./assets/logo.js";
import {
  esc,
  button,
  check,
  chip,
  dialog,
  emptyState,
  field,
  footer,
  grid,
  input,
  menu,
  note,
  radio,
  screen,
  select,
  toast,
} from "./assets/ui.js";

/* 对话框外层画框：深色应用底 + 居中对话框（模拟遮罩） */
function atop(appBg, dlg) {
  return `<div class="window" style="position:relative">
    <div style="flex:1;display:flex;min-height:0">${appBg}</div>
    ${dlg}
  </div>`;
}

/* 一个简化的应用背景（对话框后面） */
function dimApp() {
  return `<div style="flex:1;display:flex;flex-direction:column;min-height:0">
    <div style="height:38px;border-bottom:1px solid var(--border-subtle);background:var(--bg-surface);display:flex;align-items:center;padding:0 12px;gap:8px">
      <span style="display:grid;place-items:center">${navkitLogo(20)}</span>
      <span style="font-size:13px;font-weight:600">NavKit</span>
      <span style="width:1px;height:14px;background:var(--border)"></span>
      <span style="font-size:12px;color:var(--text-tertiary)">dbswitch — localhost</span>
    </div>
    <div style="height:46px;border-bottom:1px solid var(--border);background:var(--bg-surface)"></div>
    <div style="flex:1;display:flex;min-height:0;background:var(--bg-surface)">
      <div style="width:264px;border-right:1px solid var(--border);background:var(--bg-surface)"></div>
      <div style="flex:1"></div>
      <div style="width:292px;border-left:1px solid var(--border)"></div>
    </div>
    <div style="height:26px;border-top:1px solid var(--border);background:var(--bg-surface)"></div>
  </div>`;
}

const DIALOG_W = 720;

/* ========================================================= 新建连接（类型） */

const CATS = [
  "关系型数据库",
  "数仓分析",
  "国产数据库",
  "轻量数据库",
  "文档/缓存/检索",
  "图谱/向量",
  "时序数据库",
  "消息队列",
  "注册与配置",
];

const CARDS = [
  ["mysql", "MySQL", false],
  ["postgresql", "PostgreSQL", false],
  ["sqlserver", "SQL Server", false],
  ["oracle", "Oracle", false],
  ["mariadb", "MariaDB", false],
  ["firebird", "Firebird", false],
  ["cockroachdb", "CockroachDB", false],
  ["db2", "DB2", false],
  ["informix", "Informix", false],
  ["jdbcx", "JDBCX", false],
  ["spanner", "Cloud Spanner", false],
  ["iris", "IRIS", false],
  ["cache", "InterSystems Caché", false],
  ["custom_mysql", "Custom (MySQL)", false],
  ["odbc", "自定义 ODBC", false],
  ["h2", "H2", true],
  ["clickhouse", "ClickHouse", false],
  ["snowflake", "Snowflake", false],
  ["druid", "Apache Druid", true],
  ["bigquery", "BigQuery", false],
];

export function connectionPickerScreen() {
  const cards = CARDS.map(
    ([kind, label, planned]) => `<div class="state-card" style="display:flex;flex-direction:column;align-items:center;justify-content:center;gap:8px;min-height:84px;padding:10px;position:relative;${planned ? "opacity:.55;" : ""}${kind === "mysql" ? "box-shadow:inset 0 0 0 2px var(--accent-500);" : ""}">
      ${planned ? '<span style="position:absolute;right:6px;top:6px">'+chip("规划中", "warn")+"</span>" : ""}
      ${dbLogo(kind, 30, { planned }) || `<span style="color:var(--text-tertiary)">${icon("database", { size: 28 })}</span>`}
      <span style="font-size:11px;text-align:center;line-height:1.3;color:var(--text-secondary)">${esc(label)}</span>
    </div>`,
  ).join("");

  const body = `<div style="display:flex;flex-direction:column;height:100%">
    <div style="display:flex;align-items:center;gap:8px;padding:10px 14px;border-bottom:1px solid var(--border-subtle)">
      <div style="display:flex;gap:1px;padding:2px;border-radius:4px;background:var(--bg-sunken)">
        <button class="iconbtn active" style="width:26px;height:22px">${icon("grid", { size: 13 })}</button>
        <button class="iconbtn" style="width:26px;height:22px">${icon("list", { size: 13 })}</button>
      </div>
      <div class="search" style="margin:0;flex:0 1 280px">${icon("search", { size: 13 })}<input placeholder="搜索数据库类型" readonly></div>
      <div style="flex:1"></div>
      <button class="ptool" style="height:26px">${icon("info", { size: 13 })}<span>ODBC 连接</span></button>
    </div>
    <div style="display:flex;flex:1;min-height:0">
      <div class="catlist" style="width:160px">${CATS.map((c, i) => `<div class="catitem${i === 0 ? " active" : ""}">${c}</div>`).join("")}</div>
      <div style="flex:1;padding:14px;overflow:auto">
        <div style="display:grid;grid-template-columns:repeat(5,1fr);gap:10px">${cards}</div>
      </div>
    </div>
    <div style="display:flex;align-items:center;gap:8px;min-height:38px;padding:0 14px;border-top:1px solid var(--border-subtle);background:var(--bg-subtle);font-size:12px">
      ${dbLogo("mysql", 18)}
      <span style="color:var(--text-tertiary)">已选择:</span>
      <span style="font-weight:600">MySQL</span>
      <span style="color:var(--text-tertiary);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">默认端口 3306 · 支持 SSL / SSH / HTTP 通道</span>
    </div>
  </div>`;

  return screen(
    "新建连接 · 数据源类型选择",
    "900×640；左侧 9 个分类固定，右侧卡片网格 5 列；「规划中」用琥珀色 chip 且整卡降级",
    atop(
      dimApp(),
      dialog({
        title: "新建连接",
        width: 900,
        height: 640,
        flush: true,
        body,
        footer: footer([{ label: "下一步", kind: "primary" }]),
      }),
    ),
    { w: 1360, h: 860 },
  );
}

/* ============================================================= 连接属性 */

export function connectionDialogScreen() {
  const body = `<div style="padding:20px 24px 8px">
    <div style="display:flex;align-items:center;justify-content:center;gap:28px;padding:4px 0 22px">
      <div style="display:flex;flex-direction:column;align-items:center;gap:6px">
        <div style="width:52px;height:52px;border-radius:10px;background:var(--bg-sunken);display:grid;place-items:center">${dbLogo("mysql", 34)}</div>
        <span style="font-size:12px;color:var(--text-secondary)">MySQL</span>
      </div>
      <div style="width:52px;height:1px;background:var(--border-strong);position:relative">
        <span style="position:absolute;right:-4px;top:-3px;width:7px;height:7px;border-radius:50%;background:var(--border-strong)"></span>
      </div>
      <div style="display:flex;flex-direction:column;align-items:center;gap:6px">
        <div style="width:52px;height:52px;border-radius:10px;background:var(--bg-sunken);display:grid;place-items:center;color:var(--text-tertiary)">${icon("schema", { size: 26 })}</div>
        <span style="font-size:12px;color:var(--text-secondary)">数据库</span>
      </div>
    </div>
    ${field("类型", `<span style="display:inline-flex;align-items:center;gap:7px;font-weight:600">${dbLogo("mysql", 18)}MySQL</span>`)}
    <div style="margin:-4px 0 14px 112px;font-size:11px;color:var(--text-tertiary)">默认端口 3306，支持 utf8mb4 与窗口函数。</div>
    ${field("连接名", input("localhost", { w: "w-lg" }))}
    ${field("分组", `<div class="select w-lg"><span>本地连接</span>${icon("chevron-down", { size: 14 })}</div>`)}
    ${field("主机", `<input class="input" style="flex:1" value="localhost" readonly><label style="font-size:12px;color:var(--text-secondary);white-space:nowrap">端口</label><input class="input" style="width:88px" value="3306" readonly>`)}
    ${field("用户名", input("root"))}
    ${field("密码", `<input class="input" type="password" value="password" readonly><button class="iconbtn" title="显示密码">${icon("eye", { size: 14 })}</button>`)}
    <div style="margin:-4px 0 12px 112px">${check("保存密码", true)}</div>
    ${field("数据库", input("password_manager"))}
    ${field("", `<div style="display:flex;gap:8px">${button("测试连接", { size: "sm" })}<span style="display:flex;align-items:center;gap:5px;font-size:11px;color:var(--ok-600)">${icon("success", { size: 13 })}连接成功 — 8.0.36</span></div>`)}
  </div>`;

  return screen(
    "连接属性",
    "6 个选项卡收敛为「常规 / 通道 / 高级 / 信息」；类型行改为只读徽标；测试连接结果就地显示",
    atop(
      dimApp(),
      dialog({
        title: "连接属性 (MySQL)",
        width: 560,
        tabs: ["常规", "通道", "高级", "信息"],
        activeTab: 0,
        body,
        footer: footer([
          { label: "测试连接", alignLeft: true },
          { label: "确定", kind: "primary" },
          { label: "取消" },
        ]),
      }),
    ),
    { w: 1100, h: 860 },
  );
}

/* =============================================================== 导入向导 */

export function importWizardScreen() {
  const body = `<div style="padding:16px 20px">
    ${field("文件名", `<input class="input" style="flex:1" value="D:\\data\\note_category.csv" readonly>${button("浏览...", { size: "sm" })}`)}
    ${field("格式", `<div class="select" style="width:130px"><span>CSV</span>${icon("chevron-down", { size: 14 })}</div><div class="select" style="width:130px"><span>UTF-8</span>${icon("chevron-down", { size: 14 })}</div>`)}
    ${field("分隔符", `<input class="input w-sm" value="," readonly><span style="font-size:11px;color:var(--text-tertiary)">首行为列名</span>${check("", true)}`)}
    <div style="margin:16px 0 8px;font-size:11px;font-weight:600;letter-spacing:.04em;color:var(--text-tertiary)">预览（前 10 行）</div>
    <div style="border:1px solid var(--border);border-radius:6px;overflow:hidden;max-height:180px">
      <table class="grid">
        <thead><tr><th class="num" style="width:44px">#</th><th>id</th><th>name</th><th>sort_order</th><th>user_id</th><th>created_at</th></tr></thead>
        <tbody>
          ${[
            ["1", "1", "默认", "3", "1", "2026-05-12 09:14:22"],
            ["2", "2", "代码", "1", "1", "2026-05-12 09:15:03"],
            ["3", "3", "教程", "0", "1", "2026-05-12 09:16:41"],
            ["4", "33", "测试", "2", "1", "2026-05-12 10:02:11"],
          ]
            .map((r) => `<tr>${r.map((c, i) => `<td${i === 0 ? ' class="num"' : ""}>${c}</td>`).join("")}</tr>`)
            .join("")}
        </tbody>
      </table>
    </div>
    <div style="margin-top:12px">${note("info", "列按名称自动匹配；未匹配的列写入 NULL。")}</div>
  </div>`;

  return screen(
    "导入向导",
    "步骤条固定在标题下；预览面板固定在步骤内容下方，切换步骤不丢失；页脚「关闭」永远在最右",
    atop(
      dimApp(),
      dialog({
        title: "导入向导",
        width: 660,
        steps: ["源", "目标", "映射", "执行"],
        activeStep: 0,
        body,
        footer: footer([
          { label: "上一步", disabled: true },
          { label: "下一步" },
          { label: "开始导入", kind: "primary" },
          { label: "关闭" },
        ]),
      }),
    ),
    { w: 1200, h: 880 },
  );
}

/* =============================================================== 导出向导 */

export function exportWizardScreen() {
  const body = `<div style="padding:16px 20px">
    ${field("连接", `<div class="select w-lg"><span>localhost（MySQL）</span>${icon("chevron-down", { size: 14 })}</div>`)}
    ${field("数据库", `<div class="select w-lg"><span>password_manager</span>${icon("chevron-down", { size: 14 })}</div>`)}
    ${field("表", `<div class="select w-lg"><span>note_category</span>${icon("chevron-down", { size: 14 })}</div>`)}
    <div style="margin-top:16px">${note("info", "下一步选择导出目标与格式，可在此指定 WHERE 筛选与行数上限。")}</div>
  </div>`;

  return screen(
    "导出向导",
    "与导入向导共用步骤条与页脚结构；源步骤使用三个下拉（连接 / 库 / 表）逐级联动",
    atop(
      dimApp(),
      dialog({
        title: "导出向导",
        width: 660,
        steps: ["源", "目标", "格式", "执行"],
        activeStep: 0,
        body,
        footer: footer([
          { label: "上一步", disabled: true },
          { label: "下一步" },
          { label: "开始导出", kind: "primary" },
          { label: "关闭" },
        ]),
      }),
    ),
    { w: 1200, h: 820 },
  );
}

/* =============================================================== 数据传输 */

export function transferScreen() {
  const body = `<div style="padding:16px 20px">
    <div style="display:grid;grid-template-columns:1fr 32px 1fr;gap:12px;align-items:start">
      <div style="border:1px solid var(--border);border-radius:8px;padding:12px">
        <div style="font-size:12px;font-weight:600;margin-bottom:10px;display:flex;align-items:center;gap:6px">${icon("database", { size: 14 })}源</div>
        ${field("连接", select("localhost（MySQL）", { noCaret: false }), { labelW: 60 })}
        ${field("数据库", select("password_manager"), { labelW: 60 })}
        ${field("表", select("note_category"), { labelW: 60 })}
      </div>
      <div style="display:grid;place-items:center;height:100%;color:var(--text-tertiary)">${icon("transfer", { size: 18 })}</div>
      <div style="border:1px solid var(--border);border-radius:8px;padding:12px">
        <div style="font-size:12px;font-weight:600;margin-bottom:10px;display:flex;align-items:center;gap:6px">${icon("database", { size: 14 })}目标</div>
        ${field("连接", select("dev-mysql（MySQL）"), { labelW: 60 })}
        ${field("数据库", select("dbswitch"), { labelW: 60 })}
        ${field("表", select("（请先选择库）"), { labelW: 60 })}
      </div>
    </div>
    <div style="margin:16px 0 8px;font-size:11px;font-weight:600;letter-spacing:.04em;color:var(--text-tertiary)">选项</div>
    <div style="display:flex;flex-wrap:wrap;gap:16px;align-items:center">
      ${check("复制表结构", true)}${check("复制数据", true)}${check("覆盖已存在的表")}
    </div>
    <div style="margin-top:12px">${field("筛选条件", input("", { w: "w-lg", placeholder: "如：created_at > '2026-01-01'" }))}</div>
  </div>`;

  return screen(
    "数据传输",
    "源与目标并列，用中间箭头说明方向；选项行横排以降低高度；目标表在选择库前明确禁用文案",
    atop(
      dimApp(),
      dialog({
        title: "数据传输",
        width: 760,
        body,
        footer: footer([{ label: "开始", kind: "primary" }, { label: "关闭" }]),
      }),
    ),
    { w: 1240, h: 760 },
  );
}

/* ================================================================ SQL 转储 */

export function dumpScreen() {
  const body = `<div style="padding:16px 20px">
    ${field("连接", `<span style="font-size:13px">localhost（MySQL）</span>`)}
    ${field("数据库", `<span style="font-size:13px">password_manager</span>`)}
    ${field("表", `<span style="font-size:13px">note_category</span>`)}
    ${field("输出文件", `<input class="input" style="flex:1" value="D:\\backup\\note_category.sql" readonly>${button("浏览...", { size: "sm" })}`)}
    <div style="margin:14px 0 8px;font-size:11px;font-weight:600;letter-spacing:.04em;color:var(--text-tertiary)">内容</div>
    <div style="display:flex;flex-wrap:wrap;gap:16px">${check("仅结构")}${check("仅数据")}${check("包含 DROP TABLE 语句", true)}</div>
    <div style="margin-top:14px">${note("warn", "「仅结构」与「仅数据」同时勾选时，将只导出结构。")}</div>
  </div>`;

  return screen(
    "SQL 转储",
    "输出路径与内容选项分两组；互斥选项给出就地提示，避免导出后才发现内容不符",
    atop(
      dimApp(),
      dialog({
        title: "SQL 转储",
        width: 600,
        body,
        footer: footer([{ label: "开始", kind: "primary" }, { label: "关闭" }]),
      }),
    ),
    { w: 1100, h: 700 },
  );
}

/* ================================================================ 新建备份 */

export function backupDialogScreen() {
  const objects = [
    ["note", "table", true],
    ["note_category", "table", true],
    ["note_tag", "table", true],
    ["sys_user", "table", false],
    ["v_note_summary", "view", true],
  ];
  const body = `<div style="padding:16px 20px">
    ${field("连接", `<span style="font-size:13px">localhost（MySQL）</span>`)}
    ${field("数据库", `<span style="font-size:13px">password_manager</span>`)}
    ${field("注释", input("每日全量备份", { w: "w-lg" }))}
    <div style="margin:4px 0 12px 112px;display:flex;gap:16px">${check("包含结构", true)}${check("包含数据", true)}</div>
    <div style="display:flex;align-items:center;gap:8px;margin:8px 0 6px">
      <span style="font-size:11px;font-weight:600;letter-spacing:.04em;color:var(--text-tertiary)">对象</span>
      <span style="font-size:11px;color:var(--text-tertiary)">已选 4 / 5</span>
      <div style="flex:1"></div>
      ${check("全选", true)}
    </div>
    <div class="picklist" style="height:172px">
      ${objects
        .map(
          ([n, t, on]) => `<div class="pickrow">${check("", on)}<span style="color:var(--text-tertiary)">${icon(t === "view" ? "view" : "table", { size: 14 })}</span><span class="grow">${n}</span><span style="font-size:11px;color:var(--text-tertiary)">${t === "view" ? "视图" : "表"}</span></div>`,
        )
        .join("")}
    </div>
    <div style="margin-top:12px">${note("info", "备份文件写入数据目录 backups/password_manager/。")}</div>
  </div>`;

  return screen(
    "新建备份",
    "对象清单用可选列表（复选框 + 类型图标 + 类型标签），全选控件移入列表标题行",
    atop(
      dimApp(),
      dialog({
        title: "新建备份",
        width: 560,
        body,
        footer: footer([{ label: "开始", kind: "primary" }, { label: "关闭" }]),
      }),
    ),
    { w: 1080, h: 760 },
  );
}

/* ================================================================ 创建图表 */

export function chartDialogScreen() {
  const bars = [["默认", 12], ["代码", 28], ["教程", 19], ["测试", 7], ["归档", 15]];
  const max = 32;
  const chart = `<svg viewBox="0 0 560 240" style="width:100%;height:240px">
    ${[0, 1, 2, 3, 4].map((i) => {
      const y = 196 - i * 42;
      return `<line x1="44" y1="${y}" x2="548" y2="${y}" stroke="var(--n-150)"/><text x="36" y="${y + 4}" text-anchor="end" font-size="10" fill="var(--text-tertiary)" font-family="var(--font-mono)">${i * 8}</text>`;
    }).join("")}
    ${bars
      .map(([l, v], i) => {
        const h = (v / max) * 168;
        const x = 70 + i * 92;
        return `<rect x="${x}" y="${196 - h}" width="42" height="${h}" rx="3" fill="var(--accent-500)"/><text x="${x + 21}" y="${188 - h}" text-anchor="middle" font-size="10" fill="var(--text-secondary)" font-family="var(--font-mono)">${v}</text><text x="${x + 21}" y="214" text-anchor="middle" font-size="11" fill="var(--text-tertiary)">${l}</text>`;
      })
      .join("")}
    <line x1="44" y1="196" x2="548" y2="196" stroke="var(--n-300)"/>
  </svg>`;

  const body = `<div style="padding:16px 20px">
    <div style="display:flex;flex-wrap:wrap;gap:14px;align-items:center;margin-bottom:14px">
      <div style="display:flex;align-items:center;gap:6px"><span style="font-size:11px;color:var(--text-tertiary)">类型</span><div class="select" style="width:120px;height:26px"><span>柱状图</span>${icon("chevron-down", { size: 12 })}</div></div>
      <div style="display:flex;align-items:center;gap:6px"><span style="font-size:11px;color:var(--text-tertiary)">X 轴</span><div class="select" style="width:150px;height:26px"><span>category</span>${icon("chevron-down", { size: 12 })}</div></div>
      <div style="display:flex;align-items:center;gap:6px"><span style="font-size:11px;color:var(--text-tertiary)">数值列</span><div class="select" style="width:150px;height:26px"><span>count</span>${icon("chevron-down", { size: 12 })}</div></div>
    </div>
    <div style="border:1px solid var(--border-subtle);border-radius:8px;padding:16px">${chart}</div>
  </div>`;

  return screen(
    "创建图表",
    "三个配置项横排在顶部（标签内联而非 112px 字段列），图表区占满剩余空间",
    atop(
      dimApp(),
      dialog({
        title: "创建图表",
        width: 720,
        body,
        footer: footer([{ label: "导出图片", alignLeft: true }, { label: "关闭" }]),
      }),
    ),
    { w: 1200, h: 760 },
  );
}

/* ================================================================ 新建任务 */

export function scheduleDialogScreen() {
  const body = `<div style="padding:16px 20px">
    ${field("任务名", input("清理过期会话", { w: "w-lg" }))}
    ${field("连接", `<div class="select w-lg"><span>localhost（MySQL）</span>${icon("chevron-down", { size: 14 })}</div>`)}
    ${field("数据库", input("password_manager", { w: "w-lg" }))}
    ${field("执行间隔", `<div class="select" style="width:160px"><span>每 1 小时</span>${icon("chevron-down", { size: 14 })}</div>`)}
    <div style="margin:-4px 0 12px 112px">${check("启用任务", true)}</div>
    <div style="margin-bottom:6px;font-size:12px;color:var(--text-secondary)">SQL</div>
    <div style="border:1px solid var(--border-strong);border-radius:4px;background:var(--bg-surface);font-family:var(--font-mono);font-size:13px;line-height:21px;padding:8px 10px;height:150px;color:var(--text)">
      <div>DELETE FROM session</div>
      <div>WHERE expires_at &lt; NOW()</div>
      <div style="color:var(--text-tertiary)">-- 每小时清理一次过期会话</div>
    </div>
    <div style="margin-top:10px">${note("info", "下次运行：2026-05-16 09:00:00（应用需保持运行）。")}</div>
  </div>`;

  return screen(
    "新建任务（自动运行）",
    "SQL 编辑区用等宽字体与独立边框；启用开关放在间隔下方；就地说明下次运行时间",
    atop(
      dimApp(),
      dialog({
        title: "新建任务",
        width: 600,
        body,
        footer: footer([{ label: "保存", kind: "primary" }, { label: "取消" }]),
      }),
    ),
    { w: 1100, h: 760 },
  );
}

/* ================================================================ 运行历史 */

export function historyScreen() {
  const cols = [
    { label: "任务", key: "job", width: 200 },
    { label: "开始时间", key: "start", width: 168 },
    { label: "状态", key: "state", width: 90 },
    { label: "耗时", key: "cost", width: 90, align: "right" },
    { label: "详情", key: "detail" },
  ];
  const rows = [
    ["清理过期会话", "2026-05-16 08:00:00", { html: chip("成功", "ok") }, "0.42 s", "删除 128 行"],
    ["清理过期会话", "2026-05-16 07:00:00", { html: chip("成功", "ok") }, "0.38 s", "删除 96 行"],
    ["同步作业日志", "2026-05-16 06:55:00", { html: chip("失败", "danger") }, "1.20 s", "Error 1213: Deadlock found"],
    ["同步作业日志", "2026-05-16 06:50:00", { html: chip("成功", "ok") }, "2.15 s", "更新 402 行"],
    ["清理过期会话", "2026-05-16 06:00:00", { html: chip("成功", "ok") }, "0.41 s", "删除 112 行"],
  ].map((r) => ({ job: r[0], start: r[1], state: r[2], cost: r[3], detail: { html: `<span style="color:${r[1].includes("06:55") ? "var(--danger-600)" : "var(--text-secondary)"}">${r[4]}</span>` } }));

  return screen(
    "运行历史",
    "760×460；表格占满主体，状态用语义 chip，失败详情用危险色；无行号列",
    atop(
      dimApp(),
      dialog({
        title: "运行历史",
        width: 760,
        height: 460,
        flush: true,
        body: `<div style="display:flex;flex-direction:column;height:100%">
          <div style="display:flex;align-items:center;gap:8px;padding:10px 16px;border-bottom:1px solid var(--border-subtle)">
            <span style="font-size:12px;font-weight:600">清理过期会话</span>
            ${chip("最近 5 次")}
            <div style="flex:1"></div>
            <button class="ptool" style="height:24px">${icon("refresh", { size: 13 })}<span>刷新</span></button>
          </div>
          ${grid(cols, rows, {})}
        </div>`,
        footer: footer([{ label: "关闭" }]),
      }),
    ),
    { w: 1200, h: 720 },
  );
}

/* ================================================================== 选项 */

export function settingsScreen() {
  const body = `<div style="padding:16px 20px">${[
    ["每页行数", `<input class="input" style="width:96px" value="1000" readonly>`],
    ["查询超时（秒）", `<input class="input" style="width:72px" value="30" readonly>`],
    ["最近文件数", `<input class="input" style="width:72px" value="10" readonly>`],
  ]
    .map(([l, c]) => field(l, c))
    .join("")}
    <div style="margin:-4px 0 0 112px;display:flex;flex-direction:column;gap:10px">
      ${check("启用自动补全", true)}
      ${check("关闭前确认", true)}
    </div>
  </div>`;

  return screen(
    "选项",
    "三个选项卡（常规 / 字体 / SQL 编辑器）用底部指示条；数字输入统一 72–96px 宽右对齐",
    atop(
      dimApp(),
      dialog({
        title: "选项",
        width: 520,
        tabs: ["常规", "字体", "SQL 编辑器"],
        activeTab: 0,
        body,
        footer: footer([{ label: "确定", kind: "primary" }, { label: "取消" }]),
      }),
    ),
    { w: 1080, h: 640 },
  );
}

/* ================================================================ 新建用户 */

export function userDialogScreen() {
  const body = `<div style="padding:16px 20px">
    ${field("用户名", input(""))}
    ${field("主机", input("%"))}
    <div style="margin:-8px 0 14px 112px;font-size:11px;color:var(--text-tertiary)">允许连接的主机，% 表示任意主机</div>
    ${field("密码", `<input class="input w-md" type="password" value="" readonly>`)}
    ${field("确认密码", `<input class="input w-md" type="password" value="" readonly>`)}
    <div style="margin-top:14px">${note("info", "新用户默认无任何库权限，创建后请在「权限管理员」中授权。")}</div>
  </div>`;

  return screen(
    "新建用户",
    "提示文字统一放在字段下方、对齐输入列（112px）；创建后给出下一步指引",
    atop(
      dimApp(),
      dialog({
        title: "新建用户",
        width: 480,
        body,
        footer: footer([{ label: "创建", kind: "primary" }, { label: "取消" }]),
      }),
    ),
    { w: 1040, h: 620 },
  );
}

/* ============================================================== 管理收藏夹 */

export function favoritesScreen() {
  const items = [
    ["dbswitch_calendars", "localhost / dbswitch", "table"],
    ["note_category", "localhost / password_manager", "table"],
    ["计算月度活跃用户", "localhost / password_manager", "code"],
  ];
  const body = `<div style="padding:16px 20px">
    <div class="picklist" style="max-height:280px">
      ${items
        .map(
          ([name, sub, ico]) => `<div class="pickrow" style="padding:8px 10px">
            <span style="color:#b5760a">${icon("star", { size: 14 })}</span>
            <span style="display:flex;flex-direction:column;gap:1px" class="grow">
              <span style="font-size:12px;color:var(--text)">${name}</span>
              <span style="font-size:11px;color:var(--text-tertiary)">${sub}</span>
            </span>
            ${chip(ico === "code" ? "查询" : "表")}
            <button class="btn ghost sm">${icon("trash", { size: 13 })}</button>
          </div>`,
        )
        .join("")}
    </div>
  </div>`;

  return screen(
    "管理收藏夹",
    "每项显示对象名 + 所属连接/库 + 类型 chip；删除按钮在行尾，双击行直接打开",
    atop(
      dimApp(),
      dialog({
        title: "管理收藏夹",
        width: 520,
        body,
        footer: footer([{ label: "关闭", kind: "primary" }]),
      }),
    ),
    { w: 1060, h: 620 },
  );
}

/* ================================================================== 关于 */

export function aboutScreen() {
  const body = `<div style="padding:24px 24px 16px;display:flex;flex-direction:column;align-items:center;gap:6px">
    <div style="display:grid;place-items:center;margin-bottom:6px">${navkitLogo(56)}</div>
    <div style="font-size:15px;font-weight:600">NavKit</div>
    <div style="font-size:12px;color:var(--text-tertiary)">版本 1.0.0</div>
    <div style="width:100%;margin-top:18px">
      <table class="kv">
        ${[
          ["版权所有", "Copyright © 2026 NavKit"],
          ["发行商", "NavKit"],
          ["网站", "https://example.com"],
          ["支持", "https://example.com/support"],
          ["数据目录", "C:\\Users\\gaohu\\AppData\\Roaming\\NavKit\\data"],
        ]
          .map(([k, v]) => `<tr><th>${k}</th><td>${v}</td></tr>`)
          .join("")}
      </table>
    </div>
  </div>`;

  return screen(
    "关于 NavKit",
    "图标用强调色圆角块；信息表统一 104px 键列；仅一个「确定」按钮",
    atop(
      dimApp(),
      dialog({
        title: "关于 NavKit",
        width: 420,
        body,
        footer: footer([{ label: "确定", kind: "primary" }]),
      }),
    ),
    { w: 1020, h: 640 },
  );
}

/* ============================================================== 服务器信息 */

export function serverInfoScreen() {
  const body = `<div style="padding:16px 20px">
    <table class="kv">
      ${[
        ["产品", "MySQL Community Server"],
        ["版本", "8.0.36 (MySQL Community Server - GPL)"],
        ["主机", "localhost"],
        ["端口", "3306"],
        ["字符集", "utf8mb4"],
        ["排序规则", "utf8mb4_0900_ai_ci"],
        ["运行时间", "12 天 4 小时 33 分"],
        ["连接数", "3 / 151"],
        ["数据目录", "/var/lib/mysql/"],
      ]
        .map(([k, v]) => `<tr><th>${k}</th><td>${v}</td></tr>`)
        .join("")}
    </table>
  </div>`;

  return screen(
    "服务器信息",
    "单一键值表，140px 键列，值列允许换行；只读信息不放按钮以外的操作",
    atop(
      dimApp(),
      dialog({
        title: "服务器信息",
        width: 520,
        body,
        footer: footer([{ label: "确定", kind: "primary" }]),
      }),
    ),
    { w: 1060, h: 640 },
  );
}

/* ================================================================ 确认操作 */

export function confirmDialogScreen() {
  const body = `<div style="padding:24px;display:flex;gap:14px;align-items:flex-start">
    <div style="width:38px;height:38px;border-radius:50%;background:var(--danger-50);color:var(--danger-500);display:grid;place-items:center;flex:0 0 auto">${icon("warning", { size: 20 })}</div>
    <div style="flex:1">
      <div style="font-size:13px;font-weight:600;margin-bottom:6px">确定要删除这 3 张表吗？</div>
      <div style="font-size:12px;line-height:1.65;color:var(--text-secondary)">dbswitch_account、dbswitch_locks、dbswitch_calendars 将被永久删除，其中的数据无法恢复。</div>
      <div style="margin-top:12px">${note("warn", "建议先执行「备份」或「SQL 转储」。")}</div>
    </div>
  </div>`;

  return screen(
    "确认对话框（危险操作）",
    "危险操作必须二次确认：危险色图标 + 具体影响对象清单 + 可撤销建议；主按钮用危险色",
    atop(
      dimApp(),
      dialog({
        title: "确认删除",
        width: 480,
        body,
        footer: footer([
          { label: "备份后删除", alignLeft: true },
          { label: "删除", kind: "danger" },
          { label: "取消", kind: "ghost" },
        ]),
      }),
    ),
    { w: 1040, h: 600 },
  );
}
