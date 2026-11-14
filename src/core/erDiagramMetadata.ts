import type { DiagramMetadata } from "./api";
import type { ConnectionConfig } from "./types";

// Adapted from DBX diagramMetadataCache: immutable snapshots, bounded TTL,
// coalesced loads and identity-checked commits after refresh/invalidation.
export const DIAGRAM_METADATA_CACHE_TTL_MS = 5 * 60 * 1000;
const MAX_ENTRIES = 16;
interface CacheEntry { expiresAt: number; metadata: DiagramMetadata }
interface PendingLoad { force: boolean; promise: Promise<DiagramMetadata> }
const entries = new Map<string, CacheEntry>();
const inFlight = new Map<string, PendingLoad>();
const clone = (metadata: DiagramMetadata): DiagramMetadata => JSON.parse(JSON.stringify(metadata));

function cacheKey(connection: ConnectionConfig, namespace: string): string {
  // Connection edits must never reuse another physical endpoint's metadata.
  // Password is the existing encrypted configuration, never persisted/logged here.
  return JSON.stringify([1, connection.id, connection.kind, connection.host, connection.port,
    connection.user, connection.password, connection.database, connection.file, connection.params, namespace]);
}

export async function loadDiagramMetadataCached(
  connection: ConnectionConfig,
  namespace: string,
  loader: () => Promise<DiagramMetadata>,
  force = false,
): Promise<DiagramMetadata> {
  const key = cacheKey(connection, namespace);
  const existing = inFlight.get(key);
  // A refresh supersedes the snapshot: callers arriving during it join the refresh.
  if (existing && (!force || existing.force)) return clone(await existing.promise);
  const cached = entries.get(key);
  if (!force && cached && Date.now() < cached.expiresAt) {
    entries.delete(key);
    entries.set(key, cached);
    return clone(cached.metadata);
  }
  if (cached && Date.now() >= cached.expiresAt) entries.delete(key);
  const pending: PendingLoad = { force, promise: Promise.resolve({ tables: [], failures: [], limitations: [] }) };
  pending.promise = Promise.resolve().then(loader).then((metadata) => {
    // A partial read must not replace a complete, previously successful snapshot.
    if (inFlight.get(key) === pending && metadata.failures.length === 0) {
      const now = Date.now();
      for (const [expiredKey, entry] of entries) if (now >= entry.expiresAt) entries.delete(expiredKey);
      entries.delete(key);
      entries.set(key, { expiresAt: now + DIAGRAM_METADATA_CACHE_TTL_MS, metadata: clone(metadata) });
      while (entries.size > MAX_ENTRIES) entries.delete(entries.keys().next().value!);
    }
    return metadata;
  }).finally(() => {
    if (inFlight.get(key) === pending) inFlight.delete(key);
  });
  inFlight.set(key, pending);
  return clone(await pending.promise);
}

export function invalidateDiagramMetadata(connection: ConnectionConfig, namespace: string): void {
  const key = cacheKey(connection, namespace);
  entries.delete(key);
  inFlight.delete(key);
}
