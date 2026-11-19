/**
 * 原生 `<input list="…">`（datalist）的搜索增强：保留自由输入，把浏览器的建议列表换成
 * 可过滤的浮层，交互与 SearchableSelect 一致（方向键/Home/End/Enter/Escape、点击选择）。
 *
 * 为什么需要它：SearchableSelect 增强原生 select；目标表、字段、驱动文件这类
 * 「可输入 + 建议」的输入框仍是浏览器自带的 datalist 弹层，没有搜索框、也无法过滤。
 *
 * 约定：`data-search-token=","` 表示该输入框是逗号分隔的多值字段（如列名列表），
 * 建议与替换都只作用于光标所在的最后一段。
 */
type InputRoot = Document | HTMLElement;
interface OpenInput {
  input: HTMLInputElement;
  panel: HTMLDivElement;
  list: HTMLUListElement;
  status: HTMLDivElement;
  all: string[];
  values: string[];
  rows: HTMLLIElement[];
  active: number;
  /** 用户用方向键/Home/End 明确移动过选中项（默认高亮不算）。 */
  navigated: boolean;
  /** 打开时的初始筛选词覆盖：`data-search-open-all` 的控件先列全部候选，输入后才过滤。 */
  queryOverride: string | null;
  savedList: string | null;
  composing: boolean;
  restore: () => void;
}

const installations = new WeakMap<InputRoot, () => void>();
let nextId = 0;

function datalistOf(input: HTMLInputElement): HTMLDataListElement | null {
  const id = input.getAttribute("list");
  if (!id) return null;
  const list = input.ownerDocument.getElementById(id);
  return list instanceof HTMLDataListElement ? list : null;
}

function available(input: HTMLInputElement): boolean {
  const type = (input.getAttribute("type") ?? "text").toLowerCase();
  if (!["text", "search", "url"].includes(type)) return false;
  return usable(input) && datalistOf(input) !== null;
}

/** 已打开的浮层只看输入框是否仍然可用：打开期间我们会临时摘掉 `list`。 */
function usable(input: HTMLInputElement): boolean {
  return input.isConnected && !input.disabled && !input.readOnly && !input.closest("[inert]");
}

function valuesOf(list: HTMLDataListElement): string[] {
  const seen = new Set<string>();
  const values: string[] = [];
  for (const option of Array.from(list.options)) {
    const value = (option.value || option.textContent || "").trim();
    if (value && !seen.has(value)) { seen.add(value); values.push(value); }
  }
  return values;
}

function separatorOf(input: HTMLInputElement): string {
  return input.getAttribute("data-search-token") ?? "";
}

/** 前缀（含分隔符与其后的空白）与光标所在的最后一段文本。 */
function segmentOf(input: HTMLInputElement): { prefix: string; token: string } {
  const separator = separatorOf(input);
  const value = input.value;
  if (!separator) return { prefix: "", token: value.trim() };
  const cut = value.lastIndexOf(separator);
  if (cut < 0) return { prefix: "", token: value.trim() };
  const rest = value.slice(cut + separator.length);
  // 保留分隔符后的空格（`id, na`）：回写时 `id, ` 原样保留，只替换 `na`。
  const lead = /^\s*/.exec(rest)?.[0] ?? "";
  return { prefix: `${value.slice(0, cut + separator.length)}${lead}`, token: rest.slice(lead.length).trim() };
}

function nameOf(input: HTMLInputElement): string {
  const labelled = input.getAttribute("aria-labelledby")?.split(/\s+/).map(id =>
    input.ownerDocument.getElementById(id)?.textContent?.trim() ?? "").filter(Boolean).join(" ");
  return labelled || input.getAttribute("aria-label")?.trim() ||
    Array.from(input.labels ?? []).map(label => {
      const copy = label.cloneNode(true) as HTMLLabelElement;
      copy.querySelectorAll("input, select, textarea, button, script, style").forEach(node => node.remove());
      return copy.textContent?.trim();
    }).filter(Boolean).join(" ") ||
    input.title.trim() || input.placeholder.trim() || "输入建议";
}

/**
 * 在入口安装一次；动态创建的 input[list] 自动生效。重复安装同一 root 返回同一清理函数。
 * 不支持 HTML Popover 的 WebView 保留原生 datalist；清理函数关闭浮层并解除所有监听。
 */
export function installSearchableInputs(root: InputRoot = document): () => void {
  const installed = installations.get(root);
  if (installed) return installed;
  const doc = root.ownerDocument ?? root as Document;
  const win = doc.defaultView;
  if (!win || typeof HTMLElement.prototype.showPopover !== "function") return () => {};
  const listeners = new AbortController();
  let opened: OpenInput | null = null;
  // 选项写回输入框时我们也会派发 input/change；这次派发不应再打开浮层。
  let writing = false;

  const close = (): void => {
    const state = opened;
    if (!state) return;
    opened = null;
    state.restore();
    if (state.panel.matches(":popover-open")) state.panel.hidePopover();
    state.panel.remove();
  };

  const activate = (state: OpenInput, index: number, navigated = false, scroll = true): void => {
    if (!scroll && state.active === index) return;
    state.active = index;
    state.navigated = navigated;
    state.rows.forEach((row, i) => row.classList.toggle("is-active", i === index));
    if (index >= 0) state.input.setAttribute("aria-activedescendant", state.rows[index].id);
    else state.input.removeAttribute("aria-activedescendant");
    if (scroll) state.rows[index]?.scrollIntoView({ block: "nearest" });
  };

  const apply = (state: OpenInput, value: string): void => {
    const { input } = state;
    const previous = input.value;
    const separator = separatorOf(input);
    if (!separator) input.value = value;
    else input.value = `${segmentOf(input).prefix}${value}`;
    close();
    input.focus({ preventScroll: true });
    if (input.value !== previous) {
      writing = true;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new Event("change", { bubbles: true }));
      writing = false;
    }
  };

  const render = (state: OpenInput): void => {
    const { token } = state.queryOverride === null ? segmentOf(state.input) : { token: state.queryOverride };
    const query = token.toLocaleLowerCase();
    state.values = query ? state.all.filter(value => value.toLocaleLowerCase().includes(query)) : state.all.slice();
    state.rows = state.values.map((value, index) => {
      const row = doc.createElement("li");
      row.id = `${state.list.id}-${index}`;
      row.className = "searchable-select-option";
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", String(token === value));
      row.textContent = value;
      row.addEventListener("pointerdown", e => e.preventDefault());
      row.addEventListener("click", () => apply(state, value));
      row.addEventListener("pointermove", () => activate(state, index, false, false));
      return row;
    });
    state.list.replaceChildren(...state.rows);
    state.status.textContent = state.values.length ? "" : "没有匹配建议，可继续手动输入";
    state.status.hidden = state.values.length > 0;
    activate(state, state.values.length ? 0 : -1);
  };

  const position = (state: OpenInput): void => {
    const rect = state.input.getBoundingClientRect();
    const viewport = win.visualViewport;
    const left = viewport?.offsetLeft ?? 0, top = viewport?.offsetTop ?? 0;
    const width = viewport?.width ?? win.innerWidth, height = viewport?.height ?? win.innerHeight;
    const below = Math.max(0, top + height - rect.bottom - 12), above = Math.max(0, rect.top - top - 12);
    const upwards = below < 200 && above > below;
    // data-search-width="field"：格子内的可输入下拉要跟单元格同宽，不套 240 的最小浮层宽度。
    const sameWidth = state.input.getAttribute("data-search-width") === "field";
    const panelWidth = sameWidth ? Math.max(0, Math.min(rect.width, width - 16)) : Math.min(Math.max(rect.width, 240), width - 16);
    state.panel.style.width = `${Math.max(0, panelWidth)}px`;
    state.panel.style.maxHeight = `${Math.min(360, upwards ? above : below)}px`;
    state.panel.style.left = `${Math.max(left + 8, Math.min(rect.left, left + width - panelWidth - 8))}px`;
    state.panel.style.top = `${upwards ? rect.top - state.panel.getBoundingClientRect().height - 4 : rect.bottom + 4}px`;
  };

  const open = (input: HTMLInputElement, fromInput = false): void => {
    if (!available(input) || !input.getClientRects().length) return;
    if (opened?.input === input) { render(opened); position(opened); return; }
    close();
    const list = datalistOf(input);
    if (!list) return;
    const values = valuesOf(list);
    const panel = doc.createElement("div");
    panel.className = "searchable-select-popover searchable-input-popover";
    panel.popover = "manual";
    const ul = doc.createElement("ul");
    ul.id = `searchable-input-${++nextId}`;
    ul.className = "searchable-select-list";
    ul.setAttribute("role", "listbox");
    ul.setAttribute("aria-label", nameOf(input));
    const status = doc.createElement("div");
    status.className = "searchable-select-empty";
    status.setAttribute("role", "status");
    // 原生 datalist 弹层与浮层会叠在一起：先摘掉 list 属性，关闭时原样恢复。
    const savedList = input.getAttribute("list");
    const saved = ["role", "aria-expanded", "aria-controls", "aria-haspopup", "aria-autocomplete", "aria-activedescendant"]
      .map(attr => [attr, input.getAttribute(attr)] as const);
    const state: OpenInput = { input, panel, list: ul, status, all: values, values: [], rows: [], active: -1, navigated: false,
      // 单元格式下拉（表设计器默认值）打开时列全部候选：回显的表达式不适合当作筛选词。
      // 由打字触发的打开直接用当前输入过滤。
      queryOverride: !fromInput && input.hasAttribute("data-search-open-all") ? "" : null, savedList, composing: false,
      restore: () => {
        if (savedList !== null && !input.hasAttribute("list")) input.setAttribute("list", savedList);
        saved.forEach(([attr, value]) => { if (value === null) input.removeAttribute(attr); else input.setAttribute(attr, value); });
      } };
    opened = state;
    input.setAttribute("role", "combobox");
    input.setAttribute("aria-expanded", "true");
    input.setAttribute("aria-controls", ul.id);
    input.setAttribute("aria-haspopup", "listbox");
    input.setAttribute("aria-autocomplete", "list");
    if (savedList !== null) input.removeAttribute("list");
    panel.append(ul, status);
    // 与 SearchableSelect 一致：DOM 留在所属模态内，top-layer 避免被 overflow 裁切。
    (input.closest("dialog, [role='dialog'], .dialog") ?? doc.body).appendChild(panel);
    panel.addEventListener("toggle", () => { if (opened === state && !panel.matches(":popover-open")) close(); });
    try {
      panel.showPopover();
      render(state);
      position(state);
      // showPopover() 会把焦点移到浮层本身；浮层只是建议列表，焦点必须留在输入框里
      // （否则 focusout 会立刻关掉它，用户也没法继续打字）。
      input.focus({ preventScroll: true });
    } catch {
      close();
    }
  };

  const owner = (event: Event): HTMLInputElement | null => {
    const node = event.target;
    return node instanceof HTMLInputElement && root.contains(node) && usable(node) ? node : null;
  };

  /** 浮层的空白区与「没有匹配」提示也算点外部：它们常盖住下方相邻的单元格，
   *  不关掉的话用户无法直接点到被盖住的输入框。候选项与列表本身不算。 */
  const dismissTarget = (target: EventTarget | null): boolean => {
    const state = opened;
    if (!state || !(target instanceof Node)) return true;
    if (!state.panel.contains(target)) return true;
    return target === state.panel || (target instanceof Element && !!target.closest(".searchable-select-empty"));
  };

  root.addEventListener("pointerdown", e => {
    const input = owner(e);
    if (input && (e as PointerEvent).button === 0) {
      // 输入框仍是自由编辑的原生控件：重复点击/拖选文字不切换浮层，也不重置候选导航。
      if (opened?.input === input) return;
      if (available(input)) open(input);
      return;
    }
    if (opened && dismissTarget(e.target)) close();
  }, { capture: true, signal: listeners.signal });
  // 自由输入是主路径：不拦截输入，只在值变化后刷新建议。
  root.addEventListener("input", e => {
    const input = owner(e);
    if (!input || writing) return;
    if (opened?.input === input) { opened.queryOverride = null; render(opened); position(opened); }
    else if (input.value.trim() && available(input)) open(input, true);
  }, { capture: true, signal: listeners.signal });
  root.addEventListener("compositionstart", e => {
    if (opened?.input === e.target) opened.composing = true;
  }, { capture: true, signal: listeners.signal });
  root.addEventListener("compositionend", e => {
    if (opened?.input === e.target) { opened.composing = false; render(opened); }
  }, { capture: true, signal: listeners.signal });
  root.addEventListener("focusout", e => {
    if (!opened || e.target !== opened.input) return;
    // 点击浮层内选项会先触发 pointerdown(preventDefault)，焦点不会丢。
    const next = (e as FocusEvent).relatedTarget;
    if (next instanceof Node && opened.panel.contains(next)) return;
    close();
  }, { capture: true, signal: listeners.signal });
  doc.addEventListener("keydown", e => {
    const event = e as KeyboardEvent;
    const state = opened;
    if (!state) {
      const input = owner(e);
      if (input && available(input) && !event.isComposing && event.keyCode !== 229 &&
        !event.ctrlKey && !event.metaKey && !event.altKey && ["ArrowDown", "ArrowUp"].includes(event.key)) {
        e.preventDefault(); e.stopPropagation(); open(input);
      }
      return;
    }
    if (e.target !== state.input) return;
    if (state.composing || event.isComposing || event.keyCode === 229) { e.stopPropagation(); return; }
    if (event.key === "Tab") { close(); return; }
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    if (event.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); return; }
    if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Home" || event.key === "End") {
      e.preventDefault();
      const total = state.rows.length;
      if (!total) return;
      const next = event.key === "Home" ? 0 : event.key === "End" ? total - 1
        : event.key === "ArrowDown" ? (state.active + 1) % total : (state.active - 1 + total) % total;
      activate(state, next, true);
      return;
    }
    if (event.key === "Enter" && state.active >= 0) {
      // 自由输入框的 Enter 是提交，不应把用户的新表名/列名改成首个部分匹配项。
      // 只有用方向键/Home/End 明确挑选过候选，才采纳建议；鼠标点选仍直接采纳。
      const value = state.values[state.active];
      if (!state.navigated) return;
      e.preventDefault();
      e.stopPropagation();
      apply(state, value);
    }
  }, { capture: true, signal: listeners.signal });
  doc.addEventListener("pointerdown", e => {
    // root 是 document 时上面的处理器已经处理过输入框自身；这里只负责浮层之外与浮层空白区。
    if (opened && e.target !== opened.input && dismissTarget(e.target)) close();
  }, { capture: true, signal: listeners.signal });
  doc.addEventListener("scroll", e => {
    const state = opened;
    if (!state || (e.target instanceof Node && state.panel.contains(e.target))) return;
    // 容器滚动时跟随锚点，而不是直接关掉（对话框内容几乎都可滚动）。
    if (!usable(state.input) || !state.input.getClientRects().length) { close(); return; }
    const rect = state.input.getBoundingClientRect();
    const height = win.visualViewport?.height ?? win.innerHeight;
    if (rect.bottom < 0 || rect.top > height) { close(); return; }
    position(state);
  }, { capture: true, signal: listeners.signal });
  win.addEventListener("resize", () => { if (opened) position(opened); }, { signal: listeners.signal });
  win.visualViewport?.addEventListener("resize", () => { if (opened) position(opened); }, { signal: listeners.signal });
  win.visualViewport?.addEventListener("scroll", () => { if (opened) position(opened); }, { signal: listeners.signal });
  // datalist 选项被替换（元数据重新加载）时刷新建议；输入框被移除/禁用时关闭。
  const observer = new MutationObserver(records => {
    const state = opened;
    if (!state) return;
    if (!state.input.isConnected || !usable(state.input) || !state.input.getClientRects().length) { close(); return; }
    const list = state.input.ownerDocument.getElementById(state.savedList ?? "");
    if (records.some(record => list instanceof Node && (record.target === list || list.contains(record.target)))) {
      if (list instanceof HTMLDataListElement) state.all = valuesOf(list);
      else state.all = [];
      render(state);
    }
  });
  observer.observe(doc, { subtree: true, childList: true, attributes: true, attributeFilter: ["disabled", "list", "readonly"] });

  const dispose = (): void => { observer.disconnect(); listeners.abort(); close(); installations.delete(root); };
  installations.set(root, dispose);
  return dispose;
}
