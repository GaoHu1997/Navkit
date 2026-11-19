import type { TableDesignSnapshot } from "./api";

/** 只撤销尚未发送的读取；已发送的 IPC 持有槽位直到真实响应/后端清理结束。 */
export class ErDesignReadCancelled extends Error {
  constructor() { super("表设计读取尚未发送，已撤销排队。"); }
}
export interface ErDesignReadTask {
  readonly started: boolean;
  promise: Promise<TableDesignSnapshot>;
  cancelQueued(): boolean;
}
interface Job {
  started: boolean;
  run(): Promise<TableDesignSnapshot>;
  onStart(): void;
  resolve(value: TableDesignSnapshot): void;
  reject(error: unknown): void;
}
interface Queue { active: number; jobs: Job[]; pumpScheduled: boolean }
const queues = new Map<string, Queue>();
export const ER_DESIGN_READ_CONCURRENCY = 2;

/** 按物理端点/库限制在途读取；空闲即删，不缓存 owner 绑定的设计快照。 */
export function scheduleErDesignRead(key: string, run: Job["run"], onStart: Job["onStart"]): ErDesignReadTask {
  let queue = queues.get(key);
  if (!queue) { queue = { active: 0, jobs: [], pumpScheduled: false }; queues.set(key, queue); }
  const q = queue;
  const idle = () => { if (!q.active && !q.jobs.length && queues.get(key) === q) queues.delete(key); };
  const pump = () => {
    if (q.pumpScheduled) return;
    q.pumpScheduled = true;
    void Promise.resolve().then(() => {
      q.pumpScheduled = false;
      while (q.active < ER_DESIGN_READ_CONCURRENCY && q.jobs.length) {
        const next = q.jobs.shift()!;
        next.started = true; ++q.active;
        // 标记发送和调用 run 在同一微任务，切表不会错过“未发送”撤销窗口。
        let pending: Promise<TableDesignSnapshot>;
        try { next.onStart(); pending = next.run(); }
        catch (error) { pending = Promise.reject(error); }
        void pending.then(next.resolve, next.reject).finally(() => {
          --q.active;
          if (q.jobs.length) pump(); else idle();
        });
      }
      idle();
    });
  };
  let resolve!: Job["resolve"], reject!: Job["reject"];
  const promise = new Promise<TableDesignSnapshot>((yes, no) => { resolve = yes; reject = no; });
  const job: Job = { started: false, run, onStart, resolve, reject };
  q.jobs.push(job); pump();
  return {
    get started() { return job.started; }, promise,
    cancelQueued() {
      const at = q.jobs.indexOf(job);
      if (job.started || at < 0) return false;
      q.jobs.splice(at, 1);
      reject(new ErDesignReadCancelled()); idle();
      return true;
    },
  };
}
