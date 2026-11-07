/**
 * 对话框内的分页向导（Wizard）。
 *
 * 用于把「一页塞满」的长表单拆成按顺序的多步：
 *   · 顶部步骤条：序号徽标（完成态变勾）+ 连接轨道 + 步名 + 当前步强调
 *   · 每步有标题与一句说明，视线先落标题再看表单
 *   · 内容区卡片化：白底 + 发丝边 + 内边距，一屏一主题
 *   · 底部「上一步 / 下一步 / 完成」+ 错误行（就地显示校验与执行错误）
 *
 * 设计目标：一屏一主题，降低表单认知负担；已完成步骤可点击回退。
 */

export interface WizardStep {
  /** 步骤标题（显示在步骤条上）。 */
  label: string;
  /** 该步的说明文案（显示在内容区标题下方，一句话说清这一步在做什么）。 */
  hint?: string;
  /** 步骤内容构造器（只调用一次，元素常驻，切换时显隐）。 */
  render: (host: HTMLElement) => void;
  /** 允许进入下一步的前置校验；返回错误文案则阻止前进。 */
  validate?: () => string | null;
}

export interface WizardOptions {
  steps: WizardStep[];
  /** 完成后回调（最后一步点「完成」）。 */
  onFinish: () => void | Promise<void>;
  /** 完成按钮文案。 */
  finishLabel?: string;
  /** 步骤切换回调（常用于懒加载该步数据）。 */
  onEnter?: (index: number) => void;
}

/** 勾选图标（完成态徽标）。 */
const CHECK_SVG =
  '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2.4 6.3 4.9 8.7 9.6 3.4" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>';

export class Wizard {
  /** 步骤条元素（调用方 append 到 body 顶部）。 */
  readonly stepsEl: HTMLElement;
  /** 当前步内容容器（调用方 append 到 body）。 */
  readonly bodyEl: HTMLElement;
  /** 底部按钮（调用方放进 Dialog footer，顺序：上一步 → 下一步/完成）。 */
  readonly backBtn: HTMLButtonElement;
  readonly nextBtn: HTMLButtonElement;
  /** 错误提示行。 */
  readonly errorEl: HTMLElement;

  private index = 0;
  private readonly steps: WizardStep[];
  private readonly nodes: HTMLElement[] = [];
  private readonly stepEls: HTMLElement[] = [];
  private readonly badges: HTMLElement[] = [];
  private readonly lines: HTMLElement[] = [];
  private readonly opts: WizardOptions;
  private busy = false;

  constructor(opts: WizardOptions) {
    this.opts = opts;
    this.steps = opts.steps;

    this.stepsEl = document.createElement("div");
    this.stepsEl.className = "wizard-steps";
    this.stepsEl.setAttribute("role", "tablist");

    this.bodyEl = document.createElement("div");
    this.bodyEl.className = "wizard-body";

    this.errorEl = document.createElement("div");
    this.errorEl.className = "wizard-error";
    this.errorEl.setAttribute("role", "alert");
    this.errorEl.hidden = true;

    this.backBtn = document.createElement("button");
    this.backBtn.type = "button";
    this.backBtn.className = "btn";
    this.backBtn.textContent = "上一步";
    this.backBtn.addEventListener("click", () => this.back());

    this.nextBtn = document.createElement("button");
    this.nextBtn.type = "button";
    this.nextBtn.className = "btn primary";
    this.nextBtn.addEventListener("click", () => void this.next());

    this.buildSteps();
    this.buildPanels();
    this.go(0);
  }

  /** 底部按钮组（上一步 + 下一步/完成），供 Dialog footer 使用。 */
  buttons(): HTMLElement[] {
    const wrap = document.createElement("div");
    wrap.className = "wizard-actions";
    wrap.append(this.backBtn, this.nextBtn);
    return [wrap];
  }

  /** 跳到指定步（不校验）。 */
  go(index: number): void {
    const clamped = Math.max(0, Math.min(this.steps.length - 1, index));
    this.index = clamped;
    this.stepEls.forEach((el, i) => {
      const done = i < clamped;
      const active = i === clamped;
      el.classList.toggle("active", active);
      el.classList.toggle("done", done);
      el.setAttribute("aria-selected", String(active));
      el.setAttribute("aria-current", active ? "step" : "false");
      // 序号徽标：完成态换勾，当前态强调实底
      const badge = this.badges[i];
      if (badge) {
        badge.innerHTML = done ? CHECK_SVG : String(i + 1);
      }
    });
    // 轨道：已走过的段着色
    this.lines.forEach((line, i) => {
      line.classList.toggle("is-done", i < clamped);
    });
    this.nodes.forEach((node, i) => {
      node.hidden = i !== clamped;
    });
    this.clearError();
    this.backBtn.disabled = clamped === 0;
    const last = clamped === this.steps.length - 1;
    this.nextBtn.textContent = last ? (this.opts.finishLabel ?? "完成") : "下一步";
    // 切换时内容回到顶部
    const pane = this.nodes[clamped]?.querySelector<HTMLElement>(".wizard-panel-inner");
    if (pane) pane.scrollTop = 0;
    this.opts.onEnter?.(clamped);
  }

  /** 当前步索引。 */
  get current(): number {
    return this.index;
  }

  /** 上一步。 */
  back(): void {
    if (this.index > 0) this.go(this.index - 1);
  }

  /** 下一步（含校验）；最后一步触发 onFinish。 */
  async next(): Promise<void> {
    if (this.busy) return;
    const step = this.steps[this.index];
    const problem = step?.validate?.() ?? null;
    if (problem) {
      this.showError(problem);
      return;
    }
    if (this.index < this.steps.length - 1) {
      this.go(this.index + 1);
      return;
    }
    this.busy = true;
    this.nextBtn.disabled = true;
    this.nextBtn.classList.add("busy");
    try {
      await this.opts.onFinish();
    } finally {
      this.busy = false;
      this.nextBtn.disabled = false;
      this.nextBtn.classList.remove("busy");
    }
  }

  /** 显示校验/执行错误。 */
  showError(message: string): void {
    this.errorEl.textContent = message;
    this.errorEl.hidden = false;
  }

  /** 清除错误。 */
  clearError(): void {
    this.errorEl.hidden = true;
  }

  /** 锁定底部按钮（长任务执行期间避免重复提交或中途跳步）。 */
  setBusy(busy: boolean): void {
    this.busy = busy;
    this.nextBtn.disabled = busy;
    this.backBtn.disabled = busy || this.index === 0;
  }

  private buildSteps(): void {
    this.steps.forEach((s, i) => {
      const step = document.createElement("div");
      step.className = "wizard-step";
      step.setAttribute("role", "tab");
      step.tabIndex = 0;

      const badge = document.createElement("div");
      badge.className = "wizard-step-num";
      badge.textContent = String(i + 1);

      const label = document.createElement("div");
      label.className = "wizard-step-label";
      label.textContent = s.label;

      step.append(badge, label);
      // 已完成步骤可点击回退；当前/未来步骤不可点
      const activate = () => {
        if (this.busy) return;
        if (i < this.index) this.go(i);
      };
      step.addEventListener("click", activate);
      step.addEventListener("keydown", (e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          activate();
        }
      });

      this.stepsEl.appendChild(step);
      this.stepEls.push(step);
      this.badges.push(badge);

      if (i < this.steps.length - 1) {
        const line = document.createElement("div");
        line.className = "wizard-step-line";
        this.stepsEl.appendChild(line);
        this.lines.push(line);
      }
    });
  }

  private buildPanels(): void {
    this.steps.forEach((s, i) => {
      const node = document.createElement("div");
      node.className = "wizard-panel";
      node.setAttribute("role", "tabpanel");

      const inner = document.createElement("div");
      inner.className = "wizard-panel-inner";

      // 每步头部：标题 + 说明（先读标题再填表）
      const head = document.createElement("div");
      head.className = "wizard-panel-head";
      const title = document.createElement("div");
      title.className = "wizard-panel-title";
      title.textContent = s.label;
      head.appendChild(title);
      if (s.hint) {
        const hint = document.createElement("div");
        hint.className = "wizard-panel-hint";
        hint.textContent = s.hint;
        head.appendChild(hint);
      }
      // 步骤计数（右上角，弱化）
      const counter = document.createElement("div");
      counter.className = "wizard-panel-count";
      counter.textContent = `${i + 1} / ${this.steps.length}`;
      head.appendChild(counter);
      inner.appendChild(head);

      const card = document.createElement("div");
      card.className = "wizard-panel-card";
      s.render(card);
      inner.appendChild(card);

      node.appendChild(inner);
      this.bodyEl.appendChild(node);
      this.nodes.push(node);
    });
    this.bodyEl.appendChild(this.errorEl);
  }
}
