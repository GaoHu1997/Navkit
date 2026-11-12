import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { transactionFixtureBindings } from './transaction-fixture-bindings.mjs';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
function moduleFile(file, imports = {}, globals = {}) {
  const context = vm.createContext({ exports: {}, TextEncoder, ...globals, require: name => {
    if (!(name in imports)) throw new Error(`Missing test import ${name}`); return imports[name];
  } });
  vm.runInContext(ts.transpile(fs.readFileSync(path.join(root, file), 'utf8'), { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS }), context);
  return context.exports;
}
const catalog = moduleFile('src/core/dbcatalog.ts');
const builder = moduleFile('src/core/queryBuilder.ts', { './dbcatalog': catalog });
const { buildSelectQuery, defaultQueryDialect, queryBuilderUnavailableReason } = builder;
const structure = table => ({ table, database: 'main', columns: ['id', 'name', 'display name'].map(name => ({ name, type: 'TEXT' })) });
const spec = (dialect = 'sqlite', extras = {}) => ({ dialect, schema: 'main', table: 'users', columns: ['id', 'name'], filters: [], orderBy: 'id', descending: true, limit: 25, ...extras });

test('major SQL dialects build qualified SELECT, sort and driver-specific row limits', () => {
  const expected = {
    mysql: ['SELECT `id`, `name`', 'FROM `main`.`users`', 'LIMIT 25'],
    postgres: ['SELECT "id", "name"', 'FROM "main"."users"', 'LIMIT 25'],
    sqlite: ['SELECT "id", "name"', 'FROM "main"."users"', 'LIMIT 25'],
    mssql: ['SELECT TOP 25 [id], [name]', 'FROM [main].[users]', 'ORDER BY [id] DESC'],
    oracle: ['SELECT "id", "name"', 'FROM "main"."users"', 'FETCH FIRST 25 ROWS ONLY'],
    db2: ['SELECT "id", "name"', 'FROM "main"."users"', 'FETCH FIRST 25 ROWS ONLY'],
    firebird: ['SELECT FIRST 25 "id", "name"', 'FROM "users"', 'ORDER BY "id" DESC'],
    access: ['SELECT TOP 25 [id], [name]', 'FROM [users]', 'ORDER BY [id] DESC'],
    clickhouse: ['SELECT `id`, `name`', 'FROM `main`.`users`', 'LIMIT 25'],
    standard: ['SELECT "id", "name"', 'FROM "main"."users"', 'LIMIT 25'],
    ansi: ['SELECT "id", "name"', 'FROM "main"."users"', 'FETCH FIRST 25 ROWS ONLY'],
  };
  for (const [dialect, parts] of Object.entries(expected)) {
    const sql = buildSelectQuery(structure('users'), spec(dialect));
    for (const part of parts) assert.ok(sql.includes(part), `${dialect}: ${sql}`);
    assert.ok(sql.endsWith(';'));
  }
});
test('metadata identifiers are quoted as whole names and escaped, never raw SQL', () => {
  const table = 'strange.table"; DROP TABLE x;--';
  const info = { table, columns: [{ name: 'f"x' }, { name: 'f]x' }, { name: 'f`x' }] };
  const sql = buildSelectQuery(info, spec('postgres', { table, schema: 's"x', columns: ['f"x'], orderBy: '' }));
  assert.ok(sql.includes('FROM "s""x"."strange.table""; DROP TABLE x;--"'));
  assert.ok(sql.startsWith('SELECT "f""x"'));
  assert.ok(buildSelectQuery(info, spec('mssql', { table, columns: ['f]x'], orderBy: '' })).includes('[f]]x]'));
  assert.ok(buildSelectQuery(info, spec('mysql', { table, columns: ['f`x'], orderBy: '' })).includes('`f``x`'));
});
test('text, numeric, boolean and NULL filters are safe AND expressions', () => {
  const filters = [
    { column: 'name', operator: '=', value: "a\\'; DROP TABLE x;--中文", valueKind: 'text' },
    { column: 'id', operator: '>=', value: '1.25e2', valueKind: 'number' },
    { column: 'id', operator: '<>', value: 'false', valueKind: 'boolean' },
    { column: 'display name', operator: 'IS NULL', value: 'ignored', valueKind: 'text' },
  ];
  const sqlite = buildSelectQuery(structure('users'), spec('sqlite', { filters }));
  assert.ok(sqlite.includes("'a\\''; DROP TABLE x;--中文'"));
  assert.ok(sqlite.includes('AND "id" >= 1.25e2'));
  assert.ok(sqlite.includes('AND "id" <> 0'));
  assert.ok(sqlite.includes('"display name" IS NULL'));
  const mysql = buildSelectQuery(structure('users'), spec('mysql', { filters }));
  assert.match(mysql, /CONVERT\(X'[a-f0-9]+' USING utf8mb4\)/);
  assert.ok(!mysql.includes('DROP TABLE'));
  assert.ok(buildSelectQuery(structure('users'), spec('postgres', { filters })).includes("E'a\\\\''; DROP TABLE"));
  assert.throws(() => buildSelectQuery(structure('users'), spec('standard', { filters })), /反斜杠/);
});
test('builder rejects forged fields, SQL operators, numeric injection and invalid limits', () => {
  for (const extras of [{ columns: [] }, { columns: ['missing'] }, { columns: ['id', 'id'] }, { orderBy: 'id;DROP' }, { table: 'other' }, { dialect: 'bad' }, { limit: 0 }, { limit: 1.2 }, { limit: NaN }, { limit: 100001 }]) assert.throws(() => buildSelectQuery(structure('users'), spec('sqlite', extras)));
  for (const [operator, value, valueKind] of [['=', '0; DROP TABLE x', 'number'], ['OR 1=1', '1', 'number'], ['=', 'truthy', 'boolean'], ['LIKE', '1', 'number'], ['=', '\0', 'text']]) {
    assert.throws(() => buildSelectQuery(structure('users'), spec('sqlite', { filters: [{ column: 'id', operator, value, valueKind }] })));
  }
});
test('SQL protocol forks and supported driver kinds are enabled; non-SQL has explicit reason', () => {
  // 当前目录已移除通用 odbc/jdbcx 类型；用仍在目录中的真实 ODBC 数据源覆盖驱动路径。
  for (const kind of ['mysql', 'tidb', 'custom_mysql', 'postgresql', 'opengauss', 'sqlite', 'sqlserver', 'oracle', 'db2', 'duckdb', 'h2', 'rqlite', 'd1']) {
    assert.ok(catalog.DB_KINDS.some(item => item.id === kind), '测试样例必须在真实目录中：' + kind);
    assert.equal(queryBuilderUnavailableReason(kind), null, kind);
  }
  for (const kind of ['redis', 'mongodb', 'neo4j', 'kafka', 'unknown', 'odbc', 'jdbcx']) assert.ok(queryBuilderUnavailableReason(kind), kind);
  assert.equal(defaultQueryDialect('postgresql'), 'postgres'); assert.equal(defaultQueryDialect('tidb'), 'mysql');
  assert.equal(defaultQueryDialect('odbc'), ''); assert.equal(defaultQueryDialect('h2'), 'standard');
});

class Element {
  constructor(tag = 'div') { this.tagName = tag; this.children = []; this.style = { removeProperty(name) { delete this[name]; } }; this.attributes = {}; this.listeners = {}; this.disabled = false; this.classList = { add: () => {} }; }
  appendChild(child) { child.parent = this; this.children.push(child); return child; }
  append(...items) { items.forEach(i => this.appendChild(i)); }
  replaceChildren(...items) { this.children = []; if (this.tagName === 'select') this._value = undefined; this.append(...items); }
  get options() { return this.children; }
  get value() { return this._value ?? (this.tagName === 'select' ? (this.children.find(c => c.selected) ?? this.children[0])?.value ?? '' : ''); }
  set value(value) { this._value = String(value); }
  get valueAsNumber() { return Number(this.value); }
  querySelector(selector) {
    const attr = /^\[([^=]+)="([^"]+)"\]$/.exec(selector);
    return flatten(this).slice(1).find(child => attr ? child.attributes[attr[1]] === attr[2] : selector.startsWith('.') && String(child.className ?? '').split(' ').includes(selector.slice(1))) ?? null;
  }
  replaceWith(node) { const at = this.parent.children.indexOf(this); this.parent.children[at] = node; node.parent = this.parent; }
  setAttribute(key, value) { this.attributes[key] = value; }
  removeAttribute(key) { delete this.attributes[key]; }
  addEventListener(key, fn) { (this.listeners[key] ??= []).push(fn); }
  fire(key, extra = {}) { for (const fn of this.listeners[key] ?? []) fn({ target: this, preventDefault() {}, stopPropagation() {}, ...extra }); }
  remove() { this.parent.children = this.parent.children.filter(e => e !== this); }
  focus() { this.focused = true; }
  checkValidity() { return this.tagName !== 'input' || this.type !== 'number' || (this.valueAsNumber >= Number(this.min) && this.valueAsNumber <= Number(this.max) && Number.isInteger(this.valueAsNumber)); }
}
const flatten = el => [el, ...el.children.flatMap(flatten)];
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const settle = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
function dialogHarness(overrides = {}, connection = { id: 'c1', name: 'Real target', kind: 'sqlite', params: '' }, namespace = 'main') {
  const calls = [], inserted = [];
  const source = {
    ensureConnection: async id => { calls.push(['ensure', id]); return { ok: true }; },
    listTables: async (id, schema) => { calls.push(['tables', id, schema]); return [['users', 'table'], ['v_users', 'view'], ['ignored', 'procedure']]; },
    getTableSchema: async (id, schema, table) => { calls.push(['fields', id, schema, table]); return structure(table); }, ...overrides,
  };
  class Dialog { constructor(opts) { this.el = new Element(); this.body = new Element(); this.footer = new Element(); this.el.append(this.body, this.footer); this.callbacks = []; opts.render(this.body); } onClose(fn) { this.callbacks.push(fn); } close() { this.callbacks.forEach(fn => fn()); this.closed = true; } }
  const ui = {
    Dialog, field: (label, control) => { const el = new Element(); el.append(control); return el; },
    numberInput: n => { const el = new Element('input'); el.type = 'number'; el.value = n; return el; },
    textInput: value => { const el = new Element('input'); el.value = value; return el; },
    select: (items, value) => { const el = new Element('select'); for (const item of items) { const option = new Element('option'); option.value = item.value; option.textContent = item.label; option.selected = value === item.value; el.append(option); } return el; },
  };
  const tools = { accessibleToolDialog: (dlg, title, onClose) => { dlg.el.setAttribute('role', 'dialog'); dlg.onClose(onClose); }, toolButton: label => { const el = new Element('button'); el.textContent = label; return el; } };
  const document = { activeElement: null, createElement: tag => new Element(tag) };
  const filterControls = moduleFile('src/ui/TableFilterControls.ts', {}, { document });
  const { openQueryBuilderDialog } = moduleFile('src/dialogs/QueryBuilderDialog.ts', { '../core/api': { api: source, parseDriverParams: () => null }, '../core/queryBuilder': builder, '../core/namespace-label': transactionFixtureBindings, '../ui/Dialog': ui, './toolDialog': tools, '../ui/TableFilterControls': filterControls, '../ui/data-grid-controls.css': {} }, {
    HTMLElement: Element, document, Option: class extends Element { constructor(text, value) { super('option'); this.textContent = text; this.value = value; } },
  });
  const dlg = openQueryBuilderDialog(connection, namespace, sql => inserted.push(sql), source);
  const button = text => flatten(dlg.el).find(e => e.tagName === 'button' && e.textContent === text);
  const control = name => flatten(dlg.el).find(e => e.attributes['aria-label'] === name);
  return { dlg, calls, inserted, button, control };
}
test('dialog loads real table/view metadata and auto-previews SELECT; insert never executes', async () => {
  const h = dialogHarness(); await settle();
  assert.deepEqual(h.calls, [['ensure', 'c1'], ['tables', 'c1', 'main'], ['fields', 'c1', 'main', 'users']]);
  assert.deepEqual(h.control('查询表或视图').options.map(o => o.value), ['users', 'v_users']);
  // 新设计没有「生成」按钮：字段加载完成后任何输入变化都即时生成预览。
  assert.match(h.control('待插入 SQL 预览').value, /^SELECT/);
  assert.equal(h.button('插入查询编辑器').disabled, false);
  assert.equal(h.inserted.length, 0); h.button('插入查询编辑器').fire('click');
  assert.equal(h.inserted.length, 1); assert.equal(h.dlg.closed, true);
});
test('changing settings invalidates the insert; invalid limit keeps last valid preview', async () => {
  const h = dialogHarness(); await settle();
  const valid = h.control('待插入 SQL 预览').value;
  const limit = h.control('最大查询行数'); limit.value = '0'; limit.fire('input');
  assert.equal(h.button('插入查询编辑器').disabled, true);
  // 失败时保留上次有效预览并在状态行提示，不发布伪造 SQL。
  assert.equal(h.control('待插入 SQL 预览').value, valid);
  assert.ok(flatten(h.dlg.el).some(e => /行数|limit/i.test(e.textContent ?? '')));
});
test('late old-table metadata cannot replace newly selected table; close blocks publication', async () => {
  const old = deferred(); const h = dialogHarness({ getTableSchema: async (id, schema, name) => name === 'users' ? old.promise : structure(name) });
  await settle(); const table = h.control('查询表或视图'); table.value = 'v_users'; table.fire('change'); await settle();
  old.resolve(structure('users')); await settle();
  assert.ok(h.control('待插入 SQL 预览').value.includes('FROM "main"."v_users"'));
  const pending = deferred(); const second = dialogHarness({ getTableSchema: () => pending.promise }); await settle();
  second.dlg.close(); pending.resolve(structure('users')); await settle(); assert.equal(second.button('插入查询编辑器').disabled, true);
});
test('metadata failures expose an actual retry and never publish fake fields', async () => {
  let attempts = 0; const h = dialogHarness({ getTableSchema: async (id, schema, name) => { if (++attempts === 1) throw new Error('metadata denied'); return structure(name); } });
  await settle(); assert.equal(h.button('插入查询编辑器').disabled, true);
  assert.ok(flatten(h.dlg.el).some(e => e.textContent?.includes('metadata denied')));
  h.button('重新加载元数据').fire('click'); await settle(); assert.equal(attempts, 2); assert.equal(h.button('插入查询编辑器').disabled, false);
});
test('SQLite attached scope stays qualified and Access Yes/No uses -1', () => {
  const sql = buildSelectQuery(structure('users'), spec('sqlite', { schema: 'aux' }));
  assert.ok(sql.includes('FROM "aux"."users"'));
  const access = buildSelectQuery(structure('users'), spec('access', { filters: [{ column: 'id', operator: '=', value: 'true', valueKind: 'boolean' }] }));
  assert.ok(access.includes('[id] = -1'));
});
test('deleted filter controls never cause duplicate accessible names when adding again', async () => {
  const h = dialogHarness(); await settle();
  h.button('＋ 添加条件').fire('click'); h.button('＋ 添加条件').fire('click');
  h.control('删除条件 1').fire('click'); h.button('＋ 添加条件').fire('click');
  assert.ok(h.control('条件 2 字段')); assert.ok(h.control('条件 3 字段')); assert.equal(h.control('条件 1 字段'), undefined);
  const labels = flatten(h.dlg.el).map(e => e.attributes['aria-label']).filter(label => label?.startsWith('条件 '));
  assert.equal(labels.length, new Set(labels).size);
});
test('real App command opens current-query builder and preserves nonselected text without executing', async () => {
  const text = fs.readFileSync(path.join(root, 'src/core/App.ts'), 'utf8');
  const source = ts.createSourceFile('App.ts', text, ts.ScriptTarget.ES2021, true, ts.ScriptKind.TS);
  const app = source.statements.find(s => ts.isClassDeclaration(s) && s.name?.text === 'App');
  const methods = ['commandUnavailableReason', 'runCommand', 'cmdQueryBuilder', 'guardedTabToolbarItems', 'manualTransactionFor', 'manualTransactionIsBusy', 'notifyToolbar', 'syncObjectSearchInput'].map(name => app.members.find(m => m.name?.getText(source) === name).getText(source)).join('\n');
  class QueryTabView { isExecuting() { return false; } resultBusy() { return this.isExecuting(); } constructor() { this.text = 'BEFORE SELECT 1; AFTER'; this.from = 7; this.to = 16; this.editor = { replaceSelection: sql => { this.text = this.text.slice(0, this.from) + sql + this.text.slice(this.to); }, focus: () => {} }; } }
  let insertedCallback, opened;
  const context = vm.createContext({ exports: {}, ...transactionFixtureBindings, QueryTabView, DataTabView: class {}, ObjectsTabView: class {}, queryBuilderUnavailableReason, queueMicrotask, openQueryBuilderDialog: (conn, schema, callback) => { opened = [conn.id, schema]; insertedCallback = callback; } });
  vm.runInContext(ts.transpile(`exports.AppHarness = class { ${methods} }`, { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS }), context);
  const controller = new context.exports.AppHarness(), view = new QueryTabView(), connection = { id: 'c1', kind: 'sqlite' };
  controller.activeTab = { id: 'q1', kind: 'query', connectionId: 'c1', schema: 'main' }; controller.views = new Map([['q1', view]]);
  controller.connectionById = id => id === 'c1' ? connection : undefined; controller.toast = () => {}; controller.tree = undefined;
  const toolbarEl = new Element(), stableSlot = new Element(); toolbarEl.append(stableSlot);
  controller.tabToolbar = { el: toolbarEl, stableSlot, updates: [], setItems(items) { this.updates.push(items); } }; controller.tabToolbarItems = () => []; controller.renderStatus = () => {};
  controller.menu = { renderBar: () => {} }; controller.contextMenus = () => []; controller.viewMenu = () => [];
  await controller.runCommand('query-builder'); assert.deepEqual(opened, ['c1', 'main']);
  assert.equal(controller.tabToolbar.updates.length, 1, 'the real command finally refreshes toolbar guards and synchronizes the search DOM');
  assert.equal(controller.objectSearchInput, null);
  insertedCallback('SELECT "id" FROM "users";'); assert.ok(view.text.startsWith('BEFORE ')); assert.ok(view.text.endsWith(' AFTER'));
  controller.activeTab = { id: 'q2' }; assert.throws(() => insertedCallback('SELECT 2;'), /目标查询页/);
  assert.ok(!text.includes('{ id: "query-builder", label: "查询创建工具", icon: "query_builder", disabled: true }'));
  controller.activeTab = { id: 'q1', kind: 'query', connectionId: 'c1', schema: 'main' };
  let status = { active: true, owner: 'other-query', schema: 'main', failed: false };
  controller.manualTransactionController = { status: id => id === 'c1' ? status : undefined, busy: () => false };
  assert.match(controller.commandUnavailableReason('run'), /其它页面持有/);
  assert.match(controller.commandUnavailableReason('data-generation'), /手动事务/);
  assert.equal(controller.commandUnavailableReason('query-builder'), null, 'building SELECT must not execute an automatic write');
  status = { ...status, owner: 'q1' };
  assert.match(controller.commandUnavailableReason('explain'), /不能解释/);
  assert.equal(status.failed, false, 'refusing EXPLAIN must not poison the active transaction');
  controller.tabToolbarItems = () => [{ id: 'grid-commit' }, { id: 'query-builder' }];
  const guarded = controller.guardedTabToolbarItems(controller.activeTab);
  assert.equal(guarded[0].disabled, true); assert.match(guarded[0].disabledReason, /手动事务/);
  assert.notEqual(guarded[1].disabled, true);
  status = { ...status, owner: 'q1', failed: true };
  assert.match(controller.commandUnavailableReason('run-selected'), /必须回滚/);
  assert.match(controller.commandUnavailableReason('commit-transaction'), /不能提交/);
  assert.equal(controller.commandUnavailableReason('rollback-transaction'), null);
});

test('查询创建工具按实际连接显示未选命名空间与空目录原因，不发起隐式执行', async () => {
  for (const [kind, title] of [['mysql', '数据库'], ['postgresql', '模式']]) {
    const connection = { id: 'c1', name: 'Target', kind, params: '', database: 'physical_database' };
    const missing = dialogHarness({}, connection, ''); await settle();
    assert(flatten(missing.dlg.el).some(element => element.className === 'qb-chip' && element.textContent === `Target / 未选择${title}`));
    assert.match(flatten(missing.dlg.el).find(element => String(element.className ?? '').split(/\s+/).includes('qb-status')).textContent, new RegExp(`选择${title}`));
    assert.deepEqual(missing.calls, []); assert.deepEqual(missing.inserted, []);
    const empty = dialogHarness({ listTables: async () => [] }, connection, 'scope'); await settle();
    assert.match(flatten(empty.dlg.el).find(element => String(element.className ?? '').split(/\s+/).includes('qb-status')).textContent, new RegExp(`当前${title}没有可查询`));
    assert.equal(empty.button('插入查询编辑器').disabled, true);
    assert.equal(connection.database, 'physical_database');
    missing.dlg.close(); empty.dlg.close();
  }
});

test('括号组 SQL 与表筛选一致，组间/组内连接都校验白名单', () => {
  const filter = (column, value, join = 'AND') => ({ column, value, join, operator: '=', valueKind: 'number' });
  const groups = [{ filters: [filter('id', '1')] }, { join: 'AND', filters: [filter('name', '2'), filter('display name', '3', 'OR')] }, { join: 'AND', filters: [filter('id', '4')] }];
  const sql = buildSelectQuery(structure('users'), spec('sqlite', { filters: [], filterGroups: groups }));
  assert.match(sql, /WHERE \("id" = 1\)\n  AND \("name" = 2 OR "display name" = 3\)\n  AND \("id" = 4\)/);
  for (const filterGroups of [[{ join: 'OR 1=1--', filters: [filter('id', '1')] }], [{ filters: [filter('id', '1', 'AND 1=1--')] }], [{ filters: [] }], [{ filters: Array.from({ length: 21 }, () => filter('id', '1')) }]]) assert.throws(() => buildSelectQuery(structure('users'), spec('sqlite', { filterGroups })));
  assert.throws(() => buildSelectQuery(structure('users'), spec('sqlite', { filters: [filter('id', '1'), filter('id', '2', 'UNION SELECT')] })));
  assert.match(buildSelectQuery(structure('users'), spec('sqlite', { filters: [{ column: 'name', operator: 'NOT LIKE', value: '%test%', valueKind: 'text' }] })), /"name" NOT LIKE '%test%'/);
});

test('查询工具值点击编辑：Enter/失焦确认、Escape/IME取消，括号组与NULL可组合', async () => {
  const h = dialogHarness(); await settle();
  h.button('＋ 添加条件').fire('click'); assert.equal(h.button('插入查询编辑器').disabled, true);
  assert.equal(h.control('条件 1 值').tagName, 'button'); h.control('条件 1 值').fire('click');
  const input = h.control('条件 1 值'); input.value = '中文候选'; input.fire('input'); input.fire('keydown', { key: 'Enter', isComposing: true });
  assert.equal(h.control('条件 1 值'), input); input.fire('keydown', { key: 'Escape' });
  assert.equal(h.control('条件 1 值').textContent, '<?>'); assert.equal(h.button('插入查询编辑器').disabled, true);
  h.control('条件 1 值').fire('click'); const accepted = h.control('条件 1 值'); accepted.value = "O'Reilly"; accepted.fire('input'); accepted.fire('keydown', { key: 'Enter' });
  assert.equal(h.control('条件 1 值').tagName, 'button'); assert.equal(h.button('插入查询编辑器').disabled, false); assert.match(h.control('待插入 SQL 预览').value, /O''Reilly/);
  h.control('在条件组 1 内追加条件').fire('click'); assert.equal(h.button('插入查询编辑器').disabled, true);
  const nullOp = h.control('条件 2 运算符'); nullOp.value = 'IS NULL'; nullOp.fire('change');
  assert.equal(h.control('条件 2 值').style.display, 'none'); assert.match(h.control('待插入 SQL 预览').value, / OR "id" IS NULL/);
  h.control('条件 2 与上一条件的连接方式，当前 OR').fire('click'); assert.match(h.control('待插入 SQL 预览').value, / AND "id" IS NULL/);
  h.control('在条件组 1 后增加下一行').fire('click'); h.control('条件 3 值').fire('click'); const blur = h.control('条件 3 值'); blur.value = 'value'; blur.fire('input'); blur.fire('blur');
  assert.match(h.control('待插入 SQL 预览').value, /\n  AND \("id" = 'value'\)/); assert.equal(h.inserted.length, 0);
  const style = flatten(h.dlg.el).find(el => el.tagName === 'style').textContent; assert.match(style, /qb-field-grid[^}]*grid-template-columns:minmax\(0, 1fr\)/);
});
