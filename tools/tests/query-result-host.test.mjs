import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { build } from 'esbuild';
import { randomUUID } from 'node:crypto';
import { transactionFixtureBindings } from './transaction-fixture-bindings.mjs';

// Production App/QueryTabView classes and production IPC adapter/component/model execute below.
// Only editor/grid painting and the desktop invoke boundary are fixtures, not the guards/state machine.
// This suite makes no claim about browser layout, a desktop window, or a live database.
const bundle = await build({ stdin: { contents: `export { QueryResultPanel } from './src/ui/QueryResultPanel';
export { QueryResultPages } from './src/core/queryResultPages';
export { queryResultApi, querySnapshotForPage } from './src/core/query-result-api';
export { ExecutionController } from './src/core/execution';
export { queryFavorite } from './src/core/savedQueries';
export { TEXT_FORMAT_MENU } from './src/ui/menu';
`, resolveDir: process.cwd(), loader: 'ts' },
  bundle: true, platform: 'node', format: 'esm', target: 'es2021', write: false, loader: { '.css': 'empty' },
  plugins: [{ name: 'desktop-and-paint-only', setup(build) {
    build.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'invoke', namespace: 'fixture' }));
    build.onResolve({ filter: /^\.\/DataGrid$/ }, args => args.importer.endsWith('QueryResultPanel.ts') ? { path: 'grid', namespace: 'fixture' } : null);
    build.onLoad({ filter: /.*/, namespace: 'fixture' }, args => ({ loader: 'js', contents: args.path === 'invoke'
      ? `export const invoke = (command, args) => globalThis.__PL21_HOST_INVOKE__(command, args);`
      : `export class DataGrid { constructor(callbacks) { this.callbacks = callbacks; this.el = document.createElement('div'); this.el.className = 'grid-wrap'; }
        setData(data) { this.data = data; } dispose() { this.disposed = true; } }` }));
  } }], logLevel: 'silent' });
const modules = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text + '\n//# sourceURL=navkit-query-result-host-contract.mjs').toString('base64')}`);

class Element {
  constructor(tag, doc) { this.tagName = tag.toUpperCase(); this.ownerDocument = doc; this.children = []; this.parentNode = null; this.attributes = new Map(); this.listeners = new Map(); this.dataset = {}; this.style = {}; this._text = ''; this.className = ''; this.value = ''; this.type = ''; this.id = ''; this.hidden = false; this.disabled = false; this.checked = false; this.inert = false; this.isConnected = true; this.clientHeight = 600;
    this.classList = { contains: name => this.className.split(/\s+/).includes(name), add: (...names) => { this.className = [...new Set([...this.className.split(/\s+/).filter(Boolean), ...names])].join(' '); }, remove: (...names) => { this.className = this.className.split(/\s+/).filter(name => !names.includes(name)).join(' '); }, toggle: (name, force) => { const yes = force ?? !this.classList.contains(name); yes ? this.classList.add(name) : this.classList.remove(name); } };
  }
  set textContent(value) { this._text = String(value); this.children.forEach(child => { child.parentNode = null; }); this.children = []; }
  get textContent() { return this._text + this.children.map(child => child.textContent).join(''); }
  set innerHTML(value) { this.replaceChildren(); this._html = value; }
  get innerHTML() { return this._html ?? ''; }
  append(...children) { for (let child of children) { if (typeof child === 'string') child = Object.assign(new Element('#text', this.ownerDocument), { textContent: child }); if (child.parentNode) child.remove(); child.parentNode = this; this.children.push(child); } }
  get parentElement() { return this.parentNode; }
  appendChild(child) { this.append(child); return child; }
  prepend(...children) { children.reverse().forEach(child => { child.parentNode = this; this.children.unshift(child); }); }
  replaceChildren(...children) { this.children.forEach(child => { child.parentNode = null; }); this.children = []; this._text = ''; this.append(...children); }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); this.parentNode = null; this.isConnected = false; }
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  removeAttribute(name) { this.attributes.delete(name); }
  addEventListener(type, fn) { const listeners = this.listeners.get(type) ?? []; listeners.push(fn); this.listeners.set(type, listeners); }
  removeEventListener(type, fn) { this.listeners.set(type, (this.listeners.get(type) ?? []).filter(x => x !== fn)); }
  dispatch(type, detail = {}) { const event = { type, target: this, preventDefault() {}, stopPropagation() {}, ...detail }; for (const fn of this.listeners.get(type) ?? []) fn(event); this[`on${type}`]?.(event); }
  click() { if (!this.disabled) this.dispatch('click'); }
  focus() { this.ownerDocument.activeElement = this; }
  blur() { this.dispatch('blur'); }
  contains(node) { return node === this || this.children.some(child => child.contains(node)); }
  closest(selector) { return this.matches(selector) ? this : this.parentNode?.closest(selector) ?? null; }
  matches(selector) { const tag = selector.match(/^[a-z]+/i)?.[0], cls = selector.match(/\.([\w-]+)/)?.[1], attr = selector.match(/\[([\w-]+)(?:=['"]?([^'"\]]+)['"]?)?\]/);
    if (tag && this.tagName !== tag.toUpperCase() || cls && !this.className.split(/\s+/).includes(cls)) return false;
    if (attr) { const actual = this[attr[1]] ?? this.getAttribute(attr[1]); if (attr[2] === undefined ? actual == null : String(actual) !== attr[2]) return false; } return !!(tag || cls || attr); }
  querySelectorAll(selector) { return this.children.flatMap(child => [...(selector.split(',').some(part => child.matches(part.trim())) ? [child] : []), ...child.querySelectorAll(selector)]); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  getBoundingClientRect() { return { left: 0, right: 800, top: 0, bottom: 600 }; }
}
function dom() { const doc = { activeElement: null, createElement(tag) { return new Element(tag, this); }, addEventListener() {}, removeEventListener() {}, querySelector() { return null; } }; doc.body = doc.createElement('body'); globalThis.document = doc; return doc; }
class Grid { constructor(callbacks) { this.callbacks = callbacks; this.el = document.createElement('div'); this.el.className = 'grid-wrap'; } setData(data) { this.data = data; } dispose() { this.disposed = true; } }
class Editor { constructor(options) { this.options = options; this.sql = options.value; this.el = document.createElement('div'); this.el.className = 'cm-editor'; } getSql() { return this.sql; } setSql(sql) { this.sql = sql; this.options.onChange?.(sql); } getSelection() { return this.selection ?? ''; } hasSelection() { return !!this.selection; } setCompletionData() {} focus() { this.el.focus(); } destroy() { this.disposed = true; } }
const plain = value => JSON.parse(JSON.stringify(value));
const tick = () => new Promise(resolve => setTimeout(resolve, 0));
function deferred() { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
const text = value => ({ kind: 'text', value });
const integer = value => ({ kind: 'integer', value });
const revision = 'a'.repeat(64);
function serverSnapshot(context, { prefix = 3, total = 3 } = {}) {
  const editability = { source_execution_id: context.execution_id, result_index: context.result_index, generation: randomUUID(), result_token: randomUUID(), mode: 'single_table', reason: '', schema: 'main', table: 'items', metadata_revision: revision,
    columns: [{ result_column_id: 'id-source', result_index: 0, name: 'id', base_column: 'id', data_type: 'INTEGER', writable: false, key: true, generated: false, reason: '定位键只读' },
      { result_column_id: 'label-source', result_index: 1, name: 'label', base_column: 'label', data_type: 'TEXT', writable: true, key: false, generated: false, reason: '' }], key_column_ids: ['id-source'],
    actions: { update: { supported: true, reason: '' }, delete: { supported: true, reason: '' }, insert: { supported: false, reason: '未验证新增列属性' } } };
  return { editability, rows: Array.from({ length: prefix }, (_, i) => ({ token: randomUUID(), row_token: randomUUID(), values: [integer(String(i + 1)), text(`label-${i + 1}`)] })), loaded_rows: prefix, total_rows: total };
}
function makePlan() { return { plan_id: randomUUID(), sql: 'UPDATE "items" SET "label"=? WHERE "id"=? AND "label"=?', binds: [text('new'), integer('1'), text('label-1')], base_revision: revision, expected_rows: 1, summary: '受控行写入' }; }
function receipt(plan) { return { plan_id: plan.plan_id, status: 'applied', transaction: 'committed', affected: 1, verified: true, mismatches: [], sql: plan.sql }; }
const appSource = fs.readFileSync('src/core/App.ts', 'utf8'), ast = ts.createSourceFile('App.ts', appSource, ts.ScriptTarget.ES2021, true);
const classSource = ast.statements.filter(node => ts.isClassDeclaration(node) && ['App', 'TabView', 'QueryTabView'].includes(node.name?.text)).map(node => node.getText(ast).replace(/^export /, '')).join('\n');
function fixture({ snapshot, execute, grant = false, total = 3, pageSize = 200 } = {}) {
  const doc = dom(), calls = [], notices = [], recovery = [], plans = new Map(), favorites = []; let choice = 'discard', confirmAnswer = true, saveFails = false, latestExecution, snapshotHandler = snapshot;
  const cfg = { id: 'connection-a', name: 'SQLite', kind: 'sqlite', read_only: false };
  const api = { executionCapabilities: async () => ({ cancel: { level: 'unsupported', reason: 'fixture' } }),
    executeSqlV2: async request => { calls.push(['sql', plain(request)]); latestExecution = request.execution_id; return { execution_id: request.execution_id, state: 'succeeded', deadline: {}, effects: 'read_only', transaction_state: 'none', result: { elapsed_ms: 2, messages: [], spool_id: randomUUID(), sets: [{ columns: [{ name: 'id', data_type: 'INTEGER' }, { name: 'label', data_type: 'TEXT' }], rows: Array.from({ length: Math.min(total, pageSize) }, (_, i) => [String(i + 1), `label-${i + 1}`]), total, affected: 0, message: '' }] } }; },
    cancelExecution: async () => ({}), executionStatus: async () => ({}), dropQuerySpool: async scope => { calls.push(['drop-spool', plain(scope)]); },
    fetchQueryRows: async (scope, index, offset, limit) => { calls.push(['page', { scope, index, offset, limit }]); const n = Math.min(limit, total - offset); return { rows: Array.from({ length: n }, (_, i) => [String(offset + i + 1), `label-${offset + i + 1}`]), offset, total, truncated: false, message: '' }; },
    exportQuerySpooled: async (...args) => { calls.push(['export', args]); return total; },
    executeRowMutation: async (planId, ownerId) => { calls.push(['execute', { planId, ownerId }]); return execute ? execute(planId, plans.get(planId)) : receipt(plans.get(planId)); },
  };
  if (grant) api.issueGrant = async request => ({ ...request, execution_id: randomUUID(), execution_grant: 'server-grant' });
  api.addFavorite = async favorite => { calls.push(['addFavorite', plain(favorite)]); if (saveFails) throw new Error('storage full'); favorites.push(favorite); return [...favorites]; };
  globalThis.__PL21_HOST_INVOKE__ = async (command, args) => { calls.push([command, plain(args)]);
    if (command === 'query_result_snapshot') return snapshotHandler ? snapshotHandler(args.context) : serverSnapshot(args.context, { prefix: Math.min(total, 10_000), total });
    if (command === 'preview_query_row_mutation') { const plan = makePlan(); plans.set(plan.plan_id, plan); return plan; }
    if (command === 'release_query_result') return null;
    throw new Error(`unexpected IPC ${command}`);
  };
  const context = vm.createContext({ ...modules, ...transactionFixtureBindings, exports: {}, document: doc, window: { getSelection: () => null, addEventListener() {}, removeEventListener() {} }, HTMLElement: Element, HTMLInputElement: class extends Element {},
    DataGrid: Grid, DataTabView: class {}, SqlEditor: Editor, crypto: globalThis.crypto, structuredClone, TextEncoder, setTimeout, clearTimeout, console, api,
    showDraftText: (...args) => recovery.push(args), iconSvgByName: () => '', svg: () => '', renderCellPreview() {}, explainQuerySql: sql => `EXPLAIN ${sql}`, logStamp: () => '2026-01-01 00:00:00',
    ICON_PAGE_FIRST: {}, ICON_PAGE_PREV: {}, ICON_PAGE_NEXT: {}, ICON_PAGE_LAST: {}, GRID_CONTEXT_MENU: [], encodeGridCopy() { throw new Error('must not copy the stale display grid'); },
    chooseLeave: async () => choice, confirmTransactionAction: async () => confirmAnswer, openObjectAction: () => ({ onClose() {}, close() {}, showError() {} }),
  });
  vm.runInContext(ts.transpile(classSource + '\nObject.assign(globalThis, { App, QueryTabView });', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const app = Object.create(context.App.prototype), tab = { id: 'query-owner', kind: 'query', title: '查询', connectionId: cfg.id, schema: 'main', sql: 'SELECT id,label FROM items', activeResult: -1, results: null, dirty: false, selectedRows: new Set() };
  Object.assign(app, { settings: {}, draftPreparing: false, favorites, publishFavorites() {}, connections: [cfg], tabs: [tab], views: new Map(), activeTabId: tab.id, contentHost: doc.createElement('main'), copyOwner: null, copyContext: null,
    // App construction is intentionally bypassed; preserve its toolbar boundary for the real leave guard.
    ribbon: { updateDisabled() { calls.push(['ribbon-update']); } },
    connectionById: id => id === cfg.id ? cfg : undefined, connectionsValue: () => [cfg], manualTransactionFor: () => undefined, manualTransactionIsBusy: () => false,
    refreshManualTransactionStatus: async () => {}, renderTabs() {}, renderTabsPublic() {}, renderStatus() {}, notifyToolbar() {}, draftChanged() {}, toast: (...args) => notices.push(args.join(':')), skipNullState: () => false, showGridLinesState: () => true,
  });
  const view = new context.QueryTabView(app, tab); view.resultPages.limit = pageSize; view.completionLoaded = true; app.views.set(tab.id, view); doc.body.append(view.el);
  return { app, view, tab, cfg, doc, api, calls, notices, recovery, plans, favorites, context, choose(value) { choice = value; }, answerConfirm(value) { confirmAnswer = value; }, failSave() { saveFails = true; }, snapshot(value) { snapshotHandler = value; }, latestExecution: () => latestExecution };
}
async function run(f) { await f.view.executeSelection(false); for (let i = 0; i < 8 && f.view.resultEvidenceLoading; i++) await tick(); assert.equal(f.view.resultEvidenceLoading, false); return f.view.controlledResult; }
function stage(panel, value = 'new') { const row = panel.snapshot.rows[0]; panel.selectTokens([row.token]); panel.stageCell(row.token, 'label-source', text(value)); return row; }
function consent(panel) { const input = panel.el.querySelector('.qr-review').querySelector('input[type="checkbox"]'); input.checked = true; input.dispatch('change'); }

const validContext = () => ({ execution_id: randomUUID(), connection_id: 'c', owner_id: 'o', namespace: 'main', result_index: 0 });
const page = { columns: [{ name: 'id' }, { name: 'label' }], offset: 0, loaded_rows: 3, total_rows: 3 };
test('IPC adapter accepts only actual executed identity and typed captured prefix, not display values', () => {
  const ctx = validContext(), raw = serverSnapshot(ctx); const value = modules.querySnapshotForPage(raw, ctx, { ...page, offset: 1, loaded_rows: 1 });
  assert.equal(value.rows[0].row_token, raw.rows[1].row_token); assert.deepEqual(value.rows[0].values, [integer('2'), text('label-2')]); assert.equal(value.columns[0].id, 'id-source');
  value.rows[0].values[1] = text('client'); assert.equal(raw.rows[1].values[1].value, 'label-2');
});
test('IPC evidence rejects foreign execution/result, malformed typed values, duplicate tokens, counts, projection and prefix overrun', async t => {
  const mutations = [raw => { raw.editability.source_execution_id = randomUUID(); }, raw => { raw.editability.result_index = 1; }, raw => { raw.editability.generation = ''; },
    raw => { raw.rows[0].row_token = ''; }, raw => { raw.rows[1].row_token = raw.rows[0].row_token; }, raw => { raw.rows[1].token = raw.rows[0].token; },
    raw => { raw.rows[0].values[0] = { kind: 'integer', value: 9007199254740993 }; }, raw => { raw.rows[0].values[0] = { kind: 'integer', value: '18446744073709551616' }; },
    raw => { raw.loaded_rows = 2; }, raw => { raw.total_rows = 4; }, raw => { raw.editability.columns[0].name = 'foreign'; }, raw => { raw.rows[0].values.pop(); }, raw => { raw.sql = 'SELECT again'; },
    raw => { raw.rows = raw.rows.slice(0, 1); raw.loaded_rows = 1; }];
  for (const [i, change] of mutations.entries()) await t.test(`invalid evidence ${i + 1}`, () => { const ctx = validContext(), raw = serverSnapshot(ctx); change(raw); assert.throws(() => modules.querySnapshotForPage(raw, ctx, page)); });
});
test('successful C1 uses the actual granted ID and mounts the real panel; C2 carries opaque IDs and the original owner only', async () => {
  const f = fixture({ grant: true }), panel = await run(f); assert.ok(panel instanceof modules.QueryResultPanel); const snapshotCall = f.calls.find(([name]) => name === 'query_result_snapshot')[1];
  assert.equal(snapshotCall.context.execution_id, f.latestExecution()); assert.equal(snapshotCall.context.owner_id, f.tab.id); assert.deepEqual(Object.keys(snapshotCall.context).sort(), ['connection_id','execution_id','namespace','owner_id','result_index']);
  const row = stage(panel); assert.equal(await panel.previewSelected(), true); const preview = f.calls.find(([name]) => name === 'preview_query_row_mutation')[1];
  assert.equal(preview.change.row_token, row.row_token); assert.deepEqual(preview.change.changes, [['label-source', text('new')]]); assert.equal('sql' in preview.change, false); consent(panel);
  assert.equal(await panel.confirmPreview(), true); assert.equal(f.calls.filter(([name]) => name === 'execute').length, 1); assert.equal(f.calls.find(([name]) => name === 'execute')[1].ownerId, f.tab.id);
  assert.equal(f.calls.filter(([name]) => name === 'sql').length, 1); f.view.destroy();
});
test('dirty typed rows or raw field input block rerun, paging, context change and result-tab replacement without data loss', async () => {
  const f = fixture({ total: 6, pageSize: 3 }), panel = await run(f); stage(panel); const before = plain(panel.captureDraft());
  await f.view.executeSelection(false); await f.view.reload(); await f.view.loadResultPage('next'); await f.view.switchConnection('other'); assert.throws(() => f.view.clearResultSnapshot());
  f.view.resultTabsEl.querySelectorAll('button').find(button => button.textContent === '消息').click(); assert.equal(f.tab.activeResult, 0);
  assert.equal(f.calls.filter(([name]) => name === 'sql').length, 1); assert.equal(f.calls.filter(([name]) => name === 'page').length, 0); assert.deepEqual(plain(panel.captureDraft()), before); assert.equal(f.view.draftDirty(), true);
  f.view.editor.setSql('changed SQL'); f.view.markClean(); assert.equal(f.view.draftDirty(), true, 'saving SQL does not clear result draft dirty');
  panel.discardSelected(); panel.selectTokens([panel.snapshot.rows[0].token]); const input = panel.el.querySelector('.qr-editor').querySelector('textarea'); input.value = 'unapplied'; input.dispatch('input');
  await f.view.executeSelection(false); assert.equal(panel.captureDraft().editor.raw, 'unapplied'); assert.equal(f.calls.filter(([name]) => name === 'sql').length, 1); f.view.destroy();
});
test('close guard asks before discarding dirty result edits; cancel keeps the page and unknown writes block closing', async () => {
  const f = fixture({ execute: async () => { throw new Error('ACK lost'); } }), panel = await run(f); stage(panel);
  f.choose('cancel'); assert.equal(await f.app.closeTabs([f.tab.id]), false); assert.equal(f.app.views.get(f.tab.id), f.view);
  assert.equal(f.calls.filter(([name]) => name === 'ribbon-update').length, 1); assert.equal(f.app.draftPreparing, false);
  assert.equal(await panel.previewSelected(), true); consent(panel); assert.equal(await panel.confirmPreview(), false); assert.equal(f.view.draftBusy(), true); assert.equal(await f.app.closeTabs([f.tab.id]), false);
  assert.throws(() => f.view.destroy(), /禁止销毁/);
  assert.equal(f.calls.filter(([name]) => name === 'execute').length, 1); assert.equal(f.calls.filter(([name]) => name === 'sql').length, 1); assert.equal(f.view.controlledResult, panel);
  f.view.showResultRecovery(); assert.match(f.recovery.at(-1)[1], /ACK lost/);
});
test('explicit release discards row drafts, keeps the old read-only spool, then permits a deliberate rerun', async () => {
  const f = fixture(), panel = await run(f); stage(panel); f.answerConfirm(true);
  await f.view.releaseResultAfterConfirm(); await tick();
  assert.equal(f.view.controlledResult, null); assert.ok(f.tab.results.spool_id);
  assert.equal(f.calls.filter(([name]) => name === 'release_query_result').length, 1); assert.equal(f.calls.filter(([name]) => name === 'sql').length, 1);
  await run(f); assert.equal(f.calls.filter(([name]) => name === 'sql').length, 2); f.view.destroy();
});
test('after token-prefix exhaustion the entire spool page stays read-only; pagination/export do not run SQL again', async () => {
  const f = fixture({ total: 7, pageSize: 3, snapshot: ctx => serverSnapshot(ctx, { prefix: 3, total: 7 }) }); assert.ok(await run(f)); await f.view.loadResultPage('next'); for (let i = 0; i < 5; i++) await tick();
  assert.equal(f.view.controlledResult, null); assert.equal(f.view.resultPages.page(0).offset, 3); assert.equal(f.view.grid.data.rows[0][0], '4'); assert.equal(f.view.grid.data.readOnly, true); assert.match(f.view.resultEvidenceReason, /前缀/);
  const scope = f.view.captureResultExport(); assert.equal(await f.view.exportResultSnapshot({}, scope), 7); assert.equal(f.calls.filter(([name]) => name === 'sql').length, 1); f.view.destroy();
});
test('read-only/foreign/failed evidence preserves the prior display grid and exposes the concrete reason', async () => {
  for (const reason of ['JOIN 查询缺少单表来源证明', '窗口或端点不属于当前结果']) {
    const f = fixture({ snapshot: async () => { throw { code: 'QUERY_RESULT_READ_ONLY', message: reason, retryable: false }; } }); assert.equal(await run(f), null);
    assert.equal(f.view.grid.data.readOnly, true); assert.equal(f.view.grid.data.rows[0][1], 'label-1'); assert.match(f.view.resultEvidenceReason, new RegExp(reason)); assert.equal(f.calls.filter(([name]) => name === 'sql').length, 1); f.view.destroy();
  }
});
test('late snapshot cannot mount after disposal or a changed execution/owner/namespace; release names only the old context', async () => {
  for (const change of [f => f.view.destroy(), f => { f.tab.id = 'foreign-owner'; }, f => { f.tab.schema = 'other'; }, f => { f.view.executionRevision++; f.view.resultExecution = null; }]) {
    const gate = deferred(), f = fixture({ snapshot: ctx => { f.pendingContext = ctx; return gate.promise; } }); await f.view.executeSelection(false); assert.equal(f.view.resultEvidenceLoading, true); change(f);
    gate.resolve(serverSnapshot(f.pendingContext)); for (let i = 0; i < 4; i++) await tick(); assert.equal(f.view.controlledResult, null); assert.equal(f.calls.filter(([name]) => name === 'execute').length, 0);
  }
});
test('copying from the controlled result never falls through to stale display-grid rows or hidden columns', async () => {
  const f = fixture(), panel = await run(f), row = panel.snapshot.rows[0]; panel.selectTokens([row.token]); panel.setHiddenColumns(['label-source']); panel.grid.el.focus();
  const clipboard = []; f.app.writeClipboard = async value => clipboard.push(value); await f.app.cmdCopy('rows_with_headers', true);
  assert.equal(clipboard[0], 'id\r\n1'); assert.equal(clipboard[0].includes('label'), false); f.view.destroy();
});
test('closing saves unsaved SQL to the bound query when chosen, and closes without a write when not', async () => {
  for (const choice of ['discard', 'save']) {
    const f = fixture(), panel = await run(f); stage(panel); f.tab.savedQueryId = 'f_saved';
    f.favorites.push({ id: 'f_saved', name: '已存查询', sql: 'SELECT id,label FROM items', connection_id: 'connection-a', schema: 'main', timestamp: 0 });
    f.view.editor.setSql('SELECT 2'); f.choose(choice);
    assert.equal(await f.app.closeTabs([f.tab.id]), true); await tick();
    assert.equal(f.calls.filter(([name]) => name === 'ribbon-update').length, 1); assert.equal(f.app.draftPreparing, false);
    assert.equal(f.app.views.has(f.tab.id), false); assert.equal(f.tab.id === f.app.activeTabId, false);
    assert.equal(f.calls.filter(([name]) => name === 'release_query_result').length, 1); assert.equal(f.calls.filter(([name]) => name === 'sql').length, 1);
    const saves = f.calls.filter(([name]) => name === 'addFavorite');
    if (choice === 'save') { assert.equal(saves.length, 1); assert.equal(saves[0][1].sql, 'SELECT 2'); assert.equal(saves[0][1].name, '已存查询'); }
    else assert.equal(saves.length, 0);
  }
});
test('a failed query save keeps the page and its edits open', async () => {
  const f = fixture(), panel = await run(f); f.tab.savedQueryId = 'f_saved';
  f.favorites.push({ id: 'f_saved', name: '已存查询', sql: 'SELECT id,label FROM items', connection_id: 'connection-a', schema: 'main', timestamp: 0 });
  f.view.editor.setSql('SELECT 3'); f.choose('save'); f.failSave();
  assert.equal(await f.app.closeTabs([f.tab.id]), false); assert.equal(f.app.views.get(f.tab.id), f.view); assert.equal(f.view.editor.getSql(), 'SELECT 3');
  assert.equal(f.calls.filter(([name]) => name === 'ribbon-update').length, 1); assert.equal(f.app.draftPreparing, false);
  assert.ok(f.notices.some(value => value.includes('未离开当前页面')));
});
test('pending preview and pending execute prevent host rerun, close, refresh and page movement', async () => {
  const previewGate = deferred(), executeGate = deferred(), f = fixture({ execute: () => executeGate.promise }), panel = await run(f); stage(panel);
  const invoke = globalThis.__PL21_HOST_INVOKE__; globalThis.__PL21_HOST_INVOKE__ = (command, args) => command === 'preview_query_row_mutation' ? previewGate.promise : invoke(command, args);
  const previewing = panel.previewSelected(); await tick(); assert.equal(f.view.draftBusy(), true); assert.equal(await f.app.closeTabs([f.tab.id]), false); await f.view.executeSelection(false); await f.view.loadResultPage('next');
  assert.equal(f.calls.filter(([name]) => name === 'ribbon-update').length, 1); assert.equal(f.app.draftPreparing, false);
  const plan = makePlan(); previewGate.resolve(plan); assert.equal(await previewing, true); consent(panel); const confirming = panel.confirmPreview(); await tick(); assert.equal(f.view.draftBusy(), true); await f.view.reload(); assert.equal(await f.app.closeTabs([f.tab.id]), false);
  executeGate.resolve(receipt(plan)); assert.equal(await confirming, true); assert.equal(f.calls.filter(([name]) => name === 'execute').length, 1); assert.equal(f.calls.filter(([name]) => name === 'sql').length, 1); assert.equal(f.calls.filter(([name]) => name === 'page').length, 0); f.view.destroy();
});
test('stale source or a read-only connection is rejected by the host before requesting a new row plan', async () => {
  for (const mode of ['stale', 'read-only', 'owner', 'namespace']) {
    const f = fixture(), panel = await run(f); stage(panel); if (mode === 'stale') f.tab.datasourceStale = 'server changed'; if (mode === 'read-only') f.cfg.read_only = true; if (mode === 'owner') f.tab.id = 'new-owner'; if (mode === 'namespace') f.tab.schema = 'other';
    assert.equal(await panel.previewSelected(), false); assert.equal(f.calls.filter(([name]) => name === 'preview_query_row_mutation').length, 0); assert.equal(f.calls.filter(([name]) => name === 'execute').length, 0); assert.equal(panel.dirty, true); f.view.destroy();
  }
});
