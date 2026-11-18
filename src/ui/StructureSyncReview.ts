/** 原生表单预览：逐项确认扫描/破坏风险，完整计划整体应用，不生成或运行 SQL。 */
import type { StructureDiffReport } from "../core/api";
import { requiredStructureConsents, structureChangeId, structureChangeNeedsConsent } from "../core/structure-review";

const kinds: Record<string, string> = {
  add_column: "新增列", alter_column: "修改列", drop_column: "删除列", primary_key: "调整主键",
  index: "新增索引", drop_index: "删除索引", table_option: "修改表属性", unsupported: "不支持的变更",
};
function text<K extends keyof HTMLElementTagNameMap>(tag: K, value: string, className = ""): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag); element.textContent = value; element.className = className; return element;
}
export class StructureSyncReview {
  readonly el = document.createElement("fieldset");
  private report: StructureDiffReport | null = null;
  private approved = new Set<string>();
  private eligible = false;
  private progress: HTMLParagraphElement | null = null;
  constructor(private readonly changed: () => void) {
    this.el.className = "sync-structure-review sync-table-only";
    this.el.hidden = true;
    // 确认不是同步配置修改；不触发 dialog 的 change/input 使预览失效。
    for (const event of ["change", "input"]) this.el.addEventListener(event, e => e.stopPropagation());
  }
  get ready(): boolean { return this.eligible && this.report !== null && requiredStructureConsents(this.report).every(id => this.approved.has(id)); }
  get approvedIds(): string[] { return [...this.approved]; }
  setLocked(locked: boolean): void { this.el.disabled = locked; }
  clear(): void { this.report = null; this.approved.clear(); this.eligible = false; this.progress = null; this.el.hidden = true; this.el.replaceChildren(); }
  render(report: StructureDiffReport): void {
    this.clear(); this.report = structuredClone(report); this.el.hidden = false;
    this.eligible = report.target_exists && report.alterable && report.unsupported.length === 0 && report.steps.length > 0
      && report.changes.every(c => ["safe", "scan_required", "destructive"].includes(c.risk));
    this.el.append(text("legend", "结构变更预览"));
    this.progress = text("p", "", "sync-structure-review-progress");
    this.progress.setAttribute("role", "status"); this.el.append(this.progress);
    const list = document.createElement("ol"); list.className = "sync-structure-review-list";
    for (const [index, change] of report.changes.entries()) {
      const item = document.createElement("li"); const head = document.createElement("div"); head.className = "sync-structure-review-head";
      const caption = `${kinds[change.kind] ?? change.kind} · ${change.object}`;
      const needsConsent = structureChangeNeedsConsent(change);
      if (needsConsent) {
        const label = document.createElement("label"); label.className = "sync-structure-review-consent";
        const input = document.createElement("input"); input.type = "checkbox"; input.disabled = !this.eligible;
        input.setAttribute("aria-label", `确认${caption}的${change.risk === "destructive" ? "破坏" : "扫描/转换"}风险`);
        const id = structureChangeId(change, index);
        input.addEventListener("change", () => { if (input.checked) this.approved.add(id); else this.approved.delete(id); this.updateProgress(); this.changed(); });
        label.append(input, text("span", caption)); head.append(label);
      } else { head.append(text("span", caption)); }
      const badge = text("span", !this.eligible ? "不可应用" : needsConsent ? change.risk === "destructive" ? "需确认 · 删除/重建风险" : "需确认 · 扫描/转换" : "可应用", "sync-structure-review-badge");
      badge.dataset.risk = !this.eligible ? "unsupported" : change.risk; head.append(badge); item.append(head);
      if (change.before !== null || change.after !== null) {
        const detail = document.createElement("dl"); detail.className = "sync-structure-review-values";
        for (const [name, value] of [["原结构", change.before], ["新结构", change.after]] as const) {
          detail.append(text("dt", name), text("dd", value ?? "无"));
        }
        item.append(detail);
      }
      list.append(item);
    }
    this.el.append(list);
    if (report.unsupported.length) { const blocked = document.createElement("ul"); blocked.className = "sync-structure-review-blocked"; for (const reason of report.unsupported) blocked.append(text("li", reason)); this.el.append(blocked); }
    if (report.warnings.length) { const warnings = document.createElement("ul"); warnings.className = "sync-structure-review-warnings"; for (const warning of report.warnings) warnings.append(text("li", warning)); this.el.append(warnings); }
    if (report.steps.length) {
      const sql = document.createElement("details"); sql.append(text("summary", `查看后端执行计划（${report.steps.length} 步）`));
      sql.append(text("pre", report.steps.map((step, index) => `-- ${index + 1}. ${step.label}\n${step.sql}`).join("\n\n"))); this.el.append(sql);
    }
    this.updateProgress();
  }
  private updateProgress(): void {
    if (!this.progress || !this.report) return;
    const count = requiredStructureConsents(this.report).length;
    const unchanged = this.report.target_exists && this.report.alterable && this.report.unsupported.length === 0 && this.report.steps.length === 0;
    this.progress.textContent = unchanged ? "结构一致或无需执行 DDL，不创建应用授权。" : !this.eligible ? "本计划不能应用。请先处理下列限制，再重新预览。"
      : count ? `风险确认 ${this.approved.size} / ${count}。确认全部风险项后，整体应用此冻结计划；修改映射或连接需重新预览。`
        : "当前计划可应用；执行前仍会校验双端版本并核对实际结果。";
  }
}
