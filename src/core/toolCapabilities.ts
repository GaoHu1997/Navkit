/** 安全工具只允许已实现的原生事务协议；自定义 ODBC 不能借标称 kind 获得写入能力。 */
import { parseDriverParams } from "./api";
import { kindMeta } from "./dbcatalog";
import type { ConnectionConfig } from "./types";

export function toolConnectionError(connection: ConnectionConfig | undefined, writable = false): string | null {
  if (!connection) return "请先选择一个连接";
  const kind = kindMeta(connection.kind);
  if (!kind || kind.planned || parseDriverParams(connection.params) || !["sqlite", "mysql", "postgres"].includes(kind.protocol)) {
    return "此工具仅支持原生 SQLite / MySQL / PostgreSQL 协议连接，不支持自定义 ODBC 或尚未实现的协议";
  }
  if (writable && connection.read_only) return "此连接只读，不能写入数据或修改结构";
  return null;
}

/**
 * 数据同步的连接白名单：比通用写入工具更宽 —— SQL Server / ClickHouse /
 * ODBC 关系型（Oracle、达梦、DB2、Snowflake、Trino 等）有真实的元数据与
 * SQL 执行通道，可作源与目标；目标通道不支持事务（对话框会提示）。
 * Redis、Elasticsearch、Kafka、Nacos 等非关系型数据模型不适用，仍被排除。
 */
export function syncConnectionError(connection: ConnectionConfig | undefined, writable = false): string | null {
  if (!connection) return "请先选择一个连接";
  const kind = kindMeta(connection.kind);
  if (!kind || kind.planned || parseDriverParams(connection.params) || !["sqlite", "mysql", "postgres", "mssql", "clickhouse", "odbc"].includes(kind.protocol)) {
    return "数据同步支持 SQLite、MySQL/PostgreSQL 协议族、SQL Server、ClickHouse 与 ODBC 关系型数据源（Oracle/达梦/DB2/Snowflake/Trino 等）；Redis、Elasticsearch、Kafka 等非关系型数据源不支持";
  }
  if (writable && connection.read_only) return "此连接只读，不能作为同步目标";
  return null;
}
