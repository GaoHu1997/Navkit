/**
 * 设计稿公共构建器 —— 全部返回 HTML 字符串，屏幕文件只负责组合。
 * 所有尺寸来自 tokens.css，禁止在屏幕文件里写魔法数字。
 */

import { icon, ICON_COLORS } from "./icons.js";
import { pictIcon } from "./pict.js";
import { dbLogo } from "./db-logos.js";
import { navkitLogo } from "./logo.js";

/* ------------------------------------------------------------------ 基础 */

export function esc(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
}

export function iconEl(name, size = 16, cls = "") {
  return icon(name, { size, className: cls });
}

/** 复选框 */
export function check(label, on = false, opts = {}) {
  return `<span class="check${on ? " on" : ""}${opts.disabled ? " disabled" : ""}">
    <span class="box">${icon("check", { size: 11, stroke: 2.6 })}</span>${label ? `<span>${esc(label)}</span>` : ""}
  </span>`;
}

/** 单选 */
export function radio(label, on = false) {
  return `<span class="radio${on ? " on" : ""}"><span class="dot"></span><span>${esc(label)}</span></span>`;
}

/** 字段行 */
export function field(label, control, opts = {}) {
  return `<div class="field"${opts.labelW ? ` style="--label-w:${opts.labelW}px"` : ""}>
    <label>${esc(label)}</label><div class="ctl">${control}</div>
  </div>`;
}

export function input(value = "", opts = {}) {
  const cls = ["input", opts.w || "w-md", opts.mono ? "mono" : ""].filter(Boolean).join(" ");
  const ph = opts.placeholder ? ` placeholder="${esc(opts.placeholder)}"` : "";
  return `<input class="${cls}" value="${esc(value)}"${ph} readonly>`;
}

export function select(value = "", opts = {}) {
  const w = opts.width ? ` style="width:${opts.width}px"` : "";
  const caret = opts.noCaret ? "" : icon("chevron-down", { size: 14 });
  return `<div class="select${opts.w === "w-sm" ? " w-sm" : ""}"${w}><span>${esc(value)}</span>${caret}</div>`;
}

export function button(label, opts = {}) {
  const cls = ["btn", opts.kind || "", opts.size || ""].filter(Boolean).join(" ");
  const ico = opts.icon ? icon(opts.icon, { size: opts.size === "sm" ? 13 : 15 }) : "";
  return `<button class="${cls}"${opts.disabled ? " disabled" : ""}>${ico}${esc(label)}</button>`;
}

/* -------------------------------------------------------------- 应用框架 */

const MENU_ITEMS = ["文件", "编辑", "查看", "工具", "设置"];

export function titlebar(opts = {}) {
  const menu = (opts.menu ?? MENU_ITEMS)
    .map((m) => {
      const open = opts.openMenu === m;
      const pop = open && opts.menuPopup ? `<div class="menubar-pop">${opts.menuPopup}</div>` : "";
      return `<div class="menubar-item${open ? " open" : ""}">${m}${pop}</div>`;
    })
    .join("");
  return `<header class="titlebar">
    <div class="titlebar-logo">${navkitLogo(20)}</div>
    <div class="titlebar-name">NavKit</div>
    <div class="titlebar-sep"></div>
    <nav class="menubar" style="border:0;background:transparent;padding:0;height:auto">${menu}</nav>
    <div class="titlebar-spacer"></div>
    ${
      opts.doc
        ? `<div class="titlebar-doc">${esc(opts.doc)}</div><div class="titlebar-sep"></div>`
        : ""
    }
    <div class="titlebar-tools">
      <button class="tb-icon" title="全局搜索">${icon("search", { size: 15 })}</button>
      <button class="tb-icon" title="选项">${icon("settings", { size: 15 })}</button>
    </div>
    <div class="winbtns">
      <button class="winbtn" title="最小化">${winGlyph("min")}</button>
      <button class="winbtn" title="最大化">${winGlyph("max")}</button>
      <button class="winbtn close" title="关闭">${winGlyph("close")}</button>
    </div>
  </header>`;
}

function winGlyph(kind) {
  const paths = {
    min: '<path d="M4 9.5h8"/>',
    max: '<rect x="4" y="4.5" width="8" height="8" rx="1"/>',
    close: '<path d="M4.5 4.5l7 7M11.5 4.5l-7 7"/>',
  };
  return `<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round">${paths[kind]}</svg>`;
}

/** 带图标与文字的命令条按钮 */
export function cmdBtn(label, ico, opts = {}) {
  const cls = ["cmd-btn", opts.active ? "active" : "", opts.disabled ? "disabled" : "", opts.solo ? "solo" : ""]
    .filter(Boolean)
    .join(" ");
  const caret = opts.dropdown
    ? `<span class="cmd-caret">${icon("chevron-down", { size: 10, stroke: 2.2 })}</span>`
    : "";
  const glyph = opts.solo
    ? icon(ico, { size: 16 })
    : pictIcon(opts.iconKey || label, 24);
  return `<button class="${cls}">${glyph}<span class="cmd-label">${esc(label)}</span>${caret}</button>`;
}

export function cmdSep() {
  return '<div class="cmd-sep"></div>';
}

/** 主命令条（12 个入口，与 src/ui/pict.ts 一致：44px 高、统一双色扁平图标族） */
export function commandbar(opts = {}) {
  const active = opts.active || "";
  return `<div class="commandbar">
    ${cmdBtn("连接", "connection", { dropdown: true, active: active === "连接", disabled: opts.noConn })}
    ${cmdBtn("新建查询", "query", { active: active === "查询" })}
    ${cmdSep()}
    ${cmdBtn("表", "table", { active: active === "表" })}
    ${cmdBtn("视图", "view", { active: active === "视图" })}
    ${cmdBtn("函数", "function", { active: active === "函数" })}
    ${cmdBtn("用户", "user", { active: active === "用户" })}
    ${cmdBtn("其它", "more", { dropdown: true })}
    ${cmdSep()}
    ${cmdBtn("查询", "code", { iconKey: "查询工具", active: active === "查询工具栏" })}
    ${cmdBtn("备份", "backup", { active: active === "备份" })}
    ${cmdBtn("自动任务", "schedule", { iconKey: "自动运行", active: active === "自动任务" })}
    ${cmdBtn("图表", "chart", { active: active === "图表" })}
    ${cmdBtn("关系图", "model", { iconKey: "模型", active: active === "关系图" })}
    <div class="ptoolbar-spacer"></div>
    ${opts.right ? opts.right : ""}
  </div>`;
}

export function tabstrip(tabs, activeIndex = 0) {
  if (!tabs.length) {
    return `<div class="tabstrip"><span style="display:flex;align-items:center;padding-left:8px;font-size:12px;color:var(--text-tertiary)">没有打开的标签页</span></div>`;
  }
  return `<div class="tabstrip">${tabs
    .map(
      (t, i) => `<div class="tab${i === activeIndex ? " active" : ""}${t.dirty ? " dirty" : ""}">
        ${icon(t.icon || "table", { size: 14 })}
        <span class="tab-label">${esc(t.label)}</span>
        <span class="tab-close">${icon(t.dirty ? "success" : "cross", { size: 11 })}</span>
      </div>`,
    )
    .join("")}</div>`;
}

export function pagetoolbar(items) {
  return `<div class="pagetoolbar">${items
    .map((it) => {
      if (it === "-") return '<div class="ptool-sep"></div>';
      if (it === "|") return '<div class="ptoolbar-spacer"></div>';
      if (typeof it === "string") return `<button class="ptool">${esc(it)}</button>`;
      const cls = ["ptool", it.kind || "", it.disabled ? "disabled" : ""].filter(Boolean).join(" ");
      const ico = it.icon ? icon(it.icon, { size: 14 }) : "";
      const caret = it.dropdown ? `<span class="caret">${icon("chevron-down", { size: 11, stroke: 2 })}</span>` : "";
      return `<button class="${cls}">${ico}<span>${esc(it.label)}</span>${caret}</button>`;
    })
    .join("")}</div>`;
}

/* -------------------------------------------------------------------- 树 */

export function tree(nodes, opts = {}) {
  const rows = nodes.map((n) => treeRow(n)).join("");
  return `<div class="search">${icon("search", { size: 14 })}<input placeholder="搜索对象、表或连接" readonly></div>
    <div class="tree">${rows}</div>`;
}

function treeRow(n) {
  const depth = n.depth || 0;
  const indent = depth * 14;
  const arrow = n.children
    ? `<span class="tree-arrow">${icon(n.open ? "chevron-down" : "chevron-right", { size: 12, stroke: 2 })}</span>`
    : '<span class="tree-arrow empty"></span>';
  const ico = n.logo
    ? `<span class="tree-ico">${dbLogo(n.logo, 18, { gray: n.logoGray })}</span>`
    : n.icon
      ? `<span class="tree-ico" style="${n.iconColor ? `color:${n.iconColor}` : n.icon && ICON_COLORS[n.icon] ? `color:${ICON_COLORS[n.icon]}` : ""}">${icon(n.icon, { size: 15 })}</span>`
      : "";
  const trailing = n.count
    ? `<span class="tree-badge">${n.count}</span>`
    : n.connected
      ? '<span class="tree-dot"></span>'
      : "";
  const cls = ["tree-row", depth === 1 ? "depth-1" : "", n.selected ? "selected" : "", n.strong ? "strong" : ""]
    .filter(Boolean)
    .join(" ");
  return `<div class="${cls}" style="padding-left:${indent}px">${arrow}${ico}<span class="tree-name">${esc(n.label)}</span>${trailing}</div>`;
}

/** 默认连接树（演示数据）—— 连接节点一律用官方品牌 logo，库/模式节点用线性图标 */
export function defaultTree(selected = "navkit") {
  return [
    { label: "本地连接", icon: "folder-open", open: true, depth: 0, strong: true },
    { label: "localhost", logo: "mysql", open: true, depth: 0, connected: true },
    { label: "navkit", icon: "database", open: true, depth: 0, selected: selected === "navkit" },
    { label: "表", icon: "table", open: true, depth: 1, count: "19" },
    { label: "navkit_account", icon: "table", depth: 2 },
    { label: "navkit_calendars", icon: "table", depth: 2, selected: selected === "navkit_calendars" },
    { label: "navkit_cron_triggers", icon: "table", depth: 2 },
    { label: "navkit_job_details", icon: "table", depth: 2 },
    { label: "navkit_locks", icon: "table", depth: 2 },
    { label: "navkit_system_log", icon: "table", depth: 2 },
    { label: "视图", icon: "view", depth: 1, count: "3" },
    { label: "函数", icon: "function", depth: 1, count: "2" },
    { label: "查询", icon: "code", depth: 1, count: "4" },
    { label: "备份", icon: "backup", depth: 1, count: "2" },
    { label: "information_schema", icon: "schema", depth: 0, iconColor: "var(--n-400)" },
    { label: "mysql", icon: "schema", depth: 0, iconColor: "var(--n-400)" },
    { label: "password_manager", icon: "database", depth: 0 },
    { label: "performance_schema", icon: "schema", depth: 0, iconColor: "var(--n-400)" },
    { label: "sys", icon: "schema", depth: 0, iconColor: "var(--n-400)" },
    { label: "开发环境", icon: "folder", depth: 0, strong: true },
    { label: "dev-mysql", logo: "mysql", depth: 0 },
    { label: "dev-sqlserver", logo: "sqlserver", depth: 0, logoGray: true },
  ];
}

/* -------------------------------------------------------------------- 网格 */

/**
 * columns: {label, key, width, align, sort}
 * rows: 数组，值可以是字符串或 { html }
 */
export function grid(columns, rows, opts = {}) {
  const head = columns
    .map((c) => {
      const cls = [c.align === "right" ? "num" : "", c.sort ? "sorted" : ""].filter(Boolean).join(" ");
      const sortIco = c.sort ? `<span class="th-sort">${icon(c.sort === "desc" ? "chevron-down" : "chevron-up", { size: 11, stroke: 2.4 })}</span>` : "";
      return `<th class="${cls}" style="${c.width ? `width:${c.width}px` : ""}"><span class="th-inner${c.align === "right" ? " num" : ""}"><span>${esc(c.label)}</span>${sortIco}</span></th>`;
    })
    .join("");
  const body = rows
    .map((r, ri) => {
      const cls = [opts.selectedRow === ri ? "selected" : "", opts.editingRow === ri ? "row-editing" : ""]
        .filter(Boolean)
        .join(" ");
      const cells = columns
        .map((c, ci) => {
          const v = r[c.key];
          const align = c.align === "right" ? "num" : "";
          const extra = c.key === opts.pkKey ? "pk" : "";
          if (v !== null && typeof v === "object") return `<td class="${align} ${extra}">${v.html ?? ""}</td>`;
          const isNull = v === null || v === undefined;
          const text = isNull ? "NULL" : esc(v);
          return `<td class="${align} ${extra}${isNull ? " cell-null" : ""}">${text}</td>`;
        })
        .join("");
      return `<tr class="${cls}">${cells}</tr>`;
    })
    .join("");
  return `<div class="grid-wrap"><div class="grid-scroll"><table class="grid"><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table></div></div>`;
}

export function gridFooter(opts = {}) {
  return `<div class="grid-footer">
    ${opts.left || ""}
    <div class="spacer"></div>
    <div class="pagebox">
      <button class="iconbtn">${icon("page-first", { size: 12 })}</button>
      <button class="iconbtn">${icon("page-prev", { size: 12 })}</button>
      <input class="pageno" value="${opts.page || 1}" readonly>
      <button class="iconbtn">${icon("page-next", { size: 12 })}</button>
      <button class="iconbtn">${icon("page-last", { size: 12 })}</button>
    </div>
    <span>${opts.count || "共 19 行"}</span>
    <button class="iconbtn">${icon("refresh", { size: 13 })}</button>
    <button class="iconbtn">${icon("more", { size: 13 })}</button>
  </div>`;
}

/* --------------------------------------------------------------- 信息面板 */

export function infopanel(opts) {
  if (!opts || opts.empty) {
    return `<aside class="infopanel"><div class="info-empty">${icon("info", { size: 20 })}<div style="margin-top:8px">未选择对象</div><div style="margin-top:4px;font-size:11px">在左侧树中选择一个对象查看详情</div></div></aside>`;
  }
  const refs = (opts.refs || [])
    .map((r) => {
      const glyph = r.logo ? dbLogo(r.logo, 16, { gray: r.gray }) : icon(r.icon || "connection", { size: 14 });
      return `<div class="info-ref">${glyph}<span>${esc(r.text)}</span></div>`;
    })
    .join("");
  const groups = (opts.groups || [])
    .map(
      (g) => `<div class="info-group">
        ${g.title ? `<div class="info-group-title">${esc(g.title)}</div>` : ""}
        <dl style="margin:0">${g.rows
          .map(
            ([k, v]) =>
              `<div class="info-row"><dt>${esc(k)}</dt><dd${g.mono ? ' class="mono"' : ""}>${v === "" ? '<span style="color:var(--text-disabled)">—</span>' : esc(v)}</dd></div>`,
          )
          .join("")}</dl>
      </div>`,
    )
    .join("");
  return `<aside class="infopanel">
    <div class="info-head">
      <div class="info-glyph">${icon(opts.icon || "database", { size: 28 })}</div>
      <div class="info-title">${esc(opts.title)}</div>
      ${opts.subtitle ? `<div class="info-sub">${esc(opts.subtitle)}</div>` : ""}
    </div>
    ${refs ? `<div class="info-refs">${refs}</div>` : ""}
    <div class="info-body">${opts.raw || groups}</div>
  </aside>`;
}

/* ------------------------------------------------------------------ 状态栏 */

export function statusbar(opts = {}) {
  const left = (opts.left || []).join("");
  return `<footer class="statusbar">
    ${left}
    <div class="spacer"></div>
    ${opts.right || defaultStatusRight()}
  </footer>`;
}

export function defaultStatusRight() {
  return `<div class="viewsw">
      <button class="iconbtn active">${icon("details", { size: 13 })}</button>
      <button class="iconbtn">${icon("list", { size: 13 })}</button>
      <button class="iconbtn">${icon("grid", { size: 13 })}</button>
    </div>
    <div class="sb-sep"></div>
    <button class="iconbtn" title="侧边栏">${icon("panel-left", { size: 13 })}</button>
    <button class="iconbtn" title="信息面板">${icon("panel-right", { size: 13 })}</button>`;
}

/* ------------------------------------------------------------------- 菜单 */

export function menu(items, opts = {}) {
  const rows = items
    .map((it) => {
      if (it === "-") return '<div class="menu-sep"></div>';
      const cls = ["menu-item", it.checked ? "checked" : "", it.disabled ? "disabled" : "", it.hover ? "hover" : ""]
        .filter(Boolean)
        .join(" ");
      const ico = it.checked
        ? `<span class="mi-ico">${icon("check", { size: 13, stroke: 2.4 })}</span>`
        : `<span class="mi-ico">${it.icon ? icon(it.icon, { size: 13 }) : ""}</span>`;
      const key = it.key ? `<span class="mi-key">${esc(it.key)}</span>` : "";
      const arrow = it.children ? `<span class="mi-arrow">${icon("chevron-right", { size: 12 })}</span>` : "";
      return `<div class="${cls}">${ico}<span class="mi-label">${esc(it.label)}</span>${key}${arrow}</div>`;
    })
    .join("");
  const anchored = opts.x !== undefined || opts.y !== undefined;
  const style = anchored
    ? `left:${opts.x ?? 0}px;top:${opts.y ?? 0}px${opts.width ? `;min-width:${opts.width}px` : ""}`
    : `min-width:${opts.width ?? 208}px`;
  return `<div class="menu" style="${style}">${rows}</div>`;
}

/* ---------------------------------------------------------------- 对话框 */

export function dialog(opts) {
  return `<div class="backdrop">
    <div class="dialog" style="width:${opts.width}px${opts.height ? `;height:${opts.height}px` : ""}">
      <div class="dialog-title">
        <span class="dialog-title-text">${esc(opts.title)}</span>
        ${opts.noClose ? "" : `<button class="iconbtn">${icon("cross", { size: 13 })}</button>`}
      </div>
      ${
        opts.tabs
          ? `<div class="dtabs">${opts.tabs
              .map((t, i) => `<div class="dtab${i === (opts.activeTab ?? 0) ? " active" : ""}">${esc(t)}</div>`)
              .join("")}</div>`
          : ""
      }
      ${opts.steps ? steps(opts.steps, opts.activeStep ?? 0) : ""}
      <div class="dialog-body${opts.flush ? " flush" : ""}">${opts.body}</div>
      ${opts.footer ? `<div class="dialog-footer">${opts.footer}</div>` : ""}
    </div>
  </div>`;
}

export function steps(labels, active = 0) {
  const parts = [];
  labels.forEach((l, i) => {
    const state = i === active ? "active" : i < active ? "done" : "";
    parts.push(`<div class="step ${state}"><span class="bubble">${i < active ? icon("check", { size: 11, stroke: 2.6 }) : i + 1}</span><span>${esc(l)}</span></div>`);
    if (i < labels.length - 1) parts.push('<div class="step-line"></div>');
  });
  return `<div class="steps">${parts.join("")}</div>`;
}

export function footer(buttons) {
  const left = buttons.filter((b) => b.alignLeft);
  const right = buttons.filter((b) => !b.alignLeft);
  return `${left.map((b) => button(b.label, b)).join("")}${left.length ? '<div style="flex:1"></div>' : '<div class="spacer"></div>'}${right
    .map((b) => button(b.label, b))
    .join("")}`;
}

export function note(kind, text) {
  const ico = { info: "info", ok: "success", warn: "warning", error: "error" }[kind] || "info";
  return `<div class="note ${kind}"><span class="note-ico">${icon(ico, { size: 14 })}</span><span>${text}</span></div>`;
}

/* ------------------------------------------------------------------ 其他 */

export function chip(text, kind = "") {
  return `<span class="chip ${kind}">${esc(text)}</span>`;
}

export function emptyState(opts) {
  return `<div class="empty-state">
    <div class="empty-state-glyph">${icon(opts.icon || "database", { size: 32 })}</div>
    <div class="empty-state-title">${esc(opts.title)}</div>
    <div class="empty-state-text">${opts.text}</div>
    ${opts.actions ? `<div class="empty-state-actions">${opts.actions}</div>` : ""}
  </div>`;
}

export function toast(kind, text) {
  const ico = { info: "info", ok: "success", error: "error" }[kind] || "info";
  return `<div class="toast ${kind}"><span class="toast-ico">${icon(ico, { size: 15 })}</span><span>${esc(text)}</span></div>`;
}

/** 带高亮的 SQL 片段（粗粒度着色，仅用于设计稿展示） */
export function highlightSql(sql) {
  return esc(sql)
    .replace(
      /\b(SELECT|FROM|WHERE|INSERT|INTO|VALUES|UPDATE|SET|DELETE|CREATE|TABLE|ALTER|DROP|JOIN|LEFT|RIGHT|INNER|ON|GROUP|BY|ORDER|LIMIT|AS|AND|OR|NOT|NULL|PRIMARY|KEY|DEFAULT|COMMENT|ENGINE|INT|BIGINT|VARCHAR|DATETIME|TEXT|LONGTEXT|AUTO_INCREMENT|IF|EXISTS|COUNT|SUM|AVG|BEGIN|END)\b/g,
      '<span class="kw">$1</span>',
    )
    .replace(/'([^']*)'/g, '<span class="str">\'$1\'</span>')
    .replace(/(--[^\n]*)/g, '<span class="cmt">$1</span>');
}

/** 屏幕画框：用于画廊里逐个展示 */
export function screen(title, subtitle, bodyHtml, opts = {}) {
  return `<section class="screen" data-title="${esc(title)}">
    <div class="screen-meta">
      <h2>${esc(title)}</h2>
      ${subtitle ? `<p>${esc(subtitle)}</p>` : ""}
    </div>
    <div class="screen-frame" style="width:${opts.w || 1440}px;height:${opts.h || 900}px">${bodyHtml}</div>
  </section>`;
}
