import { Dialog } from "../ui/Dialog";
import type { DialogButton } from "../ui/Dialog";
import "../styles/drafts.css";

/** 模态对话框的焦点契约：Esc 取消、Tab 循环、关闭后归还焦点。 */
export function accessibleModal(dlg: Dialog, title: string, cancel: () => void, previous: HTMLElement | null = null): void {
  dlg.el.setAttribute("role", "dialog");
  dlg.el.setAttribute("aria-modal", "true");
  dlg.el.setAttribute("aria-label", title);
  dlg.el.classList.add("draft-dialog");
  dlg.el.addEventListener("keydown", e => {
    if (e.key === "Escape" && dlg.el.getAttribute("aria-busy") !== "true") { e.preventDefault(); cancel(); }
    if (e.key !== "Tab") return;
    const controls = Array.from(dlg.el.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex='0']"));
    const first = controls[0], last = controls[controls.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
  });
  dlg.onClose(() => { if (previous?.isConnected) previous.focus(); });
  dlg.footer.querySelector<HTMLButtonElement>("button:last-child")?.focus();
}

export interface LeavePromptItem {
  title: string;
  /** query：SQL 有未保存修改，可以保存；other：结构或结果行编辑，离开即丢弃。 */
  kind: "query" | "other";
}

export type LeaveChoice = "save" | "discard" | "cancel";

/** 关闭页面前的唯一提示：查询 SQL 可保存，其它编辑明确丢弃或取消。 */
export function chooseLeave(items: LeavePromptItem[], cause: string): Promise<LeaveChoice> {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  return new Promise(resolve => {
    let settled = false;
    const finish = (choice: LeaveChoice) => { if (!settled) { settled = true; resolve(choice); } };
    const queries = items.filter(item => item.kind === "query");
    const others = items.filter(item => item.kind === "other");
    const message = document.createElement("p");
    message.textContent = queries.length
      ? `${cause}前，以下查询的 SQL 尚未保存。保存会写入已保存查询；不保存会丢弃这些修改。`
      : `${cause}前，以下页面有未保存的编辑；离开后会丢弃。`;
    const list = document.createElement("ul");
    items.forEach(item => { const li = document.createElement("li"); li.textContent = item.title; list.append(li); });
    const note = document.createElement("p");
    note.className = "draft-note";
    note.textContent = others.length
      ? "保存只处理查询 SQL；页面上其它未保存编辑（结构或结果行草稿）仍会随页面关闭而丢弃。"
      : "";
    const buttons: DialogButton[] = [];
    if (queries.length) buttons.push({ label: "保存查询并离开", primary: true, onClick: () => finish("save") });
    buttons.push({ label: "不保存", onClick: () => finish("discard") }, { label: "取消", onClick: () => finish("cancel") });
    const dlg = new Dialog({
      title: "未保存的修改",
      width: 540,
      closable: false,
      render: body => {
        body.append(message, list);
        if (others.length && queries.length) body.append(note);
      },
      buttons,
    });
    dlg.onClose(() => finish("cancel"));
    accessibleModal(dlg, "未保存的修改", () => { finish("cancel"); dlg.close(); }, previous);
  });
}
