/**
 * 手写虚拟滚动数据网格。
 *
 * 规格（实测，物理 px ÷1.25 = 逻辑 px）：
 *  - 表头高 28 物理px（22.4 逻辑px），白底，底部 1px #DBDBDB
 *  - 行高 24 物理px（19.2 逻辑px），数据区白底无网格线
 *  - 列头点击排序（显示小三角），列宽可拖拽
 *  - NULL 显示为灰色斜体（可切换跳过）；行号列灰字，焦点行使用浅强调色
 *  - 虚拟滚动：只渲染可视区域的行
 */

import type { ResultColumn } from "../core/types";
import { iconSvgByName } from "./icons";

export interface GridCallbacks {
  /** 单元格开始编辑。 */
  onEditStart?: (row: number, col: number) => void;
  /** 单元格编辑结束（确认提交；Escape 取消不触发）。 */
  onEditEnd?: (row: number, col: number, value: string | null) => void;
  /** 列头点击（排序；提供 onColumnClick 时被其替代）。 */
  onSort?: (col: string) => void;
  /** 列头点击（选中列）。提供时表头单击走此回调而非排序。 */
  onColumnClick?: (col: string, index: number) => void;
  /** 焦点单元格变化（单击单元格）。 */
  onCellFocus?: (row: number, col: number) => void;
  /** 单元格进入编辑态（输入框已聚焦并全选）后触发；用于刷新内容预览面板。 */
  onEditBegin?: (row: number, col: number) => void;
  /** 行选中变化。 */
  onSelectionChange?: (rows: number[]) => void;
  /** 双击。 */
  onRowDoubleClick?: (row: number) => void;
  /** 右键。 */
  onContextMenu?: (x: number, y: number, row: number, col: number) => void;
  /** 列头右键（列名，索引，视口坐标）——用于升序 / 降序 / 移除排序菜单。 */
  onHeaderContextMenu?: (col: string, index: number, x: number, y: number) => void;
}

export interface GridOptions {
  columns: ResultColumn[];
  rows: (string | null)[][];
  /** 当前排序列。 */
  orderBy?: string | null;
  ascending?: boolean;
  /** 多字段排序，数组顺序为优先级。 */
  orderRules?: import("../core/types").SortRule[];
  /** 正在编辑的单元格。 */
  editing?: { row: number; col: number } | null;
  /** 选中的行。 */
  selectedRows?: Set<number>;
  /** 主键列名（高亮显示）。 */
  primaryKeys?: string[];
  /** 是否只读。 */
  readOnly?: boolean;
  /** 行号列宽。 */
  rowHeaderWidth?: number;
  /** 显示行号列（数据网格 true，对象网格 false）。 */
  showRowNumbers?: boolean;
  /** 显示行号的起点偏移；数据索引和选择/编辑仍使用当前页的本地索引。 */
  rowNumberOffset?: number;
  /** 右对齐的列名（数字列）。 */
  rightAlign?: string[];
  /** 每行首列图标（资源名），用于对象网格。 */
  rowIcons?: string[];
  /** 是否显示淡网格线（查看菜单「显示网格」）。 */
  lined?: boolean;
  /** 跳过 Null（NULL 单元格显示为空白）。 */
  skipNull?: boolean;
  /**
   * 列宽铺满容器：按内容量宽后，把容器剩余宽度按比例分给各列，右侧不留大片空白。
   * 手动拖拽过的列保持用户宽度，不再参与分摊；内容本就超出容器时不压缩。
   */
  fillWidth?: boolean;
  /** 行标记（未提交状态：insert=新增行 / delete=待删除行 / dirty=已修改行）。 */
  rowFlags?: Record<number, "insert" | "delete" | "dirty">;
  /** 待提交单元格集合（"row:col" 键）。 */
  dirtyCells?: Set<string>;
}

/** 兜底行高/表头高（逻辑px）。真实值运行时取自 base.css 的 --h-row / --h-header，
 *  避免 TS 常量与 CSS 各写一份、差 35% 导致可视行数算错。 */
const DEFAULT_ROW_HEIGHT = 26;
const DEFAULT_HEADER_HEIGHT = 28;
const OVERSCAN = 8;

type TemporalPickerType = "date" | "time" | "datetime-local";

function temporalPickerType(type: string): TemporalPickerType | null {
  if (/^(?:datetime\d*|timestamp(?:tz)?|smalldatetime)\b/i.test(type.trim())) return "datetime-local";
  if (/^time(?:tz)?\b/i.test(type.trim())) return "time";
  return /^date\b/i.test(type.trim()) ? "date" : null;
}

// 只拆原文中的墙上时间；不经过 Date，避免时区换算和精度截断。
const CLOCK_PATTERN = /^(\d{2}:\d{2})(?::(\d{2})(\.\d+)?)?(\s*(?:[zZ]|[+-]\d{2}(?::?\d{2})?))?$/;
const DATETIME_PATTERN = /^(\d{4}-\d{2}-\d{2})([ T])(.*)$/;

function pickerValue(type: TemporalPickerType, raw: string): string {
  if (type === "date") return /^\d{4}-\d{2}-\d{2}$/.test(raw) ? raw : "";
  const date = type === "datetime-local" ? raw.match(DATETIME_PATTERN) : null;
  if (type === "datetime-local" && !date) return "";
  const clock = (date?.[3] ?? raw).match(CLOCK_PATTERN);
  if (!clock || Number(clock[1].slice(0, 2)) > 23) return "";
  const time = `${clock[1]}:${clock[2] ?? "00"}`;
  return date ? `${date[1]}T${time}` : time;
}

function mergePickerValue(type: TemporalPickerType, raw: string, picked: string): string {
  // 清空选择器只清其辅助选择，不清权威文本；空串通过文本输入明确设置。
  if (!picked) return raw;
  if (type === "date") return picked;
  const oldDate = type === "datetime-local" ? raw.match(DATETIME_PATTERN) : null;
  const newDate = type === "datetime-local" ? picked.match(DATETIME_PATTERN) : null;
  const oldClock = (oldDate?.[3] ?? raw).match(CLOCK_PATTERN);
  const newClock = (newDate?.[3] ?? picked).match(CLOCK_PATTERN);
  if (!newClock) return raw;
  const seconds = newClock[2] ?? "00";
  const fraction = oldClock?.[3] ?? "";
  const clock = newClock[1] + (oldClock?.[2] || seconds !== "00" || fraction ? `:${seconds}` : "") + fraction;
  const offset = oldClock?.[4] ?? "";
  return (newDate ? `${newDate[1]}${oldDate?.[2] ?? "T"}` : "") + clock + offset;
}

export class DataGrid {
  readonly el: HTMLElement;
  private scroll: HTMLElement;
  private table: HTMLTableElement;
  private thead: HTMLTableSectionElement;
  private tbody: HTMLTableSectionElement;
  private opts: GridOptions;
  private cb: GridCallbacks;
  private colWidths: number[] = [];
  /** 内容量出的基准列宽（fillWidth 分摊前），容器变宽时按它重新分配。 */
  private baseWidths: number[] = [];
  /** 用户手动调过宽度的列：fillWidth 时固定为用户宽，不参与剩余空间分摊。 */
  private userSizedCols = new Set<number>();
  private defaultWidth = 96;
  /** 列宽自适应下限 / 上限（逻辑px）。 */
  private static readonly MIN_COL_WIDTH = 64;
  private static readonly MAX_COL_WIDTH = 320;
  /** 真实行高（逻辑px）。窗口缩放 / DPI / 主题变化后由 measureMetrics() 刷新。 */
  private rowHeight = DEFAULT_ROW_HEIGHT;
  private headerHeight = DEFAULT_HEADER_HEIGHT;
  /** 容器尺寸变化时重算可视行窗口，否则改窗口大小后仍按旧高度渲染，底部留白或行数不足。 */
  private resizeObserver: ResizeObserver | null = null;
  private resizing: { col: number; startX: number; startW: number } | null = null;
  private resizerEl: HTMLElement | null = null;
  private resizeOverlay: HTMLElement | null = null;
  private previousBodyCursor = "";
  private previousUserSelect = "";
  private editingInput: HTMLInputElement | null = null;
  private editSession: {
    row: HTMLTableRowElement;
    editor: HTMLElement;
    commit: () => void;
  } | null = null;
  /** 列宽拖拽结束后吞掉下一次表头 click（否则会误触发排序）。 */
  private suppressNextHeaderClick = false;
  /** 选中的列（-1 无）。点击列头/单元格选中整列，右侧信息面板联动列信息。 */
  private selectedCol = -1;
  private copyGeneration = 0;
  private contextTarget: { row: number; col: number } | null = null;
  private frozenContext: import("../core/grid-clipboard").GridCopySnapshot | null = null;
  /** 最近一次渲染的可视窗口；滚动不跨窗口时不重建 DOM（否则逐像素滚动会整表闪烁）。 */
  private lastWindow = { first: -1, last: -1, total: -1, cols: -1 };
  /** 滚动渲染的 rAF 合并句柄。 */
  private renderHandle = 0;

  constructor(cb: GridCallbacks = {}) {
    this.cb = cb;
    this.opts = { columns: [], rows: [] };

    this.el = document.createElement("div");
    this.el.className = "grid-wrap";

    this.scroll = document.createElement("div");
    this.scroll.className = "grid-scroll";

    this.table = document.createElement("table");
    this.table.className = "grid-table";

    this.thead = document.createElement("thead");
    this.tbody = document.createElement("tbody");
    this.table.appendChild(this.thead);
    this.table.appendChild(this.tbody);

    this.scroll.appendChild(this.table);
    this.el.appendChild(this.scroll);

    // 键盘导航需要本组件可获得焦点（点击行/单元格后聚焦）
    this.el.tabIndex = 0;

    this.scroll.addEventListener("scroll", () => this.scheduleRenderRows());
    this.scroll.addEventListener("mousedown", this.onMouseDown);
    this.scroll.addEventListener("dblclick", this.onDoubleClick);
    this.scroll.addEventListener("contextmenu", this.onContextMenu);
    this.scroll.addEventListener("keydown", this.onKeyDown);
    this.thead.addEventListener("click", this.onHeaderClick);
    this.thead.addEventListener("mousedown", this.onHeaderMouseDown);
    this.thead.addEventListener("mousemove", this.onHeaderHover);
    this.thead.addEventListener("mouseleave", this.onHeaderLeave);
    this.thead.addEventListener("contextmenu", this.onHeaderContextMenu);
    this.thead.addEventListener("keydown", this.onResizeKeyDown);
    window.addEventListener("mousemove", this.onMouseMove);
    window.addEventListener("mouseup", this.onMouseUp);
    window.addEventListener("blur", this.onMouseUp);

    // 网格/容器尺寸变化（拖动分栏、窗口缩放、侧栏展开）必须重算可视行窗口，
    // 否则渲染窗口停在旧尺寸：窗口变大只留空白、变小则行数溢出。
    if (typeof ResizeObserver !== "undefined") {
      this.resizeObserver = new ResizeObserver(() => this.handleContainerResize());
      this.resizeObserver.observe(this.scroll);
      this.resizeObserver.observe(this.el);
    }
  }

  dispose(): void {
    this.endEdit();
    this.onMouseUp();
    this.resizeObserver?.disconnect();
    this.resizeObserver = null;
    window.removeEventListener("mousemove", this.onMouseMove);
    window.removeEventListener("mouseup", this.onMouseUp);
    window.removeEventListener("blur", this.onMouseUp);
    this.thead.removeEventListener("mousemove", this.onHeaderHover);
    this.thead.removeEventListener("mouseleave", this.onHeaderLeave);
    this.thead.removeEventListener("contextmenu", this.onHeaderContextMenu);
    this.thead.removeEventListener("keydown", this.onResizeKeyDown);
    if (this.renderHandle) {
      cancelAnimationFrame(this.renderHandle);
      this.renderHandle = 0;
    }
  }

  /** 上一次看到的视口尺寸；尺寸没变就不重算，避免 ResizeObserver/scroll 空转。 */
  private lastViewport = { w: -1, h: -1 };

  /** 容器尺寸变化：重新量行高并重算可视行窗口。 */
  private handleContainerResize(): void {
    const w = this.scroll.clientWidth;
    const h = this.scroll.clientHeight;
    if (w === this.lastViewport.w && h === this.lastViewport.h) return;
    this.lastViewport = { w, h };
    this.measureMetrics();
    // renderRows 会应用 fillWidth 的最新列宽，同时重算可视行窗口。
    this.renderRows();
  }

  /**
   * 从实际渲染结果量取行高/表头高。
   * 优先读 CSS 变量（唯一真值来源），其次量一行真实 DOM，最后用兜底常量。
   */
  private measureMetrics(): void {
    const cs = getComputedStyle(this.el);
    const rowVar = parseFloat(cs.getPropertyValue("--h-row"));
    const headerVar = parseFloat(cs.getPropertyValue("--h-header"));
    if (Number.isFinite(rowVar) && rowVar > 0) this.rowHeight = rowVar;
    if (Number.isFinite(headerVar) && headerVar > 0) this.headerHeight = headerVar;
    const tr = this.tbody.querySelector<HTMLElement>("tr[data-row]");
    const measured = tr?.getBoundingClientRect().height ?? 0;
    if (measured > 0) this.rowHeight = measured;
    const th = this.thead.querySelector<HTMLElement>("th");
    const measuredHeader = th?.getBoundingClientRect().height ?? 0;
    if (measuredHeader > 0) this.headerHeight = measuredHeader;
  }

  /** 更新数据并重绘。 */
  setData(opts: GridOptions): void {
    ++this.copyGeneration;
    // 换页/刷新/隐藏列使旧显示坐标失效；先取消，移除输入时不能误提交到新数据。
    this.endEdit();
    this.lastFocused = null;
    this.contextTarget = null;
    // 数据已换：旧的整列选中高亮不再对应任何选中，与「没选中就不高亮、不预览」一起清掉。
    this.selectedCol = -1;
    // 数据已变化：强制下一次 renderRows 重建可视窗口。
    this.lastWindow = { first: -1, last: -1, total: -1, cols: -1 };
    const columnsChanged =
      opts.columns.length !== this.opts.columns.length ||
      opts.columns.some((c, i) => c.name !== this.opts.columns[i]?.name);
    this.opts = { ...this.opts, ...opts };
    if (columnsChanged) {
      // 列宽自适应：按列名与样本数据量宽，至少完整展示列名（不再均分固定宽）。
      this.baseWidths = this.measureColumnWidths();
      this.colWidths = this.baseWidths.slice();
      this.userSizedCols.clear();
      this.renderHeader();
    } else {
      if (this.opts.fillWidth) {
        // 同列刷新也重新量内容，保留用户锁定宽度（空列表加载出数据时尤其重要）。
        this.baseWidths = this.measureColumnWidths();
        this.colWidths = this.baseWidths.map((w, i) => this.userSizedCols.has(i) ? this.colWidths[i] : w);
      }
      this.updateHeaderState();
    }
    this.updateColumnSelectionVisual();
    this.renderRows();
  }

  /**
   * 列宽自适应：以列名完整展示为下限，并采样可见数据量宽。
   * 用 canvas measureText 代替逐格 DOM 测量；采样前 80 行足够反映内容宽度。
   * canvas 不可用时退回固定默认宽（保证任何环境下都可渲染）。
   */
  private measureColumnWidths(): number[] {
    const fallback = () => this.opts.columns.map(() => this.defaultWidth);
    try {
      const canvas = this.measureCanvas ??= document.createElement("canvas");
      const ctx = canvas.getContext?.("2d");
      if (!ctx) return fallback();
    const thStyle = getComputedStyle((this.opts.fillWidth && this.thead.querySelector("th")) || this.thead);
    const tdStyle = getComputedStyle((this.opts.fillWidth && this.tbody.querySelector("td[data-col]")) || this.tbody);
    // 表头字号加粗（与 base.css 的表头样式一致），数据用正文字体。
    ctx.font = `${thStyle.fontStyle} ${thStyle.fontWeight} ${thStyle.fontSize} ${thStyle.fontFamily}`;
    const headerPad = 18; // 内边距 + 排序三角/拖宽手柄余量
    const widths = this.opts.columns.map((col) => {
      const headerW = ctx.measureText(col.name).width + headerPad + 12;
      return Math.ceil(Math.min(DataGrid.MAX_COL_WIDTH, Math.max(DataGrid.MIN_COL_WIDTH, headerW)));
    });
    ctx.font = `${tdStyle.fontStyle} ${tdStyle.fontWeight} ${tdStyle.fontSize} ${tdStyle.fontFamily}`;
    const cellPad = 16;
    const sample = Math.min(this.opts.rows.length, 80);
    for (let r = 0; r < sample; r++) {
      const row = this.opts.rows[r] ?? [];
      row.forEach((value, c) => {
        if (value == null || c >= widths.length) return;
        const iconPad = this.opts.fillWidth && c === 0 && this.opts.rowIcons?.[r] ? 23 : 0;
        const w = ctx.measureText(value).width + cellPad + iconPad;
        if (w > widths[c]) widths[c] = Math.ceil(Math.min(DataGrid.MAX_COL_WIDTH, w));
      });
    }
      return widths;
    } catch {
      // 测量环境不可用（无 canvas / 计算样式异常）：退回固定默认宽。
      return fallback();
    }
  }
  private measureCanvas: HTMLCanvasElement | null = null;

  /** 设置显式列宽（逻辑px，按参考截图实测）。 */
  setColWidths(ws: number[]): void {
    this.colWidths = ws.slice(0, this.opts.columns.length);
    while (this.colWidths.length < this.opts.columns.length) this.colWidths.push(this.defaultWidth);
    this.baseWidths = this.colWidths.slice();
    this.userSizedCols.clear();
    this.renderHeader();
    this.applyColumnWidths();
  }

  /** 只更新选中与编辑态。 */
  setState(opts: Partial<GridOptions>): void {
    if (opts.rows || opts.columns) {
      ++this.copyGeneration;
      this.endEdit();
    }
    if (opts.readOnly) this.endEdit();
    this.lastWindow = { first: -1, last: -1, total: -1, cols: -1 };
    this.opts = { ...this.opts, ...opts };
    this.renderRows();
  }

  /** 当前选项（视图层读取）。 */
  getOptions(): Readonly<GridOptions> {
    return this.opts;
  }

  /** 渲染表头。 */
  private renderHeader(): void {
    this.thead.innerHTML = "";
    const tr = document.createElement("tr");

    if (this.opts.showRowNumbers !== false) {
      const th0 = document.createElement("th");
      th0.className = "grid-row-header";
      th0.textContent = "";
      tr.appendChild(th0);
    }

    const rightSet = new Set(this.opts.rightAlign ?? []);
    this.opts.columns.forEach((col, i) => {
      const th = document.createElement("th");
      th.style.width = `${this.colWidths[i]}px`;
      const inner = document.createElement("div");
      inner.className = rightSet.has(col.name) ? "grid-th-inner align-right" : "grid-th-inner";
      const name = document.createElement("div");
      name.className = "grid-th-name";
      name.textContent = col.name;
      // 列头不设 title：悬停列名不再弹出提示框（列信息由点击列头/列数据在右侧面板展示）。
      const sort = document.createElement("div");
      sort.className = "grid-sort";
      inner.appendChild(name);
      inner.appendChild(sort);
      th.appendChild(inner);
      th.dataset.col = String(i);
      th.dataset.name = col.name;
      // 独立命中区使用自身光标，避免 th 的 cursor:default 覆盖继承值。
      const handle = document.createElement("div");
      handle.className = "grid-column-resize-handle";
      handle.tabIndex = 0;
      handle.setAttribute("role", "separator");
      handle.setAttribute("aria-orientation", "vertical");
      handle.setAttribute("aria-label", `调整 ${col.name} 列宽`);
      handle.setAttribute("aria-valuemin", "40");
      handle.setAttribute("aria-valuenow", String(this.colWidths[i]));
      handle.title = "拖动调整列宽；方向键微调（Shift 加速）";
      Object.assign(handle.style, {
        position: "absolute", right: "0", top: "0", bottom: "0", width: "8px",
        zIndex: "4", cursor: "col-resize", borderRight: "1px solid var(--border)",
        touchAction: "none",
      });
      const hot = (on: boolean) => { handle.style.background = on ? "var(--accent-100)" : ""; };
      handle.addEventListener("mouseenter", () => hot(true));
      handle.addEventListener("mouseleave", () => hot(false));
      handle.addEventListener("focus", () => hot(true));
      handle.addEventListener("blur", () => hot(false));
      th.appendChild(handle);
      if (i === this.selectedCol) th.classList.add("col-selected");
      tr.appendChild(th);
    });

    this.thead.appendChild(tr);
    this.updateHeaderState();
    this.applyColumnWidths();
  }

  /** 更新排序标记。 */
  private updateHeaderState(): void {
    this.thead.querySelectorAll<HTMLElement>("th[data-col]").forEach((th) => {
      const name = th.dataset.name ?? "";
      const sort = th.querySelector<HTMLElement>(".grid-sort");
      if (!sort) return;
      sort.innerHTML = "";
      const rules = this.opts.orderRules ?? (this.opts.orderBy
        ? [{ column: this.opts.orderBy, ascending: !!this.opts.ascending }]
        : []);
      const priority = rules.findIndex((r) => r.column === name);
      th.removeAttribute("aria-sort");
      if (priority >= 0) {
        const ascending = rules[priority].ascending;
        th.setAttribute("aria-sort", priority === 0 ? (ascending ? "ascending" : "descending") : "other");
        sort.innerHTML = ascending
          ? '<svg viewBox="0 0 9 9" width="9" height="9"><path d="M4.5 1 L8.2 7.4 H0.8 z" fill="currentColor"/></svg>'
          : '<svg viewBox="0 0 9 9" width="9" height="9"><path d="M4.5 8 L0.8 1.6 H8.2 z" fill="currentColor"/></svg>';
        if (rules.length > 1) {
          sort.appendChild(document.createTextNode(String(priority + 1)));
          sort.style.width = "24px";
          sort.style.minWidth = "24px";
        }
        sort.title = `优先级 ${priority + 1}，${ascending ? "升序" : "降序"}`;
      } else {
        sort.style.width = "";
        sort.style.minWidth = "";
        sort.title = "";
      }
    });
  }

  /** 应用列宽到表头与表体（table-layout:fixed 需要表格有确定总宽）。 */
  private applyColumnWidths(): void {
    const widths = this.effectiveColWidths();
    this.thead.querySelectorAll<HTMLElement>("th[data-col]").forEach((th) => {
      const i = Number(th.dataset.col);
      th.style.width = `${widths[i]}px`;
      th.querySelector(".grid-column-resize-handle")?.setAttribute("aria-valuenow", String(widths[i]));
    });
    this.tbody.querySelectorAll<HTMLElement>("tr").forEach((tr) => {
      tr.querySelectorAll<HTMLElement>("td[data-col]").forEach((td) => {
        const i = Number(td.dataset.col);
        td.style.width = `${widths[i]}px`;
      });
    });
    const sum = widths.reduce((a, b) => a + b, 0);
    const rowHeader = this.rowHeaderPx();
    this.table.style.setProperty("--grid-row-header-width", `${rowHeader}px`);
    this.table.style.width = `${sum + rowHeader}px`;
  }

  /** 行号列宽（不显示行号时为 0）。 */
  private rowHeaderPx(): number {
    if (this.opts.showRowNumbers === false) return 0;
    return this.opts.rowHeaderWidth
      ?? (this.opts.rowNumberOffset !== undefined ? Math.max(40, String(this.opts.rowNumberOffset + this.opts.rows.length).length * 7 + 16) : 40);
  }

  /**
   * 实际渲染用的列宽。
   * fillWidth 开启时：内容量出的基准宽先铺满容器，剩余宽度按基准宽比例分摊给各列，
   * 右侧不再留大片空白；用户手动拖过的列锁定用户宽度、不参与分摊；
   * 基准宽本来就超出容器（需要横向滚动）时不压缩，保持可读。
   */
  private effectiveColWidths(): number[] {
    const widths = this.colWidths.slice();
    if (!this.opts.fillWidth) return widths;
    const available = this.scroll.clientWidth - this.rowHeaderPx();
    if (!(available > 0) || widths.length === 0) return widths;
    const isFree = (i: number) => !this.userSizedCols.has(i);
    const contentSum = widths.reduce((a, b) => a + b, 0);
    if (contentSum >= available) return widths;
    const freeIndexes = widths.map((_, i) => i).filter(isFree);
    const freeBase = freeIndexes.reduce((a, i) => a + (this.baseWidths[i] ?? widths[i]), 0);
    if (freeIndexes.length === 0 || freeBase <= 0) return widths;
    const extra = available - contentSum;
    let used = 0;
    for (const i of freeIndexes) {
      const share = Math.floor((extra * (this.baseWidths[i] ?? widths[i])) / freeBase);
      widths[i] += share;
      used += share;
    }
    // 取整余数补给最后一个自由列，保证总宽正好等于容器宽（否则留 1~n px 白边）。
    widths[freeIndexes[freeIndexes.length - 1]] += extra - used;
    return widths;
  }

  /** 虚拟滚动渲染（rAF 合并滚动事件；可视窗口没变时跳过重建，避免逐像素滚动整表闪烁）。 */
  private scheduleRenderRows(): void {
    if (this.renderHandle) return;
    this.renderHandle = requestAnimationFrame(() => {
      this.renderHandle = 0;
      this.renderRows();
    });
  }

  /** 虚拟滚动渲染行。 */
  private renderRows(): void {
    this.table.classList.toggle("grid-lined", !!this.opts.lined);
    const total = this.opts.rows.length;
    const viewH = this.scroll.clientHeight || 400;
    // 行高以 CSS 变量为准（首次读变量，之后可由已渲染行校准）
    if (!Number.isFinite(this.rowHeight) || this.rowHeight <= 0) this.measureMetrics();
    const rowH = this.rowHeight;
    // 数据缩量后 scrollTop 可能越界，先钳位（否则渲染一片空白）。
    // 内容总高要把 sticky 表头算进去：它仍在滚动流里占高，漏掉会让最后一行滚不出底部。
    const contentH = total * rowH + this.headerHeight;
    const maxTop = Math.max(0, contentH - viewH);
    if (this.scroll.scrollTop > maxTop) {
      this.scroll.scrollTop = maxTop;
    }
    const scrollTop = this.scroll.scrollTop;
    const first = Math.max(0, Math.floor(scrollTop / rowH) - OVERSCAN);
    const visible = Math.ceil(viewH / rowH) + OVERSCAN * 2;
    const last = Math.min(total, first + visible);
    const colCount = Math.max(1, this.opts.columns.length + (this.opts.showRowNumbers === false ? 0 : 1));
    // 可视窗口与上次完全一致（同数据量、同列数、同选中列）时不重建：
    // 这是查询结果滚动「卡住后闪动」的根源 —— 每个滚动事件整表重建 tbody。
    if (
      this.lastWindow.first === first && this.lastWindow.last === last &&
      this.lastWindow.total === total && this.lastWindow.cols === colCount &&
      this.tbody.childElementCount > 0
    ) {
      // 编辑中的单元格也留在 tbody 里，跳过重建正好保护它不被移除。
      this.applyColumnWidths();
      return;
    }
    this.lastWindow = { first, last, total, cols: colCount };

    // 编辑行是活 DOM 锚点，始终连接在原 tbody 上；即使暂时移入 fragment 也会丢焦点。
    // 只重建锚点前后的兄弟，窗口外最多多保留这一行，间隙仍按真实行高撑开。
    const anchor = this.editSession?.row ?? null;
    const editRow = anchor ? Number(anchor.dataset.row) : -1;
    const indices = Array.from({ length: last - first }, (_, i) => first + i);
    if (editRow >= 0 && (editRow < first || editRow >= last)) {
      indices.push(editRow);
      indices.sort((a, b) => a - b);
    }
    const frag = anchor ? null : document.createDocumentFragment();
    if (anchor) {
      for (const child of Array.from(this.tbody.children)) {
        if (child !== anchor) child.remove();
      }
    }
    let beforeAnchor = !!anchor;
    const insert = (node: HTMLTableRowElement) => frag ? frag.appendChild(node) : this.tbody.insertBefore(node, beforeAnchor ? anchor : null);
    let nextRow = 0;
    for (const r of indices) {
      if (r > nextRow) insert(this.buildSpacerRow(colCount, (r - nextRow) * rowH));
      if (r === editRow) beforeAnchor = false;
      else insert(this.buildRow(r));
      nextRow = r + 1;
    }
    if (nextRow < total) insert(this.buildSpacerRow(colCount, (total - nextRow) * rowH));
    if (frag) {
      this.tbody.innerHTML = "";
      this.tbody.appendChild(frag);
    }
    this.applyColumnWidths();
    this.updateSelectionVisual();
    this.updateColumnSelectionVisual();
  }

  /** 撑高用的空行（不参与选中/斑马纹/占位统计）。 */
  private buildSpacerRow(colCount: number, height: number): HTMLTableRowElement {
    const tr = document.createElement("tr");
    tr.className = "grid-spacer";
    tr.setAttribute("aria-hidden", "true");
    const td = document.createElement("td");
    td.colSpan = colCount;
    td.style.height = `${height}px`;
    tr.appendChild(td);
    return tr;
  }

  /** 构造一行。 */
  private buildRow(r: number): HTMLTableRowElement {
    const tr = document.createElement("tr");
    tr.dataset.row = String(r);
    // 按数据索引而非 nth-child 决定斑马纹：占位行插在首尾不会让条纹整体翻转
    if (r % 2 === 1) tr.classList.add("grid-row-alt");
    const flag = this.opts.rowFlags?.[r];
    if (flag === "delete") tr.classList.add("row-deleted");
    if (this.opts.selectedRows?.has(r)) tr.classList.add("selected");

    if (this.opts.showRowNumbers !== false) {
      const rowHeader = document.createElement("td");
      rowHeader.className = "grid-row-header";
      const rowNumber = (this.opts.rowNumberOffset ?? 0) + r + 1;
      if (flag === "insert") {
        rowHeader.classList.add("row-flag");
        rowHeader.textContent = `+${rowNumber}`;
      } else if (flag === "delete") {
        rowHeader.classList.add("row-flag");
        rowHeader.textContent = `✕${rowNumber}`;
      } else if (flag === "dirty") {
        rowHeader.classList.add("row-flag");
        rowHeader.textContent = `●${rowNumber}`;
      } else {
        rowHeader.textContent = String(rowNumber);
      }
      if (this.lastFocused && this.lastFocused.row === r) {
        rowHeader.classList.add("focused-col");
      }
      tr.appendChild(rowHeader);
    }

    const row = this.opts.rows[r] ?? [];
    const rightSet = new Set(this.opts.rightAlign ?? []);
    this.opts.columns.forEach((col, c) => {
      const td = document.createElement("td");
      td.dataset.col = String(c);
      td.dataset.row = String(r);
      if (rightSet.has(col.name)) td.classList.add("align-right");
      if (c === this.selectedCol) td.classList.add("col-selected");
      if (this.opts.dirtyCells?.has(`${r}:${c}`)) td.classList.add("grid-cell-dirty");
      const value = row[c];
      if (c === 0 && this.opts.rowIcons?.[r]) {
        td.innerHTML = `<span class="grid-cell-icon">${iconSvgByName(this.opts.rowIcons[r])}</span>`;
        td.appendChild(document.createTextNode(value ?? ""));
      } else if (value === null || value === undefined) {
        if (!this.opts.skipNull) {
          td.classList.add("grid-cell-null");
          td.textContent = "NULL";
        }
      } else {
        td.textContent = value;
      }
      tr.appendChild(td);
    });
    return tr;
  }

  // ------------------------------------------------------------ 交互

  private onHeaderClick = (e: MouseEvent): void => {
    // 列宽拖拽松手会派发一次 click，这里吞掉，避免"拖一次列宽就重排序一次"
    if (this.suppressNextHeaderClick) {
      this.suppressNextHeaderClick = false;
      return;
    }
    if ((e.target as HTMLElement).closest(".grid-column-resize-handle")) return;
    const th = (e.target as HTMLElement).closest<HTMLElement>("th[data-col]");
    if (!th) return;
    const name = th.dataset.name ?? "";
    const i = Number(th.dataset.col);
    if (this.cb.onColumnClick) {
      // 表数据页：表头单击 = 选中列（再点取消），排序走排序面板/表头右键。
      // 就地切换高亮，不重建 DOM（行元素保持稳定）。
      this.selectedCol = this.selectedCol === i ? -1 : i;
      this.updateColumnSelectionVisual();
      this.cb.onColumnClick(this.selectedCol >= 0 ? name : "", this.selectedCol);
      return;
    }
    this.cb.onSort?.(name);
  };

  /** 命中边缘时直接设在 th 上，而不是被 th 默认光标遮蔽的父节点。 */
  private onHeaderHover = (e: MouseEvent): void => {
    if (this.resizing) return;
    this.thead.querySelectorAll<HTMLElement>("th[data-col]").forEach((th) => { th.style.cursor = ""; });
    const th = (e.target as HTMLElement).closest<HTMLElement>("th[data-col]");
    if (th && e.clientX >= th.getBoundingClientRect().right - 8) th.style.cursor = "col-resize";
  };

  private onHeaderLeave = (): void => {
    if (this.resizing) return;
    this.thead.querySelectorAll<HTMLElement>("th[data-col]").forEach((th) => { th.style.cursor = ""; });
  };

  /** 列头右键：交给宿主弹升序 / 降序 / 移除排序菜单（与排序面板联动）。 */
  private onHeaderContextMenu = (e: MouseEvent): void => {
    const th = (e.target as HTMLElement).closest<HTMLElement>("th[data-col]");
    if (!th || !this.cb.onHeaderContextMenu) return;
    e.preventDefault();
    e.stopPropagation();
    this.cb.onHeaderContextMenu(th.dataset.name ?? "", Number(th.dataset.col), e.clientX, e.clientY);
  };

  private onResizeKeyDown = (e: KeyboardEvent): void => {
    const handle = (e.target as HTMLElement).closest<HTMLElement>(".grid-column-resize-handle");
    if (!handle || !["ArrowLeft", "ArrowRight", "Home"].includes(e.key)) return;
    e.preventDefault();
    e.stopPropagation();
    const th = handle.closest<HTMLElement>("th[data-col]");
    if (!th) return;
    const i = Number(th.dataset.col);
    if (e.key === "Home") {
      // Home = 恢复基准宽：同时解除用户锁定，重新参与 fillWidth 分摊。
      this.colWidths[i] = this.opts.fillWidth ? this.baseWidths[i] ?? this.defaultWidth : this.defaultWidth;
      this.userSizedCols.delete(i);
    } else {
      // 必须先取分摊后的宽，再锁定该列，否则首个方向键会跳回内容基准宽。
      const current = this.effectiveColWidths()[i] ?? this.colWidths[i];
      this.userSizedCols.add(i);
      this.colWidths[i] = Math.max(40, current + (e.key === "ArrowLeft" ? -1 : 1) * (e.shiftKey ? 24 : 8));
    }
    this.applyColumnWidths();
  };

  private onHeaderMouseDown = (e: MouseEvent): void => {
    if (e.button !== 0) return;
    const th = (e.target as HTMLElement).closest<HTMLElement>("th[data-col]");
    if (!th) return;
    const i = Number(th.dataset.col);
    const rect = th.getBoundingClientRect();
    if ((e.target as HTMLElement).closest(".grid-column-resize-handle") || e.clientX >= rect.right - 8) {
      e.preventDefault();
      this.suppressNextHeaderClick = true;
      this.resizing = { col: i, startX: e.clientX, startW: this.effectiveColWidths()[i] };
      // 视口坐标配 fixed 定位，水平滚动/DPI 下不混用 thead 局部坐标。
      const r = document.createElement("div");
      r.className = "grid-resizer";
      Object.assign(r.style, {
        position: "fixed", top: `${rect.top}px`, left: `${rect.right - 1}px`, width: "2px",
        height: `${Math.max(this.headerHeight, this.scroll.getBoundingClientRect().bottom - rect.top)}px`,
        zIndex: "10000", background: "var(--accent-500)", pointerEvents: "none",
      });
      const width = document.createElement("span");
      Object.assign(width.style, {
        position: "absolute", top: "2px", right: "4px", padding: "2px 5px",
        background: "var(--bg-surface)", color: "var(--text-accent)",
        border: "1px solid var(--accent-500)", borderRadius: "var(--r-1)",
        fontSize: "var(--fs-11)", fontVariantNumeric: "tabular-nums", whiteSpace: "nowrap",
      });
      width.textContent = `${Math.round(this.resizing.startW)} px`;
      r.appendChild(width);
      // 全屏透明拖动层让指针移到单元格/工具栏仍保持 col-resize，避免子元素默认光标覆盖 body。
      const overlay = document.createElement("div");
      overlay.className = "grid-resize-overlay";
      Object.assign(overlay.style, { position: "fixed", inset: "0", zIndex: "9999", cursor: "col-resize", background: "transparent" });
      document.body.appendChild(overlay);
      this.resizeOverlay = overlay;
      document.body.appendChild(r);
      this.resizerEl = r;
      this.previousBodyCursor = document.body.style.cursor;
      this.previousUserSelect = document.body.style.userSelect;
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      this.el.classList.add("grid-resizing");
      this.el.style.cursor = "col-resize";
    }
  };

  private onMouseMove = (e: MouseEvent): void => {
    if (!this.resizing) return;
    const delta = e.clientX - this.resizing.startX;
    if (delta === 0 && !this.userSizedCols.has(this.resizing.col)) return;
    const w = Math.max(40, this.resizing.startW + delta);
    this.userSizedCols.add(this.resizing.col);
    this.colWidths[this.resizing.col] = w;
    this.applyColumnWidths();
    const th = this.thead.querySelector<HTMLElement>(`th[data-col="${this.resizing.col}"]`);
    if (th && this.resizerEl) {
      this.resizerEl.style.left = `${th.getBoundingClientRect().right - 1}px`;
      const label = this.resizerEl.querySelector("span");
      if (label) label.textContent = `${Math.round(w)} px`;
    }
  };

  private onMouseUp = (): void => {
    if (!this.resizing) return;
    this.resizing = null;
    this.resizerEl?.remove();
    this.resizerEl = null;
    this.resizeOverlay?.remove();
    this.resizeOverlay = null;
    document.body.style.cursor = this.previousBodyCursor;
    document.body.style.userSelect = this.previousUserSelect;
    this.el.classList.remove("grid-resizing");
    this.el.style.cursor = "";
    this.onHeaderLeave();
    // 仅吞掉拖动当次 click，不吞掉下一次正常表头选择/排序。
    setTimeout(() => { this.suppressNextHeaderClick = false; }, 0);
  };

  private onMouseDown = (e: MouseEvent): void => {
    if (this.editSession?.editor.contains(e.target as Node)) return;
    if (e.button === 2) return; // 右键在已选范围内保留多选，命中信息由 contextmenu 冻结。
    this.frozenContext = null;
    const td = (e.target as HTMLElement).closest<HTMLElement>("td[data-col]");
    if (!td) {
      // 点击行号：与行内点击完全一致（选中行 + 焦点行号高亮 + 焦点单元格）
      const tr = (e.target as HTMLElement).closest<HTMLElement>("tr[data-row]");
      if (tr) {
        const r = Number(tr.dataset.row);
        this.lastMouseDownAt = Date.now();
        this.moveFocus(r, this.lastFocused?.col ?? 0);
      }
      return;
    }
    const r = Number(td.dataset.row);
    const c = Number(td.dataset.col);
    const selected = new Set(this.opts.selectedRows ?? []);
    if (e.ctrlKey || e.metaKey) {
      if (selected.has(r)) selected.delete(r);
      else selected.add(r);
    } else {
      selected.clear();
      selected.add(r);
    }
    this.opts.selectedRows = selected;
    this.cb.onSelectionChange?.([...selected]);

    // 双击进入编辑由 dblclick 处理；单击选中焦点
    this.el.focus?.();
    this.lastFocused = { row: r, col: c };
    this.lastMouseDownAt = Date.now();
    this.cb.onCellFocus?.(r, c);
    // 只就地更新选中 / 焦点高亮，不重建 DOM —— 与树面板同理：
    // mousedown 重建行元素后 dblclick 的目标会被替换，事件永远无法送达，
    // 只读网格（对象页）的「双击打开表 / 视图」就失效了。
    this.updateSelectionVisual();
    // 点击列数据 = 同时选中整列，右侧「列信息」面板联动显示该列。
    if (this.cb.onColumnClick && this.selectedCol !== c) {
      this.selectedCol = c;
      this.updateColumnSelectionVisual();
      this.cb.onColumnClick(this.opts.columns[c]?.name ?? "", c);
    }
  };

  /** 就地更新整列选中高亮（表头 + 可视行），不重建 DOM。 */
  private updateColumnSelectionVisual(): void {
    this.thead.querySelectorAll<HTMLElement>("th[data-col]").forEach((th) => {
      th.classList.toggle("col-selected", Number(th.dataset.col) === this.selectedCol);
    });
    this.tbody.querySelectorAll<HTMLElement>("td[data-col]").forEach((td) => {
      td.classList.toggle("col-selected", Number(td.dataset.col) === this.selectedCol);
    });
  }

  /** 只更新选中与焦点行号高亮（不重建 DOM）。 */
  private updateSelectionVisual(): void {
    this.tbody.querySelectorAll<HTMLTableRowElement>("tr[data-row]").forEach((tr) => {
      tr.classList.toggle("selected", !!this.opts.selectedRows?.has(Number(tr.dataset.row)));
    });
    this.tbody.querySelectorAll<HTMLElement>(".grid-row-header.focused-col").forEach((el) => {
      el.classList.remove("focused-col");
    });
    if (this.lastFocused) {
      const head = this.tbody.querySelector<HTMLElement>(
        `tr[data-row="${this.lastFocused.row}"] .grid-row-header`);
      head?.classList.add("focused-col");
    }
  }

  private lastFocused: { row: number; col: number } | null = null;
  /** 最近一次命中行的 mousedown 时刻（双击目标解析失败时兜底）。 */
  private lastMouseDownAt = 0;

  private onDoubleClick = (e: MouseEvent): void => {
    if (this.editSession?.editor.contains(e.target as Node)) return;
    let target = e.target as HTMLElement | null;
    // document 等节点没有 closest
    if (!target || typeof target.closest !== "function") target = null;
    const td = target?.closest<HTMLElement>("td[data-col]") ?? null;
    if (!td) {
      const tr = target?.closest<HTMLElement>("tr[data-row]") ?? null;
      const row = tr ? Number(tr.dataset.row) : this.dblClickFallbackRow();
      if (row !== null) this.cb.onRowDoubleClick?.(row);
      return;
    }
    const r = Number(td.dataset.row);
    const c = Number(td.dataset.col);
    if (this.opts.readOnly) {
      // 只读网格（对象 / 备份 / 用户 / 调度）：双击单元格 = 打开该行
      this.cb.onRowDoubleClick?.(r);
      return;
    }
    this.cb.onEditStart?.(r, c);
  };

  /** 双击时目标解析失败（DOM 被重建）的兜底：取最近点击的行。 */
  private dblClickFallbackRow(): number | null {
    if (Date.now() - this.lastMouseDownAt > 400) return null;
    return this.lastFocused ? this.lastFocused.row : null;
  }

  private onContextMenu = (e: MouseEvent): void => {
    if (this.editSession?.editor.contains(e.target as Node)) return;
    // 列头的专属菜单由 thead 分发，不能被通用网格菜单覆盖。
    if (this.cb.onHeaderContextMenu && (e.target as HTMLElement).closest("th[data-col]")) return;
    e.preventDefault();
    const td = (e.target as HTMLElement).closest<HTMLElement>("td[data-col]");
    const tr = (e.target as HTMLElement).closest<HTMLElement>("tr[data-row]");
    const r = td ? Number(td.dataset.row) : tr ? Number(tr.dataset.row) : -1;
    const c = td ? Number(td.dataset.col) : -1;
    this.contextTarget = r >= 0 ? { row: r, col: c } : null;
    this.frozenContext = this.captureCopy(true);
    this.el.focus?.();
    this.cb.onContextMenu?.(e.clientX, e.clientY, r, c);
  };

  private onKeyDown = (e: KeyboardEvent): void => {
    if (!this.lastFocused) return;
    const { row, col } = this.lastFocused;
    const total = this.opts.rows.length;
    if (!total) return;
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        this.moveFocus(Math.min(total - 1, row + 1), col);
        break;
      case "ArrowUp":
        e.preventDefault();
        this.moveFocus(Math.max(0, row - 1), col);
        break;
      case "ArrowRight":
        e.preventDefault();
        this.moveFocus(row, Math.min(this.opts.columns.length - 1, col + 1));
        break;
      case "ArrowLeft":
        e.preventDefault();
        this.moveFocus(row, Math.max(0, col - 1));
        break;
      case "Enter":
        e.preventDefault();
        if (!e.repeat && !this.opts.readOnly) this.cb.onEditStart?.(row, col);
        break;
      case "F2":
        e.preventDefault();
        if (!this.opts.readOnly) this.cb.onEditStart?.(row, col);
        break;
      case "Escape":
        // 取消焦点（不提交任何修改）
        e.preventDefault();
        break;
    }
  };

  /** 当前显示数组就是筛选/排序/隐藏列后的顺序；立即冻结原值和选择。 */
  private captureCopy(context = false): import("../core/grid-clipboard").GridCopySnapshot {
    const target = context ? this.contextTarget : this.lastFocused;
    const selected = this.opts.selectedRows ?? new Set<number>();
    const rows = target && !selected.has(target.row) && context ? [target.row] :
      selected.size ? [...selected].sort((a, b) => a - b) : target ? [target.row] : [];
    return {
      generation: this.copyGeneration,
      columns: this.opts.columns.map(c => ({ name: c.name })),
      rows: rows.filter(r => r >= 0 && r < this.opts.rows.length).map(r => [...this.opts.rows[r]]),
      cell: target && target.row >= 0 && target.col >= 0 ? this.opts.rows[target.row]?.[target.col] : undefined,
    };
  }
  freezeCopy(context = false): import("../core/grid-clipboard").GridCopySnapshot {
    if (context && this.frozenContext) {
      if (this.frozenContext.generation !== this.copyGeneration) throw new Error("结果已刷新，旧菜单选择已失效，请重新选择");
      return structuredClone(this.frozenContext);
    }
    return this.captureCopy();
  }
  flushEditing(): void { this.editSession?.commit(); }

  private moveFocus(row: number, col: number): void {
    this.frozenContext = null;
    this.contextTarget = null;
    this.lastFocused = { row, col };
    const selected = new Set(this.opts.selectedRows ?? []);
    selected.clear();
    selected.add(row);
    this.opts.selectedRows = selected;
    this.el.focus?.();
    this.cb.onSelectionChange?.([row]);
    this.cb.onCellFocus?.(row, col);
    // 就地更新高亮，保持 DOM 稳定（行号列双击依赖它）；
    // 需要滚动时 scroll 事件会触发 renderRows 重建可视窗口
    this.updateSelectionVisual();
    // 滚动到可见区域
    const top = row * this.rowHeight;
    const bottom = top + this.rowHeight;
    const viewTop = this.scroll.scrollTop;
    const viewBottom = viewTop + (this.scroll.clientHeight || 400);
    if (top < viewTop) this.scroll.scrollTop = top;
    else if (bottom > viewBottom) this.scroll.scrollTop = bottom - (this.scroll.clientHeight || 400);
  }

  /** 在指定单元格显示编辑输入框。 */
  beginEdit(row: number, col: number, current: string | null): HTMLInputElement | null {
    this.endEdit();
    if (this.opts.readOnly || !this.opts.columns[col] || !this.opts.rows[row]) return null;
    const generation = this.copyGeneration;
    const tr = this.tbody.querySelector<HTMLTableRowElement>(`tr[data-row="${row}"]`);
    if (!tr) return null;
    const td = tr.querySelector<HTMLElement>(`td[data-col="${col}"]`);
    if (!td) return null;
    td.classList.add("grid-cell-editing");
    const editor = document.createElement("div");
    editor.className = "grid-cell-editor";
    Object.assign(editor.style, { position: "absolute", inset: "0", display: "flex" });
    const input = document.createElement("input");
    input.className = "grid-cell-input";
    input.type = "text";
    input.setAttribute("aria-label", `编辑 ${this.opts.columns[col].name}`);
    Object.assign(input.style, { position: "relative", flex: "1", minWidth: "0" });
    input.value = current ?? "";
    editor.appendChild(input);
    let modified = false;
    const type = temporalPickerType(this.opts.columns[col].type);
    let picker: HTMLInputElement | null = null;
    if (type) {
      picker = document.createElement("input");
      picker.type = type;
      picker.className = "grid-cell-picker";
      picker.step = "1";
      picker.setAttribute("aria-label", `选择 ${this.opts.columns[col].name} 的${type === "date" ? "日期" : type === "time" ? "时间" : "日期时间"}`);
      picker.title = type === "time" ? "时间选择器；负数或超过24小时请直接输入" : "选择日期或时间；保留文本中的小数秒和时区";
      // 紧凑旁置控件保留原生指示器与键盘入口；完整值始终在左侧文本中可读可改。
      Object.assign(picker.style, {
        flex: "0 0 28px", width: "28px", minWidth: "0", height: "100%", padding: "0 2px",
        border: "1px solid var(--accent-500)", background: "var(--bg-surface)", color: "var(--text)",
      });
      picker.value = pickerValue(type, input.value);
      picker.addEventListener("change", () => {
        const value = mergePickerValue(type, input.value, picker!.value);
        if (value !== input.value) { input.value = value; modified = true; }
      });
      editor.appendChild(picker);
    }
    input.addEventListener("input", () => {
      modified = true;
      if (picker && type) picker.value = pickerValue(type, input.value);
    });
    input.addEventListener("beforeinput", () => { modified = true; });
    const commit = () => {
      if (this.editingInput !== input || generation !== this.copyGeneration) return;
      const value = current === null && !modified && input.value === "" ? null : input.value;
      this.endEdit();
      if (generation === this.copyGeneration) this.cb.onEditEnd?.(row, col, value);
    };
    editor.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.isComposing) return;
      if (e.target === input && (e.key === "Backspace" || e.key === "Delete")) modified = true;
      if (e.key === "Enter") {
        e.preventDefault();
        commit();
        this.el.focus({ preventScroll: true });
      } else if (e.key === "Escape") {
        e.preventDefault();
        this.endEdit();
        this.el.focus({ preventScroll: true });
      }
    });
    editor.addEventListener("focusout", (e) => {
      if (e.relatedTarget instanceof Node && editor.contains(e.relatedTarget)) return;
      // 原生 picker 与文本之间换焦点属于同一次编辑；真正离开组件才提交。
      queueMicrotask(() => { if (!editor.contains(document.activeElement)) commit(); });
    });
    this.editingInput = input;
    this.editSession = { row: tr, editor, commit };
    td.appendChild(editor);
    input.focus({ preventScroll: true });
    input.select();
    this.cb.onEditBegin?.(row, col);
    return input;
  }

  /** 结束编辑。 */
  endEdit(): void {
    const session = this.editSession;
    if (!session) return;
    // 先使所有事件失效，再移除编辑器，防止 blur/Enter 重入重复提交。
    this.editingInput = null;
    this.editSession = null;
    session.editor.remove();
    this.tbody.querySelectorAll(".grid-cell-editing").forEach((el) => {
      el.classList.remove("grid-cell-editing");
    });
    // 编辑结束（提交或取消）后内容不再选中：通知宿主收起单元格内容面板。
    // 与 onEditBegin 成对，避免退出编辑后面板残留（BUG-R1-017）。
    this.cb.onEditBegin?.(this.lastFocused?.row ?? -1, this.lastFocused?.col ?? -1);
    this.lastWindow = { first: -1, last: -1, total: -1, cols: -1 };
    this.scheduleRenderRows();
  }

  /** 导出为纯文本（复制用）。 */
  getText(rows: number[], includeHeader = true): string {
    const lines: string[] = [];
    if (includeHeader) {
      lines.push(this.opts.columns.map((c) => c.name).join("\t"));
    }
    for (const r of rows) {
      const row = this.opts.rows[r] ?? [];
      lines.push(
        row
          .map((v) => (v === null || v === undefined ? "NULL" : v))
          .join("\t"),
      );
    }
    return lines.join("\r\n");
  }

  /** 当前列。 */
  get columns(): ResultColumn[] {
    return this.opts.columns;
  }
  get rows(): (string | null)[][] {
    return this.opts.rows;
  }

  /** 焦点单元格（无则 null）。 */
  get focusedCell(): { row: number; col: number } | null {
    return this.lastFocused;
  }

  /** 选中列索引（-1 无）。 */
  get selectedColumn(): number {
    return this.selectedCol;
  }

  /** 设置选中列（-1 清除）。 */
  set selectedColumn(index: number) {
    this.selectedCol = index;
    this.updateColumnSelectionVisual();
  }

  /** 是否有单元格正在编辑（文本预览面板只在编辑选中内容时出现）。 */
  get editingActive(): boolean {
    return !!this.editingInput;
  }
}
