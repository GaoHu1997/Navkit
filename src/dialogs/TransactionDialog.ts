/** 手动事务的显式确认，复用工具对话框风格与键盘/焦点管理。 */
import { Dialog } from "../ui/Dialog";
import { accessibleToolDialog } from "./toolDialog";

let sequence = 0;
export function confirmTransactionAction(title: string, message: string, action: string): Promise<boolean> {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  return new Promise(resolve => {
    let answered = false;
    const answer = (value: boolean) => { if (!answered) { answered = true; resolve(value); } };
    const description = `manual-transaction-description-${++sequence}`;
    const dialog = new Dialog({
      title, width: 460,
      render: body => {
        const text = document.createElement("p");
        text.id = description; text.textContent = message;
        text.style.cssText = "margin:0;padding:12px 2px;line-height:1.6;white-space:pre-wrap;";
        body.appendChild(text);
      },
      buttons: [
        { label: "取消", onClick: () => answer(false) },
        { label: action, primary: true, onClick: () => answer(true) },
      ],
    });
    dialog.el.setAttribute("aria-describedby", description);
    accessibleToolDialog(dialog, title, () => answer(false), previous);
    // 进入写库确认时默认聚焦取消，而非自动接受提交/回滚。
    queueMicrotask(() => dialog.footer.querySelector<HTMLButtonElement>("button")?.focus());
  });
}
