import type { TableDesignColumn, TableDesignPayload, TableDesignSnapshot, TableDesignResponse, TableOperationResponse } from "./api";
import type { CheckDesign, TriggerDesign, TableOptionsDesign } from "./types";

export type DesignerColumn = TableDesignColumn & { key: string; deleted: boolean };
export type DesignerOptions = Omit<TableOptionsDesign, "fillfactor"> & { fillfactor?: number | string | null };
export interface TableDesignerDraft {
  columns: DesignerColumn[];
  primary_key: string[];
  indexes: NonNullable<TableDesignPayload["indexes"]> | undefined;
  foreign_keys: NonNullable<TableDesignPayload["foreign_keys"]> | undefined;
  checks: CheckDesign[] | undefined;
  triggers: TriggerDesign[] | undefined;
  options: DesignerOptions | undefined;
  comment: string | null | undefined;
}
export const designClone = <T>(value: T): T => structuredClone(value);
let nextField = 0;
/** 列展示类型：把后端返回的 `varchar(64)`、`decimal(18,4)` 里的长度/小数位拆回独立字段，
 *  让「类型」只显示基础类型，长度与小数位在各自列编辑（对齐图2）。enum/set 等复杂类型原样保留。 */
const COLUMN_TYPE_PARAMS = /^([A-Za-z_][A-Za-z0-9_ ]*?)\s*\(\s*(\d+)\s*(?:,\s*(\d+)\s*)?\)\s*$/;
function splitColumnType(column: TableDesignColumn): TableDesignColumn {
  const raw = (column.data_type ?? "").trim();
  const match = COLUMN_TYPE_PARAMS.exec(raw);
  if (!match) return column;
  const base = match[1].trim();
  if (/^(enum|set)$/i.test(base)) return column;
  const length = column.length != null && column.length !== "" ? column.length : match[2];
  const scale = column.scale != null && column.scale !== "" ? column.scale : (match[3] ?? null);
  return { ...column, data_type: base, length, scale };
}
export function draftFromSnapshot(snapshot: TableDesignSnapshot): TableDesignerDraft {
  assertDesignSnapshot(snapshot);
  const d = designClone(snapshot.design);
  return { columns: d.columns.map(c => ({ ...splitColumnType(c), key: `field-${++nextField}`, deleted: false })), primary_key: d.primary_key,
    indexes: d.indexes, foreign_keys: d.foreign_keys, checks: d.checks, triggers: d.triggers, options: d.options, comment: d.comment };
}
export function assertDesignSnapshot(snapshot: TableDesignSnapshot): void {
  if (snapshot.completeness !== "complete" || snapshot.objects.completeness !== "complete" || !snapshot.revision || !snapshot.source_fingerprint || !snapshot.owner_id) {
    throw new Error("表设计元数据不完整或未知；已保留当前草稿，不能把未知对象当作空集合保存。");
  }
  if (!snapshot.design || !Array.isArray(snapshot.design.columns) || !Array.isArray(snapshot.design.primary_key)) throw new Error("表设计基线缺少字段或有序主键。");
}
function same(a: unknown, b: unknown): boolean { return JSON.stringify(a) === JSON.stringify(b); }
export function draftSignature(draft: TableDesignerDraft): string {
  return JSON.stringify(draft, (_key, value) => value === undefined ? { __table_designer_undefined: true } : value);
}
function namesUnique(names: string[], label: string): void {
  if (names.some(n => !n.trim())) throw new Error(`${label}存在未填写的名称，请补全或明确删除该草稿行。`);
  if (new Set(names).size !== names.length) throw new Error(`${label}名称重复。`);
}
export function payloadFromDraft(snapshot: TableDesignSnapshot, draft: TableDesignerDraft): TableDesignPayload {
  assertDesignSnapshot(snapshot);
  const live = draft.columns.filter(c => !c.deleted);
  if (!live.length) throw new Error("至少保留一个字段。");
  namesUnique(live.map(c => c.name.trim()), "字段");
  const fields = new Set(live.map(c => c.name.trim()));
  for (const c of live) {
    if (c.default_value !== null && c.default_value !== undefined && !c.default_value.trim()) throw new Error(`字段「${c.name}」默认值 SQL 表达式尚未填写；请选择无默认值或填写完整表达式。`);
    if (!c.data_type.trim()) throw new Error(`字段「${c.name || "未命名"}」未填写类型。`);
    if (c.length != null && c.length !== "" && !/^\d+$/.test(c.length)) throw new Error(`字段「${c.name}」长度必须为非负整数；复杂类型参数请直接填写在类型中。`);
    if (c.scale != null && c.scale !== "" && !/^\d+$/.test(c.scale)) throw new Error(`字段「${c.name}」小数位必须为非负整数。`);
  }
  const columnList = (columns: string[], label: string) => {
    if (!columns.length) throw new Error(`${label}至少选择一个字段。`);
    namesUnique(columns, label);
    for (const name of columns) if (!fields.has(name)) throw new Error(`${label}引用不存在或待删除的字段「${name}」。`);
  };
  if (draft.primary_key.length) columnList(draft.primary_key, "主键");
  if (draft.indexes !== undefined) {
    namesUnique(draft.indexes.map(i => i.name.trim()), "索引");
    for (const i of draft.indexes) {
      columnList(i.columns, `索引「${i.name}」`);
      if (i.descending?.some(name => !i.columns.includes(name))) throw new Error(`索引「${i.name}」的排序方向引用了不存在的列。`);
    }
  }
  if (draft.foreign_keys !== undefined) {
    namesUnique(draft.foreign_keys.map(f => f.name.trim()), "外键");
    for (const f of draft.foreign_keys) {
      columnList(f.columns, `外键「${f.name}」`);
      if (!f.ref_table.trim() || !f.ref_columns.length || f.ref_columns.some(c => !c.trim()) || f.ref_columns.length !== f.columns.length) throw new Error(`外键「${f.name}」目标表和按顺序对应的引用字段未补全。`);
      namesUnique(f.ref_columns, `外键「${f.name}」引用字段`);
    }
  }
  if (draft.checks !== undefined) {
    namesUnique(draft.checks.map(c => c.name.trim()), "检查约束");
    if (draft.checks.some(c => !c.expression.trim())) throw new Error("检查约束存在未填写的表达式，请补全或明确删除。");
    if (!same(draft.checks, snapshot.design.checks) && !snapshot.capabilities.checks.supported) throw new Error(snapshot.capabilities.checks.reason || "当前协议不支持修改检查约束。");
  }
  if (draft.triggers !== undefined) {
    namesUnique(draft.triggers.map(t => t.name.trim()), "触发器");
    for (const t of draft.triggers) {
      const original = snapshot.design.triggers?.find(old => old.name === t.orig_name || old.name === t.name);
      if (same(t, original)) continue;
      if (!t.timing.trim() || !t.event.trim() || !t.statement.trim()) throw new Error(`触发器「${t.name}」的时机、事件或语句尚未补全。`);
    }
    if (!same(draft.triggers, snapshot.design.triggers) && !snapshot.capabilities.triggers.supported) throw new Error(snapshot.capabilities.triggers.reason || "当前协议不支持修改触发器。");
  }
  const fillfactor = draft.options?.fillfactor;
  if (fillfactor != null && (typeof fillfactor !== "number" || !Number.isInteger(fillfactor) || fillfactor < 10 || fillfactor > 100)) throw new Error("填充因子必须为 10 到 100 之间的整数。");
  const design: TableDesignPayload = { schema: snapshot.design.schema, table: snapshot.design.table,
    columns: live.map(({ key: _key, deleted: _deleted, ...column }) => ({ ...column, name: column.name.trim(), data_type: column.data_type.trim() })),
    primary_key: [...draft.primary_key], base_revision: snapshot.revision, source_fingerprint: snapshot.source_fingerprint };
  for (const key of ["indexes", "foreign_keys", "checks", "triggers", "comment"] as const) {
    if (!same(draft[key], snapshot.design[key])) Object.assign(design, { [key]: designClone(draft[key]) });
  }
  if (!same(draft.options, snapshot.design.options)) {
    if (!snapshot.capabilities.options.supported) throw new Error(snapshot.capabilities.options.reason || "此协议不支持修改表选项。");
    const changed: TableOptionsDesign = {};
    for (const key of Object.keys(draft.options ?? {}) as (keyof TableOptionsDesign)[]) {
      if (same(draft.options?.[key], snapshot.design.options?.[key])) continue;
      if (!snapshot.capabilities.supported_options.includes(key)) throw new Error(`后端未声明支持表选项「${key}」。`);
      Object.assign(changed, { [key]: draft.options?.[key] });
    }
    if (Object.keys(changed).length) design.options = changed;
  }
  return design;
}
export function verifiedDesignResponse(response: TableDesignResponse): TableDesignSnapshot {
  const { outcome, receipt } = response;
  if (outcome.state !== "succeeded" || outcome.resources_pending || outcome.transaction_state === "unknown" || !receipt || !receipt.verified || receipt.unverified || receipt.mismatches.length || receipt.steps.some(s => s.status !== "applied") || !["committed", "implicit", "none"].includes(receipt.transaction) || !receipt.snapshot) {
    throw new Error(`表结构回执未证实完整成功；草稿已保留，不能自动重放。${receipt?.mismatches.join("；") || outcome.error?.message || outcome.state}`);
  }
  assertDesignSnapshot(receipt.snapshot);
  return receipt.snapshot;
}
export function verifiedOperationResponse(response: TableOperationResponse, kind: string, source: string, target: string | null): void {
  const { outcome, receipt } = response;
  if (outcome.state !== "succeeded" || outcome.resources_pending || outcome.transaction_state === "unknown" || !receipt || receipt.kind !== kind || receipt.source_table !== source || (receipt.target_table ?? null) !== target || !receipt.verified || receipt.mismatches.length || receipt.steps.some(s => s.status !== "applied") || !["committed", "implicit", "none"].includes(receipt.transaction)) {
    throw new Error(`表操作未获得可验证的完整成功回执；请核对数据库状态，不能自动重放。${receipt?.mismatches.join("；") || outcome.error?.message || outcome.state}`);
  }
}
