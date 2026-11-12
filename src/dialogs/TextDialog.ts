import { Dialog } from "../ui/Dialog";
import { accessibleModal } from "./LeaveGuardDialog";

/** 只读文本证据：可选中复制，不写入文件、不执行数据库操作。 */
export function showDraftText(title: string, text: string, description: string): void {
  const previous = document.activeElement as HTMLElement | null;
  const dlg = new Dialog({ title, width: 720, closable: false, render: body => {
    const note = document.createElement("p"); note.textContent = description;
    const label = document.createElement("label"); label.textContent = "文本内容";
    const textarea = document.createElement("textarea"); textarea.className = "draft-text"; textarea.readOnly = true; textarea.value = text; textarea.setAttribute("aria-label", "文本内容");
    label.append(textarea); body.append(note, label);
  }, buttons: [{ label: "关闭" }] });
  accessibleModal(dlg, title, () => dlg.close(), previous);
}
