/** 软件更新偏好与下载协调。设计见 docs/design/DESIGN.md §6.22。
 * 自动安装只在 main.ts 构造工作区之前执行，后台下载永不重启正在使用的程序。
 */
import { api } from "./api";
import type { UpdateManifest } from "./types";

export const UPDATE_PREFERENCES_KEY = "navkit.updateprefs.v1";
export interface PendingUpdate { version: string; file: string; sha256: string }
export interface UpdatePreferences {
  autoUpdate: boolean;
  skippedVersion: string;
  pendingInstall: PendingUpdate | null;
}
const defaults = (): UpdatePreferences => ({ autoUpdate: false, skippedVersion: "", pendingInstall: null });
const listeners = new Set<() => void>();
let sessionPreferences = defaults();
let useSessionPreferences = false;
let downloading: { version: string; sha256: string; promise: Promise<string> } | null = null;
const normalizedVersion = (version: string): string => version.trim().replace(/^v/i, "");

export function readUpdatePreferences(): UpdatePreferences {
  if (useSessionPreferences) return { ...sessionPreferences, autoUpdate: false, pendingInstall: null };
  try {
    const raw = localStorage.getItem(UPDATE_PREFERENCES_KEY);
    if (!raw) return defaults();
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) return defaults();
    const stored = value as Partial<UpdatePreferences>;
    const pending = stored.pendingInstall;
    return {
      autoUpdate: stored.autoUpdate === true,
      skippedVersion: typeof stored.skippedVersion === "string" ? stored.skippedVersion : "",
      pendingInstall: pending && typeof pending.version === "string" && pending.version.trim() &&
        typeof pending.file === "string" && pending.file.trim() &&
        typeof pending.sha256 === "string" && /^[a-f\d]{64}$/i.test(pending.sha256)
        ? { version: pending.version, file: pending.file, sha256: pending.sha256.toLowerCase() } : null,
    };
  } catch {
    // 存储不可用时跳过在本会话仍生效，但不得假装已持久化自动安装授权。
    return { ...sessionPreferences, autoUpdate: false, pendingInstall: null };
  }
}

/** 返回是否成功持久化；失败时不保留自动安装授权。 */
export function saveUpdatePreferences(patch: Partial<UpdatePreferences>): boolean {
  const next = { ...readUpdatePreferences(), ...patch };
  let saved = false;
  try {
    localStorage.setItem(UPDATE_PREFERENCES_KEY, JSON.stringify(next));
    saved = true;
  } catch { /* 存储禁用/配额不足：调用方给出就地提示。 */ }
  useSessionPreferences = !saved;
  sessionPreferences = saved ? next : { ...next, autoUpdate: false, pendingInstall: null };
  for (const listener of [...listeners]) listener();
  return saved;
}

export function subscribeUpdatePreferences(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function isUpdateSkipped(version: string): boolean {
  const skipped = readUpdatePreferences().skippedVersion;
  return Boolean(skipped && normalizedVersion(skipped) === normalizedVersion(version));
}

export function skipUpdateVersion(version: string): boolean {
  const pending = readUpdatePreferences().pendingInstall;
  return saveUpdatePreferences({
    skippedVersion: normalizedVersion(version),
    ...(pending && normalizedVersion(pending.version) === normalizedVersion(version) ? { pendingInstall: null } : {}),
  });
}

/** 弹窗和徽标共享同一下载，成功回执才可生成下次启动安装记录。 */
export function downloadUpdate(manifest: UpdateManifest): Promise<string> {
  const sha256 = manifest.sha256.trim().toLowerCase();
  if (!manifest.version.trim() || !/^[a-f\d]{64}$/.test(sha256)) {
    return Promise.reject(new Error("更新清单缺少有效版本或 SHA-256，请重新检查更新"));
  }
  if (downloading) {
    if (normalizedVersion(downloading.version) !== normalizedVersion(manifest.version) || downloading.sha256 !== sha256) {
      return Promise.reject(new Error("另一个版本或更新包正在下载，请等待下载完成"));
    }
    return downloading.promise;
  }
  const promise = api.updateDownload(manifest.version, sha256).then(file => {
    saveUpdatePreferences({ pendingInstall: { version: manifest.version, file, sha256 } });
    return file;
  }).finally(() => { downloading = null; });
  downloading = { version: manifest.version, sha256, promise };
  return promise;
}

export interface StartupUpdateResult { restarting: boolean; error?: string }
/** 必须在 App 创建前调用；不检查网络、不下载，只安装先前成功下载的同一路径包。 */
export async function installPendingUpdateAtStartup(): Promise<StartupUpdateResult> {
  const preferences = readUpdatePreferences();
  const pending = preferences.pendingInstall;
  if (!preferences.autoUpdate || !pending || isUpdateSkipped(pending.version)) return { restarting: false };
  try {
    const status = await api.updateStatus();
    if (status.phase !== "ready" || status.staged_file !== pending.file) return { restarting: false };
    const current = readUpdatePreferences();
    if (!current.autoUpdate || current.pendingInstall?.version !== pending.version ||
        current.pendingInstall?.file !== pending.file || current.pendingInstall?.sha256 !== pending.sha256 ||
        isUpdateSkipped(pending.version)) return { restarting: false };
    // 先消费记录，避免安装失败后每次启动都自动重试。失败仍可从更新弹窗手动重试。
    if (!saveUpdatePreferences({ pendingInstall: null })) {
      return { restarting: false, error: "无法保存更新状态，已取消自动安装。" };
    }
    await api.updateApply(pending.sha256);
    return { restarting: true };
  } catch (error) {
    return { restarting: false, error: `自动安装失败，已继续启动当前版本：${String(error)}` };
  }
}
