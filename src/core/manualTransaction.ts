/** 手动事务的能力和界面状态；真实生命周期由 Rust 持有的事务连接决定。 */
import { parseDriverParams } from "./api";
import { kindMeta } from "./dbcatalog";
import type { ConnectionConfig } from "./types";

export interface ManualTransactionStatus {
  active: boolean;
  schema: string;
  owner: string;
  failed: boolean;
}

export function manualTransactionUnavailableReason(connection: ConnectionConfig | undefined): string | null {
  if (!connection) return "请先在当前页面选择一个连接";
  const kind = kindMeta(connection.kind);
  if (!kind || kind.planned || parseDriverParams(connection.params) || !["sqlite", "mysql", "postgres"].includes(kind.protocol)) {
    return "手动事务仅支持原生 SQLite / MySQL / PostgreSQL 协议连接";
  }
  return connection.read_only ? "此连接只读，不能开始写入事务" : null;
}

/** 事务期间其它自动提交的写入入口不能混入此查询事务。 */
export const MANUAL_TRANSACTION_WRITE_COMMANDS = new Set([
  "data-sync", "data-generation", "import", "tree-import", "transfer", "data-transfer",
  "new-table", "tree-new-table", "new-view", "tree-new-view", "new-function", "new-procedure",
  "new-backup", "tree-new-backup", "restore-backup", "sqlserver-backup",
  "delete-row", "grid-delete", "grid-insert", "grid-commit", "add-field", "insert-field", "delete-field",
  "primary-key", "move-up", "move-down", "empty-table", "truncate-table", "delete-table", "drop-table", "delete-object",
  "new-user", "delete-user", "edit-user", "connection-properties", "remove-connection",
]);

export function manualTransactionStatusText(status: ManualTransactionStatus | undefined, owner: string): string {
  if (!status?.active) return "自动提交";
  if (status.owner !== owner) return "此连接的事务由另一个查询页持有";
  return status.failed ? "事务执行失败：必须回滚" : "手动事务进行中：未提交";
}
