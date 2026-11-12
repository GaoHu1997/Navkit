/** 已加载结果的纯模型。筛选/排序/列隐藏只改变映射，编辑和复制始终按 token。 */
import type { DbValue } from "./api";
import type { GridCopySnapshot, ClipboardValue } from "./grid-clipboard";
import { queryActionReason, validateQueryEditability, type QueryEditability, type QueryResultContext, type QueryRowChange } from "./query-editability";

export interface QueryModelColumn { id: string; name: string; data_type: string; hidden?: boolean }
export interface QueryModelRow { token: string; values: DbValue[]; row_token?: string | null }
export interface QueryModelSnapshot extends QueryResultContext {
  columns: QueryModelColumn[];
  rows: QueryModelRow[];
  /** 总行数只用于标明范围，模型从不读取/推断未加载行。 */
  total_rows?: number;
  editability?: QueryEditability | null;
}
export interface QueryLocalSort { column_id: string; ascending: boolean; nulls: "first" | "last" }
export type QueryFilterOperator = "eq" | "ne" | "lt" | "lte" | "gt" | "gte" | "contains" | "starts_with" | "ends_with" | "is_null" | "is_not_null";
export interface QueryLocalFilter { column_id: string; operator: QueryFilterOperator; value?: DbValue; case_sensitive?: boolean }
export interface QueryFilterGroup { logic?: "AND" | "OR"; conditions: QueryLocalFilter[] }
export interface QueryLocalView {
  tokens: string[];
  rows: DbValue[][];
  loaded: number;
  visible: number;
  total: number;
  dirty: number;
  hidden_dirty: number;
  scope: string;
  warnings: string[];
}
interface DecimalParts { negative: boolean; digits: string; exponent: number }
const clone = <T>(value: T): T => structuredClone(value);
const text = (value: DbValue): string => value.kind === "null" ? "" : String("value" in value ? value.value : "");
const compareText = (a: string, b: string): number => a === b ? 0 : a < b ? -1 : 1;

function decimal(value: DbValue): DecimalParts | null {
  if (value.kind !== "integer" && value.kind !== "decimal" && value.kind !== "int") return null;
  const s = text(value);
  if (s.length > 1024 || (value.kind === "int" && !Number.isSafeInteger(value.value))) return null;
  const m = /^([+-]?)(?:(\d+)(?:\.(\d*))?|\.(\d+))(?:[eE]([+-]?\d+))?$/.exec(s);
  if (!m) return null;
  const power = Number(m[5] ?? 0);
  if (!Number.isSafeInteger(power) || Math.abs(power) > 1000) return null;
  const fraction = m[3] ?? m[4] ?? "";
  let digits = ((m[2] ?? "") + fraction).replace(/^0+/, "");
  let exponent = power - fraction.length;
  if (!digits) return { negative: false, digits: "0", exponent: 0 };
  const tail = /0+$/.exec(digits)?.[0].length ?? 0;
  if (tail) { digits = digits.slice(0, -tail); exponent += tail; }
  return { negative: m[1] === "-", digits, exponent };
}
function compareDecimal(a: DecimalParts, b: DecimalParts): number {
  if (a.negative !== b.negative) return a.negative ? -1 : 1;
  // 长度/指数是有界整数；不将任何数值本体交给 Number。
  const leftSize = a.digits === "0" ? -Infinity : a.digits.length + a.exponent;
  const rightSize = b.digits === "0" ? -Infinity : b.digits.length + b.exponent;
  const result = leftSize === rightSize
    ? compareText(a.digits.padEnd(Math.max(a.digits.length, b.digits.length), "0"), b.digits.padEnd(Math.max(a.digits.length, b.digits.length), "0"))
    : leftSize < rightSize ? -1 : 1;
  return a.negative ? -result : result;
}
function timestamp(value: DbValue): bigint | null {
  if (value.kind !== "timestamptz") return null;
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?(Z|([+-])(\d{2}):(\d{2}))$/.exec(value.value);
  if (!m) return null;
  const [year, month, day, hour, minute, second] = m.slice(1, 7).map(Number);
  if (month < 1 || month > 12 || day < 1 || day > 31 || hour > 23 || minute > 59 || second > 59) return null;
  const d = new Date(0); d.setUTCFullYear(year, month - 1, day); d.setUTCHours(hour, minute, second, 0);
  if (d.getUTCFullYear() !== year || d.getUTCMonth() !== month - 1 || d.getUTCDate() !== day) return null;
  const offsetHours = Number(m[10] ?? 0), offsetMinutes = Number(m[11] ?? 0);
  if (offsetHours > 18 || offsetMinutes > 59 || (offsetHours === 18 && offsetMinutes !== 0)) return null;
  const offset = (offsetHours * 60 + offsetMinutes) * (m[9] === "-" ? -1 : 1);
  return BigInt(d.getTime()) * 1_000_000n - BigInt(offset) * 60_000_000_000n + BigInt((m[7] ?? "").padEnd(9, "0") || "0");
}
function localTemporal(value: DbValue): string | null {
  const raw = text(value);
  if (value.kind === "date" && /^\d{4}-\d{2}-\d{2}$/.test(raw)) return timestamp({ kind: "timestamptz", value: `${raw}T00:00:00Z` }) === null ? null : raw;
  if (value.kind === "time") {
    const match = /^(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?$/.exec(raw);
    return match && Number(match[1]) < 24 && Number(match[2]) < 60 && Number(match[3]) < 60 ? raw.slice(0, 8) + "." + (match[4] ?? "").padEnd(9, "0") : null;
  }
  if (value.kind === "timestamp" && /^\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$/.test(raw)) {
    const normal = raw.replace(" ", "T");
    return timestamp({ kind: "timestamptz", value: `${normal}Z` }) === null ? null : normal.slice(0, 19) + "." + (normal.slice(20) || "").padEnd(9, "0");
  }
  return null;
}
/** fallback 非空意味着类型未知/混合，UI 必须显示文本比较提示。 */
export function compareQueryValues(a: DbValue, b: DbValue): { comparison: number; fallback: string | null } {
  const da = decimal(a), db = decimal(b);
  if (da && db) return { comparison: compareDecimal(da, db), fallback: null };
  if (a.kind === "float" && b.kind === "float" && Number.isFinite(a.value) && Number.isFinite(b.value)) return { comparison: a.value === b.value ? 0 : a.value < b.value ? -1 : 1, fallback: null };
  if (a.kind === "bool" && b.kind === "bool") return { comparison: Number(a.value) - Number(b.value), fallback: null };
  if (a.kind === "timestamptz" && b.kind === "timestamptz") {
    const left = timestamp(a), right = timestamp(b);
    if (left !== null && right !== null) return { comparison: left === right ? 0 : left < right ? -1 : 1, fallback: null };
  }
  const lt = localTemporal(a), rt = localTemporal(b);
  if (a.kind === b.kind && lt !== null && rt !== null) return { comparison: compareText(lt, rt), fallback: null };
  if (a.kind === "text" && b.kind === "text") return { comparison: compareText(a.value, b.value), fallback: null };
  return { comparison: compareText(text(a), text(b)), fallback: "混合、未知或无效类型使用稳定文本比较；精确数值未转换为 Number" };
}
function same(a: DbValue, b: DbValue): boolean { return JSON.stringify(a) === JSON.stringify(b); }
function clipboard(value: DbValue): ClipboardValue {
  if (value.kind === "null") return null;
  if (value.kind === "binary") return { kind: "binary", value: value.value, encoding: "base64" };
  return { kind: value.kind, value: text(value) };
}
function validValue(value: DbValue): void {
  const fail = (): never => { throw new Error("结果缺少有效且无损的类型化原值"); };
  if (!value || typeof value !== "object" || Array.isArray(value) || typeof value.kind !== "string") fail();
  const keys = Object.keys(value).sort().join(",");
  if (value.kind === "null") { if (keys !== "kind") fail(); return; }
  if (keys !== "kind,value" || !("value" in value)) fail();
  if (value.kind === "int") { if (typeof value.value !== "number" || !Number.isSafeInteger(value.value)) fail(); return; }
  if (value.kind === "float") { if (typeof value.value !== "number" || !Number.isFinite(value.value)) fail(); return; }
  if (value.kind === "bool") { if (typeof value.value !== "boolean") fail(); return; }
  if (typeof value.value !== "string" || new TextEncoder().encode(value.value).byteLength > 16 * 1024 * 1024) fail();
  switch (value.kind) {
    case "integer": {
      if (!/^[+-]?\d{1,20}$/.test(value.value)) fail();
      const n = BigInt(value.value); if (n < -9_223_372_036_854_775_808n || n > 18_446_744_073_709_551_615n) fail(); break;
    }
    case "decimal": if (!decimal(value)) fail(); break;
    case "binary": try { if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value.value) || btoa(atob(value.value)) !== value.value) fail(); } catch { fail(); } break;
    case "date": case "time": case "timestamp": if (localTemporal(value) === null) fail(); break;
    case "timestamptz": if (timestamp(value) === null) fail(); break;
    case "uuid": if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value.value)) fail(); break;
    case "json": try { JSON.parse(value.value); } catch { fail(); } break;
    case "text": break;
    default: fail();
  }
}

export class QueryResultModel {
  private snapshot: QueryModelSnapshot;
  private readonly byToken = new Map<string, QueryModelRow>();
  private readonly ordinal = new Map<string, number>();
  private readonly columns = new Map<string, number>();
  private readonly pending = new Map<string, Map<string, DbValue>>();
  private readonly deleted = new Set<string>();
  private selected = new Set<string>();
  private filters: QueryFilterGroup[] = [];
  private sorts: QueryLocalSort[] = [];
  private hidden = new Set<string>();
  private invalidReason: string | null = null;
  private copyGeneration = 0;

  constructor(snapshot: QueryModelSnapshot) {
    this.snapshot = clone(snapshot);
    if (!snapshot.source_execution_id || !snapshot.generation || !Number.isSafeInteger(snapshot.result_index) || snapshot.result_index < 0
        || (snapshot.total_rows !== undefined && (!Number.isSafeInteger(snapshot.total_rows) || snapshot.total_rows < snapshot.rows.length))) throw new Error("查询结果身份或总行数无效");
    snapshot.columns.forEach((column, index) => {
      if (!column.id || this.columns.has(column.id)) throw new Error("结果列 ID 缺失或重复");
      this.columns.set(column.id, index); if (column.hidden) this.hidden.add(column.id);
    });
    const rowTokens = new Set<string>();
    this.snapshot.rows.forEach((row, index) => {
      if (!row.token || this.byToken.has(row.token) || row.values.length !== snapshot.columns.length
          || (row.row_token != null && (typeof row.row_token !== "string" || !row.row_token || rowTokens.has(row.row_token)))) throw new Error("结果行 token 重复或列数不匹配");
      if (row.row_token) rowTokens.add(row.row_token);
      row.values.forEach(validValue); this.byToken.set(row.token, row); this.ordinal.set(row.token, index);
    });
    if (this.snapshot.editability) {
      this.snapshot.editability = validateQueryEditability(this.snapshot.editability, this.context, snapshot.columns.length);
      if (this.snapshot.editability.mode === "single_table" && this.snapshot.editability.columns.some((column, i) => column.result_column_id !== this.snapshot.columns[i].id)) throw new Error("显示列与服务端来源列 ID 不匹配");
    }
  }
  get dirty(): boolean { return this.pending.size > 0 || this.deleted.size > 0; }
  get context(): QueryResultContext { const { source_execution_id, result_index, generation } = this.snapshot; return { source_execution_id, result_index, generation }; }
  invalidate(reason: string): void { this.invalidReason = reason || "查询上下文已改变，必须重新执行"; this.copyGeneration++; }
  replace(snapshot: QueryModelSnapshot): void {
    if (this.dirty) throw new Error("查询结果有未保存改动，请先处理草稿再刷新或翻页");
    const replacement = new QueryResultModel(snapshot);
    this.snapshot = replacement.snapshot;
    this.byToken.clear(); replacement.byToken.forEach((v, k) => this.byToken.set(k, v));
    this.ordinal.clear(); replacement.ordinal.forEach((v, k) => this.ordinal.set(k, v));
    this.columns.clear(); replacement.columns.forEach((v, k) => this.columns.set(k, v));
    this.hidden = replacement.hidden; this.selected.clear(); this.filters = []; this.sorts = []; this.invalidReason = null; this.copyGeneration++;
  }
  setFilters(groups: QueryFilterGroup[]): void {
    for (const group of groups) {
      if (group.logic !== undefined && group.logic !== "AND" && group.logic !== "OR") throw new Error("未知筛选组合逻辑");
      for (const filter of group.conditions) {
      this.column(filter.column_id);
      if (!["eq", "ne", "lt", "lte", "gt", "gte", "contains", "starts_with", "ends_with", "is_null", "is_not_null"].includes(filter.operator)) throw new Error("未知本地筛选操作");
      if (filter.operator !== "is_null" && filter.operator !== "is_not_null" && !filter.value) throw new Error("筛选条件缺少类型化比较值");
      if (filter.value) validValue(filter.value);
      }
    }
    this.filters = clone(groups); this.copyGeneration++;
  }
  setSorts(sorts: QueryLocalSort[]): void {
    const used = new Set<string>();
    for (const sort of sorts) { this.column(sort.column_id); if (used.has(sort.column_id) || !["first", "last"].includes(sort.nulls) || typeof sort.ascending !== "boolean") throw new Error("排序列重复、方向或 NULL 位置未指定"); used.add(sort.column_id); }
    this.sorts = clone(sorts); this.copyGeneration++;
  }
  setHiddenColumns(ids: string[]): void { ids.forEach(id => this.column(id)); this.hidden = new Set(ids); this.copyGeneration++; }
  select(tokens: string[]): void { tokens.forEach(token => this.row(token)); this.selected = new Set(tokens); this.copyGeneration++; }
  selectedTokens(): string[] { return this.snapshot.rows.filter(row => this.selected.has(row.token)).map(row => row.token); }
  values(token: string): DbValue[] {
    const row = this.row(token), changes = this.pending.get(token);
    return row.values.map((value, i) => clone(changes?.get(this.snapshot.columns[i].id) ?? value));
  }
  stage(token: string, columnId: string, value: DbValue): void {
    this.requireAction("update", columnId); validValue(value);
    const row = this.row(token), index = this.column(columnId);
    if (!row.row_token || this.deleted.has(token)) throw new Error("此行缺少服务端令牌或已标记删除");
    const changes = this.pending.get(token) ?? new Map<string, DbValue>();
    if (same(value, row.values[index])) changes.delete(columnId); else changes.set(columnId, clone(value));
    if (changes.size) this.pending.set(token, changes); else this.pending.delete(token);
    this.copyGeneration++;
  }
  markDeleted(token: string, yes = true): void {
    this.requireAction("delete"); if (!this.row(token).row_token) throw new Error("此行缺少服务端令牌");
    if (yes) this.deleted.add(token); else this.deleted.delete(token);
    this.copyGeneration++;
  }
  discard(token?: string): void {
    if (token !== undefined) { this.row(token); this.pending.delete(token); this.deleted.delete(token); }
    else { this.pending.clear(); this.deleted.clear(); }
    this.copyGeneration++;
  }
  changes(): QueryRowChange[] {
    this.requireLive();
    const result = this.snapshot.editability?.result_token;
    if (!result && this.dirty) throw new Error("缺少结果令牌，不能提交本地改动");
    return this.snapshot.rows.flatMap<QueryRowChange>(row => this.deleted.has(row.token)
      ? [{ result_token: result!, row_token: row.row_token!, operation: "delete" as const, changes: [] }]
      : this.pending.has(row.token) ? [{ result_token: result!, row_token: row.row_token!, operation: "update" as const, changes: [...this.pending.get(row.token)!].map(([id, value]) => [id, clone(value)] as [string, DbValue]) }] : []);
  }
  /** 仅供服务端确认提交后调用；结果条件可能不再成立，保留只读草稿并要求重跑。 */
  requireRefreshAfterWrite(): void { this.invalidate("写入已核验；旧查询结果、令牌和筛选成员关系需要重新执行确认"); }
  view(): QueryLocalView {
    const warnings = new Set<string>();
    let rows = this.snapshot.rows.filter(row => this.matches(this.values(row.token), warnings));
    rows = rows.slice().sort((left, right) => {
      const a = this.values(left.token), b = this.values(right.token);
      for (const sort of this.sorts) {
        const index = this.column(sort.column_id), av = a[index], bv = b[index];
        if (av.kind === "null" || bv.kind === "null") {
          if (av.kind === bv.kind) continue;
          return av.kind === "null" ? (sort.nulls === "first" ? -1 : 1) : (sort.nulls === "first" ? 1 : -1);
        }
        const compared = compareQueryValues(av, bv); if (compared.fallback) warnings.add(compared.fallback);
        if (compared.comparison) return compared.comparison * (sort.ascending ? 1 : -1);
      }
      return this.ordinal.get(left.token)! - this.ordinal.get(right.token)!;
    });
    const tokens = rows.map(row => row.token), visible = new Set(tokens);
    const dirty = new Set([...this.pending.keys(), ...this.deleted]);
    return { tokens, rows: tokens.map(token => this.values(token)), loaded: this.snapshot.rows.length, visible: rows.length,
      total: Math.max(this.snapshot.rows.length, this.snapshot.total_rows ?? 0), dirty: dirty.size,
      hidden_dirty: [...dirty].filter(token => !visible.has(token)).length,
      scope: `仅已加载 ${this.snapshot.rows.length} 行；筛选后 ${rows.length} 行`, warnings: [...warnings] };
  }
  copySnapshot(tokens?: string[], cell?: { token: string; column_id: string }): GridCopySnapshot {
    const view = this.view(), selected = tokens ? new Set(tokens) : this.selected;
    if (tokens) tokens.forEach(token => this.row(token));
    const indexes = this.snapshot.columns.map((column, index) => ({ column, index })).filter(({ column }) => !this.hidden.has(column.id));
    if (cell && (!view.tokens.includes(cell.token) || this.hidden.has(cell.column_id))) throw new Error("隐藏行/列不能通过单格复制入口泄露");
    const cellValue = cell ? this.values(cell.token)[this.column(cell.column_id)] : undefined;
    return { generation: this.copyGeneration, columns: indexes.map(({ column }) => ({ name: column.name })),
      rows: view.tokens.filter(token => selected.has(token)).map(token => indexes.map(({ index }) => clipboard(this.values(token)[index]))),
      cell: cellValue === undefined ? undefined : clipboard(cellValue) };
  }
  private column(id: string): number { const index = this.columns.get(id); if (index === undefined) throw new Error("结果列不存在"); return index; }
  private row(token: string): QueryModelRow { const row = this.byToken.get(token); if (!row) throw new Error("结果行 token 不属于当前快照"); return row; }
  private requireLive(): void { if (this.invalidReason) throw new Error(this.invalidReason); }
  private requireAction(action: "update" | "delete" | "insert", column?: string): void { this.requireLive(); const reason = queryActionReason(this.snapshot.editability ?? null, action, column); if (reason) throw new Error(reason); }
  private matches(values: DbValue[], warnings: Set<string>): boolean {
    return this.filters.every(group => {
      if (!group.conditions.length) return true;
      const results = group.conditions.map(filter => {
        const value = values[this.column(filter.column_id)], target = filter.value;
        if (filter.operator === "is_null") return value.kind === "null";
        if (filter.operator === "is_not_null") return value.kind !== "null";
        if (value.kind === "null" || !target || target.kind === "null") return false;
        if (["contains", "starts_with", "ends_with"].includes(filter.operator)) {
          if (value.kind !== "text" || target.kind !== "text") { warnings.add("文本包含筛选仅对文本类型生效"); return false; }
          const a = filter.case_sensitive ? value.value : value.value.toLowerCase(), b = filter.case_sensitive ? target.value : target.value.toLowerCase();
          return filter.operator === "contains" ? a.includes(b) : filter.operator === "starts_with" ? a.startsWith(b) : a.endsWith(b);
        }
        const compared = compareQueryValues(value, target); if (compared.fallback) warnings.add(compared.fallback);
        const n = compared.comparison;
        return filter.operator === "eq" ? n === 0 : filter.operator === "ne" ? n !== 0 : filter.operator === "lt" ? n < 0 : filter.operator === "lte" ? n <= 0 : filter.operator === "gt" ? n > 0 : n >= 0;
      });
      return group.logic === "OR" ? results.some(Boolean) : results.every(Boolean);
    });
  }
}
