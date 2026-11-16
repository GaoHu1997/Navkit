/**
 * 原生 select 的搜索增强。原 DOM、值、样式和消费者事件保持不变。
 * `data-no-search` 的下拉只列候选、点选即生效，不渲染搜索框（字段生成方式、任务回看等）。
 */
type SelectRoot = Document | HTMLElement;
interface OpenSelect {
  select: HTMLSelectElement;
  panel: HTMLDivElement;
  search: HTMLInputElement | null;
  list: HTMLUListElement;
  status: HTMLDivElement;
  options: HTMLOptionElement[];
  rows: HTMLLIElement[];
  active: number;
  composing: boolean;
  restore: () => void;
}

const installations = new WeakMap<SelectRoot, () => void>();
let nextId = 0;

/**
 * 可增强的范围：单选（普通下拉）与多选（列表框，`size` 任意）。
 * `size > 1` 且非 multiple 的原生列表框点击即选中，套浮层反而干扰，保持原样。
 */
function enhanceable(select: HTMLSelectElement): boolean {
  return select.multiple || select.size <= 1;
}

function available(select: HTMLSelectElement): boolean {
  return enhanceable(select) && !select.matches(":disabled") && !select.closest("[inert]") &&
    select.getAttribute("aria-busy") !== "true" && select.getAttribute("aria-disabled") !== "true";
}

function placeholder(select: HTMLSelectElement, option: HTMLOptionElement): boolean {
  return option.hidden || option.hasAttribute("data-placeholder") ||
    (option.value === "" && ((select.required && option === select.options[0]) ||
      /^[（(]?\s*(?:(?:请|先)?选择|加载中|正在加载|正在读取|读取中|loading\b|choose\b|select\b)/i.test(option.label)));
}

function disabled(option: HTMLOptionElement): boolean {
  return option.disabled || option.getAttribute("aria-disabled") === "true" ||
    (option.parentElement instanceof HTMLOptGroupElement && option.parentElement.disabled);
}

function nameOf(select: HTMLSelectElement): string {
  const labelled = select.getAttribute("aria-labelledby")?.split(/\s+/).map(id =>
    select.ownerDocument.getElementById(id)?.textContent?.trim() ?? "").filter(Boolean).join(" ");
  return labelled || select.getAttribute("aria-label")?.trim() ||
    Array.from(select.labels ?? []).map(label => {
      const copy = label.cloneNode(true) as HTMLLabelElement;
      copy.querySelectorAll("select, input, textarea, button, script, style").forEach(node => node.remove());
      return copy.textContent?.trim();
    }).filter(Boolean).join(" ") ||
    select.title.trim() ||
    select.closest(".field, .form-row, .field-row, .form-field")
      ?.querySelector(".field-label, .form-label, label")?.textContent?.trim() || "选择选项";
}

/**
 * 在入口安装一次；动态创建的 select 自动生效。重复安装同一 root 返回同一清理函数。
 * 清理函数关闭浮层、恢复增强添加的 ARIA 名称并解除所有监听/观察。
 * 不支持 HTML Popover 的 WebView 保留原生 select；非多选的 size > 1 列表框保持原生。
 */
export function installSearchableSelects(root: SelectRoot = document): () => void {
  const installed = installations.get(root);
  if (installed) return installed;
  const doc = root.ownerDocument ?? root as Document;
  const win = doc.defaultView;
  if (!win || typeof HTMLElement.prototype.showPopover !== "function") return () => {};
  const listeners = new AbortController();
  const named = new Map<HTMLSelectElement, { original: string | null; assigned: string }>();
  let opened: OpenSelect | null = null;

  const ensureName = (select: HTMLSelectElement): void => {
    if (!enhanceable(select) || select.getAttribute("aria-label")?.trim() ||
      select.getAttribute("aria-labelledby")?.trim() || select.labels?.length) return;
    const assigned = nameOf(select);
    named.set(select, { original: select.getAttribute("aria-label"), assigned });
    select.setAttribute("aria-label", assigned);
  };
  const restoreName = (select: HTMLSelectElement): void => {
    const saved = named.get(select);
    if (saved && select.getAttribute("aria-label") === saved.assigned) {
      if (saved.original === null) select.removeAttribute("aria-label");
      else select.setAttribute("aria-label", saved.original);
    }
    named.delete(select);
  };
  const scan = (node: Node): void => {
    if (node instanceof HTMLSelectElement) ensureName(node);
    if (node instanceof Element || node instanceof Document) {
      node.querySelectorAll<HTMLSelectElement>("select").forEach(ensureName);
    }
  };
  const close = (returnFocus = false): void => {
    const state = opened;
    if (!state) return;
    opened = null;
    state.restore();
    if (state.panel.matches(":popover-open")) state.panel.hidePopover();
    state.panel.remove();
    if (returnFocus && state.select.isConnected && !state.select.matches(":disabled")) {
      state.select.focus({ preventScroll: true });
    }
  };
  const activate = (state: OpenSelect, index: number, scroll = true): void => {
    if (!scroll && state.active === index) return;
    state.active = index;
    state.rows.forEach((row, i) => row.classList.toggle("is-active", i === index));
    // 无搜索的下拉没有输入框，键盘焦点与 aria-activedescendant 都由列表本身承担。
    const control = state.search ?? state.list;
    if (index >= 0) {
      control.setAttribute("aria-activedescendant", state.rows[index].id);
      if (scroll) state.rows[index].scrollIntoView({ block: "nearest" });
    } else control.removeAttribute("aria-activedescendant");
  };
  const choose = (state: OpenSelect, index: number): void => {
    if (opened !== state) return;
    const option = state.options[index];
    // 再读原生 DOM，阻止观察器尚未回调时确认失效/已移除的选项。
    if (!option || !root.contains(state.select) || !available(state.select) ||
      !Array.from(state.select.options).includes(option) || disabled(option) || placeholder(state.select, option)) {
      close(true); return;
    }
    if (state.select.multiple) {
      // 多选：点选即切换勾选，浮层保持打开，方便连续挑多个再关。
      option.selected = !option.selected;
      state.rows[index]?.setAttribute("aria-selected", String(option.selected));
      state.select.dispatchEvent(new Event("input", { bubbles: true }));
      state.select.dispatchEvent(new Event("change", { bubbles: true }));
      return;
    }
    const previous = state.select.value;
    state.select.selectedIndex = option.index;
    close(true);
    if (state.select.value !== previous) {
      state.select.dispatchEvent(new Event("input", { bubbles: true }));
      state.select.dispatchEvent(new Event("change", { bubbles: true }));
    }
  };
  const render = (state: OpenSelect): void => {
    const query = state.search?.value.toLocaleLowerCase() ?? "";
    state.options = Array.from(state.select.options).filter(option =>
      !placeholder(state.select, option) &&
      (option.label.toLocaleLowerCase().includes(query) || option.value.toLocaleLowerCase().includes(query)));
    state.rows = state.options.map((option, index) => {
      const row = doc.createElement("li");
      row.id = `${state.list.id}-${option.index}`;
      row.className = "searchable-select-option";
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", String(option.selected));
      row.setAttribute("aria-disabled", String(disabled(option)));
      row.textContent = option.label;
      if (state.select.multiple) {
        const check = doc.createElement("span");
        check.className = "searchable-select-check";
        check.setAttribute("aria-hidden", "true");
        check.textContent = "✓";
        row.prepend(check);
      }
      const group = option.parentElement instanceof HTMLOptGroupElement ? option.parentElement.label : "";
      row.title = group ? `${group} · ${option.label}` : option.label;
      row.addEventListener("pointerdown", e => e.preventDefault());
      row.addEventListener("click", () => { if (!disabled(option)) choose(state, index); });
      row.addEventListener("pointermove", () => { if (!disabled(option)) activate(state, index, false); });
      return row;
    });
    state.list.replaceChildren(...state.rows);
    state.status.textContent = state.options.length ? "" : "没有匹配选项，请修改搜索词";
    state.status.hidden = state.options.length > 0;
    const selected = state.options.findIndex(option => option.selected && !disabled(option));
    activate(state, selected >= 0 ? selected : state.options.findIndex(option => !disabled(option)));
  };
  const move = (state: OpenSelect, key: string): void => {
    const indices = state.options.flatMap((option, i) => disabled(option) ? [] : [i]);
    const index = indices.indexOf(state.active);
    const next = key === "Home" ? indices[0] : key === "End" ? indices[indices.length - 1] :
      key === "ArrowDown" ? indices[Math.min(index + 1, indices.length - 1)] : indices[Math.max(0, index - 1)];
    activate(state, next ?? -1);
  };
  const place = (state: OpenSelect): void => {
    const rect = state.select.getBoundingClientRect();
    const viewport = win.visualViewport;
    const left = viewport?.offsetLeft ?? 0, top = viewport?.offsetTop ?? 0;
    const width = viewport?.width ?? win.innerWidth, height = viewport?.height ?? win.innerHeight;
    const below = Math.max(0, top + height - rect.bottom - 12), above = Math.max(0, rect.top - top - 12);
    const upwards = below < 200 && above > below;
    const panelWidth = Math.min(Math.max(rect.width, 240), width - 16);
    state.panel.style.width = `${Math.max(0, panelWidth)}px`;
    state.panel.style.maxHeight = `${Math.min(360, upwards ? above : below)}px`;
    state.panel.style.left = `${Math.max(left + 8, Math.min(rect.left, left + width - panelWidth - 8))}px`;
    state.panel.style.top = `${upwards ? rect.top - state.panel.getBoundingClientRect().height - 4 : rect.bottom + 4}px`;
  };
  const open = (select: HTMLSelectElement, query = "", initialKey?: string): void => {
    if (!available(select) || !select.getClientRects().length) return;
    close();
    ensureName(select);
    // 简单下拉：选项少、无需过滤时用 data-no-search 关掉搜索框，点开即选。
    const plain = select.hasAttribute("data-no-search");
    const panel = doc.createElement("div");
    panel.className = "searchable-select-popover";
    panel.popover = "manual";
    const list = doc.createElement("ul");
    list.id = `searchable-select-${++nextId}`;
    list.className = "searchable-select-list";
    list.setAttribute("role", "listbox");
    if (select.multiple) list.setAttribute("aria-multiselectable", "true");
    list.setAttribute("aria-label", nameOf(select));
    const search = plain ? null : doc.createElement("input");
    if (search) {
      search.type = "search";
      search.className = "searchable-select-search";
      search.autocomplete = "off";
      search.placeholder = "输入搜索…";
      search.value = query;
      search.setAttribute("aria-label", `搜索${nameOf(select)}`);
      search.setAttribute("role", "combobox");
      search.setAttribute("aria-autocomplete", "list");
      search.setAttribute("aria-expanded", "true");
      search.setAttribute("aria-haspopup", "listbox");
      search.setAttribute("aria-controls", list.id);
    }
    // 没有搜索框时把焦点交给列表本身，方向键/Enter/Escape 仍按原交互工作。
    else list.tabIndex = -1;
    const status = doc.createElement("div");
    status.className = "searchable-select-empty";
    status.setAttribute("role", "status");
    const saved = ["aria-expanded", "aria-controls", "aria-haspopup"].map(attr => [attr, select.getAttribute(attr)] as const);
    const state: OpenSelect = { select, panel, search, list, status, options: [], rows: [], active: -1,
      composing: false, restore: () => saved.forEach(([attr, value]) => {
        if (value === null) select.removeAttribute(attr); else select.setAttribute(attr, value);
      }) };
    opened = state;
    select.setAttribute("aria-expanded", "true");
    select.setAttribute("aria-controls", list.id);
    select.setAttribute("aria-haspopup", "listbox");
    panel.append(...(search ? [search] : []), list, status);
    // 浮层处于 top-layer：滚轮若交给浏览器会落到背后的对话框内容上，表现为滚动穿透。
    // 这里始终截住滚轮，只滚动候选列表。
    panel.addEventListener("wheel", event => {
      const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? list.clientHeight : 1;
      const max = list.scrollHeight - list.clientHeight;
      if (max > 0) list.scrollTop = Math.min(max, Math.max(0, list.scrollTop + event.deltaY * unit));
      event.preventDefault();
    }, { passive: false });
    // top-layer 避免 overflow 裁切；DOM 留在所属模态中，以兼容现有焦点陷阱。
    (select.closest("dialog, [role='dialog'], .dialog") ?? doc.body).appendChild(panel);
    if (search) {
      search.addEventListener("input", () => render(state));
      search.addEventListener("compositionstart", () => { state.composing = true; });
      search.addEventListener("compositionend", () => { state.composing = false; render(state); });
    }
    panel.addEventListener("toggle", () => { if (opened === state && !panel.matches(":popover-open")) close(); });
    try {
      panel.showPopover();
      render(state);
      if (initialKey === "Home" || initialKey === "End") move(state, initialKey);
      place(state);
      (search ?? list).focus({ preventScroll: true });
    } catch {
      close(true);
    }
  };
  const selectTarget = (event: Event): HTMLSelectElement | null => {
    // 多选/列表框里点击命中的是 <option>，所以从目标向上找 select。
    const node = event.target;
    const target = node instanceof Element ? node.closest("select") : null;
    return target instanceof HTMLSelectElement && root.contains(target) && enhanceable(target) ? target : null;
  };
  root.addEventListener("pointerdown", e => {
    const event = e as PointerEvent;
    const select = selectTarget(e);
    if (select && available(select) && event.button === 0) {
      e.preventDefault();
      if (opened?.select === select) close(true); else open(select);
    } else if (opened && !(e.target instanceof Node && opened.panel.contains(e.target))) close();
  }, { capture: true, signal: listeners.signal });
  // 阻止原生鼠标弹层；保留辅助技术/脚本触发的 click。
  root.addEventListener("mousedown", e => {
    const select = selectTarget(e);
    if (select && available(select) && (e as MouseEvent).button === 0) e.preventDefault();
  }, { capture: true, signal: listeners.signal });
  root.addEventListener("click", e => {
    const select = selectTarget(e);
    if (!select || !available(select)) return;
    e.preventDefault();
    if ((e as MouseEvent).detail === 0 && opened?.select !== select) open(select);
  }, { capture: true, signal: listeners.signal });
  doc.addEventListener("keydown", e => {
    const event = e as KeyboardEvent;
    const state = opened;
    if (state && e.target instanceof Node && state.panel.contains(e.target)) {
      if (state.composing || event.isComposing || event.keyCode === 229) {
        // 不干预 IME 默认行为，但也不让模态 Enter/Escape 消费候选确认。
        e.stopPropagation(); return;
      }
      if (event.key === "Tab") { close(true); return; }
      if (["ArrowDown", "ArrowUp", "Home", "End", "Enter", "Escape"].includes(event.key)) {
        e.preventDefault(); e.stopPropagation();
        if (event.key === "Escape") close(true);
        else if (event.key === "Enter") { if (state.active >= 0) choose(state, state.active); }
        else move(state, event.key);
      }
      return;
    }
    const select = selectTarget(e);
    if (!select || !available(select) || event.isComposing || event.keyCode === 229 || event.ctrlKey || event.metaKey) return;
    const openingKey = ["ArrowDown", "ArrowUp", "Home", "End", "Enter", " "].includes(event.key);
    // 无搜索的下拉不因字符键打开浮层，保留原生 type-ahead。
    const character = event.key.length === 1 && !event.altKey && !select.hasAttribute("data-no-search");
    if (openingKey || character) {
      e.preventDefault(); e.stopPropagation();
      open(select, character && event.key !== " " ? event.key : "", event.key);
    }
  }, { capture: true, signal: listeners.signal });
  doc.addEventListener("focusin", e => {
    if (opened && e.target !== opened.select && e.target instanceof Node && !opened.panel.contains(e.target) && !opened.select.contains(e.target)) close();
  }, { signal: listeners.signal });
  // root 可是局部容器；外部点击仍应关闭。多选列表框里点击命中的是 <option>，用 contains 判断。
  doc.addEventListener("pointerdown", e => {
    if (opened && e.target instanceof Node && !opened.panel.contains(e.target) && !opened.select.contains(e.target)) close();
  }, { capture: true, signal: listeners.signal });
  doc.addEventListener("scroll", e => {
    const state = opened;
    if (!state || (e.target instanceof Node && state.panel.contains(e.target))) return;
    // 容器滚动时让浮层跟随锚点，而不是直接关掉（对话框内容几乎都可滚动）。
    if (!state.select.isConnected || !state.select.getClientRects().length) { close(true); return; }
    const rect = state.select.getBoundingClientRect();
    const height = win.visualViewport?.height ?? win.innerHeight;
    if (rect.bottom < 0 || rect.top > height) { close(true); return; }
    place(state);
  }, { capture: true, signal: listeners.signal });
  win.addEventListener("resize", () => { if (opened) place(opened); }, { signal: listeners.signal });
  win.visualViewport?.addEventListener("resize", () => { if (opened) place(opened); }, { signal: listeners.signal });
  win.visualViewport?.addEventListener("scroll", () => { if (opened) place(opened); }, { signal: listeners.signal });
  const observer = new MutationObserver(records => {
    const state = opened;
    if (state && (!state.select.isConnected || !root.contains(state.select) || !available(state.select) ||
      !state.select.getClientRects().length || records.some(record =>
      (record.target === state.select && !["aria-label", "aria-labelledby"].includes(record.attributeName ?? "")) ||
      state.select.contains(record.target) && record.target !== state.select ||
      Array.from(record.removedNodes).some(node => node.contains(state.select))))) close();
    for (const record of records) {
      record.addedNodes.forEach(node => {
        if (root.contains(node)) scan(node);
        else if (node.contains(root)) scan(root);
      });
      if (record.target instanceof HTMLSelectElement && root.contains(record.target)) {
        if (enhanceable(record.target)) ensureName(record.target); else restoreName(record.target);
      }
    }
    for (const select of named.keys()) if (!select.isConnected || !root.contains(select)) restoreName(select);
    if (opened) {
      const name = nameOf(opened.select);
      if (opened.search && opened.search.getAttribute("aria-label") !== `搜索${name}`) opened.search.setAttribute("aria-label", `搜索${name}`);
      if (opened.list.getAttribute("aria-label") !== name) opened.list.setAttribute("aria-label", name);
    }
  });
  scan(root);
  observer.observe(doc, { subtree: true, childList: true, characterData: true, attributes: true,
    attributeFilter: ["disabled", "inert", "multiple", "size", "hidden", "open", "style", "class", "label", "value", "selected", "required",
      "aria-busy", "aria-disabled", "aria-label", "aria-labelledby", "data-placeholder"] });
  const dispose = (): void => {
    observer.disconnect(); listeners.abort(); close(true);
    for (const select of named.keys()) restoreName(select);
    installations.delete(root);
  };
  installations.set(root, dispose);
  return dispose;
}
