/**
 * 后台任务中心 —— 导入 / 导出 / 运行 SQL / 转储 / 备份 / 数据生成 / 数据同步 /
 * 批量导出 / 数据字典导出等耗时操作的统一任务注册表。
 *
 * 职责：
 *  · 任务登记：开始时注册，结束时更新状态（成功 / 失败 / 取消），保留最近记录
 *  · 进度：可带 done/total/message；无进度用 indeterminate
 *  · 面板联动：订阅式刷新；标题栏任务图标显示运行中数量徽标
 *  · 点击任务：优先调用任务自带的 opener（重新打开弹窗），否则打开任务详情弹窗
 *
 * 长任务本体是后端 invoke 的 floating promise；对话框只是展示层，
 * 关闭对话框（「后台执行」）不影响执行，任务结束后在面板里可以看到结果。
 */

export type TaskKind =
  | "import"
  | "export"
  | "sql-file"
  | "dump"
  | "backup"
  | "restore"
  | "datagen"
  | "sync"
  | "transfer"
  | "batch-export"
  | "dictionary"
  | "update"
  | "other";

export type TaskStatus = "running" | "success" | "error" | "cancelled";

export interface TaskProgress {
  /** 已完成量（无精确进度时缺省）。 */
  done?: number;
  /** 总量。 */
  total?: number;
  /** 阶段 / 消息。 */
  message?: string;
}

export interface BackgroundTask {
  id: string;
  kind: TaskKind;
  title: string;
  status: TaskStatus;
  createdAt: number;
  endedAt?: number;
  progress?: TaskProgress;
  /** 结果说明或错误信息。 */
  detail?: string;
  /** 产物文件路径（导出 / 备份 / 字典等），供「打开」按钮使用。 */
  resultPath?: string;
}

export interface TaskHandle {
  /** 更新进度。 */
  update(progress: TaskProgress): void;
  /** 成功完成。 */
  finish(detail?: string, resultPath?: string): void;
  /** 失败。 */
  fail(detail: string): void;
  /** 标记取消（调用方自行执行取消逻辑）。 */
  cancel(detail?: string): void;
  /** 注册「点击任务重新打开弹窗」动作。 */
  setOpener(opener: () => void): void;
}

export interface TaskCenterSnapshot {
  tasks: BackgroundTask[];
  running: number;
}

const TASK_LIST_LIMIT = 50;

type Listener = () => void;

/** 任务类型的中文名。 */
export const TASK_KIND_LABEL: Record<TaskKind, string> = {
  import: "导入",
  export: "导出",
  "sql-file": "运行 SQL 文件",
  dump: "SQL 转储",
  backup: "备份",
  restore: "还原备份",
  datagen: "数据生成",
  sync: "数据同步",
  transfer: "数据传输",
  "batch-export": "批量导出",
  dictionary: "数据字典导出",
  update: "软件更新",
  other: "任务",
};

let taskSequence = 0;

/** 任务中心（单例；全应用共享一份任务列表）。 */
export class TaskCenter {
  private tasks: BackgroundTask[] = [];
  private listeners = new Set<Listener>();
  /** 点击动作注册表（不进任务记录，避免序列化负担）。 */
  private openers = new Map<string, () => void>();

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private emit(): void {
    for (const listener of [...this.listeners]) listener();
  }

  snapshot(): TaskCenterSnapshot {
    return {
      tasks: [...this.tasks],
      running: this.tasks.filter((t) => t.status === "running").length,
    };
  }

  runningCount(): number {
    return this.tasks.reduce((n, t) => n + (t.status === "running" ? 1 : 0), 0);
  }

  /** 注册任务并返回句柄。 */
  register(kind: TaskKind, title: string): TaskHandle {
    const task: BackgroundTask = {
      id: `task-${Date.now().toString(36)}-${++taskSequence}`,
      kind,
      title,
      status: "running",
      createdAt: Date.now(),
    };
    this.tasks = [task, ...this.tasks].slice(0, TASK_LIST_LIMIT);
    this.emit();
    const done = (status: TaskStatus, detail?: string, resultPath?: string): void => {
      if (task.status !== "running") return;
      task.status = status;
      task.endedAt = Date.now();
      if (detail !== undefined) task.detail = detail;
      if (resultPath) task.resultPath = resultPath;
      this.emit();
    };
    return {
      update: (progress) => {
        if (task.status !== "running") return;
        task.progress = progress;
        this.emit();
      },
      finish: (detail, resultPath) => done("success", detail, resultPath),
      fail: (detail) => done("error", detail),
      cancel: (detail) => done("cancelled", detail),
      setOpener: (opener) => {
        this.openers.set(task.id, opener);
      },
    };
  }

  /** 点击任务：优先调用注册的 opener；返回是否命中。 */
  open(task: BackgroundTask): boolean {
    const opener = this.openers.get(task.id);
    if (opener) {
      opener();
      return true;
    }
    return false;
  }

  /** 清空已完成 / 已失败 / 已取消的任务（运行中的保留）。 */
  clearFinished(): void {
    const removed = this.tasks.filter((t) => t.status !== "running");
    this.tasks = this.tasks.filter((t) => t.status === "running");
    for (const task of removed) this.openers.delete(task.id);
    this.emit();
  }

  /** 取消全部订阅（测试用）。 */
  reset(): void {
    this.tasks = [];
    this.openers.clear();
    this.listeners.clear();
  }
}

/** 全局单例。 */
export const taskCenter = new TaskCenter();

/** 便捷包装：注册任务并跟踪一个 promise 的成败。 */
export function trackTask<T>(
  kind: TaskKind,
  title: string,
  run: (handle: TaskHandle) => Promise<T>,
): Promise<T> {
  const handle = taskCenter.register(kind, title);
  return run(handle)
    .then((value) => {
      handle.finish();
      return value;
    })
    .catch((error) => {
      handle.fail(String(error));
      throw error;
    });
}
