/** 菜单栏组件：下拉菜单、加速键、子菜单、右键菜单。 */

import { MENU_BAR, type MenuBarEntry, type MenuEntry } from "./menu";
import { iconSvgByName, svg, ICON_CHECK } from "./icons";

/** 去掉加速键标记，得到纯文案。 */
export function stripAccel(label: string): string {
  return label.replace(/&/g, "");
}

/** 菜单渲染器：由 App 持有实例，命令通过回调分发。 */
export class MenuRenderer {
  private host: HTMLElement;
  private openIndex = -1;
  private current: MenuEntry[] = [];
  private barEntries: MenuBarEntry[] = [...MENU_BAR];
  private onCommand: (id: string) => void;
  private isOpen = false;
  /** 菜单栏末尾的纯文字项（如「选项」）：点击直接执行命令，不展开下拉。 */
  private tailActions: { label: string; command: string }[] = [];

  constructor(host: HTMLElement, onCommand: (id: string) => void, private isDisabled: (id: string) => boolean = () => false) {
    this.host = host;
    this.onCommand = onCommand;
    document.addEventListener("mousedown", this.onDocDown, true);
    document.addEventListener("keydown", this.onKeyDown, true);
    window.addEventListener("blur", this.closeAll);
  }

  /** 追加「文字项」（渲染在菜单栏最右端）。 */
  setTailActions(actions: { label: string; command: string }[]): void {
    this.tailActions = actions;
  }

  dispose(): void {
    document.removeEventListener("mousedown", this.onDocDown, true);
    document.removeEventListener("keydown", this.onKeyDown, true);
    window.removeEventListener("blur", this.closeAll);
  }

  /** 渲染菜单栏；extra 中的上下文菜单插入到「查看」之后（参考程序行为）。
   *  viewMenu 可选：以动态构建的「查看」菜单替换静态定义（勾选态随实际状态同步）。 */
  renderBar(bar: HTMLElement, extra: MenuBarEntry[] = [], viewMenu?: MenuBarEntry): void {
    const base = viewMenu
      ? [MENU_BAR[0], MENU_BAR[1], viewMenu, ...MENU_BAR.slice(3)]
      : MENU_BAR;
    this.barEntries = [
      ...base.slice(0, 3),
      ...extra,
      ...base.slice(3),
    ];
    bar.innerHTML = "";
    this.barEntries.forEach((entry, i) => {
      const item = document.createElement("div");
      item.className = "menubar-item";
      item.textContent = stripAccel(entry.label);
      item.dataset.index = String(i);
      item.addEventListener("mousedown", (e) => {
        e.preventDefault();
        e.stopPropagation();
        if (this.openIndex === i) {
          this.closeAll();
        } else {
          this.openTop(i);
        }
      });
      item.addEventListener("mouseenter", () => {
        if (this.isOpen && this.openIndex !== i) {
          this.openTop(i);
        }
      });
      bar.appendChild(item);
    });

    // 末尾文字项（选项 …）：与菜单项同款外观，点击直接执行，不弹下拉
    if (this.tailActions.length) {
      const tail = document.createElement("div");
      tail.className = "menubar-tail";
      for (const action of this.tailActions) {
        const item = document.createElement("div");
        item.className = "menubar-item";
        item.textContent = action.label;
        item.setAttribute("role", "menuitem");
        item.dataset.command = action.command;
        item.addEventListener("mousedown", (e) => {
          e.preventDefault();
          e.stopPropagation();
          this.closeAll();
          this.onCommand(action.command);
        });
        tail.appendChild(item);
      }
      bar.appendChild(tail);
    }
  }

  /** 打开指定顶层菜单。 */
  private openTop(index: number): void {
    this.closeAll();
    this.isOpen = true;
    this.openIndex = index;
    const entry = this.barEntries[index];
    const anchor = this.host.querySelector<HTMLElement>(
      `.menubar-item[data-index="${index}"]`,
    );
    if (!anchor) return;
    anchor.classList.add("open");
    this.current = entry.children;
    this.popup(anchor, 0);
  }

  /** 在锚点处弹出菜单。 */
  private popup(anchor: HTMLElement, depth: number): void {
    const menu = document.createElement("div");
    menu.className = "menu-popup";
    menu.dataset.depth = String(depth);

    for (const entry of this.current) {
      menu.appendChild(this.buildRow(entry, depth));
    }

    document.body.appendChild(menu);
    const rect = anchor.getBoundingClientRect();
    this.positionPopup(menu, rect.left, rect.bottom);
  }

  /** 构造一行菜单项（含图标 / 快捷键 / 子菜单箭头）。 */
  private buildRow(entry: MenuEntry, depth: number): HTMLElement {
    entry = { ...entry, disabled: !!entry.disabled || !!(entry.id && this.isDisabled(entry.id)) };
    if (entry.separator) {
      const sep = document.createElement("div");
      sep.className = "menu-sep";
      return sep;
    }
    const row = document.createElement("div");
    row.className = "menu-entry";
    if (entry.disabled) row.classList.add("disabled");

    const iconCol = document.createElement("div");
    iconCol.className = "menu-entry-icon";
    if (entry.icon) {
      iconCol.innerHTML = iconSvgByName(entry.icon);
    } else if (entry.checked !== undefined) {
      const mark = document.createElement("div");
      mark.className = "menu-entry-check";
      mark.innerHTML = entry.checked ? svg(ICON_CHECK) : "";
      iconCol.appendChild(mark);
    }

    const label = document.createElement("div");
    label.className = "menu-entry-label";
    label.textContent = stripAccel(entry.label ?? "");

    row.appendChild(iconCol);
    row.appendChild(label);

    if (entry.shortcut) {
      const sc = document.createElement("div");
      sc.className = "menu-entry-shortcut";
      sc.textContent = entry.shortcut;
      row.appendChild(sc);
    }
    if (entry.children && entry.children.length) {
      const arrow = document.createElement("div");
      arrow.className = "menu-entry-subarrow";
      arrow.innerHTML = '<svg viewBox="0 0 6 8" width="6" height="8"><path d="M0.5 0.5 L5.5 4 L0.5 7.5 z" fill="currentColor"/></svg>';
      row.appendChild(arrow);
      row.addEventListener("mouseenter", () => {
        this.closeDeeperThan(depth);
        const saved = this.current;
        this.current = entry.children!;
        const rect = row.getBoundingClientRect();
        this.popupAt(rect.right - 3, rect.top - 3, depth + 1);
        this.current = saved;
      });
    } else {
      row.addEventListener("mouseenter", () => this.closeDeeperThan(depth));
    }

    row.setAttribute("role", entry.checked === undefined ? "menuitem" : "menuitemcheckbox");
    row.setAttribute("aria-label", stripAccel(entry.label ?? ""));
    row.setAttribute("aria-disabled", String(!!entry.disabled));
    if (entry.checked !== undefined) row.setAttribute("aria-checked", String(entry.checked));
    row.tabIndex = -1;
    row.addEventListener("mouseup", () => {
      if (entry.disabled || entry.children?.length) return;
      this.closeAll();
      if (entry.id) this.onCommand(entry.id);
    });
    return row;
  }

  /** 在指定坐标弹出（右键菜单 / 子菜单）。 */
  private popupAt(x: number, y: number, depth: number): void {
    const menu = document.createElement("div");
    menu.className = "menu-popup";
    menu.dataset.depth = String(depth);
    for (const entry of this.current) {
      menu.appendChild(this.buildRow(entry, depth));
    }
    document.body.appendChild(menu);
    this.positionPopup(menu, x, y);
  }

  /** 定位弹出菜单，超出视口时翻转。 */
  private positionPopup(menu: HTMLElement, x: number, y: number): void {
    const rect = menu.getBoundingClientRect();
    let left = x;
    let top = y;
    if (left + rect.width > window.innerWidth - 2) {
      left = Math.max(2, window.innerWidth - rect.width - 2);
    }
    if (top + rect.height > window.innerHeight - 2) {
      top = Math.max(2, y - rect.height);
    }
    menu.style.left = `${left}px`;
    menu.style.top = `${top}px`;
  }

  /** 关闭比 depth 更深的弹出菜单。 */
  private closeDeeperThan(depth: number): void {
    document.querySelectorAll<HTMLElement>(".menu-popup").forEach((m) => {
      const d = Number(m.dataset.depth ?? "0");
      if (d > depth) m.remove();
    });
  }

  /** 关闭全部菜单。 */
  closeAll = (): void => {
    document.querySelectorAll(".menu-popup").forEach((m) => m.remove());
    this.host
      .querySelectorAll(".menubar-item.open")
      .forEach((m) => m.classList.remove("open"));
    this.isOpen = false;
    this.openIndex = -1;
  };

  private onDocDown = (e: MouseEvent): void => {
    const target = e.target as HTMLElement;
    if (target.closest(".menu-popup") || target.closest(".menubar-item")) return;
    this.closeAll();
  };

  private onKeyDown = (e: KeyboardEvent): void => {
    if (e.key === "Escape") { this.closeAll(); return; }
    if (!this.isOpen) return;
    const menus = [...document.querySelectorAll<HTMLElement>(".menu-popup")];
    const menu = menus[menus.length - 1];
    if (!menu) return;
    const rows = [...menu.querySelectorAll<HTMLElement>(".menu-entry:not(.disabled)")];
    const index = rows.indexOf(document.activeElement as HTMLElement);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      rows[(index + (e.key === "ArrowDown" ? 1 : -1) + rows.length) % rows.length]?.focus();
    }
    if (e.key === "Enter" || e.key === " ") {
      const row = rows[index];
      if (!row) return;
      e.preventDefault();
      if (row.querySelector(".menu-entry-subarrow")) row.dispatchEvent(new MouseEvent("mouseenter"));
      else row.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    }
  };

  /** 弹出任意菜单（右键菜单 / 工具栏下拉）。 */
  showContext(entries: MenuEntry[], x: number, y: number): void {
    this.closeAll();
    this.isOpen = true;
    this.current = entries;
    this.popupAt(x, y, 0);
  }
}

/** 菜单栏的 HTML 骨架。 */
export function createMenuBar(): HTMLElement {
  const el = document.createElement("div");
  el.className = "menubar";
  return el;
}
