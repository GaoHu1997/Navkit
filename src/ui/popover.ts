/** 锚定浮层：贴在触发元素下方、点外部或 Esc 关闭、同一时刻只开一个。
 *  表设计器的「默认值候选 / 扩展属性 / 索引字段」都用它，取代原来的模态 Dialog。 */

export interface PopoverHandle {
  element: HTMLElement;
  anchor: HTMLElement;
  close: () => void;
}

export interface PopoverOptions {
  anchor: HTMLElement;
  /** 附加类名，例如 ext / choices */
  className?: string;
  /** 固定宽度；缺省按内容收缩（最小 180） */
  width?: number;
  ariaLabel?: string;
  render: (body: HTMLElement, close: () => void) => void;
  onClose?: () => void;
}

let active: PopoverHandle | null = null;

export function closePopover(): void {
  active?.close();
}

export function activePopover(): PopoverHandle | null {
  return active;
}

/** 该触发元素当前是否已经开着浮层（点击同一元素应关闭，而不是重开）。 */
export function isPopoverOpenFor(anchor: HTMLElement): boolean {
  return !!active && active.anchor === anchor;
}

const CHEVRON = '<svg viewBox="0 0 10 10" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true"><path d="M2 4l3 3 3-3"/></svg>';
export { CHEVRON as POPOVER_CHEVRON };

export function openPopover(options: PopoverOptions): PopoverHandle {
  closePopover();
  const { anchor } = options;
  const element = document.createElement("div");
  element.className = `popover${options.className ? ` ${options.className}` : ""}`;
  element.setAttribute("role", "dialog");
  element.setAttribute("aria-modal", "false");
  if (options.ariaLabel) element.setAttribute("aria-label", options.ariaLabel);
  if (options.width) element.style.width = `${options.width}px`;
  const body = document.createElement("div");
  body.className = "popover-body";
  element.appendChild(body);

  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    document.removeEventListener("pointerdown", onPointerDown, true);
    document.removeEventListener("keydown", onKeyDown, true);
    window.removeEventListener("resize", close);
    window.removeEventListener("scroll", onScroll, true);
    element.remove();
    if (active?.element === element) active = null;
    options.onClose?.();
  };

  const onPointerDown = (event: PointerEvent) => {
    const target = event.target as Node | null;
    if (!target) return;
    if (element.contains(target) || anchor.contains(target)) return;
    close();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    close();
    if (anchor.isConnected) anchor.focus();
  };
  const onScroll = (event: Event) => {
    const target = event.target as Node | null;
    if (target && element.contains(target)) return;
    close();
  };

  options.render(body, close);

  document.body.appendChild(element);
  place();
  document.addEventListener("pointerdown", onPointerDown, true);
  document.addEventListener("keydown", onKeyDown, true);
  window.addEventListener("resize", close);
  window.addEventListener("scroll", onScroll, true);

  function place(): void {
    const rect = anchor.getBoundingClientRect();
    element.style.visibility = "hidden";
    element.style.left = "0px";
    element.style.top = "0px";
    const size = element.getBoundingClientRect();
    const gap = 4;
    const below = rect.bottom + gap;
    const above = rect.top - size.height - gap;
    const useAbove = below + size.height > window.innerHeight - 8 && above > 8;
    const top = useAbove ? above : below;
    let left = rect.left;
    if (left + size.width > window.innerWidth - 8) left = Math.max(8, rect.right - size.width);
    element.style.left = `${Math.round(left)}px`;
    element.style.top = `${Math.round(top)}px`;
    element.dataset.placement = useAbove ? "top" : "bottom";
    element.style.visibility = "";
  }

  const handle: PopoverHandle = { element, anchor, close };
  active = handle;
  return handle;
}
