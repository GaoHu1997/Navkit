import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import ts from 'typescript';
import { fileURLToPath } from 'node:url';
import { transactionFixtureBindings } from './transaction-fixture-bindings.mjs';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const read = file => fs.readFileSync(path.join(root, file), 'utf8');
const context = vm.createContext({ exports: {} });
vm.runInContext(ts.transpile(read('src/core/chart.ts'), { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS }), context);
const logic = context.exports;
const source = (rows, columns = ['分类', '值']) => ({ title: '数据', description: 'current snapshot', columns, rows });
const plain = value => JSON.parse(JSON.stringify(value));

test('only finite decimal values are numeric; NULL/blank/hex/NaN never become zero', () => {
  for (const value of [null, undefined, '', ' ', 'abc', 'NaN', 'Infinity', '-Infinity', '0x10', '1,000', '1e309']) assert.equal(logic.chartNumber(value), null);
  for (const [value, number] of [['0', 0], ['-2.5', -2.5], [' .25 ', .25], ['1e3', 1000], ['+4', 4]]) assert.equal(logic.chartNumber(value), number);
});
test('empty and nonnumeric sources have honest error messages', () => {
  assert.match(logic.chartSourceError(null), /当前/);
  assert.match(logic.chartSourceError(source([])), /为空/);
  assert.match(logic.chartSourceError(source([['a', '']])), /数值/);
  assert.equal(logic.chartSourceError(source([['a', '0']])), null);
});
test('chart copies columns and rows so later source changes or closes cannot mutate snapshot', () => {
  const src = source([['a', '2']]); const chart = logic.createChartState(src);
  src.columns[1] = 'changed'; src.rows[0][1] = '99'; src.rows.push(['b', '88']);
  assert.deepEqual(plain(chart.source.columns), ['分类', '值']); assert.deepEqual(plain(chart.source.rows), [['a', '2']]);
});
test('duplicate column names are selected by index; signed values, null categories, zero retain true values', () => {
  const chart = logic.createChartState(source([[null, '-2', '7'], ['', '0', '8'], ['a', 'text', '9']], ['value', 'value', 'value']));
  chart.xIndex = 0; chart.yIndex = 1;
  const result = logic.projectChart(chart);
  assert.deepEqual(plain(result.points.map(point => [point.label, point.value])), [['NULL', -2], ['（空串）', 0]]); assert.equal(result.skipped, 1);
  chart.yIndex = 2; assert.deepEqual(plain(logic.projectChart(chart).points.map(point => point.value)), [7, 8, 9]);
});
test('all rows count toward invalid/omitted feedback while rendered points have a strict limit', () => {
  const chart = logic.createChartState(source(Array.from({ length: 210 }, (_, index) => [`r${index}`, String(index)]).concat([['null', null], ['blank', '']])));
  const result = logic.projectChart(chart);
  assert.equal(result.points.length, 200); assert.equal(result.omitted, 10); assert.equal(result.skipped, 2);
  chart.yIndex = -1; assert.match(logic.projectChart(chart).error, /有效/);
});
test('SVG has accessible names, escaped labels and stable finite geometry for extreme/negative/zero values', () => {
  const chart = logic.createChartState(source([['<script>"&', String(-Number.MAX_VALUE)], ['b', String(Number.MAX_VALUE)], ['zero', '0']]));
  for (const type of ['bar', 'line']) {
    chart.type = type; const svg = logic.renderDataChart(chart);
    assert.match(svg, /role="img"/); assert.match(svg, /<title>/); assert(!svg.includes('<script>')); assert(svg.includes('&lt;script&gt;'));
    assert(!svg.includes('NaN') && !svg.includes('Infinity')); assert(svg.includes(type === 'bar' ? '<rect' : '<polyline'));
  }
  chart.source.rows = [['a', '0'], ['b', '0']]; chart.type = 'bar';
  assert.match(logic.renderDataChart(chart), /height="0"/);
});

class Element {
  constructor(tag = 'div') { this.tagName = tag; this.children = []; this.attributes = {}; this.style = {}; this.listeners = {}; this.className = ''; this.value = ''; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  appendChild(child) { this.children.push(child); child.parentElement = this; return child; }
  replaceChildren(...children) { this.children = []; this.append(...children); }
  setAttribute(key, value) { this.attributes[key] = value; }
  addEventListener(key, handler) { this.listeners[key] = handler; }
  fire(key) { this.listeners[key]?.({ target: this }); }
  remove() { this.removed = true; }
  focus() { this.focused = true; }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  querySelectorAll(selector) { return this.children.flatMap(child => [...(child.tagName === selector ? [child] : []), ...child.querySelectorAll(selector)]); }
}
const appText = read('src/core/App.ts');
const parsedApp = ts.createSourceFile('App.ts', appText, ts.ScriptTarget.ES2021, true);
function classText(name) { return parsedApp.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === name).getText(parsedApp); }
function chartHarness() {
  const notices = [], apiCalls = [];
  const document = { createElement: tag => new Element(tag) };
  const env = vm.createContext({ exports: {}, document, ...logic, ...transactionFixtureBindings, console,
    api: new Proxy({}, { get: (_, name) => () => { apiCalls.push(name); throw new Error('Charts must not issue database IPC'); } }),
    DataTabView: class {}, StructureTabView: class {}, ObjectsTabView: class {},
    tab: {}, setTimeout, Map, Set,
  });
  const load = text => { vm.runInContext(ts.transpile(text, { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS }), env); Object.assign(env, env.exports); };
  load(read('src/ui/ChartPanel.ts').replace(/^import .*;$/gm, ''));
  const query = parsedApp.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === 'QueryTabView');
  const method = query.members.find(node => node.name?.getText(parsedApp) === 'chartSource');
  load(`class QueryTabView { ${method.getText(parsedApp)} } globalThis.QueryTabView = QueryTabView;`);
  load(classText('App'));
  load(classText('TabView') + '\n' + classText('ChartTabView') + '\nglobalThis.ChartTabView = ChartTabView;');
  const app = Object.create(env.App.prototype);
  Object.assign(app, { tabs: [], views: new Map(), connections: [], activeTabId: null, settings: { rows_per_page: 100 },
    ribbon: { updateDisabled() {} }, tabToolbar: { setItems() {} }, renderStatus() {},
    menu: { renderBar() {} }, contextMenus: () => [], viewMenu: () => [],
    renderTabs() { this.renders = (this.renders ?? 0) + 1; }, toast: (...args) => notices.push(args), currentConnectionId: () => 'tree-other', tree: null });
  return { env, app, notices, apiCalls, document };
}
function dataPage(h, rows = [['a', '3']]) {
  const tab = { id: 'source', kind: 'table', title: '源表', object: 'items', connectionId: 'own', connectionName: '自己的连接', schema: 'main', page: { columns: [{ name: '分类' }, { name: '值' }], rows } };
  h.app.tabs = [tab]; h.app.activeTabId = tab.id; h.app.views.set(tab.id, new h.env.DataTabView()); return tab;
}

test('native chart selectors are labelled, switch type/axes and expose original data with invalid feedback', () => {
  const h = chartHarness(); const chart = logic.createChartState(source([['a', '5'], ['b', 'bad']])); const panel = new h.env.ChartPanel(chart);
  const selectors = panel.el.querySelectorAll('select');
  assert.deepEqual(selectors.map(item => item.attributes['aria-label']), ['图表类型', '分类轴', '数值字段']);
  assert.equal(panel.dataBody.children.length, 1); assert.match(panel.status.textContent, /跳过 1/);
  selectors[0].value = 'line'; selectors[0].fire('change'); assert.equal(chart.type, 'line'); assert.match(panel.chartHost.innerHTML, /polyline/);
  selectors[2].value = '0'; selectors[2].fire('change'); assert.match(panel.status.textContent, /没有可绘制/); assert.equal(panel.chartHost.hidden, true);
  assert.deepEqual(plain(chart.source.rows), [['a', '5'], ['b', 'bad']]);
});
test('actual chart command uses only active data view, keeps own connection and opens a real chart tab without IPC', async () => {
  const h = chartHarness(), tab = dataPage(h); h.app.connections = [{ id: 'tree-other', name: '不要借用' }];
  await h.app.runCommand('chart');
  const chartTab = h.app.activeTab;
  assert.equal(chartTab.kind, 'chart'); assert.equal(chartTab.connectionId, 'own'); assert.equal(chartTab.schema, 'main');
  assert.equal(chartTab.connectionName, '自己的连接'); assert.equal(h.app.tabs.length, 2);
  assert.match(chartTab.chart.source.description, /不含未提交/);
  tab.page.rows[0][1] = '100'; assert.equal(chartTab.chart.source.rows[0][1], '3');
  const view = h.app.createView(chartTab); assert(view instanceof h.env.ChartTabView); h.app.views.set(chartTab.id, view);
  view.activate(); await view.reload();
  h.app.activateTab(tab.id); h.app.activateTab(chartTab.id);
  assert.equal(chartTab.chart.source.rows[0][1], '3');
  h.app.manualTransactionIsBusy = () => false; h.app.manualTransactionFor = () => null;
  h.app.closeTab(chartTab.id);
  assert.equal(h.app.activeTabId, tab.id); assert.equal(h.app.tabs.length, 1); assert.equal(tab.page.rows[0][1], '100');
  assert.equal(h.apiCalls.length, 0);
});
test('chart command rejects wrong view, empty and nonnumeric data instead of opening an empty tab', async () => {
  for (const rows of [[], [['a', '']], [['a', 'not numeric']]]) {
    const h = chartHarness(); dataPage(h, rows); await h.app.runCommand('chart'); assert.equal(h.app.tabs.length, 1); assert.match(h.notices[0][1], /空|数值/);
  }
  const h = chartHarness(), tab = dataPage(h); h.app.views.set(tab.id, {}); await h.app.runCommand('chart'); assert.equal(h.app.tabs.length, 1); assert.match(h.notices[0][1], /当前/);
});
test('query chart uses currently visible result not stale activeResult; messages/running reject old data', () => {
  const h = chartHarness(); const view = new h.env.QueryTabView();
  view.tab = { title: '查询', connectionName: '当前连接', schema: 'current', activeResult: 0, results: { sets: [
    { has_rows: true, columns: [{ name: 'old' }], rows: [['1']] },
    { has_rows: true, columns: [{ name: 'real' }], rows: [['9']] },
  ] } };
  view.activeResultTab = 1; view.running = false;
  assert.deepEqual(plain(view.chartSource().rows), [['9']]); assert.match(view.chartSource().title, /结果 2/);
  view.activeResultTab = -1; assert.equal(view.chartSource(), null);
  view.activeResultTab = 1; view.running = true; assert.equal(view.chartSource(), null);
});
test('chart toolbar is enabled for data/query while top ribbon chart remains hidden', () => {
  const h = chartHarness(); const tab = dataPage(h);
  for (const kind of ['table', 'query']) { tab.kind = kind; const item = h.app.tabToolbarItems(tab).find(item => item.id === 'chart'); assert(item && item.disabled !== true); }
  const ribbon = ts.createSourceFile('Ribbon.ts', read('src/ui/Ribbon.ts'), ts.ScriptTarget.ES2021, true);
  const declaration = ribbon.statements.flatMap(node => ts.isVariableStatement(node) ? [...node.declarationList.declarations] : []).find(node => node.name.getText(ribbon) === 'RIBBON_MAIN');
  assert(!declaration.initializer.getText(ribbon).includes('id: "chart"'));
});

test('real transaction gates block automatic save without blocking read-only chart snapshots', async () => {
  const h = chartHarness(), tab = dataPage(h);
  const status = { active: true, owner: 'query-owner', schema: 'main', failed: false };
  h.app.manualTransactionController = { status: id => id === 'own' ? status : undefined, busy: () => false };
  assert.match(h.app.commandUnavailableReason('save'), /手动事务/);
  assert.equal(h.app.commandUnavailableReason('chart'), null);
  assert.equal(h.env.MANUAL_TRANSACTION_WRITE_COMMANDS.has('grid-commit'), true);
  assert.equal(h.env.MANUAL_TRANSACTION_WRITE_COMMANDS.has('chart'), false);
  await h.app.runCommand('chart');
  assert.equal(h.app.activeTab.kind, 'chart');
  assert.equal(h.app.activeTab.connectionId, tab.connectionId);
  assert.equal(h.apiCalls.length, 0);
});
