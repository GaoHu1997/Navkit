/** 左侧连接树：搜索框、展开/折叠、右键菜单。 */

import type { ConnectionConfig, ConnectionMovePosition, TreeItem } from "../core/types";
import { ICON_SEARCH, ICON_TREE_ARROW, svg, iconSvgByName } from "./icons";
import { kindIcon } from "../core/dbcatalog";

export interface TreeCallbacks {
  /** 切换展开。 */
  onToggle?: (node: TreeItem) => void;
  /** 选中节点。 */
  onSelect?: (node: TreeItem) => void;
  /** 双击。 */
  onOpen?: (node: TreeItem) => void;
  /** 右键菜单。 */
  onContextMenu?: (node: TreeItem, x: number, y: number) => void;
  /** 空白处右键菜单（新建连接 / 新建组 / 刷新）。 */
  onBlankContextMenu?: (x: number, y: number) => void;
  /** 移到分组末尾；targetId + position 指定该组连接前/后的位置。 */
  onMoveConnection?: (connectionId: string, group: string, targetId?: string, position?: ConnectionMovePosition) => void | Promise<void>;
  /** 过滤变化。 */
  onFilter?: (text: string) => void;
}

export class TreePanel {
  readonly el: HTMLElement;
  private searchBox: HTMLInputElement;
  private scroll: HTMLElement;
  private cb: TreeCallbacks;
  private nodes: TreeItem[] = [];
  private selectedId: string | null = null;
  private filterText = "";
  private draggedConnectionId: string | null = null;
  private keyboardConnectionId: string | null = null;
  private moving = false;
  private rootDrop: HTMLButtonElement;
  private moveStatus: HTMLElement;
  private readonly moveHelp = "连接可拖入文件夹分组，或拖到连接上半部/下半部排在前/后。键盘：Alt+↑/↓ 同组排序；Ctrl+X 选取连接，Ctrl+V 放到分组或连接前，Ctrl+Shift+V 放到连接后；在未分组按钮按 Enter 移到根层，Esc 取消。";

  constructor(cb: TreeCallbacks = {}) {
    this.cb = cb;

    this.el = document.createElement("div");
    this.el.className = "tree-panel";

    // 搜索框
    const search = document.createElement("div");
    search.className = "tree-search";
    const box = document.createElement("div");
    box.className = "tree-search-box";
    const glass = document.createElement("span");
    glass.innerHTML = svg(ICON_SEARCH);
    glass.style.lineHeight = "0";
    this.searchBox = document.createElement("input");
    this.searchBox.type = "text";
    this.searchBox.placeholder = "搜索对象、表或连接";
    this.searchBox.setAttribute("aria-label", "搜索对象、表或连接");
    glass.setAttribute("aria-hidden", "true");
    this.searchBox.addEventListener("input", () => {
      this.filterText = this.searchBox.value.trim().toLowerCase();
      this.cb.onFilter?.(this.filterText);
      this.render();
    });
    box.appendChild(glass);
    box.appendChild(this.searchBox);
    search.appendChild(box);

    // 树容器
    this.scroll = document.createElement("div");
    this.scroll.className = "tree-scroll";
    this.scroll.setAttribute("role", "tree");
    this.scroll.setAttribute("aria-label", "数据源连接");
    this.scroll.addEventListener("dragover", (e) => this.handleDragOver(e));
    this.scroll.addEventListener("drop", (e) => this.handleDrop(e));
    this.scroll.addEventListener("dragleave", (e) => {
      if (!this.scroll.contains(e.relatedTarget as Node | null)) this.clearDropIndicator();
    });
    // 永远可见的根层落点，也是不依赖拖放的键盘入口。
    this.rootDrop = document.createElement("button");
    this.rootDrop.type = "button";
    this.rootDrop.className = "tree-root-drop";
    this.rootDrop.textContent = "未分组 · 拖放连接到此处";
    this.rootDrop.title = this.moveHelp;
    this.rootDrop.setAttribute("aria-label", "移到未分组：选取连接后按 Enter");
    this.rootDrop.addEventListener("dragover", (e) => this.handleDragOver(e, true));
    this.rootDrop.addEventListener("drop", (e) => this.handleDrop(e, true));
    this.rootDrop.addEventListener("dragleave", () => this.clearDropIndicator());
    this.rootDrop.addEventListener("click", () => {
      if (this.keyboardConnectionId) void this.requestMove(this.keyboardConnectionId, "");
      else this.moveStatus.textContent = this.moveHelp;
    });
    this.moveStatus = document.createElement("div");
    this.moveStatus.className = "tree-move-status";
    this.moveStatus.textContent = "Alt+↑/↓ 排序 · Ctrl+X/V 跨组移动";
    this.moveStatus.setAttribute("role", "status");
    this.moveStatus.setAttribute("aria-live", "polite");
    this.el.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && this.keyboardConnectionId) {
        this.keyboardConnectionId = null;
        this.moveStatus.textContent = "已取消移动连接";
        this.updateSelection();
        e.preventDefault();
        e.stopPropagation();
      }
    });

    // 空白处右键：弹出「新建连接 / 新建组 / 刷新」菜单
    // （节点上的右键由行处理器 stopPropagation，不会落到这里）
    this.scroll.addEventListener("contextmenu", (e) => {
      if ((e.target as HTMLElement).closest?.(".tree-node")) return;
      e.preventDefault();
      this.selectedId = null;
      this.updateSelection();
      this.cb.onBlankContextMenu?.(e.clientX, e.clientY);
    });

    this.el.appendChild(search);
    this.el.appendChild(this.scroll);
    this.el.appendChild(this.rootDrop);
    this.el.appendChild(this.moveStatus);
  }

  /** 设置树数据。 */
  setNodes(nodes: TreeItem[]): void {
    this.nodes = nodes;
    this.render();
  }

  /** 选中某节点。 */
  select(id: string | null): void {
    this.selectedId = id;
    this.updateSelection();
  }

  /** 就地更新状态，避免单击后的图标同步破坏双击目标。 */
  updateNodes(): void {
    this.scroll.querySelectorAll<HTMLElement>(".tree-node").forEach((row) => {
      const node = this.find(row.dataset.id ?? "");
      if (!node) return;
      const icon = row.querySelector<HTMLElement>(".tree-icon");
      // 仅在图标真的变化时才替换 SVG 子树：mousedown 里同步图标状态会重建
      // 图标节点，使 mouseup/dblclick 的目标与 mousedown 不同，双击图标失效。
      if (icon && icon.dataset.icon !== node.icon) {
        icon.innerHTML = iconSvgByName(node.icon);
        icon.dataset.icon = node.icon;
      }
      const arrow = row.querySelector<HTMLElement>(".tree-arrow");
      if (arrow) {
        arrow.classList.toggle("empty", !node.expandable);
        arrow.classList.toggle("collapsed", !node.expanded);
        if (node.expandable && !arrow.firstChild) arrow.innerHTML = svg(ICON_TREE_ARROW);
      }
      if (node.expandable) row.setAttribute("aria-expanded", String(node.expanded));
      else row.removeAttribute("aria-expanded");
      // 连接状态：打开=logo 自带外框染绿，关闭=灰阶（图标本体已由 App 切换）
      if (node.type === "connection") {
        row.classList.toggle("conn-open", !!node.connected);
        row.title = `${node.connected ? "已连接" : "未连接"}\n${this.moveHelp}`;
      }
    });
    this.updateSelection();
  }

  /** 重新渲染。 */
  render(): void {
    const active = document.activeElement as HTMLElement | null;
    const focusedId = active && this.scroll.contains(active) ? active.dataset.id : null;
    this.clearDropIndicator();
    this.scroll.innerHTML = "";
    if (this.filterText) this.renderFiltered();
    else for (const node of this.nodes) this.renderSubtree(node, 0);
    this.updateSelection();
    if (focusedId) {
      const row = [...this.scroll.querySelectorAll<HTMLElement>(".tree-node")].find((el) => el.dataset.id === focusedId);
      row?.focus({ preventScroll: true });
    }
  }

  /** 只更新选中高亮（不重建 DOM）。 */
  private updateSelection(): void {
    const rows = [...this.scroll.querySelectorAll<HTMLElement>(".tree-node")];
    const tabId = rows.some((row) => row.dataset.id === this.selectedId) ? this.selectedId : rows[0]?.dataset.id;
    rows.forEach((el) => {
      const selected = el.dataset.id === this.selectedId;
      el.classList.toggle("selected", selected);
      el.classList.toggle("move-picked", el.dataset.id === `conn:${this.keyboardConnectionId}`);
      el.setAttribute("aria-selected", String(selected));
      el.tabIndex = el.dataset.id === tabId ? 0 : -1;
    });
  }

  /** 递归渲染一个节点及其（已展开的）子节点。 */
  private renderSubtree(node: TreeItem, depth: number): void {
    this.scroll.appendChild(this.buildNode(node, depth));
    if (!node.expanded) return;
    for (const child of node.children) {
      this.renderSubtree(child, depth + 1);
    }
  }

  /** 按关键字过滤渲染。 */
  private renderFiltered(): void {
    const matches: { node: TreeItem; depth: number }[] = [];
    const walk = (list: TreeItem[], depth: number) => {
      for (const n of list) {
        if (n.label.toLowerCase().includes(this.filterText)) {
          matches.push({ node: n, depth });
        }
        if (n.children.length) walk(n.children, depth + 1);
      }
    };
    walk(this.nodes, 0);
    if (!matches.length) {
      const empty = document.createElement("div");
      empty.style.padding = "12px";
      empty.style.color = "var(--fg-muted)";
      empty.textContent = "没有匹配的项";
      this.scroll.appendChild(empty);
      return;
    }
    for (const m of matches) {
      this.scroll.appendChild(this.buildNode(m.node, m.depth, true));
    }
  }

  /** 构造一个树节点元素。 */
  private buildNode(node: TreeItem, depth: number, flat = false): HTMLElement {
    const row = document.createElement("div");
    row.className = "tree-node";
    if (this.selectedId === node.id) row.classList.add("selected");
    row.dataset.id = node.id;
    row.setAttribute("role", "treeitem");
    row.setAttribute("aria-label", node.label);
    row.setAttribute("aria-level", String(depth + 1));
    row.setAttribute("aria-selected", String(this.selectedId === node.id));
    row.tabIndex = this.selectedId === node.id ? 0 : -1;
    if (node.expandable) row.setAttribute("aria-expanded", String(node.expanded));
    if (node.type === "connection" && node.id.startsWith("conn:")) {
      row.draggable = true;
      row.title = this.moveHelp;
      row.addEventListener("dragstart", (e) => {
        if (this.moving || !this.cb.onMoveConnection || !e.dataTransfer) { e.preventDefault(); return; }
        this.draggedConnectionId = node.id.slice(5);
        e.dataTransfer.setData("application/x-navkit-connection", this.draggedConnectionId);
        e.dataTransfer.effectAllowed = "move";
        row.classList.add("dragging");
      });
      row.addEventListener("dragend", () => {
        this.draggedConnectionId = null;
        row.classList.remove("dragging");
        this.clearDropIndicator();
      });
    }
    if (node.id.startsWith("group:")) row.classList.add("group");
    row.addEventListener("keydown", (e) => this.handleNodeKey(e, node, row));
    row.addEventListener("focus", () => {
      if (this.selectedId === node.id) return;
      this.selectedId = node.id;
      this.cb.onSelect?.(node);
      this.updateSelection();
    });
    // v2 设计：缩进 14px/级（箭头 16px + 图标 18px）
    row.style.paddingLeft = `${depth * 14 + 2}px`;

    // 展开箭头
    const arrow = document.createElement("div");
    arrow.className = "tree-arrow";
    if (node.expandable && !flat) {
      arrow.innerHTML = svg(ICON_TREE_ARROW);
      if (!node.expanded) arrow.classList.add("collapsed");
      arrow.addEventListener("mousedown", (e) => {
        e.stopPropagation();
        this.cb.onToggle?.(node);
      });
    } else {
      arrow.classList.add("empty");
    }
    row.appendChild(arrow);

    // 图标
    const icon = document.createElement("div");
    icon.className = "tree-icon";
    icon.dataset.icon = node.icon;
    icon.innerHTML = iconSvgByName(node.icon);
    row.appendChild(icon);

    // 标签
    const label = document.createElement("div");
    label.className = "tree-label";
    label.textContent = node.label;
    row.appendChild(label);

    // 对象分组的子项计数（表 19 / 视图 3 …；连接分组与连接节点不显示）
    if (
      node.type === "folder" &&
      !node.id.startsWith("group:") &&
      node.children.length > 0
    ) {
      const badge = document.createElement("span");
      badge.className = "tree-badge";
      badge.textContent = String(node.children.length);
      row.appendChild(badge);
    }

    // 连接状态：由 logo 自带的圆角外框染色 + `_off` 灰阶变体表达（见 icons.ts / dbIcons.ts）
    if (node.type === "connection") {
      row.classList.add("conn-row");
      row.classList.toggle("conn-open", !!node.connected);
      row.title = `${node.connected ? "已连接" : "未连接"}\n${this.moveHelp}`;
      row.setAttribute("aria-description", this.moveHelp);
      row.setAttribute("aria-keyshortcuts", "Alt+ArrowUp Alt+ArrowDown Control+x Control+v Control+Shift+v Meta+x Meta+v Meta+Shift+v");
    }

    row.addEventListener("mousedown", (e) => {
      if (e.button === 2) return;
      this.selectedId = node.id;
      this.cb.onSelect?.(node);
      // 只更新高亮，不重建 DOM —— 否则双击时元素会被替换，
      // dblclick 事件将永远无法送达。
      this.updateSelection();
    });
    row.addEventListener("dblclick", () => this.cb.onOpen?.(node));
    row.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      e.stopPropagation();
      this.selectedId = node.id;
      this.cb.onSelect?.(node);
      this.updateSelection();
      this.cb.onContextMenu?.(node, e.clientX, e.clientY);
    });
    return row;
  }

  /** 只接受本树拖出的真实连接；不能信任外部 MIME/text 或数据库子节点。 */
  private dragId(e: DragEvent): string | null {
    if (this.moving || !this.cb.onMoveConnection || !this.draggedConnectionId ||
        !e.dataTransfer?.types.includes("application/x-navkit-connection")) return null;
    return this.connectionGroup(this.draggedConnectionId) !== null ? this.draggedConnectionId : null;
  }

  private connectionGroup(id: string): string | null {
    for (const node of this.nodes) {
      if (node.type === "connection" && node.id === `conn:${id}`) return "";
      if (node.id.startsWith("group:") && node.children.some((c) => c.type === "connection" && c.id === `conn:${id}`)) return node.id.slice(6);
    }
    return null;
  }

  private dropLocation(e: DragEvent, root: boolean): { group: string; targetId?: string; position?: ConnectionMovePosition; row: HTMLElement | null } | null {
    const element = e.target instanceof Element ? e.target : null;
    const row = root ? null : element?.closest<HTMLElement>(".tree-node") ?? null;
    if (!row) return { group: "", row: null };
    const node = this.find(row.dataset.id ?? "");
    if (node?.id.startsWith("group:")) return { group: node.id.slice(6), row };
    if (node?.type !== "connection") return null;
    const id = node.id.slice(5);
    const group = this.connectionGroup(id);
    if (group === null || id === this.draggedConnectionId) return null;
    const bounds = row.getBoundingClientRect();
    return { group, targetId: id, position: e.clientY < (bounds.top + bounds.bottom) / 2 ? "before" : "after", row };
  }

  private clearDropIndicator(): void {
    this.scroll.querySelectorAll<HTMLElement>(".tree-node").forEach((row) => row.classList.remove("drop-target", "drop-before", "drop-after"));
    this.rootDrop?.classList.remove("drop-target");
    this.scroll.classList.remove("drop-root");
  }

  private handleDragOver(e: DragEvent, root = false): void {
    this.clearDropIndicator();
    // 即使是外部文件也取消默认行为，不能让 WebView 导航离开应用。
    e.preventDefault();
    e.stopPropagation();
    if (!this.dragId(e)) { if (e.dataTransfer) e.dataTransfer.dropEffect = "none"; return; }
    const location = this.dropLocation(e, root);
    if (!location) { if (e.dataTransfer) e.dataTransfer.dropEffect = "none"; return; }
    e.preventDefault();
    e.stopPropagation();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    if (location.row) location.row.classList.add(location.position ? `drop-${location.position}` : "drop-target");
    else { this.rootDrop.classList.add("drop-target"); this.scroll.classList.add("drop-root"); }
  }

  private handleDrop(e: DragEvent, root = false): void {
    const id = this.dragId(e);
    const location = id ? this.dropLocation(e, root) : null;
    this.clearDropIndicator();
    e.preventDefault();
    e.stopPropagation();
    if (!id || !location || e.dataTransfer?.getData("application/x-navkit-connection") !== id) return;
    this.draggedConnectionId = null;
    void this.requestMove(id, location.group, location.targetId, location.position);
  }

  private async requestMove(id: string, group: string, targetId?: string, position?: ConnectionMovePosition): Promise<void> {
    if (this.moving || !this.cb.onMoveConnection || this.connectionGroup(id) === null || targetId === id) return;
    if (group && !this.nodes.some((n) => n.id === `group:${group}`)) return;
    if (targetId && this.connectionGroup(targetId) !== group) return;
    this.moving = true;
    this.rootDrop.disabled = true;
    this.scroll.setAttribute("aria-busy", "true");
    this.moveStatus.textContent = "正在保存连接顺序…";
    try {
      await this.cb.onMoveConnection(id, group, targetId, position);
      this.keyboardConnectionId = null;
      this.select(`conn:${id}`);
      this.moveStatus.textContent = targetId ? "连接顺序已保存" : group ? `已移入分组「${group}」` : "已移到未分组";
      // App 重绘后再聚焦新行，避免焦点留在已卸载节点。
      const row = [...this.scroll.querySelectorAll<HTMLElement>(".tree-node")].find((el) => el.dataset.id === `conn:${id}`);
      row?.focus({ preventScroll: true });
    } catch (e) {
      this.moveStatus.textContent = `移动失败：${String(e)}`;
    } finally {
      this.moving = false;
      this.rootDrop.disabled = false;
      this.scroll.removeAttribute("aria-busy");
      this.updateSelection();
    }
  }

  private handleNodeKey(e: KeyboardEvent, node: TreeItem, row: HTMLElement): void {
    const modifier = e.ctrlKey || e.metaKey;
    const key = e.key.toLowerCase();
    if (modifier && key === "x" && node.type === "connection") {
      e.preventDefault(); e.stopPropagation();
      if (this.moving || !this.cb.onMoveConnection) return;
      this.keyboardConnectionId = node.id.slice(5);
      this.moveStatus.textContent = `已选取「${node.label}」。到目标分组按 Ctrl+V；连接前按 Ctrl+V、后按 Ctrl+Shift+V；未分组按钮按 Enter；Esc 取消。`;
      this.updateSelection();
      return;
    }
    if (modifier && key === "v" && this.keyboardConnectionId) {
      e.preventDefault(); e.stopPropagation();
      if (node.id.startsWith("group:")) void this.requestMove(this.keyboardConnectionId, node.id.slice(6));
      else if (node.type === "connection") {
        const id = node.id.slice(5);
        const group = this.connectionGroup(id);
        if (group !== null) void this.requestMove(this.keyboardConnectionId, group, id, e.shiftKey ? "after" : "before");
      }
      return;
    }
    if (e.altKey && !modifier && (e.key === "ArrowUp" || e.key === "ArrowDown") && node.type === "connection") {
      e.preventDefault(); e.stopPropagation();
      if (this.filterText) { this.moveStatus.textContent = "请清除搜索后再用 Alt+方向键排序，或选取连接后粘贴到指定落点。"; return; }
      const id = node.id.slice(5);
      const group = this.connectionGroup(id);
      const siblings = group ? this.find(`group:${group}`)?.children ?? [] : this.nodes.filter((n) => n.type === "connection");
      const index = siblings.findIndex((n) => n.id === node.id);
      const target = siblings[index + (e.key === "ArrowUp" ? -1 : 1)];
      if (target?.type === "connection" && group !== null) void this.requestMove(id, group, target.id.slice(5), e.key === "ArrowUp" ? "before" : "after");
      return;
    }
    if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      e.preventDefault(); e.stopPropagation();
      const bounds = row.getBoundingClientRect();
      this.cb.onContextMenu?.(node, bounds.left, bounds.bottom);
      return;
    }
    if (modifier || e.altKey) return;
    if (["ArrowUp", "ArrowDown", "Home", "End"].includes(e.key)) {
      e.preventDefault(); e.stopPropagation();
      const rows = [...this.scroll.querySelectorAll<HTMLElement>(".tree-node")];
      const index = rows.indexOf(row);
      const target = e.key === "Home" ? rows[0] : e.key === "End" ? rows[rows.length - 1] : rows[index + (e.key === "ArrowUp" ? -1 : 1)];
      target?.focus();
    } else if (e.key === "Enter") { e.preventDefault(); this.cb.onOpen?.(node); }
    else if (e.key === "ArrowRight" && !node.expanded && node.expandable) { e.preventDefault(); this.cb.onToggle?.(node); }
    else if (e.key === "ArrowLeft" && node.expanded) { e.preventDefault(); this.cb.onToggle?.(node); }
  }

  /** 当前选中的节点 id。 */
  get selectedNodeId(): string | null {
    return this.selectedId;
  }

  /** 聚焦搜索框（标题栏搜索按钮入口）。 */
  focusSearch(): void {
    this.searchBox.focus();
    this.searchBox.select();
  }

  /** 查找节点。 */
  find(id: string): TreeItem | null {
    const walk = (list: TreeItem[]): TreeItem | null => {
      for (const n of list) {
        if (n.id === id) return n;
        const hit = walk(n.children);
        if (hit) return hit;
      }
      return null;
    };
    return walk(this.nodes);
  }

  /** 展开/折叠某节点（就地修改数据后重绘）。 */
  toggle(id: string): void {
    const node = this.find(id);
    if (node) {
      node.expanded = !node.expanded;
      this.render();
    }
  }

  /** 折叠全部。 */
  collapseAll(): void {
    const walk = (list: TreeItem[]) => {
      for (const n of list) {
        n.expanded = false;
        if (n.children.length) walk(n.children);
      }
    };
    walk(this.nodes);
    this.render();
  }

  /** 展开全部。 */
  expandAll(): void {
    const walk = (list: TreeItem[]) => {
      for (const n of list) {
        if (n.expandable) n.expanded = true;
        if (n.children.length) walk(n.children);
      }
    };
    walk(this.nodes);
    this.render();
  }

  /** 连接到树的分组（groupNames 为空分组提供显示入口）。 */
  static buildGroups(connections: ConnectionConfig[], groupNames: string[] = []): TreeItem[] {
    const groups = new Map<string, ConnectionConfig[]>();
    for (const g of groupNames) {
      if (g && !groups.has(g)) groups.set(g, []);
    }
    for (const c of connections) {
      const g = c.group || "";
      if (!groups.has(g)) groups.set(g, []);
      groups.get(g)!.push(c);
    }
    const out: TreeItem[] = [];
    const sorted = [...groups.keys()].sort((a, b) => {
      if (a === "") return -1;
      if (b === "") return 1;
      return a.localeCompare(b, "zh-CN");
    });
    for (const g of sorted) {
      const children: TreeItem[] = groups.get(g)!.map((c) => ({
        id: `conn:${c.id}`,
        label: c.name,
        type: "connection" as const,
        icon: kindIcon(c.kind),
        connected: false,
        children: [],
        expandable: true,
        has_children: true,
        expanded: false,
      }));
      if (g === "") {
        out.push(...children);
      } else {
        out.push({
          id: `group:${g}`,
          label: g,
          type: "folder" as const,
          icon: "info_pane_project",
          children,
          expandable: true,
          has_children: true,
          expanded: false,
        });
      }
    }
    return out;
  }
}
