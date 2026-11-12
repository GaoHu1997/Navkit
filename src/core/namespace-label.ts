import { DB_KINDS } from "./dbcatalog";
import type { ConnectionConfig } from "./types";

type NamespaceConnection = Pick<ConnectionConfig, "kind"> | undefined;

/** This describes the listDatabases namespace, not the configured physical database. */
export function namespaceType(connection: NamespaceConnection): "database" | "schema" {
  return namespaceProtocol(connection) === "postgres" ? "schema" : "database";
}

function namespaceProtocol(connection: NamespaceConnection): string | undefined {
  return DB_KINDS.find(kind => kind.id === connection?.kind)?.protocol
    ?? (connection?.kind === "mssql" ? "mssql" : undefined);
}

export function namespaceName(connection: NamespaceConnection): string {
  const protocol = namespaceProtocol(connection);
  if (protocol === "postgres") return "模式";
  if (["mysql", "mssql", "clickhouse", "sqlite"].includes(protocol ?? "")) return "数据库";
  return "命名空间";
}

/** Full form label retained for existing import/export and scheduler callers. */
export function namespaceLabel(connection: NamespaceConnection): string {
  const protocol = namespaceProtocol(connection);
  if (protocol === "postgres") return "模式 (schema)";
  if (protocol === "sqlite") return "数据库 (main)";
  return namespaceName(connection);
}
