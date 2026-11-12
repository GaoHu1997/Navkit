import type { Favorite } from "./types";

/** 已保存查询必须同时有连接和数据库；旧记录仅可作为未保存 SQL 恢复。 */
export function hasSavedQueryScope(favorite: Favorite): favorite is Favorite & { connection_id: string; schema: string } {
  return !!favorite.connection_id?.trim() && !!favorite.schema?.trim();
}

export function savedQueriesFor(items: Favorite[], connectionId: string, schema: string): Favorite[] {
  return items.filter(f => hasSavedQueryScope(f) && f.connection_id === connectionId && f.schema === schema);
}

export function savedQueryConflict(items: Favorite[], candidate: Favorite): boolean {
  return items.some(f => f.id !== candidate.id && f.name === candidate.name &&
    (f.connection_id || null) === (candidate.connection_id || null) && (f.schema || null) === (candidate.schema || null));
}

/** 只保存到明确的数据库；更新只以稳定 ID 为依据，另存为才改变保存位置。 */
export function queryFavorite(items: Favorite[], input: { id?: string; name: string; sql: string; connectionId: string; schema: string }, now = Date.now()): Favorite {
  if (!input.connectionId.trim() || !input.schema.trim()) throw new Error("保存查询前请选择连接和数据库");
  const previous = input.id ? items.find(f => f.id === input.id) : undefined;
  if (input.id && !previous) throw new Error("保存的查询已被删除，请另存为");
  if (previous && !hasSavedQueryScope(previous)) throw new Error("此记录不是已保存查询，请作为未保存 SQL 重新打开后保存");
  if (previous && (previous.connection_id !== input.connectionId || previous.schema !== input.schema)) throw new Error("查询归属已变化，请重新打开或另存为");
  const favorite: Favorite = {
    id: previous?.id ?? `f_${now.toString(36)}_${crypto.randomUUID()}`,
    name: input.name.trim(), sql: input.sql,
    connection_id: input.connectionId,
    schema: input.schema,
    timestamp: now,
  };
  if (!favorite.name) throw new Error("请输入查询名称");
  if (savedQueryConflict(items, favorite)) throw new Error("此连接和数据库中已有同名查询，请使用其它名称");
  return favorite;
}
