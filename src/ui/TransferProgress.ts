/** 只显示真实处理量；没有总量时不虚构百分比。 */
export type TransferPhase = "idle" | "running" | "done" | "failed";
const PHASE_TEXT: Record<TransferPhase, string> = {
  idle: "待执行", running: "执行中", done: "已完成", failed: "执行失败",
};
export class TransferProgress {
  readonly el = document.createElement("section");
  private readonly phaseText = document.createElement("span");
  private readonly counter = document.createElement("span");
  private readonly bar = document.createElement("div");
  private readonly fill = document.createElement("div");
  private readonly note = document.createElement("div");
  constructor(label = "执行进度") {
    this.el.className = "transfer-progress";
    this.el.setAttribute("role", "group");
    this.el.setAttribute("aria-label", label);
    const head = document.createElement("div");
    head.className = "transfer-progress-head";
    head.setAttribute("role", "status");
    const dot = document.createElement("i");
    dot.className = "dot";
    dot.setAttribute("aria-hidden", "true");
    this.counter.className = "transfer-progress-count";
    head.append(dot, this.phaseText, this.counter);
    this.bar.className = "transfer-progress-bar";
    this.bar.setAttribute("role", "progressbar");
    this.bar.setAttribute("aria-label", label);
    this.bar.setAttribute("aria-valuemin", "0");
    this.bar.setAttribute("aria-valuemax", "100");
    this.fill.className = "transfer-progress-fill";
    this.bar.append(this.fill);
    this.note.className = "transfer-progress-note";
    this.el.append(head, this.bar, this.note);
    this.reset();
  }
  begin(note = ""): void {
    this.reset();
    this.el.dataset.state = "running";
    this.el.dataset.indeterminate = "true";
    this.phaseText.textContent = PHASE_TEXT.running;
    this.bar.setAttribute("aria-valuetext", "执行中，总量未知");
    this.note.textContent = note;
  }
  update(processed: number, total?: number | null, unit = "行"): void {
    if (this.el.dataset.state !== "running" || !Number.isFinite(processed)) return;
    const count = Math.max(0, processed);
    if (total != null && Number.isFinite(total) && total > 0) {
      const percent = Math.min(100, Math.floor(count / total * 100));
      this.el.dataset.indeterminate = "false";
      this.fill.style.width = percent + "%";
      this.bar.setAttribute("aria-valuenow", String(percent));
      this.counter.textContent = count.toLocaleString("en-US") + " / " + total.toLocaleString("en-US") + " " + unit + " · " + percent + "%";
    } else {
      this.el.dataset.indeterminate = "true";
      this.fill.style.width = "";
      this.bar.removeAttribute("aria-valuenow");
      this.counter.textContent = "已处理 " + count.toLocaleString("en-US") + " " + unit + " · 总量未知";
    }
    this.bar.setAttribute("aria-valuetext", this.counter.textContent);
  }
  finish(note = ""): void {
    this.el.dataset.state = "done";
    this.el.dataset.indeterminate = "false";
    this.phaseText.textContent = PHASE_TEXT.done;
    this.counter.textContent = "100%";
    this.fill.style.width = "100%";
    this.bar.setAttribute("aria-valuenow", "100");
    this.bar.setAttribute("aria-valuetext", "已完成");
    this.note.textContent = note;
  }
  fail(note = ""): void {
    this.el.dataset.state = "failed";
    this.phaseText.textContent = PHASE_TEXT.failed;
    this.bar.setAttribute("aria-valuetext", "执行失败" + (this.counter.textContent ? "，" + this.counter.textContent : ""));
    this.note.textContent = note;
  }
  reset(): void {
    this.el.dataset.state = "idle";
    this.el.dataset.indeterminate = "false";
    this.phaseText.textContent = PHASE_TEXT.idle;
    this.counter.textContent = "";
    this.fill.style.width = "0%";
    this.bar.removeAttribute("aria-valuenow");
    this.bar.setAttribute("aria-valuetext", PHASE_TEXT.idle);
    this.note.textContent = "";
  }
}
