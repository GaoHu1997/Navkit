/**
 * 标签页二级工具栏行（高 38px）。
 *
 * 参考程序在标签页下方还有一行小图标按钮（随标签页类型变化）：
 *   对象页    打开表 / 设计表 / 新建表 / 删除表 / 导入向导 / 导出向导
 *   表数据页  开始事务 / 文本 / 筛选 / 排序 / 列 / 导入 / 导出 / 数据生成 / 创建图表
 *   查询页    保存 / 查询创建工具 / 美化 SQL / 代码段 / 文本 / 导出结果 / 创建图表
 *   表设计器  保存 / 添加字段 / 插入字段 / 删除字段 / 主键 / 上移 / 下移
 */

import { iconSvgByName, dropArrow } from "./icons";

export interface TabToolItem {
  id: string;
  label: string;
  icon?: string;
  disabled?: boolean;
  /** 当前真实能力或状态导致禁用的原因。 */
  disabledReason?: string;
  separator?: boolean;
  dropdown?: boolean;
  /** 右对齐（工具栏末端，如搜索）。 */
  alignRight?: boolean;
}

export interface TabToolCallbacks {
  onCommand: (id: string) => void;
  /** 带下拉箭头的按钮被点击（App 在按钮处弹菜单）。 */
  onDropdown?: (id: string, x: number, y: number) => void;
}

export class TabToolbar {
  readonly el: HTMLElement;
  private cb: TabToolCallbacks;

  constructor(cb: TabToolCallbacks) {
    this.cb = cb;
    this.el = document.createElement("div");
    this.el.className = "tab-toolbar";
  }

  /** 设置按钮组。 */
  setItems(items: TabToolItem[]): void {
    this.el.innerHTML = "";
    for (const item of items) {
      if (item.separator) {
        const sep = document.createElement("div");
        sep.className = "tab-tool-sep";
        this.el.appendChild(sep);
        continue;
      }
      if (item.alignRight) {
        const spacer = document.createElement("div");
        spacer.style.flex = "1";
        this.el.appendChild(spacer);
      }
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "tab-tool-btn";
      btn.disabled = !!item.disabled;
      btn.setAttribute("aria-label", item.label);
      if (item.disabled) btn.classList.add("disabled");
      btn.title = item.disabledReason || item.label;
      if (item.disabledReason) btn.setAttribute("aria-description", item.disabledReason);
      if (item.icon) {
        const ic = document.createElement("span");
        ic.innerHTML = iconSvgByName(item.icon);
        ic.style.lineHeight = "0";
        ic.style.display = "inline-flex";
        btn.appendChild(ic);
      }
      const label = document.createElement("span");
      label.textContent = item.label;
      btn.appendChild(label);
      if (item.dropdown) {
        const arrow = document.createElement("span");
        arrow.innerHTML = dropArrow(8, 5, "#5a5a5a");
        arrow.style.marginLeft = "3px";
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
      this.el.appendChild(btn);
    }
  }

  /** 清空。 */
  clear(): void {
    this.el.innerHTML = "";
  }
}
