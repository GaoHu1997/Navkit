/** 表数据筛选与查询构建共用的无边框条件控件。 */
export type FilterJoin = "AND" | "OR";
export interface FilterValueState { op: string; val: string; touched?: boolean }

export function filterJoinControl(value: FilterJoin, onChange: (value: FilterJoin) => void, label = "切换条件连接方式"): HTMLButtonElement {
  const button = document.createElement("button"); button.type = "button";
  button.className = "fb-join-text"; button.textContent = value;
  button.title = "点击切换 AND / OR";
  button.setAttribute("aria-label", label + "，当前 " + value);
  button.addEventListener("click", () => {
    const next = button.textContent === "AND" ? "OR" : "AND";
    button.textContent = next; button.setAttribute("aria-label", label + "，当前 " + next); onChange(next);
  });
  return button;
}

/** 点击值进入编辑，Enter 确认、失焦确认、Escape 恢复，兼容中文输入法。 */
export function filterValueControl(state: FilterValueState, label: string, onCommit: (enter: boolean) => void): HTMLButtonElement {
  const button = document.createElement("button"); button.type = "button";
  button.className = "fb-txt fb-val";
  button.textContent = state.val || (state.touched ? '""' : "<?>");
  if (!state.val) button.classList.add("empty");
  if (/null/i.test(state.op)) button.style.display = "none";
  button.setAttribute("aria-label", label);
  const inList = /(^|\s)in$/i.test(state.op.trim());
  button.title = inList ? "点击输入值（IN / NOT IN 使用逗号分隔多个值，如 1, 2, 3）" : "点击输入值（LIKE 可用 % 与 _ 通配；空字符串也是有效值）";
  button.addEventListener("click", () => {
    const input = document.createElement("input"); input.type = "text"; input.className = "fb-edit";
    input.setAttribute("aria-label", label); input.value = state.val; input.maxLength = 8192;
    input.placeholder = inList ? "逗号分隔多个值" : "<?>";
    button.replaceWith(input); input.focus();
    let done = false, edited = false, composing = false;
    input.addEventListener("compositionstart", () => { composing = true; });
    input.addEventListener("compositionend", () => { composing = false; });
    input.addEventListener("input", () => { edited = true; });
    const finish = (apply: boolean, enter = false) => {
      if (done) return; done = true;
      if (apply) { state.val = input.value; if (edited || enter || input.value) state.touched = true; }
      // 失焦只替换值控件，不能重绘条件行吞掉鼠标正在点击的加号或运算符。
      const display = filterValueControl(state, label, onCommit); input.replaceWith(display);
      if (!apply || enter) display.focus({ preventScroll: true });
      if (apply) onCommit(enter);
    };
    input.addEventListener("keydown", event => {
      event.stopPropagation();
      if (composing || event.isComposing || event.keyCode === 229) return;
      if (event.key === "Enter") { event.preventDefault(); finish(true, true); }
      if (event.key === "Escape") { event.preventDefault(); finish(false); }
    });
    input.addEventListener("blur", () => finish(true));
  });
  return button;
}
