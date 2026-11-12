/**
 * 可复用对象树：分组根行常显（可折叠、可全选），叶子缩进一级并支持行尾附加控件。
 * 抽自「新建备份」的 backup-tree，供数据同步 / 数据字典 / 批量导出等所有对象选择场景统一复用。
 */
import { iconSvgByName } from "./icons";

export interface ObjectTreeGroupOptions {
  title: string;
  icon: string;
  emptyText?: string;
}

export interface ObjectTreeItemOptions {
  name: string;
  icon: string;
  checked?: boolean;
  /** 复选框 aria-label，默认取 name。 */
  ariaLabel?: string;
  /** 行尾弱化说明文本（如注释）。 */
  meta?: string;
  /** 行尾附加控件（如目标表名输入框）。 */
  extra?: HTMLElement;
  /** 勾选状态变化时回调（含分组全选）。 */
  onChange?: () => void;
}

export interface ObjectTreeItemHandle {
  name: string;
  input: HTMLInputElement;
  row: HTMLElement;
  extra: HTMLElement | null;
}

export interface ObjectTreeGroupHandle {
  readonly title: string;
  readonly el: HTMLElement;
  readonly children: HTMLElement;
  readonly empty: HTMLElement;
  readonly items: ObjectTreeItemHandle[];
  refresh(): void;
}

interface ItemState extends ObjectTreeItemHandle {
  onChange?: () => void;
}

interface GroupState {
  title: string;
  el: HTMLElement;
  children: HTMLElement;
  empty: HTMLElement;
  count: HTMLElement;
  toggle: HTMLButtonElement;
  expand: HTMLButtonElement;
  collapsed: boolean;
  items: ItemState[];
  setCollapsed(next: boolean): void;
}

const TWISTY_SVG =
  '<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M3 4.5 6 7.5 9 4.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg>';

let treeSequence = 0;

export class ObjectTree {
  readonly el = document.createElement("div");
  private readonly groups: GroupState[] = [];
  private readonly stateOf = new WeakMap<ObjectTreeGroupHandle, GroupState>();

  constructor(options: { ariaLabel?: string } = {}) {
    this.el.className = "backup-tree";
    this.el.setAttribute("role", "group");
    this.el.setAttribute("aria-label", options.ariaLabel ?? "选择对象");
  }

  /** 清空所有分组（复用同一实例重建时调用）。 */
  clear(): void {
    this.groups.length = 0;
    this.el.replaceChildren();
  }

  /** 新增一个分组，返回分组句柄（用于继续 addItem / 读取 items）。 */
  addGroup(options: ObjectTreeGroupOptions): ObjectTreeGroupHandle {
    const state = this.createGroup(options);
    this.groups.push(state);
    this.el.appendChild(state.el);
    const handle: ObjectTreeGroupHandle = {
      title: state.title,
      el: state.el,
      children: state.children,
      empty: state.empty,
      items: state.items,
      refresh: () => this.refresh(state),
    };
    this.stateOf.set(handle, state);
    return handle;
  }

  /** 向分组追加一个叶子勾选行。 */
  addItem(group: ObjectTreeGroupHandle, options: ObjectTreeItemOptions): ObjectTreeItemHandle {
    const state = this.stateOf.get(group);
    if (!state) throw new Error("ObjectTree: 分组句柄不属于当前树");
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = options.checked ?? false;
    input.setAttribute("aria-label", options.ariaLabel ?? options.name);
    const icon = document.createElement("span");
    icon.className = "backup-tree-icon";
    icon.innerHTML = iconSvgByName(options.icon);
    const label = document.createElement("span");
    label.className = "backup-tree-label";
    label.textContent = options.name;

    let row: HTMLElement;
    let extraWrap: HTMLElement | null = null;
    if (options.extra || options.meta !== undefined) {
      row = document.createElement("div");
      row.className = "backup-tree-leaf is-rich";
      const main = document.createElement("label");
      main.className = "checkbox backup-tree-leaf-main";
      main.append(input, icon, label);
      row.append(main);
      if (options.meta !== undefined) {
        const meta = document.createElement("span");
        meta.className = "backup-tree-meta";
        meta.textContent = options.meta;
        meta.title = options.meta;
        row.append(meta);
      }
      if (options.extra) {
        extraWrap = document.createElement("div");
        extraWrap.className = "backup-tree-extra";
        extraWrap.append(options.extra);
        row.append(extraWrap);
      }
    } else {
      row = document.createElement("label");
      row.className = "checkbox backup-tree-leaf";
      row.append(input, icon, label);
    }

    const item: ItemState = { name: options.name, input, row, extra: extraWrap, onChange: options.onChange };
    input.addEventListener("change", () => {
      this.refresh(state);
      options.onChange?.();
    });
    state.items.push(item);
    state.children.insertBefore(row, state.empty);
    this.refresh(state);
    return item;
  }

  /** 按关键字过滤：隐藏不匹配的叶子；无可见叶子的分组整组隐藏。 */
  filter(keyword: string): void {
    const kw = keyword.trim().toLowerCase();
    for (const state of this.groups) {
      let visible = 0;
      for (const item of state.items) {
        const show = !kw || item.name.toLowerCase().includes(kw);
        item.row.hidden = !show;
        if (show) visible++;
      }
      state.el.hidden = state.items.length > 0 && visible === 0;
    }
  }

  private createGroup(options: ObjectTreeGroupOptions): GroupState {
    const el = document.createElement("div");
    el.className = "backup-tree-group";
    el.setAttribute("role", "group");
    el.setAttribute("aria-label", options.title);

    const root = document.createElement("div");
    root.className = "backup-tree-row is-root";

    const expand = document.createElement("button");
    expand.type = "button";
    expand.className = "backup-tree-toggle";
    expand.style.cssText =
      "display:flex;align-items:center;gap:8px;flex:1;min-width:0;text-align:left;padding:0;color:inherit;font-weight:inherit;";
    expand.setAttribute("aria-expanded", "true");
    expand.setAttribute("aria-label", `折叠${options.title}`);

    const twisty = document.createElement("span");
    twisty.className = "backup-tree-twisty";
    twisty.innerHTML = TWISTY_SVG;
    const icon = document.createElement("span");
    icon.className = "backup-tree-icon";
    icon.innerHTML = iconSvgByName(options.icon);
    const name = document.createElement("span");
    name.className = "backup-tree-label";
    name.textContent = options.title;

    const count = document.createElement("span");
    count.className = "backup-tree-count";

    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "backup-tree-toggle";
    toggle.hidden = true;

    expand.append(twisty, icon, name);
    root.append(expand, count, toggle);

    const children = document.createElement("div");
    children.className = "backup-tree-children";
    children.id = `backup-group-${++treeSequence}`;
    expand.setAttribute("aria-controls", children.id);

    const empty = document.createElement("div");
    empty.className = "backup-tree-empty";
    empty.textContent = options.emptyText ?? "（无）";
    children.appendChild(empty);

    el.append(root, children);

    const state: GroupState = {
      title: options.title,
      el,
      children,
      empty,
      count,
      toggle,
      expand,
      collapsed: false,
      items: [],
      setCollapsed(next: boolean) {
        state.collapsed = next;
        children.hidden = next;
        expand.setAttribute("aria-expanded", String(!next));
        expand.setAttribute("aria-label", `${next ? "展开" : "折叠"}${options.title}`);
        root.classList.toggle("is-collapsed", next);
      },
    };

    expand.addEventListener("click", () => state.setCollapsed(!state.collapsed));
    toggle.addEventListener("click", (event) => {
      event.stopPropagation();
      if (!state.items.length) return;
      const allChecked = state.items.every((item) => item.input.checked);
      for (const item of state.items) item.input.checked = !allChecked;
      this.refresh(state);
      for (const item of state.items) item.onChange?.();
    });
    return state;
  }

  private refresh(state: GroupState): void {
    const total = state.items.length;
    const checked = state.items.filter((item) => item.input.checked).length;
    state.count.textContent = total ? `${checked} / ${total}` : "0";
    state.empty.hidden = total > 0;
    if (!total) {
      state.toggle.hidden = true;
      return;
    }
    state.toggle.hidden = false;
    state.toggle.textContent = state.items.every((item) => item.input.checked) ? "全不选" : "全选";
  }
}
