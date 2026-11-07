/**
 * 手写虚拟滚动数据网格。
 *
 * 规格（实测，物理 px ÷1.25 = 逻辑 px）：
 *  - 表头高 28 物理px（22.4 逻辑px），白底，底部 1px #DBDBDB
 *  - 行高 24 物理px（19.2 逻辑px），数据区白底无网格线
 *  - 列头点击排序（显示小三角），列宽可拖拽
 *  - NULL 显示为灰色斜体（可切换跳过）；行号列灰字，焦点行行号深蓝白字
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
  /** 行选中变化。 */
  onSelectionChange?: (rows: number[]) => void;
  /** 双击。 */
  onRowDoubleClick?: (row: number) => void;
  /** 右键。 */
  onContextMenu?: (x: number, y: number, row: number, col: number) => void;
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
  /** 右对齐的列名（数字列）。 */
  rightAlign?: string[];
  /** 每行首列图标（资源名），用于对象网格。 */
  rowIcons?: string[];
  /** 是否显示淡网格线（查看菜单「显示网格」）。 */
  lined?: boolean;
  /** 跳过 Null（NULL 单元格显示为空白）。 */
  skipNull?: boolean;
  /** 行标记（未提交状态：insert=新增行 / delete=待删除行 / dirty=已修改行）。 */
  rowFlags?: Record<number, "insert" | "delete" | "dirty">;
  /** 待提交单元格集合（"row:col" 键）。 */
  dirtyCells?: Set<string>;
}

const ROW_HEIGHT = 19.2;
const HEADER_HEIGHT = 22.4;
const OVERSCAN = 8;

export class DataGrid {
  readonly el: HTMLElement;
  private scroll: HTMLElement;
  private table: HTMLTableElement;
  private thead: HTMLTableSectionElement;
  private tbody: HTMLTableSectionElement;
  private opts: GridOptions;
  private cb: GridCallbacks;
  private colWidths: number[] = [];
  private defaultWidth = 96;
  private resizing: { col: number; startX: number; startW: number } | null = null;
  private resizerEl: HTMLElement | null = null;
  private resizeOverlay: HTMLElement | null = null;
  private previousBodyCursor = "";
  private previousUserSelect = "";
  private editingInput: HTMLInputElement | null = null;
  /** 列宽拖拽结束后吞掉下一次表头 click（否则会误触发排序）。 */
  private suppressNextHeaderClick = false;
  /** 选中的列（-1 无）。点击列头选中整列，右侧信息面板联动列信息。 */
  private selectedCol = -1;

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
    this.el.style.outline = "none";

    this.scroll.addEventListener("scroll", () => this.renderRows());
    this.scroll.addEventListener("mousedown", this.onMouseDown);
    this.scroll.addEventListener("dblclick", this.onDoubleClick);
    this.scroll.addEventListener("contextmenu", this.onContextMenu);
    this.scroll.addEventListener("keydown", this.onKeyDown);
    this.thead.addEventListener("click", this.onHeaderClick);
    this.thead.addEventListener("mousedown", this.onHeaderMouseDown);
    this.thead.addEventListener("mousemove", this.onHeaderHover);
    this.thead.addEventListener("mouseleave", this.onHeaderLeave);
    this.thead.addEventListener("keydown", this.onResizeKeyDown);
    window.addEventListener("mousemove", this.onMouseMove);
    window.addEventListener("mouseup", this.onMouseUp);
    window.addEventListener("blur", this.onMouseUp);
  }

  dispose(): void {
    this.onMouseUp();
    window.removeEventListener("mousemove", this.onMouseMove);
    window.removeEventListener("mouseup", this.onMouseUp);
    window.removeEventListener("blur", this.onMouseUp);
    this.thead.removeEventListener("mousemove", this.onHeaderHover);
    this.thead.removeEventListener("mouseleave", this.onHeaderLeave);
    this.thead.removeEventListener("keydown", this.onResizeKeyDown);
  }

  /** 更新数据并重绘。 */
  setData(opts: GridOptions): void {
    const columnsChanged =
      opts.columns.length !== this.opts.columns.length ||
      opts.columns.some((c, i) => c.name !== this.opts.columns[i]?.name);
    this.opts = { ...this.opts, ...opts };
    if (columnsChanged) {
      this.colWidths = this.opts.columns.map(() => this.defaultWidth);
      this.renderHeader();
    } else {
      this.updateHeaderState();
    }
    this.renderRows();
  }

  /** 设置显式列宽（逻辑px，按参考截图实测）。 */
  setColWidths(ws: number[]): void {
    this.colWidths = ws.slice(0, this.opts.columns.length);
    while (this.colWidths.length < this.opts.columns.length) this.colWidths.push(this.defaultWidth);
    this.renderHeader();
    this.applyColumnWidths();
  }

  /** 只更新选中与编辑态。 */
  setState(opts: Partial<GridOptions>): void {
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
      th.title = `${col.name} (${col.type})`;
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
    this.thead.querySelectorAll<HTMLElement>("th[data-col]").forEach((th) => {
      const i = Number(th.dataset.col);
      th.style.width = `${this.colWidths[i]}px`;
      th.querySelector(".grid-column-resize-handle")?.setAttribute("aria-valuenow", String(this.colWidths[i]));
    });
    this.tbody.querySelectorAll<HTMLElement>("tr").forEach((tr) => {
      tr.querySelectorAll<HTMLElement>("td[data-col]").forEach((td) => {
        const i = Number(td.dataset.col);
        td.style.width = `${this.colWidths[i]}px`;
      });
    });
    const sum = this.colWidths.reduce((a, b) => a + b, 0);
    const rowHeader = this.opts.showRowNumbers === false ? 0 : 40;
    this.table.style.width = `${sum + rowHeader}px`;
  }

  /** 虚拟滚动渲染行。 */
  private renderRows(): void {
    this.table.classList.toggle("grid-lined", !!this.opts.lined);
    const total = this.opts.rows.length;
    const viewH = this.scroll.clientHeight || 400;
    // 数据缩量后 scrollTop 可能越界，先钳位（否则渲染一片空白）
    const maxTop = Math.max(0, total * ROW_HEIGHT - viewH);
    if (this.scroll.scrollTop > maxTop) {
      this.scroll.scrollTop = maxTop;
    }
    const scrollTop = this.scroll.scrollTop;
    const first = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
    const visible = Math.ceil(viewH / ROW_HEIGHT) + OVERSCAN * 2;
    const last = Math.min(total, first + visible);

    const frag = document.createDocumentFragment();
    for (let r = first; r < last; r++) {
      frag.appendChild(this.buildRow(r));
    }
    this.tbody.innerHTML = "";
    this.tbody.appendChild(frag);

    // 通过 tbody 的 padding 模拟滚动偏移
    this.tbody.style.paddingTop = `${first * ROW_HEIGHT}px`;
    this.tbody.style.paddingBottom = `${Math.max(0, (total - last) * ROW_HEIGHT)}px`;
  }

  /** 构造一行。 */
  private buildRow(r: number): HTMLTableRowElement {
    const tr = document.createElement("tr");
    tr.dataset.row = String(r);
    const flag = this.opts.rowFlags?.[r];
    if (flag === "delete") tr.classList.add("row-deleted");
    if (this.opts.selectedRows?.has(r)) tr.classList.add("selected");

    if (this.opts.showRowNumbers !== false) {
      const rowHeader = document.createElement("td");
      rowHeader.className = "grid-row-header";
      if (flag === "insert") {
        rowHeader.classList.add("row-flag");
        rowHeader.textContent = `+${r + 1}`;
      } else if (flag === "delete") {
        rowHeader.classList.add("row-flag");
        rowHeader.textContent = `✕${r + 1}`;
      } else if (flag === "dirty") {
        rowHeader.classList.add("row-flag");
        rowHeader.textContent = `●${r + 1}`;
      } else {
        rowHeader.textContent = String(r + 1);
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
      // 表数据页：表头单击 = 选中列（再点取消），排序走排序面板
      this.selectedCol = this.selectedCol === i ? -1 : i;
      this.renderHeader();
      this.renderRows();
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

  private onResizeKeyDown = (e: KeyboardEvent): void => {
    const handle = (e.target as HTMLElement).closest<HTMLElement>(".grid-column-resize-handle");
    if (!handle || !["ArrowLeft", "ArrowRight", "Home"].includes(e.key)) return;
    e.preventDefault();
    e.stopPropagation();
    const th = handle.closest<HTMLElement>("th[data-col]");
    if (!th) return;
    const i = Number(th.dataset.col);
    this.colWidths[i] = e.key === "Home" ? this.defaultWidth
      : Math.max(40, this.colWidths[i] + (e.key === "ArrowLeft" ? -1 : 1) * (e.shiftKey ? 24 : 8));
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
      this.resizing = { col: i, startX: e.clientX, startW: this.colWidths[i] };
      // 视口坐标配 fixed 定位，水平滚动/DPI 下不混用 thead 局部坐标。
      const r = document.createElement("div");
      r.className = "grid-resizer";
      Object.assign(r.style, {
        position: "fixed", top: `${rect.top}px`, left: `${rect.right - 1}px`, width: "2px",
        height: `${Math.max(HEADER_HEIGHT, this.scroll.getBoundingClientRect().bottom - rect.top)}px`,
        zIndex: "10000", background: "var(--accent-500)", pointerEvents: "none",
      });
      const width = document.createElement("span");
      Object.assign(width.style, {
        position: "absolute", top: "2px", right: "4px", padding: "2px 5px",
        background: "var(--bg-surface)", color: "var(--text-accent)",
        border: "1px solid var(--accent-500)", borderRadius: "var(--r-1)",
        fontSize: "var(--fs-11)", fontVariantNumeric: "tabular-nums", whiteSpace: "nowrap",
      });
      width.textContent = `${Math.round(this.colWidths[i])} px`;
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
    const w = Math.max(40, this.resizing.startW + delta);
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
  };

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
    e.preventDefault();
    const td = (e.target as HTMLElement).closest<HTMLElement>("td[data-col]");
    const tr = (e.target as HTMLElement).closest<HTMLElement>("tr[data-row]");
    const r = td ? Number(td.dataset.row) : tr ? Number(tr.dataset.row) : -1;
    const c = td ? Number(td.dataset.col) : -1;
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
        if (!this.opts.readOnly) this.cb.onEditStart?.(row, col);
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

  private moveFocus(row: number, col: number): void {
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
    const top = row * ROW_HEIGHT;
    const bottom = top + ROW_HEIGHT;
    const viewTop = this.scroll.scrollTop;
    const viewBottom = viewTop + (this.scroll.clientHeight || 400);
    if (top < viewTop) this.scroll.scrollTop = top;
    else if (bottom > viewBottom) this.scroll.scrollTop = bottom - (this.scroll.clientHeight || 400);
  }

  /** 在指定单元格显示编辑输入框。 */
  beginEdit(row: number, col: number, current: string | null): HTMLInputElement | null {
    this.endEdit();
    const tr = this.tbody.querySelector<HTMLElement>(`tr[data-row="${row}"]`);
    if (!tr) return null;
    const td = tr.querySelector<HTMLElement>(`td[data-col="${col}"]`);
    if (!td) return null;
    td.classList.add("grid-cell-editing");
    const input = document.createElement("input");
    input.className = "grid-cell-input";
    input.value = current ?? "";
    td.appendChild(input);
    input.focus();
    input.select();
    this.editingInput = input;
    const commit = () => {
      const v = input.value;
      const had = this.editingInput === input;
      this.endEdit();
      if (had) this.cb.onEditEnd?.(row, col, v === "" ? null : v); // 空串 = NULL
    };
    input.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") {
        e.preventDefault();
        commit();
      } else if (e.key === "Escape") {
        // 取消编辑：不提交任何值
        e.preventDefault();
        this.endEdit();
      }
    });
    input.addEventListener("blur", () => {
      if (this.editingInput === input) {
        commit();
      }
    });
    return input;
  }

  /** 结束编辑。 */
  endEdit(): void {
    if (this.editingInput) {
      // 先解除引用再移除：remove() 会同步触发 blur，blur 里的 commit() 会重入 endEdit()，
      // 若此时 editingInput 仍指向已移除节点，会对同一节点再次 remove() 并抛 NotFoundError。
      const input = this.editingInput;
      this.editingInput = null;
      input.remove();
    }
    this.tbody.querySelectorAll(".grid-cell-editing").forEach((el) => {
      el.classList.remove("grid-cell-editing");
    });
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
    this.renderHeader();
    this.renderRows();
  }
}
