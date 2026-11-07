/** 工具对话框沿用 Dialog 外观，补充局部键盘/焦点行为，不修改共享 Dialog。 */
import type { Dialog } from "../ui/Dialog";

export function accessibleToolDialog(dlg: Dialog, title: string, onClose: () => void, returnFocus: HTMLElement | null = null): void {
  const previous = returnFocus;
  dlg.el.setAttribute("role", "dialog");
  dlg.el.setAttribute("aria-modal", "true");
  dlg.el.setAttribute("aria-label", title);
  dlg.el.style.maxWidth = "calc(100vw - 32px)";
  dlg.el.style.maxHeight = "calc(100vh - 32px)";
  dlg.body.style.overflow = "auto";
  const close = dlg.el.querySelector<HTMLElement>(".dialog-title-close");
  if (close) {
    close.setAttribute("role", "button"); close.setAttribute("aria-label", "关闭"); close.tabIndex = 0;
    close.addEventListener("keydown", e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); dlg.close(); } });
  }
  dlg.el.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); dlg.close(); return; }
    if (e.key !== "Tab") return;
    const focusable = [...dlg.el.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex='0']")].filter(el => el.getClientRects().length);
    const first = focusable[0], last = focusable[focusable.length - 1];
    if (!first) { e.preventDefault(); return; }
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  });
  dlg.onClose(() => { onClose(); if (previous?.isConnected) previous.focus(); });
  queueMicrotask(() => dlg.body.querySelector<HTMLElement>("button:not(:disabled), input:not(:disabled), select:not(:disabled)")?.focus());
}

export function toolButton(label: string, primary = false): HTMLButtonElement {
  const button = document.createElement("button"); button.type = "button";
  button.className = primary ? "btn primary" : "btn"; button.textContent = label;
  return button;
}
export function toolTable(headers: string[], rows: (string | null)[][]): HTMLTableElement {
  const table = document.createElement("table"); table.style.cssText = "width:100%;border-collapse:collapse;font-size:12px;";
  const head = table.createTHead().insertRow();
  for (const name of headers) {
    const cell = document.createElement("th"); cell.scope = "col"; cell.textContent = name;
    cell.style.cssText = "text-align:left;padding:5px 8px;border-bottom:1px solid var(--border);background:var(--bg-panel);position:sticky;top:0;";
    head.appendChild(cell);
  }
  const body = table.createTBody();
  for (const row of rows) {
    const tr = body.insertRow();
    for (const value of row) {
      const cell = tr.insertCell(); cell.textContent = value === null ? "NULL" : value;
      cell.style.cssText = "padding:4px 8px;border-bottom:1px solid var(--border);white-space:pre-wrap;overflow-wrap:anywhere;max-width:300px;";
    }
  }
  return table;
}
