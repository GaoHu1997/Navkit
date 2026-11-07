/**
 * 右侧信息面板（v2 · 三段式分组）。
 *
 * 设计规范（docs/design/DESIGN.md §5.5）：
 *  1. 对象头：56px 圆角图标块（--accent-50 底 + 强调色图标）+ 15px 600 标题 + 11px 副标题
 *  2. 连接引用：图标 + 文本，12px
 *  3. 分组信息：11px 分组标题（字距 0.04em），96px | 1fr 两列键值，空值统一 `—` 且降级
 *
 * 分组为纯展示层：按字段标签把 objectDetails 产出的扁平字段归类为
 * 概要 / 属性 / 服务器 / 连接 / 定义（定义渲染为 DDL 代码块）。
 */

import { ICON_INFO_CIRCLE, svg, iconSvgLarge, iconSvgByName, linearIcon } from "./icons";

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

/** 对象头 56px 图标块内容：连接类用官方品牌 logo，其余用线性强调色图标。 */
function headerGlyph(iconRes: string): string {
  if (iconRes.startsWith("info_pane_conn_")) return iconSvgLarge(iconRes);
  const key = LINEAR_KEY_BY_RES[iconRes];
  if (key) return linearIcon(key, 28);
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
  /** 可点击的对象列表（查询页右侧的表清单）。 */
  list?: { icon: string; text: string; onClick?: () => void }[];
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
  if (
    /数量|行数|大小|总数|主键列$|源码行数/.test(label)
  ) {
    return "概要";
  }
  if (
    label.endsWith("（配置）") ||
    [
      "连接", "连接类型", "用户名", "默认数据库", "数据库文件", "只读连接",
      "所属连接", "所属数据库/模式", "数据库/模式名称", "模式名称", "对象类型",
    ].includes(label)
  ) {
    return "连接";
  }
  return "属性";
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

  constructor() {
    this.el = document.createElement("div");
    this.el.className = "info-panel";
    this.body = document.createElement("div");
    this.body.className = "info-body";
    this.el.appendChild(this.body);
  }

  /** 设置表页三视图内容（表信息 / DDL / 列信息）。 */
  setTableState(state: InfoTableState | null): void {
    // 切换对象时重置到「表信息」，避免沿用上一张表的 DDL/列信息模式
    // （同一张表刷新时 state.mode 会显式带上当前模式）
    const sameObject = !!state && this.tableState?.title === state.title;
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

  /** 空态：未选择对象（设计图空态文案）。 */
  private renderEmpty(): void {
    const empty = document.createElement("div");
    empty.className = "info-empty";
    empty.textContent = "在左侧树中选择一个对象查看详情";
    this.body.appendChild(empty);
  }

  /** 顶部工具行：ⓘ + （表页）三视图模式切换。 */
  private appendTopbar(withModes: boolean): void {
    const top = document.createElement("div");
    top.className = "info-topbar";
    const info = document.createElement("div");
    info.innerHTML = svg(ICON_INFO_CIRCLE);
    info.className = "info-top-icon-sm";
    top.appendChild(info);
    const spacer = document.createElement("div");
    spacer.style.flex = "1";
    top.appendChild(spacer);
    if (withModes) {
      // 三段式切换用单色线性图标：颜色完全由 active 状态决定，
      // 避免彩色双色图标（蓝表）让「表信息」看起来永远是高亮的。
      const modes: { key: "info" | "ddl" | "column"; icon: string; title: string }[] = [
        { key: "info", icon: "table", title: "表信息" },
        { key: "ddl", icon: "editor", title: "DDL" },
        { key: "column", icon: "columns", title: "列信息" },
      ];
      const group = document.createElement("div");
      group.className = "info-mode-group";
      group.setAttribute("role", "tablist");
      for (const m of modes) {
        const btn = document.createElement("div");
        const active = this.tableMode === m.key;
        btn.className = `info-mode-btn${active ? " active" : ""}`;
        btn.title = m.title;
        btn.setAttribute("role", "tab");
        btn.setAttribute("aria-selected", String(active));
        btn.setAttribute("aria-label", m.title);
        btn.innerHTML = linearIcon(m.icon, 15);
        btn.addEventListener("click", () => {
          this.tableMode = m.key;
          this.renderTable();
        });
        group.appendChild(btn);
      }
      top.appendChild(group);
    }
    this.body.appendChild(top);
  }

  /** 对象头：56px 图标块 + 标题 + 副标题（居中）。 */
  private appendHeader(iconRes: string, title: string, subtitle?: string): void {
    const head = document.createElement("div");
    head.className = "info-head";
    const glyph = document.createElement("div");
    glyph.className = "info-glyph";
    glyph.innerHTML = headerGlyph(iconRes);
    const t = document.createElement("div");
    t.className = "info-title";
    t.textContent = title;
    head.appendChild(glyph);
    head.appendChild(t);
    if (subtitle) {
      const sub = document.createElement("div");
      sub.className = "info-sub";
      sub.textContent = subtitle;
      head.appendChild(sub);
    }
    this.body.appendChild(head);
  }

  /** 连接引用行。 */
  private appendRefs(refs: { icon: string; text: string }[] | undefined): void {
    if (!refs?.length) return;
    const box = document.createElement("div");
    box.className = "info-refs";
    for (const ref of refs) {
      const row = document.createElement("div");
      row.className = "info-ref";
      const ic = document.createElement("span");
      ic.innerHTML = iconSvgByName(ref.icon);
      row.appendChild(ic);
      const t = document.createElement("span");
      t.textContent = ref.text;
      row.appendChild(t);
      box.appendChild(row);
    }
    this.body.appendChild(box);
  }

  /** 分组键值表。note 类字段整行降级；「定义」渲染为 DDL 块。 */
  private appendGroups(fields: InfoField[]): void {
    const bodyEl = document.createElement("div");
    bodyEl.className = "info-groups";
    let lastGroup: string | null = null;
    for (const [group, items] of groupFields(fields)) {
      if (group !== lastGroup) {
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
    this.body.innerHTML = "";
    if (!s) {
      this.renderEmpty();
      return;
    }

    this.appendTopbar(true);

    if (this.tableMode === "ddl") {
      const pre = document.createElement("pre");
      pre.className = "ddl-code";
      pre.innerHTML = highlightSql(s.ddl || "-- 无定义");
      this.body.appendChild(pre);
      return;
    }

    if (this.tableMode === "column") {
      const c = s.column;
      if (!c) {
        const hint = document.createElement("div");
        hint.className = "info-note";
        hint.textContent = "点击表头选择列，查看列信息。";
        this.body.appendChild(hint);
        return;
      }
      this.appendHeader("columns", c.name, "列");
      this.appendGroups([
        { label: "类型", value: c.type },
        { label: "不是 null", value: c.nullable },
        { label: "默认值", value: c.defaultValue },
        { label: "注释", value: c.comment },
      ]);
      return;
    }

    // 表信息（默认）
    this.appendHeader("info_pane_table", s.title, "表");
    this.appendRefs(s.refs);
    this.appendGroups(s.info);
  }

  /** 设置内容。 */
  setState(state: InfoPanelState | null): void {
    this.tableState = null;
    this.body.innerHTML = "";
    if (!state) {
      this.renderEmpty();
      return;
    }

    this.appendTopbar(false);
    this.appendHeader(state.icon, state.title, state.subtitle);
    this.appendRefs(state.refs);
    this.appendGroups(state.fields);

    // 对象列表（查询页：当前库的表清单）
    for (const item of state.list ?? []) {
      const row = document.createElement("div");
      row.className = "info-list-row";
      const ic = document.createElement("span");
      ic.innerHTML = iconSvgByName(item.icon);
      row.appendChild(ic);
      const t = document.createElement("span");
      t.textContent = item.text;
      row.appendChild(t);
      if (item.onClick) {
        row.addEventListener("dblclick", () => item.onClick!());
        row.title = `${item.text}（双击打开）`;
      }
      this.body.appendChild(row);
    }
  }

  /** 代码段面板（查询页右侧，参考截图 5）。 */
  setSnippets(items: { name: string; tag?: string; desc?: string; sql?: string }[],
    onInsert?: (item: { name: string; tag?: string; desc?: string; sql?: string }) => void): void {
    this.tableState = null;
    this.body.innerHTML = "";
    const top = document.createElement("div");
    top.className = "snippet-top";
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
    top.append(combo, sync);
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

/** SQL 简单语法高亮（DDL 页）：注释 / 字符串 / 关键字 / 类型。 */
function highlightSql(sql: string): string {
  const esc = (s: string) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const KEYWORDS = new Set([
    "CREATE", "TABLE", "PRIMARY", "KEY", "NOT", "NULL", "DEFAULT", "ENGINE",
    "CHARSET", "COLLATE", "COMMENT", "AUTO_INCREMENT", "UNSIGNED", "ZEROFILL",
    "UNIQUE", "INDEX", "CONSTRAINT", "FOREIGN", "REFERENCES", "ON", "DELETE",
    "UPDATE", "CASCADE", "SET", "VIEW", "AS", "SELECT", "FROM", "WHERE", "AND",
    "OR", "IF", "EXISTS", "REPLACE", "ALTER", "ADD", "DROP", "COLUMN", "TYPE",
    "GENERATED", "ALWAYS", "STORED", "VIRTUAL", "PARTITION", "BY", "RANGE",
    "LIST", "HASH", "CHECK", "UNIQUE", "TEMPORARY", "INCREMENT", "START",
  ]);
  const TYPES = new Set([
    "INT", "BIGINT", "SMALLINT", "TINYINT", "MEDIUMINT", "VARCHAR", "CHAR",
    "DECIMAL", "NUMERIC", "FLOAT", "DOUBLE", "REAL", "TEXT", "LONGTEXT",
    "MEDIUMTEXT", "TINYTEXT", "BLOB", "LONGBLOB", "DATETIME", "TIMESTAMP",
    "DATE", "TIME", "YEAR", "BIT", "BOOLEAN", "BOOL", "ENUM", "JSON", "UUID",
    "SERIAL", "BIGSERIAL", "TINYBLOB", "MEDIUMBLOB", "VARBINARY", "BINARY",
  ]);
  // 逐 token 扫描：注释 / 字符串 / 反引号标识符 / 单词
  const re = /(--[^\n]*|\/\*[\s\S]*?\*\/|'(?:[^'\\]|\\.|'')*'|"(?:[^"\\]|\\.)*"|`[^`]*`|[A-Za-z_][A-Za-z0-9_]*|\d+(?:\.\d+)?|\s+|.)/g;
  let out = "";
  for (const m of sql.matchAll(re)) {
    const tok = m[0];
    const first = tok[0];
    if (first === "-" || first === "/") {
      out += `<span class="ddl-comment">${esc(tok)}</span>`;
    } else if (first === "'" || first === '"') {
      out += `<span class="ddl-str">${esc(tok)}</span>`;
    } else if (first === "`") {
      out += `<span class="ddl-ident">${esc(tok)}</span>`;
    } else if (/^[A-Za-z_]/.test(tok)) {
      const up = tok.toUpperCase();
      if (KEYWORDS.has(up)) out += `<span class="ddl-kw">${esc(tok)}</span>`;
      else if (TYPES.has(up)) out += `<span class="ddl-type">${esc(tok)}</span>`;
      else out += esc(tok);
    } else {
      out += esc(tok);
    }
  }
  return out;
}
