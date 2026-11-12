/**
 * 命令条（v2 · 替代 v1 彩色 Ribbon）。
 *
 * 设计规范（docs/design/DESIGN.md §5.1）：
 *  · 高 46px，按钮 = 彩色具象图标 20px + 标签 11px，竖排
 *  · 按钮最小宽 60px，间距 2px，悬停圆角 6px
 *  · 激活态：白卡 + 强调描边（--accent-200 inset）；禁用 = 文字降级
 *  · 分组用 1px 竖线（16px 高）；带下拉的按钮右下角 10px chevron-down
 *
 * 类名沿用 v1 的 .ribbon / .ribbon-btn / ...（验收脚本选择器依赖），
 * 视觉完全由 base.css 的 v2 规则接管。
 */

import { pictIcon } from "./pict";

export interface RibbonItem {
  id: string;
  /** 图标键（PICT_BY_KEY 表）。 */
  icon: string;
  /** 文字标签。 */
  label: string;
  /** 是否带下拉箭头。 */
  dropdown?: boolean;
  /** 是否禁用。 */
  disabled?: boolean;
  /** 分组结束（后面画一条竖分隔线）。 */
  groupEnd?: boolean;
}

export interface RibbonCallbacks {
  onCommand: (id: string) => void;
  isDisabled?: (id: string) => boolean;
  /** 带下拉箭头的按钮被点击（App 负责在按钮处弹出菜单）。 */
  onDropdown?: (id: string, x: number, y: number) => void;
}

/** 图标键 → 命令条具象图标（pict.ts 的 label）。 */
const PICT_BY_KEY: Record<string, string> = {
  ribbon_connect: "连接",
  ribbon_new_query: "新建查询",
  ribbon_table: "表",
  ribbon_view: "视图",
  ribbon_function: "函数",
  ribbon_user: "用户",
  ribbon_other: "其它",
  ribbon_query: "查询",
  ribbon_backup: "备份",
  ribbon_schedule: "自动任务",
  ribbon_chart: "图表",
  ribbon_diagram: "关系图",
};

/** 右下角下拉箭头（10px chevron-down，currentColor 跟随文字色）。 */
function caret(): string {
  return `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6.5 9.8 12 15.2l5.5-5.4"/></svg>`;
}

/** 主命令条的按钮定义（查询/备份置于函数后，暂隐藏其它/图表）。 */
export const RIBBON_MAIN: RibbonItem[] = [
  { id: "new-connection", icon: "ribbon_connect", label: "连接" },
  { id: "new-query", icon: "ribbon_new_query", label: "新建查询", groupEnd: true },
  { id: "open-table", icon: "ribbon_table", label: "表" },
  { id: "open-view", icon: "ribbon_view", label: "视图" },
  { id: "open-function", icon: "ribbon_function", label: "函数" },
  { id: "open-query", icon: "ribbon_query", label: "查询" },
  { id: "backup", icon: "ribbon_backup", label: "备份", groupEnd: true },
  { id: "open-user", icon: "ribbon_user", label: "用户" },
  { id: "schedule", icon: "ribbon_schedule", label: "自动任务" },
  { id: "er-diagram", icon: "ribbon_diagram", label: "关系图", groupEnd: true },
];

export class Ribbon {
  readonly el: HTMLElement;
  private items: RibbonItem[];
  private cb: RibbonCallbacks;
  private buttons = new Map<string, HTMLElement>();

  constructor(items: RibbonItem[], cb: RibbonCallbacks) {
    this.items = items;
    this.cb = cb;
    this.el = document.createElement("div");
    this.el.className = "ribbon";
    this.render();
  }

  /** 切换整组按钮。 */
  setItems(items: RibbonItem[]): void {
    this.items = items;
    this.render();
  }

  render(): void {
    this.el.innerHTML = "";
    this.buttons.clear();
    for (const item of this.items) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "ribbon-btn";
      btn.title = item.label;
      btn.setAttribute("aria-label", item.label);

      const icon = document.createElement("div");
      icon.className = "ribbon-icon";
      const pict = PICT_BY_KEY[item.icon];
      if (pict) {
        icon.innerHTML = pictIcon(pict, 20);
      }
      btn.appendChild(icon);

      const label = document.createElement("div");
      label.className = "ribbon-label";
      label.textContent = item.label;
      btn.appendChild(label);

      if (item.dropdown) {
        const arrow = document.createElement("div");
        arrow.className = "ribbon-drop";
        arrow.innerHTML = caret();
        btn.appendChild(arrow);
      }

      btn.addEventListener("click", () => {
        if (this.isDisabled(item.id)) return;
        if (item.dropdown && this.cb.onDropdown) {
          const rect = btn.getBoundingClientRect();
          this.cb.onDropdown(item.id, rect.left, rect.bottom);
          return;
        }
        this.cb.onCommand(item.id);
      });

      this.el.appendChild(btn);
      this.buttons.set(item.id, btn);

      if (item.groupEnd) {
        const sep = document.createElement("div");
        sep.className = "ribbon-sep";
        this.el.appendChild(sep);
      }
    }
    this.updateDisabled();
  }

  /** 更新禁用状态。 */
  updateDisabled(): void {
    for (const [id, btn] of this.buttons) {
      btn.classList.toggle("disabled", this.isDisabled(id));
    }
  }

  /** 高亮某个按钮（表示当前激活的命令条分组）。 */
  setActive(id: string | null): void {
    for (const [key, btn] of this.buttons) {
      btn.classList.toggle("active", key === id);
    }
  }

  private isDisabled(id: string): boolean {
    return this.cb.isDisabled?.(id) ?? false;
  }
}
