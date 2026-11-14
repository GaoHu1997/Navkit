// Production build acceptance: explicit scheduler fixtures; no real databases or tasks.
import assert from 'node:assert/strict';
import { mkdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer } from 'node:http';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';

const root = fileURLToPath(new URL('../../', import.meta.url));
const dist = path.join(root, 'dist');
const html = (await readFile(path.join(dist, 'index.html'), 'utf8')).replace('<head>', '<head><script src="/preview-bootstrap.js"></script>');
const preview = await readFile(path.join(root, 'public/preview-bootstrap.js'), 'utf8');
const demo = preview.indexOf('// 自动演示：');
assert(demo > 0);
const server = createServer(async (req, res) => {
  try {
    const pathname = decodeURIComponent(new URL(req.url, 'http://127.0.0.1').pathname);
    if (pathname === '/') { res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }); res.end(html); return; }
    const file = path.resolve(dist, '.' + pathname);
    if (!file.startsWith(dist + path.sep)) { res.writeHead(403); res.end(); return; }
    res.writeHead(200, { 'Content-Type': { '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }[path.extname(file)] || 'application/octet-stream' });
    res.end(await readFile(file));
  } catch { res.writeHead(404); res.end(); }
});
await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  const page = await browser.newPage({ viewport: { width: 1855, height: 900 } });
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.route('**/preview-bootstrap.js*', route => route.fulfill({ contentType: 'text/javascript', body: preview.slice(0, demo) + '\n} catch (e) { throw e; }\n' }));
  await page.addInitScript(() => {
    window.fixtureJobs = [{ id: 'width-job', name: '每日日志清理', connection_id: 'c1', database: 'dbswitch', sql: 'SELECT 1', interval_secs: 0, cron: '0 0 8 * * MON-FRI', enabled: true, last_run: '2026-11-13 08:00:00', last_status: '成功', task: null }];
    window.fixtureHistory = [{ job_id: 'width-job', job_name: '日志清理', started_at: '2026-11-13 08:00:00', elapsed_ms: 1234, ok: true, detail: '执行完成，清理 12500 条日志记录' }];
    let mock;
    Object.defineProperty(window, '__PREVIEW_MOCK__', { configurable: true, get: () => mock, set: value => {
      mock = value;
      mock['plugin:event|listen'] = () => 1;
      mock['plugin:event|unlisten'] = () => null;
      mock.list_editing_drafts = () => [];
      mock.list_jobs = () => window.fixtureJobs;
      mock.list_job_history = () => window.fixtureHistory;
    } });
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/`, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.__APP__?.connections?.length);
  await page.evaluate(() => window.__APP__.openScheduleTab());
  await page.waitForFunction(() => window.__APP__.activeView()?.grid?.el.querySelectorAll('th[data-col]').length === 8);
  const schedule = page.locator('.grid-wrap:visible').first();
  const metrics = locator => locator.evaluate(el => {
    const scroll = el.querySelector('.grid-scroll');
    const table = el.querySelector('.grid-table');
    return { available: scroll.clientWidth, table: table.getBoundingClientRect().width,
      widths: [...table.querySelectorAll('th[data-col]')].map(th => th.getBoundingClientRect().width),
      cells: [...table.querySelectorAll('tr[data-row="0"] td[data-col]')].map(td => ({ text: td.textContent, clipped: td.scrollWidth > td.clientWidth + 1 })),
      aria: [...table.querySelectorAll('.grid-column-resize-handle')].map(h => Number(h.getAttribute('aria-valuenow'))) };
  });
  const settle = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const filled = m => assert(Math.abs(m.table - m.available) <= 1, `table ${m.table} should fill ${m.available}`);
  await settle();
  const initial = await metrics(schedule);
  filled(initial);
  assert(initial.cells.every(c => !c.clipped), JSON.stringify(initial));
  for (const width of [1600, 1855]) {
    await page.setViewportSize({ width, height: 900 }); await settle();
    filled(await metrics(schedule));
  }
  await page.setViewportSize({ width: 1250, height: 900 }); await settle();
  const narrow = await metrics(schedule);
  assert(narrow.table >= narrow.available, 'narrow containers must keep content widths and horizontal scrolling');
  assert(narrow.cells.every(c => !c.clipped));
  await page.setViewportSize({ width: 1855, height: 900 }); await settle();
  const handle = schedule.getByRole('separator', { name: '调整 任务名 列宽', exact: true });
  const beforeKey = (await metrics(schedule)).widths[0];
  await handle.focus(); await page.keyboard.press('ArrowRight');
  assert(Math.abs((await metrics(schedule)).widths[0] - beforeKey - 8) <= 1, 'first key must not jump to base width');
  await page.keyboard.press('Home');
  assert(Math.abs((await metrics(schedule)).widths[0] - beforeKey) <= 1);
  const box = await handle.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down(); await page.mouse.move(box.x + box.width / 2 + 45, box.y + box.height / 2, { steps: 5 }); await page.mouse.up();
  const dragged = (await metrics(schedule)).widths[0];
  assert(Math.abs(dragged - beforeKey - 45) <= 1);
  await page.evaluate(() => { window.fixtureJobs[0].name = '这是一条更新后的长名称自动任务'; return window.__APP__.activeView().reload(); });
  assert(Math.abs((await metrics(schedule)).widths[0] - dragged) <= 1, 'refresh must preserve user width');
  for (const width of [1600, 2000]) {
    await page.setViewportSize({ width, height: 900 }); await settle();
    const m = await metrics(schedule); filled(m);
    assert(Math.abs(m.widths[0] - dragged) <= 1);
  }
  await page.evaluate(() => { window.fixtureJobs = []; return window.__APP__.activeView().reload(); });
  filled(await metrics(schedule));
  await page.evaluate(() => { window.fixtureJobs = [{ id: 'width-job', name: '日志清理', connection_id: 'c1', database: 'dbswitch', sql: 'SELECT 1', interval_secs: 0, cron: '0 0 8,12,16,20 * * MON-FRI', enabled: true, last_run: '2026-11-13 08:00:00', last_status: '成功', task: null }]; return window.__APP__.activeView().reload(); });
  const refreshed = await metrics(schedule);
  filled(refreshed); assert(!refreshed.cells[2].clipped, 'new Cron data must be remeasured');
  await mkdir(path.join(root, 'artifacts'), { recursive: true });
  await page.screenshot({ path: path.join(root, 'artifacts/schedule-column-widths.png'), animations: 'disabled' });
  await page.evaluate(() => window.__APP__.runCommand('job-history'));
  const history = page.locator('.dialog:visible').filter({ hasText: '运行历史' });
  await history.waitFor(); await settle();
  const historyGrid = history.locator('.grid-wrap');
  const historyMetrics = await metrics(historyGrid);
  filled(historyMetrics);
  assert(historyMetrics.cells.every(c => !c.clipped), JSON.stringify(historyMetrics));
  await history.screenshot({ path: path.join(root, 'artifacts/job-history-column-widths.png'), animations: 'disabled' });
  await history.getByRole('button', { name: '关闭', exact: true }).click();
  // General DataGrid boundaries not tied to App internals: detached/hidden mount, row numbers, explicit widths and schema reset.
  const bundle = await build({ stdin: { contents: 'export { DataGrid } from "./src/ui/DataGrid.ts";', resolveDir: root, loader: 'ts' }, bundle: true, write: false, platform: 'browser', format: 'iife', globalName: 'gridFixture', loader: { '.css': 'empty' }, logLevel: 'silent' });
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  await page.evaluate(() => {
    const host = document.createElement('div'); host.id = 'width-host'; host.style.cssText = 'display:none;position:fixed;inset:100px auto auto 100px;width:1000px;height:160px;z-index:1000;';
    window.widthGrid = new gridFixture.DataGrid();
    window.widthColumns = ['任务', '详情'].map(name => ({ name, type: 'text', nullable: false, primary_key: false, auto_increment: false }));
    widthGrid.setData({ columns: widthColumns, rows: [['日志清理', '完成']], fillWidth: true, rowHeaderWidth: 48 });
    host.appendChild(widthGrid.el); document.body.appendChild(host); window.widthHost = host;
    host.style.display = 'flex';
  });
  await settle();
  const boundary = page.locator('#width-host .grid-wrap');
  filled(await metrics(boundary));
  await page.evaluate(() => widthGrid.setColWidths([180, 90]));
  filled(await metrics(boundary));
  await page.evaluate(() => widthGrid.setData({ columns: widthColumns, rows: Array.from({ length: 100 }, () => ['日志清理', '完成']), fillWidth: true }));
  await settle(); filled(await metrics(boundary));
  await page.evaluate(() => widthGrid.setData({ columns: widthColumns, rows: [['日志清理', '完成']], fillWidth: false }));
  const optedOut = await metrics(boundary);
  assert(optedOut.table < optedOut.available);
  await page.evaluate(() => { widthGrid.dispose(); widthHost.remove(); });
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ passed: ['任务列表铺满并展示常规内容', '窗口缩放铺满', '键盘/鼠标从显示宽调整', '手动宽度跨刷新和缩放保留', '空列表与新 Cron 刷新量宽', '运行历史铺满且常规详情不截断', '未挂载/隐藏后显示、行号列与垂直滚动条', '显式宽度与 fillWidth 关闭兼容'], initial, refreshed, history: historyMetrics }, null, 2));
} finally {
  try { if (browser) await browser.close(); }
  finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
}
