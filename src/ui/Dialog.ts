/** 通用对话框框架：1px 深边框 + 标题栏，与参考程序一致。 */

import { ICON_CROSS, svg } from "./icons";

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
      const startX = e.clientX - this.el.offsetLeft;
      const startY = e.clientY - this.el.offsetTop;
      const move = (ev: MouseEvent) => {
        const x = Math.max(-this.el.offsetWidth + 80, Math.min(window.innerWidth - 80, ev.clientX - startX));
        const y = Math.max(0, Math.min(window.innerHeight - 40, ev.clientY - startY));
        this.el.style.position = "relative";
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
          this.showError(e instanceof Error ? e.message : String(e));
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
    this.backdrop.remove();
    for (const callback of this.onCloseCbs) callback();
  }

  /** 关闭回调。 */
  onClose(cb: () => void): void {
    this.onCloseCbs.push(cb);
  }

  /** 显示错误消息（在内容区顶部插入一条）。 */
  showError(msg: string): void {
    this.body.querySelector(".dialog-error")?.remove();
    const box = document.createElement("div");
    box.className = "dialog-error toast error";
    box.style.position = "relative";
    box.style.right = "auto";
    box.style.bottom = "auto";
    box.style.marginBottom = "8px";
    const text = document.createElement("div");
    text.className = "toast-text";
    text.textContent = msg;
    box.appendChild(text);
    this.body.insertBefore(box, this.body.firstChild);
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
  wrap.style.gap = "12px";
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
