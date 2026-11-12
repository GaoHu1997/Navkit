import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { randomUUID } from "node:crypto";
import fs from "node:fs";
import vm from "node:vm";
import ts from "typescript";

// Execute production modules; only desktop invoke and DOM painting are fixtures.
// No database, browser-layout, or full desktop acceptance claim is made here.
async function load(contents, plugins = []) {
  const bundle = await build({ stdin: { contents, resolveDir: process.cwd(), loader: "ts" }, bundle: true,
    platform: "node", format: "esm", target: "es2021", write: false, loader: { ".css": "empty" }, plugins, logLevel: "silent" });
  return import(`data:text/javascript;base64,` + Buffer.from(bundle.outputFiles[0].text).toString("base64"));
}
const { IpcError, asIpcError, formatError, ExecutionController, api, queryResultApi, Dialog, Wizard, createDatasourceSyncTab } = await load(`
export { IpcError, asIpcError, formatError, ExecutionController } from './src/core/execution';
export { api } from './src/core/api';
export { queryResultApi } from './src/core/query-result-api';
export { Dialog } from './src/ui/Dialog';
export { Wizard } from './src/ui/Wizard';
export { createDatasourceSyncTab } from './src/dialogs/DatasourceSyncTab';
`);
const message = "error returned from database: 1305 (42000): PROCEDURE fn_add10 does not exist";
const structured = () => ({ message, code: "DATABASE_ERROR", operation_id: randomUUID(), retryable: true });
const context = () => ({ execution_id: randomUUID(), connection_id: "c1", owner_id: "tab1", namespace: "main", result_index: 0 });
const tick = () => new Promise(resolve => setTimeout(resolve, 0));
function restoreWindow(t, value) { const before = globalThis.window; globalThis.window = value; t.after(() => { globalThis.window = before; }); }

function assertDiagnostic(error, raw, command) {
  assert(error instanceof IpcError); assert.equal(error.name, "IpcError"); assert.equal(error.message, raw.message);
  assert.equal(error.code, raw.code); assert.equal(error.operation_id, raw.operation_id); assert.equal(error.retryable, raw.retryable);
  assert.equal(error.command, command); assert.equal(error.cause, raw);
  assert.equal(String(error), formatError(error)); assert(!String(error).startsWith("IpcError:"));
  for (const part of [message, raw.code, raw.operation_id, "retryable=true", command].filter(Boolean)) assert(String(error).includes(part));
  return true;
}
test("IpcError hides wrapper name only in text and retains full diagnostics, cause and stack", () => {
  const raw = structured(), error = new IpcError(raw, "get_definition");
  assertDiagnostic(error, raw, "get_definition"); assert.match(error.stack, /IpcError:/);
  assert(!JSON.stringify(error).includes('"cause"')); assert.equal(Object.getOwnPropertyDescriptor(error, "cause").enumerable, false);
});
test("repeated formatting and wrapping do not duplicate metadata or overwrite original command", () => {
  const raw = structured(), error = asIpcError(raw, "get_definition"), text = String(error);
  assert.equal(asIpcError(error), error); assert.equal(asIpcError(error, "execution_status"), error);
  const wrapped = new IpcError(error, "outer_boundary"); assert.equal(wrapped.message, raw.message); assert.equal(wrapped.command, "get_definition");
  assert.equal(formatError(wrapped), text); assert.equal(text.split("code=").length, 2); assert.equal(text.split("operation_id=").length, 2);
});
test("legacy strings, native Error, and encoded structured errors preserve original messages", () => {
  assert.equal(new IpcError(message).message, message); assert.equal(formatError(message), message);
  const native = new Error(message), wrapped = asIpcError(native); assert.equal(wrapped.message, message); assert.equal(wrapped.cause, native);
  assert.equal(formatError(native), message);
  const raw = structured(), encoded = JSON.stringify(raw), error = asIpcError(encoded);
  assert.equal(error.message, message); assert.equal(error.operation_id, raw.operation_id); assert.equal(error.retryable, true); assert.equal(error.cause, encoded);
  assert.equal(new IpcError('{"message":"x"}').message, '{"message":"x"}');
});
test("malformed fields cannot become codes, retry permissions or leaked unknown payloads", () => {
  const raw = { message, code: { secret: "bad" }, operation_id: ["fake"], retryable: "true", password: "private" };
  const error = new IpcError(raw); assert.equal(error.code, "APPLICATION_ERROR"); assert.equal(error.operation_id, undefined); assert.equal(error.retryable, false);
  assert(!formatError(error).includes("[object Object]")); assert(!formatError(error).includes("private"));
  assert.equal(formatError({ password: "private" }), "后端返回了无法识别的错误");
  const hostile = new Proxy({}, { get() { throw new Error("getter must not escape"); } }); assert.doesNotThrow(() => new IpcError(hostile)); assert.doesNotThrow(() => formatError(hostile));
  assert.equal(new IpcError({ message, code: "", operation_id: null, retryable: false }).operation_id, null);
});
test("native API refusal remains rejected, keeps command and never uses preview or empty data", async t => {
  const raw = structured(); let fixtures = 0, calls = 0;
  restoreWindow(t, { __PREVIEW_MOCK__: { list_connections() { fixtures++; return []; } }, __TAURI_INTERNALS__: { async invoke(command) { calls++; assert.equal(command, "list_connections"); throw raw; } } });
  await assert.rejects(api.listConnections(), error => assertDiagnostic(error, raw, "list_connections")); assert.equal(fixtures, 0); assert.equal(calls, 1);
});
test("explicit preview sync and async refusals use the same error contract", async t => {
  const raw = structured(); restoreWindow(t, { __PREVIEW_MOCK__: { list_connections() { throw raw; } } });
  await assert.rejects(api.listConnections(), error => assertDiagnostic(error, raw, "list_connections"));
  window.__PREVIEW_MOCK__.list_connections = async () => { throw raw; };
  await assert.rejects(api.listConnections(), error => assertDiagnostic(error, raw, "list_connections"));
});
test("legacy SQL preview failed outcome keeps operation_id and retryable rather than inventing success", async t => {
  const raw = structured(); restoreWindow(t, { __PREVIEW_MOCK__: { async execute_sql() { throw raw; } } });
  const req = { ...context(), sql: "SELECT fn_add10(1)", timeout_ms: 0 }, outcome = await api.executeSqlV2(req);
  assert.equal(outcome.state, "failed"); assert.equal(outcome.result, null); assert.equal(outcome.effects, "preview_fixture"); assert.deepEqual(outcome.error, raw);
});
test("query snapshot, preview and release refusals propagate full diagnostics without replacing rows", async t => {
  const raw = structured(), commands = [];
  restoreWindow(t, { __TAURI_INTERNALS__: { async invoke(command) { commands.push(command); throw raw; } } });
  const ctx = context(), page = { columns: [], offset: 0, loaded_rows: 0, total_rows: 0 };
  await assert.rejects(queryResultApi.snapshot(ctx, page), error => assertDiagnostic(error, raw, "query_result_snapshot"));
  await assert.rejects(queryResultApi.preview(ctx, { changes: [], operation: "update", result_token: randomUUID(), row_token: randomUUID() }), error => assertDiagnostic(error, raw, "preview_query_row_mutation"));
  await assert.rejects(queryResultApi.release(ctx), error => assertDiagnostic(error, raw, "release_query_result"));
  assert.deepEqual(page, { columns: [], offset: 0, loaded_rows: 0, total_rows: 0 }); assert.equal(commands.length, 3);
  await assert.rejects(queryResultApi.snapshot({ ...ctx, execution_id: "bad" }, page)); assert.equal(commands.length, 3);
});
test("status read failure remains observable without changing execution facts, unlocking or retrying SQL", async t => {
  const timers = [], logs = []; t.mock.method(globalThis, "setTimeout", callback => { timers.push(callback); return timers.length; });
  t.mock.method(globalThis, "clearTimeout", () => {}); t.mock.method(console, "error", (...args) => logs.push(args));
  const raw = structured(), req = { ...context(), sql: "SELECT 1", timeout_ms: 0 }, states = []; let finish, reads = 0, healthy = false, sends = 0;
  const source = { executeSqlV2() { sends++; return new Promise(resolve => { finish = resolve; }); }, async cancelExecution() { throw new Error("not requested"); },
    async executionStatus(ctx) { reads++; if (!healthy) throw raw; return { execution_id: ctx.execution_id, state: "unknown", resources_pending: false }; } };
  const controller = new ExecutionController(source, state => states.push(state)); t.after(() => controller.dispose());
  const run = controller.run(req); await timers.shift()(); await timers.shift()();
  assertDiagnostic(controller.statusError, raw, "execution_status"); assert.equal(logs.length, 1); assert.equal(reads, 2); assert.equal(controller.busy, true); assert.deepEqual(states, ["queued"]); assert.equal(sends, 1);
  finish({ execution_id: req.execution_id, state: "unknown", result: null, error: raw, resources_pending: true }); await run;
  healthy = true; await timers.shift()(); assert.equal(controller.statusError, null); assert.equal(controller.state, "unknown"); assert.equal(controller.busy, false); assert.equal(sends, 1); assert(!states.includes("succeeded"));
});

class Element {
  constructor(tag, doc) { this.tagName = tag; this.ownerDocument = doc; this.children = []; this.listeners = new Map(); this.attributes = new Map(); this.style = {}; this.dataset = {}; this.className = ""; this.hidden = false; this.disabled = false; this.isConnected = true; this._text = "";
    this.classList = { toggle: () => {}, add: () => {}, remove: () => {} }; }
  set textContent(value) { this._text = String(value); this.children = []; } get textContent() { return this._text + this.children.map(x => x.textContent).join(""); }
  set innerHTML(value) { this._html = value; this.children = []; } get innerHTML() { return this._html ?? ""; }
  append(...nodes) { nodes.forEach(node => { node.parent = this; this.children.push(node); }); } appendChild(node) { this.append(node); return node; }
  replaceChildren(...nodes) { this.children = []; this._text = ""; this.append(...nodes); }
  insertBefore(node, before) { node.parent = this; const i = this.children.indexOf(before); this.children.splice(i < 0 ? this.children.length : i, 0, node); }
  remove() { this.isConnected = false; if (this.parent) this.parent.children = this.parent.children.filter(x => x !== this); }
  setAttribute(name, value) { this.attributes.set(name, String(value)); } getAttribute(name) { return this.attributes.get(name); }
  addEventListener(name, fn) { const listeners = this.listeners.get(name) ?? []; listeners.push(fn); this.listeners.set(name, listeners); }
  querySelector(selector) { if (selector === "pre") return this.pre; return this.children.find(x => selector.startsWith(".") ? x.className.split(" ").includes(selector.slice(1)) : x.tagName === selector) ?? this.children.map(x => x.querySelector(selector)).find(Boolean) ?? null; }
  focus() { this.ownerDocument.activeElement = this; }
  async click() { if (!this.disabled) for (const fn of this.listeners.get("click") ?? []) await fn({ target: this }); }
}
function dom(t) { const previous = globalThis.document; const doc = { createElement(tag) { return new Element(tag, doc); } }; doc.body = doc.createElement("body"); globalThis.document = doc; t.after(() => { globalThis.document = previous; }); return doc; }
test("Dialog catches async refusal, keeps dialog open and renders diagnostics as an accessible text node", async t => {
  dom(t); const logs = []; t.mock.method(console, "error", (...args) => logs.push(args)); const raw = structured(); raw.message += '<img src=x onerror="alert(1)">';
  const dlg = new Dialog({ title: "读取", render() {}, buttons: [{ label: "确认", async onClick() { throw asIpcError(raw, "get_definition"); } }] });
  await dlg.footer.children[0].click(); assert.equal(dlg.el.isConnected, true); const box = dlg.body.querySelector(".dialog-error"); assert.equal(box.getAttribute("role"), "alert");
  assert(box.textContent.includes(raw.message)); assert(box.textContent.includes(raw.operation_id)); assert.equal(box.children[0].innerHTML, ""); assert.equal(logs.length, 1); dlg.close();
});
test("Wizard event catches finish and validation failures while direct next() still rejects and eligibility stays locked", async t => {
  dom(t); t.mock.method(console, "error", () => {}); const raw = structured(); let eligible = true, calls = 0;
  const wizard = new Wizard({ steps: [{ label: "执行", render() {} }], finishEligible: () => eligible, async onFinish() { calls++; eligible = false; throw asIpcError(raw, "run_export"); } });
  await wizard.nextBtn.click(); await tick(); assert.equal(calls, 1); assert.equal(wizard.current, 0); assert.equal(wizard.errorEl.hidden, false); assert(wizard.errorEl.textContent.includes(raw.operation_id)); assert.equal(wizard.nextBtn.disabled, true);
  eligible = true; wizard.refreshControls(); await assert.rejects(wizard.next(), e => e.code === raw.code); assert.equal(wizard.nextBtn.disabled, true);
  const validation = new Wizard({ steps: [{ label: "检查", render() {}, validate() { throw raw; } }], onFinish() { assert.fail("must not finish"); } });
  await validation.nextBtn.click(); await tick(); assert(validation.errorEl.textContent.includes(raw.operation_id));
});
test("startup errors are text-safe and logged, never interpolated into HTML", async t => {
  const doc = dom(t), root = doc.createElement("div"); root.pre = doc.createElement("pre"); doc.getElementById = () => root;
  const raw = structured(); raw.message += '<img src=x onerror="alert(1)">'; globalThis.__IPC_STARTUP_ERROR__ = raw; t.after(() => { delete globalThis.__IPC_STARTUP_ERROR__; });
  const logs = []; t.mock.method(console, "error", (...args) => logs.push(args));
  await load("import './src/main';", [{ name: "startup-only", setup(builder) {
    builder.onResolve({ filter: /^\.\/core\/App$/ }, () => ({ path: "app", namespace: "fixture" }));
    builder.onLoad({ filter: /.*/, namespace: "fixture" }, () => ({ contents: "export class App { async init() { throw globalThis.__IPC_STARTUP_ERROR__; } }", loader: "js" }));
  } }]); await tick(); assert(root.pre.textContent.includes(raw.message)); assert(root.pre.textContent.includes(raw.operation_id)); assert(!root.innerHTML.includes(raw.message)); assert.equal(logs.length, 1);
});

// Extract actual production chooser callbacks, not hand-written replacements.
function chooserCallbacks(path) {
  const source = fs.readFileSync(path, "utf8"), ast = ts.createSourceFile(path, source, ts.ScriptTarget.ES2021, true), callbacks = [];
  function visit(node) { if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === "addEventListener") {
    const callback = node.arguments[1]; if (callback && ts.isArrowFunction(callback) && /await (?:pickFile|save)\(/.test(callback.getText(ast))) callbacks.push(callback.getText(ast));
  } ts.forEachChild(node, visit); } visit(ast); return callbacks;
}
test("all six audited file chooser callbacks show failures and keep existing paths; cancellation is not an error", async () => {
  let count = 0;
  for (const path of ["src/dialogs/ImportExportDialog.ts", "src/dialogs/BatchExportDialog.ts", "src/dialogs/ScheduleDialog.ts"]) for (const callback of chooserCallbacks(path)) {
    count++; const raw = structured(), notices = [], logs = [], file = { value: "previous.csv", dispatchEvent() {} }, errorBox = { hidden: true, textContent: "" };
    const sandbox = { console: { error: (...args) => logs.push(args) }, formatError, dlg: { showError: message => notices.push(message) }, fileInput: file, input: file, file, errorBox,
      formatSelect: { value: "csv" }, format: { value: "csv" }, EXPORT_FILE_FILTERS: { csv: {} }, exportPath: { ready: Promise.resolve(), refresh() { assert.fail("failed selection cannot refresh path"); } },
      path: { ready: Promise.resolve(), refresh() { assert.fail("failed selection cannot refresh path"); } }, destination: { ready: Promise.resolve(), refresh() { assert.fail("failed selection cannot refresh path"); } },
      useSave: true, title: "保存", filters: () => [], pickFile: async () => { throw raw; }, Event: class {}, refreshPreview: async () => {} };
    // Dynamic import is the desktop plugin boundary, fixture only.
    let source = callback.replace('await import("@tauri-apps/plugin-dialog")', 'await Promise.resolve({ save: pickFile })');
    const js = ts.transpileModule(`const callback = (` + source + `); callback;`, { compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None } }).outputText;
    const action = vm.runInNewContext(js, sandbox); await action(); assert.equal(file.value, "previous.csv"); assert.equal(logs.length, 1);
    const shown = notices[0] ?? errorBox.textContent; assert(shown.includes(raw.operation_id)); assert(shown.includes("retryable=true"));
    logs.length = 0; notices.length = 0; sandbox.pickFile = async () => null; await action(); assert.equal(file.value, "previous.csv"); assert.equal(logs.length, 0); assert.equal(notices.length, 0);
  }
  assert.equal(count, 6, "exact audited chooser scope must remain covered");
});
test("chooser helpers propagate desktop failure rather than converting it to null cancellation", async () => {
  for (const path of ["src/dialogs/ImportExportDialog.ts", "src/dialogs/ScheduleDialog.ts"]) {
    const source = fs.readFileSync(path, "utf8"), ast = ts.createSourceFile(path, source, ts.ScriptTarget.ES2021, true);
    const helper = ast.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === "pickFile"); assert(helper);
    const raw = structured(), sandbox = { desktop: { open: async () => { throw raw; }, save: async () => { throw raw; } } };
    const body = helper.getText(ast).replace('await import("@tauri-apps/plugin-dialog")', 'await Promise.resolve(desktop)');
    const js = ts.transpileModule(body + "; pickFile;", { compilerOptions: { target: ts.ScriptTarget.ES2021 } }).outputText;
    const pick = vm.runInNewContext(js, sandbox), args = path.includes("Schedule") ? [true, "保存", []] : ["保存", []];
    await assert.rejects(pick(...args), e => e === raw); sandbox.desktop.save = async () => null; assert.equal(await pick(...args), null);
    sandbox.desktop.save = async () => ({ invalid: true }); await assert.rejects(pick(...args), /无法识别的路径/);
  }
});

const datasourceConfig = () => ({ url: "https://example.test/dav", user: "saved-user", auto_every: -1, remember_password: true, password_set: true, passphrase_set: true, headers_set: true, last_sync: null });
function datasourceFixture(t, getConfig) {
  dom(t); const writes = [], logs = []; let release = 0;
  t.mock.method(console, "error", (...args) => logs.push(args));
  const component = createDatasourceSyncTab({ pause: () => () => { release++; }, getConfig,
    save: async input => { writes.push(input); return { ...datasourceConfig(), ...input }; }, cancel: async () => {} });
  const all = element => [element, ...element.children.flatMap(all)], fields = all(component.el).filter(x => x.dataset.syncField);
  return { component, writes, logs, fields, field: key => fields.find(x => x.dataset.syncField === key), release: () => release };
}
test("datasource configuration refusal is shown and blocks saving unknown baseline instead of empty defaults", async t => {
  const raw = structured(), f = datasourceFixture(t, async () => { throw raw; }); await tick();
  assert.equal(f.fields.length, 6); assert(f.fields.every(field => field.disabled));
  const error = f.component.el.children.find(x => x.getAttribute("role") === "alert"); assert.equal(error.hidden, false); assert(error.textContent.includes(raw.operation_id));
  for (const field of f.fields) assert(field.getAttribute("aria-describedby").includes(error.id));
  f.field("url").value = "https://must-not-overwrite.test";
  await assert.rejects(f.component.saveIfNeeded(), error => assertDiagnostic(error, raw, "datasource_sync_get_config")); assert.equal(f.writes.length, 0); assert.equal(f.logs.length, 1);
  f.component.dispose(); await tick(); assert.equal(f.release(), 1);
});
test("datasource save waits for successful initialization before any write and valid baseline preserves secrets", async t => {
  let resolve; const pending = new Promise(yes => { resolve = yes; }), f = datasourceFixture(t, () => pending);
  const saving = f.component.saveIfNeeded(); await tick(); assert.equal(f.writes.length, 0); assert(f.fields.every(field => field.disabled));
  resolve(datasourceConfig()); await saving; assert.equal(f.writes.length, 0); assert(f.fields.every(field => !field.disabled));
  assert.equal(f.field("user").value, "saved-user");
  f.field("user").value = "edited-user"; for (const listener of f.field("user").listeners.get("input")) listener({});
  await f.component.saveIfNeeded(); assert.equal(f.writes.length, 1); assert.equal(f.writes[0].user, "edited-user");
  for (const key of ["password", "passphrase", "headers"]) assert(!Object.hasOwn(f.writes[0], key));
  f.component.dispose(); await tick();
});
test("datasource save already waiting on initialization receives the real refusal and never saves", async t => {
  let reject; const pending = new Promise((_, no) => { reject = no; }), f = datasourceFixture(t, () => pending), raw = structured();
  const refusal = assert.rejects(f.component.saveIfNeeded(), error => assertDiagnostic(error, raw, "datasource_sync_get_config"));
  await tick(); assert.equal(f.writes.length, 0); reject(raw); await refusal;
  assert.equal(f.writes.length, 0); assert(f.fields.every(field => field.disabled)); f.component.dispose(); await tick();
});
test("datasource disposal ignores late failed initialization without writes or UI updates", async t => {
  let reject; const pending = new Promise((_, no) => { reject = no; }), f = datasourceFixture(t, () => pending), raw = structured();
  const refusal = assert.rejects(f.component.saveIfNeeded(), /设置已关闭，没有保存配置/); await tick();
  f.component.dispose(); reject(raw); await refusal; await tick(); assert.equal(f.writes.length, 0); assert.equal(f.logs.length, 1); assert.equal(f.release(), 1);
  const error = f.component.el.children.find(x => x.getAttribute("role") === "alert"); assert.equal(error.hidden, true); assert.equal(f.field("url").value, undefined);
});
test("actual ScheduleTabView.reload refusal preserves old jobs/grid/selection without fake empty data or rejection", async () => {
  const source = fs.readFileSync("src/core/App.ts", "utf8"), ast = ts.createSourceFile("App.ts", source, ts.ScriptTarget.ES2021, true);
  const klass = ast.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === "ScheduleTabView");
  const method = klass?.members.find(node => ts.isMethodDeclaration(node) && node.name.getText(ast) === "reload"); assert(method);
  const raw = structured(), notices = [], old = [{ id: "saved-job", name: "already loaded" }], selected = new Set([0]), previousGrid = { rows: [["already loaded"]] }; let renders = 0, gridWrites = 0;
  const sandbox = { api: { listJobs: async () => { throw asIpcError(raw, "list_jobs"); } }, formatInterval: () => "手动" };
  const js = ts.transpileModule("class Schedule { " + method.getText(ast) + " } Schedule;", { compilerOptions: { target: ts.ScriptTarget.ES2021 } }).outputText;
  const Schedule = vm.runInNewContext(js, sandbox), view = new Schedule();
  Object.assign(view, { jobs: old, tab: { selectedRows: selected }, grid: { data: previousGrid, setData(value) { gridWrites++; this.data = value; } }, app: { toast: (...args) => notices.push(args), renderStatus: () => { renders++; }, connectionName: id => id } });
  await assert.doesNotReject(view.reload()); assert.equal(view.jobs, old); assert.equal(view.grid.data, previousGrid); assert.equal(view.tab.selectedRows, selected); assert.equal(gridWrites, 0); assert.equal(renders, 0);
  assert.match(notices[0][1], /保留上次已加载列表.*当前状态未确认/); assert(notices[0][1].includes(raw.operation_id));
  sandbox.api.listJobs = async () => []; await view.reload(); assert.equal(gridWrites, 1); assert.equal(view.grid.data.rows.length, 0); assert.equal(view.jobs.length, 0);
});

test("actual import mapping accepts only TABLE_NOT_FOUND for new tables and retains drafts on every other read failure", async t => {
  const doc = dom(t), source = fs.readFileSync("src/dialogs/ImportExportDialog.ts", "utf8"), ast = ts.createSourceFile("ImportExportDialog.ts", source, ts.ScriptTarget.ES2021, true);
  let refresh, run; function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === "refreshMapping") refresh = node.getText(ast);
    if (ts.isVariableDeclaration(node) && node.name.getText(ast) === "runImport") run = node.initializer.getText(ast);
    ts.forEachChild(node, visit);
  } visit(ast); assert(refresh && run);
  for (const checked of [false, true]) for (const code of ["PERMISSION_DENIED", "NETWORK_ERROR", "APPLICATION_ERROR", "OBJECT_NOT_FOUND_OR_UNREADABLE", "TABLE_NOT_FOUND"]) {
    const raw = structured(); raw.code = code; const old = [{ source: "csv_col", target: { value: "kept_target" } }], oldRow = doc.createElement("tr"), mapTable = doc.createElement("table"), mapNote = doc.createElement("div"), table = { value: "chosen_table" }, notices = [], logs = [];
    mapTable.append(oldRow); let pending = false, schemaCalls = 0, outcome = raw;
    const sandbox = { api: { previewCsv: async () => [["csv_col"]], getTableSchema: async () => { schemaCalls++; if (pending) return new Promise(() => {}); if (outcome instanceof Error || outcome?.code) throw outcome; return outcome; } },
      formatError, console: { error: (...args) => logs.push(args) }, formatSelect: { value: "csv" }, fileInput: { value: "chosen.csv" }, delimInput: { value: "," }, encodingSelect: { value: "utf-8" }, headerInput: { checked: true }, createInput: { checked }, connection: { id: "c1", read_only: false }, targetSchema: { value: "main" }, schema: "main", targetTableName: () => table.value,
      mapTable, mapNote, document: doc, select: (_items, value) => Object.assign(doc.createElement("select"), { value }), old,
      running: false, dlg: { showError: message => notices.push(message) }, transferTasks: { import() { assert.fail("failed or pending mapping must not send SQL"); } } };
    const body = "let mappingInputs = old; let mappingError = '', mappingLoading = false; " + refresh + "; const runImport = " + run + "; ({ refreshMapping, runImport, inputs: () => mappingInputs, error: () => mappingError });";
    const js = ts.transpileModule(body, { compilerOptions: { target: ts.ScriptTarget.ES2021 } }).outputText, actual = vm.runInNewContext(js, sandbox);
    await actual.refreshMapping(); const allowed = checked && code === "TABLE_NOT_FOUND";
    assert.equal(table.value, "chosen_table"); assert.equal(sandbox.fileInput.value, "chosen.csv");
    if (allowed) { assert.notEqual(actual.inputs(), old); assert.equal(actual.inputs()[0].target.value, "csv_col"); assert.equal(actual.error(), ""); assert.equal(logs.length, 0); }
    else {
      assert.equal(actual.inputs(), old); assert.equal(mapTable.children[0], oldRow); assert(actual.error().includes(raw.operation_id)); assert.equal(logs[0][1], raw);
      assert.equal(await actual.runImport(), false); assert(notices[0].includes(raw.operation_id));
      // A valid refresh is required to clear the error, never merely flipping the create switch.
      outcome = { columns: [{ name: "database_col" }] }; await actual.refreshMapping(); assert.equal(actual.error(), ""); assert.notEqual(actual.inputs(), old); assert.equal(schemaCalls, 2);
    }
    outcome = { columns: [] }; const saved = actual.inputs(), rows = [...mapTable.children]; await actual.refreshMapping();
    assert.equal(actual.inputs(), saved); assert.deepEqual(mapTable.children, rows); assert.match(actual.error(), /列元数据为空/);
    pending = true; void actual.refreshMapping(); await tick(); assert.equal(await actual.runImport(), false); assert.match(notices.at(-1), /仍在读取/);
  }
});
