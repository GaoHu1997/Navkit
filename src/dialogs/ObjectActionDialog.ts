import { Dialog } from "../ui/Dialog";

/** 对象操作保留输入、关联错误与重试入口，复用现有 Dialog。 */
export function openObjectAction(options: {
  title: string; context: string; label?: string; value?: string; confirm: string;
  submit: (value: string) => Promise<void>;
}): Dialog {
  const previous = document.activeElement as HTMLElement | null;
  const input = options.label ? document.createElement("input") : null;
  const error = document.createElement("p");
  error.className = "object-action-error";
  error.id = `object-error-${crypto.randomUUID()}`;
  error.setAttribute("aria-live", "assertive");
  if (input) {
    input.type = "text"; input.value = options.value ?? "";
    input.id = `object-name-${crypto.randomUUID()}`;
    input.setAttribute("aria-describedby", error.id);
  }
  let busy = false;
  const dialog = new Dialog({
    title: options.title, width: 460,
    render: body => {
      const context = document.createElement("p"); context.textContent = options.context;
      body.append(context);
      if (input) {
        const label = document.createElement("label"); label.htmlFor = input.id; label.textContent = options.label!;
        label.className = "object-action-field"; label.append(input); body.append(label);
      }
      body.append(error);
    },
    buttons: [{ label: options.confirm, primary: true, onClick: async dlg => {
      if (busy) return false;
      error.textContent = ""; input?.removeAttribute("aria-invalid");
      busy = true; dlg.el.setAttribute("aria-busy", "true");
      const buttons = [...dlg.footer.querySelectorAll<HTMLButtonElement>("button")];
      buttons.forEach(b => { b.disabled = true; });
      try {
        if (input && !input.value.trim()) throw new Error("请输入名称");
        await options.submit(input?.value.trim() ?? "");
      } catch (e) {
        error.textContent = `操作失败：${String(e)}。当前内容已保留，可以重试。`;
        input?.setAttribute("aria-invalid", "true"); input?.focus();
        return false;
      } finally {
        busy = false; dlg.el.setAttribute("aria-busy", "false");
        buttons.forEach(b => { b.disabled = false; });
      }
    } }, { label: "取消" }],
  });
  dialog.el.setAttribute("role", "dialog"); dialog.el.setAttribute("aria-modal", "true"); dialog.el.setAttribute("aria-label", options.title);
  const close = dialog.close.bind(dialog);
  dialog.close = () => { if (!busy) close(); };
  dialog.el.addEventListener("keydown", e => {
    if (e.key === "Escape" && !busy) { e.preventDefault(); e.stopPropagation(); dialog.close(); }
    if (e.key !== "Tab") return;
    const nodes = [...dialog.el.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex='0']")];
    const first = nodes[0], last = nodes[nodes.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
  });
  (input ?? dialog.footer.querySelector<HTMLElement>("button"))?.focus();
  dialog.onClose(() => { if (previous?.isConnected) previous.focus(); });
  return dialog;
}
