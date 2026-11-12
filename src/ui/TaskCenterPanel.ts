/**
 * 任务中心面板 —— 标题栏任务图标的下拉弹层。
 *
 * 交互（按需求文档 1.3）：
 *  · 点击任务图标显示任务列表，再次点击关闭
 *  · 运行中的任务显示进度；点击任务行重新打开对应弹窗（有 opener 时）
 *  · 「清空已完成」清掉成功 / 失败 / 已取消的记录，运行中的保留
 */

import { Dialog } from "../ui/Dialog";
import { accessibleDialog } from "../dialogs/SyncDialog";
import { iconSvgByName } from "../ui/icons";
import {
  TASK_KIND_LABEL,
  taskCenter,
  type BackgroundTask,
  type TaskKind,
} from "../core/backgroundTasks";
import "./../styles/task-center.css";

/** 任务类型 → 图标键（复用 icons.ts NAME_MAP 既有图标）。 */
const KIND_ICON: Record<TaskKind, string> = {
  import: "import_wizard",
  export: "export_wizard",
  "sql-file": "editor",
  dump: "dbn_backup",
  backup: "dbn_backup",
  restore: "dbn_backup",
  datagen: "data_generation_ent_preview_image",
  sync: "data_sync_ent_preview_image",
  transfer: "data_transfer_ent_preview_image",
  "batch-export": "export_wizard",
  dictionary: "editor",
  update: "settings_gear",
  other: "info_pane_table",
};

const FALLBACK_ICON = "info_pane_table";

const STATUS_LABEL: Record<BackgroundTask["status"], string> = {
  running: "进行中",
  success: "已完成",
  error: "失败",
  cancelled: "已取消",
};

function svgByName(kind: TaskKind): string {
  return iconSvgByName(KIND_ICON[kind] ?? FALLBACK_ICON);
}

/** 时间显示：今天显示时分，其余显示月-日 时分。 */
function timeText(ts: number): string {
  const d = new Date(ts);
  const now = new Date();
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  if (d.toDateString() === now.toDateString()) return hm;
  return `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")} ${hm}`;
}

/** 任务中心弹层。宿主负责挂载与开关切换。 */
export class TaskCenterPanel {
  readonly el: HTMLElement;
  private readonly listHost: HTMLElement;
  private readonly countEl: HTMLElement;
  private readonly clearBtn: HTMLButtonElement;
  private readonly emptyEl: HTMLElement;
  private unsubscribe: (() => void) | null = null;
  private outsideHandler: ((e: MouseEvent) => void) | null = null;
  private escHandler: ((e: KeyboardEvent) => void) | null = null;
  /** 点击行为接管（App 注入，用于打开任务详情弹窗）。 */
  onOpenTask: (task: BackgroundTask) => void = () => {};

  constructor() {
    this.el = document.createElement("div");
    this.el.className = "task-center";
    this.el.setAttribute("role", "region");
    this.el.setAttribute("aria-label", "后台任务");
    this.el.hidden = true;

    const head = document.createElement("div");
    head.className = "task-center-head";
    const title = document.createElement("div");
    title.className = "task-center-title";
    title.textContent = "后台任务";
    this.countEl = document.createElement("span");
    this.countEl.className = "task-center-count";
    title.appendChild(this.countEl);
    this.clearBtn = document.createElement("button");
    this.clearBtn.type = "button";
    this.clearBtn.className = "task-center-clear";
    this.clearBtn.textContent = "清空已完成";
    this.clearBtn.addEventListener("click", () => {
      taskCenter.clearFinished();
      this.refresh();
    });
    head.append(title, this.clearBtn);

    this.listHost = document.createElement("div");
    this.listHost.className = "task-center-list";

    this.emptyEl = document.createElement("div");
    this.emptyEl.className = "task-center-empty";
    this.emptyEl.textContent = "暂无后台任务";

    this.el.append(head, this.listHost, this.emptyEl);
    this.refresh();
  }

  /** 打开 / 关闭。 */
  toggle(anchor: HTMLElement): void {
    if (this.el.hidden) {
      this.open(anchor);
    } else {
      this.close();
    }
  }

  open(anchor: HTMLElement): void {
    this.el.hidden = false;
    this.refresh();
    this.unsubscribe = taskCenter.subscribe(() => this.refresh());
    document.body.appendChild(this.el);
    const rect = anchor.getBoundingClientRect();
    const width = this.el.offsetWidth || 320;
    this.el.style.top = `${rect.bottom + 6}px`;
    this.el.style.left = `${Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8))}px`;
    // 外部点击关闭
    this.outsideHandler = (e: MouseEvent) => {
      if (!this.el.contains(e.target as Node) && !anchor.contains(e.target as Node)) this.close();
    };
    this.escHandler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        this.close();
      }
    };
    document.addEventListener("mousedown", this.outsideHandler, true);
    document.addEventListener("keydown", this.escHandler, true);
  }

  close(): void {
    this.el.hidden = true;
    this.el.remove();
    this.unsubscribe?.();
    this.unsubscribe = null;
    if (this.outsideHandler) document.removeEventListener("mousedown", this.outsideHandler, true);
    if (this.escHandler) document.removeEventListener("keydown", this.escHandler, true);
    this.outsideHandler = null;
    this.escHandler = null;
  }

  /** 重新渲染任务列表。 */
  refresh(): void {
    const { tasks, running } = taskCenter.snapshot();
    this.countEl.textContent = running > 0 ? `（${running} 个进行中）` : "";
    this.clearBtn.disabled = tasks.every((t) => t.status === "running") || tasks.length === 0;
    this.emptyEl.hidden = tasks.length > 0;
    this.listHost.replaceChildren();
    for (const task of tasks) {
      this.listHost.appendChild(this.renderRow(task));
    }
  }

  private renderRow(task: BackgroundTask): HTMLElement {
    const row = document.createElement("button");
    row.type = "button";
    row.className = `task-row is-${task.status}`;
    row.setAttribute("aria-label", `${task.title}（${STATUS_LABEL[task.status]}）`);

    const icon = document.createElement("span");
    icon.className = "task-row-icon";
    icon.innerHTML = svgByName(task.kind);

    const body = document.createElement("span");
    body.className = "task-row-body";
    const title = document.createElement("span");
    title.className = "task-row-title";
    title.textContent = task.title;
    title.title = task.title;
    const meta = document.createElement("span");
    meta.className = "task-row-meta";
    const progress = task.progress;
    const pct =
      progress && progress.total ? Math.round(((progress.done ?? 0) / progress.total) * 100) : null;
    const parts: string[] = [TASK_KIND_LABEL[task.kind]];
    if (task.status === "running") {
      parts.push(progress?.message || (pct !== null ? `${pct}%` : "进行中"));
    } else {
      parts.push(STATUS_LABEL[task.status]);
    }
    if (pct !== null) parts.push(`${pct}%`);
    meta.textContent = parts.join(" · ");
    body.append(title, meta);

    const side = document.createElement("span");
    side.className = "task-row-side";
    const time = document.createElement("span");
    time.className = "task-row-time";
    time.textContent = timeText(task.endedAt ?? task.createdAt);
    side.appendChild(time);

    // 进度条（运行中且可量化）
    if (task.status === "running" && progress?.total) {
      const track = document.createElement("span");
      track.className = "task-row-track";
      const fill = document.createElement("span");
      fill.className = "task-row-fill";
      fill.style.width = `${pct ?? 0}%`;
      track.appendChild(fill);
      body.appendChild(track);
    }
    // 失败原因
    if (task.status === "error" && task.detail) {
      const detail = document.createElement("span");
      detail.className = "task-row-error";
      detail.textContent = task.detail;
      detail.title = task.detail;
      body.appendChild(detail);
    }

    row.append(icon, body, side);
    { // 运行中的任务也可点开之前的任务框（opener）查看实时进度。
      row.addEventListener("click", () => this.openTask(task));
      row.disabled = false;
    }
    return row;
  }

  private openTask(task: BackgroundTask): void {
    this.close();
    if (taskCenter.open(task)) return;
    this.onOpenTask(task);
  }
}

/** 任务详情弹窗（无 opener 的任务点击时展示状态 / 详情 / 产物路径）。 */
export function openTaskDetailDialog(task: BackgroundTask, onOpenPath?: (path: string) => void): Dialog {
  const dlg = new Dialog({ title: "任务详情", width: 480, render: () => {}, buttons: [{ label: "关闭" }] });
  const body = dlg.body;
  body.style.display = "flex";
  body.style.flexDirection = "column";
  body.style.gap = "10px";

  const addRow = (label: string, value: string): void => {
    const row = document.createElement("div");
    row.style.cssText = "display:grid;grid-template-columns:64px 1fr;gap:8px;font-size:12px;";
    const l = document.createElement("span");
    l.style.color = "var(--text-tertiary)";
    l.textContent = label;
    const v = document.createElement("span");
    v.style.overflowWrap = "anywhere";
    v.textContent = value;
    row.append(l, v);
    body.appendChild(row);
  };

  addRow("名称", task.title);
  addRow("类型", TASK_KIND_LABEL[task.kind]);
  addRow("状态", STATUS_LABEL[task.status]);
  addRow("开始", new Date(task.createdAt).toLocaleString());
  if (task.endedAt) addRow("结束", new Date(task.endedAt).toLocaleString());
  if (task.progress?.total) {
    const pct = Math.round(((task.progress.done ?? 0) / task.progress.total) * 100);
    addRow("进度", `${task.progress.done ?? 0} / ${task.progress.total}（${pct}%）`);
  }
  if (task.detail) addRow("详情", task.detail);
  if (task.resultPath) {
    addRow("产物", task.resultPath);
    if (onOpenPath) {
      const openBtn = document.createElement("button");
      openBtn.type = "button";
      openBtn.className = "btn";
      openBtn.textContent = "打开所在文件夹";
      openBtn.addEventListener("click", () => onOpenPath(task.resultPath!));
      body.appendChild(openBtn);
    }
  }
  accessibleDialog(dlg, "任务详情");
  return dlg;
}
