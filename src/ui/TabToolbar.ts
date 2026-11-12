/**
 * 标签页二级工具栏行（高 38px）。
 *
 * 参考程序在标签页下方还有一行小图标按钮（随标签页类型变化）：
 *   对象页    打开表 / 设计表 / 新建表 / 删除表 / 导入向导 / 导出向导
 *   表数据页  文本 / 筛选 / 排序 / 列 / 导入 / 导出 / 数据生成 / 创建图表
 *   查询页    开始事务 / 保存 / 查询创建工具 / 美化 SQL / 代码段 / 文本 / 导出结果 / 创建图表
 *   表设计器  保存 / 添加字段 / 插入字段 / 删除字段 / 主键 / 上移 / 下移
 */

import { iconSvgByName, dropArrow } from "./icons";

export interface TabToolItem {
  id: string;
  label: string;
  icon?: string;
  /** 使用现有主操作变体（例如保存）。 */
  primary?: boolean;
  disabled?: boolean;
  /** 当前真实能力或状态导致禁用的原因。 */
  disabledReason?: string;
  separator?: boolean;
  dropdown?: boolean;
  /** 分体下拉按钮：左侧主操作与右侧箭头分开点击（主操作走 onCommand，箭头走 onDropdown）。 */
  split?: boolean;
  /** 右对齐（工具栏末端，如搜索）。 */
  alignRight?: boolean;
  /** 只显示图标不显示文字（如「刷新」「设置」齿轮；aria-label/title 仍用 label）。 */
  iconOnly?: boolean;
  /** 当前处于激活状态（如「文本▼」已选中的显示格式），工具栏会高亮该按钮。 */
  active?: boolean;
}

export interface TabToolCallbacks {
  onCommand: (id: string) => void;
  /** 带下拉箭头的按钮被点击（App 在按钮处弹菜单）。 */
  onDropdown?: (id: string, x: number, y: number) => void;
}

export class TabToolbar {
  readonly el: HTMLElement;
  /** 持久插槽：按钮刷新不能移除正在输入（包括 IME 组合）的控件。 */
  readonly stableSlot: HTMLElement;
  private readonly itemsHost: HTMLElement;
  private cb: TabToolCallbacks;

  constructor(cb: TabToolCallbacks) {
    this.cb = cb;
    this.el = document.createElement("div");
    this.el.className = "tab-toolbar";
    this.itemsHost = document.createElement("div");
    this.itemsHost.style.display = "contents";
    this.stableSlot = document.createElement("div");
    this.stableSlot.className = "tab-tool-slot";
    this.stableSlot.style.display = "contents";
    this.el.append(this.itemsHost, this.stableSlot);
  }

  /** 设置按钮组。 */
  setItems(items: TabToolItem[]): void {
    this.itemsHost.replaceChildren();
    for (const item of items) {
      if (item.separator) {
        const sep = document.createElement("div");
        sep.className = "tab-tool-sep";
        this.itemsHost.appendChild(sep);
        continue;
      }
      if (item.alignRight) {
        const spacer = document.createElement("div");
        spacer.style.flex = "1";
        this.itemsHost.appendChild(spacer);
      }
      if (item.split) {
        // 分体下拉按钮：左侧主操作 + 右侧下拉箭头，各自独立点击区域。
        const group = document.createElement("div");
        group.className = "tab-tool-split";
        group.dataset.command = item.id;
        if (item.active) group.classList.add("active");
        if (item.disabled) group.classList.add("disabled");

        const main = document.createElement("button");
        main.type = "button";
        main.className = "tab-tool-btn tab-tool-main";
        main.disabled = !!item.disabled;
        main.setAttribute("aria-label", item.label);
        main.setAttribute("aria-pressed", item.active ? "true" : "false");
        main.title = item.disabledReason || item.label;
        if (item.disabled) main.classList.add("disabled");
        if (item.disabledReason) main.setAttribute("aria-description", item.disabledReason);
        if (item.icon) {
          const ic = document.createElement("span");
          ic.className = "tab-tool-icon";
          ic.innerHTML = iconSvgByName(item.icon);
          ic.style.lineHeight = "0";
          ic.style.display = "inline-flex";
          main.appendChild(ic);
        }
        if (!item.iconOnly) {
          const label = document.createElement("span");
          label.textContent = item.label;
          main.appendChild(label);
        }
        main.addEventListener("click", () => {
          if (item.disabled) return;
          this.cb.onCommand(item.id);
        });

        const caretBtn = document.createElement("button");
        caretBtn.type = "button";
        caretBtn.className = "tab-tool-btn tab-tool-caret";
        caretBtn.disabled = !!item.disabled;
        caretBtn.setAttribute("aria-label", `${item.label}：选择显示格式`);
        caretBtn.setAttribute("aria-haspopup", "menu");
        caretBtn.title = item.disabledReason || `${item.label}：选择显示格式`;
        if (item.disabled) caretBtn.classList.add("disabled");
        const arrow = document.createElement("span");
        arrow.className = "tab-tool-arrow";
        arrow.innerHTML = dropArrow(8, 5);
        arrow.style.lineHeight = "0";
        caretBtn.appendChild(arrow);
        caretBtn.addEventListener("click", () => {
          if (item.disabled || !this.cb.onDropdown) return;
          const rect = caretBtn.getBoundingClientRect();
          this.cb.onDropdown(item.id, rect.left, rect.bottom);
        });

        group.append(main, caretBtn);
        this.itemsHost.appendChild(group);
        continue;
      }
      const btn = document.createElement("button");
      btn.type = "button";
      btn.dataset.command = item.id;
      btn.className = "tab-tool-btn";
      if (item.primary) btn.classList.add("primary");
      if (item.active) { btn.classList.add("active"); btn.setAttribute("aria-pressed", "true"); }
      else if (item.id === "text") btn.setAttribute("aria-pressed", "false");
      btn.disabled = !!item.disabled;
      btn.setAttribute("aria-label", item.label);
      if (item.disabled) btn.classList.add("disabled");
      btn.title = item.disabledReason || item.label;
      if (item.disabledReason) btn.setAttribute("aria-description", item.disabledReason);
      if (item.icon) {
        const ic = document.createElement("span");
        ic.className = "tab-tool-icon";
        ic.innerHTML = iconSvgByName(item.icon);
        ic.style.lineHeight = "0";
        ic.style.display = "inline-flex";
        btn.appendChild(ic);
      }
      if (!item.iconOnly) {
        const label = document.createElement("span");
        label.textContent = item.label;
        btn.appendChild(label);
      }
      if (item.dropdown) {
        const arrow = document.createElement("span");
        arrow.className = "tab-tool-arrow";
        // currentColor 跟随按钮文字色（hover/激活/禁用联动）；尺寸由 .tab-tool-arrow svg 固定，避免被图标规则放大
        arrow.innerHTML = dropArrow(8, 5);
        arrow.style.lineHeight = "0";
        btn.appendChild(arrow);
      }
      btn.addEventListener("click", () => {
        if (item.disabled) return;
        if (item.dropdown && this.cb.onDropdown) {
          const rect = btn.getBoundingClientRect();
          this.cb.onDropdown(item.id, rect.left, rect.bottom);
          return;
        }
        this.cb.onCommand(item.id);
      });
      this.itemsHost.appendChild(btn);
    }
  }

  /** 清空。 */
  clear(): void {
    this.itemsHost.replaceChildren();
    this.stableSlot.replaceChildren();
  }
}
