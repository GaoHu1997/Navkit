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
  assert.equal(await pager.locator('.query-page-count').count(), 0);
  assert.equal(await page.locator('.result-preview-note:visible').count(), 0);
  assert.equal(await pager.locator('.query-page-range').count(), 0);
  const footerStatus = pager.locator('.query-footer-status');
  assert.match(await footerStatus.innerText(), /运行时间: 0.456s.*第 1\/501 页（共 100001 行）/);
  assert.doesNotMatch(await page.locator('.statusbar').innerText(), /运行时间|共 100001 行/);
  const footerPosition = await pager.evaluate(el => ({ left: el.getBoundingClientRect().left, statusLeft: el.querySelector('.query-footer-status').getBoundingClientRect().left, inputLeft: el.querySelector('input').getBoundingClientRect().left }));
  assert(footerPosition.statusLeft - footerPosition.left <= 10);
  assert(footerPosition.inputLeft > footerPosition.statusLeft);
  const dimensions = await pager.evaluate(el => ({ gap: getComputedStyle(el).gap, buttons: [...el.querySelectorAll('button')].map(b => ({ width: b.getBoundingClientRect().width, height: b.getBoundingClientRect().height })), height: el.getBoundingClientRect().height }));
  assert.equal(dimensions.gap, '4px'); assert(dimensions.buttons.every(b => b.width === 22 && b.height === 22));
  await input.fill('501'); await input.press('Enter');
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy() && window.__APP__.activeView().resultPages.page(0).offset === 100000);
  assert.equal(await input.inputValue(), '501');
  assert.match(await footerStatus.innerText(), /第 501\/501 页/);
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
  assert.deepEqual(await pageSize.locator('option').evaluateAll(nodes => nodes.map(n => n.value)), ['200', '500', '1000']);
  const sizePosition = await pager.evaluate(el => ({ selectRight: el.querySelector('select').getBoundingClientRect().right, firstButtonLeft: el.querySelector('button').getBoundingClientRect().left }));
  assert(sizePosition.selectRight < sizePosition.firstButtonLeft);
  await pageSize.selectOption('500');
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy() && window.__APP__.activeView().resultPages.page(0).rows.length === 500);
  assert.equal(await input.inputValue(), '1'); assert.match(await footerStatus.innerText(), /第 1\/201 页/);
  assert.equal(await firstRowNumber.innerText(), '1');
  await pageSize.selectOption('1000');
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy() && window.__APP__.activeView().resultPages.page(0).rows.length === 1000);
  assert.match(await footerStatus.innerText(), /第 1\/101 页/);
  await pager.getByRole('button', { name: '下一页（读取本次执行缓存，不重新执行 SQL）' }).click();
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy() && window.__APP__.activeView().resultPages.page(0).offset === 1000);
  assert.equal(await input.inputValue(), '2'); assert.match(await footerStatus.innerText(), /第 2\/101 页/);
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
  await input.press('Tab');
  await page.locator('.toast').waitFor({ state: 'detached', timeout: 15000 });
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
  assert.deepEqual(errors, []);
  console.log('PASS browser table preview: query isolation, other table isolation, original-table restoration, close remains closed');
  console.log(JSON.stringify({ passed: ['compact controls', 'left result-footer time and row range', 'no page-count suffix', '200/500/1000 page-size selection', 'absolute row numbers across pages', 'hidden completion banner', 'Enter and blur jumps', 'invalid-page validation', 'no SQL rerun'], dimensions, screenshot: 'artifacts/query-result-pagination.png' }));
} catch (error) {
  console.log(await page.evaluate(() => ({ errors: document.body.innerText.slice(-4000), tab: window.__APP__?.activeTab, status: window.__APP__?.activeView()?.executionStatusEl?.textContent })));
  throw error;
} finally { await browser.close(); }
