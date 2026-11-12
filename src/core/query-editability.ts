/** 后端证据的前端投影；本地 SQL/列名/行号均不能授予编辑资格。 */
import type { DbValue } from "./api";

export interface QueryActionCapability { supported: boolean; reason: string }
export interface QueryColumnOrigin {
  result_column_id: string;
  result_index: number;
  name: string;
  base_column: string;
  data_type: string;
  writable: boolean;
  key: boolean;
  generated: boolean;
  /** 新回执来自真实基表元数据；旧回执缺字段不授予主键新增权限。 */
  insertable?: boolean;
  nullable?: boolean;
  auto_increment?: boolean;
  default_value?: string | null;
  reason: string;
}
export interface QueryEditability {
  source_execution_id: string;
  result_index: number;
  generation: string;
  result_token: string | null;
  mode: "read_only" | "single_table";
  reason: string;
  schema: string | null;
  table: string | null;
  metadata_revision: string | null;
  columns: QueryColumnOrigin[];
  key_column_ids: string[];
  actions: Record<"update" | "delete" | "insert", QueryActionCapability>;
}
export interface QueryRowChange {
  result_token: string;
  row_token: string | null;
  operation: "update" | "delete" | "insert";
  /** 键是服务端签发的 result_column_id，绝不是客户端 base column/table。 */
  changes: [string, DbValue][];
}
export interface QueryResultContext { source_execution_id: string; result_index: number; generation: string }

const denied = (reason: string): QueryActionCapability => ({ supported: false, reason });
export function readOnlyQueryEditability(context: QueryResultContext, reason: string): QueryEditability {
  return { ...context, result_token: null, mode: "read_only", reason, schema: null, table: null,
    metadata_revision: null, columns: [], key_column_ids: [],
    actions: { update: denied(reason), delete: denied(reason), insert: denied(reason) } };
}
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
const nonempty = (value: unknown): value is string => typeof value === "string" && value.length > 0;

/** 回执不完整/身份不匹配时保留只读。显示组件不能替服务端补齐缺失证明。 */
export function validateQueryEditability(value: unknown, expected: QueryResultContext, resultColumnCount: number): QueryEditability {
  const fail = (): never => { throw new Error("查询结果编辑证据不完整或不属于当前结果，请重新执行查询"); };
  if (!object(value) || value.source_execution_id !== expected.source_execution_id || value.result_index !== expected.result_index
      || value.generation !== expected.generation || !Number.isSafeInteger(resultColumnCount) || resultColumnCount < 0
      || (value.mode !== "single_table" && value.mode !== "read_only") || typeof value.reason !== "string"
      || !Array.isArray(value.columns) || !Array.isArray(value.key_column_ids) || !object(value.actions)) return fail();
  for (const action of ["update", "delete", "insert"]) {
    const cap = value.actions[action];
    if (!object(cap) || typeof cap.supported !== "boolean" || typeof cap.reason !== "string" || (!cap.supported && !cap.reason)) return fail();
  }
  if (value.mode === "read_only") {
    if (["update", "delete", "insert"].some(action => (value.actions as Record<string, QueryActionCapability>)[action].supported)
        || value.result_token !== null || !value.reason || value.schema !== null || value.table !== null || value.metadata_revision !== null
        || value.columns.length !== 0 || value.key_column_ids.length !== 0) return fail();
    return structuredClone(value) as unknown as QueryEditability;
  }
  if (!nonempty(value.result_token) || value.schema !== "main" || !nonempty(value.table)
      || typeof value.metadata_revision !== "string" || !/^[0-9a-f]{64}$/.test(value.metadata_revision)
      || value.columns.length !== resultColumnCount || value.columns.length === 0) return fail();
  const ids = new Set<string>(), bases = new Set<string>();
  for (let i = 0; i < value.columns.length; i++) {
    const col = value.columns[i];
    if (!object(col) || !nonempty(col.result_column_id) || col.result_index !== i || typeof col.name !== "string"
        || !nonempty(col.base_column) || typeof col.data_type !== "string" || typeof col.writable !== "boolean"
        || typeof col.key !== "boolean" || typeof col.generated !== "boolean" || typeof col.reason !== "string"
        || ("insertable" in col && typeof col.insertable !== "boolean")
        || ("nullable" in col && typeof col.nullable !== "boolean")
        || ("auto_increment" in col && typeof col.auto_increment !== "boolean")
        || ("default_value" in col && col.default_value !== null && typeof col.default_value !== "string")
        || ids.has(col.result_column_id) || bases.has(col.base_column.toLowerCase())
        || (col.generated && col.writable) || (col.key && col.writable)) return fail();
    ids.add(col.result_column_id); bases.add(col.base_column.toLowerCase());
  }
  const validatedColumns = value.columns;
  if (!value.key_column_ids.length || new Set(value.key_column_ids).size !== value.key_column_ids.length
      || value.key_column_ids.some(id => typeof id !== "string" || !ids.has(id)
        || !validatedColumns.some(col => object(col) && col.result_column_id === id && col.key === true))) return fail();
  if ((value.actions.update as QueryActionCapability).supported && !validatedColumns.some(col => object(col) && col.writable === true)) return fail();
  if (validatedColumns.some(col => object(col) && col.key === true && !(value.key_column_ids as unknown[]).includes(col.result_column_id))) return fail();
  return structuredClone(value) as unknown as QueryEditability;
}

export function queryActionReason(evidence: QueryEditability | null, action: "update" | "delete" | "insert", columnId?: string): string | null {
  if (!evidence || evidence.mode !== "single_table" || !evidence.result_token) return evidence?.reason || "此结果缺少服务端来源与行令牌，保持只读";
  if (!evidence.actions[action].supported) return evidence.actions[action].reason;
  if (action === "update" && columnId !== undefined) {
    const col = evidence.columns.find(item => item.result_column_id === columnId);
    if (!col?.writable) return col?.reason || "此结果列没有可写的基表列来源";
  }
  return null;
}
