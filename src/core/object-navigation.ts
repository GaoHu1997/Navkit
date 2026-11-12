/** 将后端完整对象目录投影到导航；身份始终随描述符传递，不从标签/路径还原。 */
import { objectAction, objectKey, type MetadataSection, type ObjectDescriptor } from "./object-catalog";
import type { TreeItem, ObjectType } from "./types";
import type { InfoPanelState } from "../ui/InfoPanel";

/** 目录树当前只展示函数/存储过程；触发器、事件、序列、物化视图暂无入口，需要时再加回。 */
export const TREE_CATALOG_KINDS = ["function", "procedure"];
/** 后端目录接口与对象浏览器支持的完整类别（树入口暂时只开放上面两类）。 */
export const CATALOG_KINDS = ["function", "procedure", "trigger", "event", "sequence", "materialized_view"];
export type CatalogListing = { objects: ObjectDescriptor[]; sections: Record<string, MetadataSection>; revision: string };
const labels: Record<string, string> = { function: "函数", procedure: "存储过程", trigger: "触发器", event: "事件", sequence: "序列", materialized_view: "物化视图" };
export const catalogKindLabel = (kind: string): string => labels[kind] ?? kind;
export const catalogIcon = (kind: string): string => kind === "function" || kind === "procedure" ? "info_pane_function" : kind === "trigger" ? "trigger" : kind === "materialized_view" ? "info_pane_view" : "info_pane_database";
export function catalogNotice(sections: Record<string, MetadataSection>): string {
  return Object.entries(sections).filter(([, section]) => section.state !== "known").map(([kind, section]) => `${catalogKindLabel(kind)}：${section.state === "unsupported" ? "当前驱动不支持" : "目录完整性未知"}${section.reason ? `（${section.reason}）` : ""}`).join("；");
}
export function catalogTreeItem(baseId: string, descriptor: ObjectDescriptor): TreeItem {
  const object = descriptor.object;
  return { id: `${baseId}/catalog/${encodeURIComponent(objectKey(object))}`, label: descriptor.display_name,
    type: object.kind as ObjectType, icon: catalogIcon(object.kind),
    objectDescriptor: descriptor, children: [], expandable: false, has_children: false, expanded: false };
}
export function catalogTreeGroups(baseId: string, catalog: CatalogListing): TreeItem[] {
  const groups = [
    { suffix: "function", label: "函数", kinds: ["function", "procedure"], icon: "pict_function" },
  ];
  return groups.flatMap<TreeItem>(group => {
    const sections: Record<string, MetadataSection> = Object.fromEntries(group.kinds.map(kind => [kind, catalog.sections[kind] ?? { state: "unknown", reason: "后端未提供此类别的读取状态" }]));
    // 当前驱动不支持的类别不生成目录节点；未知状态仍保留节点，原因只放在详情与浏览器提示里。
    const kinds = group.kinds.filter(kind => sections[kind].state !== "unsupported");
    if (kinds.length === 0) return [];
    const notice = catalogNotice(Object.fromEntries(kinds.map(kind => [kind, sections[kind]])));
    const children = catalog.objects.filter(item => kinds.includes(item.object.kind)).map(item => catalogTreeItem(baseId, item));
    return [{ id: `${baseId}/grp-${group.suffix}`, label: group.label, type: "folder", icon: group.icon,
      children, expandable: true, has_children: children.length > 0 || !!notice, expanded: false, catalogNotice: notice || undefined }];
  });
}
export function catalogReadReason(descriptor: ObjectDescriptor, readOnly = false): string | null {
  const capability = objectAction(descriptor, "read_definition", readOnly);
  return capability.level === "supported" || capability.level === "limited" ? null : capability.reason;
}
export function catalogCallReason(descriptor: ObjectDescriptor, readOnly = false): string | null {
  const capability = objectAction(descriptor, "call", readOnly);
  return capability.level === "supported" || capability.level === "limited" ? null : capability.reason;
}
export function catalogInfo(descriptor: ObjectDescriptor, connectionName: string): InfoPanelState {
  const object = descriptor.object;
  const fields = [
    { label: "连接", value: connectionName }, { label: "目录", value: object.catalog ?? "不适用" },
    { label: "模式", value: object.schema }, { label: "物理名称", value: object.name },
    { label: "类型", value: catalogKindLabel(object.kind) },
    ...(object.identity_arguments !== null ? [{ label: "身份参数", value: object.identity_arguments || "零参数" }] : []),
    ...(object.parent ? [{ label: "父对象", value: `${object.parent.schema}.${object.parent.name}` }] : []),
    ...(descriptor.backend_object_id ? [{ label: "后端对象标识", value: descriptor.backend_object_id }] : []),
    { label: "所有者", value: descriptor.owner ?? "未知" },
    { label: "启用状态", value: descriptor.enabled === null ? "未知/不适用" : descriptor.enabled ? "已启用" : "已停用" },
    ...Object.entries(descriptor.properties).map(([label, value]) => ({ label, value })),
  ];
  return { icon: catalogIcon(object.kind), title: descriptor.display_name, subtitle: "完整对象身份", fields };
}
