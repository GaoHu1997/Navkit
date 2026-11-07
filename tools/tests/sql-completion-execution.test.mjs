import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { transactionFixtureBindings } from './transaction-fixture-bindings.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const appSource = fs.readFileSync(path.join(root, 'src/core/App.ts'), 'utf8');
const editorSource = fs.readFileSync(path.join(root, 'src/ui/SqlEditor.ts'), 'utf8');
const helperContext = vm.createContext({ exports: {} });
vm.runInContext(ts.transpile(fs.readFileSync(path.join(root, 'src/core/sqlCompletion.ts'), 'utf8'),
  { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS }), helperContext);
const { analyzeSqlCompletion, sqlExecutionText } = helperContext.exports;
const metadata = {
  __tables__: ['users', 'orders', 'Order Items', 'active_users'],
  __views__: ['active_users'], __schemas__: ['main'],
  users: ['id', 'name', 'display name'], orders: ['id', 'user_id', 'total'],
  'Order Items': ['Item Id'], active_users: ['id', 'name'],
};
const plain = value => JSON.parse(JSON.stringify(value));
function complete(marked, data = metadata, schema = 'main', quote = '"') {
  const cursor = marked.indexOf('|');
  assert.ok(cursor >= 0, 'completion fixture needs cursor');
  return analyzeSqlCompletion(marked.replace('|', ''), cursor, data, ['SELECT', 'FROM', 'JOIN', 'WHERE'], ['COUNT'], schema, quote);
}
const labels = result => plain(result?.options.map(item => item.label) ?? []);

test('FROM/JOIN/UPDATE/INTO and FROM comma suggest real tables/views, not columns or special keys', () => {
  for (const sql of ['SELECT * FROM |', 'SELECT * FROM users u JOIN |', 'UPDATE |', 'INSERT INTO |', 'SELECT * FROM users, |']) {
    const result = complete(sql);
    assert.ok(labels(result).includes('users'), sql);
    assert.ok(labels(result).includes('active_users'), sql);
    assert.equal(result.options.find(o => o.label === 'active_users').detail, '视图');
    assert.ok(!labels(result).includes('name'), sql);
    assert.ok(!labels(result).some(label => label.startsWith('__')), sql);
  }
});

test('SELECT-list completion uses later FROM/JOIN aliases and only referenced fields', () => {
  assert.deepEqual(labels(complete('SELECT u.| FROM users AS u JOIN orders o ON u.id = o.user_id')), metadata.users);
  assert.deepEqual(labels(complete('SELECT o.| FROM users u JOIN orders AS o ON u.id = o.user_id')), metadata.orders);
  const result = labels(complete('SELECT na| FROM users'));
  assert.deepEqual(result, ['name']);
  assert.ok(!labels(complete('SELECT | FROM users')).includes('total'));
});

test('qualified relation/schema completion accepts current scope and rejects foreign metadata', () => {
  assert.deepEqual(labels(complete('SELECT main.users.| FROM main.users')), metadata.users);
  assert.ok(labels(complete('SELECT * FROM main.|')).includes('orders'));
  assert.deepEqual(labels(complete('SELECT foreign.users.| FROM foreign.users')), []);
  assert.deepEqual(labels(complete('SELECT unknown.| FROM users')), []);
  const qualified = { __tables__: ['sales.orders'], 'sales.orders': ['sale_id'] };
  assert.deepEqual(labels(complete('SELECT sales.orders.| FROM sales.orders', qualified, 'sales')), ['sale_id']);
});

test('backtick, double-quote and bracket names/aliases preserve identifier quoting', () => {
  for (const [open, close] of [['`', '`'], ['"', '"'], ['[', ']']]) {
    assert.deepEqual(labels(complete(`SELECT ${open}item alias${close}.| FROM ${open}Order Items${close} AS ${open}item alias${close}`)), ['Item Id']);
    const result = complete(`SELECT * FROM ${open}Ord|`);
    const quoted = result.options.find(o => o.label === 'Order Items');
    assert.ok(quoted);
    assert.equal(quoted.apply, `Order Items${close}`);
  }
  assert.equal(complete('SELECT * FROM Ord|', metadata, 'main', '`').options.find(o => o.label === 'Order Items').apply, '`Order Items`');
});

test('semicolon isolation and subquery scopes do not leak unrelated aliases', () => {
  assert.deepEqual(labels(complete('SELECT * FROM users u; SELECT u.| FROM orders o')), []);
  assert.deepEqual(labels(complete('SELECT o.| FROM users u WHERE EXISTS (SELECT 1 FROM orders o)')), []);
  assert.deepEqual(labels(complete('SELECT * FROM users u WHERE EXISTS (SELECT o.| FROM orders o)')), metadata.orders);
});

test('function arguments inherit query aliases; sibling subqueries remain isolated', () => {
  assert.deepEqual(labels(complete('SELECT COUNT(u.|) FROM users u')), metadata.users);
  assert.deepEqual(labels(complete('SELECT (SELECT o.| FROM orders o), (SELECT 1 FROM users o)')), metadata.orders);
  assert.deepEqual(labels(complete('SELECT (SELECT x.| FROM orders o), (SELECT 1 FROM users x)')), []);
});

test('completion replaces the identifier suffix at a mid-token cursor', () => {
  const sql = 'SELECT na|me FROM users';
  const result = complete(sql);
  assert.deepEqual(labels(result), ['name']);
  assert.equal(result.from, 7); assert.equal(result.to, 11);
  const quoted = complete('SELECT * FROM "Order It|ems"');
  assert.equal(quoted.options[0].apply, 'Order Items"');
  assert.equal(quoted.to, 'SELECT * FROM "Order Items"'.length);
});

test('strings and comments are not relations and completion does not enter them', () => {
  for (const sql of ["SELECT 'FROM users u |'", 'SELECT 1 -- FROM users u |', 'SELECT /* FROM users | */ 1', 'SELECT $$FROM users u |$$']) assert.equal(complete(sql), null, sql);
  assert.deepEqual(labels(complete("SELECT u.| FROM orders o WHERE 'FROM users u' = ''")), []);
  assert.deepEqual(labels(complete('SELECT u.| FROM orders o /* FROM users u */')), []);
});

test('view-only metadata works and __schemas__/__views__ never become columns', () => {
  const data = { __tables__: [], __views__: ['v'], __schemas__: ['main'], v: ['real_field'] };
  assert.deepEqual(labels(complete('SELECT v.| FROM v', data)), ['real_field']);
  assert.equal(complete('SELECT * FROM v|', data).options[0].detail, '视图');
});

test('selection existence wins over trimming; reversed and empty selections are deterministic', () => {
  const sql = 'SELECT 1;\n  \nSELECT 2;';
  assert.equal(sqlExecutionText(sql, 0, 0), sql);
  assert.equal(sqlExecutionText(sql, 0, 9), 'SELECT 1;');
  assert.equal(sqlExecutionText(sql, 9, 0), 'SELECT 1;');
  assert.equal(sqlExecutionText(sql, 10, 12), '  ');
});

class Element {
  constructor(tag = 'div') {
    this.tagName = tag; this.children = []; this.style = {}; this.attributes = {}; this.listeners = {}; this.className = ''; this.value = '';
    this.classList = { add: (...names) => { this.className += ' ' + names.join(' '); }, remove: (...names) => { this.className = this.className.split(' ').filter(n => !names.includes(n)).join(' '); } };
  }
  set innerHTML(value) { this.children = []; this._html = value; }
  get innerHTML() { return this._html ?? ''; }
  appendChild(child) { this.children.push(child); return child; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  setAttribute(name, value) { this.attributes[name] = value; }
  addEventListener(name, fn) { (this.listeners[name] ??= []).push(fn); }
  fire(name) { for (const fn of this.listeners[name] ?? []) fn({ target: this }); }
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function queryHarness() {
  const document = { createElement: tag => new Element(tag) };
  const calls = [], notices = [];
  const api = {
    ensureConnection: async id => { calls.push(['ensure', id]); },
    listDatabases: async id => { calls.push(['databases', id]); return [['main']]; },
    completionData: async (id, schema) => { calls.push(['metadata', id, schema]); return metadata; },
    executeSql: async (id, sql, schema, owner) => { calls.push(['execute', id, sql, schema, owner]); return { sets: [], messages: [], elapsed_ms: 1 }; },
  };
  class SqlEditor {
    constructor(opts) { this.opts = opts; this.el = new Element(); this.sql = opts.value; this.selection = ''; this.selectionExists = false; this.updates = []; }
    getSql() { return this.sql; }
    getSelection() { return this.selectionExists ? this.selection : this.sql; }
    hasSelection() { return this.selectionExists; }
    setCompletionData(data, schema) { this.updates.push([data, schema]); }
    focus() {}
    destroy() { this.destroyed = true; }
  }
  class DataGrid { constructor() { this.el = new Element(); } setData() {} dispose() {} }
  const context = vm.createContext({ ...transactionFixtureBindings, document, api, SqlEditor, DataGrid, iconSvgByName: () => '', renderCellPreview() {}, GRID_CONTEXT_MENU: [],
    TabView: class { constructor(app, tab) { this.app = app; this.tab = tab; this.el = new Element(); } },
  });
  const source = appSource.slice(appSource.indexOf('class QueryTabView extends'), appSource.indexOf('/** 表结构视图（可编辑'));
  vm.runInContext(ts.transpile(source + '\nglobalThis.QueryTabView = QueryTabView;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const tab = { id: 'query-a', connectionId: 'a', schema: '', sql: 'SELECT 1; SELECT 2;', results: null, activeResult: 0 };
  const transactions = new Map(), busyTransactions = new Set();
  const app = { connectionsValue: () => [{ id: 'a', name: 'A' }], connectionById: id => ({ id, name: id, kind: 'sqlite' }), settingsValue: {},
    manualTransactionFor: id => transactions.get(id), manualTransactionIsBusy: id => busyTransactions.has(id),
    refreshManualTransactionStatus: async id => transactions.get(id),
    toast: (...args) => notices.push(args), runCommand() {}, renderTabsPublic() {}, skipNullState: () => false, showGridLinesState: () => false };
  const view = new context.QueryTabView(app, tab);
  view.renderResults = () => {};
  return { view, tab, api, calls, notices, transactions, busyTransactions, app };
}

test('runbar, query menu, reload and editor callbacks execute selection or full text consistently', async () => {
  const { view, calls } = queryHarness();
  view.editor.selectionExists = true; view.editor.selection = 'SELECT 2;';
  await view.executeSelection(false);
  await view.executeSelection(true);
  await view.reload();
  assert.ok(calls.filter(c => c[0] === 'execute').every(c => c[2] === 'SELECT 2;'));
  calls.length = 0;
  view.editor.selectionExists = false;
  await view.executeSelection(true);
  assert.equal(calls[0][2], view.editor.sql);
  calls.length = 0;
  view.editor.selectionExists = true; view.editor.selection = '  ';
  await view.executeSelection(false);
  assert.equal(calls.length, 0, 'blank selection must never fall back to the whole document');
  assert.equal(view.runBtn.tagName, 'button');
  assert.equal(view.connSelect.attributes['aria-label'], '查询连接');
  assert.equal(view.stopBtn.disabled, true, 'no backend cancellation API means no fake stop');
  assert.match(view.stopBtn.title, /不支持取消/);
});

test('initial metadata waits for connection and database resolution', async () => {
  const { view, api, calls, tab } = queryHarness();
  const databases = deferred();
  api.listDatabases = async () => databases.promise;
  const loading = view.refreshCompletionData();
  await Promise.resolve(); await Promise.resolve();
  assert.equal(calls.some(c => c[0] === 'metadata'), false);
  databases.resolve([['main']]);
  await loading;
  assert.equal(tab.schema, 'main');
  assert.deepEqual(calls.find(c => c[0] === 'metadata'), ['metadata', 'a', 'main']);
  assert.deepEqual(plain(view.editor.updates.at(-1)), [metadata, 'main']);
});

test('late old-schema metadata cannot overwrite the new schema', async () => {
  const { view, tab, api } = queryHarness();
  const old = deferred();
  api.completionData = async (_id, schema) => schema === 'old' ? old.promise : { __tables__: ['new_table'], new_table: ['new_col'] };
  tab.schema = 'old'; const loadingOld = view.loadCompletionData();
  tab.schema = 'new'; await view.loadCompletionData();
  old.resolve({ __tables__: ['old_table'], old_table: ['old_col'] });
  await loadingOld;
  assert.deepEqual(plain(view.editor.updates.at(-1)), [{ __tables__: ['new_table'], new_table: ['new_col'] }, 'new']);
});

test('late old-connection database listings cannot rewrite current selection', async () => {
  const { view, api, tab } = queryHarness();
  const old = deferred();
  api.listDatabases = async id => id === 'a' ? old.promise : [['new_schema']];
  const first = view.refreshCompletionData();
  await Promise.resolve(); await Promise.resolve();
  await view.switchConnection('b');
  old.resolve([['old_schema']]); await first;
  assert.equal(tab.connectionId, 'b'); assert.equal(tab.schema, 'new_schema');
});

test('destroy prevents pending metadata publication; failures clear stale metadata and remain retryable', async () => {
  const { view, api, notices } = queryHarness();
  api.completionData = async () => { throw new Error('permission denied'); };
  await view.loadCompletionData();
  assert.equal(view.completionLoaded, false);
  assert.deepEqual(plain(view.editor.updates.at(-1)[0]), {});
  assert.match(notices[0][1], /permission denied/);
  const pending = deferred(); api.completionData = async () => pending.promise;
  const loading = view.loadCompletionData(); const count = view.editor.updates.length;
  view.destroy(); pending.resolve(metadata); await loading;
  assert.equal(view.editor.updates.length, count);
});

test('all editor execution shortcuts share selected-or-full dispatcher; query toolbar has no text dropdown', () => {
  for (const key of ['Mod-Enter', 'Mod-Shift-Enter', 'F5']) {
    const block = editorSource.slice(editorSource.indexOf(`key: "${key}"`), editorSource.indexOf(`key: "${key}"`) + 140);
    assert.match(block, /this\.executeCurrent\(\)/);
  }
  const queryToolbar = appSource.slice(appSource.indexOf('case "query": {'), appSource.indexOf('case "query": {') + 1400);
  // Bound the actual toolbar branch instead of matching other table/text controls.
  const toolbarItems = appSource.match(/items = \[[\s\S]*?id: "query-builder"[\s\S]*?\];/)?.[0] ?? queryToolbar;
  assert.ok(!toolbarItems.includes('id: "text"'));
  assert.match(editorSource, /menu\.setAttribute\("role", "menu"\)/);
  assert.match(editorSource, /event\.key === "Escape" \|\| event\.key === "Tab"/);
  assert.match(editorSource, /this\.view\.focus\(\)/);
});

test('real transaction status controls and execution gates preserve owner and reject conflicting work', async () => {
  const { view, tab, calls, notices, transactions, busyTransactions } = queryHarness();
  assert.equal(view.transactionStatus.textContent, '自动提交');
  tab.schema = 'main';
  const own = { active: true, owner: tab.id, schema: tab.schema, failed: false };
  transactions.set('a', own); view.syncManualTransactionControls();
  assert.equal(view.transactionStatus.textContent, transactionFixtureBindings.manualTransactionStatusText(own, tab.id));
  assert.equal(view.runBtn.disabled, false);
  assert.equal(view.explainBtn.disabled, true); assert.match(view.explainBtn.title, /解释计划.*提交或回滚/);
  assert.equal(view.connSelect.disabled, true); assert.equal(view.dbSelect.disabled, true);
  await view.explainPublic();
  view.explainBtn.fire('click'); view.editor.opts.onExplain('SELECT 1;');
  await Promise.resolve();
  assert.equal(calls.length, 0); assert.equal(own.failed, false);
  assert.match(notices.at(-1)[1], /解释计划.*提交或回滚/);
  view.editor.selectionExists = true; view.editor.selection = 'SELECT 2;';
  await view.executeSelection(false);
  assert.deepEqual(calls.find(call => call[0] === 'execute'), ['execute', 'a', 'SELECT 2;', 'main', tab.id]);
  await view.execute('SELECT 1;', true);
  assert.deepEqual(calls.filter(call => call[0] === 'execute').at(-1), ['execute', 'a', 'SELECT 1;', 'main', tab.id]);
  for (const status of [{ ...own, owner: 'other-query' }, { ...own, schema: 'other-schema' }, { ...own, failed: true }]) {
    calls.length = 0; transactions.set('a', status); view.syncManualTransactionControls();
    assert.equal(view.runBtn.disabled, true); assert.equal(view.explainBtn.disabled, true);
    await view.executeSelection(false);
    assert.equal(calls.length, 0); assert.match(notices.at(-1)[1], /事务|回滚/);
  }
  transactions.delete('a'); busyTransactions.add('a'); view.syncManualTransactionControls();
  assert.equal(view.transactionStatus.textContent, '事务操作进行中…');
  calls.length = 0; await view.executeSelection(false); assert.equal(calls.length, 0);
  busyTransactions.clear(); view.syncManualTransactionControls();
  assert.equal(view.transactionStatus.textContent, '自动提交'); assert.equal(view.runBtn.disabled, false);
  assert.equal(view.connSelect.disabled, false); assert.equal(view.dbSelect.disabled, false);
  assert.equal(view.explainBtn.disabled, false);
  calls.length = 0; await view.explainPublic();
  assert.deepEqual(calls.find(call => call[0] === 'execute'), ['execute', 'a', 'EXPLAIN SELECT 2;', 'main', undefined]);
});

test('automatic queries omit transaction owner; stale active requests never fall back to automatic commit', async () => {
  const { view, tab, api, app, calls, transactions } = queryHarness();
  tab.schema = 'main';
  const originalExecute = api.executeSql;
  api.executeSql = async (id, sql, schema, owner) => {
    if (owner !== undefined) {
      calls.push(['execute', id, sql, schema, owner]);
      throw new Error('该查询的手动事务已经结束，请刷新事务状态');
    }
    return originalExecute(id, sql, schema, owner);
  };
  await view.execute('SELECT 1;', false);
  assert.deepEqual(calls.find(call => call[0] === 'execute'), ['execute', 'a', 'SELECT 1;', 'main', undefined]);
  transactions.set('a', { active: true, owner: tab.id, schema: 'main', failed: false });
  app.refreshManualTransactionStatus = async () => { transactions.delete('a'); };
  calls.length = 0;
  await view.execute('SELECT 2;', true);
  assert.deepEqual(calls.filter(call => call[0] === 'execute'), [['execute', 'a', 'SELECT 2;', 'main', tab.id]]);
  assert.match(view.lastMessages[0].text, /手动事务已经结束/);
  assert.equal(view.lastMessages[0].error, true);
  calls.length = 0;
  await view.execute('SELECT 3;', false);
  assert.deepEqual(calls.find(call => call[0] === 'execute'), ['execute', 'a', 'SELECT 3;', 'main', undefined]);
});

test('real execute finally keeps controls busy until transaction status confirmation completes', async () => {
  const { view, app, calls } = queryHarness();
  const confirmation = deferred(); let confirms = 0;
  app.refreshManualTransactionStatus = async () => { confirms++; return confirmation.promise; };
  const executing = view.executeSelection(false);
  for (let turn = 0; turn < 4; turn++) await Promise.resolve();
  assert.equal(calls.filter(call => call[0] === 'execute').length, 1);
  assert.equal(confirms, 1); assert.equal(view.isExecuting(), true);
  assert.equal(view.runBtn.disabled, true); assert.equal(view.connSelect.disabled, true);
  confirmation.resolve(undefined); await executing;
  assert.equal(view.isExecuting(), false); assert.equal(view.runBtn.disabled, false);
  assert.equal(view.transactionStatus.textContent, '自动提交');
});
