import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { transactionFixtureBindings } from './transaction-fixture-bindings.mjs';
import { build } from 'esbuild';
import { webcrypto } from 'node:crypto';

// 当前查询页使用真实 ExecutionController；fixture 仍以显式终态模拟 IPC。
const executionBundle = await build({ entryPoints: [fileURLToPath(new URL('../../src/core/execution.ts', import.meta.url))], bundle: true, write: false, platform: 'node', format: 'esm', logLevel: 'silent' });
const { ExecutionController } = await import(`data:text/javascript;base64,${Buffer.from(executionBundle.outputFiles[0].text).toString('base64')}`);
const queryExecutionBundle = await build({ entryPoints: [fileURLToPath(new URL('../../src/core/queryExecution.ts', import.meta.url))], bundle: true, write: false, platform: 'node', format: 'esm', logLevel: 'silent' });
const { explainQuerySql } = await import(`data:text/javascript;base64,${Buffer.from(queryExecutionBundle.outputFiles[0].text).toString('base64')}`);

const queryPagesBundle = await build({ entryPoints: [fileURLToPath(new URL('../../src/core/queryResultPages.ts', import.meta.url))], bundle: true, write: false, platform: 'node', format: 'esm', logLevel: 'silent' });
const { QueryResultPages } = await import(`data:text/javascript;base64,${Buffer.from(queryPagesBundle.outputFiles[0].text).toString('base64')}`);

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
  __meta__: {
    version: 2, schema: 'main',
    relations: {
      users: { kind: 'table', comment: '用户基础信息', columns_known: true },
      orders: { kind: 'table', comment: null, columns_known: true },
      active_users: { kind: 'view', comment: null, columns_known: true },
    },
    columns: {
      users: {
        id: { type: 'BIGINT', comment: '用户编号' },
        name: { type: 'VARCHAR(64)', comment: null },
        'display name': { type: 'VARCHAR(64)', comment: null },
      },
    },
  },
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
    assert.equal(result.options.find(o => o.label === 'active_users').detail, undefined);
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

test('tables show only their comment; columns show type and comment; keywords stay bare', () => {
  const tables = complete('SELECT * FROM |').options;
  assert.equal(tables.find(o => o.label === 'users').detail, '用户基础信息');
  assert.equal(tables.find(o => o.label === 'orders').detail, undefined);
  assert.equal(tables.find(o => o.label === 'active_users').detail, undefined);

  const columns = complete('SELECT u.| FROM users AS u').options;
  assert.equal(columns.find(o => o.label === 'id').detail, 'BIGINT · 用户编号');
  assert.equal(columns.find(o => o.label === 'name').detail, 'VARCHAR(64)');

  const keywords = complete('SELECT | FROM users').options;
  assert.equal(keywords.find(o => o.label === 'SELECT').detail, undefined);
  assert.equal(keywords.find(o => o.label === 'COUNT').detail, undefined);
});

test('legacy metadata without __meta__ still shows the owning relation for columns', () => {
  const legacy = { __tables__: ['users'], __views__: [], __schemas__: ['main'], users: ['id', 'name'] };
  assert.equal(complete('SELECT * FROM |', legacy).options[0].detail, undefined);
  assert.equal(complete('SELECT u.| FROM users u', legacy).options[0].detail, 'users');
});

test('strings and comments are not relations and completion does not enter them', () => {
  for (const sql of ["SELECT 'FROM users u |'", 'SELECT 1 -- FROM users u |', 'SELECT /* FROM users | */ 1', 'SELECT $$FROM users u |$$']) assert.equal(complete(sql), null, sql);
  assert.deepEqual(labels(complete("SELECT u.| FROM orders o WHERE 'FROM users u' = ''")), []);
  assert.deepEqual(labels(complete('SELECT u.| FROM orders o /* FROM users u */')), []);
});

test('view-only metadata works and __schemas__/__views__ never become columns', () => {
  const data = { __tables__: [], __views__: ['v'], __schemas__: ['main'], v: ['real_field'] };
  assert.deepEqual(labels(complete('SELECT v.| FROM v', data)), ['real_field']);
  assert.equal(complete('SELECT * FROM v|', data).options[0].detail, undefined);
});

test('selection existence wins over trimming; reversed and empty selections are deterministic', () => {
  const sql = 'SELECT 1;\n  \nSELECT 2;';
  assert.equal(sqlExecutionText(sql, 0, 0), sql);
  assert.equal(sqlExecutionText(sql, 0, 9), 'SELECT 1;');
  assert.equal(sqlExecutionText(sql, 9, 0), 'SELECT 1;');
  assert.equal(sqlExecutionText(sql, 10, 12), '  ');
});

function completeWith(marked, functions, keywords = ['SELECT', 'FROM', 'JOIN', 'WHERE']) {
  const cursor = marked.indexOf('|');
  assert.ok(cursor >= 0, 'completion fixture needs cursor');
  return analyzeSqlCompletion(marked.replace('|', ''), cursor, metadata, keywords, functions, 'main', '"');
}

test('SELECT 列表提示查询内容（函数/*/字段）而不是表名', () => {
  const list = labels(completeWith('SELECT |', []));
  assert.ok(list.includes('*'), '应提供 *');
  assert.ok(!list.includes('users'), '没有 FROM 时不应把表名当查询内容');
  const fromUsers = labels(completeWith('SELECT | FROM users', []));
  assert.ok(fromUsers.includes('name') && fromUsers.includes('id'));
  assert.ok(fromUsers.indexOf('name') < fromUsers.indexOf('SELECT'), '字段应排在关键字之前');
});

test('函数补全自动插入括号并把光标放进括号', () => {
  for (const [marked, fn, cursor] of [['SELECT SU|', 'SUM', 4], ['SELECT AV|', 'AVG', 4], ['SELECT I|', 'IF', 3]]) {
    const option = completeWith(marked, [fn]).options.find(o => o.label === fn);
    assert.equal(option.apply, fn + '()', fn);
    assert.equal(option.cursor, cursor, fn);
  }
  const typedParen = completeWith('SELECT SU|M()', ['SUM']).options.find(o => o.label === 'SUM');
  assert.equal(typedParen.apply, undefined, '光标后已有左括号时不重复补');
});

test('CASE 自动插入完整骨架，并按 CASE 阶段提示 WHEN/THEN/ELSE/END', () => {
  const skeleton = completeWith('SELECT |', []).options.find(o => o.label === 'CASE');
  assert.equal(skeleton.apply, 'CASE WHEN  THEN  ELSE  END');
  assert.equal(skeleton.cursor, 'CASE WHEN '.length);
  assert.ok(labels(completeWith('SELECT CASE |', [])).includes('WHEN'));
  assert.ok(labels(completeWith('SELECT CASE WHEN x > 1 |', [])).includes('THEN'));
  const tail = labels(completeWith('SELECT CASE WHEN x THEN 1 |', []));
  assert.ok(tail.includes('ELSE') && tail.includes('END'));
});

test('表名之后先提示 WHERE/JOIN 等子句，而不是直接列字段', () => {
  const list = labels(completeWith('SELECT * FROM users u |', []));
  assert.ok(list.includes('WHERE') && list.includes('JOIN') && list.includes('GROUP BY'));
  assert.ok(list.indexOf('WHERE') < list.indexOf('name'), 'WHERE 应排在字段之前');
  const joined = labels(completeWith('SELECT * FROM users u JOIN orders o |', []));
  assert.ok(joined.indexOf('ON') < joined.indexOf('total'), 'JOIN 后应先给 ON');
});

test('写完一个字段后提示逗号续列与 FROM，而不是继续堆字段', () => {
  const list = labels(completeWith('SELECT id | FROM users', []));
  assert.ok(list.includes(',') && list.includes('FROM'));
  assert.ok(list.indexOf('FROM') < list.indexOf('name'), 'FROM 应排在字段之前');
  const nextField = labels(completeWith('SELECT id, | FROM users', []));
  assert.ok(nextField.includes('name') && nextField.indexOf('name') < nextField.indexOf('FROM'), '逗号后应继续给字段');
  const where = labels(completeWith('SELECT * FROM users WHERE id |', []));
  assert.ok(where.includes('=') && where.includes('LIKE'), 'WHERE 表达式后应给运算符');
});

class Element {
  constructor(tag = 'div') {
    this.tagName = tag; this.children = []; this.style = {}; this.attributes = {}; this.listeners = {}; this.className = ''; this.value = '';
    this.classList = { add: (...names) => { this.className += ' ' + names.join(' '); }, remove: (...names) => { this.className = this.className.split(' ').filter(n => !names.includes(n)).join(' '); } };
  }
  set innerHTML(value) { this.children = []; this._html = value; }
  get innerHTML() { return this._html ?? ''; }
  appendChild(child) { this.children.push(child); child.parentElement = this; return child; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  // 新代码使用 replaceChildren 管理结果面板；桩提供等价实现
  replaceChildren(...children) { this.children = [...children]; }
  remove() {}
  set textContent(value) { this._text = value; this.children = []; }
  get textContent() { return this._text ?? ''; }
  setAttribute(name, value) { this.attributes[name] = value; }
  removeAttribute(name) { delete this.attributes[name]; }
  addEventListener(name, fn) { (this.listeners[name] ??= []).push(fn); }
  querySelector() { return this.children.find(child => child.className?.includes('active')) ?? null; }
  focus() { this.focused = true; }
  fire(name, detail = {}) { for (const fn of this.listeners[name] ?? []) fn({ target: this, preventDefault() {}, ...detail }); }
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function queryHarness(bindings = {}) {
  // 查询页构造时会挂 document 级监听（selectionchange），桩提供 no-op。
  const document = { createElement: tag => new Element(tag), addEventListener() {}, removeEventListener() {} };
  const calls = [], notices = [], evidenceCalls = [];
  const api = {
    ensureConnection: async id => { calls.push(['ensure', id]); },
    listDatabases: async id => { calls.push(['databases', id]); return [['main']]; },
    completionData: async (id, schema) => { calls.push(['metadata', id, schema]); return metadata; },
    executeSql: async (id, sql, schema, owner) => { calls.push(['execute', id, sql, schema, owner]); return { sets: [], messages: [], elapsed_ms: 1 }; },
    dropQuerySpool: async scope => { calls.push(['drop', plain(scope)]); },
    fetchQueryRows: async (scope, index, offset, limit) => { calls.push(['fetch', plain(scope), index, offset, limit]); return { rows: Array.from({ length: Math.min(limit, 10001 - offset) }, (_, i) => [String(offset + i)]), total: 10001, truncated: false, message: '' }; },
    exportQuerySpooled: async (opts, scope, index) => { calls.push(['export-spool', plain(opts), plain(scope), index]); return 10001; },
    exportQueryResult: async (opts, columns, rows) => { calls.push(['export-loaded', plain(opts), plain(columns), plain(rows)]); return rows.length; },
    planQueryPagination: async () => ({ mode: 'unsupported', sql: null, reason: 'fixture explicit unsupported' }),
    planQueryKeyset: async () => ({ supported: false, reason: 'fixture no keyset' }),
    exportQueryRequery: async (opts, sql, owner, namespace) => { calls.push(['export-requery', plain(opts), sql, owner, namespace]); return 10001; },
    executionCapabilities: async () => ({ cancel: { level: 'unsupported', reason: 'fixture 不支持取消' } }),
    executeSqlV2: async request => {
      try { return { execution_id: request.execution_id, state: 'succeeded', result: await api.executeSql(request.connection_id, request.sql, request.namespace || undefined, request.transaction_owner ?? undefined) }; }
      catch (error) { return { execution_id: request.execution_id, state: 'failed', result: null, error: { message: error.message, code: 'FIXTURE_DATABASE_ERROR', retryable: false } }; }
    },
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
  class DataGrid { constructor() { this.el = new Element(); } setData(data) { this.data = data; } dispose() {} }
  // IPC explicitly declines editability; no row token, generation or editable snapshot is fabricated.
  const readOnlyEvidence = () => Object.assign(new Error('该结果未提供受控行来源证据，保持只读'), { code: 'QUERY_RESULT_READ_ONLY' });
  const queryResultApi = {
    snapshot: async (context, page) => { evidenceCalls.push(['snapshot', plain(context), plain(page)]); throw readOnlyEvidence(); },
    preview: async () => { evidenceCalls.push(['preview']); throw readOnlyEvidence(); },
    release: async context => { evidenceCalls.push(['release', plain(context)]); },
  };
  const context = vm.createContext({ ...transactionFixtureBindings, QueryResultPages, ExecutionController, explainQuerySql, logStamp: () => '2026-01-01 00:00:00', queryResultApi, crypto: webcrypto, setTimeout, clearTimeout, document, api, SqlEditor, DataGrid, iconSvgByName: () => '', svg: () => '', ICON_PAGE_FIRST: {}, ICON_PAGE_PREV: {}, ICON_PAGE_NEXT: {}, ICON_PAGE_LAST: {}, renderCellPreview() {}, GRID_CONTEXT_MENU: [],
    TabView: class { constructor(app, tab) { this.app = app; this.tab = tab; this.el = new Element(); } },
    ...bindings,
  });
  const source = appSource.slice(appSource.indexOf('class QueryTabView extends'), appSource.indexOf('/** 表结构视图（可编辑'));
  vm.runInContext(ts.transpile(source + '\nglobalThis.QueryTabView = QueryTabView;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const tab = { id: 'query-a', connectionId: 'a', schema: '', sql: 'SELECT 1; SELECT 2;', results: null, activeResult: 0, selectedRows: new Set() };
  const transactions = new Map(), busyTransactions = new Set();
  const app = { connectionsValue: () => [{ id: 'a', name: 'A' }], connectionById: id => ({ id, name: id, kind: 'sqlite' }), settingsValue: {},
    async guardLeave(ids, _cause, action) { assert.deepEqual(Array.from(ids), tab.schema || tab.savedQueryId ? ['query-a'] : []); await action(); return true; },
    manualTransactionFor: id => transactions.get(id), manualTransactionIsBusy: id => busyTransactions.has(id),
    refreshManualTransactionStatus: async id => transactions.get(id),
    toast: (...args) => notices.push(args), runCommand() {}, renderTabsPublic() {}, renderStatus() {}, notifyToolbar() {}, skipNullState: () => false, showGridLinesState: () => false };
  const view = new context.QueryTabView(app, tab);
  const renderResults = view.renderResults.bind(view);
  view.renderResults = () => {};
  return { view, tab, api, calls, notices, evidenceCalls, transactions, busyTransactions, app, renderResults };
}
async function readOnlyResultsSettled(view, evidenceCalls) {
  const deadline = Date.now() + 2000;
  while (view.resultEvidenceLoading) {
    assert.ok(Date.now() < deadline, 'the real evidence request must settle before paging, export or another execution');
    await new Promise(resolve => setImmediate(resolve));
  }
  assert.ok(evidenceCalls.some(call => call[0] === 'snapshot'), 'real rendering requests backend source evidence');
  assert.equal(view.controlledResult, null, 'rejected evidence cannot create an editable result panel');
  assert.equal(view.resultPanelHost.children.length, 0);
  assert.equal(view.grid.data.readOnly, true, 'fallback result grids remain read-only');
  assert.equal(evidenceCalls.some(call => call[0] === 'preview'), false, 'read-only paging and export never request a write plan');
}

test('run always executes the whole editor text; only run-selected reads the selection', async () => {
  const { view, calls, notices } = queryHarness();
  view.editor.selectionExists = true; view.editor.selection = '* FROM t_big';
  view.editor.sql = 'SELECT * FROM t_big WHERE id = 1';
  await view.executeSelection(false);
  assert.equal(calls.at(-1)[2], 'SELECT * FROM t_big WHERE id = 1',
    'run must never execute a stale partial selection (would drop SELECT and cause MySQL 1064)');
  await view.reload();
  assert.equal(calls.at(-1)[2], 'SELECT * FROM t_big WHERE id = 1', 'reload must also execute the whole text');

  calls.length = 0;
  await view.executeSelection(true);
  assert.equal(calls.at(-1)[2], '* FROM t_big', 'run-selected still executes exactly the selection');

  calls.length = 0;
  view.editor.selectionExists = false;
  await view.executeSelection(true);
  assert.equal(calls.length, 0, 'run-selected without a selection must never fall back to the whole document');
  assert.match(notices.at(-1)[1], /请先选中要执行的 SQL/);

  calls.length = 0;
  view.editor.selectionExists = true; view.editor.selection = '  ';
  await view.executeSelection(true);
  assert.equal(calls.length, 0, 'blank selection must never fall back to the whole document');
  assert.equal(view.runBtn.tagName, 'button');
  assert.equal(view.connSelect.attributes['aria-label'], '查询连接');
  assert.equal(view.stopBtn.disabled, true, 'no backend cancellation API means no fake stop');
  assert.match(view.stopBtn.title, /不支持取消|没有正在执行|取消能力/);
});

test('真实查询页停止等待后端终态，冻结上下文；本次执行一开始就作废上一次结果', async t => {
  const { view, api, tab } = queryHarness(), pending = deferred(); let sent, stops = 0;
  t.after(() => { view.execution.dispose(); pending.resolve({ execution_id: sent?.execution_id, state: 'cancelled', result: null, error: null }); });
  const old = { sets: [], messages: ['旧结果'], elapsed_ms: 1 }; tab.results = old;
  api.executionCapabilities = async () => ({ cancel: { level: 'supported', reason: 'fixture pinned 控制' } });
  api.executeSqlV2 = request => { sent = request; return pending.promise; };
  api.cancelExecution = async context => { assert.equal(context.execution_id, sent.execution_id); stops++; return { execution_id: context.execution_id, accepted: true, state: 'cancel_requested', message: '等待DB' }; };
  const running = view.execute('SELECT 1', false); await new Promise(resolve => setImmediate(resolve));
  assert.equal(view.stopBtn.disabled, false); assert.equal(view.connSelect.disabled, true); assert.equal(view.dbSelect.disabled, true); assert.equal(view.runbar.attributes['aria-busy'], 'true');
  view.stopBtn.fire('click'); await view.execution.cancel();
  assert.equal(stops, 1); assert.equal(view.running, true); assert.equal(view.stopBtn.disabled, true);
  pending.resolve({ execution_id: sent.execution_id, state: 'cancelled', result: null, error: null }); await running;
  // 新执行提交即失效旧结果：等待期间屏上不能留着上一次的输出被读成本次结果。
  assert.equal(tab.results, null); assert.equal(view.running, false); assert.match(view.lastMessages[0].text, /不保证回滚/); assert.equal(view.runbar.attributes['aria-busy'], 'false');
});
test('结果unknown锁住重试和新事务，保留SQL草稿但作废旧结果', async () => {
  const { view, api, tab, notices } = queryHarness(); let runs = 0; const old = { sets: [], messages: [], elapsed_ms: 1 }; tab.results = old;
  api.executeSqlV2 = async request => { runs++; return { execution_id: request.execution_id, state: 'unknown', result: null, error: { message: '控制通道失联' } }; };
  const sql = tab.sql; await view.execute(sql, false); assert.equal(tab.results, null); assert.equal(tab.sql, sql); assert.equal(view.runBtn.disabled, true);
  await view.execute(sql, false); assert.equal(runs, 1); assert.match(notices.at(-1)[1], /禁止重试/);
  await assert.rejects(view.prepareForTransaction(), /结果未知/);
});

test('initial metadata waits for connection and retains the explicitly chosen database', async () => {
  const { view, api, calls, tab } = queryHarness(); tab.schema = 'main';
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

test('initial metadata without a chosen database keeps SQL unsaved and the database unselected', async () => {
  const { view, tab, calls } = queryHarness(); await view.refreshCompletionData();
  assert.equal(tab.schema, ''); assert.equal(tab.savedQueryId, undefined); assert.equal(view.dbSelect.value, '');
  assert.deepEqual(calls.find(c => c[0] === 'metadata'), ['metadata', 'a', '']);
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
  await view.switchConnection('b'); assert.equal(tab.schema, '', 'first binding must not choose a database for the user');
  view.dbSelect.value = 'new_schema'; view.dbSelect.fire('change'); await new Promise(resolve => setImmediate(resolve));
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

test('editor shortcuts split run from run-selected; menu labels the real shortcut', () => {
  const expectations = { 'Mod-Enter': 'false', 'Mod-Shift-Enter': 'true', 'F5': 'false' };
  for (const [key, flag] of Object.entries(expectations)) {
    const start = editorSource.indexOf(`key: "${key}"`);
    const block = editorSource.slice(start, start + 160);
    assert.match(block, new RegExp(`this\\.executeCurrent\\(${flag}\\)`), `${key} must pass ${flag}`);
  }
  assert.match(editorSource, /label: "运行已选择的", shortcut: "Ctrl\+Shift\+Enter", run: \(\) => this\.executeCurrent\(true\)/);
  const queryToolbar = appSource.slice(appSource.indexOf('case "query": {'), appSource.indexOf('case "query": {') + 1400);
  // Bound the actual toolbar branch instead of matching other table/text controls.
  const toolbarItems = appSource.match(/items = \[[\s\S]*?id: "query-builder"[\s\S]*?\];/)?.[0] ?? queryToolbar;
  assert.ok(!toolbarItems.includes('id: "text"'));
  assert.match(editorSource, /menu\.setAttribute\("role", "menu"\)/);
  assert.match(editorSource, /event\.key === "Escape" \|\| event\.key === "Tab"/);
  assert.match(editorSource, /this\.view\.focus\(\)/);
});

test('查询页事务：其它页面持有时门禁执行，本页持有时允许执行 SQL', async () => {
  const { view, tab, calls, notices, transactions, busyTransactions } = queryHarness();
  // 需求：事务入口移至查询页，本页持有的手动事务允许在其中执行 SQL；其它页面持有的则门禁。
  assert.equal(view.transactionStatus, undefined);
  tab.schema = 'main';
  const own = { active: true, owner: 'tab-table', schema: tab.schema, failed: false };
  transactions.set('a', own); view.syncManualTransactionControls();
  assert.equal(view.runBtn.disabled, true); assert.equal(view.explainBtn.disabled, true);
  assert.match(view.runBtn.title, /其它页面持有/);
  assert.equal(view.connSelect.disabled, true); assert.equal(view.dbSelect.disabled, true);
  await view.explainPublic();
  assert.equal(calls.length, 0); assert.match(notices.at(-1)[1], /不能解释/);
  view.editor.selectionExists = true; view.editor.selection = 'SELECT 2;';
  await view.executeSelection(false);
  assert.equal(calls.length, 0);
  for (const status of [{ ...own, schema: 'other-schema' }, { ...own, failed: true }]) {
    calls.length = 0; transactions.set('a', status); view.syncManualTransactionControls();
    assert.equal(view.runBtn.disabled, true); assert.equal(view.explainBtn.disabled, true);
    await view.executeSelection(false);
    assert.equal(calls.length, 0);
  }
  // 本页持有的手动事务：允许整段执行，并把事务所有者传给执行请求；EXPLAIN 仍禁用。
  tab.schema = 'main';
  transactions.set('a', { active: true, owner: tab.id, schema: tab.schema, failed: false }); view.syncManualTransactionControls();
  assert.equal(view.runBtn.disabled, false); assert.equal(view.explainBtn.disabled, true);
  calls.length = 0;
  await view.executeSelection(false);
  assert.deepEqual(calls.find(call => call[0] === 'execute'), ['execute', 'a', view.editor.getSql(), 'main', tab.id]);
  transactions.delete('a'); busyTransactions.add('a'); view.syncManualTransactionControls();
  calls.length = 0; await view.executeSelection(false); assert.equal(calls.length, 0);
  busyTransactions.clear(); view.syncManualTransactionControls();
  assert.equal(view.runBtn.disabled, false);
  assert.equal(view.connSelect.disabled, false); assert.equal(view.dbSelect.disabled, false);
  assert.equal(view.explainBtn.disabled, false);
  calls.length = 0; await view.explainPublic();
  assert.equal(calls.length, 0, 'multi-statement explain must not execute later statements');
  assert.match(notices.at(-1)[1], /解释只支持单条语句/);
  view.editor.sql = 'SELECT 1;';
  await view.explainPublic();
  assert.deepEqual(calls.find(call => call[0] === 'execute'), ['execute', 'a', 'EXPLAIN SELECT 1', 'main', undefined],
    'explain from the toolbar explains one full-document statement, not a stale selection');
  calls.length = 0; await view.explainSelection(true);
  assert.deepEqual(calls.find(call => call[0] === 'execute'), ['execute', 'a', 'EXPLAIN SELECT 2', 'main', undefined],
    'explain-selected still explains exactly the selection');
  view.editor.selectionExists = false; calls.length = 0;
  await view.explainSelection(true);
  assert.equal(calls.length, 0, 'explain-selected without a selection must never fall back to the whole document');
  assert.match(notices.at(-1)[1], /请先选中要解释的 SQL/);
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
  // 等待 IPC/执行控制器进入挂起的确认阶段，不依赖固定微任务层数。
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(calls.filter(call => call[0] === 'execute').length, 1);
  assert.equal(confirms, 1); assert.equal(view.isExecuting(), true);
  assert.equal(view.runBtn.disabled, true); assert.equal(view.connSelect.disabled, true);
  confirmation.resolve(undefined); await executing;
  assert.equal(view.isExecuting(), false); assert.equal(view.runBtn.disabled, false);
});

function spoolResult(id = 'spool-a', totals = [10001]) {
  return { spool_id: id, sets: totals.map(total => ({ columns: [{ name: 'value', type: 'text' }], rows: Array.from({ length: Math.min(10000, total) }, (_, i) => [String(i)]), total, affected: 0, message: '' })), messages: [], elapsed_ms: 1 };
}
const spoolContext = { connectionId: 'a', ownerId: 'query-a', namespace: 'main' };
const exportOptions = { connection_id: 'a', schema: '', table: 'result', file: 'result.csv', format: 'csv', encoding: 'utf-8', delimiter: ',', has_header: true, where_clause: null, limit: null };

test('spool paging replaces rows, navigates the cached total beyond 10000 and isolates result sets', async () => {
  const pages = new QueryResultPages(200), result = spoolResult('spool', [10001, 700]);
  pages.replace(result, spoolContext);
  assert.equal(result.sets[0].rows.length, 200);
  assert.equal(pages.range(0), '第 1/51 页（共 10001 行）');
  const read = async (_scope, index, offset, limit) => ({ rows: Array.from({ length: Math.min(limit, result.sets[index].total - offset) }, (_, i) => [String(offset + i)]), total: result.sets[index].total, truncated: false, message: '' });
  await pages.fetch(0, pages.offset(0, 'next'), read);
  assert.equal(result.sets[0].rows.length, 200); assert.equal(result.sets[0].rows[0][0], '200');
  assert.equal(pages.page(1).offset, 0); assert.equal(result.sets[1].rows[0][0], '0');
  await pages.fetch(0, pages.offset(0, 'last'), read);
  assert.equal(pages.page(0).offset, 10000); assert.equal(result.sets[0].rows.at(-1)[0], '10000');
  assert.equal(pages.range(0), '第 51/51 页（共 10001 行）');
  await pages.fetch(0, pages.offset(0, 'previous'), read); assert.equal(pages.page(0).offset, 200);
  await pages.fetch(0, pages.offset(0, 'first'), read); assert.equal(pages.page(0).offset, 0);
});

test('byte-short pages advance by actual rows, preserve database total and retry a failed page', async () => {
  const pages = new QueryResultPages(), result = spoolResult(); pages.replace(result, spoolContext);
  await pages.fetch(0, 200, async () => ({ rows: [['200'], ['201']], total: 10000, truncated: true, message: 'byte limit' }));
  assert.equal(pages.offset(0, 'next'), 202); assert.equal(result.sets[0].total, 10001);
  const rows = result.sets[0].rows;
  await assert.rejects(pages.fetch(0, 202, async () => { throw new Error('read denied'); }), /read denied/);
  assert.equal(result.sets[0].rows, rows); assert.equal(pages.page(0).offset, 200); assert.equal(pages.busy, false);
  assert.match(pages.page(0).message, /保留当前页/);
  await pages.fetch(0, 202, async () => ({ rows: [['202']], total: 10000, truncated: false, message: '' }));
  assert.equal(pages.page(0).offset, 202); assert.equal(result.sets[0].rows.length, 1);
});

test('late page completion cannot overwrite a newer snapshot or another result', async () => {
  const pages = new QueryResultPages(), old = spoolResult('old'), fresh = spoolResult('new'), pending = deferred();
  pages.replace(old, spoolContext); const loading = pages.fetch(0, 200, () => pending.promise);
  assert.equal(pages.busy, true); const released = pages.replace(fresh, spoolContext); assert.equal(released.spoolId, 'old');
  pending.resolve({ rows: [['stale']], total: 10001, truncated: false, message: '' });
  assert.equal(await loading, false); assert.equal(fresh.sets[0].rows[0][0], '0'); assert.equal(pages.page(0).offset, 0);
});

/* These tests execute actual QueryTabView render, paging and export methods.
   Evidence IPC declines editable provenance, so real read-only fallback and async
   settlement are observed through the DOM. No editable token or generation is fabricated. */
test('real query result controls page from spool and keep exact keyboard accessible result labels', async () => {
  const { view, api, tab, calls, evidenceCalls, renderResults } = queryHarness(); view.renderResults = renderResults;
  api.executeSql = async () => spoolResult('spool', [10001, 700]); await view.execute('SELECT 1; SELECT 2', false);
  await readOnlyResultsSettled(view, evidenceCalls);
  assert.deepEqual(view.resultTabsEl.children.map(child => child.textContent), ['消息', '结果 1', '结果 2']);
  assert.equal(view.resultTabsEl.children[1].tagName, 'button'); assert.equal(view.resultTabsEl.children[1].attributes['aria-pressed'], 'true');
  await view.loadResultPage('next'); await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(view.grid.data.rows.length, 200); assert.equal(view.grid.data.rows[0][0], '200');
  assert.equal(view.grid.data.rowNumberOffset, 200);
  assert.equal(view.statusInfo().pageText, '共 10001 行');
  assert.equal(view.pageNumberInput.value, "2"); assert.equal(view.footerStatusEl.textContent, '运行耗时 0.001s | 共 10001 行');
  assert.equal(view.pageCountEl.textContent, '/ 51 页');
  view.resultTabsEl.children[2].fire('click'); await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(tab.activeResult, 1); assert.equal(view.grid.data.rows[0][0], '0');
  assert.equal(view.grid.data.rowNumberOffset, 0);
  view.resultTabsEl.children[1].fire('click'); await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(tab.activeResult, 0); assert.equal(view.grid.data.rows[0][0], '200');
  assert.equal(view.grid.data.rowNumberOffset, 200);
  assert.equal(calls.filter(call => call[0] === 'fetch').length, 1);
});

for (const sql of ['UPDATE t SET value = 1', 'DELETE FROM t', 'INSERT INTO t VALUES (1)', 'CREATE TABLE t (value INT)']) {
  test(`non-rowset execution shows only messages and affected rows: ${sql}`, async () => {
    for (const affected of [0, 7]) {
      const { view, api, tab, evidenceCalls, renderResults } = queryHarness(); view.renderResults = renderResults;
      api.executeSql = async () => ({ elapsed_ms: 125, messages: [], sets: [{ columns: [], rows: [], total: 0, affected, message: 'statement completed' }] });
      await view.execute(sql, false);
      assert.equal(tab.activeResult, -1);
      assert.deepEqual(view.resultTabsEl.children.map(child => child.textContent), ['消息']);
      assert.equal(view.grid.el.style.display, 'none'); assert.equal(view.messagesEl.style.display, '');
      assert.equal(view.footerStatusEl.textContent, `运行耗时 0.125s | 影响 ${affected} 行`);
      assert.equal(view.resultPager.style.display, 'flex'); assert.equal(view.pageControls.style.display, 'none');
      assert.equal(view.pageSizeSelect.parentElement.style.display, 'none');
      assert.match(view.lastMessages[0].text, new RegExp(`影响 ${affected} 行`));
      assert.equal(evidenceCalls.some(call => call[0] === 'snapshot'), false);
      view.destroy();
    }
  });
}

test('invalid or missing backend affected counts stay unknown instead of NaN or guessed zero', async () => {
  for (const affected of [undefined, -1, NaN, '7', Number.MAX_SAFE_INTEGER + 1]) {
    const { view, api, renderResults } = queryHarness(); view.renderResults = renderResults;
    api.executeSql = async () => ({ elapsed_ms: 125, messages: [], sets: [{ columns: [], rows: [], total: 0, affected, message: '' }] });
    await view.execute('UPDATE t SET value = 1', false);
    assert.equal(view.footerStatusEl.textContent, '运行耗时 0.125s | 影响行数未知');
    assert.match(view.lastMessages[0].text, /影响行数未知/);
    view.destroy();
  }
});

test('empty SELECT and writes with RETURNING remain real result sets', async () => {
  for (const [sql, rows] of [['SELECT value FROM t WHERE 1 = 0', []], ['UPDATE t SET value = 1 RETURNING value', [['1']]]]) {
    const { view, api, tab, evidenceCalls, renderResults } = queryHarness(); view.renderResults = renderResults;
    api.executeSql = async () => ({ elapsed_ms: 125, messages: [], sets: [{ columns: [{ name: 'value' }], rows, total: rows.length, affected: rows.length, message: '' }] });
    await view.execute(sql, false); await readOnlyResultsSettled(view, evidenceCalls);
    assert.equal(tab.activeResult, 0);
    assert.deepEqual(view.resultTabsEl.children.map(child => child.textContent), ['消息', '结果 1']);
    assert.equal(view.footerStatusEl.textContent, `运行耗时 0.125s | 共 ${rows.length} 行`);
    assert.equal(view.pageControls.style.display, 'flex');
    view.destroy();
  }
});

test('mixed batches hide receipts without renumbering cache, evidence and export indexes', async () => {
  const { view, api, tab, calls, evidenceCalls, renderResults } = queryHarness(); view.renderResults = renderResults;
  const result = spoolResult('mixed', [10001, 700]);
  const receipt = affected => ({ columns: [], rows: [], total: 0, affected, message: 'write completed' });
  result.sets = [receipt(3), result.sets[0], receipt(4), result.sets[1]];
  api.executeSql = async () => result;
  await view.execute('UPDATE t SET value = 1; SELECT * FROM t; DELETE FROM t; SELECT * FROM other', false);
  await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(tab.activeResult, 1);
  assert.deepEqual(view.resultTabsEl.children.map(child => child.textContent), ['消息', '结果 1', '结果 2']);
  await view.loadResultPage('next'); await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(calls.find(call => call[0] === 'fetch')[2], 1);
  assert.equal(evidenceCalls.find(call => call[0] === 'snapshot')[1].result_index, 1);
  await view.exportResultSnapshot(exportOptions, view.captureResultExport());
  assert.equal(calls.find(call => call[0] === 'export-spool')[3], 1);
  view.resultTabsEl.children[0].fire('click');
  assert.equal(view.footerStatusEl.textContent, '运行耗时 0.001s | 影响 7 行');
  assert.equal(view.pageControls.style.display, 'none');
  view.resultTabsEl.children[2].fire('click');
  assert.equal(view.resultTabsEl.children[2].focused, true);
  await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(tab.activeResult, 3); assert.equal(view.pageCountEl.textContent, '/ 4 页');
  view.destroy();
});

test('spooled export writes all execution rows with pinned scope and never sends loaded rows', async () => {
  const { view, api, calls, tab } = queryHarness(); tab.schema = 'main'; api.executeSql = async () => spoolResult();
  await view.execute('SELECT * FROM t', false); const snapshot = view.captureResultExport();
  assert.equal(await view.exportResultSnapshot(exportOptions, snapshot), 10001);
  const call = calls.find(call => call[0] === 'export-spool');
  assert.equal(call[2].ownerId, tab.id); assert.equal(call[2].namespace, 'main'); assert.equal(call[1].schema, 'main'); assert.equal(call[3], 0);
  assert.equal(calls.some(call => call[0] === 'export-loaded'), false);
  api.executeSql = async () => spoolResult('new'); await view.execute('SELECT 2', false);
  await assert.rejects(view.exportResultSnapshot(exportOptions, snapshot), /已经替换/);
});

test('legacy results explicitly export only loaded rows and never fetch or rerun SQL', async () => {
  const { view, api, calls, evidenceCalls, renderResults } = queryHarness(); view.renderResults = renderResults;
  const result = spoolResult(); delete result.spool_id; result.sets[0].rows = [['loaded']]; api.executeSql = async () => result;
  await view.execute('SELECT * FROM t', false); await readOnlyResultsSettled(view, evidenceCalls);
  assert.match(view.gridPreviewNoteEl.textContent, /只含已加载行/); assert.equal(view.pageButtons.every(button => button.disabled), true);
  assert.equal(await view.exportResultSnapshot(exportOptions, view.captureResultExport()), 1);
  assert.equal(calls.some(call => call[0] === 'export-spool' || call[0] === 'fetch'), false);
  assert.deepEqual(calls.find(call => call[0] === 'export-loaded')[3], [['loaded']]);
});

test('a new execution expires the previous spool up front; failure and success never leave it on screen', async () => {
  const { view, api, tab, calls, evidenceCalls, renderResults } = queryHarness(); view.renderResults = renderResults;
  api.executeSql = async () => spoolResult('old'); await view.execute('SELECT 1', false);
  await readOnlyResultsSettled(view, evidenceCalls);
  api.executeSql = async () => { throw new Error('syntax failure'); }; await view.execute('bad sql', false);
  assert.equal(tab.results, null); assert.equal(tab.activeResult, -1); assert.match(view.lastMessages[0].text, /syntax failure/);
  // 旧结果在本次执行提交时就被作废，缓存随之释放，不再作为「上次的只读快照」留在屏上。
  assert.deepEqual(calls.filter(call => call[0] === 'drop').map(call => call[1].spoolId), ['old']);
  api.executeSql = async () => spoolResult('new'); await view.execute('SELECT 2', false);
  await readOnlyResultsSettled(view, evidenceCalls);
  assert.deepEqual(calls.filter(call => call[0] === 'drop').map(call => call[1].spoolId), ['old']);
  await view.switchConnection('b'); assert.equal(tab.results, null);
  assert.deepEqual(calls.filter(call => call[0] === 'drop').map(call => call[1].spoolId), ['old', 'new']);
});

test('paging and exporting freeze execution and context controls until completion', async () => {
  const { view, api, calls } = queryHarness(); api.executeSql = async () => spoolResult(); await view.execute('SELECT 1', false);
  const pending = deferred(); api.fetchQueryRows = () => pending.promise; const loading = view.loadResultPage('next');
  assert.equal(view.resultBusy(), true); assert.equal(view.runBtn.disabled, true); assert.equal(view.explainBtn.disabled, true); assert.equal(view.connSelect.disabled, true);
  await view.execute('must not run', false); await view.switchConnection('b'); assert.equal(view.tab.connectionId, 'a');
  await assert.rejects(view.exportResultSnapshot(exportOptions, view.captureResultExport()), /进行中/);
  pending.resolve({ rows: [['200']], total: 10001, truncated: false, message: '' }); await loading;
  const exporting = deferred(); api.exportQuerySpooled = () => exporting.promise;
  const exportTask = view.exportResultSnapshot(exportOptions, view.captureResultExport()); assert.equal(view.runBtn.disabled, true); assert.equal(view.dbSelect.disabled, true);
  const count = calls.length; await view.loadResultPage('last'); await view.execute('must not run', false); assert.equal(calls.length, count);
  exporting.resolve(10001); await exportTask; assert.equal(view.resultBusy(), false); assert.equal(view.runBtn.disabled, false);
});

test('closing releases active spool and late execute completion releases its unpublished spool', async () => {
  const active = queryHarness(); active.api.executeSql = async () => spoolResult('active'); await active.view.execute('SELECT 1', false); active.view.destroy();
  assert.deepEqual(active.calls.filter(call => call[0] === 'drop').map(call => call[1].spoolId), ['active']);
  const { view, api, calls, tab } = queryHarness(), pending = deferred(); let request;
  api.executeSqlV2 = sent => { request = sent; return pending.promise; };
  const execution = view.execute('SELECT 1', false); await new Promise(resolve => setImmediate(resolve)); view.destroy();
  pending.resolve({ execution_id: request.execution_id, state: 'succeeded', result: spoolResult('late') }); await execution;
  assert.equal(tab.results, null); assert.deepEqual(calls.filter(call => call[0] === 'drop').map(call => call[1].spoolId), ['late']);
});

test('a first row beyond the 8MiB page budget reports failure, unlocks execution and still exports full spool', async () => {
  const { view, api, notices, calls, evidenceCalls, renderResults } = queryHarness(); view.renderResults = renderResults;
  const result = spoolResult('huge', [1]); result.sets[0].rows = []; result.sets[0].message = '初始预览超出字节预算，完整执行已缓存';
  api.executeSql = async () => result; await view.execute('SELECT huge_blob FROM t', false);
  await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(view.grid.data.rows.length, 0); assert.equal(view.pageButtons[0].disabled, false);
  assert.match(view.gridPreviewNoteEl.textContent, /完整执行已缓存/);
  api.fetchQueryRows = async () => { throw new Error('单行超过 8 MiB 分页预算，请使用完整缓存导出'); };
  await view.loadResultPage('first'); await readOnlyResultsSettled(view, evidenceCalls);
  assert.match(view.gridPreviewNoteEl.textContent, /8 MiB/); assert.match(view.gridPreviewNoteEl.textContent, /保留当前页，可重试/);
  assert.match(notices.at(-1)[1], /读取结果缓存失败/);
  assert.equal(view.resultBusy(), false); assert.equal(view.runBtn.disabled, false); assert.equal(view.explainBtn.disabled, false); assert.equal(view.connSelect.disabled, false);
  assert.equal(view.pageButtons[0].disabled, false); assert.equal(view.resultPager.attributes['aria-busy'], 'false');
  api.exportQuerySpooled = async (_opts, scope) => { calls.push(['export-huge', scope.spoolId]); return 1; };
  assert.equal(await view.exportResultSnapshot(exportOptions, view.captureResultExport()), 1);
  assert.deepEqual(calls.at(-1), ['export-huge', 'huge']); assert.equal(view.grid.data.rows.length, 0);
});

test('an expired spool page shows its error without replacing rows or leaving execution locked', async () => {
  const { view, api, evidenceCalls, renderResults } = queryHarness(); view.renderResults = renderResults;
  api.executeSql = async () => spoolResult('expired'); await view.execute('SELECT 1', false);
  await readOnlyResultsSettled(view, evidenceCalls);
  const rows = view.grid.data.rows; api.fetchQueryRows = async () => { throw new Error('查询缓存不存在或已经过期'); };
  await view.loadResultPage('next'); await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(view.grid.data.rows, rows); assert.match(view.gridPreviewNoteEl.textContent, /已经过期/);
  assert.equal(view.resultBusy(), false); assert.equal(view.runBtn.disabled, false); assert.equal(view.dbSelect.disabled, false);
  api.executeSql = async () => spoolResult('renewed'); await view.execute('SELECT 2', false);
  await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(view.tab.results.spool_id, 'renewed'); assert.doesNotMatch(view.gridPreviewNoteEl.textContent, /已经过期/);
});

test('真实查询连接切换后namespace标题与ARIA同步，事务门禁仍拒绝切库并采用当前语义', async () => {
  const { view, app, tab, transactions, notices, calls } = queryHarness();
  const connections = [{ id: 'a', name: 'PG', kind: 'postgresql', database: 'physical_pg' }, { id: 'b', name: 'MySQL', kind: 'mysql' }];
  app.connectionsValue = () => connections;
  view.syncManualTransactionControls();
  assert.equal(view.dbSelect.attributes['aria-label'], '查询模式'); assert.equal(view.dbSelect.title, '查询模式');
  await view.switchConnection('b');
  assert.equal(tab.connectionId, 'b'); assert.equal(view.dbSelect.attributes['aria-label'], '查询数据库'); assert.equal(view.dbSelect.title, '查询数据库');
  transactions.set('b', { owner: 'other', schema: 'main', active: true, failed: false });
  view.syncManualTransactionControls(); view.dbSelect.value = 'other'; view.dbSelect.fire('change');
  assert.equal(view.dbSelect.disabled, true); assert.match(notices.at(-1)[1], /再切换数据库/);
  assert.equal(view.dbSelect.value, tab.schema); assert.match(view.dbSelect.title, /事务绑定当前连接和数据库/);
  transactions.clear(); await view.switchConnection('a');
  assert.equal(view.dbSelect.attributes['aria-label'], '查询模式'); assert.equal(view.dbSelect.title, '查询模式');
  assert.equal(connections[0].database, 'physical_pg'); assert.equal(calls.filter(call => call[0] === 'execute').length, 0);
  view.destroy();
});

test('manual page entry validates boundaries and jumps via cached offsets without rerunning SQL', async () => {
  const { view, api, calls, evidenceCalls, renderResults } = queryHarness(); view.renderResults = renderResults;
  api.executeSql = async (_id, sql) => { calls.push(['execute', sql]); return spoolResult('numbered', [10001]); }; await view.execute('SELECT value FROM t', false);
  await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(view.pageNumberInput.value, '1'); assert.equal(view.footerStatusEl.textContent, '运行耗时 0.001s | 共 10001 行');
  assert.equal(view.resultPager.children[0], view.footerStatusEl);
  assert.equal(view.pageCountEl.textContent, '/ 51 页');
  assert.equal(view.pageControls.children[2].className, 'grid-pager-position');
  for (const value of ['', '0', '-1', '1.5', 'abc', '52', '1e1', '9007199254740993']) {
    view.pageNumberInput.value = value; await view.jumpResultPage();
    assert.equal(view.pageNumberInput.attributes['aria-invalid'], 'true'); assert.equal(view.pageErrorEl.hidden, false);
  }
  assert.equal(calls.filter(call => call[0] === 'fetch').length, 0);
  view.pageNumberInput.value = '51'; view.pageNumberInput.fire('keydown', { key: 'Enter' });
  await readOnlyResultsSettled(view, evidenceCalls); await new Promise(resolve => setImmediate(resolve));
  await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(view.resultPages.page(0).offset, 10000); assert.equal(view.pageNumberInput.value, '51');
  assert.equal(view.statusInfo().pageText, '共 10001 行');
  view.pageNumberInput.value = '2'; view.pageNumberInput.fire('blur');
  await new Promise(resolve => setImmediate(resolve)); await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(view.resultPages.page(0).offset, 200);
  view.pageNumberInput.value = '3'; view.pageNumberInput.fire('keydown', { key: 'Escape' });
  assert.equal(view.pageNumberInput.value, '2');
  assert.equal(calls.filter(call => call[0] === 'execute').length, 1);
  assert.deepEqual(calls.filter(call => call[0] === 'fetch').map(call => call[3]), [10000, 200]);
});

test('ordinary cached completion notices no longer appear above the grid; incomplete-cache warnings remain', async () => {
  for (const [message, visible] of [
    ['查询已完成，共 10001 行；当前返回前 10000 行的内存预览。查询页可分页读取本次执行缓存，导出默认包含全部行，不会重新执行 SQL。', false],
    ['查询已完成，共 10001 行；当前显示前 10000 行，分页与导出使用本次缓存。', false],
    ['查询已完成，共 10001 行；结果缓存不完整，可读取 300 行。', true],
  ]) {
    const { view, api, evidenceCalls, renderResults } = queryHarness(); view.renderResults = renderResults;
    const result = spoolResult(); result.sets[0].message = message; api.executeSql = async () => result;
    await view.execute('SELECT value FROM t', false); await readOnlyResultsSettled(view, evidenceCalls);
    assert.equal(view.gridPreviewNoteEl.style.display, visible ? 'block' : 'none');
    assert.equal(view.statusInfo().elapsed, '运行耗时 0.001s');
  }
});

test('page-size changes load cache atomically, reset navigation and stay isolated between result sets', async () => {
  const pages = new QueryResultPages(); pages.replace(spoolResult('sizes', [10001, 700]), spoolContext);
  const reads = [];
  const read = async (_scope, _index, offset, limit) => { reads.push([offset, limit]); return { rows: Array.from({ length: limit }, (_, i) => [String(offset + i)]), total: 10001, truncated: false, message: '' }; };
  await pages.fetch(0, 200, read);
  await pages.fetch(0, 0, read, 500);
  assert.equal(pages.pageLimit(0), 500); assert.equal(pages.pageCount(0), 21); assert.equal(pages.offset(0, 'next'), 500);
  assert.equal(pages.page(0).previous.length, 0); assert.equal(pages.pageLimit(1), 200);
  await assert.rejects(pages.fetch(0, 0, async () => { throw new Error('cache unavailable'); }, 1000));
  assert.equal(pages.pageLimit(0), 500); assert.equal(pages.page(0).rows.length, 500);
  await pages.fetch(0, 0, read, 1000); assert.equal(pages.pageLimit(0), 1000); assert.equal(pages.pageCount(0), 11);
  assert.equal(pages.pageOffset(0, 11), 10000); assert.deepEqual(reads, [[200, 200], [0, 500], [0, 1000]]);
});

test('query page-size selector has fixed choices and reloads cache without SQL, blocking busy/dirty changes', async () => {
  const { view, api, calls, evidenceCalls, renderResults } = queryHarness(); view.renderResults = renderResults;
  api.executeSql = async (_id, sql) => { calls.push(['execute', sql]); return spoolResult('sized', [10001]); };
  await view.execute('SELECT value FROM t', false); await readOnlyResultsSettled(view, evidenceCalls);
  assert.deepEqual(view.pageSizeSelect.children.map(option => option.value), ['200', '500', '1000']);
  view.pageSizeSelect.value = '500'; view.pageSizeSelect.fire('change');
  await new Promise(resolve => setImmediate(resolve)); await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(view.grid.data.rows.length, 500); assert.equal(view.pageNumberInput.value, '1'); assert.equal(view.pageSizeSelect.value, '500');
  await view.loadResultPage('next'); await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(view.resultPages.page(0).offset, 500);
  view.pageSizeSelect.value = '1000'; view.pageSizeSelect.fire('change');
  await new Promise(resolve => setImmediate(resolve)); await readOnlyResultsSettled(view, evidenceCalls);
  assert.equal(view.resultPages.page(0).offset, 0); assert.equal(view.grid.data.rows.length, 1000);
  const before = calls.filter(call => call[0] === 'fetch').length;
  view.running = true; view.renderResultPager(); assert.equal(view.pageSizeSelect.disabled, true);
  view.pageSizeSelect.value = '200'; view.pageSizeSelect.fire('change'); assert.equal(view.pageSizeSelect.value, '1000');
  view.running = false; view.controlledResult = { dirty: true }; view.renderResultPager(); assert.equal(view.pageSizeSelect.disabled, true);
  view.pageSizeSelect.value = '200'; view.pageSizeSelect.fire('change');
  assert.equal(calls.filter(call => call[0] === 'fetch').length, before); assert.equal(calls.filter(call => call[0] === 'execute').length, 1);
});

test('page numbers share fixed last-page offsets and reject invalid and empty-result page requests', () => {
  const pages = new QueryResultPages(200); pages.replace(spoolResult('pages', [201, 400, 0]), spoolContext);
  assert.equal(pages.pageCount(0), 2); assert.equal(pages.pageOffset(0, 2), 200);
  assert.equal(pages.offset(0, 'last'), 200); assert.equal(pages.offset(1, 'last'), 200);
  assert.equal(pages.pageNumber(2), 0); assert.equal(pages.pageCount(2), 0);
  for (const number of [0, -1, 1.5, 3, NaN, Infinity]) assert.equal(pages.pageOffset(0, number), null);
  assert.equal(pages.pageOffset(2, 1), null);
});

function queryClockFixture() {
  let now = 0, next = 0;
  const timers = new Map();
  const h = queryHarness({
    performance: { now: () => now },
    setInterval: (callback, ms) => { assert.equal(ms, 50); const id = ++next; timers.set(id, callback); return id; },
    clearInterval: id => timers.delete(id),
  });
  return { ...h, timers, advance(ms) { now += ms; for (const callback of [...timers.values()]) callback(); } };
}

test('rerun clears old grid, selection, preview and spool before capability response; live clocks stay synchronized', async () => {
  const h = queryClockFixture(), { view, api, tab, calls, timers, advance, renderResults, evidenceCalls } = h;
  view.renderResults = renderResults;
  api.executeSql = async () => spoolResult('previous');
  await view.execute('SELECT value FROM previous', false);
  await readOnlyResultsSettled(view, evidenceCalls);
  tab.selectedRows.add(0);
  const oldExport = view.captureResultExport();
  const capabilities = deferred(), outcome = deferred(); let sent;
  api.executionCapabilities = () => capabilities.promise;
  api.executeSqlV2 = request => { sent = request; return outcome.promise; };
  api.cancelExecution = async context => ({ execution_id: context.execution_id, accepted: true, state: 'cancel_requested', message: '' });
  const task = view.execute('SELECT value FROM next', false);
  assert.equal(tab.results, null);
  assert.equal(tab.activeResult, -1);
  assert.equal(tab.selectedRows.size, 0);
  assert.equal(view.grid.data.rows.length, 0);
  assert.equal(view.grid.data.columns.length, 0);
  assert.equal(view.cellPreviewEl.children.length, 0);
  assert.equal(view.executionLabelEl.textContent, '查询中');
  assert.equal(view.runbar.querySelector('.query-transaction-status'), null);
  assert.equal(view.resultLoadingLabelEl.textContent, '正在查询… ');
  assert.equal(view.resultLoadingTimerEl.textContent, '0.000 s');
  assert.equal(view.resultLoadingTimerEl.attributes['aria-hidden'], 'true');
  const centralTimer = view.resultLoadingTimerEl;
  assert.equal(timers.size, 1);
  assert.deepEqual(calls.filter(c => c[0] === 'drop').map(c => c[1].spoolId), ['previous']);
  await assert.rejects(view.exportResultSnapshot(exportOptions, oldExport), /已经替换/);
  advance(372);
  assert.equal(view.executionTimerEl.textContent, '0.372 s');
  assert.equal(view.resultLoadingTimerEl, centralTimer);
  assert.equal(view.resultLoadingTimerEl.textContent, '0.372 s');
  assert.equal(view.executionMessageTimerEl.textContent, '0.372 s');
  assert.equal(view.executionMessageTimerEl.attributes['aria-hidden'], 'true');
  assert.equal(view.lastMessages[0].text, '查询中… 0.372 s');
  capabilities.resolve({ cancel: { level: 'supported', reason: '' } });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(timers.size, 1, 'queued publication must not reset the clock');
  await view.execution.cancel();
  advance(128);
  assert.equal(view.executionMessageLabelEl.textContent, '停止中…');
  assert.equal(view.resultLoadingLabelEl.textContent, '正在停止查询… ');
  assert.equal(view.resultLoadingTimerEl.textContent, '0.500 s');
  assert.equal(view.executionMessageTimerEl.textContent, '0.500 s');
  outcome.resolve({ execution_id: sent.execution_id, state: 'cancelled', result: null, error: null });
  await task;
  assert.equal(timers.size, 0);
  assert.equal(view.resultLoadingEl.hidden, true);
  assert.equal(view.resultLoadingTimerEl.textContent, '');
  assert.equal(view.executionTimerEl.textContent, '0.500 s');
  advance(1000);
  assert.equal(view.executionTimerEl.textContent, '0.500 s');
  assert.equal(tab.results, null);
  view.destroy();
});

test('capability failure freezes the clock and reports failure rather than leaving 查询中 active', async () => {
  const { view, api, timers, advance } = queryClockFixture();
  const pending = deferred(); api.executionCapabilities = () => pending.promise;
  const task = view.execute('SELECT 1', false);
  advance(123);
  pending.reject(new Error('capability lookup failed'));
  await task;
  assert.equal(view.executionLabelEl.textContent, '执行失败');
  assert.equal(view.executionStatusEl.attributes['data-state'], 'failed');
  assert.equal(view.executionTimerEl.textContent, '0.123 s');
  assert.equal(view.executionClockStartedAt, null);
  assert.equal(timers.size, 0);
  assert.match(view.lastMessages[0].text, /capability lookup failed/);
  assert.equal(view.running, false);
  view.destroy();
});

test('completion log is not a live timer during transaction cleanup, and destroy cancels a pending clock', async () => {
  const h = queryClockFixture(), { view, api, app, timers, advance, renderResults } = h;
  view.renderResults = renderResults;
  const cleanup = deferred(); app.refreshManualTransactionStatus = () => cleanup.promise;
  const task = view.execute('SELECT 1', false);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(view.running, true, 'transaction confirmation still owns the busy gate');
  assert.equal(timers.size, 0);
  assert.equal(view.executionMessageTimerEl, null);
  assert.equal(view.messagesEl.children[0].className.includes('msg-running'), false);
  const record = view.lastMessages[0].text;
  advance(500);
  assert.equal(view.lastMessages[0].text, record);
  cleanup.resolve(); await task;
  view.destroy();
  const pending = deferred(), fresh = queryClockFixture();
  fresh.api.executionCapabilities = () => pending.promise;
  const running = fresh.view.execute('SELECT 2', false);
  assert.equal(fresh.timers.size, 1);
  fresh.view.destroy();
  assert.equal(fresh.timers.size, 0);
  pending.resolve({ cancel: { level: 'unsupported', reason: '' } });
  await running;
});

// 真 QueryTabView + 真 ExecutionController，不以源码正则代替分页行为测试。
function serverPaginationHarness({ keyset = true, size = 2, truncate = false } = {}) {
  const h = queryHarness();
  h.tab.schema = 'tenant_a';
  h.view.resultPages = new QueryResultPages(size);
  h.api.planQueryPagination = async (id, sql, limit, offset) => {
    h.calls.push(['plan-page', id, sql, limit, offset]);
    return { mode: 'page', sql: `PAGE ${offset} ${limit}`, reason: null };
  };
  h.api.planQueryKeyset = async (id, sql, ns) => {
    h.calls.push(['keyset-info', id, sql, ns]); return { supported: keyset, column: 'id' };
  };
  h.api.planQueryKeysetPage = async (id, sql, after, limit, ns) => {
    h.calls.push(['plan-keyset', id, sql, after, limit, ns]);
    return { mode: 'page', sql: `PAGE ${Number(after)} ${limit}`, reason: null };
  };
  h.api.executeSql = async (id, sql, ns) => {
    h.calls.push(['execute', id, sql, ns]);
    const [, start, limit] = /^PAGE (\d+) (\d+)$/.exec(sql) ?? ['', '0', String(size)];
    const rows = Array.from({ length: Number(limit) }, (_, i) => [String(Number(start) + i + 1)]);
    return { sets: [{ columns: [{ name: 'id', data_type: 'BIGINT' }], rows: truncate ? rows.slice(0, 1) : rows, total: rows.length, affected: 0, message: '' }], messages: [], elapsed_ms: 10, spool_id: `page-${start}` };
  };
  h.api.fetchQueryRows = async (scope, index, offset, limit) => {
    h.calls.push(['fetch', plain(scope), index, offset, limit]);
    const start = Number(scope.spoolId.slice(5));
    return { rows: Array.from({ length: limit }, (_, i) => [String(start + offset + i + 1)]), total: size, truncated: false, message: '' };
  };
  h.sql = 'SELECT id FROM items ORDER BY id';
  h.count = type => h.calls.filter(c => c[0] === type).length;
  return h;
}

const settleTick = () => new Promise(resolve => setImmediate(resolve));

test('server pagination initial query sends page SQL not original SQL and caches readonly without spool', async () => {
  const h = serverPaginationHarness(); await h.view.execute(h.sql, false);
  assert.equal(h.count('execute'), 1); assert.equal(h.calls.find(c => c[0] === 'execute')[2], 'PAGE 0 2');
  assert.equal(h.view.serverPageCache.size, 1); assert.equal(h.view.resultPages.scope, null);
  assert.equal(h.tab.results.spool_id, null); assert.equal(h.view.resultExecution, null);
  assert.equal(h.count('drop'), 1); h.view.destroy();
});

test('first next page uses initial last unique key and both Keyset APIs carry namespace', async () => {
  const h = serverPaginationHarness(); await h.view.execute(h.sql, false);
  assert.equal(h.view.serverPageAnchor, '2');
  await h.view.loadResultPage('next');
  const plan = h.calls.find(c => c[0] === 'plan-keyset');
  assert.equal(plan[3], '2'); assert.equal(plan[5], 'tenant_a');
  assert.equal(h.calls.find(c => c[0] === 'keyset-info')[3], 'tenant_a');
  assert.deepEqual(plain(h.tab.results.sets[0].rows), [['3'], ['4']]); h.view.destroy();
});

test('return to viewed page performs zero planning and zero database calls, no old scope or write panel', async () => {
  const h = serverPaginationHarness(); await h.view.execute(h.sql, false); await h.view.loadResultPage('next');
  const db = h.count('execute'), plans = h.count('plan-page') + h.count('plan-keyset');
  let disposed = 0; h.view.controlledResult = { dispose() { disposed++; } };
  h.view.resultExecution = { context: { execution_id: 'old', connection_id: 'a', owner_id: h.tab.id, namespace: h.tab.schema }, result: h.tab.results };
  await h.view.loadResultPage('previous');
  assert.equal(h.count('execute'), db); assert.equal(h.count('plan-page') + h.count('plan-keyset'), plans);
  assert.equal(disposed, 1); assert.equal(h.view.controlledResult, null); assert.equal(h.view.resultExecution, null);
  assert.equal(h.view.resultPages.scope, null); assert.deepEqual(plain(h.tab.results.sets[0].rows), [['1'], ['2']]); h.view.destroy();
});

test('eight-page cache evicts oldest, requery only evicted page, released scopes never reattach', async () => {
  const h = serverPaginationHarness(); await h.view.execute(h.sql, false);
  for (let i = 0; i < 8; i++) await h.view.loadResultPage('next');
  assert.equal(h.view.serverPageCache.size, 8); assert.equal(h.view.serverPageCache.has(0), false);
  assert.equal(h.count('drop'), 9);
  const db = h.count('execute'); await h.view.loadResultPage('first');
  assert.equal(h.count('execute'), db + 1); assert.equal(h.view.resultPages.scope, null); h.view.destroy();
});

test('new execution and clear/destroy invalidate all seen-page memory', async () => {
  const h = serverPaginationHarness(); await h.view.execute(h.sql, false); await h.view.loadResultPage('next');
  await h.view.execute(h.sql, false); assert.equal(h.view.serverPageCache.size, 1); assert.equal(h.view.serverPageCache.has(2), false);
  h.view.clearResultSnapshot(); assert.equal(h.view.serverPageCache.size, 0); assert.equal(h.view.serverPage, null);
  await h.view.execute(h.sql, false); h.view.destroy(); assert.equal(h.view.serverPageCache.size, 0); assert.equal(h.view.serverPage, null);
});

test('planning IPC rejection never silently executes original full query', async () => {
  const h = serverPaginationHarness(); h.api.planQueryPagination = async () => { throw new Error('planning IPC failed'); };
  await h.view.execute(h.sql, false); assert.equal(h.count('execute'), 0); assert.equal(h.view.running, false);
  assert.match(h.view.lastMessages[0].text, /planning IPC failed/); h.view.destroy();
});

test('explicit unsupported is permitted legacy execution and exposes reason', async () => {
  const h = serverPaginationHarness(); h.api.planQueryPagination = async () => ({ mode: 'unsupported', sql: null, reason: 'JOIN driver unsupported' });
  await h.view.execute(h.sql, false); assert.equal(h.calls.find(c => c[0] === 'execute')[2], h.sql);
  assert.equal(h.view.serverPage, null); assert.ok(h.notices.some(c => /JOIN driver unsupported/.test(c[1]))); h.view.destroy();
});

test('active owned manual transaction does not plan server pages and preserves original SQL/owner', async () => {
  const h = serverPaginationHarness(); h.transactions.set('a', { owner: h.tab.id, schema: h.tab.schema, active: true });
  let request; const execute = h.api.executeSqlV2; h.api.executeSqlV2 = r => { request = r; return execute(r); };
  await h.view.execute(h.sql, false); assert.equal(h.count('plan-page'), 0); assert.equal(h.view.serverPage, null);
  assert.equal(request.sql, h.sql); assert.equal(request.transaction_owner, h.tab.id);
  assert.ok(h.notices.some(c => /手动事务不使用服务器分页/.test(c[1]))); h.view.destroy();
});

test('double-click next while planning holds busy and sends only one database query', async () => {
  const h = serverPaginationHarness({ keyset: false }); await h.view.execute(h.sql, false);
  const gate = deferred(), planner = h.api.planQueryPagination;
  h.api.planQueryPagination = async (...args) => { h.calls.push(['pending-plan']); await gate.promise; return planner(...args); };
  const first = h.view.loadResultPage('next'); const second = h.view.loadResultPage('next');
  assert.equal(h.view.running, true); assert.equal(h.count('pending-plan'), 1); assert.equal(h.count('execute'), 1);
  gate.resolve(); await Promise.all([first, second]); assert.equal(h.count('execute'), 2); h.view.destroy();
});

test('stopping initial planning before reply sends no SQL and clears busy', async () => {
  const h = serverPaginationHarness(), gate = deferred(); h.api.planQueryPagination = () => gate.promise;
  const task = h.view.execute(h.sql, false); await settleTick(); h.view.stopBtn.fire('click');
  gate.resolve({ mode: 'page', sql: 'PAGE 0 2' }); await task;
  assert.equal(h.count('execute'), 0); assert.equal(h.view.running, false); assert.match(h.view.lastMessages[0].text, /没有发送 SQL/); h.view.destroy();
});

test('stopping next-page planning rejects late reply without database cancellation/requery', async () => {
  const h = serverPaginationHarness({ keyset: false }); await h.view.execute(h.sql, false);
  const gate = deferred(); h.api.planQueryPagination = () => gate.promise;
  const task = h.view.loadResultPage('next'); h.view.stopBtn.fire('click');
  gate.resolve({ mode: 'page', sql: 'PAGE 2 2' }); await task;
  assert.equal(h.count('execute'), 1); assert.equal(h.view.running, false); h.view.destroy();
});

for (const change of ['close', 'connection', 'namespace', 'revision', 'serverPage']) {
  test(`late next-page planning after ${change} never executes or overwrites cached page`, async () => {
    const h = serverPaginationHarness({ keyset: false }); await h.view.execute(h.sql, false);
    const gate = deferred(); h.api.planQueryPagination = () => gate.promise;
    const task = h.view.loadResultPage('next');
    if (change === 'close') h.view.destroy();
    if (change === 'connection') h.tab.connectionId = 'b';
    if (change === 'namespace') h.tab.schema = 'tenant_b';
    if (change === 'revision') h.view.executionRevision++;
    if (change === 'serverPage') h.view.serverPage = { ...h.view.serverPage };
    gate.resolve({ mode: 'page', sql: 'PAGE 2 2' }); await task;
    assert.equal(h.count('execute'), 1); assert.deepEqual(plain(h.tab.results.sets[0].rows), [['1'], ['2']]);
    if (change !== 'close') h.view.destroy();
  });
}

test('unknown page outcome prevents cache return and any subsequent pagination reexecution', async () => {
  const h = serverPaginationHarness(); await h.view.execute(h.sql, false);
  h.api.executeSqlV2 = async r => ({ execution_id: r.execution_id, state: 'unknown', result: null, error: { message: 'unknown effects' } });
  await h.view.loadResultPage('next'); assert.equal(h.view.executionNeedsReview, true);
  const plans = h.count('plan-keyset'); await h.view.loadResultPage('previous'); await h.view.loadResultPage('next');
  assert.equal(h.count('plan-keyset'), plans); assert.ok(h.notices.some(c => /结果未知/.test(c[1]))); h.view.destroy();
});

test('byte-truncated initial preview hydrates same page from spool then anchors exact last row', async () => {
  const h = serverPaginationHarness({ truncate: true }); await h.view.execute(h.sql, false);
  assert.equal(h.count('fetch'), 1); assert.deepEqual(plain(h.tab.results.sets[0].rows), [['1'], ['2']]);
  assert.equal(h.view.serverPageAnchor, '2'); await h.view.loadResultPage('next');
  assert.equal(h.calls.find(c => c[0] === 'plan-keyset')[3], '2'); assert.equal(h.view.resultPages.scope, null); h.view.destroy();
});

test('unreadable byte-truncated remainder explicitly errors rather than marking tail or skipping rows', async () => {
  const h = serverPaginationHarness({ truncate: true }); h.api.fetchQueryRows = async () => ({ rows: [], total: 2, truncated: true, message: 'large row' });
  await h.view.execute(h.sql, false); assert.equal(h.view.serverPageCache.size, 0); assert.equal(h.tab.results, null);
  assert.match(h.view.lastMessages[0].text, /不会跳到下一页遗漏数据/); assert.equal(h.count('drop'), 1); h.view.destroy();
});

test('server page footer is current-page count not query total and export requeries original SQL', async () => {
  const h = serverPaginationHarness(); await h.view.execute(h.sql, false); h.renderResults();
  assert.match(h.view.statusInfo().pageText, /本页 2 行.*总行数未知/);
  assert.match(h.view.footerStatusEl.textContent, /运行耗时.*服务器分页/);
  assert.match(h.view.pageCountEl.textContent, /总页数未知/);
  await h.view.exportResultSnapshot({ format: 'csv' }, h.view.captureResultExport());
  assert.equal(h.count('export-requery'), 1); assert.equal(h.calls.find(c => c[0] === 'export-requery')[2], h.sql);
  assert.equal(h.count('export-spool'), 0); h.view.destroy();
});
