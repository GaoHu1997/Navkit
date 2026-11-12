/** 页面编辑态：仅用于离页判断、丢弃与只读证据展示；不写入任何本机恢复文件。 */
export type DraftKind = "query" | "view" | "routine" | "table_structure" | "row_changes";

export interface DraftSnapshot {
  kind: DraftKind;
  baseline: unknown;
  payload: unknown;
}
