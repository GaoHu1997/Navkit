/** 标签页条（高 32px）：活动标签白底，其余 #F0F0F0。 */

import type { TabState } from "../core/types";
import { ICON_CROSS, svg, iconSvgByName } from "./icons";

export interface TabCallbacks {
  onSelect?: (tab: TabState) => void;
  onClose?: (tab: TabState) => void;
  onContextMenu?: (tab: TabState, x: number, y: number) => void;
}

export class TabStrip {
  readonly el: HTMLElement;
  private cb: TabCallbacks;
  private tabs: TabState[] = [];
  private activeId: string | null = null;

  constructor(cb: TabCallbacks = {}) {
    this.cb = cb;
    this.el = document.createElement("div");
    this.el.className = "tabstrip";
  }

  setTabs(tabs: TabState[], activeId: string | null): void {
    this.tabs = tabs;
    this.activeId = activeId;
    this.render();
  }

  render(): void {
    this.el.innerHTML = "";
    if (!this.tabs.length) {
      const empty = document.createElement("div");
      empty.className = "tabstrip-empty";
      empty.textContent = "没有打开的标签页";
      this.el.appendChild(empty);
      return;
    }
    for (const tab of this.tabs) {
      this.el.appendChild(this.buildTab(tab));
    }
    // 标签溢出时把激活标签滚入可视区（block:nearest 避免牵动纵向布局）。
    this.el.querySelector<HTMLElement>(".tab.active")
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }

  private buildTab(tab: TabState): HTMLElement {
    const el = document.createElement("div");
    el.className = "tab";
    if (tab.id === this.activeId) el.classList.add("active");
    if (tab.dirty) el.classList.add("dirty");
    el.dataset.id = tab.id;

    const icon = document.createElement("div");
    icon.className = "tab-icon";
    icon.innerHTML = iconSvgByName(this.iconFor(tab));

    const title = document.createElement("div");
    title.className = "tab-title";
    title.textContent = tab.title;

    const close = document.createElement("div");
    close.className = "tab-close";
    close.innerHTML = svg(ICON_CROSS);
    close.title = "关闭";

    el.appendChild(icon);
    el.appendChild(title);
    el.appendChild(close);

    el.addEventListener("mousedown", (e) => {
      if (e.button === 2) return;
      if ((e.target as HTMLElement).classList.contains("tab-close")) return;
      this.cb.onSelect?.(tab);
    });
    close.addEventListener("mousedown", (e) => {
      e.stopPropagation();
      this.cb.onClose?.(tab);
    });
    el.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      e.stopPropagation();
      this.cb.onContextMenu?.(tab, e.clientX, e.clientY);
    });
    return el;
  }

  private iconFor(tab: TabState): string {
    switch (tab.kind) {
      case "query":
        return "editor";
      case "structure":
        return "columns";
      case "result":
        return "info_preview";
      case "datasync":
        return "data_sync_ent_preview_image";
      case "backup":
        return "dbn_backup";
      case "schedule":
        return "info_pane_query";
      case "model":
      case "chart":
        return "data_profiling_ent_preview_image";
      default:
        return "info_pane_table";
    }
  }
}
