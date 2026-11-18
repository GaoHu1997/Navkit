/** 确认项只对应后端已冻结的完整 review，不用于拼接/筛选 SQL。 */
import type { StructureChange, StructureDiffReport } from "./api";

export function structureChangeId(change: StructureChange, index: number): string {
  return JSON.stringify([index, change.kind, change.object, change.before, change.after, change.risk]);
}
export function structureChangeNeedsConsent(change: StructureChange): boolean {
  return change.risk === "scan_required" || change.risk === "destructive";
}
export function requiredStructureConsents(report: StructureDiffReport): string[] {
  return report.changes.flatMap((change, index) => structureChangeNeedsConsent(change) ? [structureChangeId(change, index)] : []);
}
export function hasCompleteStructureConsent(report: StructureDiffReport, approved: readonly string[]): boolean {
  const required = requiredStructureConsents(report);
  return approved.length === required.length && new Set(approved).size === approved.length
    && required.every(id => approved.includes(id));
}
