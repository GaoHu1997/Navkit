// Behavior regressions for the three real-IPC user-test defects. No database/network is used here.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
// Opt-in original-source replay proves these tests fail before the fixes, without restoring any product file.
const baseline = process.env.NAVKIT_USER_FLOW_BASELINE;
const appText = fs.readFileSync(baseline ? path.resolve(root, baseline, 'App.ts') : path.join(root, 'src/core/App.ts'), 'utf8');
const appSource = ts.createSourceFile('App.ts', appText, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
const connectionText = fs.readFileSync(baseline ? path.resolve(root, baseline, 'ConnectionDialog.ts') : path.join(root, 'src/dialogs/ConnectionDialog.ts'), 'utf8');
const dialogText = fs.readFileSync(path.join(root, 'src/ui/Dialog.ts'), 'utf8');
const runtimeText = fs.readFileSync(path.join(root, 'src/ui/RuntimeProfileEditor.ts'), 'utf8');
const runtimeHelpers = fs.readFileSync(path.join(root, 'src/core/runtime-profile-editor.ts'), 'utf8');
const sourceClass = name => {
  const result = appSource.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === name);
  assert(result, `Missing production class ${name}`); return result;
};
const members = (name, names) => sourceClass(name).members.filter(node => names.includes(node.name?.getText(appSource))).map(node => node.getText(appSource)).join('\n');
const stripImports = text => text.replace(/^import[\s\S]*?;\s*/gm, '').replace(/^export\s+/gm, '');
const runTs = (text, context) => vm.runInContext(ts.transpileModule(text, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None } }).outputText, context);
const plain = value => JSON.parse(JSON.stringify(value));

class Element {
  constructor(tag = 'div') {
    this.tagName = tag.toLowerCase(); this.children = []; this.parentNode = null;
    this.style = {}; this.dataset = {}; this.attributes = new Map(); this.listeners = new Map();
    this.className = ''; this.value = ''; this.checked = false; this.disabled = false; this.hidden = false;
    this._text = ''; this._html = ''; this.id = '';
    this.classList = {
      add: (...names) => { this.className = [...new Set([...this.className.split(/\s+/).filter(Boolean), ...names])].join(' '); },
      remove: (...names) => { this.className = this.className.split(/\s+/).filter(name => !names.includes(name)).join(' '); },
      toggle: (name, enabled) => { const yes = enabled ?? !this.classList.contains(name); yes ? this.classList.add(name) : this.classList.remove(name); return yes; },
      contains: name => this.className.split(/\s+/).includes(name),
    };
  }
  set textContent(value) { this._text = String(value); this.children = []; }
  get textContent() { return this._text + this.children.map(child => child.textContent).join(''); }
  set innerHTML(value) { this._html = String(value); this._text = ''; this.children = []; }
  get innerHTML() { return this._html; }
  appendChild(child) { child.remove(); this.children.push(child); child.parentNode = this; return child; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  replaceChildren(...children) { this.children = []; children.forEach(child => this.appendChild(child)); }
  insertBefore(child, before) { child.remove(); const index = this.children.indexOf(before); this.children.splice(index < 0 ? this.children.length : index, 0, child); child.parentNode = this; }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); this.parentNode = null; }
  get firstChild() { return this.children[0] ?? null; }
  get isConnected() { return this.parentNode !== null; }
  setAttribute(name, value) { this.attributes.set(name, String(value)); if (name === 'id') this.id = String(value); }
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  removeAttribute(name) { this.attributes.delete(name); }
  addEventListener(name, handler) { const handlers = this.listeners.get(name) ?? []; handlers.push(handler); this.listeners.set(name, handlers); }
  async emit(name, extra = {}) { for (const handler of this.listeners.get(name) ?? []) await handler({ target: this, preventDefault() {}, ...extra }); }
  focus() { if (this.ownerDocument) this.ownerDocument.activeElement = this; }
  checkValidity() { return this.type !== 'number' || Number.isFinite(Number(this.value)) && Number(this.value) >= Number(this.min ?? 0) && Number(this.value) <= Number(this.max ?? 65535); }
  getClientRects() { return this.hidden ? [] : [{}]; }
  matches(selector) {
    const excluded = [...selector.matchAll(/:not\(([^)]+)\)/g)].map(match => match[1]);
    if (excluded.some(part => this.matches(part))) return false;
    selector = selector.replace(/:not\([^)]+\)/g, '').trim();
    const tag = selector.match(/^[\w-]+/); if (tag && this.tagName !== tag[0]) return false;
    if ([...selector.matchAll(/\.([\w-]+)/g)].some(match => !this.classList.contains(match[1]))) return false;
    for (const match of selector.matchAll(/\[([\w-]+)(?:=['"]?([^'"\]]+)['"]?)?\]/g)) {
      const value = this.getAttribute(match[1]) ?? (match[1] === 'disabled' ? (this.disabled ? '' : null) : this[match[1]] ?? null);
      if (value === null || match[2] !== undefined && String(value) !== match[2]) return false;
    }
    return true;
  }
  querySelectorAll(selector) {
    const alternatives = selector.split(',').map(part => part.trim()); const result = [];
    const visit = node => { for (const child of node.children) { if (alternatives.some(part => child.matches(part))) result.push(child); visit(child); } };
    visit(this); return result;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}
function environment() {
  const document = { addEventListener() {}, removeEventListener() {}, activeElement: null, createElement: tag => { const element = new Element(tag); element.ownerDocument = document; return element; } };
  document.body = document.createElement('body');
  return vm.createContext({ document, console, TextEncoder, TextDecoder, setTimeout, clearTimeout, Error, Map, Set, Date, Math,
    window: { addEventListener() {}, removeEventListener() {}, innerWidth: 1600, innerHeight: 900 },
    svg: () => '', ICON_CROSS: {}, iconSvgByName: () => '', kindIcon: kind => kind, kindLabel: kind => kind === 'sqlite' ? 'SQLite' : 'MySQL',
    kindMeta: kind => ({ protocol: kind === 'sqlite' ? 'sqlite' : 'mysql', port: kind === 'sqlite' ? 0 : 3306 }),
    parseDriverParams: () => null, parseVersionSelection: () => null, currentDriverStatuses: () => [],
    ensureDriverStatuses: async () => [], encodeVersionParams: () => '', encodeDriverParams: () => '',
    driverApi: { testConnection: async () => ({ ok: true }), list: async () => [] }, openFile: async () => null,
  });
}
function treeHarness(kind = 'mysql') {
  const gates = [], errors = [];
  const context = environment();
  context.api = { listDatabases: () => new Promise((resolve, reject) => gates.push({ resolve, reject })),
    listTables: async () => [['fresh_table', 'table']] };
  context.namespaceType = () => 'database'; context.namespaceName = () => '数据库';
  // refreshTree 依赖 TreePanel.buildGroups；默认返回空，测试可覆盖。
  context.TreePanel = { buildGroups: () => [] };
  runTs(`globalThis.TreeApp = class { treeLoads = new WeakMap(); ${members('App', ['toggleNode', 'loadChildren', 'cmdTreeRefresh', 'cmdCloseConnection', 'isRefreshableContainer', 'refreshTree', 'reloadExpandedContainers'])} };`, context);
  const app = new context.TreeApp();
  const node = { id: 'conn:a', type: 'connection', children: [], expandable: true, expanded: false, loaded: false };
  let live = node;
  Object.assign(app, {
    connections: [{ id: 'a', kind }], favorites: [], openConnections: new Set(['a']), openDbs: new Set(), tabs: [],
    tree: { find: id => id === live?.id ? live : undefined, selectedNodeId: node.id, render() {} },
    ensureConnection: async () => {}, syncConnIcons() {}, toast: (...args) => errors.push(args),
    currentConnectionId: () => 'a', guardLeave: async (_ids, _label, action) => { await action(); return true; },
    manualTransactionController: { closeConnection: async () => true }, suspendDatasourceTabs() {}, stopKeepAlive() {},
  });
  return { app, node, gates, errors, context, replace: replacement => { live = replacement; } };
}
const treeSettle = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };

test('D5: collapse/re-expand refreshes added and removed databases and retains unchanged subtrees', async () => {
  const h = treeHarness();
  const first = h.app.toggleNode(h.node); await treeSettle();
  h.gates[0].resolve([['keep'], ['removed']]); await first;
  const keep = h.node.children[0]; keep.expanded = keep.loaded = true; keep.children = [{ id: 'cached-table' }];
  await h.app.toggleNode(h.node); assert.equal(h.node.loaded, false);
  const second = h.app.toggleNode(h.node); await treeSettle();
  assert.equal(h.gates.length, 2);
  h.gates[1].resolve([['keep'], ['created']]); await second;
  assert.deepEqual(plain(h.node.children.map(child => child.label)), ['keep', 'created']);
  assert.equal(h.node.children[0], keep); assert.equal(keep.expanded, true);
  assert.equal(keep.children[0].id, 'cached-table'); assert.equal(h.node.loading, false);
});

// 第四轮实测缺陷（B6）：库/分组节点折叠后仍沿用旧 children，
// 导致其他客户端新建的表/视图在树里一直不出现，必须重开连接才可见。
test('B6: collapsing a database or object group invalidates its children so new objects appear', async () => {
  const h = treeHarness();
  const db = { id: 'conn:a/db/shop', label: 'shop', type: 'database', children: [],
    expandable: true, expanded: true, loaded: true };
  // 库节点下的"表"分组是另一个刷新型容器。
  const group = { id: 'conn:a/db/shop/grp-table', label: '表', type: 'folder', children: [],
    expandable: true, expanded: true, loaded: true };
  db.children = [group];
  Object.assign(h.app, { tree: { find: id => (id === db.id ? db : id === group.id ? group : undefined), selectedNodeId: db.id, render() {} } });

  await h.app.toggleNode(db);
  assert.equal(db.expanded, false);
  assert.equal(db.loaded, false, '库节点折叠后必须失效，不能沿用缓存');
  assert.deepEqual(plain(db.children), [], '旧对象列表必须被丢弃，否则新表永远不出现');

  await h.app.toggleNode(group);
  assert.equal(group.expanded, false);
  assert.equal(group.loaded, false, '对象分组折叠后同样必须失效');
  assert.deepEqual(plain(group.children), []);
});

test('B6: structural detail groups stay cached (fields/indexes are not re-read on every collapse)', async () => {
  const h = treeHarness();
  const detail = { id: 'conn:a/db/shop/table/orders/grp-column', label: '字段', type: 'folder',
    children: [{ id: 'cached-column' }], expandable: true, expanded: true, loaded: true };
  Object.assign(h.app, { tree: { find: id => (id === detail.id ? detail : undefined), selectedNodeId: detail.id, render() {} } });

  await h.app.toggleNode(detail);
  assert.equal(detail.expanded, false);
  assert.equal(detail.loaded, true, '字段等结构详情不应因折叠而失效');
  assert.deepEqual(plain(detail.children.map(child => child.id)), ['cached-column'], '缓存必须保留');
});

// refreshTree 的重载路径依赖真实 Tree 实例的对象身份（tree.find 返回同一节点），
// vm harness 无法忠实复现；此处只锁定"哪些节点算刷新型容器"的判定，
// 以及"折叠即失效"的行为，重载本身由真机回归覆盖。
test('B6: refreshable-container classification covers databases and object groups only', async () => {
  const h = treeHarness();
  const kindOf = node => h.app.isRefreshableContainer(node);
  const make = (id, expandable = true) => ({ id, expandable });

  assert.equal(kindOf(make('conn:a/db/shop')), true, '库/模式节点必须可刷新');
  for (const grp of ['table', 'view', 'function', 'event', 'sequence', 'query', 'backup']) {
    assert.equal(kindOf(make(`conn:a/db/shop/grp-${grp}`)), true, `${grp} 分组必须可刷新`);
  }
  // 表内结构详情：变化少、重查贵，不应因折叠而失效。
  for (const grp of ['column', 'index', 'fk', 'trigger']) {
    assert.equal(kindOf(make(`conn:a/db/shop/table/orders/grp-${grp}`)), false, `${grp} 属于结构详情，不应刷新`);
  }
  // 表/视图对象自身与连接节点由各自的既有逻辑处理。
  assert.equal(kindOf(make('conn:a/db/shop/table/orders')), false);
  assert.equal(kindOf(make('conn:a')), false);
  assert.equal(kindOf(make('conn:a/db/shop', false)), false, '不可展开的节点不可能刷新');
});

test('D5: failed refresh remains retryable, including an empty successful database list', async () => {
  const h = treeHarness();
  const first = h.app.toggleNode(h.node); await treeSettle(); h.gates[0].reject(Error('metadata failed')); await first;
  assert.equal(h.node.loaded, false); assert.equal(h.node.loading, false); assert.equal(h.node.expanded, false);
  assert.equal(h.errors.length, 1);
  const second = h.app.toggleNode(h.node); await treeSettle(); h.gates[1].resolve([]); await second;
  assert.equal(h.node.loaded, true);
  await h.app.toggleNode(h.node);
  const third = h.app.toggleNode(h.node); await treeSettle(); h.gates[2].resolve([['new_db']]); await third;
  assert.equal(h.node.children[0].label, 'new_db');
});

test('D5: superseded load cannot overwrite a re-expanded connection or clear its pending state', async () => {
  const h = treeHarness();
  const old = h.app.toggleNode(h.node); await treeSettle();
  await h.app.toggleNode(h.node);
  const fresh = h.app.toggleNode(h.node); await treeSettle();
  h.gates[0].resolve([['stale']]); await old;
  assert.equal(h.node.children.length, 0); assert.equal(h.node.loading, true);
  h.gates[1].resolve([['fresh']]); await fresh;
  assert.equal(h.node.children[0].label, 'fresh'); assert.equal(h.node.loading, false);
});

test('D5: explicit refresh supersedes pending load; disconnect and node replacement discard late responses', async () => {
  for (const action of ['refresh', 'disconnect', 'replace']) {
    const h = treeHarness(); const pending = h.app.toggleNode(h.node); await treeSettle();
    if (action === 'refresh') { h.app.cmdTreeRefresh(); await treeSettle(); }
    if (action === 'disconnect') await h.app.cmdCloseConnection();
    if (action === 'replace') h.replace({ ...h.node, children: [] });
    h.gates[0].resolve([['stale']]); await pending;
    assert.equal(h.node.children.length, 0, action);
    if (action === 'refresh') {
      assert.equal(h.node.loading, true); h.gates[1].resolve([['fresh']]); await treeSettle();
      assert.equal(h.node.children[0].label, 'fresh');
    }
    if (action === 'disconnect') { assert.equal(h.node.expanded, false); assert.equal(h.node.loaded, false); }
  }
});

test('D5: a late connection handshake cannot mark a collapsed or disconnected node open', async () => {
  const h = treeHarness();
  h.app.openConnections.clear();
  h.app.startKeepAlive = () => { throw Error('stale handshake must not start keepalive'); };
  const context = environment(); let resolve;
  context.api = { ensureConnection: () => new Promise(done => { resolve = done; }) };
  runTs(`globalThis.ensure = class { ${members('App', ['ensureConnection'])} }.prototype.ensureConnection;`, context);
  h.app.ensureConnection = context.ensure;
  const pending = h.app.toggleNode(h.node); await treeSettle();
  await h.app.toggleNode(h.node);
  resolve({ ok: true }); await pending;
  assert.equal(h.app.openConnections.has('a'), false); assert.equal(h.gates.length, 0);
  assert.equal(h.node.loaded, false); assert.equal(h.node.expanded, false);
});

test('D5: SQLite re-expand refreshes tables and database nodes also refresh (B6)', async () => {
  const h = treeHarness('sqlite');
  h.node.children = [{ id: 'stale-table' }]; h.node.loaded = h.node.expanded = true;
  await h.app.toggleNode(h.node);
  const pending = h.app.toggleNode(h.node); await treeSettle(); h.gates[0].resolve([['main']]); await pending;
  assert.equal(h.node.children[0].label, 'fresh_table');
  // B6（第四轮实测）：库/分组折叠后必须失效。旧行为"保持 cached"会让其他客户端
  // 新建的表一直不出现，因此这里断言缓存被丢弃、按需重读。
  const child = { id: 'conn:a/db/main', children: [{ id: 'cached' }], expandable: true, expanded: true, loaded: true };
  await h.app.toggleNode(child);
  assert.equal(child.loaded, false, '库节点折叠后必须失效');
  assert.deepEqual(plain(child.children), [], '旧 children 必须丢弃');
});

function gridView(hidden = []) {
  const context = environment();
  const constructor = sourceClass('DataTabView').members.find(ts.isConstructorDeclaration);
  let callbacks;
  const visit = node => { if (ts.isNewExpression(node) && node.expression.getText(appSource) === 'DataGrid') callbacks = node.arguments[0].getText(appSource); ts.forEachChild(node, visit); };
  visit(constructor); assert(callbacks, 'Missing actual DataTabView grid callbacks');
  runTs(`globalThis.DataView = class { ${members('DataTabView', ['displayRows', 'originalRowCount', 'allColumns', 'orderedColumnNames', 'applyData', 'applyEdit', 'rowFlags'])} }; globalThis.makeCallbacks = function() { return ${callbacks}; };`, context);
  const view = new context.DataView();
  const columns = ['id', 'name', 'score', 'note'].map(name => ({ name }));
  Object.assign(view, {
    tab: { page: { columns, rows: [['1', 'Alice 中文', '10', '首行'], ['2', 'Bob', '20', null]], primary_keys: ['id'] }, selectedRows: new Set() },
    hiddenCols: new Set(hidden), pinnedCols: new Set(), columnOrder: [], pendingInserts: [], pendingInsertEdits: new Map(), pendingUpdates: new Map(), pendingDeletes: new Set(), txAppliedInserts: [], txAppliedUpdates: new Map(), txAppliedDeletes: new Set(),
    app: { renderStatus() {}, showGridLinesState: () => false, skipNullState: () => false, toast() {} },
    appliedSortRules: () => [], renderFooter() {},
    grid: { setData(options) { this.columns = options.columns; this.rows = options.rows; }, beginEdit(row, col, value) { this.edit = { row, col, value }; } },
  });
  view.callbacks = context.makeCallbacks.call(view); view.applyData(); return view;
}
function queryView() {
  const context = environment();
  runTs(`${stripImports(fs.readFileSync(path.join(root, 'src/core/queryResultPages.ts'), 'utf8'))}\nglobalThis.QueryView = class { ${members('QueryTabView', ['resultPages', 'exporting', 'pageButtons', 'resultBusy', 'activeResultTab', 'currentSet', 'statusInfo', 'releaseSpool', 'updateResultLoading', 'renderResultPager', 'renderResults', 'renderMessages', 'runningMessageText', 'requestControlledResult', 'disposeControlledResult', 'allowResultReplacement', 'footerStatsText'])} };`, context);
  const view = new context.QueryView(); const make = () => context.document.createElement('div');
  let statuses = 0, toolbars = 0;
  const first = { columns: [{ name: 'first_marker' }], rows: [['FIRST']], total: 1, has_rows: true };
  const second = { columns: [{ name: 'second_marker' }], rows: [['SECOND_A'], ['SECOND_B']], total: 4, has_rows: true,
    message: '当前仅预览 2 / 4 行；复制、筛选和导出仅包含预览。' };
  Object.assign(view, { tab: { activeResult: 0, title: 'Query', results: { sets: [first, second], elapsed_ms: 25 } },
    executionClockStartedAt: null, executionLabelEl: make(), executionTimerEl: make(),
    editor: { getSelection: () => '', getSql: () => 'SELECT' }, running: false, lastMessages: [{ text: 'OK', error: false }], resultRatio: 0.5,
    resultTabsEl: make(), resultSplitter: make(), resultBody: make(), messagesEl: make(), cellPreviewEl: make(), gridPreviewNoteEl: make(), resultPager: make(), pageControls: make(), pageCountEl: make(), pageNumberInput: make(), pageSizeSelect: Object.assign(make(), { parentElement: make() }), footerStatusEl: make(), pagerDividerEl: make(), pageErrorEl: make(),
    // 结果证据面板宿主（工作树新增）；fixture 提供空元素，证据请求走显式拒绝
    resultPanelHost: make(), resultRecoveryTools: make(), resultEvidenceRequest: 0, resultEvidenceLoading: false,
    app: { renderStatus: () => statuses++, notifyToolbar: () => toolbars++, skipNullState: () => false, showGridLinesState: () => false },
    grid: { el: make(), setData(options) { this.options = options; } },
  });
  view.notifications = () => ({ statuses, toolbars }); view.renderResults();
  view.select = label => view.resultTabsEl.children.find(element => element.textContent === label).emit('click');
  return view;
}
function connection(existing, onSaved, runtimeProfiles) {
  const context = environment(); runTs(stripImports(dialogText), context); runTs(stripImports(runtimeHelpers), context); runTs(stripImports(runtimeText), context);
  const createEditor = context.createRuntimeProfileEditor;
  context.createRuntimeProfileEditor = (...args) => {
    const editor = createEditor(...args); context.runtimeProfileReady = editor.ready; return editor;
  };
  runTs(stripImports(connectionText), context);
  const dialog = context.openConnectionDialog(existing, [], onSaved, 'sqlite', undefined, runtimeProfiles);
  const input = label => dialog.body.querySelectorAll('input').find(element => element.getAttribute('aria-label') === label);
  const button = label => dialog.footer.querySelectorAll('button').find(element => element.textContent === label);
  return { dialog, context, input, button, ready: context.runtimeProfileReady ?? Promise.resolve(), save: () => button('保存').emit('click'), status: () => dialog.body.querySelector('[role="status"]').textContent };
}
const original = { id: 'original-id', name: 'Original', kind: 'sqlite', file: 'C:/isolated/qa.sqlite', port: 0, host: '', user: '', database: '', password: '', params: '', group: 'QA', read_only: false, keep_alive: false };

for (const [label, hidden, row, col, expected] of [
  ['hidden first column, first row', ['id'], 0, 0, 'Alice 中文'],
  ['hidden first column, second row', ['id'], 1, 0, 'Bob'],
  ['multiple hidden columns', ['id', 'name'], 1, 0, '20'],
  ['hidden middle column', ['name'], 0, 1, '10'],
  ['NULL after hidden columns', ['id', 'score'], 1, 1, null],
]) test(`grid: ${label} preserves clicked cell's actual value`, () => {
  const view = gridView(hidden); view.callbacks.onEditStart(row, col); assert.equal(view.grid.edit.value, expected);
  assert.equal(view.editingCol, view.grid.columns[col].name);
});
test('grid: opening hidden-column editor and confirming unchanged value creates no update', () => {
  const view = gridView(['id']); view.callbacks.onEditStart(0, 0); view.callbacks.onEditEnd(0, 0, view.grid.edit.value);
  assert.equal(view.pendingUpdates.size, 0); assert.equal(view.grid.rows[0][0], 'Alice 中文');
});
test('grid: existing draft and inserted-row draft are kept when columns are hidden', () => {
  const view = gridView(['id']); view.pendingUpdates.set(0, new Map([['name', 'Draft']]));
  view.pendingInserts.push([null, 'Inserted', '30', null]); view.pendingInsertEdits.set(0, new Map([['name', 'Inserted draft']])); view.applyData();
  view.callbacks.onEditStart(0, 0); assert.equal(view.grid.edit.value, 'Draft');
  view.callbacks.onEditStart(2, 0); assert.equal(view.grid.edit.value, 'Inserted draft');
});
test('grid: changed hidden-column cell updates name, never its primary key', () => {
  const view = gridView(['id']); view.callbacks.onEditStart(0, 0); view.callbacks.onEditEnd(0, 0, 'Renamed');
  assert.deepEqual([...view.pendingUpdates.get(0)], [['name', 'Renamed']]); assert.equal(view.tab.page.rows[0][0], '1');
});
test('query: result 2 shares the same source for grid, export, and record count', async () => {
  const view = queryView(); await view.select('结果 2');
  assert.equal(view.tab.activeResult, 1); assert.equal(view.activeResultTab, 1);
  assert.equal(view.currentSet(), view.tab.results.sets[1]); assert.deepEqual(plain(view.grid.options.rows), [['SECOND_A'], ['SECOND_B']]);
  // 「创建图表」已按需求移除：查询视图不再提供 chartSource
  assert.equal(view.chartSource, undefined); assert.match(view.statusInfo().pageText, /共 4 行/);
  assert.equal(view.statusInfo().pageText, '共 4 行');
  assert.equal(view.gridPreviewNoteEl.textContent, '无完整缓存，仅已加载 2 / 4 行；复制、筛选与导出只含已加载行，不会重新执行 SQL。');
  assert.equal(view.gridPreviewNoteEl.style.display, 'block');
  assert.deepEqual(view.notifications(), { statuses: 1, toolbars: 1 });
});
test('query: message tab has no export/chart source and result 1 restores it', async () => {
  const view = queryView(); view.cellPreviewEl.style.display = ''; view.cellPreviewEl.textContent = '上个结果的单元格';
  await view.select('消息'); assert.equal(view.tab.activeResult, -1);
  assert.equal(view.messagesEl.textContent, 'OK', '未运行时保留完成日志，不创建活计时消息');
  assert.equal(view.messagesEl.querySelector('.msg-running'), null);
  assert.equal(view.gridPreviewNoteEl.style.display, 'none'); assert.equal(view.gridPreviewNoteEl.textContent, '');
  assert.equal(view.cellPreviewEl.style.display, 'none'); assert.equal(view.cellPreviewEl.innerHTML, '');
  assert.equal(view.currentSet(), null); assert.equal(view.chartSource, undefined); assert.equal(view.statusInfo().pageText, '');
  assert.equal(view.gridPreviewNoteEl.style.display, 'none');
  await view.select('结果 1'); assert.equal(view.currentSet(), view.tab.results.sets[0]); assert.match(view.statusInfo().pageText, /共 1 行/);
});
test('query: external selected-result state and empty result sets do not fall back to result 1', () => {
  const view = queryView(); view.tab.activeResult = 1; assert.equal(view.activeResultTab, 1); view.renderResults();
  assert.equal(view.grid.options.rows.length, 2); view.tab.results.sets = []; view.tab.activeResult = -1; view.lastMessages = []; view.renderResults();
  assert.equal(view.currentSet(), null); assert.equal(view.grid.options.rows.length, 0);
});
test('connection: each copied connection has a nonempty independent stable ID', async () => {
  const received = [];
  for (let index = 0; index < 2; index++) {
    const form = connection({ ...original, id: '', name: `Copy ${index}` }, result => { received.push(result); });
    assert.match(form.dialog.el.querySelector('.dialog-title-text').textContent, /^新建连接/);
    await form.save(); assert.equal(form.dialog.closed, true);
  }
  const ids = received.map(result => result.config.id); assert(ids.every(id => id && id !== original.id)); assert.equal(new Set(ids).size, 2);
  assert(received.every(result => result.config.file === original.file && result.config.group === original.group));
});
test('connection: rejected async save keeps input, clears busy state, and retries with the same ID', async () => {
  const received = []; let attempts = 0;
  const form = connection({ ...original, id: '' }, async result => { received.push(result); if (++attempts === 1) throw new Error('backend rejected'); });
  form.input('连接名称').value = 'Retry copy'; await form.save();
  assert.equal(form.dialog.closed, false); assert.match(form.status(), /保存失败.*backend rejected/);
  assert.equal(form.input('连接名称').value, 'Retry copy'); assert.equal(form.button('保存').disabled, false); assert.equal(form.dialog.el.getAttribute('aria-busy'), null);
  await form.save(); assert.equal(form.dialog.closed, true); assert.equal(received.length, 2); assert(received[0].config.id); assert.equal(received[0].config.id, received[1].config.id);
});
test('connection: in-flight save is awaited and duplicate clicks cannot duplicate persistence', async () => {
  let resolve; let calls = 0; const pending = new Promise(done => { resolve = done; });
  const form = connection({ ...original, id: '' }, async () => { calls++; await pending; });
  const saving = form.save(); assert.equal(calls, 1); assert.equal(form.dialog.closed, false);
  assert.equal(form.dialog.el.getAttribute('aria-busy'), 'true'); assert.equal(form.button('保存').disabled, true); assert.equal(form.button('测试连接').disabled, true);
  await form.save(); assert.equal(calls, 1); resolve(); await saving; assert.equal(form.dialog.closed, true);
});
test('connection: editing preserves original ID and saved ciphertext without resubmitting plaintext', async () => {
  const loaded = []; let saved;
  const form = connection({ ...original, kind: 'mysql', host: 'localhost', port: 3306, user: 'root', password: 'encrypted-ciphertext' }, result => { saved = result; }, {
    load: async id => { loaded.push(id); return null; }, test: async () => assert.fail('保存不应测试数据库连接'),
  });
  await form.ready;
  assert.deepEqual(loaded, [original.id]);
  assert.equal(form.input('SSH 密码').value, '', '真实运行配置编辑器只显示空秘密输入框');
  assert.match(form.dialog.el.querySelector('.dialog-title-text').textContent, /^连接属性/); await form.save();
  assert.equal(saved.config.id, original.id); assert.equal(saved.config.password, 'encrypted-ciphertext'); assert.equal(saved.password, undefined);
});
test('connection: empty required name prevents persistence and leaves dialog open', async () => {
  let calls = 0; const form = connection({ ...original, id: '' }, () => { calls++; }); form.input('连接名称').value = ''; await form.save();
  assert.equal(calls, 0); assert.equal(form.dialog.closed, false); assert.match(form.status(), /请输入连接名称/);
});
