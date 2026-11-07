/**
 * 驱动状态会话缓存 —— 「新建连接」/「驱动管理」共享一份快照。
 *
 * - 应用启动即预热：等用户打开「新建连接」时结果已在手，全程无感；
 * - 已有快照时立即返回，后台静默再校验（stale-while-revalidate），结果变化才通知订阅者；
 * - 下载/删除驱动后调用 refreshDriverStatuses() 强制重检。
 * 后端另有指纹缓存：驱动文件未变化时重检本身也是零哈希、零探针的。
 */
import { driverApi, type DatabaseDriverStatus } from "./api";

let snapshot: DatabaseDriverStatus[] | null = null;
let inflight: Promise<DatabaseDriverStatus[]> | null = null;
const subscribers = new Set<(statuses: DatabaseDriverStatus[]) => void>();

/** 只比较影响 UI 的字段；message 等描述性文本变化不值得重建徽标。 */
function signature(list: DatabaseDriverStatus[]): string {
  return JSON.stringify(list.map((s) => [s.kind, s.version, s.status, s.can_download, s.library, s.download_path]));
}

async function fetchStatuses(): Promise<DatabaseDriverStatus[]> {
  if (inflight) return inflight;
  inflight = driverApi.statuses().then((list) => {
    inflight = null;
    const changed = !snapshot || signature(snapshot) !== signature(list);
    snapshot = list;
    if (changed) for (const notify of subscribers) notify(list);
    return list;
  }).catch((error) => { inflight = null; throw error; });
  return inflight;
}

/** 订阅状态变化（静默刷新后通知）。返回退订函数。 */
export function subscribeDriverStatuses(notify: (statuses: DatabaseDriverStatus[]) => void): () => void {
  subscribers.add(notify);
  return () => { subscribers.delete(notify); };
}

/** 当前快照；尚未预热时为 null。 */
export function currentDriverStatuses(): DatabaseDriverStatus[] | null {
  return snapshot;
}

/** 立即返回快照（无则等待首次拉取）；有快照时后台静默再校验。 */
export async function ensureDriverStatuses(): Promise<DatabaseDriverStatus[]> {
  if (snapshot) {
    void fetchStatuses().catch(() => {});
    return snapshot;
  }
  return fetchStatuses();
}

/** 强制重检（下载/删除驱动后调用）。 */
export async function refreshDriverStatuses(): Promise<DatabaseDriverStatus[]> {
  return fetchStatuses();
}

// 模块加载即预热：与后端启动预热互补，双保险；后端指纹缓存保证二者只算一次。
void fetchStatuses().catch(() => {});
