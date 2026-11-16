/** 驱动管理 · 重新设计（v3）
 *
 *  诊断（旧版）：
 *   1. 两个页签「托管驱动 / 自定义驱动」按实现划分，用户不知道差别；
 *   2. 自定义驱动内部又是三步向导，改一个字段要过三关；
 *   3. 官方驱动页每行一段长文案（来源/许可/限制全挤在状态行），扫不出重点；
 *   4. 下载/删除结果只写到底部一行 feedback，与操作对象脱节；
 *   5. 「推荐版本」只是标题后缀，没有视觉层级；被连接引用的版本删除时才报错。
 *
 *  新版结构：一个窗口 = 左清单（有什么/缺什么）+ 右详情（这个数据源怎么办）。
 *   「自定义驱动」降为页脚次级入口，向导改为单页表单。
 */

import { icon } from "./assets/icons.js";
import { dbLogo } from "./assets/db-logos.js";
import { navkitLogo } from "./assets/logo.js";
import { esc, chip, note, screen } from "./assets/ui.js";

/* ------------------------------------------------------------------ 外框 */

function dimApp() {
  return `<div style="flex:1;display:flex;flex-direction:column;min-height:0">
    <div style="height:36px;border-bottom:1px solid var(--border-subtle);background:var(--bg-surface);display:flex;align-items:center;padding:0 12px;gap:8px">
      <span style="display:grid;place-items:center">${navkitLogo(20)}</span>
      <span style="font-size:13px;font-weight:600">NavKit</span>
      <span style="width:1px;height:14px;background:var(--border)"></span>
      <span style="font-size:12px;color:var(--text-tertiary)">navkit — localhost</span>
    </div>
    <div style="height:44px;border-bottom:1px solid var(--border);background:var(--bg-surface)"></div>
    <div style="flex:1;display:flex;min-height:0;background:var(--bg-surface)">
      <div style="width:264px;border-right:1px solid var(--border)"></div>
      <div style="flex:1"></div>
      <div style="width:292px;border-left:1px solid var(--border)"></div>
    </div>
    <div style="height:26px;border-top:1px solid var(--border);background:var(--bg-surface)"></div>
  </div>`;
}

function atop(dlg) {
  return `<div class="window" style="position:relative">
    <div style="flex:1;display:flex;min-height:0">${dimApp()}</div>
    <div class="backdrop">${dlg}</div>
  </div>`;
}

/** 对话框骨架（自绘，便于控制内部栅格）。 */
function shell({ title, subtitle, toolbar, left, right, footerLeft, footerRight }) {
  return `<div class="dialog" style="width:1040px;height:700px">
    <div class="dialog-title">
      <span class="dialog-title-text">${esc(title)}</span>
      <button class="iconbtn">${icon("cross", { size: 13 })}</button>
    </div>
    ${
      subtitle
        ? `<div style="padding:8px 16px;border-bottom:1px solid var(--border-subtle);background:var(--bg-subtle);font-size:12px;color:var(--text-tertiary);line-height:1.55">${subtitle}</div>`
        : ""
    }
    ${toolbar || ""}
    <div class="dialog-body flush" style="display:flex;min-height:0">
      <div style="width:296px;flex:0 0 296px;border-right:1px solid var(--border);display:flex;flex-direction:column;min-height:0">${left}</div>
      <div style="flex:1;min-width:0;display:flex;flex-direction:column;min-height:0">${right}</div>
    </div>
    <div class="dialog-footer">
      ${footerLeft || ""}
      <div class="spacer"></div>
      ${footerRight || ""}
    </div>
  </div>`;
}

/* -------------------------------------------------------------- 左栏清单 */

const STATE_CHIP = {
  ready: ["已下载", "ok"],
  partial: ["部分就绪", "accent"],
  missing: ["未下载", ""],
  manual: ["需手动", "warn"],
};

function stateChip(state) {
  const [label, kind] = STATE_CHIP[state];
  return chip(label, kind);
}

function listRow(r) {
  const logo = dbLogo(r.kind, 18);
  const bg = r.active ? "background:var(--bg-selected);" : "";
  const bar = r.active
    ? '<span style="position:absolute;left:0;top:4px;bottom:4px;width:2px;border-radius:0 2px 2px 0;background:var(--accent-500)"></span>'
    : "";
  const nameColor = r.active ? "color:var(--accent-600);font-weight:600;" : "color:var(--text);";
  return `<div style="position:relative;display:flex;align-items:center;gap:8px;height:32px;padding:0 12px 0 14px;border-radius:0;${bg}">
      ${bar}
      <span style="width:18px;height:18px;display:grid;place-items:center;flex:0 0 auto">${
        logo || `<span style="color:var(--text-tertiary)">${icon("database", { size: 15 })}</span>`
      }</span>
      <span style="flex:1;min-width:0;font-size:12px;${nameColor}overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(r.label)}</span>
      ${r.sub ? `<span style="font-size:11px;color:var(--text-tertiary);font-variant-numeric:tabular-nums">${esc(r.sub)}</span>` : ""}
      ${stateChip(r.state)}
    </div>`;
}

function leftPanel(groups, footerNote) {
  const body = groups
    .map(
      (g) => `<div style="padding:10px 0 2px">
        <div style="padding:0 14px 4px;font-size:11px;font-weight:600;letter-spacing:.04em;color:var(--text-tertiary)">${esc(g.title)}</div>
        ${g.rows.map(listRow).join("")}
      </div>`,
    )
    .join("");
  return `<div style="flex:1;min-height:0;overflow:auto;padding-bottom:8px">${body}</div>
    <div style="flex:0 0 auto;padding:8px 14px;border-top:1px solid var(--border-subtle);background:var(--bg-subtle);font-size:11px;color:var(--text-tertiary);line-height:1.5">${footerNote}</div>`;
}

/* -------------------------------------------------------------- 顶部工具条 */

function toolbar(activeFilter = 0) {
  const filters = [
    ["全部", "12"],
    ["已就绪", "5"],
    ["缺少", "4"],
    ["需手动", "3"],
  ];
  return `<div style="display:flex;align-items:center;gap:8px;height:40px;padding:0 16px;border-bottom:1px solid var(--border-subtle);background:var(--bg-surface);flex:0 0 auto">
    <div class="search" style="margin:0;flex:0 1 240px;height:26px">${icon("search", { size: 13 })}<input placeholder="搜索数据源名称" readonly></div>
    <div style="display:flex;gap:1px;padding:2px;border-radius:4px;background:var(--bg-sunken)">
      ${filters
        .map(
          ([label, n], i) => `<button class="iconbtn${i === activeFilter ? " active" : ""}" style="width:auto;height:22px;padding:0 8px;font-size:11px;gap:4px;display:inline-flex;align-items:center">${label}<span style="color:var(--text-tertiary);font-variant-numeric:tabular-nums">${n}</span></button>`,
        )
        .join("")}
    </div>
    <div style="flex:1"></div>
    <span style="font-size:11px;color:var(--text-tertiary)">${icon("database", { size: 12 })}</span>
    <span style="font-size:11px;color:var(--text-tertiary)">架构 x86_64</span>
    <span style="width:1px;height:16px;background:var(--border)"></span>
    <button class="ptool" style="height:26px">${icon("refresh", { size: 13 })}<span>重新检测</span></button>
  </div>`;
}

/* -------------------------------------------------------------- 右栏详情 */

function detailHead({ kind, label, meta, state, action }) {
  const logo = dbLogo(kind, 30);
  return `<div style="padding:16px 20px 14px;border-bottom:1px solid var(--border-subtle);display:flex;gap:12px;align-items:flex-start">
    <span style="width:30px;height:30px;display:grid;place-items:center;flex:0 0 auto">${
      logo || `<span style="color:var(--text-secondary)">${icon("database", { size: 26 })}</span>`
    }</span>
    <div style="flex:1;min-width:0">
      <div style="display:flex;align-items:center;gap:8px">
        <span style="font-size:15px;font-weight:600">${esc(label)}</span>
        ${stateChip(state)}
      </div>
      <div style="margin-top:3px;font-size:11px;color:var(--text-tertiary);line-height:1.6">${meta}</div>
    </div>
    ${action || ""}
  </div>`;
}

function sectionTitle(text, right = "") {
  return `<div style="display:flex;align-items:center;gap:8px;padding:14px 20px 6px">
    <span style="font-size:11px;font-weight:600;letter-spacing:.04em;color:var(--text-tertiary)">${esc(text)}</span>
    <div style="flex:1"></div>${right}
  </div>`;
}

const VER_COLS = "1fr 88px 76px 108px 92px";

function versionHeader() {
  const cols = ["版本", "来源", "大小", "被连接引用", ""];
  return `<div style="display:grid;grid-template-columns:${VER_COLS};gap:10px;padding:0 20px 6px;font-size:11px;color:var(--text-tertiary)">
    ${cols.map((c) => `<span>${esc(c)}</span>`).join("")}
  </div>`;
}

/** 版本行。tone: ready | missing | busy | locked */
function versionRow({ name, desc, recommended, source, size, refs, tone, action }) {
  const isLocked = tone === "locked";
  const rowBg = tone === "busy" ? "background:var(--accent-50);" : "";
  return `<div style="display:grid;grid-template-columns:${VER_COLS};gap:10px;align-items:center;padding:9px 20px;border-top:1px solid var(--border-subtle);${rowBg}">
    <div style="min-width:0">
      <div style="display:flex;align-items:center;gap:6px">
        <span style="font-size:12px;font-weight:600;color:var(--text)">${esc(name)}</span>
        ${recommended ? chip("推荐", "accent") : ""}
      </div>
      <div style="margin-top:2px;font-size:11px;color:var(--text-tertiary);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${desc}</div>
    </div>
    <span style="font-size:11px;color:var(--text-secondary)">${esc(source)}</span>
    <span style="font-size:11px;color:var(--text-secondary);font-variant-numeric:tabular-nums">${esc(size)}</span>
    <span style="font-size:11px;color:var(--text-tertiary)">${refs || "—"}</span>
    <div style="display:flex;justify-content:flex-end">${action}</div>
  </div>`;
}

function btn(label, kind = "", opts = {}) {
  const cls = ["btn", kind, opts.sm ? "sm" : ""].filter(Boolean).join(" ");
  return `<button class="${cls}"${opts.disabled ? ' disabled style="opacity:.5"' : ""}>${label}</button>`;
}

/* ---------------------------------------------------------- 屏 1：主界面 */

const GROUPS = [
  {
    title: "关系型数据库",
    rows: [
      { kind: "oracle", label: "Oracle", state: "partial", sub: "1/2", active: true },
      { kind: "firebird", label: "Firebird", state: "missing" },
      { kind: "dameng", label: "达梦 DM8", state: "manual" },
      { kind: "sundb", label: "SUNDB", state: "manual" },
      { kind: "oscar", label: "神通 OSCAR", state: "missing" },
    ],
  },
  {
    title: "数仓与分析",
    rows: [
      { kind: "duckdb", label: "DuckDB", state: "ready" },
      { kind: "trino", label: "Trino", state: "partial", sub: "1/2" },
      { kind: "bigquery", label: "BigQuery", state: "missing" },
      { kind: "snowflake", label: "Snowflake", state: "manual" },
    ],
  },
  {
    title: "其它",
    rows: [{ kind: "cache", label: "InterSystems Caché", state: "missing" }],
  },
];

export function driverManagerScreen() {
  const right = `${detailHead({
    kind: "oracle",
    label: "Oracle",
    meta: "关系型数据库 · 支持 ODBC 原生驱动与 thin JDBC 两条通道 · 共 2 个已核验版本",
    state: "partial",
    action: `<button class="ptool" style="height:26px">${icon("open", { size: 13 })}<span>打开驱动目录</span></button>`,
  })}
    <div style="flex:1;min-height:0;overflow:auto">
      ${sectionTitle("已下载", `<span style="font-size:11px;color:var(--text-tertiary)">1 个 · 占用 18.4 MB</span>`)}
      ${versionHeader()}
      ${versionRow({
        name: "v23.26.0",
        desc: "Oracle ODBC 完整包 · 免安装 · 原样保留厂商许可",
        recommended: true,
        source: "官方源",
        size: "18.4 MB",
        refs: chip("2 个连接在用", "warn"),
        tone: "locked",
        action: btn("删除", "ghost sm", { disabled: true }),
      })}
      <div style="padding:0 20px 2px;font-size:11px;color:var(--text-tertiary);line-height:1.5">被连接引用的版本不能删除；先在连接属性里改用其它版本。</div>

      ${sectionTitle("可下载", `<span style="font-size:11px;color:var(--text-tertiary)">校验 SHA-256 后入库</span>`)}
      ${versionRow({
        name: "thin JDBC v23.26",
        desc: "ojdbc11 · 8.8 MB · 需要 Java 11+",
        recommended: false,
        source: "官方源",
        size: "8.8 MB",
        refs: "",
        tone: "missing",
        action: btn("下载", "primary sm"),
      })}
      ${versionRow({
        name: "ODBC 21.13",
        desc: "历史版本 · 仅用于兼容旧版服务器",
        recommended: false,
        source: "官方源",
        size: "17.9 MB",
        refs: "",
        tone: "missing",
        action: btn("下载", "sm"),
      })}

      <div style="padding:14px 20px 20px">
        ${note("info", "下载只保存免安装驱动并校验完整性，不运行安装器、不接受交互式许可。ODBC 包仍需系统安装 VC++ 2015–2022 x64 运行库。")}
      </div>
    </div>`;

  const dlg = shell({
    title: "驱动管理",
    subtitle:
      "左边是全部需要外部驱动的数据源，右边是选中数据源的版本与操作。内置协议（MySQL / PostgreSQL 等）不在此列出。",
    toolbar: toolbar(0),
    left: leftPanel(GROUPS, `驱动保存在 <span style="font-family:var(--font-mono)">data/managed-drivers</span>，随便携包一起复制。`),
    right,
    footerLeft: `<button class="btn ghost">${icon("plus", { size: 13 })}<span style="margin-left:4px">添加自定义驱动…</span></button>
      <span style="font-size:11px;color:var(--text-tertiary);margin-left:2px">JDBC JAR / ODBC 原生库 / DSN</span>`,
    footerRight: `<button class="btn">关闭</button>`,
  });

  return screen(
    "驱动管理 · 主界面",
    "左清单回答「我装了什么、还缺什么」，右详情回答「这个数据源现在能怎么办」。状态用徽标表达，操作只在行尾出现一次。",
    atop(dlg),
    { w: 1180, h: 800 },
  );
}

/* ------------------------------------------------- 屏 2：需手动获取的数据源 */

export function driverManualScreen() {
  const right = `${detailHead({
    kind: "dameng",
    label: "达梦 DM8",
    meta: "关系型数据库 · ODBC 原生驱动 · 厂商授权限制，NavKit 不能代下载",
    state: "manual",
    action: "",
  })}
    <div style="flex:1;min-height:0;overflow:auto;padding:0 20px 20px">
      <div style="margin-top:16px;border:1px solid var(--warn-100);border-radius:6px;background:var(--warn-50);padding:14px 16px;display:flex;gap:10px">
        <span style="color:var(--warn-500);flex:0 0 auto;margin-top:1px">${icon("warning", { size: 16 })}</span>
        <div style="min-width:0">
          <div style="font-size:12px;font-weight:600;color:var(--warn-600);margin-bottom:4px">这个驱动需要你手动获取</div>
          <div style="font-size:12px;line-height:1.7;color:var(--text-secondary)">达梦的 ODBC 驱动只随官方客户端发行，没有可公开直连的下载地址，也没有再分发授权。NavKit 不会替你下载或安装它。</div>
        </div>
      </div>

      <div style="margin-top:16px;font-size:11px;font-weight:600;letter-spacing:.04em;color:var(--text-tertiary)">按顺序做这三步</div>

      <div style="margin-top:8px;border:1px solid var(--border);border-radius:6px;overflow:hidden">
        ${[
          ["1", "从官网下载并安装达梦客户端", "安装时勾选 ODBC 驱动组件，默认路径 D:\\dmdbms\\drivers\\odbc"],
          ["2", "回到这里点「重新检测」", "NavKit 会在系统 ODBC 清单里查找 DM8 ODBC DRIVER，找到后本页变为「已下载」"],
          ["3", "在新建连接里选择 DM8", "连接类型选 ODBC，驱动名填 DM8 ODBC DRIVER"],
        ]
          .map(
            ([n, title, desc], i) => `<div style="display:flex;gap:12px;padding:12px 16px;${i ? "border-top:1px solid var(--border-subtle)" : ""}">
              <span style="width:20px;height:20px;border-radius:50%;background:var(--bg-sunken);color:var(--text-secondary);font-size:11px;font-weight:600;display:grid;place-items:center;flex:0 0 auto">${n}</span>
              <div style="min-width:0">
                <div style="font-size:12px;font-weight:600">${esc(title)}</div>
                <div style="margin-top:2px;font-size:11px;line-height:1.6;color:var(--text-tertiary)">${esc(desc)}</div>
              </div>
            </div>`,
          )
          .join("")}
      </div>

      <div style="display:flex;gap:8px;margin-top:14px">
        ${btn("复制获取说明", "", { sm: false })}
        <div style="flex:1"></div>
        ${btn("我装好了，重新检测", "primary", { sm: false })}
      </div>

      <div style="margin-top:18px">
        ${sectionTitle("当前检测结果", "")}
        <table class="kv" style="margin-top:2px">
          ${[
            ["架构", "x86_64（与 NavKit 一致）"],
            ["已装 ODBC 驱动", "3 个，均不含 DM8"],
            ["系统 DSN", "0 个"],
            ["最后检测", "2026-11-07 11:42:08"],
          ]
            .map(([k, v]) => `<tr><th>${esc(k)}</th><td>${esc(v)}</td></tr>`)
            .join("")}
        </table>
      </div>
    </div>`;

  const groups = GROUPS.map((g) => ({
    ...g,
    rows: g.rows.map((r) => (r.kind === "dameng" ? { ...r, active: true } : { ...r, active: false })),
  }));

  const dlg = shell({
    title: "驱动管理",
    subtitle: "",
    toolbar: toolbar(3),
    left: leftPanel(groups, `驱动保存在 <span style="font-family:var(--font-mono)">data/managed-drivers</span>，随便携包一起复制。`),
    right,
    footerLeft: `<button class="btn ghost">${icon("plus", { size: 13 })}<span style="margin-left:4px">添加自定义驱动…</span></button>
      <span style="font-size:11px;color:var(--text-tertiary);margin-left:2px">JDBC JAR / ODBC 原生库 / DSN</span>`,
    footerRight: `<button class="btn">关闭</button>`,
  });

  return screen(
    "驱动管理 · 需手动获取",
    "不能自动下载时不要只给一句错误文案：说清为什么、给出可执行的三步、并把「重新检测」放在同一屏。",
    atop(dlg),
    { w: 1180, h: 800 },
  );
}

/* ------------------------------------------------------ 屏 3：状态与反馈 */

function stateCell({ tone, title, body, action, chipText, chipKind }) {
  const dot = {
    ready: "var(--ok-500)",
    missing: "var(--n-400)",
    manual: "var(--warn-500)",
    busy: "var(--accent-500)",
    locked: "var(--n-300)",
    error: "var(--danger-500)",
  }[tone];
  return `<div style="display:flex;flex-direction:column;gap:8px;padding:14px 16px;border:1px solid var(--border);border-radius:6px;background:var(--bg-surface)">
    <div style="display:flex;align-items:center;gap:8px">
      <span style="width:7px;height:7px;border-radius:50%;background:${dot};flex:0 0 auto"></span>
      <span style="font-size:12px;font-weight:600">${esc(title)}</span>
      ${chipText ? chip(chipText, chipKind || "") : ""}
    </div>
    <div style="font-size:11px;line-height:1.65;color:var(--text-tertiary)">${body}</div>
    <div style="margin-top:auto;padding-top:4px">${action || ""}</div>
  </div>`;
}

export function driverStatesScreen() {
  const grid = `<div style="display:grid;grid-template-columns:repeat(3,1fr);gap:12px">
    ${stateCell({
      tone: "ready",
      title: "已下载 · 可用",
      chipText: "已下载",
      chipKind: "ok",
      body: "文件完整、指纹校验通过，连接时直接使用。行尾只有「删除」。",
      action: `<div style="display:flex;gap:6px;align-items:center"><span style="font-size:11px;color:var(--text-tertiary)">v2.0.0.1 · 14.1 MB</span><div style="flex:1"></div>${btn("删除", "ghost sm")}</div>`,
    })}
    ${stateCell({
      tone: "missing",
      title: "未下载 · 可自动获取",
      chipText: "未下载",
      body: "有已核验直链。主按钮是「下载」，不需要用户去猜来源。",
      action: `<div style="display:flex;gap:6px;align-items:center"><span style="font-size:11px;color:var(--text-tertiary)">v476 · 12.7 MB</span><div style="flex:1"></div>${btn("下载", "primary sm")}</div>`,
    })}
    ${stateCell({
      tone: "manual",
      title: "需手动获取",
      chipText: "需手动",
      chipKind: "warn",
      body: "无直链或受授权限制。按钮改为「获取方式」，不假装能下载。",
      action: `<div style="display:flex;gap:6px;align-items:center"><span style="font-size:11px;color:var(--text-tertiary)">授权限制</span><div style="flex:1"></div>${btn("获取方式", "sm")}</div>`,
    })}
    ${stateCell({
      tone: "busy",
      title: "下载中",
      chipText: "下载中",
      chipKind: "accent",
      body: `后端只返回最终结果、没有进度事件，所以用不确定态进度条 + 阶段文案，不编造百分比，也不给点了没反应的「取消」。`,
      action: `<div style="display:flex;flex-direction:column;gap:6px">
        <div style="height:4px;border-radius:2px;background:var(--bg-sunken);overflow:hidden"><div style="width:38%;height:100%;background:var(--accent-500)"></div></div>
        <div style="display:flex;align-items:center;gap:6px"><span style="font-size:11px;color:var(--text-tertiary)">正在下载并校验完整性…</span><div style="flex:1"></div>${btn("下载中…", "ghost sm", { disabled: true })}</div>
      </div>`,
    })}
    ${stateCell({
      tone: "locked",
      title: "被连接引用",
      chipText: "2 个连接在用",
      chipKind: "warn",
      body: "删除按钮置灰，并在行下方写明原因与解除办法；不让用户点下去才报错。",
      action: `<div style="display:flex;gap:6px;align-items:center"><span style="font-size:11px;color:var(--text-tertiary)">navkit、报表库</span><div style="flex:1"></div>${btn("删除", "ghost sm", { disabled: true })}</div>`,
    })}
    ${stateCell({
      tone: "error",
      title: "下载失败",
      chipText: "失败",
      chipKind: "danger",
      body: "错误留在该行内，附「重试」；不把失败写进全局提示条后就消失。",
      action: `<div style="display:flex;gap:6px;align-items:center"><span style="font-size:11px;color:var(--danger-600)">校验和不匹配</span><div style="flex:1"></div>${btn("重试", "sm")}</div>`,
    })}
  </div>`;

  const rules = `<div style="margin-top:18px;display:grid;grid-template-columns:1fr 1fr;gap:12px">
    <div style="border:1px solid var(--border);border-radius:6px;padding:14px 16px">
      <div style="font-size:12px;font-weight:600;margin-bottom:8px">文案规则</div>
      <ul style="margin:0;padding-left:16px;font-size:11px;line-height:1.9;color:var(--text-secondary)">
        <li>状态只用一个词：<b>已下载 / 未下载 / 部分就绪 / 需手动获取 / 下载中 / 下载失败</b>，不再出现「已核验可用」「未确认完整」这类同义反复。</li>
        <li>来源、许可、限制属于详情，放在展开区或二级说明，不进列表行。</li>
        <li>每个按钮只做一件事，文案是动词：「下载」「删除」「获取方式」。</li>
        <li>同一屏里禁止出现两个都叫「下载」的按钮指向不同对象。</li>
      </ul>
    </div>
    <div style="border:1px solid var(--border);border-radius:6px;padding:14px 16px">
      <div style="font-size:12px;font-weight:600;margin-bottom:8px">布局规则</div>
      <ul style="margin:0;padding-left:16px;font-size:11px;line-height:1.9;color:var(--text-secondary)">
        <li>左清单固定 288px，右详情自适应；清单项 30px 一行，一屏可见 14 个数据源。</li>
        <li>徽标永远在行尾同一列，扫读时成一条竖线。</li>
        <li>主操作（下载）用强调色，删除用幽灵按钮，危险操作才用红色。</li>
        <li>反馈就近：下载进度在该行，失败也在该行，不做全局 toast 了事。</li>
      </ul>
    </div>
  </div>`;

  const body = `<div class="window" style="padding:0">
    <div style="flex:1;min-height:0;padding:20px 24px 24px;overflow:auto">
      <div style="font-size:15px;font-weight:600;margin-bottom:4px">驱动行的六种状态</div>
      <div style="font-size:12px;color:var(--text-tertiary);margin-bottom:16px;line-height:1.6">每一行都必须能回答：现在能不能用、下一步点哪里、点了会发生什么。</div>
      ${grid}
      ${rules}
    </div>
  </div>`;

  return screen(
    "驱动管理 · 状态与文案矩阵",
    "旧版把来源、许可、限制全塞进一行状态文案；新版把「状态」压成一个词，把细节挪到详情。",
    body,
    { w: 1180, h: 720 },
  );
}

/* ------------------------------------------------- 屏 4：自定义驱动（单页） */

export function customDriverScreen() {
  const saved = [
    { name: "MySQL 8 本地 JAR", meta: "JDBC · 本地 JAR", active: true },
    { name: "公司 Oracle 薄驱动", meta: "JDBC · 本地 JAR" },
    { name: "旧版 SQL Server", meta: "ODBC · 已安装驱动" },
    { name: "测试 DSN", meta: "ODBC · DSN" },
  ];

  const left = `<div style="padding:12px 12px 8px;display:flex;gap:6px">
      <div class="search" style="margin:0;flex:1;height:26px">${icon("search", { size: 13 })}<input placeholder="搜索驱动名称" readonly></div>
    </div>
    <div style="flex:1;min-height:0;overflow:auto;padding:0 8px 8px">
      ${saved
        .map(
          (d) => `<div style="display:flex;flex-direction:column;gap:1px;padding:7px 10px;border-radius:4px;${d.active ? "background:var(--bg-selected)" : ""}">
            <span style="font-size:12px;${d.active ? "color:var(--accent-600);font-weight:600" : "color:var(--text)"}">${esc(d.name)}</span>
            <span style="font-size:11px;color:var(--text-tertiary)">${esc(d.meta)}</span>
          </div>`,
        )
        .join("")}
    </div>
    <div style="padding:8px 12px;border-top:1px solid var(--border-subtle)">
      <button class="btn ghost" style="width:100%">${icon("plus", { size: 13 })}<span style="margin-left:4px">新建驱动</span></button>
    </div>`;

  const fieldRow = (label, control, hint) => `<div style="display:grid;grid-template-columns:96px 1fr;gap:10px;align-items:start;margin-bottom:12px">
      <label style="font-size:12px;color:var(--text-secondary);text-align:right;padding-top:6px">${esc(label)}</label>
      <div style="min-width:0">
        ${control}
        ${hint ? `<div style="margin-top:4px;font-size:11px;line-height:1.5;color:var(--text-tertiary)">${hint}</div>` : ""}
      </div>
    </div>`;

  const input = (v, ph) => `<input class="input" style="width:100%;height:26px" value="${esc(v)}" placeholder="${esc(ph || "")}" readonly>`;

  const right = `<div style="display:flex;flex-direction:column;min-height:0;flex:1">
    <div style="padding:14px 20px 10px;border-bottom:1px solid var(--border-subtle);display:flex;align-items:center;gap:10px">
      <span style="font-size:13px;font-weight:600">MySQL 8 本地 JAR</span>
      ${chip("JDBC", "accent")}
      <div style="flex:1"></div>
      <span style="font-size:11px;color:var(--text-tertiary)">已保存 · 无引用</span>
    </div>
    <div style="flex:1;min-height:0;overflow:auto;padding:16px 20px">
      <div style="font-size:11px;font-weight:600;letter-spacing:.04em;color:var(--text-tertiary);margin-bottom:10px">基本信息</div>
      ${fieldRow("显示名称", input("MySQL 8 本地 JAR"))}
      ${fieldRow(
        "驱动类型",
        `<div style="display:flex;gap:6px">
          <button class="btn sm" style="box-shadow:inset 0 0 0 2px var(--accent-500);color:var(--accent-600)">JDBC（.jar）</button>
          <button class="btn sm">ODBC（原生库）</button>
        </div>`,
        "JDBC 需要 Java 11+；ODBC 只加载与 NavKit 同架构的原生库。",
      )}
      ${fieldRow(
        "驱动文件",
        `<div style="display:flex;gap:6px">
          <input class="input mono" style="flex:1;height:26px" value="C:\\drivers\\mysql-connector-j-9.1.0.jar" readonly>
          <button class="btn">选择…</button>
        </div>`,
        "文件不会复制，只记录路径；便携包换机器时需重新指定。",
      )}
      ${fieldRow("驱动类名", input("com.mysql.cj.jdbc.Driver"), "留空则交给 DriverManager 自动发现；自动发现失败时再填。")}

      <div style="height:1px;background:var(--border-subtle);margin:16px 0"></div>
      <div style="font-size:11px;font-weight:600;letter-spacing:.04em;color:var(--text-tertiary);margin-bottom:10px">连接模板</div>
      ${fieldRow(
        "URL 模板",
        `<textarea class="input mono" rows="2" style="width:100%;resize:vertical;padding:6px 8px;line-height:1.6" readonly>jdbc:mysql://{{host}}:{{port}}/{{database}}</textarea>`,
        "可用占位符：{{host}} {{port}} {{database}} {{user}} {{password}}；值由后端转义，不要再套大括号。",
      )}
      ${fieldRow("验证 SQL", input("SELECT 1"))}
    </div>
    <div style="flex:0 0 auto;border-top:1px solid var(--border-subtle);background:var(--bg-subtle);padding:10px 20px;display:flex;align-items:center;gap:8px">
      <button class="btn ghost">${icon("trash", { size: 13 })}<span style="margin-left:4px">删除</span></button>
      <div style="flex:1"></div>
      <span style="font-size:11px;color:var(--text-tertiary);margin-right:4px">${icon("success", { size: 12 })} 上次验证：连接成功（2026-11-06 22:14）</span>
      <button class="btn">验证连接…</button>
      <button class="btn primary">保存</button>
    </div>
  </div>`;

  const dlg = `<div class="dialog" style="width:940px;height:620px">
    <div class="dialog-title">
      <span class="dialog-title-text">自定义驱动</span>
      <button class="iconbtn">${icon("cross", { size: 13 })}</button>
    </div>
    <div style="padding:8px 16px;border-bottom:1px solid var(--border-subtle);background:var(--bg-subtle);font-size:12px;color:var(--text-tertiary);line-height:1.55">
      这里放 NavKit 不能替你下载的驱动：厂商 JAR、公司内部 ODBC 库、已注册的 DSN。官方驱动请回上一页。
    </div>
    <div class="dialog-body flush" style="display:flex;min-height:0">
      <div style="width:248px;flex:0 0 248px;border-right:1px solid var(--border);display:flex;flex-direction:column;min-height:0">${left}</div>
      ${right}
    </div>
    <div class="dialog-footer">
      <span style="font-size:11px;color:var(--text-tertiary)">保存在 <span style="font-family:var(--font-mono)">data/drivers.json</span>，随便携包一起复制</span>
      <div class="spacer"></div>
      <button class="btn">关闭</button>
    </div>
  </div>`;

  return screen(
    "自定义驱动 · 单页表单",
    "原来三步向导（选择→配置→验证保存）压成一页：左边选，右边改，底部验证与保存。字段一屏看全，不再有「下一步」的仪式感。",
    atop(dlg),
    { w: 1100, h: 740 },
  );
}

/* --------------------------------------------------------- 屏 5：改版说明 */

export function driverRationaleScreen() {
  const rows = [
    [
      "两个页签按实现划分：托管 / 自定义",
      "用户不知道「托管」指什么",
      "一个窗口：官方驱动清单 + 页脚「添加自定义驱动…」次级入口",
    ],
    [
      "自定义驱动内部再套三步向导",
      "改一个字段要连点三次下一步",
      "单页表单，左列表右详情，底部验证/保存",
    ],
    [
      "每行一段长文案（来源·许可·限制·状态）",
      "扫不出「能用吗」",
      "状态压成一个词 + 徽标；来源/大小/引用进表格列；限制进详情说明",
    ],
    [
      "「（推荐）」只是标题后缀",
      "看不出哪个该装",
      "推荐版本置顶 + 强调色 chip；其余版本折叠在下方",
    ],
    [
      "删除被引用的版本 → 点下去才报错",
      "误操作 + 挫败感",
      "行内显示「N 个连接在用」，删除按钮置灰并写明解除办法",
    ],
    [
      "下载/删除结果只写到底部一行 feedback",
      "不知道是哪一行出的结果",
      "反馈就近：进度条在行内，失败在该行附重试",
    ],
    [
      "需手动获取的数据源只给一句原因",
      "用户卡住，不知道下一步",
      "专门的详情页：为什么不能下 + 三步操作 + 重新检测",
    ],
    [
      "架构、ODBC 清单信息堆在页面顶部小字",
      "重要环境信息被忽略",
      "移入详情页「当前检测结果」表格，按需查看",
    ],
  ];

  const table = `<table class="kv" style="width:100%;font-size:12px">
    <thead><tr>
      <th style="width:34%;font-size:11px;letter-spacing:.04em">旧版问题</th>
      <th style="width:22%;font-size:11px;letter-spacing:.04em">造成的困扰</th>
      <th style="font-size:11px;letter-spacing:.04em">新版做法</th>
    </tr></thead>
    <tbody>
      ${rows
        .map(
          ([a, b, c]) => `<tr>
            <td style="color:var(--text)">${esc(a)}</td>
            <td style="color:var(--danger-600)">${esc(b)}</td>
            <td style="color:var(--text-secondary)">${esc(c)}</td>
          </tr>`,
        )
        .join("")}
    </tbody>
  </table>`;

  const principles = [
    ["一个页面只回答一个问题", "「我缺什么驱动」和「怎么装这个驱动」是两件事，分成左清单与右详情，而不是两个页签。"],
    ["状态词唯一", "已下载 / 未下载 / 需手动 / 下载中 / 失败。同义词只会让人怀疑它们有区别。"],
    ["操作紧贴对象", "按钮在该行行尾，结果也在该行。全局提示条只用于「检测完成」这类与单行无关的事件。"],
    ["不假装能做的事", "没有直链就写「需手动」，不给一个点了会弹错误框的「下载」按钮。"],
  ];

  const body = `<div class="window">
    <div style="flex:1;min-height:0;padding:24px;overflow:auto">
      <div style="font-size:15px;font-weight:600;margin-bottom:4px">驱动管理改版依据</div>
      <div style="font-size:12px;color:var(--text-tertiary);margin-bottom:16px;line-height:1.6">旧版的每个问题都对应一条具体的界面决策，而不是「重画一遍好看点」。</div>
      <div style="border:1px solid var(--border);border-radius:6px;overflow:hidden;background:var(--bg-surface);padding:4px 12px 8px">${table}</div>
      <div style="display:grid;grid-template-columns:repeat(4,1fr);gap:12px;margin-top:16px">
        ${principles
          .map(
            ([t, d]) => `<div style="border:1px solid var(--border);border-radius:6px;padding:14px;background:var(--bg-surface)">
              <div style="font-size:12px;font-weight:600;margin-bottom:6px">${esc(t)}</div>
              <div style="font-size:11px;line-height:1.7;color:var(--text-tertiary)">${esc(d)}</div>
            </div>`,
          )
          .join("")}
      </div>
    </div>
  </div>`;

  return screen(
    "驱动管理 · 改版依据",
    "八条旧问题 → 八条对应改法；四条贯穿全部界面的原则。",
    body,
    { w: 1180, h: 700 },
  );
}
