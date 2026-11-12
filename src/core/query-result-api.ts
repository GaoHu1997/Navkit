/** PL21 IPC adapter. It never sends SQL or invents old values/row identities. */
import { invoke } from "@tauri-apps/api/core";
import { asIpcError } from "./execution";
import type { MutationPlan } from "./api";
import { validateQueryEditability, type QueryRowChange } from "./query-editability";
import { QueryResultModel, type QueryModelSnapshot } from "./query-result-model";

export interface QueryResultIpcContext {
  execution_id: string;
  connection_id: string;
  owner_id: string;
  namespace: string;
  result_index: number;
}
export interface QueryResultPageIdentity {
  columns: readonly { name: string }[];
  offset: number;
  loaded_rows: number;
  total_rows: number;
}
const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const uuid = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value) && !/^0{8}-0{4}-0{4}-0{4}-0{12}$/.test(value);
function contextCopy(value: QueryResultIpcContext): QueryResultIpcContext {
  if (!uuid(value.execution_id) || !value.connection_id || !value.owner_id || typeof value.namespace !== "string"
      || !Number.isSafeInteger(value.result_index) || value.result_index < 0 || value.result_index >= 128) throw new Error("查询结果上下文无效；没有发送请求");
  return { execution_id: value.execution_id, connection_id: value.connection_id, owner_id: value.owner_id, namespace: value.namespace, result_index: value.result_index };
}
function fail(message: string): never { throw new Error(`${message}；保留原只读结果，不会重新执行 SQL`); }
/** A complete server prefix is validated before selecting the current spool page by capture order. */
export function querySnapshotForPage(raw: unknown, context: QueryResultIpcContext, page: QueryResultPageIdentity): QueryModelSnapshot {
  contextCopy(context);
  if (!record(raw) || Object.keys(raw).sort().join(",") !== "editability,loaded_rows,rows,total_rows" || !record(raw.editability)
      || !uuid(raw.editability.generation) || !Array.isArray(raw.rows) || !Number.isSafeInteger(raw.loaded_rows)
      || raw.loaded_rows !== raw.rows.length || raw.rows.length > 10_000 || !Number.isSafeInteger(raw.total_rows)
      || (raw.total_rows as number) < raw.rows.length || !Array.isArray(page.columns) || page.columns.length > 1024
      || !Number.isSafeInteger(page.offset) || page.offset < 0 || !Number.isSafeInteger(page.loaded_rows) || page.loaded_rows < 0
      || !Number.isSafeInteger(page.total_rows) || page.total_rows < 0 || raw.total_rows !== page.total_rows) fail("查询证据的完整行数、代际或页边界无效");
  if (new TextEncoder().encode(JSON.stringify(raw)).length > 20 * 1024 * 1024) fail("查询证据超过单结果缓存限额");
  const expected = { source_execution_id: context.execution_id, result_index: context.result_index, generation: raw.editability.generation };
  const proof = validateQueryEditability(raw.editability, expected, page.columns.length);
  if (proof.mode !== "single_table") fail(proof.reason);
  if (proof.columns.some((column, i) => column.name !== page.columns[i].name)) fail("查询证据与当前结果的投影列不一致");
  const rows = raw.rows.map(row => {
    if (!record(row) || Object.keys(row).sort().join(",") !== "row_token,token,values" || !uuid(row.token) || !uuid(row.row_token) || !Array.isArray(row.values)) fail("查询行令牌或类型化原值不完整");
    return { token: row.token, row_token: row.row_token, values: structuredClone(row.values) };
  });
  const full = { ...expected, columns: proof.columns.map(column => ({ id: column.result_column_id, name: column.name, data_type: column.data_type })),
    rows, total_rows: raw.total_rows, editability: proof } as QueryModelSnapshot;
  // Production model checks typed values, row widths and duplicate row/view identities.
  new QueryResultModel(full);
  if (page.offset + page.loaded_rows > rows.length || page.loaded_rows === 0 && raw.total_rows !== 0) fail("当前缓存页超出已签发行令牌的前缀范围，此页只读");
  return { ...full, rows: full.rows.slice(page.offset, page.offset + page.loaded_rows) };
}
async function call<T>(command: string, args: Record<string, unknown>): Promise<T> {
  try { return await invoke<T>(command, args); } catch (error) { throw asIpcError(error, command); }
}
export const queryResultApi = {
  snapshot: async (context: QueryResultIpcContext, page: QueryResultPageIdentity): Promise<QueryModelSnapshot> => {
    const pinned = contextCopy(context);
    const identity = structuredClone(page);
    return querySnapshotForPage(await call<unknown>("query_result_snapshot", { context: pinned }), pinned, identity);
  },
  preview: (context: QueryResultIpcContext, change: QueryRowChange): Promise<MutationPlan> => {
    if (!record(change) || Object.keys(change).sort().join(",") !== "changes,operation,result_token,row_token") throw new Error("查询写入只接受结果令牌、行令牌和类型化改动");
    return call<MutationPlan>("preview_query_row_mutation", { context: contextCopy(context), change: structuredClone(change) });
  },
  release: async (context: QueryResultIpcContext): Promise<void> => {
    const response = await call<unknown>("release_query_result", { context: contextCopy(context) });
    if (response !== undefined && response !== null) throw new Error("释放查询来源证据的回执无效");
  },
};
