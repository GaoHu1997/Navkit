// Production UI acceptance: delayed fixture IPC only; no live database operations.
import assert from 'node:assert/strict';
import { mkdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer } from 'node:http';
import { chromium } from 'playwright-core';

const root = fileURLToPath(new URL('../../', import.meta.url));
const dist = path.join(root, 'dist');
const html = (await readFile(path.join(dist, 'index.html'), 'utf8')).replace('<head>', '<head><script src="/preview-bootstrap.js"></script>');
const server = createServer(async (request, response) => {
  try {
    const pathname = decodeURIComponent(new URL(request.url, 'http://127.0.0.1').pathname);
    if (pathname === '/') { response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }); response.end(html); return; }
    const filename = path.resolve(dist, '.' + pathname);
    if (!filename.startsWith(dist + path.sep)) { response.writeHead(403); response.end(); return; }
    response.writeHead(200, { 'Content-Type': { '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }[path.extname(filename)] || 'application/octet-stream' });
    response.end(await readFile(filename));
  } catch { response.writeHead(404); response.end(); }
});
await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  page.setDefaultTimeout(7000);
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  const preview = await readFile(path.join(root, 'public/preview-bootstrap.js'), 'utf8');
  const demo = preview.indexOf('// 自动演示：'); assert(demo > 0);
  await page.route('**/preview-bootstrap.js*', route => route.fulfill({ contentType: 'text/javascript', body: preview.slice(0, demo) + '\n} catch (e) { throw e; }\n' }));
  await page.addInitScript(() => {
    let mock;
    window.requests = [];
    Object.defineProperty(window, '__PREVIEW_MOCK__', { configurable: true, get: () => mock, set: value => {
      mock = value; mock['plugin:event|listen'] = () => 1; mock['plugin:event|unlisten'] = () => null;
      mock.list_editing_drafts = () => [];
      mock.page_table = args => new Promise((resolve, reject) => window.requests.push({ req: args.req, resolve, reject }));
    } });
    window.resolvePage = index => {
      const pending = window.requests[index];
      const names = ['id', 'tradeType', 'applyStatus', 'tradeStatus', ...Array.from({ length: 150 }, (_, i) => 'extra_' + i)];
      pending.resolve({ columns: names.map(name => ({ name, type: 'INTEGER', nullable: false, primary_key: name === 'id', auto_increment: false })),
        rows: Array.from({ length: 20 }, (_, i) => names.map((_, c) => c === 0 ? String(pending.req.offset + i + 1) : c === 1 ? '130' : '3')),
        total: 200, total_exact: true, has_more: pending.req.offset + 20 < 200, primary_keys: ['id'], elapsed_ms: 1 });
    };
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/`, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.__APP__?.connections?.length);
  await page.evaluate(() => { void window.__APP__.openTableTab('c1', 'dbswitch', 'interaction_fixture', 'table'); });
  await page.waitForFunction(() => requests.length === 1);
  assert.match(await page.locator('.data-load-status').textContent(), /正在加载当前页.*返回后自动更新/);
  await page.evaluate(() => resolvePage(0));
  await page.waitForFunction(() => !window.__APP__.activeView().loading);
  await page.evaluate(() => { const v = window.__APP__.activeView(); v.tab.pageLimit = 20; v.loadedPageRequest.limit = 20; v.renderFooter(v.tab.page); });
  const record = () => page.locator('.statusbar-query-elapsed + .statusbar-cell').textContent();
  // Cell selectors are scoped through the actual grid host, not the tab class convention.
  const clickRow = async row => {
    await page.locator(`td[data-row="${row}"][data-col="0"]`).last().click();
  };
  await clickRow(4); assert.match(await record(), /^第 5 条记录/);
  await clickRow(2); assert.match(await record(), /^第 3 条记录/);
  await page.locator('td[data-row="7"][data-col="0"]').last().click({ modifiers: ['Control'] });
  assert.match(await record(), /^第 8 条记录/, 'multi-selection must report the last clicked row, not the first selected');

  // Actual filter field search, then click Apply once while response is deliberately withheld.
  await page.evaluate(() => {
    const v = window.__APP__.activeView(); v.focusFilter();
    v.filterLines = [{ conds: [{ col: 'id', op: '=', val: '130', touched: true }] }]; v.renderFilterLines();
  });
  await page.getByLabel('筛选字段 1.1', { exact: true }).click();
  await page.locator('.searchable-select-search').fill('tradetype');
  assert.deepEqual(await page.locator('.searchable-select-option').allTextContents(), ['tradeType']);
  await page.locator('.searchable-select-search').press('Enter');
  assert.equal(await page.getByLabel('筛选字段 1.1', { exact: true }).inputValue(), 'tradeType');
  const applyFilter = page.getByRole('button', { name: '应用筛选', exact: true });
  await applyFilter.click();
  await page.waitForFunction(() => requests.length === 2);
  assert.equal(await applyFilter.isDisabled(), true);
  assert.equal(await applyFilter.textContent(), '应用筛选中…');
  assert.match(await page.locator('.data-load-status').textContent(), /正在应用筛选.*下方暂为上次结果/);
  assert.match(await record(), /正在应用筛选/);
  assert.equal(await page.evaluate(() => window.__APP__.activeView().grid.el.inert), true);
  assert.equal(await page.locator('tbody').last().evaluate(el => getComputedStyle(el).opacity), '0.55');
  await mkdir(path.join(root, 'artifacts'), { recursive: true });
  await page.screenshot({ path: path.join(root, 'artifacts/data-grid-filter-loading.png'), fullPage: true, animations: 'disabled' });
  // Deliberately exercise disabled/stale event paths in addition to the real first click.
  await applyFilter.dispatchEvent('click');
  await page.evaluate(() => { const v = window.__APP__.activeView(); void v.applyFilters(); void v.applySort(); });
  assert.equal(await page.evaluate(() => requests.length), 2);
  assert.equal(await page.evaluate(() => requests[1].req.filters[0].conds[0].column), 'tradeType');
  await page.evaluate(() => resolvePage(1));
  await page.waitForFunction(() => !window.__APP__.activeView().loading);
  assert.equal(await applyFilter.isDisabled(), false);
  assert.equal(await applyFilter.textContent(), '应用筛选');
  assert.match(await page.locator('.data-load-status').textContent(), /筛选已应用/);
  assert.match(await record(), /^第 1 条记录/);

  // Long-list sort field search and slow response failure retain old applied order + editable draft.
  await page.evaluate(() => {
    const v = window.__APP__.activeView(); v.focusSort();
    v.sortRows = [{ column: 'id', ascending: true }]; v.renderSortRows();
  });
  await page.getByLabel('排序字段 1', { exact: true }).click();
  await page.locator('.searchable-select-search').fill('extra_111');
  assert.deepEqual(await page.locator('.searchable-select-option').allTextContents(), ['extra_111']);
  await page.locator('.searchable-select-search').press('Enter');
  const applySort = page.getByRole('button', { name: '应用排序', exact: true });
  await applySort.click();
  await page.waitForFunction(() => requests.length === 3);
  assert.equal(await applySort.textContent(), '应用排序中…'); assert.equal(await applySort.isDisabled(), true);
  assert.match(await page.locator('.data-load-status').textContent(), /正在应用排序/);
  await applySort.dispatchEvent('click');
  assert.equal(await page.evaluate(() => requests.length), 3);
  await page.evaluate(() => requests[2].reject(new Error('offline fixture')));
  await page.waitForFunction(() => !window.__APP__.activeView().loading);
  assert.match(await page.locator('.data-load-status').textContent(), /失败.*重试.*上次结果/);
  assert.equal(await applySort.isDisabled(), false);
  assert.deepEqual(await page.evaluate(() => window.__APP__.activeView().tab.order_rules ?? []), []);
  assert.equal(await page.getByLabel('排序字段 1', { exact: true }).inputValue(), 'extra_111');
  await applySort.click(); await page.waitForFunction(() => requests.length === 4);
  await page.evaluate(() => resolvePage(3)); await page.waitForFunction(() => !window.__APP__.activeView().loading);
  assert.match(await page.locator('.data-load-status').textContent(), /排序已应用/);
  // Wait for the deliberately induced failure toast to expire before the pagination capture.
  await page.getByText('offline fixture', { exact: false }).waitFor({ state: 'hidden', timeout: 15000 });

  // Real pagination click; preserve a stale button to verify it cannot enqueue a second request.
  const next = page.getByRole('button', { name: '下一页', exact: true });
  await next.evaluate(el => { window.staleNext = el; });
  await next.click(); await page.waitForFunction(() => requests.length === 5);
  assert.equal(await next.isDisabled(), true); assert.equal(await page.getByLabel('当前页码', { exact: true }).isDisabled(), true);
  assert.match(await page.locator('.data-load-status').textContent(), /正在加载第 2 页/);
  await page.screenshot({ path: path.join(root, 'artifacts/data-grid-page-loading.png'), fullPage: true, animations: 'disabled' });
  await page.evaluate(() => { staleNext.click(); staleNext.dispatchEvent(new Event('click')); });
  assert.equal(await page.evaluate(() => requests.length), 5);
  assert.equal(await page.evaluate(() => requests[4].req.offset), 20);
  await page.evaluate(() => resolvePage(4)); await page.waitForFunction(() => !window.__APP__.activeView().loading);
  await clickRow(6); assert.match(await record(), /^第 27 条记录/);
  // Failed next-page load must restore page number and row ordinal to the result still shown.
  await next.click(); await page.waitForFunction(() => requests.length === 6);
  await page.evaluate(() => requests[5].reject(new Error('page fixture failure')));
  await page.waitForFunction(() => !window.__APP__.activeView().loading);
  assert.equal(await page.getByLabel('当前页码', { exact: true }).inputValue(), '2');
  assert.match(await record(), /^第 27 条记录/);
  assert.equal(await page.evaluate(() => window.__APP__.activeView().grid.el.inert), false);
  await clickRow(3); assert.match(await record(), /^第 24 条记录/);
  // Form navigation is the same current-record state, including after returning to grid.
  await page.getByRole('button', { name: '表单视图', exact: true }).click();
  await page.getByRole('button', { name: '下一条', exact: true }).click();
  assert.match(await record(), /^第 25 条记录/);
  await page.getByRole('button', { name: '网格视图', exact: true }).click();
  assert.match(await record(), /^第 25 条记录/);
  const narrow = await page.evaluate(() => ({ scroll: document.documentElement.scrollWidth, width: innerWidth }));
  assert(narrow.scroll <= narrow.width);
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ mode: 'production-ui-delayed-mock-ipc', requests: await page.evaluate(() => requests.length),
    passed: ['点击即时反馈/禁用与去重', '查询期间旧结果不可编辑且变淡', '筛选排序字段输入搜索', '失败保留旧结果与草稿并支持重试', '翻页旧按钮去重与失败页码恢复', '单选/多选/分页记录序号跟随最后点击行', '表单导航与返回网格序号同步'], narrow }, null, 2));
} finally {
  try { if (browser) await browser.close(); }
  finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
}
