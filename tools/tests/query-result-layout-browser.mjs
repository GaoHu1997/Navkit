// Browser layout/interaction regression against built UI and explicit mock IPC.
// Does not start a server or claim native database acceptance.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import { chromium } from 'playwright-core';
const root = process.cwd();
const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
const page = await browser.newPage({ viewport: { width: 1600, height: 900 } });
page.setDefaultTimeout(7000);
const errors = [];
page.on('pageerror', error => errors.push(error.message));
try {
  let html = await fs.readFile(path.join(root, 'dist/index.html'), 'utf8');
  html = html.replace('<head>', '<head><script src="/preview-bootstrap.js"></script>');
  let bootstrap = await fs.readFile(path.join(root, 'public/preview-bootstrap.js'), 'utf8');
  bootstrap = bootstrap.slice(0, bootstrap.indexOf('// 自动演示：')) + `
} catch (e) { console.error(e); }`;
  await page.addInitScript(() => {
    let mock;
    Object.defineProperty(window, '__PREVIEW_MOCK__', { configurable: true, get: () => mock, set: value => {
      mock = value; mock['plugin:event|listen'] = () => 1; mock['plugin:event|unlisten'] = () => null;
    } });
  });
  await page.route('http://localhost/**', async route => {
    const url = new URL(route.request().url());
    if (url.pathname === '/') return route.fulfill({ contentType: 'text/html', body: html });
    if (url.pathname === '/preview-bootstrap.js') return route.fulfill({ contentType: 'text/javascript', body: bootstrap });
    const file = path.join(root, 'dist', url.pathname);
    try { await route.fulfill({ contentType: file.endsWith('.css') ? 'text/css' : file.endsWith('.js') ? 'text/javascript' : 'application/octet-stream', body: await fs.readFile(file) }); }
    catch { await route.fulfill({ status: 404, body: 'Not found' }); }
  });
  await page.goto('http://localhost/');
  await page.waitForFunction(() => !!window.__APP__);
  await page.evaluate(async () => {
    window.__PAGER_CALLS__ = [];
    const mock = window.__PREVIEW_MOCK__;
    // 这个 fixture 验证的是本次执行缓存（spool）分页，不是服务器分页规划。
    mock.plan_query_pagination = () => ({ mode: 'unsupported', sql: null, reason: 'fixture uses spool pagination' });
    mock.count_query_page = () => ({ supported: false, total: null, reason: 'fixture spool pagination' });
    mock.plan_query_keyset = () => ({ supported: false, reason: 'fixture spool pagination' });
    mock.execute_sql = args => {
      window.__PAGER_CALLS__.push(['sql', args.sql]);
      return { elapsed_ms: 456, messages: [], spool_id: crypto.randomUUID(), sets: [{
        columns: ['id', 'name', 'created_at', 'amount'].map(name => ({ name, data_type: 'TEXT' })),
        rows: Array.from({ length: 10000 }, (_, i) => [String(i + 1), 'name_' + String(i + 1).padStart(4, '0'), '2026-01-02 12:00:00', '1.3700']),
        total: 100001, affected: 0,
        message: '查询已完成，共 100001 行；当前返回前 10000 行的内存预览。查询页可分页读取本次执行缓存，导出默认包含全部行，不会重新执行 SQL。',
      }] };
    };
    mock.fetch_query_rows = args => {
      window.__PAGER_CALLS__.push(['page', args.offset]);
      const count = Math.min(args.limit, 100001 - args.offset);
      return { offset: args.offset, rows: Array.from({ length: count }, (_, i) => [String(args.offset + i + 1), 'name_' + (args.offset + i + 1), '2026-01-02 12:00:00', '1.3700']), total: 100001, truncated: false, message: '' };
    };
    mock.query_result_snapshot = () => { throw new Error('UI fixture intentionally read-only'); };
    const app = window.__APP__;
    await app.openQueryTab('c1', 'SELECT * FROM large_table', 'navkit');
    await app.activeView().executeSelection(false);
  });
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy());
  const pager = page.locator('nav[aria-label="当前结果缓存分页"]');
  const input = pager.getByRole('textbox', { name: '当前页码' });
  await assert.equal(await input.inputValue(), '1');
  const firstRowNumber = page.locator('tbody .grid-row-header:visible').first();
  assert.equal(await firstRowNumber.innerText(), '1');
  const pageCount = pager.locator('.grid-page-count');
  assert.equal(await pageCount.innerText(), '/ 501 页');
  assert.equal(await page.locator('.result-preview-note:visible').count(), 0);
  assert.equal(await pager.locator('.query-page-range').count(), 0);
  const footerStatus = pager.locator('.query-footer-status');
  assert.equal(await footerStatus.innerText(), '运行耗时 0.456s（共 100001 行）');
  assert.doesNotMatch(await page.locator('.statusbar').innerText(), /运行耗时|共 100001 行/);
  const footerPosition = await pager.evaluate(el => ({ left: el.getBoundingClientRect().left, statusLeft: el.querySelector('.query-footer-status').getBoundingClientRect().left, inputLeft: el.querySelector('.query-page-number').getBoundingClientRect().left }));
  assert(footerPosition.statusLeft - footerPosition.left <= 10);
  assert(footerPosition.inputLeft > footerPosition.statusLeft);
  const dimensions = await pager.evaluate(el => ({ gap: getComputedStyle(el).gap, controlGap: getComputedStyle(el.querySelector('.grid-pager')).gap, input: { width: el.querySelector('.query-page-number').getBoundingClientRect().width, height: el.querySelector('.query-page-number').getBoundingClientRect().height }, buttons: [...el.querySelectorAll('button')].map(b => ({ width: b.getBoundingClientRect().width, height: b.getBoundingClientRect().height })), height: el.getBoundingClientRect().height }));
  assert.equal(dimensions.gap, '4px'); assert.equal(dimensions.controlGap, '4px');
  assert.deepEqual(dimensions.input, { width: 40, height: 22 });
  assert(dimensions.buttons.every(b => b.width === 22 && b.height === 22));
  await input.fill('501'); await input.press('Enter');
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy() && window.__APP__.activeView().resultPages.page(0).offset === 100000);
  assert.equal(await input.inputValue(), '501');
  assert.equal(await pageCount.innerText(), '/ 501 页');
  assert.equal(await footerStatus.innerText(), '运行耗时 0.456s（共 100001 行）');
  assert.equal(await firstRowNumber.innerText(), '100001');
  await input.fill('2'); await input.press('Tab');
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy() && window.__APP__.activeView().resultPages.page(0).offset === 200);
  assert.equal(await firstRowNumber.innerText(), '201');
  await input.fill('502'); await input.press('Enter');
  assert.equal(await input.getAttribute('aria-invalid'), 'true');
  assert.equal(await pager.locator('.query-page-error').innerText(), '请输入 1–501 的整数页码');
  await input.press('Escape'); assert.equal(await input.inputValue(), '2');
  const calls = await page.evaluate(() => window.__PAGER_CALLS__);
  assert.equal(calls.filter(c => c[0] === 'sql').length, 1);
  assert.deepEqual(calls.filter(c => c[0] === 'page').map(c => c[1]), [100000, 200]);
  const pageSize = pager.getByRole('combobox', { name: '每页条数' });
  assert.deepEqual(await pageSize.locator('option').evaluateAll(nodes => nodes.map(n => n.value)), ['100', '200', '500', '1000', '2000']);
  assert.equal(await pageSize.inputValue(), '200');
  assert.equal(await pager.locator('.query-page-size-unit').innerText(), '条');
  // 每页条数与翻页控件之间有竖分隔线，且落在两者之间
  const dividerLayout = await pager.evaluate(el => {
    const size = el.querySelector('.query-page-size').getBoundingClientRect();
    const divider = el.querySelector('.query-pager-divider');
    if (!divider) return null;
    const box = divider.getBoundingClientRect();
    const first = el.querySelector('.grid-pager button').getBoundingClientRect();
    return { width: box.width, height: box.height, sizeRight: size.right, dividerLeft: box.left, dividerRight: box.right, firstButtonLeft: first.left, background: getComputedStyle(divider).backgroundColor };
  });
  assert.ok(dividerLayout, 'page-size and pager must be separated by a divider');
  assert.equal(dividerLayout.width, 1);
  assert.ok(dividerLayout.dividerLeft > dividerLayout.sizeRight, 'divider sits after the page-size control');
  assert.ok(dividerLayout.dividerRight < dividerLayout.firstButtonLeft, 'divider sits before pager buttons');
  assert.notEqual(dividerLayout.background, 'rgba(0, 0, 0, 0)');
  await pageSize.selectOption('500');
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy() && window.__APP__.activeView().resultPages.page(0).rows.length === 500);
  assert.equal(await input.inputValue(), '1'); assert.equal(await pageCount.innerText(), '/ 201 页');
  assert.equal(await firstRowNumber.innerText(), '1');
  await pageSize.selectOption('2000');
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy() && window.__APP__.activeView().resultPages.page(0).rows.length === 2000);
  assert.equal(await pageCount.innerText(), '/ 51 页');
  assert.equal(await pageSize.inputValue(), '2000');
  await pageSize.selectOption('1000');
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy() && window.__APP__.activeView().resultPages.page(0).rows.length === 1000);
  assert.equal(await pageCount.innerText(), '/ 101 页');
  await pager.getByRole('button', { name: '下一页（读取本次执行缓存，不重新执行 SQL）' }).click();
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy() && window.__APP__.activeView().resultPages.page(0).offset === 1000);
  assert.equal(await input.inputValue(), '2'); assert.equal(await pageCount.innerText(), '/ 101 页');
  assert.equal(await firstRowNumber.innerText(), '1001');
  const renderedRows = await page.locator('tbody tr[data-row]:visible').evaluateAll(rows => rows.map(row => ({ index: Number(row.dataset.row), number: row.querySelector('.grid-row-header')?.textContent })));
  assert(renderedRows.every(row => row.number === String(row.index + 1001)));
  await page.evaluate(() => { const grid = window.__APP__.activeView().grid; grid.scroll.scrollTop = 500 * grid.rowHeight; });
  await page.waitForFunction(() => Number(document.querySelector("tbody tr[data-row]")?.dataset.row) > 400);
  const scrolledRows = await page.locator("tbody tr[data-row]:visible").evaluateAll(rows => rows.map(row => ({ index: Number(row.dataset.row), number: row.querySelector(".grid-row-header")?.textContent })));
  assert(scrolledRows.length && scrolledRows.every(row => row.number === String(row.index + 1001)));
  await page.evaluate(() => { window.__APP__.activeView().grid.scroll.scrollTop = 0; });
  await page.waitForFunction(() => document.querySelector("tbody tr[data-row] .grid-row-header")?.textContent === "1001");
  assert.equal(await page.evaluate(() => window.__PAGER_CALLS__.filter(c => c[0] === 'sql').length), 1);
  // 执行与缓存分页都只在结果区域中心显示遮罩，不再弹常规成功提示。
  await page.evaluate(() => {
    const mock = window.__PREVIEW_MOCK__, execute = mock.execute_sql_v2;
    mock.execute_sql_v2 = args => new Promise(resolve => { window.completeQuery = () => resolve(execute(args)); });
    void window.__APP__.activeView().executeSelection(false);
  });
  const overlay = page.locator('.query-result-loading');
  await overlay.waitFor({ state: 'visible' }); assert.match(await overlay.textContent(), /^正在查询… \d+\.\d{3} s$/);
  assert.equal(await page.locator('.query-runbar .query-transaction-status').count(), 0);
  const timer = overlay.locator('.query-loading-timer');
  const initialTime = await timer.textContent();
  await page.waitForFunction(first => document.querySelector('.query-result-loading .query-loading-timer')?.textContent !== first, initialTime);
  assert.match(await timer.textContent(), /^\d+\.\d{3} s$/);
  assert.equal(await timer.getAttribute('aria-hidden'), 'true');
  assert.equal(await timer.evaluate(el => getComputedStyle(el).fontVariantNumeric), 'tabular-nums');
  const overlayPosition = await overlay.evaluate(el => { const a = el.getBoundingClientRect(), b = el.parentElement.getBoundingClientRect(); return { x: Math.abs(a.x + a.width / 2 - b.x - b.width / 2), y: Math.abs(a.y + a.height / 2 - b.y - b.height / 2), align: getComputedStyle(el).alignItems }; });
  assert(overlayPosition.x < 1 && overlayPosition.y < 1); assert.equal(overlayPosition.align, 'center');
  await fs.mkdir(path.join(root, 'artifacts'), { recursive: true });
  await page.screenshot({ path: path.join(root, 'artifacts/query-result-loading-overlay.png') });
  await page.evaluate(() => completeQuery());
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy());
  assert.equal(await overlay.isVisible(), false); assert.equal(await page.locator('.result-preview-note:visible').count(), 0);
  assert.equal(await page.locator('.toast-success:visible').count(), 0);
  await page.evaluate(() => {
    const mock = window.__PREVIEW_MOCK__, fetch = mock.fetch_query_rows;
    mock.fetch_query_rows = args => new Promise(resolve => { window.completePage = () => resolve(fetch(args)); });
  });
  await pager.getByRole('button', { name: '下一页（读取本次执行缓存，不重新执行 SQL）' }).click();
  await overlay.waitFor({ state: 'visible' }); assert.equal(await overlay.textContent(), '正在加载结果页…');
  assert.equal(await page.evaluate(() => window.__APP__.activeView().grid.el.inert), true);
  await page.evaluate(() => completePage()); await page.waitForFunction(() => !window.__APP__.activeView().resultBusy());
  assert.equal(await overlay.isVisible(), false); assert.equal(await page.evaluate(() => window.__APP__.activeView().grid.el.inert), false);
  await input.press('Tab');
  await page.waitForFunction(() => document.querySelectorAll('.toast').length === 0, null, { timeout: 15000 });
  await fs.mkdir(path.join(root, 'artifacts'), { recursive: true });
  await page.screenshot({ path: path.join(root, 'artifacts/query-result-pagination.png') });
  const queryId = await page.evaluate(() => window.__APP__.activeTabId);
  await page.evaluate(async () => { await window.__APP__.openTableTab('c1', 'navkit', 'navkit_assignment_config', 'table'); });
  await page.waitForFunction(() => !!window.__APP__.activeTab.page && !window.__APP__.activeView().loading);
  await page.locator('td[data-row="0"][data-col="1"]:visible').first().click();
  assert.equal(await page.locator('.cell-preview:visible').count(), 0);
  await page.evaluate(() => window.__APP__.setTextFormat('hex'));
  await page.locator('.cell-preview:visible').waitFor();
  const firstId = await page.evaluate(() => window.__APP__.activeTabId);
  await page.evaluate(queryId => window.__APP__.activateTab(queryId), queryId);
  await page.locator('td[data-row="0"][data-col="1"]:visible').first().click();
  assert.equal(await page.locator('.cell-preview:visible').count(), 0);
  await page.evaluate(async () => { await window.__APP__.openTableTab('c1', 'navkit', 'navkit_database_connection', 'table'); });
  await page.waitForFunction(() => !!window.__APP__.activeTab.page && !window.__APP__.activeView().loading);
  await page.locator('td[data-row="0"][data-col="1"]:visible').first().click();
  assert.equal(await page.locator('.cell-preview:visible').count(), 0);
  await page.evaluate(firstId => window.__APP__.activateTab(firstId), firstId);
  await page.locator('td[data-row="0"][data-col="1"]:visible').first().click();
  await page.locator('.cell-preview:visible').waitFor();
  assert.equal(await page.evaluate(() => window.__APP__.textFormatValue), 'hex');
  await page.getByRole('button', { name: '关闭单元格内容预览' }).click();
  await page.locator('td[data-row="0"][data-col="1"]:visible').first().click();
  assert.equal(await page.locator('.cell-preview:visible').count(), 0);
  await page.evaluate(queryId => window.__APP__.activateTab(queryId), queryId);
  for (const [sql, affected] of [['UPDATE t SET value = 1', 7], ['DELETE FROM t', 0], ['INSERT INTO t VALUES (1)', 1]]) {
    await page.evaluate(async ({ sql, affected }) => {
      window.__PREVIEW_MOCK__.execute_sql_v2 = ({ request }) => ({ execution_id: request.execution_id, state: 'succeeded', error: null,
        result: { elapsed_ms: 125, messages: [], sets: [{ columns: [], rows: [], total: 0, affected, message: '' }] } });
      const view = window.__APP__.activeView(); view.editor.setSql(sql); await view.executeSelection(false);
    }, { sql, affected });
    await page.waitForFunction(() => !window.__APP__.activeView().resultBusy());
    assert.deepEqual(await page.locator('.result-tabs:visible .result-tab').allTextContents(), ['消息']);
    assert.equal(await page.locator('.grid-wrap:visible').count(), 0);
    assert.equal(await pager.locator('.grid-pager:visible').count(), 0);
    assert.equal(await pager.getByRole('combobox', { name: '每页条数' }).count(), 0);
    assert.equal(await pager.locator('.query-pager-divider:visible').count(), 0);
    assert.equal(await footerStatus.innerText(), `运行耗时 0.125s（影响 ${affected} 行）`);
    const footerBottomGap = await pager.evaluate(el => Math.abs(el.getBoundingClientRect().bottom - el.parentElement.getBoundingClientRect().bottom));
    assert(footerBottomGap < 1, 'message statistics remain at the bottom of the output area');
    assert.match(await page.locator('.result-messages:visible').innerText(), new RegExp(`影响 ${affected} 行`));
  }
  await page.screenshot({ path: path.join(root, 'artifacts/query-result-mutation-messages.png') });
  assert.deepEqual(errors, []);
  console.log('PASS browser table preview: query isolation, other table isolation, original-table restoration, close remains closed');
  console.log(JSON.stringify({ passed: ['central millisecond timer grows every 50ms', 'top execution status removed', 'execution and page-loading centered overlays', 'no success toast or top running message', 'compact controls', 'left result-footer time and total rows', 'table-style page-count suffix', 'non-rowset writes show messages and affected rows only', 'page-size dropdown 100/200/500/1000/2000 with 条 unit and divider', 'absolute row numbers across pages', 'hidden completion banner', 'Enter and blur jumps', 'invalid-page validation', 'no SQL rerun'], dimensions, divider: dividerLayout, screenshot: 'artifacts/query-result-pagination.png' }));
} catch (error) {
  console.log(await page.evaluate(() => ({ errors: document.body.innerText.slice(-4000), tab: window.__APP__?.activeTab, status: window.__APP__?.activeView()?.executionStatusEl?.textContent })));
  throw error;
} finally { await browser.close(); }
