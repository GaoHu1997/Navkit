import test, { afterEach } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { webcrypto } from 'node:crypto';
import { build } from 'esbuild';
import ts from 'typescript';
import { fileURLToPath } from 'node:url';
import { transactionFixtureBindings } from './transaction-fixture-bindings.mjs';

const read = file => fs.readFileSync(new URL(`../../${file}`, import.meta.url), 'utf8');
async function module(file) {
  const result = await build({ entryPoints: [fileURLToPath(new URL(`../../${file}`, import.meta.url))], bundle: true, write: false, platform: 'node', format: 'esm', logLevel: 'silent' });
  return import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
}
const { objectKey, objectLabel: catalogObjectLabel } = await module('src/core/object-catalog.ts');
const helpers = { ...await module('src/core/savedQueries.ts'), ...await module('src/core/objectBrowser.ts'), ...await module('src/core/execution.ts'), ...await module('src/core/queryResultPages.ts'), ...await module('src/core/object-catalog-loader.ts'), ...await module('src/core/object-navigation.ts'), ...await module('src/core/routine-editor.ts'), ...await module('src/core/routine-create.ts'), objectKey, catalogObjectLabel };
const liveHarnesses = new Set();
function disposeHarness(h) {
  const errors = [];
  try {
    for (const view of new Set([...h.createdViews, ...h.app.views.values()])) {
      try { view.destroy?.(); } catch (error) { errors.push(error); }
    }
    for (const dialog of h.dialogs) {
      try { if (!dialog.closed) dialog.close(); } catch (error) { errors.push(error); }
    }
  } finally {
    for (const timer of h.timers) clearTimeout(timer);
    h.timers.clear(); h.document.clearEventListeners(); h.env.window.clearEventListeners();
    liveHarnesses.delete(h);
  }
  if (errors.length) throw new AggregateError(errors, 'fixture cleanup failed');
}
afterEach(() => {
  const errors = [];
  for (const h of [...liveHarnesses]) { try { disposeHarness(h); } catch (error) { errors.push(error); } }
  if (errors.length) throw new AggregateError(errors, 'object browser fixtures did not dispose');
});
const plain = value => JSON.parse(JSON.stringify(value));
const cfg = (patch = {}) => ({ id: 'own', name: '自己的连接', kind: 'mysql', params: '', read_only: false, ...patch });
const favorite = (id, schema, extra = {}) => ({ id, name: '同名查询', sql: "SELECT '原文';\r\n-- 原样", connection_id: 'own', schema, timestamp: 42, ...extra });
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
const flush = () => new Promise(resolve => setImmediate(resolve));
const sourceFingerprint = 'a'.repeat(64);
function descriptor(connectionId, schema, kind, name, identityArguments = '') {
  const object = { connection_id: connectionId, catalog: schema, schema, kind, name, identity_arguments: identityArguments, parent: null };
  return { object, object_key: objectKey(object), display_name: name, backend_object_id: `${schema}:${kind}:${name}:${identityArguments}`, owner: 'fixture-owner', enabled: null, definition_readable: true, actions: { read_definition: { level: 'supported', reason: 'fixture提供可见定义' }, call: { level: 'unsupported', reason: '本测试不调用数据库例程' }, drop: { level: 'unsupported', reason: '本测试没有受控删除计划' } }, properties: {} };
}
function catalogPage(scope, items = []) {
  return { objects: items, completeness: 'complete', sections: Object.fromEntries(scope.kinds.map(kind => [kind, { state: 'known', reason: null }])), revision: 'catalog-fixture-revision', next_cursor: null };
}
function snapshot(object, definition) {
  assert.equal(typeof object.identity_arguments, 'string', 'opening a routine requires the catalog signature, including the explicit zero-argument signature');
  return { object: { connection_id: object.connection_id, schema: object.schema, kind: object.kind, name: object.name, identity_arguments: object.identity_arguments }, definition, revision: 'opening-fixture-revision', source_fingerprint: sourceFingerprint, overloads: 1, completeness: 'complete', backend_object_id: `${object.kind}:${object.name}`, creation_context: {}, security_completeness: 'complete', replacement_reason: 'fixture安全回执' };
}

class EventTargetFixture {
  constructor() { this.listeners = new Map(); }
  addEventListener(type, callback) { if (!this.listeners.has(type)) this.listeners.set(type, new Set()); this.listeners.get(type).add(callback); }
  removeEventListener(type, callback) { this.listeners.get(type)?.delete(callback); }
  clearEventListeners() { this.listeners.clear(); }
  fire(type, patch = {}) {
    const event = { type, target: this, currentTarget: this, key: '', defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, stopPropagation() {}, ...patch };
    let result;
    for (const callback of [...(this.listeners.get(type) ?? [])]) result = callback(event);
    return result;
  }
}
class TextNodeFixture {
  constructor(value) { this.textContent = String(value); this.parentElement = null; }
}
class Element extends EventTargetFixture {
  constructor(tag = 'div') {
    super();
    this.tagName = tag; this.children = []; this.attributes = {}; this.style = {}; this.value = ''; this.className = ''; this.isConnected = true;
    this.dataset = new Proxy({}, { set: (target, name, value) => { target[name] = String(value); this.attributes[`data-${name.replace(/[A-Z]/g, ch => `-${ch.toLowerCase()}`)}`] = String(value); return true; } });
    this.classList = {
      add: (...names) => { this.className = [...new Set([...this.className.split(/\s+/).filter(Boolean), ...names])].join(' '); },
      remove: (...names) => { this.className = this.className.split(/\s+/).filter(name => !names.includes(name)).join(' '); },
      contains: name => this.className.split(/\s+/).includes(name),
    };
  }
  set innerHTML(value) { this.replaceChildren(); this.html = value; }
  get innerHTML() { return this.html ?? ''; }
  set textContent(value) { this.replaceChildren(); this.text = String(value ?? ''); }
  get textContent() { return (this.text ?? '') + this.children.map(child => child.textContent).join(''); }
  append(...children) { children.forEach(child => this.appendChild(typeof child === 'string' ? new TextNodeFixture(child) : child)); }
  appendChild(child) { child.parentElement?.removeChild(child); child.parentElement = this; child.ownerDocument ??= this.ownerDocument; this.children.push(child); return child; }
  removeChild(child) { this.children = this.children.filter(item => item !== child); child.parentElement = null; return child; }
  replaceChildren(...children) { for (const child of this.children) child.parentElement = null; this.children = []; this.text = ''; this.append(...children); }
  setAttribute(key, value) { this.attributes[key] = String(value); }
  getAttribute(key) { return this.attributes[key] ?? null; }
  removeAttribute(key) { delete this.attributes[key]; }
  querySelectorAll(selector) {
    return this.children.flatMap(child => child instanceof Element ? [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)] : []);
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  focus() { this.focused = true; if (this.ownerDocument) this.ownerDocument.activeElement = this; }
  matches(selector) {
    return selector.split(',').some(part => {
      const item = part.trim(), tag = item.match(/^\w+/)?.[0], id = item.match(/#([\w-]+)/)?.[1], className = item.match(/\.([\w-]+)/)?.[1];
      if (tag && tag !== this.tagName || id && id !== this.id || className && !this.classList.contains(className)) return false;
      if (item.includes(':not([disabled])') && this.disabled || item.includes(':not([hidden])') && this.hidden) return false;
      return [...item.replace(/:not\(\[[^\]]+\]\)/g, '').matchAll(/\[([\w-]+)(?:="([^"]*)")?\]/g)].every(([, key, value]) => key in this.attributes && (value === undefined || this.attributes[key] === value));
    });
  }
  contains(node) { return node === this || this.children.some(child => child === node || child instanceof Element && child.contains(node)); }
  remove() { this.parentElement?.removeChild(this); this.isConnected = false; }
  getBoundingClientRect() { return { left: 10, top: 20 }; }
}
const source = ts.createSourceFile('App.ts', read('src/core/App.ts'), ts.ScriptTarget.ES2021, true);
const classText = name => source.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === name).getText(source);
function harness(apiPatch = {}) {
  const dialogs = [], calls = [], notices = [], infoStates = [], createdViews = new Set(), timers = new Set();
  const trackView = view => { createdViews.add(view); return view; };
  class Dialog {
    constructor(opts) { this.options = opts; this.el = new Element(); this.body = new Element(); this.footer = new Element(); this.el.append(this.body, this.footer); this.closed = false; this.callbacks = []; opts.render(this.body, this); opts.buttons.forEach(b => { const button = new Element('button'); button.textContent = b.label; this.footer.append(button); }); dialogs.push(this); }
    onClose(cb) { this.callbacks.push(cb); }
    close() { this.closed = true; this.callbacks.forEach(cb => cb()); }
    async submit(value) { const input = this.body.querySelector('input'); if (input && value !== undefined) input.value = value; const result = await this.options.buttons[0].onClick(this); if (result !== false) this.close(); return result; }
  }
  class DataGrid {
    constructor(callbacks) { this.cb = callbacks; this.el = new Element(); this.rows = []; }
    setData(opts) { Object.assign(this, opts); }
    setState(opts) { Object.assign(this, opts); }
    setColWidths(widths) { this.widths = widths; }
    dispose() { this.disposed = true; }
  }
  class SqlEditor {
    constructor(opts) { this.opts = opts; this.sql = opts.value ?? ''; this.el = new Element(); }
    getSql() { return this.sql; }
    setSql(sql) { this.sql = sql; this.opts.onChange?.(sql); }
    getSelection() { return ''; }
    setCompletionData() {}
    focus() {}
    destroy() {}
  }
  const document = new EventTargetFixture();
  document.activeElement = null;
  document.createElement = tag => { const element = new Element(tag); element.ownerDocument = document; return element; };
  document.createTextNode = value => new TextNodeFixture(value);
  document.body = document.createElement('body'); document.querySelector = selector => document.body.querySelector(selector);
  const window = new EventTargetFixture();
  const schedule = (callback, delay, ...args) => { const timer = setTimeout(() => { timers.delete(timer); callback(...args); }, delay); timers.add(timer); return timer; };
  const unschedule = timer => { timers.delete(timer); clearTimeout(timer); };
  const api = new Proxy(apiPatch, { get(target, name) {
    if (target[name]) return target[name];
    if (name === 'listObjectCatalog') return async (scope, cursor) => { calls.push([name, plain(scope), cursor]); return catalogPage(scope); };
    if (name === 'readRoutineDesignObject') return async () => { throw new Error('fixture必须显式提供真实完整例程基线'); };
    if (name === 'objectDefinition') return async () => { throw new Error('fixture必须显式提供完整对象定义回执'); };
    if (name === 'getRoutineDefinition' || name === 'listRoutines') return async () => { throw new Error('旧裸名称目录不能代替精确对象身份'); };
    return async (...args) => { calls.push([name, ...args]); if (name === 'listDatabases') return [['one', 'database'], ['two', 'database']]; if (name === 'listObjects') return []; if (name === 'executeSql') throw new Error('打开 SQL 不得执行'); return {}; };
  } });
  const env = vm.createContext({ exports: {}, ...helpers, ...transactionFixtureBindings, Dialog, DataGrid, SqlEditor, document, api, crypto: webcrypto, structuredClone, console, setTimeout: schedule, clearTimeout: unschedule, window,
    DataTabView: class {}, StructureTabView: class {}, BackupTabView: class {}, UsersTabView: class {}, ModelTabView: class {}, ChartTabView: class {},
    iconSvgByName: () => '', svg: () => '', ICON_PAGE_FIRST: '', ICON_PAGE_PREV: '', ICON_PAGE_NEXT: '', ICON_PAGE_LAST: '', GRID_CONTEXT_MENU: [], renderCellPreview() {},
  });
  const load = text => vm.runInContext(ts.transpile(text, { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS }), env);
  load(read('src/dialogs/ObjectActionDialog.ts').replace(/^import .*;\r?$/gm, '') + '\nglobalThis.openObjectAction = exports.openObjectAction;');
  load(read('src/ui/RoutineDocumentView.ts').replace(/^import .*;\r?$/gm, '') + '\nglobalThis.RoutineDocumentView = exports.RoutineDocumentView;');
  load([classText('TabView'), classText('ObjectsTabView'), classText('QueryTabView'), classText('SqlDesignTabView'), classText('RoutineTabView'), classText('App'), 'globalThis.classes = { App, ObjectsTabView, QueryTabView, SqlDesignTabView, RoutineTabView };'].join('\n'));
  const app = Object.create(env.classes.App.prototype);
  Object.assign(app, { tabs: [], views: new Map(), favorites: [], activeTabId: null, connections: [cfg(), cfg({ id: 'tree-other', read_only: false })], settings: { rows_per_page: 100 },
    openDbs: new Set(), openConnections: new Set(), keepAliveTimers: new Map(), infoRequest: 0, infoPanel: { setState: state => infoStates.push(plain(state)) }, drafts: { busy: false }, draftPreparing: false,
    tree: { selectedNodeId: 'conn:tree-other/db/unrelated/table/wrong', find: () => null, setNodes() {}, render() {} }, treeNodes: [],
    manualTransactionFor: () => null, manualTransactionIsBusy: () => false, syncConnIcons() {}, renderTabs() {}, renderStatus() {}, notifyToolbar() {},
    updateInfoPanelForNode() {}, syncDbIcons() {}, showObjectDetails() {}, toast: (...args) => notices.push(args),
    tabToolbar: { setItems() {} }, ribbon: { updateDisabled() {} }, async ensureConnection() {},
    menu: { renderBar() {} }, contextMenus: () => [], viewMenu: () => [],
  });
  const tab = { id: 'objects', kind: 'objects', title: '对象', connectionId: 'own', connectionName: '自己的连接', schema: 'one', objectType: 'table', selectedRows: new Set(), dirty: false };
  app.tabs.push(tab); app.activeTabId = tab.id;
  const view = trackView(new env.classes.ObjectsTabView(app, tab)); app.views.set(tab.id, view);
  function query(fav = null) {
    const q = { ...tab, id: `q${app.tabs.length}`, kind: 'query', object: fav?.name ?? '', title: fav?.name ?? '查询', sql: fav?.sql ?? 'SELECT 1', savedQueryId: fav?.id, selectedRows: new Set(), results: null, activeResult: 0 };
    app.tabs.push(q); app.activeTabId = q.id;
    const qv = trackView(new env.classes.QueryTabView(app, q)); qv.renderResults = () => {}; app.views.set(q.id, qv); return { q, qv };
  }
  const result = { app, tab, view, query, env, api, dialogs, calls, notices, infoStates, document, createdViews, timers, trackView }; liveHarnesses.add(result); return result;
}

test('查询对象按连接和数据库隔离，缺少保存位置的旧记录不进入任何数据库', () => {
  const items = [favorite('one', 'one'), favorite('two', 'two'), favorite('legacy', undefined), favorite('global', null, { connection_id: null }), favorite('foreign', null, { connection_id: 'elsewhere' }), favorite('blank', ' '), favorite('no-connection', 'one', { connection_id: '' })];
  assert.deepEqual(helpers.savedQueriesFor(items, 'own', 'one').map(f => f.id), ['one']);
  assert.deepEqual(helpers.savedQueriesFor(items, 'own', 'two').map(f => f.id), ['two']);
  assert.deepEqual(helpers.savedQueriesFor(items, 'own', ''), []);
  assert.deepEqual(helpers.savedQueriesFor(items, '', 'one'), []);
  assert.equal(items.length, 7); assert.equal(items[2].schema, undefined);
});
test('稳定 ID 更新不按名称覆盖，同名跨库允许，另存为生成新 ID', () => {
  const a = favorite('a', 'one'), b = favorite('b', 'two');
  const next = helpers.queryFavorite([a, b], { id: 'a', name: a.name, sql: 'SELECT 2', connectionId: 'own', schema: 'one' });
  assert.equal(next.id, 'a'); assert.equal(next.sql, 'SELECT 2'); assert.equal(b.sql, favorite('b', 'two').sql);
  assert.throws(() => helpers.queryFavorite([a, b], { name: a.name, sql: 'x', connectionId: 'own', schema: 'one' }), /同名/);
  const copy = helpers.queryFavorite([a, b], { name: '副本', sql: 'x', connectionId: 'own', schema: 'one' }); assert.notEqual(copy.id, a.id);
  assert.throws(() => helpers.queryFavorite([a], { id: 'a', name: a.name, sql: 'x', connectionId: 'own', schema: 'two' }), /归属/);
});
test('没有连接或数据库不能生成已保存查询', () => {
  for (const connectionId of ['', ' ', 'own']) for (const schema of ['', ' ', 'one']) {
    if (connectionId === 'own' && schema === 'one') continue;
    assert.throws(() => helpers.queryFavorite([], { name: '查询', sql: 'SELECT 1', connectionId, schema }), /请选择连接和数据库/);
  }
});
test('旧记录只恢复为未保存 SQL，明确保存到数据库时不猜库或覆盖原文', () => {
  const old = favorite('legacy', undefined);
  assert.throws(() => helpers.queryFavorite([old], { id: old.id, name: old.name, sql: old.sql, connectionId: 'own', schema: 'one' }), /不是已保存查询/);
  const next = helpers.queryFavorite([old], { name: old.name, sql: old.sql, connectionId: 'own', schema: 'one' });
  assert.equal(next.schema, 'one'); assert.notEqual(next.id, old.id); assert.equal(next.sql, old.sql);
  assert.equal(old.schema, undefined); assert.equal(old.id, 'legacy');
});
test('对象数据大小按1024进位，缺失统计与视图仍保持未知', () => {
  for (const category of ['table', 'folder']) {
    const values = [512, 1024, 1024 ** 2, 1024 ** 3, 1024 ** 4];
    const objects = values.map((dataLength, index) => ({ name: String(index), type: 'table', dataLength }));
    const grid = helpers.objectGrid(category, objects);
    assert.deepEqual(grid.rows.map(row => row[2]), ['512 B', '1.00 KB', '1.00 MB', '1.00 GB', '1.00 TB']);
    assert.deepEqual(objects.map(object => object.dataLength), values);
    const unknown = helpers.objectGrid(category, [{ name: 'missing', type: 'table' }, { name: 'zero', type: 'table', dataLength: 0 }, { name: 'view', type: 'view', dataLength: 2048 }, { name: 'invalid', type: 'table', dataLength: Infinity }]);
    assert.deepEqual(unknown.rows.map(row => row[2]), ['—', '—', '—', '—']);
  }
});

test('分类列独立，视图/函数/查询不伪造表指标', () => {
  for (const [type, names] of [['view', ['名称', '修改时间', '注释']], ['function', ['名称', '类型', '模式', '身份参数 / 父对象', '状态']], ['query', ['名称', '修改时间', 'SQL 摘要']]]) {
    const data = helpers.objectGrid(type, [{ name: 'x', type, favorite: favorite('q', 'one') }]);
    assert.deepEqual(data.columns.map(c => c.name), names); assert.equal(data.rows[0].length, names.length);
    assert(!data.columns.some(c => /引擎|递增|行数/.test(c.name)));
  }
  assert.equal(helpers.objectGrid('function', [{ name: 'p', type: 'procedure' }]).rows[0][1], '存储过程');
  assert(helpers.objectGrid('table', [{ name: 't', type: 'table', autoIncrement: 42 }], true).columns.some(c => c.name === '自增值'));
  assert(!helpers.objectGrid('view', [{ name: 'v', type: 'view', autoIncrement: 42 }], true).columns.some(c => c.name === '自增值'));
});
test('对象页所有工具栏操作使用统一普通样式，不突出新建', () => {
  for (const category of ['folder', 'table', 'view', 'function', 'query']) {
    const type = category === 'folder' ? 'table' : category;
    for (const read_only of [false, true]) {
      for (const selected of [null, { name: '测试对象', type, favorite: favorite('q', 'one') }]) {
        const tools = helpers.objectTools(category, selected, cfg({ read_only }));
        assert(tools.every(item => !item.primary), `${category} 的操作应全部使用普通按钮样式`);
        assert(!tools.some(item => item.id === 'object-assign'), '对象页不应提供未归属查询的归属操作');
      }
    }
  }
});
test('未选择禁用行操作，工具栏按类别控制向导与只读能力', () => {
  for (const type of ['table', 'view', 'function', 'query']) {
    const tools = helpers.objectTools(type, null, cfg());
    assert(tools.filter(t => ['object-open', 'object-design', 'object-rename', 'object-delete'].includes(t.id)).every(t => t.disabled));
    if (type !== 'table') assert(!tools.some(t => t.id === 'object-import'));
    if (['function', 'query'].includes(type)) assert(!tools.some(t => t.id === 'object-export'));
  }
  const selected = { name: 'p', type: 'procedure' };
  assert.equal(helpers.objectTools('function', selected, cfg()).find(t => t.id === 'object-delete').label, '删除存储过程');
  assert(helpers.objectTools('view', { name: 'v', type: 'view' }, cfg({ read_only: true })).find(t => t.id === 'object-delete').disabled);
  assert(!helpers.objectTools('query', { name: 'q', type: 'query', favorite: favorite('q', 'one') }, cfg({ read_only: true })).find(t => t.id === 'object-delete').disabled);
});
test('方言模板确实创建指定对象，标识符转义且不预先删除', () => {
  for (const kind of ['sqlite', 'mysql', 'postgresql']) {
    const types = kind === 'sqlite' ? ['table', 'view'] : ['table', 'view', 'function', 'procedure'];
    for (const type of types) {
      const sql = helpers.newObjectSql(cfg({ kind }), 'db', '名`"称', type);
      assert(sql.startsWith(`CREATE ${type.toUpperCase()}`)); assert(!/DROP|ALTER/.test(sql));
    }
  }
  assert.equal(helpers.dropObjectSql(cfg(), 'd`b', { name: 'v`x', type: 'view' }), 'DROP VIEW `d``b`.`v``x`');
  assert.equal(helpers.dropObjectSql(cfg({ kind: 'sqlite' }), 'main', { name: 'v"x', type: 'view' }), 'DROP VIEW "main"."v""x"');
  assert.match(helpers.objectCapability(cfg({ kind: 'postgresql' }), 'function', 'delete'), /签名/);
  assert.match(helpers.objectCapability(cfg({ kind: 'redis' }), 'view', 'create'), /协议/);
  assert.match(helpers.objectCapability(cfg(), 'function', 'update'), /保存已禁用/);
});
test('四类树入口和顶部查询复用每连接一个对象页，新建查询才创建编辑器', async () => {
  const h = harness(); h.app.currentDbTarget = () => ({ connId: 'own', schema: 'one' }); h.app.selectDbTreeNode = () => {};
  for (const type of ['table', 'view', 'function', 'query']) { await h.app.selectNode({ id: `conn:own/db/one/grp-${type}` }); assert.equal(h.tab.objectType, type); }
  await h.app.runCommand('open-query'); assert.equal(h.app.tabs.length, 1); assert.equal(h.tab.title, '对象');
  await h.app.runCommand('new-query'); assert.equal(h.app.tabs.length, 2); assert.equal(h.app.activeTab.kind, 'query'); assert.equal(h.app.activeTab.sql, '');
  assert(!h.calls.some(c => c[0] === 'executeSql'));
});
test('查询列表只读本库收藏，双击恢复名称/SQL/归属且绝不执行', async () => {
  const h = harness(); h.app.favorites = [favorite('a', 'one'), favorite('b', 'two')]; h.tab.objectType = 'query'; await h.view.reload();
  assert.equal(h.view.grid.rows.length, 1); h.view.grid.cb.onRowDoubleClick(0); await flush();
  const q = h.app.activeTab; assert.equal(q.savedQueryId, 'a'); assert.equal(q.title, '同名查询'); assert.equal(q.schema, 'one'); assert.equal(q.sql, h.app.favorites[0].sql);
  assert.equal(h.calls.length, 0);
  await h.app.openSavedQuery('a'); assert.equal(h.app.tabs.length, 2);
});
test('本库没有已保存 SQL 时只显示空列表，不提供未归属查询入口', async () => {
  const h = harness(); h.app.favorites = [favorite('legacy', null), favorite('global', null, { connection_id: null })]; h.tab.objectType = 'query'; await h.view.reload();
  // 顶部信息条/空态文案已按需求移除：仅网格表头，无行。
  assert.equal(h.view.grid.rows.length, 0);
  assert.equal('legacy' in h.view, false); assert.equal('unassigned' in h.view, false);
  assert.equal(h.app.favorites.length, 2);
});
test('异步旧连接/库/类别结果和旧选择不能覆盖新对象', async () => {
  for (const change of [tab => { tab.schema = 'two'; }, tab => { tab.connectionId = 'tree-other'; }, tab => { tab.objectType = 'function'; }]) {
    const old = deferred(); let count = 0;
    const h = harness({ listObjects: () => ++count === 1 ? old.promise : Promise.resolve([{ name: 'current', type: 'table' }]), listObjectCatalog: async scope => catalogPage(scope, [descriptor(scope.connection_id, scope.schema, 'procedure', 'current')]) });
    const pending = h.view.reload(); h.tab.selectedRows.add(0); change(h.tab); await h.view.reload();
    old.resolve([{ name: 'stale', type: 'table' }]); await pending;
    assert.equal(h.view.grid.rows[0][0], 'current'); assert.equal(h.tab.selectedRows.size, 0); assert.equal(h.view.selectedObject(), null);
  }
});
test('加载失败、空列表、搜索无结果可区分且过滤后行映射正确', async () => {
  let fail = true; const h = harness({ listObjects: async () => { if (fail) throw new Error('offline'); return [{ name: 'a', type: 'table' }, { name: 'b', type: 'table' }]; } });
  await h.view.reload(); assert.match(h.view.metadataNotice, /offline/); assert.equal(h.view.grid.rows.length, 0);
  fail = false; await h.view.reload(); h.view.setSearchValue('b'); h.view.grid.cb.onSelectionChange([0]); assert.equal(h.view.selectedObject().name, 'b');
  h.view.setSearchValue('missing'); assert.equal(h.view.selectedObject(), null);
});
test('数据库写入使用对象页连接和库；只读、事务、失效守卫覆盖新命令', async () => {
  const h = harness({ listObjects: async () => [{ name: 'right', type: 'table' }] }); await h.view.reload(); h.view.grid.cb.onSelectionChange([0]);
  h.app.connections[0].read_only = true; assert.match(h.app.commandUnavailableReason('object-delete'), /只读/);
  h.app.connections[0].read_only = false; h.app.manualTransactionFor = id => id === 'own' ? { active: true } : null;
  assert.match(h.app.commandUnavailableReason('object-delete'), /事务/); assert(h.app.guardedTabToolbarItems(h.tab).find(t => t.id === 'object-new').disabled);
  h.app.manualTransactionFor = () => null; h.tab.datasourceStale = '端点已失效'; assert.equal(h.app.commandUnavailableReason('object-delete'), '端点已失效');
  delete h.tab.datasourceStale; let opened; h.app.openStructureTab = async (...args) => { opened = args; }; await h.app.runCommand('object-design'); assert.deepEqual(opened, ['own', 'one', 'right']);
});
test('只读和失效连接仍允许本地查询重命名/删除，离页进行中时拦截', async () => {
  const h = harness(); h.tab.objectType = 'query'; h.app.favorites = [favorite('q', 'one')]; await h.view.reload(); h.view.grid.cb.onSelectionChange([0]);
  h.app.connections[0].read_only = true; h.tab.datasourceStale = '失效';
  assert.equal(h.app.commandUnavailableReason('object-delete'), null); assert.equal(h.app.commandUnavailableReason('object-rename'), null);
  h.app.draftPreparing = true; assert.match(h.app.commandUnavailableReason('object-delete'), /离页/);
});
test('保存未选择数据库的 SQL 被拒绝，文本仍为未保存且不创建对象', () => {
  for (const [connectionId, schema] of [['', 'one'], ['own', ''], ['own', ' '], ['missing', 'one']]) {
    const h = harness(); const { q, qv } = h.query(); q.connectionId = connectionId; q.schema = schema; qv.editor.setSql('SELECT unsaved');
    h.app.cmdSaveQuery(); assert.equal(h.dialogs.length, 0); assert.equal(q.savedQueryId, undefined); assert(q.dirty);
    assert.equal(qv.editor.getSql(), 'SELECT unsaved'); assert.equal(h.app.favorites.length, 0);
    assert(h.notices.some(([, message]) => /请选择连接和数据库/.test(message)));
  }
});
test('保存对话框打开后连接被删除，提交失败保留未保存 SQL', async () => {
  const h = harness(); const { q, qv } = h.query(); qv.editor.setSql('SELECT unsaved'); h.app.cmdSaveQuery(); h.app.connections = [];
  await h.dialogs[0].submit('查询'); assert.equal(h.dialogs[0].closed, false); assert(q.dirty); assert.equal(q.savedQueryId, undefined); assert.equal(h.app.favorites.length, 0);
  assert.match(h.dialogs[0].body.children.at(-1).textContent, /连接已不存在/);
});
test('保存落盘失败保留 dirty、内容和对话框，重试成功才发布 ID 和名称', async () => {
  let fail = true, stored = [];
  const h = harness({ addFavorite: async f => { if (fail) throw new Error('disk full'); stored = [f]; return stored; } }); const { q, qv } = h.query(); qv.editor.setSql('SELECT 2'); h.app.cmdSaveQuery();
  const dialog = h.dialogs[0]; await dialog.submit('新查询'); assert.equal(dialog.closed, false); assert.equal(q.dirty, true); assert.equal(q.savedQueryId, undefined); assert.equal(h.app.favorites.length, 0); assert.equal(qv.editor.getSql(), 'SELECT 2');
  assert.match(dialog.body.children.at(-1).textContent, /disk full.*重试/); assert.equal(dialog.body.querySelector('input').attributes['aria-invalid'], 'true');
  fail = false; await dialog.submit(); assert(dialog.closed); assert.equal(q.savedQueryId, stored[0].id); assert.equal(q.title, '新查询'); assert.equal(q.dirty, false); assert.equal(stored[0].schema, 'one');
});
test('重复保存稳定 ID；另存为才创建新 ID；写入期间继续编辑仍 dirty', async () => {
  let items = [favorite('saved', 'one')], pending;
  const h = harness({ addFavorite: async f => { if (pending) await pending.promise; items = [f, ...items.filter(x => x.id !== f.id)]; return items; } }); h.app.favorites = items; const { q, qv } = h.query(items[0]);
  qv.editor.setSql('SELECT 2'); h.app.cmdSaveQuery(); await h.dialogs.at(-1).submit('原查询'); assert.equal(q.savedQueryId, 'saved'); assert.equal(items.length, 1);
  pending = deferred(); qv.editor.setSql('SELECT 3'); h.app.cmdSaveQuery(); const saving = h.dialogs.at(-1).submit('原查询'); qv.editor.setSql('SELECT 4'); pending.resolve(); await saving;
  assert.equal(items[0].sql, 'SELECT 3'); assert.equal(q.dirty, true);
  pending = null; h.app.cmdSaveQuery(true); await h.dialogs.at(-1).submit('副本'); assert.notEqual(q.savedQueryId, 'saved'); assert.equal(items.length, 2);
});
test('重命名失败不改变列表或 SQL，成功同步页签；删除解除 ID 保留文本并标 dirty', async () => {
  let fail = true;
  const h = harness({ addFavorite: async f => { if (fail) throw new Error('write denied'); return [f]; }, removeFavorite: async () => [] }); const old = favorite('q', 'one'); h.app.favorites = [old]; const { q, qv } = h.query(old);
  h.app.renameSavedQuery('q'); const dialog = h.dialogs.at(-1); await dialog.submit('新名字'); assert.equal(q.title, old.name); assert.equal(dialog.closed, false);
  fail = false; await dialog.submit('新名字'); assert.equal(q.title, '新名字'); assert.equal(h.app.favorites[0].sql, old.sql);
  h.app.deleteSavedQuery('q'); assert.equal(q.savedQueryId, 'q'); await h.dialogs.at(-1).submit(); assert.equal(q.savedQueryId, undefined); assert.equal(qv.editor.getSql(), old.sql); assert.equal(q.dirty, true);
});
test('旧 SQL 打开为未保存；选库保存失败可重试，只有成功后才成为库内对象', async () => {
  let fail = true; const old = favorite('old', null, { connection_id: null }); let stored = [old];
  const h = harness({ addFavorite: async f => { if (fail) throw new Error('io failed'); stored = [f, ...stored.filter(item => item.id !== f.id)]; return stored; } }); h.app.favorites = stored; h.tab.objectType = 'query';
  await h.app.openSavedQuery('old'); const q = h.app.activeTab;
  assert.equal(q.savedQueryId, undefined); assert.equal(q.schema, ''); assert.equal(q.connectionId, ''); assert(q.dirty); assert.equal(q.sql, old.sql); assert.match(q.title, /未保存/);
  assert.equal(stored.length, 1); assert.equal(old.schema, null); assert.equal(h.calls.length, 0);
  const qv = h.trackView(new h.env.classes.QueryTabView(h.app, q)); qv.renderResults = () => {}; qv.detachSavedQuery(); h.app.views.set(q.id, qv);
  q.connectionId = 'own'; q.connectionName = '自己的连接'; q.schema = 'one'; h.app.cmdSaveQuery(); const dialog = h.dialogs.at(-1);
  await dialog.submit('恢复查询'); assert.equal(q.savedQueryId, undefined); assert.equal(stored.length, 1); assert(q.dirty); assert(!dialog.closed);
  fail = false; await dialog.submit(); assert(dialog.closed); assert(q.savedQueryId); assert.notEqual(q.savedQueryId, old.id); assert.equal(q.dirty, false);
  assert.equal(h.app.savedQueries('own', 'one').length, 1); assert.equal(stored.length, 2); assert.equal(old.schema, null);
  assert.equal(h.app.savedQueries('own', 'one')[0].sql, old.sql);
});
test('视图新建进入真实创建模式，首次保存只执行 CREATE 并带正确数据库', async () => {
  const ddl = []; const h = harness({ executeDdl: async (...args) => { ddl.push(args); return 1; } }); h.tab.objectType = 'view';
  await h.app.runCommand('object-new'); await h.dialogs.at(-1).submit('new_view'); const design = h.app.activeTab;
  assert.equal(design.kind, 'viewdesign'); assert.equal(design.designCreating, true); assert.equal(ddl.length, 0);
  assert.equal(h.app.tabToolbarItems(design).find(item => item.id === 'refresh').disabled, true);
  const view = h.trackView(new h.env.classes.SqlDesignTabView(h.app, design, 'view')); h.app.views.set(design.id, view); assert.equal(view.draftDirty(), true);
  await h.app.cmdSaveViewDesign(); assert.equal(ddl.length, 1); assert.equal(ddl[0][0], 'own'); assert.equal(ddl[0][2], 'one'); assert(ddl[0][1].startsWith('CREATE VIEW `one`.`new_view`')); assert(!ddl[0][1].includes('DROP')); assert.equal(design.designCreating, false);
});
test('函数与过程新建使用不存在基线和C1，预览失败保留草稿并允许显式重试', async () => {
  for (const [id, type] of [['object-new', 'function'], ['object-new-procedure', 'procedure']]) {
    let fail = true; const requests = [], executions = [], proofs = [];
    const h = harness({
      executeDdl: async () => { assert.fail('例程创建不能调用旧裸DDL入口'); },
      readRoutineCreationBaseline: async (object, ownerId) => { proofs.push([plain(object), ownerId]); return { baseline_id: webcrypto.randomUUID(), object: plain(object), source_fingerprint: sourceFingerprint, base_revision: 'opening-absence', completeness: 'complete', absent: true, expires_in_ms: 120000, strategy: 'create_implicit_commit', warnings: [] }; },
      previewRoutineCreation: async draft => { requests.push(plain(draft)); if (fail) throw new Error('database offline'); return { ...plain(draft), steps: [{ label: 'create', sql: draft.definition }], request_sql: `ROUTINE_CREATION ${'b'.repeat(64)}`, strategy: 'create_implicit_commit', executable: true, expires_in_ms: 100000, warnings: [] }; },
      issueGrant: async request => ({ ...request, execution_id: webcrypto.randomUUID(), execution_grant: 'fixture-grant' }),
      executeRoutineCreation: async (draft, request) => { executions.push([plain(draft), plain(request)]); return { outcome: { execution_id: request.execution_id, state: 'succeeded', effects: 'completed', transaction_state: 'committed', resources_pending: false, result: null, error: null, deadline: {} }, receipt: { baseline_id: draft.baseline_id, object: plain(draft.object), source_fingerprint: sourceFingerprint, status: 'applied', verified: true, transaction: 'implicit_commit_acknowledged', steps: [{ label: 'create', sql: draft.definition, status: 'applied', error: null }], snapshot: snapshot(draft.object, draft.definition), warnings: [] } }; },
    }); h.tab.objectType = 'function';
    await h.app.runCommand(id); await h.dialogs.at(-1).submit('routine_name'); const design = h.app.activeTab; assert.equal(design.objectType, type); assert(design.sql.startsWith(`CREATE ${type.toUpperCase()}`)); assert.equal(proofs.length, 1); assert.equal(proofs[0][1], design.id);
    const view = h.trackView(new h.env.classes.RoutineTabView(h.app, design)); h.app.views.set(design.id, view); assert(view.draftDirty()); await h.app.cmdSaveRoutineDesign(); assert.equal(design.designCreating, true); assert(view.draftDirty()); assert.match(view.document.error.textContent, /database offline/); assert.equal(executions.length, 0);
    fail = false; await h.app.cmdSaveRoutineDesign(); assert.equal(design.designCreating, false); assert.equal(view.draftDirty(), false); assert.equal(executions.length, 1); assert.equal(executions[0][1].owner_id, design.id); assert.equal(executions[0][1].execution_grant, 'fixture-grant'); assert.match(executions[0][1].sql, /^ROUTINE_CREATION /); assert.equal(requests[0].baseline_id, requests[1].baseline_id); assert.equal(proofs.length, 1);
  }
});
test('数据库对象删除等待确认和草稿守卫，使用所选类型及引用，失败保留对话框', async () => {
  const ddl = []; let allow = false;
  const h = harness({ listObjects: async () => [{ name: 'v`name', type: 'view' }], executeDdl: async (...args) => { ddl.push(args); } }); h.tab.objectType = 'view'; await h.view.reload(); h.view.grid.cb.onSelectionChange([0]);
  h.app.guardLeave = async (_ids, _cause, action) => { if (!allow) return false; await action(); return true; };
  await h.app.runCommand('object-delete'); assert.equal(ddl.length, 0); const dialog = h.dialogs.at(-1); await dialog.submit(); assert(!dialog.closed); assert.equal(ddl.length, 0);
  allow = true; await dialog.submit(); assert.deepEqual(ddl[0], ['own', 'DROP VIEW `one`.`v``name`', 'one']); assert(dialog.closed);
});
test('切换查询连接和数据库仍先走 guardLeave，取消保留 saved ID 与 dirty', async () => {
  const h = harness(); const f = favorite('q', 'one'); h.app.favorites = [f]; const { q, qv } = h.query(f); qv.editor.setSql('SELECT changed');
  let allow = false, guards = 0; h.app.guardLeave = async (_ids, _cause, action) => { guards++; if (!allow) return false; await action(); return true; };
  await qv.switchConnection('tree-other'); assert.equal(q.connectionId, 'own'); assert.equal(q.savedQueryId, 'q'); assert(q.dirty);
  qv.dbSelect.value = 'two'; qv.dbSelect.fire('change'); await flush(); assert.equal(q.schema, 'one'); assert.equal(q.savedQueryId, 'q');
  allow = true; qv.dbSelect.value = 'two'; qv.dbSelect.fire('change'); await flush(); assert.equal(q.schema, 'two'); assert.equal(q.savedQueryId, undefined); assert(q.dirty); assert.equal(guards, 3);
});
test('首次为未保存 SQL 选择连接和数据库保留文本，不要求先保存或丢弃', async () => {
  const h = harness(); const { q, qv } = h.query(); q.connectionId = ''; q.schema = ''; qv.editor.setSql('SELECT unbound');
  const guarded = []; h.app.guardLeave = async (ids, _cause, action) => { guarded.push([...ids]); await action(); return true; };
  await qv.switchConnection('own'); assert.equal(q.connectionId, 'own'); assert.equal(q.schema, ''); assert.equal(qv.editor.getSql(), 'SELECT unbound');
  qv.dbSelect.value = 'two'; qv.dbSelect.fire('change'); await flush();
  assert.equal(q.schema, 'two'); assert.equal(q.savedQueryId, undefined); assert(q.dirty); assert.equal(qv.editor.getSql(), 'SELECT unbound');
  assert.deepEqual(guarded, [[], []]); assert(!h.calls.some(([name]) => name.startsWith('execute')));
});
test('首次绑定未保存 SQL 仍受离页进行状态保护', async () => {
  const h = harness(); const { q, qv } = h.query(); q.schema = ''; qv.editor.setSql('SELECT unbound'); h.app.draftPreparing = true;
  await qv.switchConnection('tree-other'); assert.equal(q.connectionId, 'own');
  qv.dbSelect.value = 'two'; qv.dbSelect.fire('change'); await flush(); assert.equal(q.schema, ''); assert.equal(qv.editor.getSql(), 'SELECT unbound');
});
test('未保存 SQL 缺少数据库时元数据刷新不能代选其它数据库', async () => {
  const h = harness(); const { q, qv } = h.query(); q.schema = '';
  await qv.refreshDbSelect(); assert.equal(q.schema, ''); assert.equal(qv.dbSelect.value, '');
  assert(qv.dbSelect.children.some(option => option.textContent === '请选择数据库'));
});
test('保存查询的数据库元数据缺失或失败不会改写已保存归属', async () => {
  const h = harness({ listDatabases: async () => [['different', 'database']] }); const { q, qv } = h.query(favorite('q', 'one')); await qv.refreshDbSelect(); assert.equal(q.schema, 'one');
  h.api.listDatabases = async () => { throw new Error('offline'); }; await assert.rejects(qv.refreshDbSelect(), /offline/); assert.equal(q.schema, 'one');
});
test('对象菜单与双击共用动作，搜索后所选行不会借用无关树', async () => {
  const h = harness({ listObjects: async () => [{ name: 'a', type: 'table' }, { name: 'b', type: 'view' }] }); h.tab.objectType = 'view'; await h.view.reload();
  let menu, opened; h.app.showContextMenu = entries => { menu = entries; }; h.app.openTableTab = async (...args) => { opened = args; };
  h.view.grid.cb.onContextMenu(10, 20, 0); assert(menu.some(i => i.id === 'object-open')); assert(!menu.some(i => i.id === 'object-import'));
  h.view.grid.cb.onRowDoubleClick(0); await flush(); assert.deepEqual(opened, ['own', 'one', 'b', 'view']);
});
test('对话框输入有标签与关联错误，写入期间防重复且保留可见失败反馈', async () => {
  const h = harness(); const pending = deferred(); let submissions = 0;
  const dialog = h.env.openObjectAction({ title: '保存', context: 'own/one', label: '名称', confirm: '保存', submit: async () => { submissions++; await pending.promise; } });
  const input = dialog.body.querySelector('input'); assert(input.attributes['aria-describedby']); assert.equal(dialog.el.attributes.role, 'dialog');
  const saving = dialog.submit('query'); await dialog.submit('query'); assert.equal(submissions, 1); assert.equal(dialog.closed, false);
  pending.reject(new Error('disk full')); await saving; assert.equal(dialog.closed, false); assert.match(dialog.body.children.at(-1).textContent, /disk full/); assert.equal(dialog.body.children.at(-1).attributes['aria-live'], 'assertive');
});

test('收藏读取损坏明确呈现错误，不当成空列表且不自动覆盖', async () => {
  const h = harness(); h.tab.objectType = 'query'; h.app.favoritesError = '收藏解析失败，原文件已保留';
  await h.view.reload(); assert.equal(h.view.grid.rows.length, 0); assert.equal(h.calls.length, 0);
  assert.match(feedback(h).textContent, /收藏解析失败，原文件已保留/);
  assert.match(feedback(h).textContent, /尚未取得对象清单/);
  assert.doesNotMatch(feedback(h).textContent, /当前数据库没有已保存查询/);
});
test('删除收藏落盘失败保留打开的 ID 和原文，重试后才解除绑定', async () => {
  let fail = true; const h = harness({ removeFavorite: async () => { if (fail) throw new Error('delete denied'); return []; } });
  const f = favorite('q', 'one'); h.app.favorites = [f]; const { q, qv } = h.query(f); h.app.deleteSavedQuery(f.id);
  const dialog = h.dialogs.at(-1); await dialog.submit(); assert(!dialog.closed); assert.equal(q.savedQueryId, f.id); assert.equal(h.app.favorites.length, 1);
  fail = false; await dialog.submit(); assert.equal(q.savedQueryId, undefined); assert.equal(qv.editor.getSql(), f.sql);
});
test('真实目录中的同名重载按完整签名选择，打开两个页面而不退回裸名', async () => {
  const definitionReads = [], routineReads = [];
  const items = ['integer', 'text'].map(args => {
    const item = descriptor('own', 'one', 'function', 'same', args);
    item.object.catalog = 'fixture_pg_db'; item.object_key = objectKey(item.object); item.display_name = `same(${args})`; return item;
  });
  const h = harness({
    listObjectCatalog: async scope => catalogPage(scope, items),
    objectDefinition: async object => { definitionReads.push(plain(object)); return { object, backend_object_id: `function:${object.identity_arguments}`, definition: `CREATE FUNCTION same(${object.identity_arguments})`, revision: 'definition-fixture-revision', properties: {}, completeness: 'complete' }; },
    readRoutineDesignObject: async object => { routineReads.push(plain(object)); return snapshot(object, `CREATE FUNCTION same(${object.identity_arguments})`); },
  });
  h.app.connections[0].kind = 'postgresql'; h.tab.objectType = 'function'; await h.view.reload();
  assert.equal(h.view.grid.rows.length, 2); assert.equal(h.view.grid.columns.length, 5);
  for (const args of ['text', 'integer']) {
    h.app.activeTabId = h.tab.id; h.view.setSearchValue(args);
    assert.equal(h.view.grid.rows.length, 1); h.view.grid.cb.onSelectionChange([0]); await flush();
    assert.equal(h.view.selectedObject().descriptor.object.identity_arguments, args);
    await h.app.runCommand('object-open');
    assert.equal(h.app.activeTab.kind, 'routinedesign'); assert.equal(h.app.activeTab.routineObject.catalog, 'fixture_pg_db');
    assert.equal(h.app.activeTab.routineObject.identity_arguments, args); assert.equal(h.app.activeTab.routineSnapshot.source_fingerprint, sourceFingerprint);
  }
  assert.deepEqual(routineReads.map(object => object.identity_arguments), ['text', 'integer']);
  assert.deepEqual(definitionReads.map(object => object.identity_arguments), ['text', 'integer']);
  const routines = h.app.tabs.filter(tab => tab.kind === 'routinedesign'); assert.equal(routines.length, 2);
  assert.notEqual(objectKey(routines[0].routineObject), objectKey(routines[1].routineObject));
  assert(!h.calls.some(([name]) => name.startsWith('execute')));
});
test('同名触发器保留各自父对象，迟到详情不能覆盖后来选中的完整身份', async () => {
  const old = deferred(), current = deferred(), reads = [];
  const items = ['items_a', 'items_b'].map(parentName => {
    const item = descriptor('own', 'one', 'trigger', 'same_trigger', null);
    item.object.parent = { schema: 'one', kind: 'table', name: parentName };
    item.object_key = objectKey(item.object); item.display_name = `same_trigger — one.${parentName}`; return item;
  });
  const h = harness({ listObjectCatalog: async scope => catalogPage(scope, items),
    objectDefinition: object => { reads.push(plain(object)); return object.parent.name === 'items_a' ? old.promise : current.promise; },
  });
  h.tab.objectType = 'trigger'; await h.view.reload(); assert.equal(h.view.grid.rows.length, 2);
  h.view.grid.cb.onSelectionChange([0]); h.view.grid.cb.onSelectionChange([1]);
  assert.equal(h.view.selectedObject().descriptor.object.parent.name, 'items_b');
  const definition = (item, sql) => ({ object: item.object, backend_object_id: item.backend_object_id, definition: sql, revision: 'trigger-fixture-revision', properties: {}, completeness: 'complete' });
  current.resolve(definition(items[1], 'CREATE TRIGGER on_items_b')); await flush();
  old.resolve(definition(items[0], 'CREATE TRIGGER on_items_a')); await flush();
  assert.deepEqual(reads.map(object => object.parent.name), ['items_a', 'items_b']); assert.notEqual(items[0].object_key, items[1].object_key);
  assert.equal(h.infoStates.at(-1).fields.find(field => field.label === '父对象').value, 'one.items_b');
  assert.equal(h.infoStates.at(-1).fields.find(field => field.label === '定义').value, 'CREATE TRIGGER on_items_b');
});
test('目录失败保留当前作用域旧对象与未知说明，搜索和换库不冒充已知空目录', async () => {
  let fail = false;
  const h = harness({ listObjectCatalog: async scope => { if (fail) throw new Error('metadata permission denied'); return catalogPage(scope, [descriptor(scope.connection_id, scope.schema, 'function', 'existing')]); } });
  h.tab.objectType = 'function'; await h.view.reload(); assert.equal(h.view.grid.rows.length, 1);
  h.tab.selectedRows.add(0); fail = true; await h.view.reload();
  assert.equal(h.view.grid.rows.length, 1); assert.equal(h.tab.selectedRows.size, 0); assert.equal(h.view.selectedObject(), null);
  assert.match(h.view.metadataNotice, /metadata permission denied/);
  h.view.setSearchValue('missing'); assert.equal(h.view.grid.rows.length, 0);
  assert.equal(h.view.selectedObject(), null);
  h.tab.schema = 'two'; await h.view.reload(); assert.equal(h.view.grid.rows.length, 0);
  // 读取说明必须真正呈现在可见页脚，不能只检查私有状态。
  assert.match(feedback(h).textContent, /目录完整性未知/);
  assert.doesNotMatch(feedback(h).textContent, /当前数据库没有/);
  assert.equal(h.view.selectedObject(), null); assert(!h.calls.some(([name]) => name.startsWith('execute')));
});
function feedback(h) {
  const node = h.view.el.querySelector('[role="status"]');
  assert(node, '对象读取状态必须存在于真实DOM中');
  assert.equal(node.attributes['aria-atomic'], 'true');
  assert.equal(h.view.grid.el.attributes['aria-describedby'], node.id);
  assert.equal(node.style.display ?? '', '');
  assert.equal(node.hidden, undefined);
  return node;
}

test('VM事件移除和父节点文本合并按DOM语义工作', () => {
  const h = harness(), parent = h.document.createElement('div'), child = h.document.createElement('span');
  child.textContent = '完整性未知'; parent.append(h.document.createTextNode('状态：'), child);
  assert.equal(parent.textContent, '状态：完整性未知');
  child.textContent = '已加载'; assert.equal(parent.textContent, '状态：已加载');
  let calls = 0; const listener = () => { calls++; };
  for (const target of [parent, h.document, h.env.window]) {
    target.addEventListener('test-event', listener); target.fire('test-event');
    target.removeEventListener('test-event', listener); target.fire('test-event');
  }
  assert.equal(calls, 3);
  parent.textContent = '重置'; assert.equal(parent.children.length, 0); assert.equal(parent.textContent, '重置');
});

test('首次加载前与加载中可见状态不同，成功空结果有确定空态', async () => {
  const pending = deferred(), h = harness({ listObjects: () => pending.promise });
  assert.equal(feedback(h).textContent, '尚未加载对象清单');
  const loading = h.view.reload();
  assert.equal(h.view.loading, true); assert.equal(h.view.el.attributes['aria-busy'], 'true');
  assert.match(feedback(h).textContent, /正在加载表清单/); assert.equal(feedback(h).attributes['aria-live'], 'polite');
  assert.equal(h.view.selectedObject(), null);
  pending.resolve([]); await loading;
  assert.equal(h.view.el.attributes['aria-busy'], 'false'); assert.equal(h.view.loading, false);
  assert.equal(feedback(h).textContent, '当前数据库没有表');
});

test('旧表和视图列表继续使用listObjects且不借用目录空态', async () => {
  for (const type of ['table', 'view']) {
    const reads = [], h = harness({ listObjects: async (...scope) => { reads.push(scope); return [{ name: 't', type: 'table' }, { name: 'v', type: 'view' }]; }, listObjectCatalog: async () => assert.fail('表和视图必须保留旧listObjects行为') });
    h.tab.objectType = type; await h.view.reload();
    assert.deepEqual(reads, [['own', 'one']]); assert.equal(h.view.grid.rows.length, 1);
    assert.equal(h.view.grid.rows[0][0], type === 'table' ? 't' : 'v');
    assert.equal(feedback(h).textContent, `1 个${type === 'table' ? '表' : '视图'}`);
  }
});

test('已知空目录与权限未知目录不能显示同一空态', async () => {
  const h = harness({ listObjectCatalog: async scope => catalogPage(scope) }); h.tab.objectType = 'function';
  await h.view.reload(); assert.equal(feedback(h).textContent, '当前数据库没有函数');
  h.api.listObjectCatalog = async scope => ({ ...catalogPage(scope), completeness: 'unknown', sections: { function: { state: 'unknown', reason: 'permission denied by catalog' }, procedure: { state: 'known', reason: null } } });
  await h.view.reload(); assert.equal(h.view.grid.rows.length, 0);
  assert.match(feedback(h).textContent, /目录完整性未知.*permission denied by catalog/);
  assert.match(feedback(h).textContent, /尚不能确认当前类别为空/);
  assert.equal(feedback(h).attributes['aria-live'], 'assertive');
  assert.doesNotMatch(feedback(h).textContent, /当前数据库没有函数/);
});

test('未知空目录保留同scope旧对象仅供核对，所有旧行动作均被阻止', async () => {
  const pending = deferred(), item = descriptor('own', 'one', 'function', 'kept', 'text');
  let reload = false;
  const h = harness({ listObjectCatalog: scope => reload ? pending.promise : Promise.resolve(catalogPage(scope, [item])) });
  h.tab.objectType = 'function'; await h.view.reload(); h.view.grid.cb.onSelectionChange([0]);
  const actions = [], details = []; h.app.runCommand = async id => actions.push(id); h.app.showCatalogDetails = object => details.push(object); h.app.showContextMenu = () => actions.push('menu');
  reload = true; const loading = h.view.reload();
  assert.match(feedback(h).textContent, /正在加载.*上次已加载，仅供核对/);
  for (const attempt of [() => h.view.grid.cb.onSelectionChange([0]), () => h.view.grid.cb.onRowDoubleClick(0), () => h.view.grid.cb.onContextMenu(10, 20, 0), () => h.view.grid.el.fire('keydown', { key: 'Enter' }), () => h.view.grid.el.fire('keydown', { key: 'ContextMenu' })]) attempt();
  assert.equal(h.view.selectedObject(), null); assert.equal(h.tab.selectedRows.size, 0); assert.equal(actions.length, 0); assert.equal(details.length, 0);
  pending.resolve({ ...catalogPage({ kinds: ['function', 'procedure'] }), completeness: 'unknown', sections: { function: { state: 'unknown', reason: 'permission lost' }, procedure: { state: 'known', reason: null } } }); await loading;
  assert.equal(h.view.grid.rows.length, 1); assert.match(feedback(h).textContent, /permission lost.*上次已加载，仅供核对/);
  h.view.grid.cb.onSelectionChange([0]); h.view.grid.cb.onRowDoubleClick(0); h.view.grid.cb.onContextMenu(10, 20, 0); h.view.grid.el.fire('keydown', { key: 'Enter' });
  assert.equal(h.view.selectedObject(), null); assert.equal(h.tab.selectedRows.size, 0); assert.equal(actions.length, 0); assert.equal(details.length, 0);
});

test('listObjects读取失败持久显示错误，刷新成功后才重新允许选择', async () => {
  let failed = false;
  const h = harness({ listObjects: async () => { if (failed) throw new Error('list read offline'); return [{ name: 'old_table', type: 'table' }]; } });
  await h.view.reload(); failed = true; await h.view.reload();
  assert.equal(h.view.grid.rows.length, 1); assert.match(feedback(h).textContent, /加载失败.*list read offline.*上次已加载清单/);
  h.view.grid.cb.onSelectionChange([0]); assert.equal(h.view.selectedObject(), null);
  h.view.setSearchValue('missing'); assert.match(feedback(h).textContent, /list read offline.*显示 0 \/ 1 个表/);
  assert.doesNotMatch(feedback(h).textContent, /当前数据库没有表/);
  failed = false; h.view.setSearchValue(''); await h.view.reload();
  assert.equal(feedback(h).textContent, '1 个表'); h.view.grid.cb.onSelectionChange([0]);
  assert.equal(h.view.selectedObject().name, 'old_table'); assert.match(feedback(h).textContent, /已选中 1 行/);
});

test('首次读取失败可见错误不能被计数或toast替代', async () => {
  const h = harness({ listObjects: async () => { throw new Error('metadata access denied'); } }); await h.view.reload();
  assert.equal(h.view.grid.rows.length, 0); assert.match(feedback(h).textContent, /加载失败.*metadata access denied.*尚未取得对象清单/);
  assert.match(feedback(h).textContent, /尚不能确认当前类别为空/); assert.doesNotMatch(feedback(h).textContent, /当前数据库没有/);
  assert.equal(h.view.loading, false); assert.equal(h.view.el.attributes['aria-busy'], 'false');
  assert.equal(feedback(h).style.whiteSpace, 'normal'); assert.equal(feedback(h).style.height, 'auto');
});

test('不支持和缺少类别状态均有可见反馈，不能被空对象数组隐藏', async () => {
  const h = harness(); h.tab.objectType = 'function';
  h.app.navigationCatalog = async () => ({ objects: [], revision: 'partial', sections: { function: { state: 'unsupported', reason: 'unsupported function catalog' } } });
  await h.view.reload(); assert.match(feedback(h).textContent, /unsupported function catalog/);
  assert.match(feedback(h).textContent, /后端未提供此类别的读取状态/);
  assert.match(feedback(h).textContent, /尚不能确认当前类别为空/); assert.doesNotMatch(feedback(h).textContent, /当前数据库没有/);
});

test('部分可见目录展示完整对象身份及未知说明，不能误报完整', async () => {
  const item = descriptor('own', 'one', 'function', 'overload', 'text'); item.display_name = 'overload(text)';
  const h = harness(); h.tab.objectType = 'function';
  h.app.navigationCatalog = async () => ({ objects: [item], revision: 'partial', sections: { function: { state: 'unknown', reason: 'visible rows only' }, procedure: { state: 'known', reason: null } } });
  await h.view.reload(); assert.equal(h.view.grid.rows[0][0], 'overload(text)');
  assert.match(feedback(h).textContent, /visible rows only.*当前可见清单，可能不完整/);
  h.app.showCatalogDetails = () => {}; h.view.grid.cb.onSelectionChange([0]);
  assert.equal(objectKey(h.view.selectedObject().descriptor.object), item.object_key);
});

test('空收藏和损坏收藏通过可见反馈区分，原记录从不自动覆盖', async () => {
  const h = harness(); h.tab.objectType = 'query'; await h.view.reload();
  assert.equal(feedback(h).textContent, '当前数据库没有已保存查询');
  const saved = favorite('kept-query', 'one'); h.app.favorites = [saved]; await h.view.reload();
  h.app.favoritesError = 'favorites checksum invalid'; await h.view.reload();
  assert.equal(h.view.grid.rows.length, 1); assert.match(feedback(h).textContent, /favorites checksum invalid.*上次已加载清单/);
  assert.equal(h.view.selectedObject(), null); assert.deepEqual(h.app.favorites, [saved]); assert.equal(h.calls.length, 0);
});

test('搜索没有匹配与库中已知为空分开显示，搜索清空即恢复计数', async () => {
  const h = harness({ listObjects: async () => [{ name: 'only', type: 'table' }] }); await h.view.reload();
  h.view.setSearchValue('absent'); assert.equal(feedback(h).textContent, '没有匹配的表 · 共 1 个表');
  h.view.setSearchValue(''); assert.equal(feedback(h).textContent, '1 个表');
});

test('切换scope立即清除旧清单，迟到错误不覆盖当前成功反馈', async () => {
  const old = deferred(); let calls = 0;
  const h = harness({ listObjects: async (_connection, schema) => { if (schema === 'one' && ++calls > 1) return old.promise; return [{ name: schema === 'one' ? 'old' : 'current', type: 'table' }]; } });
  await h.view.reload(); h.view.setSearchValue('old'); const obsolete = h.view.reload();
  h.tab.schema = 'two'; const current = h.view.reload();
  assert.equal(h.view.grid.rows.length, 0); assert.equal(h.view.searchValue(), ''); assert.match(feedback(h).textContent, /正在加载/);
  await current; old.reject(new Error('late failed old scope')); await obsolete;
  assert.equal(h.view.grid.rows[0][0], 'current'); assert.equal(feedback(h).textContent, '1 个表'); assert.equal(h.notices.length, 0);
});

test('销毁页签后迟到加载不能发布数据或错误，再次reload不访问后端', async () => {
  const pending = deferred(); let reads = 0;
  const h = harness({ listObjects: async () => { reads++; return pending.promise; } });
  const loading = h.view.reload(); h.view.destroy(); const text = feedback(h).textContent;
  pending.reject(new Error('late after dispose')); await loading; await h.view.reload();
  assert.equal(reads, 1); assert.equal(h.view.grid.disposed, true); assert.equal(feedback(h).textContent, text); assert.equal(h.notices.length, 0);
});

test('夹具即使断言失败也销毁被替换视图并清理计时器', async () => {
  const h = harness(); const { q } = h.query(); h.app.views.delete(q.id);
  let disposed = 0; h.trackView({ destroy() { disposed++; } });
  h.env.setTimeout(() => assert.fail('已清理的计时器不应执行'), 60000);
  try { assert.fail('模拟业务负例断言失败'); } catch (error) { assert.equal(error.code, 'ERR_ASSERTION'); } finally { disposeHarness(h); }
  assert.equal(disposed, 1); assert.equal(h.timers.size, 0); assert.equal(h.document.listeners.size, 0); assert.equal(liveHarnesses.has(h), false);
});

test('同名视图和例程设计页跨库/类型分别定位，已有页面重开不覆盖 dirty', async () => {
  const reads = [];
  const h = harness({ getViewDefinition: async () => 'CREATE VIEW v AS SELECT 1',
    listObjectCatalog: async scope => catalogPage(scope, scope.kinds.map(kind => descriptor(scope.connection_id, scope.schema, kind, 'same'))),
    readRoutineDesignObject: async object => { reads.push(plain(object)); return snapshot(object, helpers.newObjectSql(cfg(), object.schema, object.name, object.kind)); },
  });
  await h.app.openViewDesignTab('own', 'one', 'same'); await h.app.openViewDesignTab('own', 'two', 'same');
  await h.app.openRoutineDesignTab('own', 'one', 'same', 'function'); await h.app.openRoutineDesignTab('own', 'one', 'same', 'procedure');
  assert.equal(h.app.tabs.length, 5);
  assert.deepEqual(reads.map(object => [object.schema, object.kind, object.identity_arguments]), [['one', 'function', ''], ['one', 'procedure', '']]);
  const routine = h.app.tabs.find(t => t.kind === 'routinedesign' && t.objectType === 'function');
  assert.equal(routine.routineSnapshot.source_fingerprint, sourceFingerprint); assert.equal(routine.routineSnapshot.completeness, 'complete');
  const rv = new h.env.classes.RoutineTabView(h.app, routine); h.app.views.set(routine.id, rv);
  rv.document.editor.setSql('dirty routine content'); assert.equal(rv.draftDirty(), true);
  await h.app.openRoutineDesignTab('own', 'one', 'same', 'function', routine.routineObject);
  assert.equal(h.app.activeTab.id, routine.id); assert.equal(rv.getSql(), 'dirty routine content'); assert.equal(reads.length, 2, '重开已有精确页面不重读/替换加载基线');
  const tab = h.app.tabs.find(t => t.kind === 'viewdesign' && t.schema === 'one'); tab.sql = 'dirty content'; tab.dirty = true;
  await h.app.openViewDesignTab('own', 'one', 'same'); assert.equal(h.app.activeTab.id, tab.id); assert.equal(tab.sql, 'dirty content');
});

test('原连接已删除的查询仍保留保存归属和原文，运行受失效守卫保护', async () => {
  const h = harness(); const f = favorite('gone-query', 'original_db', { connection_id: 'gone-connection' }); h.app.favorites = [f];
  await h.app.openSavedQuery(f.id); const tab = h.app.activeTab;
  assert.equal(tab.connectionId, 'gone-connection'); assert.equal(tab.schema, 'original_db'); assert.equal(tab.sql, f.sql); assert.match(tab.datasourceStale, /原连接已不存在/); assert.equal(h.calls.length, 0);
});
test('SQLite 连接树提供查询组且只展示 main 的已归属 SQL', async () => {
  const h = harness({ listTables: async () => [['table1', 'table']] }); h.app.connections[0].kind = 'sqlite';
  h.app.favorites = [favorite('main-query', 'main'), favorite('elsewhere', 'other'), favorite('legacy', null)];
  const node = { id: 'conn:own', children: [] }; await h.app.loadChildren(node);
  const group = node.children.find(n => n.id.endsWith('/grp-query')); assert(group); assert.deepEqual(Array.from(group.children, f => f.id), ['conn:own/query/main-query']);
  await h.app.selectNode(group); assert.equal(h.app.activeTab.objectType, 'query'); assert.equal(h.app.activeTab.schema, 'main');
});

test('对象搜索真实命令可达，刷新保留同一输入焦点与IME，清空和Esc恢复全列表', async () => {
  const h = harness({ listObjects: async () => [{ name: '中文表', type: 'table' }, { name: 'other', type: 'table' }] });
  await h.view.reload();
  h.env.dropArrow = () => '';
  vm.runInContext(ts.transpile(read('src/ui/TabToolbar.ts').replace(/^import .*;\r?$/gm, ''), { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS }), h.env);
  h.app.tabToolbar = new h.env.exports.TabToolbar({ onCommand: id => void h.app.runCommand(id) });
  delete h.app.notifyToolbar;
  let searches = 0; const apply = h.view.applySearch.bind(h.view); h.view.applySearch = () => { searches++; apply(); };
  await h.app.runCommand('object-search');
  const input = h.app.objectSearchInput; assert(input, 'object-* 路由不能吞掉搜索命令'); assert.equal(h.document.activeElement, input);
  input.selectionStart = 1; input.selectionEnd = 1;
  for (let n = 0; n < 3; n++) h.app.notifyToolbar();
  assert.equal(h.app.objectSearchInput, input); assert.equal(h.document.activeElement, input); assert.equal(input.selectionStart, 1);
  input.fire('compositionstart'); input.value = '中文'; input.fire('input'); h.app.notifyToolbar();
  assert.equal(h.view.searchValue(), ''); assert.equal(h.app.objectSearchInput, input);
  input.fire('compositionend'); input.fire('input'); assert.equal(searches, 1); assert.equal(h.view.grid.rows.length, 1);
  input.value = 'missing'; input.fire('input'); assert.equal(h.view.grid.rows.length, 0);
  input.value = ''; input.fire('input'); assert.equal(h.view.grid.rows.length, 2);
  input.value = '中文'; input.fire('input'); input.fire('keydown', { key: 'Escape' });
  assert.equal(h.view.searchValue(), ''); assert.equal(h.view.grid.rows.length, 2); assert.equal(h.app.objectSearchInput, null);
  assert.equal(h.document.activeElement.dataset.command, 'object-search');
});

test('无选中导出通过按钮、动作守卫和cmdExport，右键已选集合保留多对象预选', async () => {
  const h = harness({ listObjects: async () => [{ name: 'a', type: 'table' }, { name: 'b', type: 'table' }] }); await h.view.reload();
  const opened = []; h.env.openBatchExportWizard = (...args) => opened.push(args); h.app.showContextMenu = () => {};
  assert.equal(h.app.guardedTabToolbarItems(h.tab).find(item => item.id === 'object-export').disabled, false);
  await h.app.runCommand('object-export'); assert.equal(opened.length, 1); assert.deepEqual(plain(opened[0][2]), []);
  h.view.grid.cb.onSelectionChange([0, 1]); h.view.grid.cb.onContextMenu(10, 20, 1);
  assert.deepEqual(Array.from(h.tab.selectedRows), [0, 1]);
  await h.app.runCommand('object-export'); assert.deepEqual(plain(opened[1][2]), [{ name: 'a', object_type: 'table' }, { name: 'b', object_type: 'table' }]);
  assert.equal(opened[1][0].id, 'own'); assert.equal(opened[1][1], 'one');
  h.view.grid.cb.onContextMenu(10, 20, -1); assert.equal(h.tab.selectedRows.size, 0);
  await h.app.runCommand('object-export'); assert.deepEqual(plain(opened[2][2]), []);
});

test('备份页快速换库与销毁后迟到响应不能串库或发布状态', async () => {
  const one = deferred(), two = deferred(), requests = [];
  const h = harness({ listBackups: async (id, schema) => { requests.push([id, schema]); return schema === 'one' ? one.promise : two.promise; } });
  h.env.formatSize = value => String(value);
  vm.runInContext(ts.transpile(classText('BackupTabView') + '\nglobalThis.RealBackupView = BackupTabView;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), h.env);
  const tab = { id: 'backup', connectionId: 'own', schema: 'one', selectedRows: new Set() };
  const view = h.trackView(new h.env.RealBackupView(h.app, tab));
  const old = view.reload(); tab.schema = 'two'; const next = view.reload();
  two.resolve([{ name: 'two.sql', modified: '', size: 20, comment: '' }]); await next;
  one.resolve([{ name: 'one.sql', modified: '', size: 10, comment: '' }]); await old;
  assert.equal(view.grid.rows[0][0], 'two.sql'); assert.deepEqual(requests, [['own', 'one'], ['own', 'two']]);
  const late = deferred(); h.api.listBackups = () => late.promise;
  let published = 0; h.app.renderStatus = () => { published++; };
  const pending = view.reload(); view.destroy(); late.resolve([{ name: 'late.sql', size: 1 }]); await pending;
  assert.equal(published, 0); await view.reload(); assert.equal(published, 0);
});

