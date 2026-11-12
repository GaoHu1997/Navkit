import { Dialog } from "../ui/Dialog";
import type { TableDesignPreview } from "../core/api";

export function accessibleTableDialog(dialog: Dialog, label: string, canClose: () => boolean = () => true): void {
  const previous = document.activeElement;
  dialog.el.classList.add("table-plan-dialog");
  dialog.el.setAttribute("role", "dialog"); dialog.el.setAttribute("aria-modal", "true"); dialog.el.setAttribute("aria-label", label); dialog.el.tabIndex = -1;
  dialog.el.addEventListener("keydown", event => {
    if (event.key === "Escape" && canClose()) { event.preventDefault(); dialog.close(); return; }
    if (event.key !== "Tab") return;
    const focusable = [...dialog.el.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex="0"]')].filter(el => !el.hidden);
    if (!focusable.length) { event.preventDefault(); dialog.el.focus(); return; }
    const first = focusable[0], last = focusable[focusable.length - 1];
    if (event.shiftKey && (document.activeElement === first || document.activeElement === dialog.el)) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
  });
  dialog.onClose(() => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); });
  (dialog.el.querySelector<HTMLElement>('input:not(:disabled), button:not(:disabled), [tabindex="0"]') ?? dialog.el).focus();
}
export function appendTablePlan(body: HTMLElement, preview: TableDesignPreview): void {
  const sql = document.createElement("pre"); sql.className = "designer-sql"; sql.tabIndex = 0; sql.setAttribute("aria-label", "后端实际 SQL 计划");
  sql.textContent = preview.steps.map((step, i) => `-- ${i + 1}. ${step.label}\n${step.sql}`).join("\n\n") || "后端计划无待执行步骤。"; body.appendChild(sql);
  for (const limitation of preview.limitations) { const notice = document.createElement("p"); notice.textContent = limitation; body.appendChild(notice); }
}
export function reviewTablePlan(title: string, preview: TableDesignPreview, target: string, description: string): Promise<boolean> {
  return new Promise(resolve => {
    let confirmed = false;
    const dialog = new Dialog({ title, width: 760, closable: false,
      render: body => { const intro = document.createElement("p"); intro.textContent = `${target}。${description}`; body.appendChild(intro); appendTablePlan(body, preview); },
      buttons: [{ label: "取消" }, { label: preview.destructive ? "确认执行破坏性计划" : "确认执行", primary: true, onClick: () => { confirmed = true; } }] });
    dialog.onClose(() => resolve(confirmed)); accessibleTableDialog(dialog, title);
  });
}
