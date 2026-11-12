/** Typed scheduler-tool template construction. Only backend services execute these options. */
import { schedulerId, type SchedulerActionKind, type SchedulerTemplate } from "./scheduler";

export type SchedulerToolKind = Exclude<SchedulerActionKind, "sql">;
export interface SchedulerToolDraft { name: string; kind: SchedulerToolKind; options: Record<string, unknown> }
export interface SchedulerToolDefinition { kind: SchedulerToolKind; options: Record<string, unknown> }
const TOOL_KINDS: readonly string[] = ["backup", "export", "import", "data_sync", "structure_sync"];
function requireTool(ok: unknown, message: string): asserts ok { if (!ok) throw new Error(`SCHEDULE_TOOL_OPTIONS: ${message}`); }
function text(value: unknown, field: string, required = true): string {
  requireTool(typeof value === "string" && !/[\u0000-\u001f]/.test(value) && value.length <= 16_384, `${field} 无效`);
  requireTool(!required || value.trim().length > 0, `${field} 不能为空`); return value;
}
function names(value: unknown, field: string, max: number): string[] {
  requireTool(Array.isArray(value) && value.length <= max && value.every(v => typeof v === "string" && v.length > 0 && v.length <= 1024 && !/[\u0000-\u001f]/.test(v)), `${field} 无效`);
  requireTool(new Set(value).size === value.length, `${field} 不能重复`); return [...value];
}
function exactNumbers(value: unknown, depth = 0): void {
  requireTool(depth <= 32, "参数嵌套过深");
  if (typeof value === "number") requireTool(Number.isFinite(value) && Number.isSafeInteger(value), "数值必须是精确安全整数，不能保存已舍入的行数");
  if (value && typeof value === "object") { for (const [key, child] of Object.entries(value)) { requireTool(!["__proto__", "constructor", "prototype"].includes(key), "参数含不支持的原型字段"); exactNumbers(child, depth + 1); } }
}
export function validateSchedulerToolOptions(kind: SchedulerToolKind, source: Record<string, unknown>): Record<string, unknown> {
  requireTool(TOOL_KINDS.includes(kind) && source && typeof source === "object" && !Array.isArray(source), "工具类型无效");
  exactNumbers(source); const o = structuredClone(source);
  if (kind === "data_sync" || kind === "structure_sync") {
    for (const key of ["source_connection", "source_table", "target_connection", "target_table"]) text(o[key], key);
    text(o.source_schema, "源命名空间", false); text(o.target_schema, "目标命名空间", false);
    if (kind === "data_sync") {
      requireTool(o.mode === "data" && o.write_mode === "append" && o.create_target === false && o.delete_missing !== true && o.task_id == null, "自动同步仅追加已有目标表；不能隐式建表、替换、重建或删除目标额外行");
      requireTool(o.sync_mode === "full" || o.sync_mode === "incremental" || o.sync_mode === "", "同步比较模式无效");
      names(o.key_columns ?? [], "业务键", 256);
    } else { requireTool(o.allow_drop === false && o.target_revision === "", "结构模板不能预批准删除或携带客户端目标 revision"); names(o.columns ?? [], "同步列", 256); }
  } else {
    text(o.connection_id, "连接"); text(o.schema, "命名空间", false);
    if (kind === "backup") {
      requireTool(names(o.tables ?? [], "明确备份表集合", 512).length > 0, "请明确选择备份表；空集合不能在执行时重新解释为全部对象");
      requireTool(names(o.routines ?? [], "例程", 512).length === 0, "备份例程尚缺完整对象身份证明，不能用裸名称批准");
      requireTool(typeof o.include_data === "boolean" && typeof o.include_structure === "boolean" && (o.include_data || o.include_structure), "请至少备份结构或数据");
      if (o.file_name != null && o.file_name !== "") requireTool(typeof o.file_name === "string" && !/[\\/\u0000-\u001f]/.test(o.file_name) && ![".", ".."].includes(o.file_name), "备份文件名不能指定目录");
    } else {
      text(o.table, "表"); text(o.file, "文件"); text(o.encoding, "编码");
      requireTool(o.format === "csv" || o.format === "json" || (kind === "export" && o.format === "sql"), "自动导入仅支持 CSV / JSON，不能执行 SQL 文件");
      requireTool(typeof o.has_header === "boolean" && typeof o.delimiter === "string" && o.delimiter.length <= 8, "文件格式选项无效");
      if (kind === "import") { requireTool(o.create_table === false && o.truncate === false, "自动导入不能隐式建表或清空目标"); requireTool(["", "error", "skip", "update"].includes(String(o.on_conflict ?? "")), "冲突策略无效"); names(o.conflict_columns ?? [], "冲突键", 256); }
      else if (o.limit != null) requireTool(Number.isSafeInteger(o.limit) && Number(o.limit) >= 0, "行数上限不能舍入或为负");
    }
  }
  return o;
}
export function readSchedulerToolDefinition(template: SchedulerTemplate): SchedulerToolDefinition {
  requireTool(template.configuration.kind === "tool", "不是工具模板");
  requireTool(template.configuration.options_json.length <= 1_048_576, "工具模板超过 1 MiB");
  const definition: unknown = JSON.parse(template.configuration.options_json);
  requireTool(definition && typeof definition === "object" && !Array.isArray(definition), "模板 JSON 无效");
  const value = definition as Record<string, unknown>;
  requireTool(Object.keys(value).every(key => key === "kind" || key === "options") && value.kind === template.kind && TOOL_KINDS.includes(template.kind), "模板类型身份不一致");
  const options = validateSchedulerToolOptions(template.kind as SchedulerToolKind, value.options as Record<string, unknown>);
  const connection = template.kind === "data_sync" || template.kind === "structure_sync" ? options.target_connection : options.connection_id;
  const namespace = template.kind === "data_sync" || template.kind === "structure_sync" ? options.target_schema : options.schema;
  requireTool(template.connection_id === connection && template.namespace === namespace, "模板绑定必须是实际写入目标，不是源连接");
  return { kind: template.kind as SchedulerToolKind, options };
}
export function schedulerToolTemplate(draft: SchedulerToolDraft, previous?: SchedulerTemplate): SchedulerTemplate {
  const options = validateSchedulerToolOptions(draft.kind, draft.options); const name = text(draft.name, "模板名");
  requireTool(name.length <= 256, "模板名超过 256 字符");
  requireTool(!previous || previous.kind === draft.kind, "不能原地修改模板类型身份");
  const target = draft.kind === "data_sync" || draft.kind === "structure_sync";
  return { id: previous?.id ?? schedulerId(), revision: previous?.revision ?? "", name,
    kind: draft.kind, connection_id: String(target ? options.target_connection : options.connection_id),
    namespace: String(target ? options.target_schema : options.schema), read_only: draft.kind === "backup" || draft.kind === "export",
    configuration: { kind: "tool", options_json: JSON.stringify({ kind: draft.kind, options }) } };
}
