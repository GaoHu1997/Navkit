/** Real namespace metadata shared by legacy schedules and typed tool templates. */
import { api } from "./api";
import { namespaceLabel } from "./namespace-label";
import type { ConnectionConfig } from "./types";
import { field } from "../ui/Dialog";
export { namespaceLabel };
export interface DatabasePicker {
  el: HTMLElement; select: HTMLSelectElement; value(): string; problem(): string | null;
  ready: Promise<void>; reload(): Promise<void>;
}
let pickerId = 0;
export function createDatabasePicker(connections: readonly ConnectionConfig[], connection: HTMLSelectElement, initial = "", prefix = ""): DatabasePicker {
  const input = document.createElement("select"); input.id = `database-picker-${++pickerId}`;
  const status = document.createElement("div"); status.id = `${input.id}-status`;
  status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite"); status.className = "form-note";
  input.setAttribute("aria-describedby", status.id);
  const control = document.createElement("div"); control.append(input, status);
  const el = field("", control), label = el.querySelector<HTMLElement>(".field-label")!;
  let version = 0, loading = false, error = "", known = new Set<string>();
  function placeholder(text: string): void { const option = document.createElement("option"); option.value = ""; option.textContent = text; input.replaceChildren(option); input.value = ""; }
  const reload = async (preferred = ""): Promise<void> => {
    const requestVersion = ++version, id = connection.value;
    known.clear(); error = ""; loading = true; input.disabled = true; input.dataset.metadataDisabled = "true";
    el.setAttribute("aria-busy", "true"); placeholder("正在加载…"); status.textContent = "正在读取真实数据库元数据…";
    const cfg = connections.find(c => c.id === id), title = `${prefix}${namespaceLabel(cfg)}`;
    label.textContent = `${title}:`; input.setAttribute("aria-label", title);
    try {
      if (!cfg) throw new Error("请选择实际存在的连接");
      const connected = await api.ensureConnection(id);
      if (requestVersion !== version || id !== connection.value) return;
      if (!connected.ok) throw new Error(connected.message || "连接失败");
      const databases = await api.listDatabases(id);
      if (requestVersion !== version || id !== connection.value) return;
      placeholder("请选择…");
      for (const [name] of databases) {
        if (!name || known.has(name)) continue;
        known.add(name); const option = document.createElement("option"); option.value = name; option.textContent = name; input.appendChild(option);
      }
      if (preferred && known.has(preferred)) { input.value = preferred; status.textContent = ""; }
      else if (preferred) status.textContent = `旧值「${preferred}」不在当前真实目录中，请重新选择。`;
      else status.textContent = known.size ? "请选择真实数据库或模式。" : "当前连接没有可用数据库或模式。";
    } catch (reason) {
      if (requestVersion !== version || id !== connection.value) return;
      error = `数据库元数据读取失败：${String(reason)}`; placeholder("读取失败"); status.textContent = error;
    } finally {
      if (requestVersion === version && id === connection.value) {
        loading = false; input.disabled = Boolean(error) || !known.size; input.dataset.metadataDisabled = String(input.disabled); el.setAttribute("aria-busy", "false");
      }
    }
  };
  input.addEventListener("change", () => { if (known.has(input.value)) status.textContent = ""; });
  connection.addEventListener("change", () => { void reload(); });
  const ready = reload(initial);
  return { el, select: input, ready, reload: () => reload(), value: () => known.has(input.value) ? input.value : "",
    problem: () => loading ? "数据库元数据正在加载，请稍候" : error || (!known.size ? "当前连接没有可用数据库或模式" : !known.has(input.value) ? "请选择当前连接真实存在的数据库或模式" : null) };
}
