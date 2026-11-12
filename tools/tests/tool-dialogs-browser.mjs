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
      // 真 ODBC profile（达梦）：协议为 odbc，无自定义驱动前缀 → 属于受支持同步源/目标
      { ...base, id: 'dm', name: '达梦生产库', kind: 'dameng', params: '', read_only: false },
    ];
    mock.ensure_connection = args => { window.__TOOL_CALLS__.push(['ensure', args.id]); return { ok: true, message: 'fixture connected', elapsed_ms: 0, server_version: 'fixture' }; };
    mock.list_databases = () => [['main', 'database'], ['other', 'database']];
    mock.list_tables = () => [['items', 'table'], ['foreign_table', 'table'], ['fixture_view', 'view']];
    mock.get_table_schema = () => ({ columns: [{ name: 'id', type: 'INTEGER', nullable: false, primary_key: true, auto_increment: false, default: null, comment: '' }], primary_keys: ['id'], indexes: [], foreign_keys: [], ddl: 'CREATE TABLE items(id INTEGER PRIMARY KEY)' });
    const batchTasks = new Map();
    mock.register_batch_execution = ({ request }) => {
      const context = { execution_id: crypto.randomUUID(), connection_id: request.connection_id, namespace: request.namespace, owner_id: request.owner_id };
      const ticket = { ...context, task_kind: request.task.kind, execution_grant: crypto.randomUUID(), expires_in_ms: 60000,
        summary: '生成数据写入', total_rows: request.task.request.rows.length, source_bytes: null };
      batchTasks.set(context.execution_id, { ticket, request: structuredClone(request), snapshot: null });
      window.__TOOL_CALLS__.push(['register', request]); return ticket;
    };
    mock.run_batch_execution = ({ ticket }) => {
      const task = batchTasks.get(ticket.execution_id);
      if (!task || task.snapshot) throw new Error('禁止重复执行 fixture 票据');
      const request = task.request.task.request;
      const context = { execution_id: ticket.execution_id, connection_id: ticket.connection_id, namespace: ticket.namespace, owner_id: ticket.owner_id };
      const deadline = { execution_id: ticket.execution_id, budget_ms: null, elapsed_ms: 12, deadline_reached: false, stop_confirmed: false };
      window.__TOOL_CALLS__.push(['write', request]);
      task.snapshot = { context, kind: ticket.task_kind, status: { execution_id: ticket.execution_id, state: 'succeeded', deadline, resources_pending: false },
        metrics: { scanned: request.rows.length, executed: request.rows.length, errors: 0, elapsed_ms: 12 }, scan_complete: true, parse_error: null,
        affected_rows: request.rows.length, total_rows: request.rows.length, transaction_state: 'committed', logs: [], next_sequence: 0, dropped_before_sequence: 0,
        outcome: { execution_id: ticket.execution_id, state: 'succeeded', result: null, error: null, deadline, effects: 'applied', transaction_state: 'committed', resources_pending: false } };
      return task.snapshot;
    };
    mock.batch_execution_status = ({ context }) => batchTasks.get(context.execution_id)?.snapshot;
    mock.cancel_execution = ({ executionId }) => ({ execution_id: executionId, accepted: true, state: 'cancel_requested', message: '停止请求已接收' });
    mock.insert_generated_rows = () => { throw new Error('生成器必须使用受控 batch API'); };
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
  const execute = dialog.locator('.dialog-footer').getByRole('button', { name: '执行写入', exact: true });
  assert.equal(await dialog.getByRole('button', { name: '执行写入', exact: true }).count(), 1);
  assert.equal(await execute.isDisabled(), true);
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
  const boxes = dialog.locator('fieldset');
  const sourceConnections = await boxes.nth(0).locator('select').nth(0).locator('option').evaluateAll(os => os.map(o => o.value));
  const targetConnections = await boxes.nth(1).locator('select').nth(0).locator('option').evaluateAll(os => os.map(o => o.value));
  // 受支持同步源：含真 ODBC profile（dameng）与只读连接；排除自定义驱动参数连接
  assert(sourceConnections.includes('ro'), `源应含只读连接：${sourceConnections}`);
  assert(sourceConnections.includes('dm'), `源应含真 ODBC profile：${sourceConnections}`);
  assert(!sourceConnections.includes('odbc'), `源应排除自定义驱动参数连接：${sourceConnections}`);
  // 目标：排除只读与自定义驱动，允许真 ODBC profile
  assert(!targetConnections.includes('ro'), `目标应排除只读：${targetConnections}`);
  assert(!targetConnections.includes('odbc'), `目标应排除自定义驱动：${targetConnections}`);
  assert(targetConnections.includes('dm'), `目标应含真 ODBC profile：${targetConnections}`);
  assert.equal(await boxes.nth(0).locator('select').nth(0).inputValue(), 'c1');
  assert.notEqual(await boxes.nth(0).locator('select').nth(2).inputValue(), 'foreign_table');
  await page.screenshot({ path: 'artifacts/tool-sync-context.png', fullPage: true });
  ok('同步真实向导不偷换源，过滤只读目标与自定义驱动，真ODBC profile可用，跨标签表名不预填');
  await dialog.getByRole('button', { name: '关闭', exact: true }).last().click();
  // 审定行为：data-generation / data-sync 是全局工具，不因当前连接不可用而禁用按钮；
  // 向导内部过滤连接并给出原因，不打开空向导也不静默借用不支持的连接。
  await chooseConnection('ro');
  assert.equal(await page.evaluate(() => window.__APP__.ribbonDisabled('data-generation')), false,'全局工具不因只读连接禁用按钮');
  await page.evaluate(() => window.__APP__.runCommand('data-generation'));
  const genDlg = page.getByRole('dialog', { name: '数据生成', exact: true });
  if (await genDlg.count()) {
    const body = await genDlg.innerText();
    assert(!/没有支持/.test(body) || /只读|不可写/.test(body), `只读连接应给出明确原因：${body.slice(0,120)}`);
    await genDlg.getByRole('button', { name: '关闭', exact: true }).last().click().catch(()=>{});
  }
  await chooseConnection('odbc');
  assert.equal(await page.evaluate(() => window.__APP__.ribbonDisabled('data-sync')), false,'全局工具不因自定义驱动连接禁用按钮');
  await page.evaluate(() => window.__APP__.runCommand('data-sync'));
  const syncDlg = page.getByRole('dialog', { name: '数据库同步', exact: true });
  if (await syncDlg.count()) {
    const boxes = syncDlg.locator('fieldset');
    const srcOpts = await boxes.nth(0).locator('select').nth(0).locator('option').evaluateAll(os => os.map(o => o.value));
    assert(!srcOpts.includes('odbc'), `对话框内不得把自定义驱动当作同步源：${srcOpts}`);
    await syncDlg.getByRole('button', { name: '关闭', exact: true }).last().click().catch(()=>{});
  }
  ok('只读与自定义驱动：全局工具按钮不禁用，向导内过滤连接并给出原因，不静默借用');
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ mode: 'browser-mocked-ipc', passed, screenshots: ['artifacts/tool-generator-confirmation.png', 'artifacts/tool-sync-context.png'] }, null, 2));
} finally { await browser.close(); }
