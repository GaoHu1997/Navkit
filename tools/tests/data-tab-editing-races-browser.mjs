// 完整 App/DataTabView 的真实 Chromium 回归。--source 使用当前完整应用模块，默认使用 dist。
// 自有随机端口与显式 IPC fixture；不连接数据库、不替代运行中的应用、不自造 onEditStart。
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer as createHttpServer } from 'node:http';
import { createServer as createViteServer } from 'vite';
import { chromium } from 'playwright-core';

const root = fileURLToPath(new URL('../../', import.meta.url));
const source = process.argv.includes('--source');
let server, browser, closeServer;
const passed = [];
try {
  if (source) {
    const vite = await createViteServer({ root, configFile: false, optimizeDeps: { noDiscovery: true, include: [] }, server: { middlewareMode: true, watch: null, hmr: false } });
    server = createHttpServer((request, response) => vite.middlewares(request, response, () => { response.writeHead(404); response.end(); }));
    closeServer = async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); await vite.close(); };
  } else {
    const dist = path.join(root, 'dist');
    const html = (await readFile(path.join(dist, 'index.html'), 'utf8')).replace('<head>', '<head><script src="/preview-bootstrap.js"></script>');
    server = createHttpServer(async (request, response) => {
      try {
        const pathname = decodeURIComponent(new URL(request.url, 'http://127.0.0.1').pathname);
        if (pathname === '/' || pathname === '/preview.html') { response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }); response.end(html); return; }
        const filename = path.resolve(dist, '.' + pathname);
        if (!filename.startsWith(dist + path.sep)) { response.writeHead(403); response.end(); return; }
        const body = await readFile(filename);
        response.writeHead(200, { 'Content-Type': { '.js': 'application/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png' }[path.extname(filename)] || 'application/octet-stream' });
        response.end(body);
      } catch { response.writeHead(404); response.end(); }
    });
    closeServer = async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); };
  }
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 760 } });
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const preview = await readFile(path.join(root, 'public/preview-bootstrap.js'), 'utf8');
  const demo = preview.indexOf('// 自动演示：');
  assert.ok(demo > 0);
  await page.route('**/preview-bootstrap.js*', route => route.fulfill({ contentType: 'text/javascript', body: preview.slice(0, demo) + '\n} catch (e) { throw e; }\n' }));
  await page.addInitScript(() => {
    let mock;
    const fixture = window.__RACE_FIXTURE__ = { delayNext: false, requests: [], writes: [], pending: null };
    Object.defineProperty(window, '__PREVIEW_MOCK__', { configurable: true, get: () => mock, set: value => {
      mock = value;
      const settings = value.get_settings;
      mock.get_settings = () => ({ ...settings(), rows_per_page: 25 });
      mock.get_draft_recovery_enabled = () => false;
      mock.list_editing_drafts = () => [];
      mock['plugin:event|listen'] = () => 1;
      mock['plugin:event|unlisten'] = () => null;
      mock.page_table = ({ req }) => {
        const sequence = fixture.requests.length + 1;
        fixture.requests.push(structuredClone(req));
        const result = { columns: [
          { name: 'id', type: 'INTEGER', nullable: false, primary_key: true, auto_increment: false },
          { name: 'team', type: 'TEXT', nullable: true, primary_key: false, auto_increment: false },
          { name: 'name', type: 'TEXT', nullable: true, primary_key: false, auto_increment: false },
        ], rows: Array.from({ length: 25 }, (_, i) => [String(req.offset + i + 1), `团队-${sequence}-${req.offset + i + 1}`, `姓名-${sequence}-${req.offset + i + 1}`]), total: 75, primary_keys: ['id'], elapsed_ms: 300 };
        if (!fixture.delayNext) return result;
        fixture.delayNext = false;
        const started = performance.now();
        return new Promise(resolve => { fixture.pending = { sequence, offset: req.offset, started, release: () => setTimeout(() => { fixture.pending = null; resolve(result); }, Math.max(0, 300 - (performance.now() - started))) }; });
      };
      for (const command of ['submit_editor_mutation', 'execute_sql_v2', 'update_row', 'insert_row', 'delete_row']) {
        mock[command] = args => { fixture.writes.push({ command, args }); throw new Error('浏览器竞态验收禁止数据库写入'); };
      }
    } });
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/preview.html`, { waitUntil: 'domcontentloaded', timeout: 30000 });
  await page.waitForFunction(() => window.__APP__?.connections?.length);
  await page.evaluate(() => window.__APP__.openTableTab('c1', 'dbswitch', 'editing_race_fixture', 'table'));
  await page.waitForFunction(() => window.__APP__.activeView()?.grid?.rows.length === 25 && !window.__APP__.activeView().loading);
  const cell = () => page.locator('.grid-table tbody tr[data-row="0"] td[data-col="1"]');
  const editor = () => page.locator('.grid-cell-input');

  for (const trigger of ['header ASC', 'pagination']) {
    const old = await page.evaluate(() => ({ row: [...window.__APP__.activeView().grid.rows[0]], requests: window.__RACE_FIXTURE__.requests.length }));
    await page.evaluate(() => { window.__RACE_FIXTURE__.delayNext = true; });
    if (trigger === 'header ASC') {
      await page.locator('.grid-table thead th[data-col="1"]').click({ button: 'right' });
      await page.getByText('ASC 升序排序', { exact: true }).click();
    } else {
      await page.locator('.grid-pager button[title="下一页"]').click();
    }
    await page.waitForFunction(() => window.__RACE_FIXTURE__.pending && window.__APP__.activeView().loading);
    const inFlight = await page.evaluate(() => ({ row: [...window.__APP__.activeView().grid.rows[0]], offset: window.__RACE_FIXTURE__.pending.offset, sequence: window.__RACE_FIXTURE__.pending.sequence }));
    assert.deepEqual(inFlight.row, old.row, `${trigger}: 请求未回时仍显示旧页供重现`);
    await cell().dblclick();
    assert.equal(await editor().count(), 0, `${trigger}: 真实双击不能在旧页打开 editor`);
    await page.keyboard.press('F2');
    assert.equal(await editor().count(), 0, `${trigger}: 真实 F2 不能在旧页打开 editor`);
    await page.keyboard.press('Enter');
    assert.equal(await editor().count(), 0, `${trigger}: 真实 Enter 不能在旧页打开 editor`);
    const blocked = await page.evaluate(() => {
      const view = window.__APP__.activeView();
      return { loading: view.loading, active: view.grid.editingActive, pendingUpdates: view.pendingUpdates.size, row: [...view.grid.rows[0]], writes: window.__RACE_FIXTURE__.writes.length, requests: window.__RACE_FIXTURE__.requests.length, message: document.body.textContent.includes('表格正在载入或提交，请稍候') };
    });
    assert.equal(blocked.loading, true);
    assert.equal(blocked.active, false);
    assert.equal(blocked.pendingUpdates, 0);
    assert.equal(blocked.writes, 0);
    assert.equal(blocked.requests, old.requests + 1);
    assert.equal(blocked.message, true, `${trigger}: 门禁说明应可见`);
    assert.deepEqual(blocked.row, old.row, `${trigger}: 三种尝试都不能改变旧页`);
    await page.evaluate(() => window.__RACE_FIXTURE__.pending.release());
    await page.waitForFunction(() => !window.__APP__.activeView().loading && !window.__RACE_FIXTURE__.pending);
    const settled = await page.evaluate(() => ({ row: [...window.__APP__.activeView().grid.rows[0]], pendingUpdates: window.__APP__.activeView().pendingUpdates.size }));
    const fresh = [String(inFlight.offset + 1), `团队-${inFlight.sequence}-${inFlight.offset + 1}`, `姓名-${inFlight.sequence}-${inFlight.offset + 1}`];
    assert.deepEqual(settled.row, fresh, `${trigger}: 延迟回包应完整应用新页`);
    assert.equal(settled.pendingUpdates, 0, `${trigger}: 不能把旧值当成新页草稿`);
    await cell().dblclick();
    assert.equal(await editor().inputValue(), fresh[1], `${trigger}: 结束载入后可以编辑新页真实 team`);
    const draft = `新页草稿-${trigger}`;
    await editor().fill(draft);
    await page.keyboard.press('Enter');
    await page.waitForFunction(value => window.__APP__.activeView().pendingUpdates.get(0)?.get('team') === value, draft);
    const edited = await page.evaluate(() => {
      const view = window.__APP__.activeView();
      return { entries: [...view.pendingUpdates.get(0)], source: [...view.tab.page.rows[0]], shown: [...view.grid.rows[0]], writes: window.__RACE_FIXTURE__.writes.length };
    });
    assert.deepEqual(edited.entries, [['team', draft]], `${trigger}: 只按新页 team 身份记草稿`);
    assert.deepEqual(edited.source, fresh, `${trigger}: 原始新页快照没有被草稿篡改`);
    assert.deepEqual(edited.shown, [fresh[0], draft, fresh[2]]);
    assert.equal(edited.writes, 0);
    passed.push(`${trigger} 300ms+ delayed IPC：完整 App 的 dblclick/F2/Enter 均被加载门禁拦截，回包后新页可编辑且字段身份正确`);
    await page.evaluate(() => window.__APP__.activeView().rollbackEdits());
  }
  // 真实活编辑稿遇到 header sort，应保留成原页字段草稿并阻止翻页请求。
  const beforeDraft = await page.evaluate(() => window.__RACE_FIXTURE__.requests.length);
  await cell().dblclick();
  await editor().fill('原页活编辑稿');
  await page.locator('.grid-table thead th[data-col="2"]').click({ button: 'right' });
  await page.getByText('ASC 升序排序', { exact: true }).click();
  const liveDraft = await page.evaluate(() => ({ requests: window.__RACE_FIXTURE__.requests.length, edits: [...window.__APP__.activeView().pendingUpdates.get(0)], shown: window.__APP__.activeView().grid.rows[0][1] }));
  assert.equal(liveDraft.requests, beforeDraft);
  assert.deepEqual(liveDraft.edits, [['team', '原页活编辑稿']]);
  assert.equal(liveDraft.shown, '原页活编辑稿');
  passed.push('已有活编辑稿尝试排序：保留原页 team 草稿并阻止发出分页请求');
  assert.deepEqual(await page.evaluate(() => window.__RACE_FIXTURE__.writes), []);
  assert.deepEqual(errors, []);
  await mkdir(path.join(root, 'artifacts'), { recursive: true });
  const screenshot = `artifacts/data-tab-editing-races-${source ? 'source' : 'production'}.png`;
  await page.screenshot({ path: path.join(root, screenshot), animations: 'disabled' });
  console.log(JSON.stringify({ mode: source ? 'full-current-App-source-mocked-ipc' : 'production-full-App-mocked-ipc', passed, screenshot }, null, 2));
} finally {
  try { await browser?.close(); }
  finally { if (closeServer) await closeServer(); }
}
