import { api } from "./api";
import type { ConnectionConfig } from "./types";
import { field } from "../ui/Dialog";

export interface MetadataTablePicker { el: HTMLElement; select: HTMLSelectElement; value(): string; problem(): string | null; ready: Promise<void>; reload(): Promise<void>; }
let pickerId = 0;

export function createTablePicker(connections: readonly ConnectionConfig[], connection: HTMLSelectElement, schema: HTMLSelectElement, initial = "", prefix = ""): MetadataTablePicker {
  const input = document.createElement("select"); input.id = "table-picker-" + (++pickerId); input.setAttribute("aria-label", prefix + "表");
  const status = document.createElement("div"); status.id = input.id + "-status"; status.className = "form-note"; status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite"); input.setAttribute("aria-describedby", status.id);
  const control = document.createElement("div"); control.append(input, status); const el = field(prefix + "表:", control);
  let version = 0, loading = false, error = "", known = new Set<string>();
  const placeholder = (text: string): void => { input.replaceChildren(new Option(text, "")); input.value = ""; };
  const reload = async (preferred = ""): Promise<void> => {
    const request = ++version, connectionId = connection.value, schemaName = schema.value; known.clear(); error = ""; loading = true; input.disabled = true; input.dataset.metadataDisabled = "true"; el.setAttribute("aria-busy", "true"); placeholder("正在加载…"); status.textContent = "正在读取真实表元数据…";
    try {
      if (!connections.some(item => item.id === connectionId)) throw new Error("请选择实际存在的连接"); if (!schemaName) throw new Error("请先选择数据库或模式");
      const connected = await api.ensureConnection(connectionId); if (request !== version || connectionId !== connection.value || schemaName !== schema.value) return; if (!connected.ok) throw new Error(connected.message || "连接失败");
      const tables = await api.listTables(connectionId, schemaName); if (request !== version || connectionId !== connection.value || schemaName !== schema.value) return; placeholder("请选择表…");
      for (const [name, type] of tables) { if (!name || type !== "table" || known.has(name)) continue; known.add(name); input.add(new Option(name, name)); }
      if (preferred && known.has(preferred)) { input.value = preferred; status.textContent = ""; } else if (preferred) status.textContent = "旧值「" + preferred + "」不在当前表目录中，请重新选择。"; else status.textContent = known.size ? "请选择真实表。" : "当前数据库或模式没有可用表。";
    } catch (reason) { if (request !== version || connectionId !== connection.value || schemaName !== schema.value) return; error = "表元数据读取失败：" + String(reason); placeholder("读取失败"); status.textContent = error; }
    finally { if (request === version && connectionId === connection.value && schemaName === schema.value) { loading = false; input.disabled = Boolean(error) || !known.size; input.dataset.metadataDisabled = String(input.disabled); el.setAttribute("aria-busy", "false"); } }
  };
  input.addEventListener("change", () => { if (known.has(input.value)) status.textContent = ""; }); connection.addEventListener("change", () => { void reload(); }); schema.addEventListener("change", () => { void reload(); });
  const ready = reload(initial); return { el, select: input, ready, reload: () => reload(), value: () => known.has(input.value) ? input.value : "", problem: () => loading ? "表元数据正在加载，请稍候" : error || (!known.size ? "当前数据库或模式没有可用表" : !known.has(input.value) ? "请选择当前数据库或模式中真实存在的表" : null) };
}

export interface MetadataTableMultiPicker { el: HTMLElement; select: HTMLSelectElement; values(): string[]; problem(): string | null; ready: Promise<void>; reload(): Promise<void>; }
export function createTableMultiPicker(connections: readonly ConnectionConfig[], connection: HTMLSelectElement, schema: HTMLSelectElement, initial: readonly string[] = [], label = "表:", required = true): MetadataTableMultiPicker {
  const picker = createTablePicker(connections, connection, schema, ""); const select = picker.select; select.multiple = true; select.size = 4; picker.el.querySelector<HTMLElement>(".field-label")!.textContent = label;
  const reload = async (): Promise<void> => { await picker.reload(); const valid = new Set(Array.from(select.options, option => option.value).filter(Boolean)); for (const option of select.options) option.selected = valid.has(option.value) && initial.includes(option.value); };
  const ready = reload(); return { el: picker.el, select, ready, reload, values: () => Array.from(select.selectedOptions, option => option.value).filter(Boolean), problem: () => picker.problem() || (required && !select.selectedOptions.length ? "至少选择一张真实表" : null) };
}