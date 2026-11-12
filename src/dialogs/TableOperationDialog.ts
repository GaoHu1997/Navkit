import { api, type TableOperationPreview, type TableOperationResponse, type TableOperationReceipt } from "../core/api";
import type { TableOperation } from "../core/types";
import { assertDesignSnapshot, verifiedOperationResponse } from "../core/table-design-draft";
import { ExecutionController } from "../core/execution";
import { Dialog } from "../ui/Dialog";
import { accessibleTableDialog, appendTablePlan } from "./TablePlanDialog";
import "../styles/table-designer.css";

export interface TableOperationDialogOptions {
  connectionId: string; schema: string; table: string; kind: TableOperation["kind"]; ownerId: string;
  writeReason(): string | null;
  applied(receipt: TableOperationReceipt, schema: string): Promise<void>;
  toast(kind: "error" | "info" | "success", text: string): void;
}
/** 一个对象一次消费；unknown/partial 保留计划与原始目标，绝不重发 execute。 */
export function openTableOperationDialog(options: TableOperationDialogOptions): void {
  const title = options.kind === "clear" ? "清空表数据" : options.kind === "copy_structure" ? "复制表结构" : "复制表结构和数据";
  let busy = false, locked = false, sent = false;
  let operation: TableOperation | null = null, preview: TableOperationPreview | null = null;
  let execution: ExecutionController | null = null;
  let preparedName = "", effectiveSchema = options.schema;
  const target = document.createElement("input"); target.type = "text"; target.value = `${options.table}_copy`; target.setAttribute("aria-label", "复制目标表名");
  const status = document.createElement("p"); status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite"); status.textContent = "先读取真实完整基线，再预览实际后端计划。";
  const plan = document.createElement("div");
  const invalidate = () => { operation = null; preview = null; plan.innerHTML = ""; status.textContent = "目标表名已更改，请重新预览。"; update(); };
  target.addEventListener("input", invalidate);
  const dialog = new Dialog({ title, width: 780, closable: false,
    render: body => {
      const identity = document.createElement("p"); identity.textContent = `源表：${options.schema ? options.schema + "." : ""}${options.table}`; body.appendChild(identity);
      if (options.kind === "clear") { const warning = document.createElement("p"); warning.className = "designer-notice"; warning.textContent = "此操作将清空全部数据，不删除表结构。请查看真实计划并确认；外键引用、重置自增和事务行为由后端裁定。"; body.appendChild(warning); }
      else { const label = document.createElement("label"); label.className = "designer-control"; const text = document.createElement("span"); text.textContent = "新表名称（不得覆盖原表或已有表）"; label.append(text, target); body.appendChild(label); }
      body.append(status, plan);
    },
    buttons: [
      { label: "关闭", onClick: () => !busy && !execution?.busy },
      { label: "预览后端计划", onClick: async () => {
        if (busy || locked) return false;
        busy = true; update();
        try {
          const reason = options.writeReason(); if (reason) throw new Error(reason);
          const name = options.kind === "clear" ? null : target.value.trim();
          if (name !== null && (!name || name === options.table || name.toLocaleLowerCase() === options.table.toLocaleLowerCase())) throw new Error("请填写不同于原表的新表名称；不能覆盖原表。");
          const snapshot = await api.readTableDesignSnapshot(options.connectionId, options.schema, options.table, options.ownerId);
          assertDesignSnapshot(snapshot);
          if (snapshot.owner_id !== options.ownerId || snapshot.design.table !== options.table || snapshot.design.schema !== (options.schema || snapshot.schema.database)) throw new Error("操作快照身份不匹配，未执行。");
          const capability = snapshot.capabilities[options.kind]; if (!capability.supported) throw new Error(capability.reason || "当前后端未声明支持此表操作。");
          effectiveSchema = snapshot.design.schema;
          const draft: TableOperation = { connection_id: options.connectionId, schema: effectiveSchema, table: options.table, kind: options.kind, target_table: name,
            base_revision: snapshot.revision, source_fingerprint: snapshot.source_fingerprint };
          const next = await api.previewTableOperation(draft, options.ownerId);
          const bound = next.operation;
          if (!next.request_sql || next.revision !== snapshot.revision || bound.connection_id !== draft.connection_id || bound.schema !== draft.schema || bound.table !== draft.table || bound.kind !== draft.kind || (bound.target_table ?? null) !== (draft.target_table ?? null) || bound.base_revision !== draft.base_revision || bound.source_fingerprint !== draft.source_fingerprint) throw new Error("预览未绑定完整操作载荷或版本不匹配。");
          if (options.kind !== "clear" && target.value.trim() !== name) throw new Error("预览期间目标名称变化，请重新预览。");
          operation = draft; preview = next; preparedName = name ?? ""; plan.innerHTML = ""; appendTablePlan(plan, next); status.textContent = "后端计划已就绪。请核对源表、目标表和全部 SQL 后确认执行。";
        } catch (error) { operation = null; preview = null; status.textContent = String(error); options.toast("error", String(error)); }
        finally { busy = false; update(); }
        return false;
      } },
      { label: options.kind === "clear" ? "确认清空全部数据" : "确认创建复制表", primary: true, onClick: async () => {
        if (busy || locked || !operation || !preview) return false;
        busy = true; update();
        try {
          const reason = options.writeReason(); if (reason) throw new Error(reason);
          if (options.kind !== "clear" && target.value.trim() !== preparedName) throw new Error("复制名已变化，请重新预览。");
          const frozenOperation = operation, frozenPreview = preview;
          let response: TableOperationResponse | null = null;
          execution = new ExecutionController({ ...api, executeSqlV2: async request => { sent = true; response = await api.executeTableOperation(request, frozenOperation); return response.outcome; } }, state => { status.textContent = `执行状态：${state}。原目标身份已冻结，不自动重放。`; update(); });
          await execution.run({ execution_id: crypto.randomUUID(), connection_id: options.connectionId, namespace: frozenOperation.schema, owner_id: options.ownerId,
            sql: frozenPreview.request_sql, origin: "table-operation" });
          if (!response) throw new Error("缺少表操作回执。");
          verifiedOperationResponse(response, options.kind, options.table, frozenOperation.target_table ?? null);
          const receipt = (response as TableOperationResponse).receipt!;
          locked = true;
          status.textContent = options.kind === "clear" ? `已验证清空源表「${receipt.source_table}」，影响 ${receipt.affected_rows} 行。` : `已验证创建目标表「${receipt.target_table}」，复制 ${receipt.copied_rows} 行。`;
          options.toast("success", status.textContent);
          try { await options.applied(receipt, effectiveSchema); } catch (error) { status.textContent += ` 操作已成功，但目标刷新失败：${String(error)}。不要重复执行。`; options.toast("error", status.textContent); }
        } catch (error) { if (sent) locked = true; status.textContent = `${String(error)}${sent ? " 请核对真实数据库状态；已禁止重放。" : " 没有确认执行成功。"}`; options.toast("error", status.textContent); }
        finally { busy = false; update(); }
        return false;
      } },
    ] });
  function update(): void {
    const buttons = [...dialog.footer.querySelectorAll<HTMLButtonElement>("button")];
    if (buttons[0]) buttons[0].disabled = busy || !!execution?.busy;
    if (buttons[1]) buttons[1].disabled = busy || locked;
    if (buttons[2]) buttons[2].disabled = busy || locked || !operation || !preview;
    target.disabled = busy || locked;
    dialog.el.setAttribute("aria-busy", String(busy));
  }
  dialog.onClose(() => execution?.dispose()); accessibleTableDialog(dialog, title, () => !busy && !execution?.busy); update();
}
