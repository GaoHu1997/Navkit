import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';

const read = file => fs.readFileSync(new URL(`../../${file}`, import.meta.url), 'utf8');
const flush = () => new Promise(resolve => setImmediate(resolve));
async function module(file, mockApi = false) {
  const result = await build({ entryPoints: [fileURLToPath(new URL(`../../${file}`, import.meta.url))], bundle: true, write: false, platform: 'node', format: 'cjs', logLevel: 'silent',
    plugins: mockApi ? [{ name: 'metadata-only-api', setup(builder) {
      builder.onResolve({ filter: /^\.\/api$/ }, () => ({ path: 'api', namespace: 'metadata-fixture' }));
      builder.onLoad({ filter: /.*/, namespace: 'metadata-fixture' }, () => ({ contents: 'export const api = {};', loader: 'js' }));
    } }] : [],
  });
  const context = vm.createContext({ module: { exports: {} }, exports: {} });
  vm.runInContext(result.outputFiles[0].text, context);
  return context.module.exports;
}
const namespaces = await module('src/core/namespace-label.ts');
const { DB_KINDS } = await module('src/core/dbcatalog.ts');
const { treeContextMenu } = await module('src/ui/menu.ts');
const { loadObjectDetails, objectDetailsLoading } = await module('src/core/objectDetails.ts', true);
const plain = value => JSON.parse(JSON.stringify(value));
const connection = (kind = 'postgresql') => ({ id: 'own', name: '连接', kind, database: 'physical_database', read_only: false });

class Element {
  constructor(tag = 'div') { this.tagName = tag; this.children = []; this.style = {}; this.value = ''; this.textContent = ''; this.className = ''; }
  appendChild(child) { this.children.push(child); return child; }
  append(...children) { this.children.push(...children); }
}
const document = { createElement: tag => new Element(tag) };
const source = ts.createSourceFile('App.ts', read('src/core/App.ts'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
const appClass = source.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === 'App');
const memberNames = ['nodeKind', 'treeMenuFor', 'loadChildren', 'updateInfoPanelForNode', 'openDatabaseNode', 'closeDatabaseNode', 'runCommand', 'cmdNewDatabase', 'cmdDropDatabase', 'databaseDdl'];
const members = memberNames.map(name => {
  const member = appClass.members.find(node => node.name?.getText(source) === name);
  assert(member, `actual App member ${name}`);
  return member.getText(source);
}).join('\n');
function harness(kind = 'postgresql', overrides = {}) {
  const cfg = connection(kind), dialogs = [], calls = [], messages = [], nodes = new Map();
  const api = { listDatabases: async () => [['public', 'public']], openDatabase: async () => ({ ok: true }), executeDdl: async (...args) => { calls.push(args); }, ...overrides };
  class Dialog { constructor(options) { this.options = options; this.body = new Element(); options.render?.(this.body, this); dialogs.push(this); } }
  const exports = {}, context = vm.createContext({ exports, document, Dialog, api, treeContextMenu, ...namespaces, kindMeta: value => DB_KINDS.find(item => item.id === value) });
  vm.runInContext(ts.transpile(`export class App { ${members} }`, { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS }), context);
  const app = new exports.App();
  Object.assign(app, { connections: [cfg], openDbs: new Set(), openConnections: new Set(), tabs: [], favorites: [],
    connectionById: id => id === cfg.id ? cfg : undefined, currentConnectionId: () => cfg.id, currentDbName: () => 'public',
    ensureConnection: async () => {}, syncConnIcons() {}, openObjectsTab() {}, cmdTreeRefresh() {}, suspendDatasourceTabs() {}, manualTransactionFor: () => null,
    guardLeave: async (_ids, _cause, action) => { await action(); return true; },
    toast: (...args) => messages.push(args), tree: { find: id => nodes.get(id), render() {} },
    commandUnavailableReason: () => null, notifyToolbar() {}, renderStatus() {}, menu: { renderBar() {} }, contextMenus: () => [], viewMenu: () => [],
  });
  return { app, cfg, dialogs, calls, messages, nodes, api };
}

// These assertions exercise the shared production helper, not a second protocol mapping.
test('命名空间协议族区分PG模式、MySQL和SQLServer数据库，物理库配置保持原值', () => {
  for (const kind of ['postgresql', 'custom_pg', 'cockroachdb']) {
    assert.equal(namespaces.namespaceType(connection(kind)), 'schema', kind);
    assert.equal(namespaces.namespaceName(connection(kind)), '模式', kind);
  }
  for (const kind of ['mysql', 'mariadb', 'sqlserver', 'mssql', 'clickhouse', 'sqlite']) {
    assert.equal(namespaces.namespaceType(connection(kind)), 'database', kind);
    assert.equal(namespaces.namespaceName(connection(kind)), '数据库', kind);
  }
  assert.equal(namespaces.namespaceLabel(connection('sqlite')), '数据库 (main)');
  assert.equal(namespaces.namespaceName(connection('unknown_odbc')), '命名空间');
  assert.equal(connection().database, 'physical_database');
});

test('真实App加载树以namespaceType建节点，菜单与详情入口使用节点所属连接', async () => {
  for (const [kind, expected] of [['postgresql', 'schema'], ['mariadb', 'database'], ['sqlserver', 'database']]) {
    const h = harness(kind), root = { id: 'conn:own', children: [] };
    await h.app.loadChildren(root);
    assert.equal(root.children.length, 1);
    const node = root.children[0];
    assert.equal(node.type, expected); assert.equal(h.app.nodeKind(node), expected);
    assert.match(h.app.treeMenuFor(node).find(item => item.id === 'db-open').label, expected === 'schema' ? /模式/ : /数据库/);
    assert.equal(h.app.nodeKind({ id: 'conn:own/db/public/table/orders/grp-index' }), 'table');
    const details = []; h.app.showObjectDetails = (...args) => details.push(args);
    h.app.updateInfoPanelForNode(node);
    assert.equal(details[0][1], 'public'); assert.equal(details[0][2], ''); assert.equal(details[0][3], expected);
    assert.equal(h.cfg.database, 'physical_database');
  }
});

test('真实目录读取失败呈现对应命名空间错误，不生成main节点', async () => {
  const h = harness('postgresql', { listDatabases: async () => { throw Error('catalog permission denied'); } });
  const root = { id: 'conn:own', children: [] }; await h.app.loadChildren(root);
  assert.equal(root.children.length, 0); assert.match(h.messages[0][1], /无法列出模式.*catalog permission denied/);
});

test('右键模式菜单保留运行与转储同组，编辑动作明确是编辑连接', () => {
  const menu = treeContextMenu('schema'), run = menu.findIndex(item => item.id === 'tree-run-sql-file');
  assert.equal(menu[run + 1].id, 'tree-dump');
  assert.equal(menu.find(item => item.id === 'db-new').label, '新建模式...');
  assert.equal(menu.find(item => item.id === 'db-drop').label, '删除模式');
  assert.equal(menu.find(item => item.id === 'tree-edit-db').label, '编辑连接...');
});

test('真实详情与加载态区分命名空间和物理默认库，SQLServer真实schema仍称模式', async () => {
  const api = { listObjects: async () => [], listRoutines: async () => [], serverInfo: async () => ({ version: 'fixture', extra: [] }) };
  for (const [kind, type, title] of [['postgresql', 'schema', '模式'], ['postgresql', 'database', '模式'], ['mysql', 'database', '数据库'], ['sqlserver', 'schema', '模式']]) {
    const cfg = connection(kind), details = await loadObjectDetails(cfg, 'public', '', type, {}, api);
    assert.equal(details.subtitle, title); assert.equal(objectDetailsLoading(cfg, 'public', '', type).subtitle, title);
    assert(details.fields.some(field => field.label === `${title}名称` && field.value === 'public'));
    assert(details.fields.some(field => field.label === '默认数据库（配置）' && field.value === 'physical_database'));
    assert(!details.fields.some(field => field.label.includes('数据库/模式')));
  }
});

test('新建和删除模式的真实Dialog字段及回执匹配CREATE/DROP SCHEMA，只调用模拟DDL', async () => {
  const h = harness(); h.app.cmdNewDatabase();
  const create = h.dialogs[0], row = create.body.children[0];
  assert.equal(create.options.title, '新建模式'); assert.equal(row.children[0].textContent, '模式名:');
  row.children[1].value = 'a"b'; create.options.buttons[0].onClick(); await flush();
  assert.deepEqual(plain(h.calls), [['own', 'CREATE SCHEMA "a""b"']]);
  assert.match(h.messages.at(-1)[1], /^模式「a"b」已创建$/);
  h.app.cmdDropDatabase(); const drop = h.dialogs[1];
  assert.equal(drop.options.title, '删除模式'); assert.match(drop.body.textContent, /删除模式「public」/);
  drop.options.buttons[0].onClick(); await flush();
  assert.deepEqual(plain(h.calls[1]), ['own', 'DROP SCHEMA IF EXISTS "public" CASCADE']);
  assert.match(h.messages.at(-1)[1], /^模式「public」已删除$/);
  assert.equal(h.cfg.database, 'physical_database');
});

test('DDL按实际协议族引用标识符，MySQL兼容族和SQLServer不借用PG模式操作', () => {
  const h = harness();
  assert.equal(h.app.databaseDdl('mariadb', 'create', 'a`b'), 'CREATE DATABASE `a``b`');
  assert.equal(h.app.databaseDdl('custom_pg', 'drop', 'a"b'), 'DROP SCHEMA IF EXISTS "a""b" CASCADE');
  assert.equal(h.app.databaseDdl('sqlserver', 'create', 'a]b'), 'CREATE DATABASE [a]]b]');
});

test('实际打开失败与取消关闭不发布成功状态，成功关闭文案采用模式', async () => {
  const h = harness('postgresql', { openDatabase: async () => ({ ok: false, message: 'permission denied' }) });
  const node = { id: 'conn:own/db/public', loaded: false, children: [] }; h.nodes.set(node.id, node);
  await h.app.runCommand('db-open');
  assert.equal(h.app.openDbs.size, 0); assert.equal(node.loading, false);
  assert(h.messages.some(([, text]) => text.includes('打开模式失败'))); assert(!h.messages.some(([status]) => status === 'success'));
  h.messages.length = 0; h.app.openDbs.add('own/public'); h.app.guardLeave = async () => false;
  await h.app.runCommand('db-close'); assert.equal(h.messages.length, 0); assert(h.app.openDbs.has('own/public'));
  h.app.guardLeave = async (_ids, _cause, action) => { await action(); return true; };
  await h.app.runCommand('db-close'); assert.equal(h.app.openDbs.size, 0); assert.match(h.messages.at(-1)[1], /^模式「public」已关闭$/);
});
