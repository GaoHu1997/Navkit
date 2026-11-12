import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';
import vm from 'node:vm';
import ts from 'typescript';

// 使用实际组件与CSS，在隔离页面注入只读样例API；不启动服务器、不访问真实数据库。
const root = fileURLToPath(new URL('../..', import.meta.url));
const built = await build({
  stdin: { contents: 'export { InfoPanel } from "./src/ui/InfoPanel.ts"; export { TreePanel } from "./src/ui/TreePanel.ts"; export { openServerMonitorDialog } from "./src/dialogs/ServerMonitorDialog.ts";', resolveDir: root, loader: 'ts' },
  bundle: true, write: false, platform: 'browser', format: 'iife', globalName: 'DatabaseUi', loader: { '.css': 'empty' }, logLevel: 'silent',
});
const css = (await Promise.all(['base.css', 'server-monitor-dialog.css'].map(file => readFile(new URL('../../src/styles/' + file, import.meta.url), 'utf8')))).join('\n');

// 只提取本次涉及的真实App方法，避免依赖其它模块正在迁移的测试夹具。
const appSource = await readFile(new URL('../../src/core/App.ts', import.meta.url), 'utf8');
const ast = ts.createSourceFile('App.ts', appSource, ts.ScriptTarget.Latest, true);
const appClass = ast.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === 'App');
const methods = ['showObjectDetails', 'updateInfoPanel'].map(name => appClass.members.find(node => ts.isMethodDeclaration(node) && node.name.getText(ast) === name).getText(ast));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const flush = () => new Promise(resolve => setImmediate(resolve));
function detailsHarness() {
  const gates = [], states = [];
  const context = vm.createContext({ connIconOf: () => 'connection', QueryTabView: class {}, querySnippets: () => [],
    loadObjectDetails: () => { const gate = deferred(); gates.push(gate); return gate.promise; },
  });
  vm.runInContext(ts.transpileModule('class DetailsApp {' + methods.join(';') + '} globalThis.DetailsApp = DetailsApp;', { compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None } }).outputText, context);
  const app = new context.DetailsApp();
  Object.assign(app, { infoRequest: 0, infoPanel: { setState: state => states.push(JSON.parse(JSON.stringify(state))), setSnippets() {} }, activeView: () => null });
  const cfg = { id: 'own', name: 'local', kind: 'mysql' };
  return { app, gates, states, show: schema => app.showObjectDetails(cfg, schema, 'items', 'table') };
}
test('同对象在途请求合并，刷新不清空内容，跨库迟到响应不会串选', async () => {
  const h = detailsHarness();
  const state = title => ({ icon: 'table', title, fields: [{ label: '数据大小', value: '2 MB' }] });
  h.show('one'); h.show('one'); assert.equal(h.gates.length, 1); assert.equal(h.states.length, 1);
  h.gates[0].resolve(state('one')); await flush();
  const displayed = h.states.length;
  h.show('one'); assert.equal(h.gates.length, 2); assert.equal(h.states.length, displayed);
  h.show('two'); h.gates[1].resolve(state('old one')); await flush(); assert.notEqual(h.states.at(-1).title, 'old one');
  h.gates[2].resolve(state('two')); await flush(); assert.equal(h.states.at(-1).title, 'two');
  h.show('two'); assert.equal(h.states.at(-1).title, 'two');
  h.gates[3].reject(new Error('offline')); await flush(); assert.match(h.states.at(-1).fields[0].value, /offline/);
});
test('切换到查询页使旧对象详情失效，不覆盖代码段', async () => {
  const h = detailsHarness(); h.show('one');
  h.app.activeTab = { kind: 'query' };
  let snippets = 0; h.app.infoPanel.setSnippets = () => snippets++;
  h.app.updateInfoPanel(); const count = h.states.length;
  h.gates[0].resolve({ icon: 'table', title: 'late', fields: [] }); await flush();
  assert.equal(snippets, 1); assert.equal(h.states.length, count);
});

test('真实浏览器：树双击、信息面板DOM稳定、电源操作列和只读保护', async () => {
  const browser = await chromium.launch({ executablePath: process.env.NAVKIT_EDGE_PATH || 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
    const errors = []; page.on('pageerror', error => errors.push(error.message));
    await page.route('https://navkit-ui.test/**', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><html><body></body></html>' }));
    await page.goto('https://navkit-ui.test/');
    await page.addStyleTag({ content: css }); await page.addScriptTag({ content: built.outputFiles[0].text });
    const stable = await page.evaluate(async () => {
      const panel = new DatabaseUi.InfoPanel(); document.body.appendChild(panel.el);
      const state = { identity: 'local/one/items', title: 'items', refs: [{ icon: 'info_pane_conn_mysql', text: 'local' }], info: [{ label: '数据大小', value: '1.00 MB' }], ddl: 'CREATE TABLE items (id INT);', column: null };
      panel.setTableState(state);
      const identity = panel.el.querySelector('.info-identity');
      panel.setTableState(structuredClone(state));
      const sameDom = identity === panel.el.querySelector('.info-identity');
      panel.el.querySelectorAll('.info-mode-tab')[1].click();
      const ddl = panel.el.querySelector('.ddl-code');
      panel.setTableState(structuredClone(state));
      const sameDdl = ddl === panel.el.querySelector('.ddl-code');
      panel.setState({ icon: 'info_pane_database', title: 'other', fields: [] });
      panel.setTableState(state);
      const restored = !!panel.el.querySelector('.info-modes');
      panel.el.querySelectorAll('.info-mode-tab')[1].click();
      panel.setTableState({ ...state, identity: 'local/two/items' });
      const resetMode = panel.el.querySelector('.info-mode-tab.active').textContent;
      panel.el.remove(); return { sameDom, sameDdl, restored, resetMode };
    });
    assert.deepEqual(stable, { sameDom: true, sameDdl: true, restored: true, resetMode: '表信息' });
    await page.evaluate(() => {
      window.selections = 0; window.opens = 0;
      window.tree = new DatabaseUi.TreePanel({ onSelect: () => selections++, onOpen: node => { opens++; node.expanded = !node.expanded; tree.render(); } });
      tree.setNodes([{ id: 'db:main', label: 'main', type: 'database', icon: 'database', children: [], expandable: true, expanded: false }]);
      document.body.appendChild(tree.el);
    });
    await page.locator('.tree-label').dblclick();
    await page.locator('.tree-label').dblclick();
    assert.deepEqual(await page.evaluate(() => ({ selections, opens })), { selections: 1, opens: 2 });
    await page.evaluate(() => tree.el.remove());
    await page.evaluate(() => {
      window.kills = [];
      window.monitorConnections = [{ id: 'write', name: 'local', kind: 'mysql', read_only: false }, { id: 'read', name: 'read only', kind: 'mysql', read_only: true }];
      window.monitor = DatabaseUi.openServerMonitorDialog(monitorConnections, monitorConnections[0], {
        listDbProcesses: async () => [{ id: '941', user: 'app', host: 'localhost:52346', db: 'sample', command: 'Query', state: 'executing', elapsed_secs: 12, sql: 'SELECT ' + 'very_long_column_name,'.repeat(100) + '1 FROM sample;', is_self: false }],
        killDbProcess: async (...args) => { kills.push(args); return '已结束'; },
      });
    });
    await page.locator('.monitor-kill').waitFor(); await page.locator('.monitor-pause').click();
    const measure = () => page.evaluate(() => {
      const wrap = document.querySelector('.monitor-table-wrap'), cell = document.querySelector('.monitor-session-action'), button = cell.querySelector('button');
      const w = wrap.getBoundingClientRect(), c = cell.getBoundingClientRect(), b = button.getBoundingClientRect();
      return { cellWidth: c.width, buttonWidth: b.width, buttonHeight: b.height, visible: b.left >= w.left && b.right <= w.right + 1, label: button.getAttribute('aria-label'), svg: !!button.querySelector('svg'), sticky: getComputedStyle(cell).position };
    });
    for (const width of [1280, 760, 480]) {
      await page.setViewportSize({ width, height: 800 });
      const layout = await measure();
      assert.ok(layout.cellWidth >= 64, JSON.stringify({ width, layout }));
      assert.equal(layout.buttonWidth, 24); assert.equal(layout.buttonHeight, 24);
      assert.equal(layout.visible, true); assert.equal(layout.sticky, 'sticky');
      assert.equal(layout.label, '结束会话 941'); assert.equal(layout.svg, true);
    }
    await page.locator('.monitor-kill').focus(); await page.keyboard.press('Enter');
    await page.locator('.server-monitor-confirm').waitFor();
    assert.equal(await page.locator('.server-monitor-confirm').getAttribute('role'), 'alertdialog');
    assert.deepEqual(await page.evaluate(() => kills), []);
    await page.getByRole('button', { name: '取消', exact: true }).click();
    await page.locator('.monitor-connection').selectOption('read');
    await page.waitForFunction(() => document.querySelectorAll('.monitor-table tbody tr[data-session-id]').length === 1 && document.querySelector('.monitor-table-wrap').getAttribute('aria-busy') === 'false');
    assert.equal(await page.locator('.monitor-kill').count(), 0);
    assert.equal(await page.locator('.monitor-table col').count(), 7);
    assert.deepEqual(await page.evaluate(() => kills), []);
    await page.evaluate(() => monitor.close()); assert.deepEqual(errors, []);
  } finally { await browser.close(); }
});
