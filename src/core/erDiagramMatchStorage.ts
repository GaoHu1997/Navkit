/**
 * 关系图匹配记录的本地持久化 —— 按「连接 + 数据库」粒度隔离。
 *
 * 与现有 `erDiagram.loadPositions/savePositions` 保持同一范式（`navkit:` 前缀 + localStorage），
 * 不引入新的存储机制。所有读写都 try-catch，localStorage 不可用时静默降级为「不记忆」。
 */

export interface MatchRule {
  id: string;
  name: string;
  pattern: string;
  enabled: boolean;
  priority: number;
}

const PREFIX = "navkit:diagram:match";
const MAX_IDS = 2000;

function key(type: "confirms" | "ignores" | "rules" | "enabled", connectionId: string, database: string): string {
  if (type === "enabled") return `${PREFIX}:enabled`;
  return `${PREFIX}:${type}:v1:${connectionId}:${database}`;
}

function readStringArray(keyName: string): string[] {
  try {
    const raw = localStorage.getItem(keyName);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is string => typeof item === "string").slice(0, MAX_IDS);
  } catch {
    return [];
  }
}

function writeStringArray(keyName: string, values: readonly string[]): void {
  try {
    localStorage.setItem(keyName, JSON.stringify(values.slice(0, MAX_IDS)));
  } catch {
    /* 配额满或 localStorage 不可用时静默：匹配仍生效，只是不记忆 */
  }
}

export function loadMatchConfirms(connectionId: string, database: string): string[] {
  return readStringArray(key("confirms", connectionId, database));
}

export function saveMatchConfirms(ids: readonly string[], connectionId: string, database: string): void {
  writeStringArray(key("confirms", connectionId, database), ids);
}

export function loadMatchIgnores(connectionId: string, database: string): string[] {
  return readStringArray(key("ignores", connectionId, database));
}

export function saveMatchIgnores(ids: readonly string[], connectionId: string, database: string): void {
  writeStringArray(key("ignores", connectionId, database), ids);
}

/** 在已有记录上增删一条（去重、保序）。 */
export function toggleMatchId(ids: readonly string[], id: string, enabled: boolean): string[] {
  const set = new Set(ids);
  if (enabled) set.add(id);
  else set.delete(id);
  return [...set];
}

export function loadMatchRules(connectionId: string, database: string): MatchRule[] {
  try {
    const raw = localStorage.getItem(key("rules", connectionId, database));
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is MatchRule => {
      if (!item || typeof item !== "object") return false;
      const rule = item as Partial<MatchRule>;
      return typeof rule.id === "string" && typeof rule.pattern === "string" && typeof rule.name === "string";
    });
  } catch {
    return [];
  }
}

export function saveMatchRules(rules: readonly MatchRule[], connectionId: string, database: string): void {
  try {
    localStorage.setItem(key("rules", connectionId, database), JSON.stringify(rules));
  } catch {
    /* 静默降级 */
  }
}

/** 自动匹配总开关，默认开启。 */
export function isAutoMatchEnabled(): boolean {
  try {
    return localStorage.getItem(key("enabled", "", "")) !== "false";
  } catch {
    return true;
  }
}

export function setAutoMatchEnabled(enabled: boolean): void {
  try {
    localStorage.setItem(key("enabled", "", ""), String(enabled));
  } catch {
    /* 静默降级 */
  }
}
