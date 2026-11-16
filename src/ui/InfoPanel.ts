/**
 * 右侧信息面板（v2 · 身份块 + 指标带 + 分组键值）。
 *
 * 设计规范（docs/design/DESIGN.md §5.5，表页按
 * docs/design/info-panel-redesign/ 的方案 A 落地）：
 *  1. 顶栏 30px：ⓘ + 连接 chip（品牌图标 + 连接名，替代原来的独立「引用行」）
 *  2. 身份块：40px 圆角图标块（--accent-50 底 + 强调色图标）+ 15px 600 标题
 *     + 类型 chip（表 / 列）+ 11px 副行（库名 · 引擎）
 *  3. 指标带：列 / 索引 / 外键 / 行 四格，从键值表提到顶部（表页才有）
 *  4. 整宽三视图分段：带文字标签的表信息 / DDL / 列信息（替代纯图标切换）
 *  5. 分组信息：11px 分组标题（字距 0.05em），96px | 1fr 两列键值，空值统一 `—` 且降级
 *
 * 分组为纯展示层：按字段标签把 objectDetails 产出的扁平字段归类为
 * 概要 / 属性 / 服务器 / 连接 / 定义（定义渲染为 DDL 代码块）。
 */

import { ICON_INFO_CIRCLE, svg, iconSvgLarge, iconSvgByName, linearIcon } from "./icons";
import { highlightSql } from "./sql-highlight";

/** 资源名 → v2 线性图标 key（信息面板对象头用强调色线性图标）。 */
const LINEAR_KEY_BY_RES: Record<string, string> = {
  info_pane_database: "database",
  info_pane_schema: "schema",
  info_pane_table: "table",
  info_pane_view: "view",
  info_pane_function: "function",
  info_pane_procedure: "function",
  info_pane_trigger: "function",
  info_pane_query: "query",
  info_pane_backup: "backup",
  info_pane_folder: "folder",
  info_pane_project: "folder",
  columns: "columns",
};

/** 对象头图标块内容：连接类用官方品牌 logo，其余用线性强调色图标。 */
function headerGlyph(iconRes: string, size = 22): string {
  if (iconRes.startsWith("info_pane_conn_")) return iconSvgLarge(iconRes);
  const key = LINEAR_KEY_BY_RES[iconRes];
  if (key) return linearIcon(key, size);
  return iconSvgLarge(iconRes);
}

export interface InfoField {
  label: string;
  value: string;
}

/** 列信息（信息面板「列信息」页）。 */
export interface TableColumnInfo {
  name: string;
  type: string;
  /** "是" / "否"。 */
  nullable: string;
  defaultValue: string;
  comment: string;
}

/** 表页信息面板三视图数据。 */
export interface InfoTableState {
  /** 连接/库/表的稳定身份，区分跨库同名表。 */
  identity?: string;
  /** 表名（标题）。 */
  title: string;
  /** 连接引用行。 */
  refs?: { icon: string; text: string }[];
  /** 「表信息」页字段（顺序即展示顺序）。 */
  info: InfoField[];
  /** 「DDL」页建表语句。 */
  ddl: string;
  /** 「列信息」页（选中列时；null 时显示引导文案）。 */
  column: TableColumnInfo | null;
  /** 初始模式。 */
  mode?: "info" | "ddl" | "column";
}

export interface InfoPanelState {
  /** 顶部大图标（资源名）。 */
  icon: string;
  /** 主标题。 */
  title: string;
  /** 副标题。 */
  subtitle?: string;
  /** 连接引用行（icon + 文本），如 localhost。 */
  refs?: { icon: string; text: string }[];
  /** 字段行。 */
  fields: InfoField[];
}

/* ---------------------------------------------------------------- 分组 */

/** 分组展示顺序。 */
const GROUP_ORDER = ["概要", "属性", "服务器", "连接", "定义"] as const;

/** 状态 / 统计说明 / 读取失败等不进键值表，整行降级显示。 */
function isNoteField(label: string): boolean {
  return (
    label === "状态" ||
    label === "统计说明" ||
    label.endsWith("读取失败") ||
    label.endsWith("状态")
  );
}

/** 按标签把字段归入分组（纯展示层映射，见文件头注释）。 */
function groupOf(label: string): string {
  if (isNoteField(label)) return "";
  if (label === "定义") return "定义";
  if (label.startsWith("服务器") || label === "version") return "服务器";
  // 概要 = 「是什么 / 有多大 / 用什么字符集」这类整体字段（表页指标带之外剩下的部分）
  if (
    label === "主键" ||
    /数量|行数|大小|总数|长度$|排序规则|字符集|主键列$|源码行数/.test(label)
  ) {
    return "概要";
  }
  if (
    [
      "连接", "连接类型", "主机", "端口", "用户名", "默认数据库", "数据库文件", "只读连接",
      "所属连接", "所属数据库/模式", "数据库/模式名称", "模式名称", "对象类型",
    ].includes(label)
  ) {
    return "连接";
  }
  return "属性";
}

/* ------------------------------------------------------- 指标带（表页顶部） */

/** 提升为顶部指标带四格的字段（提升后不再出现在键值表里）。 */
const KPI_FIELDS: { label: string; short: string }[] = [
  { label: "列数量", short: "列" },
  { label: "索引数量", short: "索引" },
  { label: "外键数量", short: "外键" },
  { label: "行", short: "行" },
];

/** 指标带数字紧凑化（292px 面板四格均分放不下 1,252,059）：1252059 → 125万。 */
function compactNumber(value: string): string {
  const n = Number(value.replace(/[,\s]/g, ""));
  if (!Number.isFinite(n) || n < 10000) return value;
  if (n < 1e8) {
    const wan = n / 10000;
    return `${wan >= 100 ? Math.round(wan) : Math.round(wan * 10) / 10}万`;
  }
  return `${Math.round((n / 1e8) * 10) / 10}亿`;
}

/** 取出指标带条目，并返回剔除它们之后的字段（精确值留在 title 里）。 */
function splitKpis(
  fields: InfoField[],
): [{ short: string; value: string; title: string }[], InfoField[]] {
  const kpis: { short: string; value: string; title: string }[] = [];
  const used = new Set<string>();
  for (const k of KPI_FIELDS) {
    const hit = fields.find((f) => f.label === k.label && !used.has(f.label));
    if (!hit) continue;
    used.add(k.label);
    const raw = (hit.value ?? "").trim();
    // 空值 / — 不进指标带（宁可少一格，也不显示 0 或空白）
    if (raw === "" || raw === "--" || raw === "—") continue;
    kpis.push({ short: k.short, value: compactNumber(raw), title: `${k.label}：${raw}` });
  }
  return [kpis, fields.filter((f) => !used.has(f.label))];
}

/** 取字段值（用于身份块副行）。 */
function fieldValue(fields: InfoField[], label: string): string {
  const v = fields.find((f) => f.label === label)?.value?.trim();
  return v && v !== "--" ? v : "";
}

/** 副行已展示的字段（从键值表剔除，避免同一信息出现两次）。 */
const META_FIELDS = ["数据库", "引擎"];

/** 身份块副行（库名 · 引擎）+ 剔除后的字段。 */
function splitMeta(fields: InfoField[]): [string[], InfoField[]] {
  const meta = META_FIELDS.map((label) => fieldValue(fields, label)).filter(Boolean);
  return [meta, fields.filter((f) => !META_FIELDS.includes(f.label))];
}

/** 把扁平字段整理为 [分组, 字段[]] 序列（保持组内原顺序）。 */
function groupFields(fields: InfoField[]): [string, InfoField[]][] {
  const buckets = new Map<string, InfoField[]>();
  for (const f of fields) {
    const g = groupOf(f.label);
    if (!buckets.has(g)) buckets.set(g, []);
    buckets.get(g)!.push(f);
  }
  const out: [string, InfoField[]][] = [];
  // 说明 / 状态 / 读取失败等备注行没有分组标题，放在最前面整行降级显示（错误与统计口径不能被藏起来）。
  const notes = buckets.get("");
  if (notes?.length) out.push(["", notes]);
  for (const g of GROUP_ORDER) {
    const items = buckets.get(g);
    if (items?.length) out.push([g, items]);
  }
  // 兜底：映射外的分组（新增字段不至于丢失）
  for (const [g, items] of buckets) {
    if (g && !GROUP_ORDER.includes(g as (typeof GROUP_ORDER)[number]) && items.length) {
      out.push([g, items]);
    }
  }
  return out;
}

export class InfoPanel {
  readonly el: HTMLElement;
  private body: HTMLElement;
  /** 表页三视图当前模式。 */
  private tableMode: "info" | "ddl" | "column" = "info";
  private tableState: InfoTableState | null = null;
  /** 最近一次 setState 的内容签名：内容相同就不重建，避免树操作导致右侧闪动。 */
  private lastStateKey = "";
  private lastTableKey = "";

  constructor(private readonly onToggle?: () => void) {
    this.el = document.createElement("div");
    this.el.id = "app-info-panel";
    this.el.className = "info-panel";
    this.body = document.createElement("div");
    this.body.className = "info-body";
    this.el.appendChild(this.body);
  }

  /** 设置表页三视图内容（表信息 / DDL / 列信息）。 */
  setTableState(state: InfoTableState | null): void {
    this.lastStateKey = "";
    // 切换对象时重置到「表信息」，避免沿用上一张表的 DDL/列信息模式
    // （同一张表刷新时 state.mode 会显式带上当前模式）
    const sameObject = !!state && this.tableState?.title === state.title
      && this.tableState?.identity === state.identity
      && JSON.stringify(this.tableState?.refs ?? []) === JSON.stringify(state.refs ?? []);
    this.tableState = state;
    if (state?.mode) this.tableMode = state.mode;
    else if (!sameObject) this.tableMode = "info";
    this.renderTable();
  }

  /** 设置当前选中列（列信息页联动）。 */
  setTableColumn(column: TableColumnInfo | null): void {
    if (!this.tableState) return;
    this.tableState = { ...this.tableState, column };
    // 列信息实时跟随选中列：已切到列页或刚选列时直接展示
    if (column && this.tableMode === "column") this.renderTable();
  }

  /** 空态：未选择对象（设计图空态文案）。顶栏保持可用，便于随时收起信息栏。 */
  private renderEmpty(): void {
    this.appendTopbar(undefined);
    const empty = document.createElement("div");
    empty.className = "info-empty";
    empty.textContent = "在左侧树中选择一个对象查看详情";
    this.body.appendChild(empty);
  }

  /** 顶部工具行：ⓘ（点击收起）+ 连接 chip（方案 A：连接不再单独占一行）。 */
  private appendTopbar(refs: { icon: string; text: string }[] | undefined): void {
    const top = document.createElement("div");
    top.className = "info-topbar";
    const info = document.createElement("button");
    info.type = "button";
    info.innerHTML = svg(ICON_INFO_CIRCLE);
    info.className = "info-top-icon-sm";
    info.setAttribute("aria-label", "收起信息栏");
    info.title = "收起信息栏";
    info.addEventListener("click", () => this.onToggle?.());
    top.appendChild(info);
    for (const ref of refs ?? []) {
      const chip = document.createElement("div");
      chip.className = "info-conn-chip";
      chip.title = ref.text;
      const ic = document.createElement("span");
      ic.className = "info-conn-chip-icon";
      ic.innerHTML = iconSvgByName(ref.icon);
      const t = document.createElement("span");
      t.textContent = ref.text;
      chip.append(ic, t);
      top.appendChild(chip);
    }
    this.body.appendChild(top);
  }

  /** 身份块：40px 图标块 + 标题 + 类型 chip + 副行（库名 · 引擎）。 */
  private appendIdentity(
    iconRes: string,
    title: string,
    kind?: string,
    meta: string[] = [],
  ): void {
    const box = document.createElement("div");
    box.className = "info-identity";
    const glyph = document.createElement("div");
    glyph.className = "info-glyph";
    glyph.innerHTML = headerGlyph(iconRes);
    const text = document.createElement("div");
    text.className = "info-idtext";
    const t = document.createElement("div");
    t.className = "info-title";
    t.textContent = title;
    t.title = title;
    text.appendChild(t);
    const sub = document.createElement("div");
    sub.className = "info-meta";
    if (kind) {
      const chip = document.createElement("span");
      chip.className = "info-kind";
      chip.textContent = kind;
      sub.appendChild(chip);
    }
    meta.filter(Boolean).forEach((item, i) => {
      if (i > 0 || kind) {
        const dot = document.createElement("span");
        dot.className = "info-meta-dot";
        sub.appendChild(dot);
      }
      const s = document.createElement("span");
      s.textContent = item;
      s.title = item;
      sub.appendChild(s);
    });
    if (sub.childElementCount > 0) text.appendChild(sub);
    box.append(glyph, text);
    this.body.appendChild(box);
  }

  /** 指标带：列 / 索引 / 外键 / 行（完整精确值放 title）。 */
  private appendKpiBand(kpis: { short: string; value: string; title: string }[]): void {
    if (!kpis.length) return;
    const band = document.createElement("div");
    band.className = "info-kpis";
    band.style.gridTemplateColumns = `repeat(${kpis.length}, 1fr)`;
    for (const k of kpis) {
      const cell = document.createElement("div");
      cell.className = "info-kpi";
      cell.title = k.title;
      const b = document.createElement("b");
      b.textContent = k.value;
      const s = document.createElement("span");
      s.textContent = k.short;
      cell.append(b, s);
      band.appendChild(cell);
    }
    this.body.appendChild(band);
  }

  /** 整宽三视图分段（带文字标签）。 */
  private appendModes(): void {
    const modes: { key: "info" | "ddl" | "column"; icon: string; title: string }[] = [
      { key: "info", icon: "table", title: "表信息" },
      { key: "ddl", icon: "editor", title: "DDL" },
      { key: "column", icon: "columns", title: "列信息" },
    ];
    const group = document.createElement("div");
    group.className = "info-modes";
    group.setAttribute("role", "tablist");
    for (const m of modes) {
      const btn = document.createElement("div");
      const active = this.tableMode === m.key;
      btn.className = `info-mode-tab${active ? " active" : ""}`;
      btn.title = m.title;
      btn.setAttribute("role", "tab");
      btn.setAttribute("aria-selected", String(active));
      const ic = document.createElement("span");
      ic.innerHTML = linearIcon(m.icon, 12);
      const label = document.createElement("span");
      label.textContent = m.title;
      btn.append(ic, label);
      btn.addEventListener("click", () => {
        if (this.tableMode === m.key) return;
        this.tableMode = m.key;
        this.renderTable();
      });
      group.appendChild(btn);
    }
    this.body.appendChild(group);
  }

  /** 分组键值表。note 类字段整行降级；「定义」渲染为 DDL 块。 */
  private appendGroups(fields: InfoField[]): void {
    const bodyEl = document.createElement("div");
    bodyEl.className = "info-groups";
    let lastGroup: string | null = null;
    for (const [group, items] of groupFields(fields)) {
      // 备注行（空分组）没有标题，整行降级显示。
      if (group && group !== lastGroup) {
        const gt = document.createElement("div");
        gt.className = "info-group-title";
        gt.textContent = group;
        bodyEl.appendChild(gt);
        lastGroup = group;
      }
      for (const f of items) {
        if (f.label === "定义") {
          const pre = document.createElement("pre");
          pre.className = "ddl-code";
          pre.innerHTML = highlightSql(f.value || "-- 无定义");
          bodyEl.appendChild(pre);
          continue;
        }
        if (isNoteField(f.label)) {
          const note = document.createElement("div");
          note.className = "info-note";
          note.textContent = `${f.label}：${f.value}`;
          bodyEl.appendChild(note);
          continue;
        }
        const row = document.createElement("div");
        row.className = "info-row";
        const dt = document.createElement("div");
        dt.textContent = f.label;
        const dd = document.createElement("div");
        const v = (f.value ?? "").trim();
        dd.textContent = v !== "" && v !== "--" ? v : "—";
        if (v === "" || v === "--") dd.classList.add("dimmed");
        row.append(dt, dd);
        bodyEl.appendChild(row);
      }
    }
    this.body.appendChild(bodyEl);
  }

  private renderTable(): void {
    const s = this.tableState;
    const key = JSON.stringify([s, this.tableMode]);
    if (key === this.lastTableKey) return;
    this.lastTableKey = key;
    const scrollTop = this.body.scrollTop;
    this.body.innerHTML = "";
    queueMicrotask(() => { if (this.body.isConnected) this.body.scrollTop = scrollTop; });
    if (!s) {
      this.renderEmpty();
      return;
    }

    this.appendTopbar(s.refs);

    const [kpis, rest] = splitKpis(s.info);
    const [meta, groups] = splitMeta(rest);

    if (this.tableMode === "ddl") {
      this.appendIdentity("info_pane_table", s.title, "表", meta);
      this.appendModes();
      const pre = document.createElement("pre");
      pre.className = "ddl-code";
      pre.innerHTML = highlightSql(s.ddl || "-- 无定义");
      this.body.appendChild(pre);
      return;
    }

    if (this.tableMode === "column") {
      const c = s.column;
      if (!c) {
        this.appendIdentity("info_pane_table", s.title, "表", meta);
        this.appendModes();
        const hint = document.createElement("div");
        hint.className = "info-note";
        hint.textContent = "点击表头选择列，查看列信息。";
        this.body.appendChild(hint);
        return;
      }
      this.appendIdentity("columns", c.name, "列", [s.title, c.type].filter(Boolean));
      this.appendModes();
      this.appendGroups([
        { label: "类型", value: c.type },
        { label: "不是 null", value: c.nullable },
        { label: "默认值", value: c.defaultValue },
        { label: "注释", value: c.comment },
      ]);
      return;
    }

    // 表信息（默认）：指标带放在视图分段之下，切换 DDL / 列信息时分段位置不动
    this.appendIdentity("info_pane_table", s.title, "表", meta);
    this.appendModes();
    this.appendKpiBand(kpis);
    this.appendGroups(groups);
  }

  /** 设置内容。 */
  setState(state: InfoPanelState | null): void {
    // 同一内容重复下发（树刷新、标签切换、工具栏重建）不再重建 DOM：
    // 重建会打断阅读位置并造成可见闪动。
    const key = state === null
      ? "null"
      : JSON.stringify([state.icon, state.title, state.subtitle ?? "", state.refs ?? [], state.fields]);
    if (key === this.lastStateKey) return;
    this.lastStateKey = key;
    const scrollTop = this.body.scrollTop;
    this.tableState = null;
    this.lastTableKey = "";
    this.body.innerHTML = "";
    if (!state) {
      this.renderEmpty();
      return;
    }

    this.appendTopbar(state.refs);
    this.appendIdentity(state.icon, state.title, state.subtitle);
    this.appendGroups(state.fields);

    // 重建后保持原阅读位置，避免内容相同的刷新把视图弹回顶部。
    this.body.scrollTop = scrollTop;
  }

  /** 代码段面板（查询页右侧）。与「代码段」对话框共用数据源；支持新增 / 删除自定义代码段。 */
  setSnippets(
    items: { name: string; tag?: string; desc?: string; sql?: string; custom?: boolean }[],
    onInsert?: (item: { name: string; tag?: string; desc?: string; sql?: string }) => void,
    manage?: { onAdd?: () => void; onDelete?: (name: string) => void },
  ): void {
    this.lastStateKey = "";
    this.tableState = null;
    this.lastTableKey = "";
    this.body.innerHTML = "";
    const top = document.createElement("div");
    top.className = "snippet-top";
    // 代码段面板同样保留左上角 ⓘ 收起入口，和表页信息栏保持同一开关语义。
    const collapse = document.createElement("button");
    collapse.type = "button";
    collapse.className = "info-top-icon-sm";
    collapse.innerHTML = svg(ICON_INFO_CIRCLE);
    collapse.setAttribute("aria-label", "收起信息栏");
    collapse.title = "收起信息栏";
    collapse.addEventListener("click", () => this.onToggle?.());
    top.appendChild(collapse);
    const combo = document.createElement("select");
    combo.className = "snippet-combo";
    combo.setAttribute("aria-label", "代码段标签");
    for (const tag of ["", ...new Set(items.map((item) => item.tag).filter((tag): tag is string => !!tag))]) {
      const option = document.createElement("option");
      option.value = tag;
      option.textContent = tag || "全部标签";
      combo.appendChild(option);
    }
    const sync = document.createElement("button");
    sync.type = "button";
    sync.className = "snippet-sync";
    sync.textContent = "重置";
    const add = document.createElement("button");
    add.type = "button";
    add.className = "snippet-sync";
    add.textContent = "＋";
    add.title = "新增自定义代码段";
    add.setAttribute("aria-label", "新增自定义代码段");
    add.addEventListener("click", () => manage?.onAdd?.());
    top.append(combo, sync, add);
    this.body.appendChild(top);
    const search = document.createElement("input");
    search.type = "search";
    search.placeholder = "搜索代码段";
    search.setAttribute("aria-label", "搜索代码段");
    search.className = "snippet-search";
    this.body.appendChild(search);
    const list = document.createElement("div");
    list.className = "snippet-list";
    this.body.appendChild(list);
    const render = () => {
      list.innerHTML = "";
      const query = search.value.trim().toLowerCase();
      for (const item of items.filter((item) => (!combo.value || item.tag === combo.value) &&
        `${item.name} ${item.desc ?? ""}`.toLowerCase().includes(query))) {
        const row = document.createElement("button");
        row.type = "button";
        row.className = "snippet-item";
        row.setAttribute("aria-label", `插入代码段 ${item.name}`);
        row.title = "点击插入编辑器（不执行）";
        const icon = document.createElement("span");
        icon.className = "snippet-icon";
        const main = document.createElement("div");
        main.className = "snippet-main";
        const title = document.createElement("div");
        title.className = "snippet-title";
        title.textContent = item.name;
        if (item.tag) {
          const tag = document.createElement("span");
          tag.className = `snippet-tag tag-${item.tag}`;
          tag.textContent = item.tag;
          title.appendChild(tag);
        }
        const desc = document.createElement("div");
        desc.className = "snippet-desc";
        desc.textContent = item.desc ?? "";
        main.append(title, desc);
        row.append(icon, main);
        if (item.custom && manage?.onDelete) {
          const del = document.createElement("span");
          del.className = "snippet-del";
          del.textContent = "×";
          del.title = "删除此自定义代码段";
          del.setAttribute("role", "button");
          del.setAttribute("aria-label", `删除代码段 ${item.name}`);
          del.addEventListener("click", (e) => {
            e.stopPropagation();
            manage.onDelete?.(item.name);
          });
          row.appendChild(del);
        }
        row.addEventListener("click", () => onInsert?.(item));
        list.appendChild(row);
      }
      if (!list.children.length) list.textContent = "没有匹配的代码段";
    };
    combo.addEventListener("change", render);
    search.addEventListener("input", render);
    sync.addEventListener("click", () => { combo.value = ""; search.value = ""; render(); });
    render();
  }

  /** 显示 / 隐藏。 */
  setVisible(v: boolean): void {
    this.el.classList.toggle("hidden", !v);
  }
}

