import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { webcrypto } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { transactionFixtureBindings } from './transaction-fixture-bindings.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const read = file => fs.readFileSync(path.join(root, file), 'utf8');
const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
class Element {
  constructor(tag = 'div') { this.tagName = tag; this.children = []; this.attributes = {}; this.dataset = {}; this.style = {}; this.listeners = {}; this.disabled = false; this._value = ''; this.classList = { add() {}, remove() {}, toggle() {} }; }
  set textContent(value) { this._text = String(value); this.children = []; }
  get textContent() { return (this._text ?? '') + this.children.map(child => child.textContent ?? '').join(''); }
  set value(v) { this._value = String(v); }
  get value() { return this._value || (this.tagName === 'select' ? this.children[0]?.value ?? '' : ''); }
  get options() { return this.children; }
  get valueAsNumber() { return Number(this.value); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  setAttribute(k, v) { this.attributes[k] = v; }
  removeAttribute(k) { delete this.attributes[k]; }
  appendChild(c) { if (typeof c === 'string') c = Object.assign(new Element('#text'), { textContent: c }); this.children.push(c); c.parentElement = this; return c; }
  append(...cs) { cs.forEach(c => this.appendChild(c)); }
  insertBefore(c) { return this.appendChild(c); }
  replaceChildren(...cs) { this.children = []; this._value = ''; this.append(...cs); }
  addEventListener(k, f) { (this.listeners[k] ??= []).push(f); }
  fire(k) { for (const f of this.listeners[k] ?? []) f({ target: this, preventDefault() {} }); }
  querySelectorAll(selector) { return this.children.flatMap(c => [...((selector.startsWith('.') ? String(c.className ?? '').split(/\s+/).includes(selector.slice(1)) : ['input', 'select', 'button'].includes(c.tagName) && selector.includes(c.tagName)) ? [c] : []), ...c.querySelectorAll(selector)]); }
  focus() {}
  checkValidity() { return true; }
  remove() {}
  get tBodies() { return [this]; }
  insertRow() { const r = new Element('tr'); r.cells = []; r.insertCell = () => { const c = new Element('td'); r.cells.push(c); r.append(c); return c; }; this.append(r); return r; }
}
function harness() {
  const notices = [], dialogs = [], wizards = [], launched = [];
  const timers = new Map(); let nextTimer = 1;
  const window = { setInterval(callback, interval) { const id = nextTimer++; timers.set(id, { callback, interval }); return id; }, clearInterval(id) { timers.delete(id); }, setTimeout(callback, interval) { const id = nextTimer++; timers.set(id, { callback, interval }); return id; }, clearTimeout(id) { timers.delete(id); } };
  const document = { body: new Element('body'), activeElement: null, createElement: t => new Element(t), createTextNode: text => Object.assign(new Element('#text'), { textContent: text }), createDocumentFragment: () => new Element('fragment') };
  class Dialog { constructor(opts) { this.body = new Element(); this.footer = new Element(); this.overlay = new Element(); this.el = this.overlay; dialogs.push(this); opts.render(this.body); } showError(s) { this.error = s; } clearError() { this.error = null; } onClose(f) { this.onClosed = f; } close() { this.closed = true; this.onClosed?.(); } }
  class Wizard { constructor(opts) { this.opts = opts; this.step = 0; this.stepsEl = new Element(); this.bodyEl = new Element(); this.busy = false; wizards.push(this); for (const s of opts.steps) { const panel = new Element(); this.bodyEl.append(panel); s.render(panel); } } buttons() { return []; } showError(s) { this.error = s; } clearError() { this.error = null; } setBusy(b) { this.busy = b; } async next() { if (this.busy) return; const error = this.opts.steps[this.step].validate?.(); if (error) { this.showError(error); return; } if (this.step < this.opts.steps.length - 1) this.step++; else await this.opts.onFinish(); } }
  const context = vm.createContext({ exports: {}, ...transactionFixtureBindings, console, document, HTMLElement: Element, Option: class extends Element { constructor(label, value) { super('option'); this.textContent = label; this.value = value; } }, crypto: webcrypto, structuredClone, window, setTimeout, clearTimeout, setInterval, clearInterval, Dialog, Wizard,
    api: {}, syncApi: { typeMapping: async () => [] }, transferTasks: {},
    field: (label, input) => { const e = new Element('label'); e.textContent = label; const title = new Element(); title.className = 'field-label'; title.textContent = label; e.append(title, input); return e; },
    select: (options, selected) => { const e = new Element('select'); options.forEach(o => { const item = new Element('option'); item.value = o.value; item.textContent = o.label; e.append(item); }); if (selected && options.some(o => o.value === selected)) e.value = selected; return e; },
    textInput: v => Object.assign(new Element('input'), { value: v }), numberInput: v => Object.assign(new Element('input'), { value: v }),
    toolButton: label => Object.assign(new Element('button'), { textContent: label }), toolTable: () => new Element('table'), accessibleToolDialog() {},
    parseDriverParams: v => v?.startsWith('@navkit-driver:') ? {} : null,
    QueryTabView: class {}, DataTabView: class {}, StructureTabView: class {}, ObjectsTabView: class {},
    openDataGeneratorDialog: (...args) => launched.push(args), openSyncDialog: (...args) => launched.push(args),
    openServerMonitorDialog: (...args) => launched.push({ tool: 'monitor', args }),
  });
  function load(file, source = read(file)) { source = source.replace(/^import .*;\r?$/gm, ''); vm.runInContext(ts.transpile(source, { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS }), context, { filename: file }); Object.assign(context, context.exports); }
  load('src/core/execution.ts'); load('src/core/structure-review.ts'); load('src/core/structure-sync.ts');
  load('src/ui/StructureSyncReview.ts');
  load('src/core/connection-health-client.ts');
  load('src/core/dbcatalog.ts'); load('src/core/toolCapabilities.ts');
  const source = read('src/core/App.ts');
  load('src/core/App.ts', source.slice(source.indexOf('export class App'), source.indexOf('abstract class TabView')));
  const declarations = ts.createSourceFile('App.ts', source, ts.ScriptTarget.ES2021, true).statements;
  const classText = name => { const declaration = declarations.find(node => ts.isClassDeclaration(node) && node.name?.text === name); assert(declaration, `missing production class ${name}`); return declaration.getText(); };
  load('TabView.ts', classText('TabView') + '\nexports.TabView = TabView;');
  const app = Object.create(context.App.prototype);
  const toolbarEl = new Element(), stableSlot = new Element(); toolbarEl.append(stableSlot);
  Object.assign(app, { objectSearchOpen: false, objectSearchInput: null, keepAliveTimers: new Map(), tabs: [], activeTabId: null, views: new Map(), tree: { find: () => null }, connections: [], ribbon: { updateDisabled() {} }, tabToolbar: { el: toolbarEl, stableSlot, updates: [], setItems(items) { this.updates.push(items); } }, tabToolbarItems: () => [], menu: { renderBar() {} }, contextMenus: () => [], viewMenu: () => [], renderStatus() {}, toast: (...a) => notices.push(a), buildTabToolbar() {}, currentTableRef: () => null, currentDbName: () => 'main' });
  return { context, load, app, notices, dialogs, wizards, launched, timers };
}
const cfg = (id = 'c', extra = {}) => ({ id, name: id, kind: 'sqlite', params: '', read_only: false, ...extra });
function dataPage(h, readOnly = false, objectType = 'table') { const tab = { id: 't', connectionId: 'c', kind: 'table', objectType, selectedRows: new Set() }; h.app.tabs = [tab]; h.app.activeTabId = 't'; h.app.connections = [cfg('c', { read_only: readOnly })]; const view = new h.context.DataTabView(); h.app.views.set('t', view); return { tab, view }; }

test('tool capability uses actual native protocol; custom ODBC and read-only writes fail closed', () => {
  const h = harness(), error = h.context.toolConnectionError;
  assert.equal(error(cfg()), null); assert.equal(error(cfg('c', { kind: 'postgresql' })), null);
  assert.match(error(cfg('c', { params: '@navkit-driver:{}' })), /ODBC/);
  assert.match(error(cfg('c', { kind: 'redis' })), /原生/);
  assert.equal(error(cfg('c', { read_only: true })), null);
  assert.match(error(cfg('c', { read_only: true }), true), /只读/);
});

test('App command guard reports missing context, read-only/view writes and unknown commands', async () => {
  const h = harness(); await h.app.runCommand('save'); await h.app.runCommand('data-generation');
  assert.match(h.notices[0][1], /没有可保存/); assert.match(h.notices[1][1], /选择一个连接/);
  const { view } = dataPage(h, true); let writes = 0; view.commitEdits = async () => writes++;
  await h.app.runCommand('grid-commit'); assert.equal(writes, 0); assert.match(h.notices.at(-1)[1], /只读/);
  dataPage(h, false, 'view'); await h.app.runCommand('grid-insert'); assert.match(h.notices.at(-1)[1], /不能写入视图/);
  await h.app.runCommand('a-visible-unimplemented-command'); assert.match(h.notices.at(-1)[1], /尚未实现/);
});

test('save-as exports data without silently committing pending database edits', async () => {
  const h = harness(); const { view } = dataPage(h, true); let writes = 0, exports = 0;
  view.commitEdits = async () => writes++; h.app.cmdExport = () => exports++;
  await h.app.runCommand('save-as'); assert.equal(exports, 1); assert.equal(writes, 0);
});

test('object-list select-all updates the actual grid instead of doing nothing', async () => {
  const h = harness(); const { tab } = dataPage(h); const view = new h.context.ObjectsTabView(); let renders = 0;
  view.grid = { rows: [['a'], ['b']], setState(state) { this.state = state; renders++; } }; h.app.views.set('t', view);
  await h.app.runCommand('select-all'); assert.deepEqual([...tab.selectedRows], [0, 1]); assert.equal(renders, 1);
});

test('tool launch never borrows a table from another connection or database', async () => {
  const h = harness(); h.app.connections = [cfg()]; h.app.currentConnectionId = () => 'c';
  h.app.currentTableRef = () => ({ connId: 'other', schema: 'main', table: 'secret' });
  await h.app.runCommand('data-generation'); assert.equal(h.launched[0][2], '');
  h.app.currentTableRef = () => ({ connId: 'c', schema: 'aux', table: 'secret' });
  await h.app.runCommand('data-sync'); assert.equal(h.launched[1][2].table, '');
  h.app.connections = [cfg('c', { kind: 'mysql' }), cfg('other')];
  h.app.tabs = [{ id: 'foreign', connectionId: 'other', schema: 'foreign_schema', kind: 'table' }]; h.app.activeTabId = 'foreign';
  h.app.tree = { selectedNodeId: 'conn:c', find: () => ({ id: 'conn:c' }) };
  await h.app.runCommand('data-generation'); assert.equal(h.launched[2][1], '');
  await h.app.runCommand('data-sync'); assert.equal(h.launched[3][2].schema, '');
});

test('global tools stay clickable with an unsupported selection and pass it through for in-dialog picking', async () => {
  const h = harness();
  h.app.connections = [cfg('c', { kind: 'redis' }), cfg('db')];
  h.app.tree = { selectedNodeId: 'conn:c', find: () => ({ id: 'conn:c' }) };
  // 选中的库不支持同步/生成：全局工具命令不再被禁用（手动事务占用仍单独拦截）。
  assert.equal(h.app.commandUnavailableReason('data-sync'), null);
  assert.equal(h.app.commandUnavailableReason('data-generation'), null);
  await h.app.runCommand('data-sync');
  assert.equal(h.launched[0][1]?.id, 'c', '原样传入当前连接，由对话框空选处理');
  await h.app.runCommand('data-generation');
  assert.equal(h.launched[1][0]?.id, 'c');
  assert.equal(h.launched[1][5]?.length, 2, '连接清单传给对话框供其下拉过滤');
});

function loadGenerator(h) {
  h.context.testWizards = h.wizards;
  h.load('src/ui/Wizard.ts', read('src/ui/Wizard.ts').replace('export class Wizard', 'class ProductionWizard') + '\nexports.Wizard = class extends ProductionWizard { constructor(opts) { super(opts); testWizards.push(this); } get step() { return this.current; } set step(value) { this.go(value); } get error() { return this.errorEl.hidden ? null : this.errorEl.textContent; } };');
  h.load('src/core/batchExecution.ts');
  // BatchExecutionPanel 运行期引用 ExecutionProgress / ExecutionLogList（import 行被剥掉，须先加载为上下文全局）
  h.load('src/core/log-format.ts');
  h.load('src/ui/ExecutionProgress.ts');
  h.load('src/ui/ExecutionLogList.ts');
  h.load('src/ui/BatchExecutionPanel.ts');
  h.load('src/core/backgroundTasks.ts');
  h.load('src/core/dataGenerator.ts'); h.load('src/dialogs/DataGeneratorDialog.ts');
}
function generatorHarness(source, connection = cfg(), schema = '') {
  const h = harness(); loadGenerator(h);
  const done = []; const dlg = h.context.openDataGeneratorDialog(connection, schema, '', (...a) => done.push(a), source);
  return { ...h, dlg, done, controls: () => [...h.wizards[0].bodyEl.querySelectorAll('input,select,button'), ...dlg.footer.querySelectorAll('input,select,button')] };
}
const tableSchema = { columns: [{ name: 'id', type: 'INTEGER', primary_key: true, nullable: false, auto_increment: false, default: null }], foreign_keys: [], indexes: [] };
function generatorApi() {
  const calls = [], issued = new Map();
  const source = {
    calls,
    ensureConnection: async id => { calls.push(['ensure', id]); return { ok: true }; },
    listDatabases: async () => { calls.push(['databases']); return [['main', 'database'], ['other', 'database']]; },
    listTables: async (id, schema) => { calls.push(['tables', schema]); return [['items', 'table'], ['view', 'view']]; },
    getTableSchema: async (id, schema) => { calls.push(['schema', id, schema]); return tableSchema; },
    registerBatchExecution: async request => {
      assert.equal(request.task.kind, 'generated_rows'); assert.ok(request.owner_id);
      const captured = structuredClone(request);
      const ticket = { execution_id: webcrypto.randomUUID(), execution_grant: webcrypto.randomUUID(), connection_id: request.connection_id, namespace: request.namespace, owner_id: request.owner_id,
        task_kind: request.task.kind, expires_in_ms: 60000, summary: '生成数据写入', total_rows: request.task.request.rows.length, source_bytes: null };
      issued.set(ticket.execution_id, { ticket, captured, sent: false, snapshot: null }); calls.push(['register', captured]); return ticket;
    },
    runBatchExecution: async ticket => {
      const entry = issued.get(ticket.execution_id); assert.ok(entry, 'run consumes only a registered ticket'); assert.deepEqual(ticket, entry.ticket);
      assert.equal(entry.sent, false, 'a registered execution is never replayed'); entry.sent = true;
      const request = entry.captured.task.request; calls.push(['write', request]);
      const context = { execution_id: ticket.execution_id, connection_id: ticket.connection_id, namespace: ticket.namespace, owner_id: ticket.owner_id };
      const deadline = { execution_id: ticket.execution_id, budget_ms: null, elapsed_ms: 1, deadline_reached: false, stop_confirmed: false };
      entry.snapshot = { context, kind: 'generated_rows', status: { execution_id: ticket.execution_id, state: 'succeeded', deadline, resources_pending: false },
        metrics: { scanned: request.rows.length, executed: request.rows.length, errors: 0, elapsed_ms: 1 }, scan_complete: true, parse_error: null, affected_rows: request.rows.length,
        total_rows: request.rows.length, transaction_state: 'committed', logs: [], next_sequence: 0, dropped_before_sequence: 0,
        outcome: { execution_id: ticket.execution_id, state: 'succeeded', result: null, error: null, deadline, effects: 'applied', transaction_state: 'committed', resources_pending: false } };
      return entry.snapshot;
    },
    batchExecutionStatus: async context => { const entry = issued.get(context.execution_id); assert.ok(entry?.snapshot); assert.deepEqual(context, entry.snapshot.context); return entry.snapshot; },
    cancelExecution: async () => { throw new Error('these generator cases never request stop'); },
    insertGeneratedRows: async () => { throw new Error('legacy direct generator writes must not be used'); },
  };
  return source;
}

test('generator connects before loading databases and supports empty connection-entry schema', async () => {
  const source = generatorApi(), h = generatorHarness(source); await flush();
  assert.deepEqual(source.calls.slice(0, 3), [['ensure', 'c'], ['databases'], ['tables', 'main']]);
  const table = h.controls().find(e => e.attributes['aria-label'] === '目标表');
  assert.deepEqual(table.options.map(o => o.value), ['items']); assert.equal(h.wizards[0].opts.steps[0].validate(), null);
});

test('generator reports connection failure and performs no metadata/write calls for read-only target', async () => {
  let source = generatorApi(); source.ensureConnection = async () => ({ ok: false, message: 'real connection failure' });
  let h = generatorHarness(source);
  const wizard = h.wizards[0];
  await new Promise(resolve => {
    const showError = wizard.showError.bind(wizard);
    wizard.showError = message => { showError(message); resolve(); };
  });
  assert.match(wizard.error, /real connection failure/); assert.equal(wizard.errorEl.hidden, false); assert.equal(source.calls.length, 0);
  source = generatorApi(); h = generatorHarness(source, cfg('c', { read_only: true })); assert.match(h.wizards[0].error, /只读/); assert.equal(h.wizards[0].errorEl.hidden, false); assert.equal(source.calls.length, 0);
});

test('generator requires preview and explicit confirmation; one submit path locks and invalidates stale preview', async () => {
  const source = generatorApi(), h = generatorHarness(source); await flush(); const wizard = h.wizards[0];
  wizard.step = 2; await wizard.next(); assert.equal(source.calls.filter(c => c[0] === 'write').length, 0);
  const preview = h.controls().find(e => e.textContent === '生成预览'); preview.fire('click');
  await wizard.next(); assert.equal(source.calls.filter(c => c[0] === 'write').length, 0);
  const check = h.controls().find(e => e.type === 'checkbox'); check.checked = true; check.fire('change');
  const execute = h.controls().find(e => e.textContent === '执行写入'); execute.fire('click'); execute.fire('click'); await flush();
  const writes = source.calls.filter(c => c[0] === 'write'); assert.equal(writes.length, 1); assert.equal(writes[0][1].schema, 'main'); assert.equal(h.done.length, 1); assert.equal(check.checked, false); assert.equal(wizard.busy, false);
  const registrations = source.calls.filter(c => c[0] === 'register'); assert.equal(registrations.length, 1); assert.equal(registrations[0][1].task.kind, 'generated_rows');
  const record = h.context.batchExecutionServiceFor(source).list()[0]; assert.equal(record.state, 'succeeded'); assert.equal(record.snapshot.transaction_state, 'committed'); assert.equal(record.snapshot.status.resources_pending, false);
  check.checked = true; execute.fire('click'); await flush(); assert.equal(source.calls.filter(c => c[0] === 'write').length, 1);
});

test('generator database change discards preview and uses selected scope', async () => {
  const source = generatorApi(), h = generatorHarness(source); await flush();
  const controls = h.controls(), preview = controls.find(e => e.textContent === '生成预览'); preview.fire('click');
  const database = controls.find(e => e.attributes['aria-label'] === '目标数据库 (main)'); database.value = 'other'; database.fire('change'); await flush();
  assert.match(h.wizards[0].opts.steps[2].validate(), /先生成预览/); assert.ok(source.calls.some(c => c[0] === 'tables' && c[1] === 'other'));
});

test('generator keeps the connection unselected when the current one is unsupported, then loads on explicit pick', async () => {
  const source = generatorApi(), h = harness(); loadGenerator(h);
  const connections = [cfg('main-conn'), cfg('ro', { read_only: true }), cfg('bad', { kind: 'redis' }), cfg('alt')];
  h.context.openDataGeneratorDialog(connections[2], 'main', '', () => {}, source, connections);
  await flush();
  const controls = () => h.wizards[0].bodyEl.querySelectorAll('input,select,button');
  const connSel = controls().find(e => e.attributes['aria-label'] === '连接');
  assert.deepEqual(connSel.options.map(o => o.value), ['', 'main-conn', 'alt'], '只读与 redis 连接不进下拉');
  assert.equal(connSel.value, '', '当前数据源不支持时空选，不默认第一个');
  assert.deepEqual(source.calls, [], '选定连接前不发起任何请求');
  assert.match(h.wizards[0].opts.steps[0].validate(), /请先在「连接」中选择/);
  connSel.value = 'main-conn'; connSel.fire('change'); await flush();
  assert.deepEqual(source.calls.slice(0, 3), [['ensure', 'main-conn'], ['databases'], ['tables', 'main']]);
  assert.ok(source.calls.some(c => c[0] === 'schema' && c[1] === 'main-conn'), '读取表结构使用选定的连接');
  const table = controls().find(e => e.attributes['aria-label'] === '目标表');
  assert.deepEqual(table.options.map(o => o.value), ['items']);
});

test('sync preselects the target table name from the auto-selected source table so the wizard can advance', async () => {
  const h = harness();
  h.context.api = { ensureConnection: async () => ({ ok: true }), listDatabases: async () => [['main', 'database']], listTables: async () => [['items', 'table'], ['orders', 'table']] };
  h.load('src/dialogs/SyncDialog.ts');
  const dlg = h.context.openSyncDialog([cfg('source'), cfg('target')], cfg('source'), { schema: 'main', table: '' });
  await flush();
  const targetTable = h.wizards[0].bodyEl.querySelectorAll('input').find(e => String(e.attributes['aria-label'] || '').includes('目标表'));
  assert.ok(targetTable, '目标表输入框应存在');
  assert.equal(targetTable.value, 'items', '源表自动选中后目标表名应默认同名，而不是留空');
  assert.equal(h.wizards[0].opts.steps[0].validate(), null, '目标表名默认填好后应可通过第一步校验');
  dlg.close();
});

test('sync keeps the source unselected instead of defaulting when the current source is unsupported; excludes read-only targets', async () => {
  const h = harness(); h.context.api = { ensureConnection: async () => ({ ok: true }), listDatabases: async () => [['main', 'database']], listTables: async () => [['items', 'table']] };
  h.load('src/dialogs/SyncDialog.ts');
  // 当前数据源不支持：对话框照常打开，源/目标空选（占位项），不默认选中第一个。
  h.context.openSyncDialog([cfg()], cfg('bad', { kind: 'redis' })); await flush();
  const selects = h.wizards[0].bodyEl.querySelectorAll('select');
  const connectionSelectors = selects.filter(e => e.options.some(o => o.value === 'c') && e.options.some(o => o.value === ''));
  assert.equal(connectionSelectors.length, 2, '源/目标两个连接下拉都应存在');
  for (const e of connectionSelectors) {
    assert.equal(e.value, '', '不支持的数据源必须空选，而不是默认选第一个受支持连接');
    assert.equal(e.options.some(o => o.value === 'bad'), false, 'redis 不应出现在下拉里');
    assert.equal(e.options.some(o => o.value === 'c'), true, '受支持连接仍可被选中');
  }
  assert.match(h.wizards[0].opts.steps[0].validate(), /请选择源连接/);
  assert.throws(() => h.context.openSyncDialog([cfg('ro', { read_only: true })], cfg('ro', { read_only: true })), /没有可写/);
  h.context.openSyncDialog([cfg('source', { read_only: true }), cfg('target')], cfg('source', { read_only: true })); await flush();
  const selects2 = h.wizards[1].bodyEl.querySelectorAll('select');
  const connectionSelectors2 = selects2.filter(e => e.options.some(o => ['source', 'target'].includes(o.value)));
  assert.equal(connectionSelectors2.length, 2, 'Both actual connection selectors must remain present');
  assert.equal(connectionSelectors2[0].value, 'source');
  assert.deepEqual(connectionSelectors2[0].options.map(o => o.value), ['source', 'target']);
  assert.deepEqual(connectionSelectors2[1].options.map(o => o.value), ['target']);
});

test('sync offers sqlserver/clickhouse/odbc(oracle) but never redis/es/kafka/registry connections', async () => {
  const h = harness(); h.context.api = { ensureConnection: async () => ({ ok: true }), listDatabases: async () => [['main', 'database']], listTables: async () => [['items', 'table']] };
  h.load('src/dialogs/SyncDialog.ts');
  const connections = [
    cfg('source'), cfg('target'),
    cfg('mss', { kind: 'sqlserver' }), cfg('ch', { kind: 'clickhouse' }), cfg('ora', { kind: 'oracle' }),
    cfg('redis', { kind: 'redis' }), cfg('es', { kind: 'elasticsearch' }),
    cfg('kafka', { kind: 'kafka' }), cfg('nacos', { kind: 'nacos' }),
  ];
  // 非关系型作为「当前数据源」：不再抛错 —— 对话框打开、数据源空选，且它们不出现在下拉里。
  for (const unsupported of connections.slice(5)) {
    h.wizards.length = 0;
    const dlg = h.context.openSyncDialog(connections, unsupported); await flush();
    const selects = h.wizards[0].bodyEl.querySelectorAll('select');
    const connectionSelectors = selects.filter(e => e.options.some(o => connections.some(c => c.id === o.value)));
    assert.equal(connectionSelectors.length, 2, `${unsupported.kind} 打开时仍应有源/目标两个连接下拉`);
    for (const e of connectionSelectors) {
      assert.equal(e.value, '', `${unsupported.kind} 作为当前数据源时必须空选`);
      assert.equal(e.options.some(o => o.value === unsupported.id), false, `${unsupported.id} 不应出现在下拉中`);
    }
    dlg.close();
  }
  for (const extended of connections.slice(2, 5)) {
    h.wizards.length = 0;
    const dlg = h.context.openSyncDialog(connections, extended); await flush();
    dlg.close();
  }
  // 支持的源 + 全部不支持的目标：下拉里出现全部 5 个支持的连接，redis/es/kafka/nacos 选不到。
  h.context.openSyncDialog(connections, cfg('source')); await flush();
  const selects = h.wizards[0].bodyEl.querySelectorAll('select');
  const connectionSelectors = selects.filter(e => e.options.some(o => ['source', 'target'].includes(o.value)));
  assert.equal(connectionSelectors.length, 2);
  const ids = selectors => selectors.flatMap(e => e.options.map(o => o.value));
  // source 连接可写，也会出现在目标下拉（同连接换库/换表名是合法用法）；
  // 关键断言：sqlserver/clickhouse/oracle 可选，redis/elasticsearch/kafka/nacos 不可选。
  assert.deepEqual(ids(connectionSelectors), ['source', 'target', 'mss', 'ch', 'ora', 'source', 'target', 'mss', 'ch', 'ora']);
  for (const banned of ['redis', 'es', 'kafka', 'nacos']) {
    for (const e of connectionSelectors) {
      assert.equal(e.options.some(o => o.value === banned), false, `${banned} 不应出现在下拉中`);
    }
  }
});

test('real imported transaction capability and App tool gates reject active or pending automatic writes', async () => {
  const h = harness(); dataPage(h);
  const reason = h.context.manualTransactionUnavailableReason;
  assert.equal(reason(cfg()), null);
  assert.match(reason(cfg('c', { read_only: true })), /只读/);
  assert.match(reason(cfg('c', { params: '@navkit-driver:{"version":1,"driver_id":"custom"}' })), /原生/);
  assert.match(reason(cfg('c', { kind: 'redis' })), /原生/);
  let active = { active: true, owner: 'query-owner', schema: 'main', failed: false }, busy = false;
  h.app.manualTransactionController = { status: id => id === 'c' ? active : undefined, busy: id => id === 'c' && busy };
  for (const command of ['data-sync', 'data-generation', 'grid-commit', 'save']) {
    assert.equal(h.context.MANUAL_TRANSACTION_WRITE_COMMANDS.has(command) || command === 'save', true);
    assert.match(h.app.commandUnavailableReason(command), /手动事务/);
    await h.app.runCommand(command);
  }
  assert.equal(h.launched.length, 0);
  assert.equal(h.app.commandUnavailableReason('save-as'), null);
  active = undefined; busy = true;
  assert.match(h.app.commandUnavailableReason('data-sync'), /事务操作正在进行/);
  busy = false;
  assert.equal(h.app.commandUnavailableReason('data-generation'), null);
});

test('监控无须选中连接或打开数据库，入口传完整列表并按选择惰性建立连接', async () => {
  const h = harness(), ensured = []; let healthReads = 0, probes = 0;
  h.context.api.connectionHealth = async id => { healthReads++; return { connection_id: id, generation: 'opened-generation', enabled: true, state: 'healthy', interval_secs: 180, timeout_secs: 5, resources_pending: false, next_due_in_ms: 180_000, probe_skipped_reason: null }; };
  h.context.api.checkConnectionHealth = async () => { probes++; assert.fail('惰性打开连接只读取健康状态，不提前发送探测 SQL'); };
  assert.equal(h.app.commandUnavailableReason('server-monitor'), null);
  await h.app.runCommand('server-monitor');
  const initial = h.launched[0]; assert.equal(initial.tool, 'monitor');
  assert.equal(initial.args[0], h.app.connections); assert.equal(initial.args[1], undefined);
  assert.equal(initial.args[2], h.context.api); assert.equal(typeof initial.args[3], 'function');
  h.app.connections = [cfg('c', { kind: 'mysql' }), cfg('ro', { kind: 'postgresql', read_only: true })];
  h.app.openConnections = new Set(); h.app.syncConnIcons = () => {};
  h.context.api.ensureConnection = async id => { ensured.push(id); return { ok: true, message: '已连接' }; };
  h.context.api.openDatabase = () => assert.fail('监控不打开数据库');
  await h.app.runCommand('server-monitor'); const opened = h.launched.at(-1);
  assert.equal(opened.args[0], h.app.connections); assert.equal(opened.args[0].length, 2);
  await opened.args[3]('c'); await opened.args[3]('c'); await flush();
  assert.deepEqual(ensured, ['c']); assert.equal(h.app.tabs.length, 0); assert.deepEqual(h.notices, []);
  assert.equal(healthReads, 1); assert.equal(probes, 0);
  assert.deepEqual([...h.timers.values()].map(timer => timer.interval), [180_000], '重复确保连接只保留一个由服务器到期时间决定的定时器');
  h.app.stopKeepAlive('c'); assert.equal(h.timers.size, 0, '断开连接停止当前代次保活');
});
test('监控和 SQL 历史可脱离失效数据页，其他写入命令仍被原守卫拦截', async () => {
  const h = harness(); dataPage(h); h.app.tabs[0].datasourceStale = true;
  const blocked = h.app.commandUnavailableReason('grid-commit'); assert.ok(blocked);
  assert.equal(h.app.commandUnavailableReason('server-monitor'), null);
  assert.equal(h.app.commandUnavailableReason('history-log'), null);
  assert.equal(h.app.commandUnavailableReason('grid-commit'), blocked);
  await h.app.runCommand('server-monitor'); assert.equal(h.launched.at(-1).tool, 'monitor');
  await h.app.runCommand('grid-commit'); assert.equal(h.notices.at(-1)[1], blocked);
});

test('同步两端命名空间标签和ARIA随实际连接切换更新，不静态绑定初始连接', async () => {
  const h = harness();
  h.context.api = { ensureConnection: async () => ({ ok: true }), listDatabases: async () => [['scope', 'scope']], listTables: async () => [['items', 'table']] };
  h.load('src/dialogs/SyncDialog.ts');
  const pg = cfg('pg', { kind: 'postgresql' }), mysql = cfg('my', { kind: 'mysql' });
  const dlg = h.context.openSyncDialog([pg, mysql], pg); await flush();
  const body = h.wizards[0].bodyEl, selects = body.querySelectorAll('select');
  const sourceConnection = selects.find(select => select.options.some(option => option.value === 'pg') && select.options.some(option => option.value === 'my'));
  const sourceNamespace = selects.find(select => select.attributes['aria-label'] === '源模式');
  const targetNamespace = selects.find(select => select.attributes['aria-label'] === '目标数据库');
  assert(sourceNamespace); assert(targetNamespace);
  assert.equal(sourceNamespace.parentElement.children[0].textContent, '模式:');
  assert.equal(targetNamespace.parentElement.children[0].textContent, '数据库:');
  sourceConnection.value = 'my'; sourceConnection.fire('change');
  assert.equal(sourceNamespace.attributes['aria-label'], '源数据库');
  assert.equal(sourceNamespace.parentElement.children[0].textContent, '数据库:'); await flush();
  sourceConnection.value = 'pg'; sourceConnection.fire('change'); await flush();
  assert.equal(sourceNamespace.attributes['aria-label'], '源模式');
  assert.equal(sourceNamespace.parentElement.children[0].textContent, '模式:');
  assert.equal(pg.database, undefined); dlg.close();
});

test('sync disables diff / structure preview when the dialect cannot support them', async () => {
  const h = harness();
  h.context.api = { ensureConnection: async () => ({ ok: true }), listDatabases: async () => [['scope', 'scope']], listTables: async () => [['items', 'table']] };
  h.load('src/dialogs/SyncDialog.ts');
  const buttons = () => { const all = []; const walk = node => { if (node.tagName === 'button') all.push(node); (node.children ?? []).forEach(walk); }; walk(h.wizards.at(-1).bodyEl); return all; };
  const button = text => buttons().find(b => b.textContent === text);

  // SQLite → SQLite：受控结构同步后端已认证，比较差异与预览结构差异都应可点。
  const sqlite = h.context.openSyncDialog([cfg('a'), cfg('b')], cfg('a')); await flush();
  assert.equal(button('比较差异').disabled, false);
  assert.equal(button('预览结构差异').disabled, false);
  sqlite.close();

  // MySQL → MySQL：原生服务端已接入；元数据和版本限制由后端预览进一步检查。
  const mysql = h.context.openSyncDialog([cfg('m1', { kind: 'mysql' }), cfg('m2', { kind: 'mysql' })], cfg('m1', { kind: 'mysql' })); await flush();
  assert.equal(button('比较差异').disabled, false);
  assert.equal(button('预览结构差异').disabled, false);
  mysql.close();
  const cross = h.context.openSyncDialog([cfg('s'), cfg('m', { kind: 'mysql' })], cfg('s')); await flush();
  assert.equal(button('预览结构差异').disabled, true); assert.match(button('预览结构差异').title, /跨方言/);
  cross.close();
});
