/** 内嵌字段侧栏；键由调用方提供，查询结果必须使用 column.id。 */
export interface SidebarColumn { id: string; name: string; comment?: string | null }
export class ColumnSidebar {
  readonly el = document.createElement("aside");
  private readonly search = document.createElement("input");
  private readonly list = document.createElement("div");
  private trigger: HTMLElement | null = null;
  private renderedKey = "";
  constructor(readonly id: string, private readonly columns: () => SidebarColumn[], private readonly hidden: () => Set<string>, private readonly change: (ids: string[]) => void) {
    this.el.id = id; this.el.className = "column-sidebar"; this.el.hidden = true;
    this.el.setAttribute("aria-label", "选择显示字段");
    const heading = document.createElement("div"); heading.className = "column-sidebar-heading"; heading.textContent = "显示字段";
    // 侧栏由工具栏图标开关，不再提供内部关闭按钮。
    this.search.type = "search"; this.search.placeholder = "搜索字段"; this.search.setAttribute("aria-label", "搜索字段"); this.search.addEventListener("input", () => this.render());
    const restore = document.createElement("button"); restore.type = "button"; restore.textContent = "全选 / 恢复全部字段"; restore.addEventListener("click", () => { this.change([]); this.render(); });
    this.list.className = "column-sidebar-list qr-columns";
    this.el.append(heading, this.search, restore, this.list);
    this.el.addEventListener("keydown", e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); this.close(); } });
  }
  bind(trigger: HTMLElement): void { this.trigger = trigger; trigger.setAttribute("aria-controls", this.id); trigger.setAttribute("aria-expanded", String(!this.el.hidden)); }
  toggle(trigger?: HTMLElement): void { if (trigger) this.bind(trigger); this.el.hidden = !this.el.hidden; this.trigger?.setAttribute("aria-expanded", String(!this.el.hidden)); if (!this.el.hidden) { this.render(); this.search.focus({ preventScroll: true }); } }
  close(): void { this.el.hidden = true; this.trigger?.setAttribute("aria-expanded", "false"); this.trigger?.focus({ preventScroll: true }); }
  render(): void {
    const columns = this.columns(), hidden = this.hidden(), visible = columns.filter(c => !hidden.has(c.id)).length;
    const query = this.search.value.toLocaleLowerCase();
    const matches = columns.filter(c => c.name.toLocaleLowerCase().includes(query)), key = JSON.stringify(matches);
    // 勾选只更新状态，保留原复选框和键盘焦点；搜索/列元数据变化才重建列表。
    if (this.renderedKey === key) {
      this.list.querySelectorAll<HTMLInputElement>('input[type="checkbox"]').forEach(input => { input.checked = !hidden.has(input.dataset.columnId!); input.disabled = input.checked && visible <= 1; });
      return;
    }
    this.renderedKey = key; this.list.replaceChildren();
    for (const column of matches) {
      const row = document.createElement("label"); row.className = "column-sidebar-row"; row.title = column.name;
      const input = document.createElement("input"); input.type = "checkbox"; input.checked = !hidden.has(column.id); input.disabled = input.checked && visible <= 1;
      input.dataset.columnId = column.id;
      input.addEventListener("change", () => {
        const next = this.hidden(); if (input.checked) next.delete(column.id); else next.add(column.id);
        try { this.change([...next]); } finally { input.checked = !this.hidden().has(column.id); this.render(); }
      });
      const name = document.createElement("span"); name.className = "column-sidebar-name"; name.textContent = column.name;
      const text = document.createElement("div"); text.className = "column-sidebar-text"; text.append(name);
      input.setAttribute("aria-label", column.name);
      const comment = column.comment?.trim();
      if (comment) {
        const note = document.createElement("small"); note.className = "column-sidebar-comment"; note.textContent = comment;
        note.id = `${this.id}-comment-${columns.indexOf(column)}`;
        input.setAttribute("aria-describedby", note.id); text.append(note);
        row.title = `${column.name}\n${comment}`;
      }
      row.append(input, text); this.list.append(row);
    }
    if (!this.list.children.length) { const empty = document.createElement("p"); empty.textContent = "没有匹配字段"; this.list.append(empty); }
  }
}
