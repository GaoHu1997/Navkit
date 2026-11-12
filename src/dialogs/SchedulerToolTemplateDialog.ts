/** Existing TaskBox editors are reused for typed templates; saving never creates or runs a legacy job. */
import { buildScheduleToolBox, type TaskBox } from "./ScheduleDialog";
import { field, select, textInput, type Dialog } from "../ui/Dialog";
import { schedulerModal, showModalError } from "../ui/SchedulerPanel";
import { readSchedulerToolDefinition, schedulerToolTemplate, type SchedulerToolKind } from "../core/scheduler-tool-template";
import { SCHEDULER_KIND_LABELS, schedulerCanonicalJson, type ScheduleV2Bindings, type SchedulerTemplate } from "../core/scheduler";
import { createDatabasePicker } from "../core/database-picker";
import { createTablePicker } from "../core/table-picker";
import type { ConnectionConfig } from "../core/types";

function lines(value: string): string[] { return value.split(/\r?\n/).map(line => line.trim()).filter(Boolean); }
function structureBox(connections: readonly ConnectionConfig[], initial: Record<string, unknown>): TaskBox {
  const items = [{ value: "", label: "请选择连接…" }, ...connections.map(c => ({ value: c.id, label: c.name }))];
  const source = select(items, String(initial.source_connection ?? connections[0]?.id ?? ""));
  const target = select(items, String(initial.target_connection ?? connections[0]?.id ?? ""));
  if (initial.source_connection && !connections.some(c => c.id === initial.source_connection)) source.value = "";
  if (initial.target_connection && !connections.some(c => c.id === initial.target_connection)) target.value = "";
  const sourcePicker = createDatabasePicker(connections, source, String(initial.source_schema ?? ""), "源");
  const targetPicker = createDatabasePicker(connections, target, String(initial.target_schema ?? ""), "目标");
  const sourceSchema = sourcePicker.select, targetSchema = targetPicker.select;
  const sourceTablePicker = createTablePicker(connections, source, sourceSchema, String(initial.source_table ?? ""), "源");
  const targetTablePicker = createTablePicker(connections, target, targetSchema, String(initial.target_table ?? ""), "目标");
  const columns = document.createElement("textarea"); columns.value = Array.isArray(initial.columns) ? initial.columns.join("\n") : "";
  const el = document.createElement("div"); el.style.display = "grid"; el.style.gap = "8px";
  el.append(field("源连接:", source), sourcePicker.el, sourceTablePicker.el, field("目标连接:", target), targetPicker.el, targetTablePicker.el, field("同步列（逐行填写；空为全部）:", columns));
  return { el, connId: () => target.value, db: () => targetSchema.value,
    validate: () => sourcePicker.problem() || targetPicker.problem() || sourceTablePicker.problem() || targetTablePicker.problem() || (!source.value || !target.value ? "源、目标连接不能为空" : null),
    options: () => ({ ...initial, source_connection: source.value, source_schema: sourceSchema.value, source_table: sourceTablePicker.value(), target_connection: target.value, target_schema: targetSchema.value, target_table: targetTablePicker.value(), columns: lines(columns.value), allow_drop: false, target_revision: "" }) };
}
function busy(dlg: Dialog, value: boolean): void {
  dlg.el.setAttribute("aria-busy", String(value));
  for (const input of dlg.el.querySelectorAll<HTMLInputElement | HTMLButtonElement | HTMLSelectElement | HTMLTextAreaElement>("input,button,select,textarea")) input.disabled = value || input.dataset.metadataDisabled === "true";
}
export function openSchedulerToolTemplate(bindings: Pick<ScheduleV2Bindings, "saveTemplate">, connections: readonly ConnectionConfig[], kind: SchedulerToolKind, previous?: SchedulerTemplate): Promise<SchedulerTemplate | null> {
  const initial = previous ? readSchedulerToolDefinition(previous).options : {};
  if (previous && previous.kind !== kind) return Promise.reject(new Error("模板类型身份不一致"));
  const name = textInput(previous?.name ?? `${SCHEDULER_KIND_LABELS[kind]}模板`);
  const source = kind === "data_sync" ? String(initial.source_connection ?? connections[0]?.id ?? "") : previous?.connection_id ?? connections[0]?.id ?? "";
  const namespace = kind === "data_sync" ? String(initial.source_schema ?? "") : previous?.namespace ?? "";
  const box = kind === "structure_sync" ? structureBox(connections, initial) : buildScheduleToolBox([...connections], kind, initial, source, namespace, kind === "backup");
  const fileName = textInput(typeof initial.file_name === "string" ? initial.file_name : "");
  return new Promise(resolve => {
    let saving = false, settled = false;
    const finish = (template: SchedulerTemplate | null) => { if (!settled) { settled = true; resolve(template); } };
    const dlg = schedulerModal({ title: `${previous ? "编辑" : "新建"}${SCHEDULER_KIND_LABELS[kind]}动作模板`, width: 710, height: 650, closable: false,
      render: body => {
        body.style.overflow = "auto"; body.style.display = "grid"; body.style.gap = "8px";
        const note = document.createElement("p"); note.textContent = "仅保存本机密文模板，不执行工具、不批准端点。保存后仍须逐项核对实际数据库、文件和写入风险；模板变化会使旧批准失效。";
        body.append(field("模板名:", name), note, box.el);
        if (kind === "backup") body.append(field("备份文件名（空为后端生成，不含目录）:", fileName));
      },
      buttons: [{ label: "保存模板", primary: true, onClick: async () => {
        if (saving || settled) return false;
        try {
          const problem = box.validate(); if (problem) throw new Error(problem);
          const options = { ...initial, ...box.options() };
          if (kind === "backup") { options.routines = []; options.file_name = fileName.value.trim() || null; }
          const submitted = schedulerToolTemplate({ name: name.value.trim(), kind, options }, previous);
          saving = true; busy(dlg, true);
          const result = await bindings.saveTemplate(submitted, previous?.revision || null);
          if (result.id !== submitted.id || !result.revision || result.name !== submitted.name || result.read_only !== submitted.read_only || result.kind !== submitted.kind || result.connection_id !== submitted.connection_id || result.namespace !== submitted.namespace || schedulerCanonicalJson(result.configuration) !== schedulerCanonicalJson(submitted.configuration)) throw new Error("模板保存回执内容未确认；草稿保留，请重新读取核对");
          finish(result); return true;
        } catch (error) { showModalError(dlg, error instanceof Error ? error.message : String(error)); return false; }
        finally { saving = false; busy(dlg, false); }
      } }, { label: "取消", onClick: () => !saving }] }, () => saving);
    dlg.onClose(() => finish(null));
  });
}
