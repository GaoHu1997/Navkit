/** PL-19对象身份与metadata状态。与object_catalog.rs序列化契约对应。 */
import type { Capability } from "./execution";

export interface ObjectParent { schema: string; kind: string; name: string }
export interface DatabaseObjectId {
  connection_id: string; catalog: string | null; schema: string; kind: string; name: string;
  identity_arguments: string | null; parent: ObjectParent | null;
}
export interface ObjectDescriptor {
  object: DatabaseObjectId; object_key: string; display_name: string;
  backend_object_id: string | null; owner: string | null; enabled: boolean | null;
  definition_readable: boolean; actions: Record<string, Capability>; properties: Record<string, string>;
}
export interface MetadataSection { state: "known" | "unknown" | "unsupported"; reason: string | null }
export interface ObjectCatalogPage {
  objects: ObjectDescriptor[]; completeness: "complete" | "partial";
  sections: Record<string, MetadataSection>; revision: string; next_cursor: string | null;
}
export interface ObjectDefinition {
  object: DatabaseObjectId; backend_object_id: string | null; definition: string;
  revision: string; properties: Record<string, string>; completeness: string;
}
export interface ObjectCatalogScope { connection_id: string; catalog: string | null; schema: string; kinds: string[] }
export interface ObjectCatalogPort { list(scope: ObjectCatalogScope, cursor: string | null): Promise<ObjectCatalogPage> }

/** null签名与空签名不等价；trigger的父表始终参与稳定键。 */
export function objectKey(object: DatabaseObjectId): string {
  return JSON.stringify([object.connection_id, object.catalog ?? null, object.schema, object.kind, object.name,
    object.identity_arguments ?? null, object.parent ? [object.parent.schema, object.parent.kind, object.parent.name] : null]);
}
export function objectLabel(object: DatabaseObjectId): string {
  if (object.identity_arguments !== null) return `${object.schema}.${object.name}(${object.identity_arguments})`;
  return `${object.schema}.${object.name}${object.parent ? ` — ${object.parent.schema}.${object.parent.name}` : ""}`;
}
export function objectAction(object: ObjectDescriptor, action: string, readOnly: boolean): Capability {
  if (readOnly && !["read", "definition", "read_definition"].includes(action)) return { level: "unsupported", reason: "只读连接禁止图形化写入和可能有副作用的例程调用" };
  return object.actions[action] ?? { level: "unsupported", reason: "驱动未提供该操作能力，没有执行SQL" };
}
export class ObjectCatalogController {
  objects: ObjectDescriptor[] = [];
  sections: Record<string, MetadataSection> = {};
  revision = "";
  nextCursor: string | null = null;
  busy = false;
  error: string | null = null;
  private generation = 0;
  private catalogResolved: boolean;
  private resolvedCatalog: string | null;
  readonly scope: ObjectCatalogScope;
  constructor(scope: ObjectCatalogScope, private readonly port: ObjectCatalogPort) {
    this.scope = Object.freeze({ ...scope, kinds: Object.freeze([...scope.kinds]) as unknown as string[] });
    this.catalogResolved = scope.catalog !== null;
    this.resolvedCatalog = scope.catalog;
  }
  async load(append = false): Promise<void> {
    if (this.busy) throw new Error("对象目录仍在读取");
    if (append && !this.nextCursor) return;
    const generation = this.generation;
    const cursor = append ? this.nextCursor : null;
    this.busy = true; this.error = null;
    try {
      const page = await this.port.list({ ...this.scope, catalog: this.catalogResolved ? this.resolvedCatalog : null, kinds: [...this.scope.kinds] }, cursor);
      if (generation !== this.generation) return;
      if (!page.revision || (append && page.revision !== this.revision)) throw new Error("OBJECT_CURSOR_STALE: 分页基线已经变化，请重新读取");
      const items = append ? [...this.objects] : [];
      const pageCatalogs = new Set(page.objects.map(item => item.object.catalog));
      let resolved = this.catalogResolved;
      let expectedCatalog = this.resolvedCatalog;
      if (!resolved) {
        if (pageCatalogs.size > 1) throw new Error("OBJECT_CATALOG_MISMATCH: 单次目录返回了多个物理catalog");
        if (pageCatalogs.size === 1) { expectedCatalog = [...pageCatalogs][0]; resolved = true; }
      }
      const seen = new Set(items.map(item => item.object_key));
      for (const item of page.objects) {
        if (item.object_key !== objectKey(item.object) || item.object.connection_id !== this.scope.connection_id || item.object.schema !== this.scope.schema || item.object.catalog !== expectedCatalog) throw new Error("OBJECT_IDENTITY_MISMATCH: 对象目录返回了错误连接或对象身份");
        if (seen.has(item.object_key)) throw new Error("OBJECT_IDENTITY_DUPLICATE: 对象分页出现重复身份");
        seen.add(item.object_key); items.push(item);
      }
      this.catalogResolved = resolved; this.resolvedCatalog = expectedCatalog;
      this.objects = items; this.sections = page.sections; this.revision = page.revision; this.nextCursor = page.next_cursor;
    } catch (error) {
      if (generation === this.generation) {
        this.error = error instanceof Error ? error.message : String(error);
        // 旧的可见对象可保留供辨认，但失败必须是unknown，不能改为known空列表。
        for (const kind of this.scope.kinds) this.sections[kind] = { state: "unknown", reason: this.error };
      }
      throw error;
    } finally { if (generation === this.generation) this.busy = false; }
  }
  dispose(): void { ++this.generation; this.busy = false; }
}

/** 编辑器打开时持有的不可变CAS基线。保存构造draft不允许重新读取baseline。 */
export interface RoutineObjectId { connection_id: string; schema: string; name: string; kind: string; identity_arguments: string }
export interface RoutineSnapshot {
  object: RoutineObjectId; definition: string; revision: string; source_fingerprint: string | null; overloads: number; completeness: string;
  backend_object_id: string | null; creation_context: Record<string, string>; security_completeness: string; replacement_reason: string;
}
export interface RoutineReplacementDraft { object: RoutineObjectId; base_revision: string; source_fingerprint: string | null; definition: string; allow_drop_recreate: boolean }
export interface RoutineReplacementPreview {
  object: RoutineObjectId; base_revision: string; backend_object_id: string | null; source_fingerprint: string | null;
  request_sql: string;
  steps: { label: string; sql: string }[]; strategy: string; executable: boolean; warnings: string[];
}
export function requireRoutineSourceFingerprint(source: unknown): string {
  if (typeof source !== "string" || !/^[0-9a-f]{64}$/.test(source)) throw new Error("ROUTINE_SOURCE_REQUIRED: 缺少有效的加载时端点指纹，请重新读取例程后合并草稿；没有发送SQL");
  return source;
}
export class RoutineEditorBaseline {
  private readonly base: RoutineSnapshot;
  constructor(snapshot: RoutineSnapshot) {
    if (!snapshot.revision || snapshot.completeness !== "complete" || snapshot.overloads !== 1) throw new Error("例程编辑必须打开精确单对象和已知基线，不能编辑整组重载");
    requireRoutineSourceFingerprint(snapshot.source_fingerprint);
    this.base = structuredClone(snapshot);
  }
  get revision(): string { return this.base.revision; }
  get sourceFingerprint(): string { return requireRoutineSourceFingerprint(this.base.source_fingerprint); }
  draft(definition: string, allowDropRecreate = false): RoutineReplacementDraft {
    return { object: { ...this.base.object }, base_revision: this.base.revision, source_fingerprint: this.sourceFingerprint, definition, allow_drop_recreate: allowDropRecreate };
  }
}
