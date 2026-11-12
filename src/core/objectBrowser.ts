import type { ConnectionConfig, Favorite, ObjectInfo, ObjectType } from "./types";
import type { TabToolItem } from "../ui/TabToolbar";
import type { ObjectDescriptor } from "./object-catalog";
import { catalogReadReason, catalogCallReason, catalogKindLabel } from "./object-navigation";
import { kindMeta } from "./dbcatalog";
import { formatBytes } from "./format-size";
import { parseDriverParams } from "./api";

export type BrowserObject = Pick<ObjectInfo, "name" | "type"> & Partial<ObjectInfo> & { favorite?: Favorite; descriptor?: ObjectDescriptor };
export const objectLabel = (type: string): string => ({ table: "表", view: "视图", function: "函数", procedure: "存储过程", query: "查询", trigger: "触发器", event: "事件", sequence: "序列", materialized_view: "物化视图" }[type] ?? "对象");

export function objectProtocol(cfg?: ConnectionConfig): string {
  return cfg && !parseDriverParams(cfg.params) && !kindMeta(cfg.kind)?.planned ? kindMeta(cfg.kind)?.protocol ?? "" : "";
}
export function objectCapability(cfg: ConnectionConfig | undefined, type: string, action: "create" | "design" | "delete" | "update"): string | null {
  if (type === "query") return null;
  if (!cfg) return "连接不存在";
  if (!["table", "view", "function", "procedure"].includes(type)) return "当前对象类型尚未接入受控结构修改";
  const protocol = objectProtocol(cfg);
  if (action !== "design" && cfg.read_only) return "此连接只读，不能修改数据库对象";
  if (!["sqlite", "mysql", "postgres"].includes(protocol)) return "此协议尚未可靠支持此对象操作";
  if ((type === "function" || type === "procedure") && protocol === "sqlite") return "SQLite 不支持 SQL 函数 / 存储过程设计";
  if ((type === "function" || type === "procedure") && protocol === "postgres" && action === "delete") return "PostgreSQL 例程删除需要精确参数签名；当前列表未提供，已禁用";
  if ((type === "function" || type === "procedure") && protocol === "mysql" && action === "update") return "MySQL 不支持原地替换例程正文；当前设计器支持查看和新建，已有例程保存已禁用";
  return null;
}
export function qualifiedObject(cfg: ConnectionConfig, schema: string, name: string): string {
  const tick = objectProtocol(cfg) === "mysql";
  const q = (s: string) => tick ? `\`${s.replace(/`/g, "``")}\`` : `"${s.replace(/"/g, '""')}"`;
  return schema ? `${q(schema)}.${q(name)}` : q(name);
}
export function newObjectSql(cfg: ConnectionConfig, schema: string, name: string, type: string): string {
  const reason = objectCapability(cfg, type, "create");
  if (reason) throw new Error(reason);
  const full = qualifiedObject(cfg, schema, name), protocol = objectProtocol(cfg);
  if (type === "table") return `CREATE TABLE ${full} (id ${protocol === "sqlite" ? "INTEGER PRIMARY KEY AUTOINCREMENT" : protocol === "postgres" ? "BIGSERIAL PRIMARY KEY" : "BIGINT PRIMARY KEY AUTO_INCREMENT"});`;
  if (type === "view") return `CREATE VIEW ${full} AS\nSELECT 1 AS value;`;
  if (type === "procedure") return protocol === "postgres"
    ? `CREATE PROCEDURE ${full}()\nLANGUAGE SQL\nAS $$ SELECT 1; $$;`
    : `CREATE PROCEDURE ${full}()\nBEGIN\n  SELECT 1;\nEND;`;
  return protocol === "postgres"
    ? `CREATE FUNCTION ${full}()\nRETURNS INTEGER\nLANGUAGE SQL\nAS $$ SELECT 1; $$;`
    : `CREATE FUNCTION ${full}()\nRETURNS INTEGER\nDETERMINISTIC\nRETURN 1;`;
}
export function dropObjectSql(cfg: ConnectionConfig, schema: string, object: BrowserObject): string {
  const reason = object.descriptor ? object.descriptor.actions.drop?.reason ?? "当前对象尚无受控删除计划" : objectCapability(cfg, object.type, "delete");
  if (reason) throw new Error(reason);
  const keyword = { table: "TABLE", view: "VIEW", function: "FUNCTION", procedure: "PROCEDURE" }[object.type];
  if (!keyword) throw new Error("此类别不是可删除的数据库对象");
  return `DROP ${keyword} ${qualifiedObject(cfg, schema, object.name)}`;
}
export function objectTools(category: ObjectType, selected: BrowserObject | null, cfg?: ConnectionConfig): TabToolItem[] {
  const type = selected?.type ?? (category === "folder" ? "table" : category), label = objectLabel(type);
  const make = (id: string, text: string, icon: string, reason: string | null = null): TabToolItem => ({ id, label: text, icon, disabled: !!reason, disabledReason: reason ?? undefined });
  const selection = selected ? null : "请先选择一个对象";
  if (selected?.descriptor) {
    const descriptor = selected.descriptor, readReason = catalogReadReason(descriptor, cfg?.read_only);
    const actions: TabToolItem[] = [
      make("object-new", `新建${objectLabel(category === "folder" ? "table" : category)}`, "new_table", objectCapability(cfg, category === "folder" ? "table" : category, "create")),
      ...(category === "function" ? [make("object-new-procedure", "新建存储过程", "new_table", objectCapability(cfg, "procedure", "create"))] : []),
      make("object-open", "打开定义", "open_table", readReason),
      make("object-design", ["function", "procedure"].includes(type) ? `设计${label}` : "查看定义", "design_table", readReason),
    ];
    if (["function", "procedure"].includes(type)) actions.push(make("object-call", "调用例程…", "run", catalogCallReason(descriptor, cfg?.read_only)));
    actions.push(
      make("object-delete", `删除${label}`, "delete_table", cfg?.read_only ? "此连接只读，不能删除对象" : descriptor.actions.drop && descriptor.actions.drop.level !== "unsupported" && objectProtocol(cfg) === "sqlite" && type === "trigger" ? null : descriptor.actions.drop?.reason ?? "此对象的受控删除计划尚未接入"),
      // 搜索图标点击后就地展开筛选框；刷新只保留图标。
      { id: "object-search", label: "搜索对象", icon: "btn_search", iconOnly: true, alignRight: true },
      { id: "refresh", label: "刷新", icon: "dbn_refresh", iconOnly: true },
    );
    return actions;
  }
  const tools = [make("object-new", `新建${objectLabel(category === "folder" ? "table" : category)}`, "new_table", objectCapability(cfg, category === "folder" ? "table" : category, "create"))];
  if (category === "function") tools.push(make("object-new-procedure", "新建存储过程", "new_table", objectCapability(cfg, "procedure", "create")));
  tools.push(make("object-open", type === "function" || type === "procedure" ? "打开定义" : `打开${label}`, "open_table", selection || (["function", "procedure"].includes(type) ? objectCapability(cfg, type, "design") : null)));
  if (category === "query") {
    tools.push(make("object-rename", "重命名", "design_table", selection));
  } else tools.push(make("object-design", `设计${label}`, "design_table", selection || objectCapability(cfg, type, "design")));
  tools.push(make("object-delete", `删除${label}`, "delete_table", selection || objectCapability(cfg, type, "delete")));
  if (category === "table" || category === "folder") tools.push(make("object-import", "导入", "import_wizard", selection || (type !== "table" ? "只有表支持导入" : cfg?.read_only ? "此连接只读" : null)));
  if (["table", "view", "folder"].includes(category)) tools.push(make("object-export", "导出", "export_wizard", !["table", "view"].includes(type) ? "请选择表或视图" : null));
  tools.push(
    { id: "object-search", label: "搜索对象", icon: "btn_search", iconOnly: true, alignRight: true },
    { id: "refresh", label: "刷新", icon: "dbn_refresh", iconOnly: true },
  );
  return tools;
}
export function objectGrid(category: ObjectType, objects: BrowserObject[], showAutoIncrement = false) {
  if (["function", "procedure", "trigger", "event", "sequence", "materialized_view"].includes(category) || objects.some(object => object.descriptor)) {
    const names = ["名称", "类型", "模式", "身份参数 / 父对象", "状态"];
    return { columns: names.map(name => ({ name, type: "text", nullable: false, primary_key: false, auto_increment: false })), rows: objects.map(object => {
      const descriptor = object.descriptor, id = descriptor?.object;
      return [descriptor?.display_name ?? object.name, catalogKindLabel(object.type), id?.schema ?? "—", id?.identity_arguments !== null && id?.identity_arguments !== undefined ? id.identity_arguments || "零参数" : id?.parent ? `${id.parent.schema}.${id.parent.name}` : "—", descriptor?.enabled === null || descriptor?.enabled === undefined ? "未知/不适用" : descriptor.enabled ? "已启用" : "已停用"];
    }) };
  }
  const names = category === "query" ? ["名称", "修改时间", "SQL 摘要"] : category === "function" ? ["名称", "类型"] : category === "view" ? ["名称", "修改时间", "注释"] : ["名称", "修改时间", "数据大小", "引擎", "行数", "注释"];
  const rows = objects.map(o => category === "query" ? [o.name, o.favorite?.timestamp ? new Date(o.favorite.timestamp).toLocaleString() : "—", o.favorite?.sql.replace(/\s+/g, " ").slice(0, 180) ?? "—"]
    : category === "function" ? [o.name, objectLabel(o.type)]
    : category === "view" ? [o.name, o.updateTime || "—", o.comment || "—"]
    : [o.name, o.updateTime || "—", o.type === "table" && (o.dataLength ?? 0) > 0 ? formatBytes(o.dataLength!) : "—", o.type === "table" ? o.engine || "—" : "—", o.type === "table" && Number.isFinite(o.rows) && o.rows! >= 0 ? String(o.rows) : "—", o.comment || "—"]);
  if (showAutoIncrement && ["table", "folder"].includes(category)) {
    names.push("自增值");
    rows.forEach((row, index) => row.push(objects[index].type === "table" && Number.isFinite(objects[index].autoIncrement) ? String(objects[index].autoIncrement) : "—"));
  }
  return { columns: names.map(name => ({ name, type: "text", nullable: false, primary_key: false, auto_increment: false })), rows };
}
