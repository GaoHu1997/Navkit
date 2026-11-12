/** 长任务进度条：双段填充（已扫描 / 已执行）+ 状态色 + 速率 ETA。 */
import "../styles/execution-log.css";

export type ProgressPhase = "indeterminate" | "running" | "stopping" | "done" | "stopped" | "failed" | "unknown";

export interface ProgressInput {
  phase: ProgressPhase;
  scanned: number;
  executed: number;
  total: number | null;
  elapsedMs: number;
  /** 进度区下方的一句说明；省略时按阶段给默认文案。 */
  foot?: string;
  /** 计数单位：SQL 文件是「条语句」，生成写入是「行」。 */
  unit?: string;
}

const phaseText: Record<ProgressPhase, string> = {
  indeterminate: "扫描中", running: "执行中", stopping: "正在停止",
  done: "已完成", stopped: "已停止", failed: "执行失败", unknown: "结果未知",
};

function count(value: number): string {
  return Number.isFinite(value) && value > 0 ? Math.floor(value).toLocaleString("en-US") : "0";
}
function clock(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return String(Math.floor(seconds / 60)).padStart(2, "0") + ":" + String(seconds % 60).padStart(2, "0");
}

export class ExecutionProgress {
  readonly el = document.createElement("section");
  private readonly stateText = document.createElement("span");
  private readonly countValue = document.createElement("b");
  private readonly countUnit = document.createElement("span");
  private readonly eta = document.createElement("span");
  private readonly bar = document.createElement("div");
  private readonly scanFill = document.createElement("div");
  private readonly runFill = document.createElement("div");
  private readonly foot = document.createElement("div");
  private readonly samples: { executed: number; elapsedMs: number }[] = [];
  private lastExecuted = -1;

  constructor() {
    this.el.className = "exec-log-progress";
    this.el.setAttribute("role", "group");
    this.el.setAttribute("aria-label", "任务进度");
    const head = document.createElement("div"); head.className = "exec-log-progress-head";
    const state = document.createElement("span"); state.className = "exec-log-state";
    const dot = document.createElement("i"); dot.setAttribute("aria-hidden", "true");
    this.stateText.className = "exec-log-state-text";
    state.append(dot, this.stateText);
    const countBox = document.createElement("span"); countBox.className = "exec-log-count";
    countBox.append(this.countValue, this.countUnit);
    this.eta.className = "exec-log-eta";
    head.append(state, countBox, this.eta);
    this.bar.className = "exec-log-bar";
    this.bar.setAttribute("role", "progressbar");
    this.bar.setAttribute("aria-valuemin", "0");
    this.scanFill.className = "exec-log-bar-scan";
    this.runFill.className = "exec-log-bar-run";
    this.bar.append(this.scanFill, this.runFill);
    this.foot.className = "exec-log-progress-foot";
    this.el.append(head, this.bar, this.foot);
  }

  setInput(input: ProgressInput): void {
    const phase = input.phase;
    this.el.dataset.phase = phase;
    this.stateText.textContent = phaseText[phase];
    const total = typeof input.total === "number" && Number.isFinite(input.total) && input.total > 0 ? Math.floor(input.total) : null;
    const scanned = Math.max(0, Math.floor(input.scanned));
    const executed = Math.max(0, Math.floor(input.executed));

    if (phase === "indeterminate" || total === null) {
      this.countValue.textContent = "已扫描 " + count(scanned) + " 条";
      this.countUnit.textContent = "";
      this.scanFill.style.width = scanned > 0 ? "100%" : "0%";
      this.runFill.style.width = "0%";
      this.bar.removeAttribute("aria-valuemax");
      this.bar.removeAttribute("aria-valuenow");
      this.bar.setAttribute("aria-valuetext", "正在扫描完整语句，暂不给出百分比");
      this.eta.textContent = "已用 " + clock(input.elapsedMs);
    } else {
      const done = phase === "done" ? total : Math.min(executed, total);
      this.countValue.textContent = count(done) + " / " + count(total);
      this.countUnit.textContent = " " + (input.unit ?? "条语句");
      this.scanFill.style.width = Math.min(100, Math.round((scanned / total) * 10000) / 100) + "%";
      this.runFill.style.width = Math.round((done / total) * 10000) / 100 + "%";
      this.bar.setAttribute("aria-valuemax", String(total));
      this.bar.setAttribute("aria-valuenow", String(done));
      this.bar.setAttribute("aria-valuetext", "已完成 " + count(done) + " / " + count(total) + " 条语句");
      const remaining = this.estimate(executed, total, input.elapsedMs);
      this.eta.textContent = remaining === null
        ? "已用 " + clock(input.elapsedMs)
        : "已用 " + clock(input.elapsedMs) + " · 剩余约 " + clock(remaining);
    }
    if (phase === "done") this.eta.textContent = "共 " + count(total ?? executed) + " " + (input.unit ?? "条") + " · 用时 " + clock(input.elapsedMs);
    this.foot.textContent = input.foot ?? this.defaultFoot(phase, scanned);
  }

  private defaultFoot(phase: ProgressPhase, scanned: number): string {
    if (phase === "indeterminate") return "正在扫描 SQL 文件，扫描到的完整语句会立即执行";
    if (phase === "stopping") return "当前语句返回后才停止，之后不再提交";
    if (phase === "stopped") return "已停止；已提交的语句保持写入，不再提交后续语句";
    if (phase === "unknown") return "结果未知，正在回看同一任务；不提供重放";
    if (phase === "failed") return "失败语句已定位，已成功写入不自动回滚";
    return scanned > 0 ? "已扫描 " + count(scanned) + " 条完整语句" : "";
  }

  /** 只在执行量增长时采样，至少 20 条且速率可用才给剩余时间。 */
  private estimate(executed: number, total: number, elapsedMs: number): number | null {
    if (executed !== this.lastExecuted) {
      this.lastExecuted = executed;
      this.samples.push({ executed, elapsedMs });
      if (this.samples.length > 6) this.samples.shift();
    }
    if (executed < 20 || executed >= total || this.samples.length < 2) return null;
    const first = this.samples[0];
    const dExecuted = executed - first.executed;
    const dElapsed = elapsedMs - first.elapsedMs;
    if (dExecuted <= 0 || dElapsed <= 0) return null;
    const remaining = ((total - executed) * dElapsed) / dExecuted;
    return Number.isFinite(remaining) && remaining >= 0 ? remaining : null;
  }

  dispose(): void { this.samples.length = 0; }
}