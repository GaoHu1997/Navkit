import type { ConnectionHealthView } from "./api";

export interface ConnectionHealthPort {
  view(id: string): Promise<ConnectionHealthView>;
  check(id: string): Promise<ConnectionHealthView>;
  offline(id: string, message: string): void;
  schedule(callback: () => void, delayMs: number): number;
  cancel(timer: number): void;
}
interface Watch { generation: string | null; timer: number | null }

/** Backend owns cadence/leases. This client never overlaps probes or carries a watch across generations. */
export class ConnectionHealthMonitor {
  private readonly watches = new Map<string, Watch>();
  constructor(private readonly port: ConnectionHealthPort) {}

  start(id: string): void {
    if (!id || this.watches.has(id)) return;
    const watch: Watch = { generation: null, timer: null };
    this.watches.set(id, watch);
    void this.read(id, watch, false);
  }
  stop(id: string): void {
    const watch = this.watches.get(id);
    if (watch?.timer !== null && watch?.timer !== undefined) this.port.cancel(watch.timer);
    this.watches.delete(id);
  }
  stopAll(): void { for (const id of [...this.watches.keys()]) this.stop(id); }

  private current(id: string, watch: Watch): boolean { return this.watches.get(id) === watch; }
  private async read(id: string, watch: Watch, probe: boolean): Promise<void> {
    try {
      const health = await (probe ? this.port.check(id) : this.port.view(id));
      if (!this.current(id, watch)) return;
      if (!health || health.connection_id !== id || typeof health.generation !== "string" || !health.generation ||
        typeof health.enabled !== "boolean" || typeof health.resources_pending !== "boolean" ||
        !Number.isFinite(health.interval_secs) || health.interval_secs < 10 || health.interval_secs > 3600 ||
        !Number.isFinite(health.timeout_secs) || health.timeout_secs < 1 || health.timeout_secs > 60 || health.timeout_secs > health.interval_secs ||
        !["unknown", "healthy", "suspect", "offline"].includes(health.state)) {
        this.stop(id); return;
      }
      if (watch.generation !== null && watch.generation !== health.generation) { this.stop(id); return; }
      watch.generation = health.generation;
      if (!health.enabled || health.probe_skipped_reason === "unsupported" || health.probe_skipped_reason === "disabled") { this.stop(id); return; }
      if (health.state === "offline") {
        this.stop(id); this.port.offline(id, health.last_error ?? "保活探测失败"); return;
      }
      const pending = health.resources_pending;
      const due = health.next_due_in_ms;
      if (!pending && (typeof due !== "number" || !Number.isFinite(due) || due < 0 || due > 3_600_000)) { this.stop(id); return; }
      // Pending workers are observed without scheduling another SQL probe.
      const delay = pending ? Math.max(1000, health.timeout_secs * 1000) : Math.max(1000, due!);
      watch.timer = this.port.schedule(() => {
        if (!this.current(id, watch)) return;
        watch.timer = null;
        void this.read(id, watch, !pending);
      }, delay);
    } catch { if (this.current(id, watch)) this.stop(id); }
  }
}
