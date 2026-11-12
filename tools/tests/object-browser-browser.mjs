// 使用已有预览和明确的模拟 IPC；不启动服务、不读取用户数据、不执行真实 SQL。
import assert from 'node:assert/strict';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
const base = process.env.PREVIEW_URL || 'http://localhost:5183';
const root = new URL('../..', import.meta.url);
const out = fileURLToPath(new URL('../../artifacts/object-browser-20261108/', import.meta.url));
await mkdir(out, { recursive: true });
const bootstrap = await readFile(new URL('public/preview-bootstrap.js', root), 'utf8');
const marker = bootstrap.indexOf('// 自动演示：'); assert(marker > 0);
const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
const page = await browser.newPage({ viewport: { width: 1423, height: 900 } });
const errors = [], checks = [];
page.on('pageerror', error => errors.push(error.message));
await page.route('**/preview-bootstrap.js*', route => route.fulfill({ contentType: 'text/javascript', body: bootstrap.slice(0, marker) + '\n} catch (error) { throw error; }\n' }));
await page.route('**/src/**', route => { const url = new URL(route.request().url()); url.searchParams.set('object-review', '20261108'); return route.continue({ url: url.href }); });
await page.addInitScript(() => {
  let mock;
  window.__OBJECT_FIXTURE__ = { fail: false, sqlCalls: [], favorites: [
    { id: 'qa', name: '本库查询', sql: "SELECT '中文原文';\r\n-- 不执行", connection_id: 'objects-fixture', schema: 'one', timestamp: 1730000000000 },
    { id: 'qb', name: '其它库查询', sql: 'SELECT 2;', connection_id: 'objects-fixture', schema: 'two', timestamp: 1730000000000 },
    { id: 'old', name: '旧查询', sql: 'SELECT legacy;', connection_id: 'objects-fixture', timestamp: 1730000000000 },
  ] };
  Object.defineProperty(window, '__PREVIEW_MOCK__', { configurable: true, get: () => mock, set(value) {
    mock = value;
    const f = window.__OBJECT_FIXTURE__;
    const connection = { id: 'objects-fixture', name: '对象验收连接', kind: 'mysql', host: 'fixture.invalid', port: 3306, user: 'fixture', password: '', database: 'one', file: '', params: '', group: '', read_only: false, keep_alive: false };
    mock.list_connections = () => [connection]; mock.list_groups = () => [];
    mock.list_databases = () => [['one', 'database'], ['two', 'database']];
    mock.list_favorites = () => structuredClone(f.favorites);
    mock.add_favorite = ({ favorite }) => { if (f.fail) throw new Error('模拟磁盘写入失败'); f.favorites = [favorite, ...f.favorites.filter(x => x.id !== favorite.id)]; return structuredClone(f.favorites); };
    mock.remove_favorite = ({ id }) => { if (f.fail) throw new Error('模拟删除落盘失败'); f.favorites = f.favorites.filter(x => x.id !== id); return structuredClone(f.favorites); };
    mock.list_objects = () => [
      { name: 'customers', type: 'table', autoIncrement: 100, updateTime: '2026-11-08', dataLength: 4096, engine: 'InnoDB', rows: 99, comment: '客户表' },
      { name: 'active_customers', type: 'view', autoIncrement: 0, updateTime: '', dataLength: 0, engine: '', rows: 0, comment: '' },
    ];
    mock.list_routines = () => [['function', 'customer_total'], ['procedure', 'refresh_customers']];
    mock.get_view_definition = () => 'CREATE VIEW `one`.`active_customers` AS SELECT 1';
    mock.get_routine_definition = ({ routineType }) => routineType === 'procedure' ? 'CREATE PROCEDURE p() SELECT 1' : 'CREATE FUNCTION f() RETURNS INTEGER RETURN 1';
    mock.execute_sql = args => { f.sqlCalls.push(args); throw new Error('验收不得执行查询'); };
    mock.execute_sql_v2 = args => { f.sqlCalls.push(args); throw new Error('验收不得执行查询'); };
    mock.execution_capabilities = () => ({ version: 1, protocol: 'mysql', read_only: false,
      arbitrary_sql: { level: 'supported', reason: 'fixture' }, cancel: { level: 'unsupported', reason: '验收不执行 SQL' }, deadline: { level: 'unsupported', reason: 'fixture' }, manual_transaction: { level: 'supported', reason: 'fixture' }, safe_browse: { level: 'supported', reason: 'fixture' } });
    mock.execute_ddl = args => { f.sqlCalls.push(args); return 1; };
    mock['plugin:event|listen'] = () => 1; mock['plugin:event|unlisten'] = () => null;
    mock.get_draft_recovery_enabled = () => false; mock.list_editing_drafts = () => [];
  } });
});
try {
  // 实际预览模块源码与本次工作树一致，避免缓存旧代码。
  const response = await fetch(`${base}/src/core/App.ts?object-review=20261108`);
  const transformed = await response.text();
  const map = JSON.parse(Buffer.from(transformed.match(/sourceMappingURL=data:application\/json;base64,([A-Za-z0-9+/=]+)/)[1], 'base64').toString());
  const hash = text => createHash('sha256').update(text).digest('hex');
  assert.equal(hash(map.sourcesContent[0]), hash(await readFile(new URL('src/core/App.ts', root))));
  await page.goto(`${base}/preview.html`, { waitUntil: 'networkidle' });
  await page.waitForFunction(() => window.__APP__?.connections?.[0]?.id === 'objects-fixture');
  const show = type => page.evaluate(type => window.__APP__.openObjectsTab('objects-fixture', 'one', type), type);
  await show('query'); await page.waitForFunction(() => document.querySelector('.object-browser')?.getAttribute('aria-busy') === 'false');
  assert.equal(await page.locator('.object-browser .grid-table tbody tr').count(), 1);
  assert.equal(await page.getByRole('button', { name: /未归属|归属到本库/ }).count(), 0);
  assert.equal(await page.locator('.object-browser').getByText('旧查询', { exact: true }).count(), 0);
  checks.push('查询对象只显示本库已保存 SQL，没有未归属状态或归属操作');
  await page.locator('.object-browser').getByText('本库查询', { exact: true }).dblclick();
  await page.waitForFunction(() => window.__APP__.activeTab?.savedQueryId === 'qa');
  assert.equal(await page.evaluate(() => window.__APP__.activeTab.sql), "SELECT '中文原文';\r\n-- 不执行");
  assert.equal(await page.evaluate(() => window.__OBJECT_FIXTURE__.sqlCalls.length), 0); checks.push('查询双击恢复原文、名称和归属，零 SQL 执行');
  await page.evaluate(() => { const app = window.__APP__; app.views.get(app.activeTab.id).editor.setSql('SELECT edited;'); window.__OBJECT_FIXTURE__.fail = true; return app.runCommand('save'); });
  let dialog = page.getByRole('dialog', { name: '保存查询', exact: true });
  await dialog.getByRole('button', { name: '保存', exact: true }).click(); await dialog.getByText(/模拟磁盘写入失败/).waitFor();
  assert.equal(await dialog.getByLabel('查询名称').getAttribute('aria-invalid'), 'true');
  assert(await page.evaluate(() => window.__APP__.activeTab.dirty));
  await page.evaluate(() => { window.__OBJECT_FIXTURE__.fail = false; });
  await dialog.getByRole('button', { name: '保存', exact: true }).click(); await dialog.waitFor({ state: 'detached' });
  assert.equal(await page.evaluate(() => window.__APP__.activeTab.savedQueryId), 'qa'); checks.push('保存失败保留 dirty 和对话框，重试稳定 ID 更新');
  for (const type of ['table', 'view', 'function', 'query']) {
    await show(type); await page.waitForFunction(() => document.querySelector('.object-browser')?.getAttribute('aria-busy') === 'false');
    const headers = await page.locator('.object-browser th').allTextContents();
    if (type !== 'table') assert(!headers.some(text => /引擎|递增|行数/.test(text)));
    await page.screenshot({ path: `${out}/${type}-1423x900.png` });
    checks.push(`${type} 分类列及工具栏截图`);
  }
  await page.setViewportSize({ width: 920, height: 720 }); await show('function');
  assert(await page.getByRole('button', { name: '新建函数', exact: true }).isVisible());
  assert(await page.getByRole('button', { name: '新建存储过程', exact: true }).isVisible());
  await page.screenshot({ path: `${out}/functions-920x720.png` }); checks.push('窄窗保留新建函数和存储过程入口');
  await show('view'); await page.locator('.object-browser').getByText('active_customers', { exact: true }).click();
  await page.evaluate(() => { const app = window.__APP__; app.connections[0].read_only = true; app.notifyToolbar(); });
  assert(await page.getByRole('button', { name: '删除视图', exact: true }).isDisabled());
  await page.evaluate(() => { const app = window.__APP__; app.connections[0].read_only = false; app.notifyToolbar(); }); checks.push('只读视图禁用数据库删除');
  await show('view'); await page.getByRole('button', { name: '新建视图', exact: true }).click();
  dialog = page.getByRole('dialog', { name: '新建视图', exact: true });
  await dialog.getByLabel('视图名称').fill('new_view'); await dialog.getByRole('button', { name: '进入设计器' }).click();
  assert(await page.evaluate(() => window.__APP__.activeTab.designCreating)); assert.equal(await page.evaluate(() => window.__OBJECT_FIXTURE__.sqlCalls.length), 0);
  assert(await page.getByRole('button', { name: '重新载入定义', exact: true }).isDisabled());
  checks.push('新建视图进入真实创建模式，保存前零 DDL');
  assert.deepEqual(errors, []);
  await writeFile(`${out}/report.json`, JSON.stringify({ status: 'passed', mode: 'actual-product-with-mocked-ipc', checks, pageErrors: errors }, null, 2));
  console.log(JSON.stringify({ passed: checks.length, pageErrors: errors.length, artifacts: out }));
} finally { await browser.close(); }
