import { openConnectionDialog, type ConnectionDialogResult } from "../../src/dialogs/ConnectionDialog";
import { openConnectionPicker } from "../../src/dialogs/ConnectionPickerDialog";
import { openDriverManager } from "../../src/dialogs/DriverManagerDialog";
import { encodeDriverParams, parseDriverParams, type DriverDefinition } from "../../src/core/api";
import type { ConnectionConfig } from "../../src/core/types";

const output = document.querySelector<HTMLPreElement>("#results")!;
const tick = async () => { await new Promise((resolve) => setTimeout(resolve, 0)); };
/** 异步 IPC 链路（保存/删除）需要比 tick 更长的等待，避免断言跑在回填之前。 */
const settle = async () => { await new Promise((resolve) => setTimeout(resolve, 40)); };
const find = <T extends Element>(element: Element, selector: string): T => {
  const value = element.querySelector<T>(selector); if (!value) throw new Error(`Missing ${selector}`); return value;
};
const button = (element: Element, label: string): HTMLButtonElement => {
  const found = [...element.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === label);
  if (!found) throw new Error(`Missing button ${label}`); return found;
};
const input = (element: Element, label: string): HTMLInputElement => find(element, `input[aria-label='${label}']`);
const assert = (condition: unknown, message: string) => { if (!condition) throw new Error(message); output.textContent += `PASS ${message}\n`; };
const existing: ConnectionConfig = { id: "isolated_test", name: "已有连接", kind: "mysql", host: "localhost", port: 3306,
  user: "root", password: "ENCRYPTED_EXISTING", database: "original", file: "", params: "", group: "保留分组", read_only: false, keep_alive: true };

async function run() {
  output.textContent = "";
  let drivers: DriverDefinition[] = [];
  const tests: unknown[] = [];
  (window as any).__PREVIEW_MOCK__ = {
    list_custom_drivers: () => [...drivers],
    list_driver_statuses: () => [],
    list_odbc_inventory: () => ({ architecture: "x86_64", drivers: [{ name: "ODBC Driver 17 for SQL Server", library: "" }], dsns: [{ name: "TestDSN", driver: "SQL Server" }] }),
    save_custom_driver: ({ driver }: { driver: DriverDefinition }) => { drivers = [...drivers.filter((d) => d.id !== driver.id), driver]; return [...drivers]; },
    delete_custom_driver: ({ id }: { id: string }) => { drivers = drivers.filter((d) => d.id !== id); return [...drivers]; },
    test_connection_with_saved_password: (args: unknown) => { tests.push(args); return { ok: true, message: "mock only", version: "", charset: "" }; },
    test_custom_driver: () => ({ ok: true, message: "mock only", version: "1", charset: "" }),
  };
  let result: ConnectionDialogResult | undefined;
  let dlg = openConnectionDialog(existing, ["保留分组"], (value) => { result = value; });
  const database = input(dlg.el, "数据库");
  assert(database.isConnected && !database.closest<HTMLElement>("[role='tabpanel']")!.hidden, "database input is visible in general panel");
  database.value = "edited_db"; database.dispatchEvent(new Event("input", { bubbles: true }));
  button(dlg.el, "高级").click(); button(dlg.el, "常规").click();
  assert(input(dlg.el, "数据库") === database && database.value === "edited_db", "database identity/value survives tab switches");
  assert(!dlg.el.textContent?.includes("分组:"), "group selector removed");
  button(dlg.el, "测试连接").click(); await tick();
  assert((tests[0] as any).useSavedPassword === true, "editing test uses saved password without exposing plaintext");
  button(dlg.el, "保存").click(); await tick();
  assert(result?.password === undefined && result.config.password === existing.password, "blank unchanged password preserves ciphertext");
  assert(result?.config.group === existing.group && result.config.database === "edited_db", "existing grouping and edited database preserved");

  result = undefined; dlg = openConnectionDialog(existing, [], (value) => { result = value; });
  find<HTMLInputElement>(dlg.el, ".checkbox input").checked = false; button(dlg.el, "保存").click(); await tick();
  assert(result?.password === "" && result.config.password === "", "unchecked save password explicitly clears ciphertext");
  result = undefined; dlg = openConnectionDialog(existing, [], (value) => { result = value; });
  input(dlg.el, "密码").value = "replacement"; input(dlg.el, "密码").dispatchEvent(new Event("input", { bubbles: true }));
  button(dlg.el, "保存").click(); await tick(); assert(result?.password === "replacement", "changed password returned for encryption");

  // 驱动管理是「一个页面一个任务」：主界面只管托管驱动，自定义驱动从页脚次级入口进单页表单。
  const manager = openDriverManager(); await tick();
  button(manager.el, "添加自定义驱动…").click(); await tick();
  const backdrops = [...document.querySelectorAll<HTMLElement>(".dialog-backdrop")];
  const customDialog = backdrops[backdrops.length - 1].querySelector<HTMLElement>(".dialog")!;
  input(customDialog, "显示名称").value = "我的真实驱动"; input(customDialog, "驱动目标").value = "ODBC Driver 17 for SQL Server";
  button(customDialog, "保存驱动").click(); await settle();
  assert(drivers.length === 1 && drivers[0].name === "我的真实驱动", "driver manager persists custom definition through IPC");
  input(customDialog, "显示名称").value = "重命名驱动"; button(customDialog, "保存驱动").click(); await settle();
  assert(drivers.length === 1 && drivers[0].name === "重命名驱动", "driver edit updates existing ID");
  customDialog.querySelector<HTMLElement>(".dialog-title-close")?.click(); await tick();
  manager.close();
  let picked: DriverDefinition | undefined;
  const picker = openConnectionPicker((_kind, driver) => { picked = driver; }); await tick();
  button(picker.el, "自定义驱动").click();
  [...picker.el.querySelectorAll<HTMLElement>(".conn-kind-name")].find((el) => el.textContent === "重命名驱动")!.closest<HTMLButtonElement>("button")!.click();
  button(picker.el, "下一步").click(); await tick(); assert(picked?.id === drivers[0].id, "saved custom driver selectable and returned by picker");
  assert(!picker.el.textContent?.includes("JDBCX"), "unsupported JDBCX hidden from picker");
  result = undefined; dlg = openConnectionDialog(null, [], (value) => { result = value; }, "odbc", picked);
  input(dlg.el, "数据库").value = "custom_db"; button(dlg.el, "保存").click(); await tick();
  assert(result?.config.kind === "odbc" && parseDriverParams(result.config.params)?.driver_id === picked?.id, "custom connection persists real ODBC driver reference");
  assert(parseDriverParams(encodeDriverParams("id", "DSN={{dsn}}"))?.template === "DSN={{dsn}}", "driver params encoding roundtrip");
  output.textContent += "ALL 14 REGRESSION CHECKS PASSED (UI IPC mocked; native connection checked separately in Rust)\n";
}
document.querySelector("#run")!.addEventListener("click", () => { void run().catch((error) => { output.textContent += `FAIL ${String(error)}\n`; }); });
