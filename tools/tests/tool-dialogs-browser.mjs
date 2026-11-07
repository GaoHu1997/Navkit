// Real browser/wizard UI with explicit mocked IPC; not a native DB acceptance test.
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright-core';
const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe', headless: true });
const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
page.setDefaultTimeout(10000);
const errors = [], passed = [];
page.on('pageerror', e => errors.push(e.message));
await page.addInitScript(() => {
  let mock;
  window.__TOOL_CALLS__ = [];
  Object.defineProperty(window, '__PREVIEW_MOCK__', { configurable: true, get: () => mock, set: value => {
    mock = value;
    mock['plugin:event|listen'] = () => 1;
    mock['plugin:event|unlisten'] = () => null;
    const base = mock.list_connections()[0];
    mock.list_connections = () => [
      { ...base, id: 'c1', name: '工具源', kind: 'mysql', params: '', read_only: false },
      { ...base, id: 'c2', name: '工具目标', kind: 'sqlite', params: '', read_only: false },
      { ...base, id: 'ro', name: '只读源', kind: 'mysql', params: '', read_only: true },
      { ...base, id: 'odbc', name: '自定义驱动', kind: 'mysql', params: '@navkit-driver:{"version":1,"driver_id":"fake","template":""}', read_only: false },
    ];
    mock.ensure_connection = args => { window.__TOOL_CALLS__.push(['ensure', args.id]); return { ok: true, message: 'fixture connected', elapsed_ms: 0, server_version: 'fixture' }; };
    mock.list_databases = () => [['main', 'database'], ['other', 'database']];
    mock.list_tables = () => [['items', 'table'], ['foreign_table', 'table'], ['fixture_view', 'view']];
    mock.get_table_schema = () => ({ columns: [{ name: 'id', type: 'INTEGER', nullable: false, primary_key: true, auto_increment: false, default: null, comment: '' }], primary_keys: ['id'], indexes: [], foreign_keys: [], ddl: 'CREATE TABLE items(id INTEGER PRIMARY KEY)' });
    mock.insert_generated_rows = args => { window.__TOOL_CALLS__.push(['write', args.req]); return args.req.rows.length; };
    mock.sync_type_mapping = () => [{ source: 'id', source_type: 'INTEGER', target_type: 'BIGINT' }];
    mock.page_table = () => ({ columns: [{ name: 'id', type: 'INTEGER', nullable: false, primary_key: true, auto_increment: false }], rows: [['1']], primary_keys: ['id'], total: 1, elapsed_ms: 0 });
    mock.list_objects = () => [{ name: 'items', type: 'table', rows: 1, size: null, engine: null, comment: '' }];
    mock.table_status = () => [];
    mock.get_table_ddl = () => 'CREATE TABLE items(id INTEGER PRIMARY KEY)';
  } });
});
const ok = label => { passed.push(label); console.log(`PASS ${label}`); };
const chooseConnection = async id => page.evaluate(id => { const app = window.__APP__; app.tree.select(`conn:${id}`); app.tree.render(); }, id);
try {
  await page.goto(`${process.env.PREVIEW_URL || 'http://localhost:5183'}/preview.html`);
  await page.waitForFunction(() => !!window.__APP__);
  // Keep a different table tab active, then explicitly choose a connection-only context.
  await page.evaluate(() => window.__APP__.openTableTab('c2', 'other', 'foreign_table', 'table'));
  await chooseConnection('c1');
  await page.evaluate(() => window.__APP__.runCommand('data-generation'));
  let dialog = page.getByRole('dialog', { name: '数据生成', exact: true });
  await dialog.waitFor();
  await page.waitForFunction(() => document.querySelector('[aria-label="目标表"]')?.options.length > 0);
  console.log('context', await page.evaluate(() => ({ selected: window.__APP__.tree.selectedNodeId, node: window.__APP__.tree.find(window.__APP__.tree.selectedNodeId)?.id, target: window.__APP__.currentDbTarget(), connections: window.__APP__.connections.map(c => c.id) })));
  assert.equal(await dialog.getByLabel('目标数据库 / 模式').inputValue(), 'main');
  assert.equal(await dialog.getByLabel('目标表', { exact: true }).inputValue(), 'items');
  assert.equal(await dialog.getByLabel('目标表', { exact: true }).locator('option[value="fixture_view"]').count(), 0);
  assert((await page.evaluate(() => window.__TOOL_CALLS__)).some(c => c[0] === 'ensure' && c[1] === 'c1'));
  ok('生成连接入口可选择真实数据库，先ensure，拒绝借用另一标签表名和视图');
  await dialog.getByRole('button', { name: '下一步', exact: true }).click();
  await dialog.getByRole('button', { name: '下一步', exact: true }).click();
  await dialog.getByRole('button', { name: '生成预览', exact: true }).click();
  const execute = dialog.getByRole('button', { name: '执行写入', exact: true }).last();
  await execute.click();
  assert.equal((await page.evaluate(() => window.__TOOL_CALLS__)).filter(c => c[0] === 'write').length, 0);
  await dialog.getByRole('checkbox', { name: /我确认将预览中的数据/ }).check();
  await execute.click();
  await page.waitForFunction(() => window.__TOOL_CALLS__.some(c => c[0] === 'write'));
  const writes = (await page.evaluate(() => window.__TOOL_CALLS__)).filter(c => c[0] === 'write');
  assert.equal(writes.length, 1); assert.equal(writes[0][1].connection_id, 'c1'); assert.equal(writes[0][1].schema, 'main'); assert.equal(writes[0][1].table, 'items'); assert.equal(writes[0][1].rows.length, 10);
  await mkdir('artifacts', { recursive: true });
  await page.screenshot({ path: 'artifacts/tool-generator-confirmation.png', fullPage: true });
  ok('真实向导预览与确认门禁、单批参数IPC和写后预览失效');
  await dialog.getByRole('button', { name: '关闭', exact: true }).last().click();
  await page.evaluate(() => window.__APP__.runCommand('data-sync'));
  dialog = page.getByRole('dialog', { name: '数据库同步', exact: true });
  await dialog.waitFor();
  await page.waitForFunction(() => [...document.querySelectorAll('[role="dialog"] select')].filter(e => e.options.length).length >= 6);
  const selects = dialog.locator('select');
  const sourceConnections = await selects.nth(0).locator('option').evaluateAll(os => os.map(o => o.value));
  const targetConnections = await selects.nth(3).locator('option').evaluateAll(os => os.map(o => o.value));
  assert(sourceConnections.includes('ro') && !sourceConnections.includes('odbc'));
  assert(!targetConnections.includes('ro') && !targetConnections.includes('odbc'));
  assert.equal(await selects.nth(0).inputValue(), 'c1');
  assert.notEqual(await selects.nth(2).inputValue(), 'foreign_table');
  await page.screenshot({ path: 'artifacts/tool-sync-context.png', fullPage: true });
  ok('同步真实向导不偷换源，过滤只读目标与ODBC，跨标签表名不预填');
  await dialog.getByRole('button', { name: '关闭', exact: true }).last().click();
  await chooseConnection('ro');
  assert.equal(await page.evaluate(() => window.__APP__.ribbonDisabled('data-generation')), true);
  assert.equal(await page.evaluate(() => window.__APP__.ribbonDisabled('data-sync')), false);
  await page.evaluate(() => window.__APP__.runCommand('data-generation'));
  assert.equal(await page.getByRole('dialog', { name: '数据生成', exact: true }).count(), 0);
  await chooseConnection('odbc');
  assert.equal(await page.evaluate(() => window.__APP__.ribbonDisabled('data-sync')), true);
  await page.evaluate(() => window.__APP__.runCommand('data-sync'));
  assert.equal(await page.getByRole('dialog', { name: '数据库同步', exact: true }).count(), 0);
  ok('只读生成与自定义ODBC工具能力禁用且直接命令报错不打开空向导');
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ mode: 'browser-mocked-ipc', passed, screenshots: ['artifacts/tool-generator-confirmation.png', 'artifacts/tool-sync-context.png'] }, null, 2));
} finally { await browser.close(); }
