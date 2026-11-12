/** 通用对话框框架：1px 深边框 + 标题栏，与参考程序一致。 */

import { ICON_CROSS, svg } from "./icons";
import { formatError } from "../core/execution";

export interface DialogOptions {
  title: string;
  width?: number;
  height?: number;
  /** 内容区构造器。 */
  render: (body: HTMLElement, dlg: Dialog) => void;
  /** 底部按钮定义。 */
  buttons?: DialogButton[];
  /** 是否显示关闭按钮。 */
  closable?: boolean;
  /** 点击遮罩是否关闭。 */
  modal?: boolean;
}

export interface DialogButton {
  label: string;
  /** 是否为默认按钮。 */
  primary?: boolean;
  /** 靠左放置（如「测试连接」）。 */
  alignLeft?: boolean;
  /** 渲染后是否隐藏（由调用方按状态切换 el.hidden）。 */
  hidden?: boolean;
  /** 点击回调；返回 false 阻止关闭。 */
  onClick?: (dlg: Dialog) => boolean | void | Promise<boolean | void>;
}

export class Dialog {
  readonly el: HTMLElement;
  readonly body: HTMLElement;
  readonly footer: HTMLElement;
  private backdrop: HTMLElement;
  private onCloseCbs: (() => void)[] = [];
  private closed = false;
  /** 当前错误提示浮层及其自动消失计时器。 */
  private errorBox: HTMLElement | null = null;
  private errorTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(opts: DialogOptions) {
    this.backdrop = document.createElement("div");
    this.backdrop.className = "dialog-backdrop";

    this.el = document.createElement("div");
    this.el.className = "dialog";
    if (opts.width) this.el.style.width = `${opts.width}px`;
    if (opts.height) this.el.style.height = `${opts.height}px`;

    // 标题栏（可拖动）
    const title = document.createElement("div");
    title.className = "dialog-title";
    const titleText = document.createElement("div");
    titleText.className = "dialog-title-text";
    titleText.textContent = opts.title;
    title.appendChild(titleText);
    title.addEventListener("mousedown", (e) => {
      if ((e.target as HTMLElement).closest(".dialog-title-close")) return;
      // 以视口坐标记录抓取点：对话框初始由遮罩 flex 居中，offsetLeft 相对定位祖先，
      // 直接用 offsetLeft 换算会把弹窗跳到偏移量翻倍的位置（点击标题就"乱跳"）。
      const rect = this.el.getBoundingClientRect();
      const grabX = e.clientX - rect.left;
      const grabY = e.clientY - rect.top;
      let dragging = false;

      const move = (ev: MouseEvent) => {
        if (!dragging) {
          if (Math.abs(ev.clientX - e.clientX) < 3 && Math.abs(ev.clientY - e.clientY) < 3) return;
          dragging = true;
          // 从居中布局切到固定定位时先用当前矩形锁位，首帧不会跳。
          this.el.style.position = "fixed";
          this.el.style.margin = "0";
          this.el.style.left = `${rect.left}px`;
          this.el.style.top = `${rect.top}px`;
        }
        const maxX = Math.max(0, window.innerWidth - rect.width);
        const maxY = Math.max(0, window.innerHeight - rect.height);
        const x = Math.min(maxX, Math.max(0, ev.clientX - grabX));
        const y = Math.min(maxY, Math.max(0, ev.clientY - grabY));

        this.el.style.left = `${x}px`;
        this.el.style.top = `${y}px`;
      };
      const up = () => {
        window.removeEventListener("mousemove", move);
        window.removeEventListener("mouseup", up);
      };
      window.addEventListener("mousemove", move);
      window.addEventListener("mouseup", up);
      e.preventDefault();
    });
    if (opts.closable !== false) {
      const close = document.createElement("div");
      close.className = "dialog-title-close";
      close.innerHTML = svg(ICON_CROSS);
      close.addEventListener("click", () => this.close());
      title.appendChild(close);
    }

    // 内容
    this.body = document.createElement("div");
    this.body.className = "dialog-body";

    // 底部按钮
    this.footer = document.createElement("div");
    this.footer.className = "dialog-footer";

    this.el.appendChild(title);
    this.el.appendChild(this.body);
    this.el.appendChild(this.footer);

    this.backdrop.appendChild(this.el);
    document.body.appendChild(this.backdrop);

    if (opts.modal !== false) {
      this.backdrop.addEventListener("mousedown", (e) => {
        if (e.target === this.backdrop && opts.closable !== false) this.close();
      });
    }

    opts.render(this.body, this);
    this.buildButtons(opts.buttons ?? []);
    this.focusFirst();
  }

  private buildButtons(buttons: DialogButton[]): void {
    this.footer.innerHTML = "";
    let leftGroup: HTMLElement | null = null;
    for (const b of buttons) {
      if (b.alignLeft && !leftGroup) {
        // 左侧按钮组（.dialog-footer-left 自带 margin-right:auto 推开右组）
        leftGroup = document.createElement("div");
        leftGroup.className = "dialog-footer-left";
        this.footer.insertBefore(leftGroup, this.footer.firstChild);
      }
      const btn = document.createElement("button");
      btn.className = b.primary ? "btn primary" : "btn";
      btn.textContent = b.label;
      if (b.hidden) btn.hidden = true;
      btn.addEventListener("click", async () => {
        if (!b.onClick) {
          this.close();
          return;
        }
        try {
          const r = await b.onClick(this);
          if (r !== false) this.close();
        } catch (e) {
          console.error(`对话框操作失败：${b.label}`, e);
          this.showError(formatError(e));
        }
      });
      if (b.alignLeft && leftGroup) {
        leftGroup.appendChild(btn);
      } else {
        this.footer.appendChild(btn);
      }
    }
  }

  private focusFirst(): void {
    const input = this.body.querySelector<HTMLElement>(
      "input:not([type=hidden]), select, textarea",
    );
    input?.focus();
  }

  /** 设置标题。 */
  setTitle(t: string): void {
    const el = this.el.querySelector<HTMLElement>(".dialog-title-text");
    if (el) el.textContent = t;
  }

  /** 关闭。 */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.clearError();
    this.backdrop.remove();
    for (const callback of this.onCloseCbs) callback();
  }

  /** 关闭回调。 */
  onClose(cb: () => void): void {
    this.onCloseCbs.push(cb);
  }

  /** 显示错误消息：悬浮在内容区顶部，不挤占布局；自动消失，也可手动关闭。 */
  showError(msg: string): void {
    this.clearError();
    const box = document.createElement("div");
    box.className = "dialog-error toast error";
    box.setAttribute("role", "alert");
    const text = document.createElement("div");
    text.className = "toast-text";
    text.textContent = msg;
    const close = document.createElement("button");
    close.type = "button";
    close.className = "dialog-error-close";
    close.setAttribute("title", "关闭提示");
    close.setAttribute("aria-label", "关闭提示");
    close.innerHTML = svg(ICON_CROSS);
    close.addEventListener("click", () => this.clearError());
    box.appendChild(text);
    box.appendChild(close);
    this.body.insertBefore(box, this.body.firstChild);
    this.errorBox = box;
    // 悬停时暂停倒计时，移开后再继续；避免读长消息时提示突然消失。
    box.addEventListener("mouseenter", () => this.stopErrorTimer());
    box.addEventListener("mouseleave", () => this.scheduleErrorDismiss(box, msg.length));
    this.scheduleErrorDismiss(box, msg.length);
  }

  /** 立即移除当前错误提示并停止自动消失计时。 */
  clearError(): void {
    this.stopErrorTimer();
    this.errorBox = null;
    this.body.querySelector(".dialog-error")?.remove();
  }

  /** 短消息停留 5 秒，长消息最多 12 秒。 */
  private scheduleErrorDismiss(box: HTMLElement, length: number): void {
    this.stopErrorTimer();
    // 最小 DOM 测试环境没有定时器：保留手动关闭即可。
    const defer = (globalThis as unknown as { setTimeout?: typeof setTimeout }).setTimeout;
    if (typeof defer !== "function") return;
    const ttl = Math.min(12000, Math.max(5000, 3000 + length * 60));
    this.errorTimer = defer(() => {
      this.errorTimer = null;
      if (this.errorBox === box) this.clearError();
    }, ttl);
    // Node 测试环境里不要让计时器拖住进程退出。
    (this.errorTimer as unknown as { unref?: () => void }).unref?.();
  }

  private stopErrorTimer(): void {
    if (this.errorTimer === null) return;
    const cancel = (globalThis as unknown as { clearTimeout?: (id: ReturnType<typeof setTimeout>) => void }).clearTimeout;
    if (typeof cancel === "function") cancel(this.errorTimer);
    this.errorTimer = null;
  }
}

/** 构造一个「标签: 控件」字段行。 */
export function field(label: string, control: HTMLElement): HTMLElement {
  const row = document.createElement("div");
  row.className = "field";
  const lab = document.createElement("div");
  lab.className = "field-label";
  lab.textContent = label;
  const wrap = document.createElement("div");
  wrap.className = "field-control";
  wrap.appendChild(control);
  row.appendChild(lab);
  row.appendChild(wrap);
  return row;
}

/** 构造一个文本输入框。 */
export function textInput(value = "", width?: number): HTMLInputElement {
  const el = document.createElement("input");
  el.type = "text";
  el.value = value;
  if (width) el.style.width = `${width}px`;
  return el;
}

/** 构造一个密码输入框。 */
export function passwordInput(value = "", width?: number): HTMLInputElement {
  const el = document.createElement("input");
  el.type = "password";
  el.value = value;
  if (width) el.style.width = `${width}px`;
  return el;
}

/** 构造一个数字输入框。 */
export function numberInput(value: number, width?: number): HTMLInputElement {
  const el = document.createElement("input");
  el.type = "number";
  el.value = String(value);
  if (width) el.style.width = `${width}px`;
  return el;
}

/** 构造一个下拉框。 */
export function select(
  options: { value: string; label: string }[],
  value?: string,
  width?: number,
): HTMLSelectElement {
  const el = document.createElement("select");
  if (width) el.style.width = `${width}px`;
  for (const o of options) {
    const opt = document.createElement("option");
    opt.value = o.value;
    opt.textContent = o.label;
    if (value !== undefined && o.value === value) opt.selected = true;
    el.appendChild(opt);
  }
  return el;
}

/** 构造一个复选框。 */
export function checkbox(label: string, checked = false): HTMLLabelElement {
  const el = document.createElement("label");
  el.className = "checkbox";
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = checked;
  const span = document.createElement("span");
  span.textContent = label;
  el.appendChild(input);
  el.appendChild(span);
  return el;
}

/** 构造一个单选按钮组。 */
export function radioGroup(
  name: string,
  options: { value: string; label: string }[],
  value: string,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.style.display = "flex";
  wrap.style.gap = "16px";
  wrap.style.flexWrap = "wrap";
  for (const o of options) {
    const el = document.createElement("label");
    el.className = "radio";
    const input = document.createElement("input");
    input.type = "radio";
    input.name = name;
    input.value = o.value;
    input.checked = o.value === value;
    const span = document.createElement("span");
    span.textContent = o.label;
    el.appendChild(input);
    el.appendChild(span);
    wrap.appendChild(el);
  }
  return wrap;
}

/** 构造一个分组框。 */
export function groupbox(title: string): { el: HTMLElement; body: HTMLElement } {
  const el = document.createElement("div");
  el.className = "groupbox";
  const t = document.createElement("div");
  t.className = "groupbox-title";
  t.textContent = title;
  const body = document.createElement("div");
  el.appendChild(t);
  el.appendChild(body);
  return { el, body };
}
