import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { transactionFixtureBindings } from './transaction-fixture-bindings.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const read = file => fs.readFileSync(path.join(root, file), 'utf8');
const source = read('src/core/App.ts');
const definitions = [
  '`id` bigint unsigned NOT NULL AUTO_INCREMENT',
  "`title` enum('a,b','c)') CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT 'a,b' COMMENT '引号,括号)'",
  '`computed` int GENERATED ALWAYS AS ((`id` + length(\'x,y\'))) STORED',
  '`updated` timestamp(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3)',
];
const ddl = (order = ['id', 'title', 'computed', 'updated']) => 'CREATE TABLE `items` (\n' + order.map(name => definitions.find(d => d.startsWith('`' + name + '`'))).join(',\n') + ',\nPRIMARY KEY (`title`,`id`),\nKEY `title_idx` (`title`),\nCONSTRAINT `items_check` CHECK ((`id` > 0))\n) ENGINE=InnoDB';
const json = value => JSON.parse(JSON.stringify(value));

function harness(kind = 'mysql') {
  const doc = { activeElement: null, createElement: tag => new Element(tag) };
  class Element {
    constructor(tag = 'div') { this.tagName = tag; this.children = []; this.style = {}; this.listeners = {}; this.className = ''; this.attributes = {}; this.classList = { add() {}, remove() {} }; }
    set innerHTML(value) { this.children = []; this.html = value; }
    get innerHTML() { return this.html ?? ''; }
    appendChild(child) { child.parentElement = this; this.children.push(child); return child; }
    addEventListener(name, fn) { (this.listeners[name] ??= []).push(fn); }
    setAttribute(name, value) { this.attributes[name] = value; }
    contains(child) { return child === this || this.children.some(c => c.contains(child)); }
    matches(selector) { if (selector.startsWith('.')) return this.className.split(' ').includes(selector.slice(1)); const m = /^(\w+)(?:\[type=(\w+)\])?$/.exec(selector); return !!m && this.tagName === m[1] && (!m[2] || this.type === m[2]); }
    querySelectorAll(selector) {
      return selector.split(',').flatMap(part => {
        const selectors = part.trim().split(/\s+/), last = selectors.pop();
        const all = this.children.flatMap(c => [c, ...c.querySelectorAll('*')]);
        if (last === '*') return all;
        return all.filter(node => {
          if (!node.matches(last)) return false;
          let parent = node.parentElement;
          for (let i = selectors.length - 1; i >= 0; i--) { while (parent && !parent.matches(selectors[i])) parent = parent.parentElement; if (!parent) return false; parent = parent.parentElement; }
          return true;
        });
      });
    }
    querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
    fire(name, bubble = false) { for (const fn of this.listeners[name] ?? []) fn({ target: this }); if (bubble) this.parentElement?.fire(name, true); }
    blur() { if (doc.activeElement === this) { doc.activeElement = null; this.fire('blur'); } }
    focus() { doc.activeElement?.blur(); doc.activeElement = this; this.fire('focus'); this.fire('focusin', true); }
  }
  const state = { order: ['id', 'title', 'computed', 'updated'], sql: [], notices: [], toolbarUpdates: 0, ddlOverride: null };
  const columns = [
    { name: 'id', type: 'bigint unsigned', length: null, scale: null, nullable: false, primary_key: true, auto_increment: true, default: null, comment: null },
    { name: 'title', type: "enum('a,b','c)')", length: null, scale: null, nullable: false, primary_key: true, auto_increment: false, default: "'a,b'", comment: '引号,括号)' },
    { name: 'computed', type: 'int', length: null, scale: null, nullable: true, primary_key: false, auto_increment: false, default: null, comment: null },
    { name: 'updated', type: 'timestamp(3)', length: 3, scale: null, nullable: false, primary_key: false, auto_increment: false, default: 'CURRENT_TIMESTAMP(3)', comment: null },
  ];
  const schema = () => ({ database: 'db', table: 'items', engine: 'InnoDB', collation: 'utf8mb4_general_ci', comment: null, columns: state.order.map(name => json(columns.find(c => c.name === name))), indexes: [{ name: 'PRIMARY', columns: ['title', 'id'], unique: true, primary: true }], foreign_keys: [] });
  const api = { getTableSchema: async () => schema(), getTableDdl: async () => state.ddlOverride ?? ddl(state.order), executeDdl: async (connectionId, sql) => { state.sql.push({ connectionId, sql }); state.order = [...state.desiredOrder ?? state.order]; } };
  const context = vm.createContext({ exports: {}, ...transactionFixtureBindings, console, document: doc, HTMLElement: Element, api, svg: () => '', ICON_KEY_GOLD: '', TabView: class { constructor(app, tab) { this.app = app; this.tab = tab; this.el = new Element(); } }, QueryTabView: class {}, DataTabView: class {}, ObjectsTabView: class {} });
  const load = (text, file) => { context.exports = {}; vm.runInContext(ts.transpile(text, { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS }), context, { filename: file }); Object.assign(context, context.exports); };
  load(read('src/core/columnOrder.ts'), 'columnOrder.ts');
  load(source.slice(source.indexOf('export class App'), source.indexOf('abstract class TabView')), 'App.ts');
  load(source.slice(source.indexOf('class StructureTabView extends TabView'), source.indexOf('class ChartTabView extends TabView')) + '\nexports.StructureTabView = StructureTabView;', 'StructureTabView.ts');
  const app = Object.create(context.App.prototype), cfg = { id: 'c', kind, params: '', read_only: false };
  const tab = { id: 't', connectionId: 'c', schema: 'db', object: 'items', kind: 'structure', dirty: false };
  Object.assign(app, { connections: [cfg], tabs: [tab], activeTabId: 't', views: new Map(), commandUnavailableReason: () => null, toast: (...args) => state.notices.push(args), renderStatus() {}, notifyToolbar: () => state.toolbarUpdates++, tabToolbar: { setItems() {} }, ribbon: { updateDisabled() {} }, menu: { renderBar() {} }, contextMenus: () => [], viewMenu: () => [] });
  const view = new context.StructureTabView(app, tab); app.views.set('t', view);
  return { context, view, app, api, state, cfg, tab, doc, schema, Element };
}

async function loaded(kind) { const h = harness(kind); await h.view.reload(); assert.equal(h.view.fields.length, 4); return h; }
function reorder(h) { h.view.selectedField = 2; h.view.moveField(-1); h.view.moveField(-1); h.state.desiredOrder = ['computed', 'id', 'title', 'updated']; }

test('physical column order capability fails closed for other dialects, ODBC and read-only', () => {
  const { context: c } = harness();
  assert.equal(c.columnOrderCapability('mysql'), null); assert.equal(c.columnOrderCapability('mariadb'), null);
  for (const kind of ['sqlite', 'postgresql', 'sqlserver', 'tidb']) assert.match(c.columnOrderCapability(kind), /不支持安全原地调整/);
  assert.match(c.columnOrderCapability('mysql', '@navkit-driver:{}'), /自定义驱动/);
  assert.match(c.columnOrderCapability('mysql', '', true), /只读/);
});

test('SHOW CREATE lexer preserves enum commas, generated expressions, collation and ON UPDATE', () => {
  const { context: c } = harness();
  assert.deepEqual(json([...c.mysqlColumnDefinitions(ddl()).values()]), definitions);
  const tricky = "CREATE TABLE `a(b` (`a` varchar(20) DEFAULT 'can\\'t,(split)', `b` int /* , ) */ DEFAULT (length('a,b')), -- comma, parenthesis )\n `c``x` varchar(40) DEFAULT 'it''s,one', CHECK (`b` > 0))";
  const parts = c.mysqlColumnDefinitions(tricky);
  assert.equal(parts.size, 2); // 非标准 SHOW CREATE 的前置行注释不能冒充可复用的字段。
  assert.match(parts.get('a'), /can\\'t,\(split\)/);
  assert.match(parts.get('b'), /\/\* , \) \*\//);
});

test('one ALTER reuses full definitions and never drops or rebuilds compound PK/index/constraint', () => {
  const { context: c } = harness();
  const sql = c.mysqlColumnOrderSql(ddl(), 'db', 'items', ['id', 'title', 'computed', 'updated'], ['computed', 'updated', 'id', 'title']);
  assert.match(sql, /^ALTER TABLE `db`\.`items`/);
  assert.equal((sql.match(/MODIFY COLUMN/g) ?? []).length, 2);
  assert.ok(sql.includes(definitions[2] + ' FIRST'));
  assert.ok(sql.includes(definitions[3] + ' AFTER `computed`'));
  assert.doesNotMatch(sql, /DROP|CREATE|PRIMARY KEY|CONSTRAINT|KEY `title_idx`/);
  assert.equal(c.mysqlColumnOrderSql(ddl(), '', 'items', ['id', 'title', 'computed', 'updated'], ['id', 'title', 'computed', 'updated']), null);
});

test('identifier quoting is safe for database, table and malicious column names', () => {
  const { context: c } = harness();
  const name = 'x`; DROP TABLE y; --';
  const escaped = name.replace(/`/g, '``');
  const sql = c.mysqlColumnOrderSql('CREATE TABLE `t` (`id` int, `' + escaped + '` int)', 'a`b', 't`z', ['id', name], [name, 'id']);
  assert.ok(sql.startsWith('ALTER TABLE `a``b`.`t``z`'));
  assert.ok(sql.includes('MODIFY COLUMN `' + escaped + '` int FIRST'));
});

test('malformed, duplicate, renamed and concurrent order definitions are rejected', () => {
  const { context: c } = harness();
  for (const input of ['DROP TABLE x', 'CREATE TABLE x (`a` int', 'CREATE TABLE x (`a` int, `a` int)', 'CREATE TABLE x (a int)']) assert.throws(() => c.mysqlColumnDefinitions(input));
  assert.throws(() => c.mysqlColumnOrderSql(ddl(), 'db', 't', ['wrong'], ['wrong']), /已发生变化/);
  assert.throws(() => c.mysqlColumnOrderSql(ddl(), 'db', 't', ['id', 'title', 'computed', 'updated'], ['id', 'title', 'computed', 'renamed']), /完整保留/);
});

test('real toolbar enables interior moves, keeps boundary reasons and reports unsupported dialect', async () => {
  const h = await loaded(); h.view.selectedField = 1;
  const items = h.app.tabToolbarItems(h.tab);
  assert.equal(items.find(item => item.id === 'move-up').disabled, false);
  assert.equal(items.find(item => item.id === 'move-down').disabled, false);
  h.view.selectedField = 0;
  assert.match(h.app.tabToolbarItems(h.tab).find(item => item.id === 'move-up').disabledReason, /第一个/);
  h.cfg.kind = 'sqlite';
  assert.match(h.app.tabToolbarItems(h.tab).find(item => item.id === 'move-down').disabledReason, /不支持安全原地调整/);
});

test('command dispatch actually reorders fields, marks draft and preserves selected row/focus', async () => {
  const h = await loaded(); h.view.selectedField = 1;
  await h.app.runCommand('move-up');
  assert.deepEqual(json(h.view.fields.map(f => f.name)), ['title', 'id', 'computed', 'updated']);
  assert.equal(h.view.selectedField, 0); assert.equal(h.tab.dirty, true);
  assert.equal(h.doc.activeElement.value, 'title');
  await h.app.runCommand('move-down');
  assert.deepEqual(json(h.view.fields.map(f => f.name)), ['id', 'title', 'computed', 'updated']); assert.equal(h.tab.dirty, false);
});

test('keyboard field focus selects row without destroying live input', async () => {
  const h = await loaded();
  const input = h.view.el.querySelectorAll('.designer-grid tbody tr')[2].querySelector('input');
  input.focus();
  assert.equal(h.view.selectedField, 2); assert.equal(h.doc.activeElement, input); assert.equal(h.view.el.contains(input), true);
  input.value = 'renamed'; input.blur();
  assert.equal(h.view.fields[2].name, 'renamed'); assert.match(h.view.fieldMoveReason(-1), /不能与新增/);
});

test('safe reorder submits exactly one fully-qualified ALTER and verifies real returned order', async () => {
  const h = await loaded(); reorder(h); const pkBefore = json(h.tab.schemaInfo.indexes[0]);
  await h.view.saveChanges();
  assert.equal(h.state.sql.length, 1);
  assert.ok(h.state.sql[0].sql.includes(definitions[2] + ' FIRST'));
  assert.doesNotMatch(h.state.sql[0].sql, /DROP|PRIMARY KEY/);
  assert.deepEqual(json(h.tab.schemaInfo.indexes[0]), pkBefore);
  assert.deepEqual(json(h.view.fields.map(f => f.name)), h.state.desiredOrder);
  assert.equal(h.tab.dirty, false); assert.equal(h.view.isSavingStructure(), false);
  assert.match(h.state.notices.at(-1)[1], /核对数据库实际顺序/);
});

test('concurrent metadata or hidden full-definition changes refuse execution and retain draft', async () => {
  for (const mode of ['metadata', 'definition']) {
    const h = await loaded(); reorder(h);
    if (mode === 'metadata') h.api.getTableSchema = async () => { const s = h.schema(); s.columns[0].type = 'varchar(20)'; return s; };
    else h.state.ddlOverride = ddl().replace('length(\'x,y\')', 'length(\'changed\')');
    await h.view.saveChanges();
    assert.equal(h.state.sql.length, 0); assert.equal(h.tab.dirty, true);
    assert.match(h.state.notices.at(-1)[1], /已发生变化/);
    assert.deepEqual(json(h.view.fields.map(f => f.name)), h.state.desiredOrder);
  }
});

test('mixed field edits, read-only, unsupported dialect and mid-flight read-only state never write', async () => {
  for (const mode of ['mixed', 'readonly', 'sqlite', 'changed']) {
    const h = await loaded(); reorder(h);
    if (mode === 'mixed') h.view.fields[0].comment = 'changed';
    if (mode === 'readonly') h.cfg.read_only = true;
    if (mode === 'sqlite') h.cfg.kind = 'sqlite';
    if (mode === 'changed') h.api.getTableDdl = async () => { h.cfg.read_only = true; return ddl(); };
    await h.view.saveChanges(); assert.equal(h.state.sql.length, 0); assert.equal(h.tab.dirty, true);
  }
});

test('busy save is single-flight and blocks all direct field mutation commands', async () => {
  const h = await loaded(); reorder(h);
  let release; const gate = new Promise(resolve => { release = resolve; });
  const getSchema = h.api.getTableSchema; h.api.getTableSchema = async (...args) => { await gate; return getSchema(...args); };
  const first = h.view.saveChanges(); assert.equal(h.view.isSavingStructure(), true);
  const before = JSON.stringify(h.view.fields);
  h.view.addField(); h.view.insertField(); h.view.deleteField(); h.view.togglePrimaryKey(); h.view.moveField(1);
  assert.equal(JSON.stringify(h.view.fields), before);
  await h.view.saveChanges(); release(); await first;
  assert.equal(h.state.sql.length, 1); assert.equal(h.view.isSavingStructure(), false);
});

test('DDL errors or unconfirmed actual order never report success and preserve dirty draft', async () => {
  for (const mode of ['execute', 'order', 'readback']) {
    const h = await loaded(); reorder(h);
    if (mode === 'execute') h.api.executeDdl = async () => { throw new Error('database denied ALTER'); };
    if (mode === 'order') h.api.executeDdl = async (_id, sql) => { h.state.sql.push({ sql }); };
    if (mode === 'readback') h.api.executeDdl = async (_id, sql) => { h.state.sql.push({ sql }); h.api.getTableSchema = async () => { throw new Error('readback offline'); }; };
    await h.view.saveChanges();
    assert.equal(h.tab.dirty, true); assert.equal(h.view.isSavingStructure(), false);
    assert.equal(h.state.notices.some(n => n[0] === 'success'), false);
    assert.match(h.state.notices.at(-1)[1], mode === 'execute' ? /未确认保存/ : /已执行.*尚未确认/);
    assert.deepEqual(json(h.view.fields.map(f => f.name)), h.state.desiredOrder);
  }
});

test('accessible field names and persistent save status are visible, busy selection cannot recreate enabled editors', async () => {
  const h = await loaded();
  const input = h.view.el.querySelector('.designer-grid tbody tr input[type=text]');
  assert.equal(input.attributes['aria-label'], 'id：名称');
  reorder(h);
  assert.match(h.view.el.querySelector('.field-order-status').textContent, /待保存草稿/);
  let release; const gate = new Promise(resolve => { release = resolve; });
  const getSchema = h.api.getTableSchema; h.api.getTableSchema = async (...args) => { await gate; return getSchema(...args); };
  const saving = h.view.saveChanges();
  assert.equal(h.view.el.attributes['aria-busy'], 'true');
  const selected = h.view.selectedField;
  h.view.el.querySelectorAll('.designer-grid tbody tr')[3].fire('mousedown');
  assert.equal(h.view.selectedField, selected);
  assert.equal(h.view.el.querySelector('.designer-footer select').disabled, true);
  release(); await saving;
  assert.equal(h.view.el.attributes['aria-busy'], 'false');
  assert.match(h.view.el.querySelector('.field-order-status').textContent, /已保存/);
});

test('actual metadata database fully qualifies empty tab schema; unknown namespace refuses order moves', async () => {
  const h = await loaded(); h.tab.schema = ''; reorder(h);
  await h.view.saveChanges();
  assert.equal(h.state.sql.length, 1); assert.match(h.state.sql[0].sql, /^ALTER TABLE `db`\.`items`/);
  const unknown = await loaded(); unknown.tab.schema = ''; unknown.tab.schemaInfo.database = ''; unknown.view.selectedField = 1;
  assert.match(unknown.view.fieldMoveReason(-1), /未知命名空间/);
  unknown.view.moveField(-1); assert.equal(unknown.tab.dirty, false);
});

test('normal appended field draft is not mistaken for reordering', async () => {
  const h = await loaded(); h.view.addField(); const f = h.view.fields.at(-1); f.name = 'new_value'; f.type = 'int';
  await h.view.saveChanges();
  assert.equal(h.state.sql.length, 1); assert.match(h.state.sql[0].sql, /ADD COLUMN/); assert.doesNotMatch(h.state.sql[0].sql, /FIRST|AFTER/);
});

test('actual transaction guard disables column writes and leaves draft and database untouched', async () => {
  const h = await loaded(); h.view.selectedField = 1;
  // Other column tests isolate field behavior; this test restores the real App gate.
  h.app.commandUnavailableReason = h.context.App.prototype.commandUnavailableReason.bind(h.app);
  let active = { active: true, schema: 'db', owner: 'query-owner', failed: false }, busy = false;
  h.app.manualTransactionController = { status: id => id === 'c' ? active : undefined, busy: id => id === 'c' && busy };
  const before = JSON.stringify(h.view.fields);
  const item = h.app.guardedTabToolbarItems(h.tab).find(item => item.id === 'move-up');
  assert.equal(item.disabled, true); assert.match(item.disabledReason, /手动事务/);
  await h.app.runCommand('move-up'); await h.app.runCommand('save');
  assert.equal(JSON.stringify(h.view.fields), before); assert.equal(h.state.sql.length, 0);
  active = undefined; busy = true;
  assert.match(h.app.commandUnavailableReason('move-down'), /事务操作正在进行/);
  busy = false;
  assert.equal(h.app.commandUnavailableReason('move-up'), null);
});
