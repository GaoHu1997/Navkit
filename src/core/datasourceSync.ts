import type { ConnectionConfig } from "./types";

export interface DatasourceSyncConfig {
  url: string; user: string; auto_every: number; remember_password: boolean;
  password_set: boolean; passphrase_set: boolean; headers_set: boolean; last_sync: string | null;
}
export interface DatasourceSyncInput {
  url: string; user: string; auto_every: number; remember_password: boolean;
  password?: string; passphrase?: string; headers?: string;
}
export type SyncChoice = "local" | "remote";
export interface DatasourceSyncPreview {
  plan_id: string;
  conflicts: { key: string; label: string; local_summary: string; remote_summary: string }[];
  summary: { added: number; updated: number; deleted: number; organization: boolean };
  warnings: string[]; remote_exists: boolean; has_changes: boolean; requires_confirmation: boolean;
}
export interface DatasourceSyncResult {
  connections: ConnectionConfig[]; groups: string[]; changed_ids: string[]; deleted_ids: string[];
  warnings: string[]; last_sync: string; recovery_required?: boolean;
}
export interface DatasourceSyncSource {
  getConfig(): Promise<DatasourceSyncConfig>;
  setConfig(config: DatasourceSyncInput): Promise<DatasourceSyncConfig>;
  test(): Promise<string>;
  preview(): Promise<DatasourceSyncPreview>;
  commit(planId: string, choices: Record<string, SyncChoice>, automatic: boolean): Promise<DatasourceSyncResult>;
  cancel(planId: string): Promise<void>;
}
interface SyncHost {
  applied(result: DatasourceSyncResult): void;
  ready(): boolean;
  status(message: string, error: boolean): void;
}
interface SyncClock {
  set(callback: () => void, milliseconds: number): unknown;
  clear(id: unknown): void;
}
const browserClock: SyncClock = {
  set: (cb, ms) => setTimeout(cb, ms),
  clear: id => clearTimeout(id as ReturnType<typeof setTimeout>),
};

/** 单实例调度器；手动预览、配置与自动同步共享互斥，秘密不在控制器中保存。 */
export class DatasourceSync {
  config: DatasourceSyncConfig | null = null;
  busy = false;
  private plan: DatasourceSyncPreview | null = null;
  private timer: unknown = null;
  private pauseCount = 0;
  private stopped = true;

  constructor(private source: DatasourceSyncSource, private host: SyncHost, private clock: SyncClock = browserClock) {}

  async start(): Promise<void> {
    this.stopped = false;
    try { this.config = await this.source.getConfig(); }
    catch { this.host.status("数据源同步配置无法读取，请打开数据源同步检查", true); }
    this.schedule();
  }
  stop(): void { this.stopped = true; this.clearTimer(); }
  pause(): () => void {
    this.pauseCount++; this.clearTimer();
    let released = false;
    return () => { if (!released) { released = true; this.pauseCount--; this.schedule(); } };
  }
  private clearTimer(): void {
    if (this.timer !== null) this.clock.clear(this.timer);
    this.timer = null;
  }
  private schedule(): void {
    this.clearTimer();
    const minutes = this.config?.auto_every ?? -1;
    if (this.stopped || this.pauseCount || this.busy || ![3, 15, 30, 60].includes(minutes)) return;
    this.timer = this.clock.set(() => { this.timer = null; void this.automatic(); }, minutes * 60_000);
  }
  private async exclusive<T>(work: () => Promise<T>): Promise<T> {
    if (this.busy) throw new Error("数据源同步正在执行，请稍后再试");
    this.busy = true; this.clearTimer();
    try { return await work(); }
    finally { this.busy = false; this.schedule(); }
  }
  private async discard(): Promise<void> {
    const old = this.plan; this.plan = null;
    if (old) await this.source.cancel(old.plan_id);
  }
  async getConfig(): Promise<DatasourceSyncConfig> {
    return this.exclusive(async () => this.config = await this.source.getConfig());
  }
  async save(config: DatasourceSyncInput): Promise<DatasourceSyncConfig> {
    return this.exclusive(async () => {
      await this.discard();
      return this.config = await this.source.setConfig(config);
    });
  }
  test(): Promise<string> { return this.exclusive(() => this.source.test()); }
  preview(): Promise<DatasourceSyncPreview> {
    return this.exclusive(async () => {
      await this.discard();
      return this.plan = await this.source.preview();
    });
  }
  async cancel(): Promise<void> { return this.exclusive(() => this.discard()); }
  commit(id: string, choices: Record<string, SyncChoice>): Promise<DatasourceSyncResult> {
    return this.exclusive(async () => {
      if (!this.plan || this.plan.plan_id !== id) throw new Error("预览已失效，请重新预览");
      if (!this.host.ready()) throw new Error("查询、任务或未提交的编辑尚未完成，请先处理后再同步");
      const keys = this.plan.conflicts.map(c => c.key);
      if (keys.some(key => !["local", "remote"].includes(choices[key])) || Object.keys(choices).some(key => !keys.includes(key))) {
        throw new Error("请明确选择每一项冲突的本机或远端版本");
      }
      this.plan = null; // 后端提交消耗预览，失败也不能重用。
      const result = await this.source.commit(id, choices, false);
      this.applied(result);
      return result;
    });
  }
  private applied(result: DatasourceSyncResult): void {
    if (this.config && !result.recovery_required) this.config.last_sync = result.last_sync;
    this.host.applied(result);
    if (result.recovery_required) this.host.status("数据源已应用，但同步尚未完成；请手动预览恢复", true);
    else this.host.status("数据源同步完成", false);
  }
  async automatic(): Promise<void> {
    if (this.stopped || this.pauseCount || this.busy || this.plan) return;
    if (!this.host.ready()) { this.host.status("数据源自动同步已延后：请先完成查询、事务或编辑", false); this.schedule(); return; }
    try {
      await this.exclusive(async () => {
        const plan = this.plan = await this.source.preview();
        if (plan.requires_confirmation || plan.conflicts.length) {
          await this.discard();
          this.host.status("数据源同步需要手动确认，请打开数据源同步并重新预览", false);
          return;
        }
        if (!this.host.ready()) { await this.discard(); return; }
        this.plan = null;
        this.applied(await this.source.commit(plan.plan_id, {}, true));
      });
    } catch {
      // 不传播可能带服务端文本的错误，也不后台弹窗、循环重试或静默选冲突。
      if (this.plan) { try { await this.discard(); } catch { /* 后端 TTL 清理 */ } }
      this.host.status("数据源自动同步未完成，请打开数据源同步检查配置或重新预览", true);
    }
  }
}

/** 与后端 requires_reopen 保持一致；纯组织调整不清理运行时连接。 */
export function invalidatedDatasourceIds(before: ConnectionConfig[], after: ConnectionConfig[]): Set<string> {
  const fields: (keyof ConnectionConfig)[] = ["kind", "host", "port", "user", "password", "database", "file", "params", "read_only", "keep_alive"];
  const next = new Map(after.map(c => [c.id, c]));
  return new Set(before.filter(old => {
    const current = next.get(old.id);
    return !current || fields.some(key => old[key] !== current[key]);
  }).map(c => c.id));
}
