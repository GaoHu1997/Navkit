import { Dialog } from "../ui/Dialog";
import { api } from "../core/api";
import { IpcError } from "../core/execution";
import { ObjectMutationSession } from "../core/object-mutation";
import { objectLabel, type DatabaseObjectId } from "../core/object-catalog";

/** Explicit baseline → review → confirmation; unknown never reuses the same DROP. */
export function openObjectMutationDialog(object: DatabaseObjectId, ownerId: string, checkContext: () => void, committed: () => Promise<void>): Dialog {
  const previous = document.activeElement as HTMLElement | null;
  const status = document.createElement("p"); status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite");
  const details = document.createElement("pre"); details.style.whiteSpace = "pre-wrap";
  const confirm = document.createElement("input"); confirm.type = "checkbox"; confirm.id = `object-drop-${crypto.randomUUID()}`;
  const label = document.createElement("label"); label.htmlFor = confirm.id; label.append(confirm, " 我已核对完整对象与父表，确认删除触发器；不会删除父表或其它对象。");
  let dialog: Dialog;
  const session = new ObjectMutationSession(object, ownerId, { ...api, executeObjectMutation: (request, work) => { try { checkContext(); } catch (error) { throw new IpcError({ code: "OBJECT_MUTATION_NOT_REGISTERED", message: String(error), retryable: false }); } return api.executeObjectMutation(request, work); } }, () => render());
  function render(): void {
    if (!dialog) return;
    dialog.el.setAttribute("aria-busy", String(session.busy));
    const buttons = [...dialog.footer.querySelectorAll<HTMLButtonElement>("button")];
    buttons[0].disabled = session.busy || session.locked || !session.baseline;
    buttons[1].disabled = session.busy || session.locked || !session.preview || !confirm.checked;
    buttons[1].title = session.locked ? "结果需要核对；禁止重放" : !confirm.checked ? "请先核对预览并勾选确认" : "";
    buttons[2].disabled = !session.execution.busy || ["unknown", "succeeded", "failed"].includes(session.execution.state);
    buttons[3].disabled = session.busy;
    confirm.disabled = session.busy || session.locked || !session.preview;
  }
  dialog = new Dialog({ title: "受控删除触发器", width: 620, closable: false, render: body => {
    const context = document.createElement("p"); context.textContent = objectLabel(object);
    body.append(context, status, details, label);
  }, buttons: [
    { label: "读取删除预览", onClick: async () => {
      confirm.checked = false; status.textContent = "正在读取新预览；旧预览与确认已失效，尚未发送删除 SQL。";
      try { checkContext(); const p = await session.prepare(); checkContext(); confirm.checked = false; details.textContent = `对象：${objectLabel(p.object)}\n原结构版本：${p.base_revision}\n后端计划：${p.plan_id}\n审核摘要：${p.review_digest}\n同一物理连接事务内删除并核对父表/其它对象；预览不会执行 SQL。`; status.textContent = "预览已固定，请核对后确认。"; }
      catch (error) { status.textContent = `预览失败：${String(error)}；没有提交删除。原始基线保留。`; }
      render(); return false;
    } },
    { label: "确认删除", primary: true, onClick: async () => {
      try { checkContext(); await session.submit(confirm.checked); status.textContent = "删除及同事务读回已确认。"; await committed(); dialog.close(); }
      catch (error) { status.textContent = `${String(error)}${session.locked ? "；原对象证据已保留，禁止重放。关闭后请刷新真实对象目录核对。" : "；请重新读取预览后明确确认。"}`; }
      render(); return false;
    } },
    { label: "停止", onClick: async () => { try { await session.cancel(); status.textContent = "停止请求已发送，仍需等待执行及资源排空回执。"; } catch (error) { status.textContent = `停止回执缺失：${String(error)}，继续等待原执行。`; } render(); return false; } },
    { label: "关闭", onClick: () => !session.busy },
  ] });
  const close = dialog.close.bind(dialog);
  dialog.close = () => { if (!session.busy) close(); };
  dialog.el.setAttribute("role", "dialog"); dialog.el.setAttribute("aria-modal", "true"); dialog.el.setAttribute("aria-label", "受控删除触发器");
  confirm.addEventListener("change", render);
  dialog.el.addEventListener("keydown", event => {
    if (event.key === "Escape" && !session.busy) { event.preventDefault(); event.stopPropagation(); dialog.close(); }
    if (event.key !== "Tab") return;
    const nodes = [...dialog.el.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)")];
    if (event.shiftKey && document.activeElement === nodes[0]) { event.preventDefault(); nodes[nodes.length - 1]?.focus(); }
    else if (!event.shiftKey && document.activeElement === nodes[nodes.length - 1]) { event.preventDefault(); nodes[0]?.focus(); }
  });
  dialog.onClose(() => { session.dispose(); if (previous?.isConnected) previous.focus(); });
  status.textContent = "正在读取打开时的完整对象基线；尚未发送删除 SQL。";
  void session.load().then(() => { status.textContent = "基线已读取。请先读取固定删除预览。"; }).catch(error => { status.textContent = `基线读取失败：${String(error)}，未发送删除 SQL。`; }).finally(() => { render(); dialog.footer.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus(); });
  render(); return dialog;
}
