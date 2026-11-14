// Production-browser acceptance for filter summaries and live query timing; fixture IPC only.
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
    if (pathname === '/' || pathname === '/preview.html') {
      response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }); response.end(html); return;
    }
    const filename = path.resolve(dist, '.' + pathname);
    if (!filename.startsWith(dist + path.sep)) { response.writeHead(403); response.end(); return; }
    const content = await readFile(filename);
    response.writeHead(200, { 'Content-Type': { '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }[path.extname(filename)] || 'application/octet-stream' });
    response.end(content);
  } catch { response.writeHead(404); response.end(); }
});
await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  const page = await browser.newPage({ viewport: { width: 2150, height: 1246 } });
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const preview = await readFile(path.join(root, 'public/preview-bootstrap.js'), 'utf8');
  const demo = preview.indexOf('// 自动演示：');
  assert.ok(demo > 0);
  await page.route('**/preview-bootstrap.js*', route => route.fulfill({ contentType: 'text/javascript', body: preview.slice(0, demo) + '\n} catch (e) { throw e; }\n' }));
  await page.addInitScript(() => {
    let mock;
    window.__STATUS_REQUESTS__ = [];
    Object.defineProperty(window, '__PREVIEW_MOCK__', { configurable: true, get: () => mock, set: value => {
      mock = value;
      mock['plugin:event|listen'] = () => 1;
      mock['plugin:event|unlisten'] = () => null;
      mock.list_editing_drafts = () => [];
      mock.page_table = args => new Promise((resolve, reject) => window.__STATUS_REQUESTS__.push({ req: args.req, resolve, reject }));
    } });
    window.__STATUS_RESOLVE__ = index => {
      const request = window.__STATUS_REQUESTS__[index];
      request.resolve({ columns: [
        { name: 'tradeType', type: 'INTEGER', nullable: false, primary_key: true, auto_increment: false },
        { name: 'applyStatus', type: 'INTEGER', nullable: false, primary_key: false, auto_increment: false },
        { name: 'tradeStatus', type: 'INTEGER', nullable: false, primary_key: false, auto_increment: false },
      ], rows: [['130', '3', '3']], total: 1, total_exact: true, has_more: false, primary_keys: ['tradeType'], elapsed_ms: 1 });
    };
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/preview.html`, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.__APP__?.connections?.length);
  await page.evaluate(() => { void window.__APP__.openTableTab('c1', 'dbswitch', 'status_fixture', 'table'); });
  await page.waitForFunction(() => window.__STATUS_REQUESTS__.length === 1);
  const elapsed = page.locator('.statusbar-query-elapsed');
  assert.match(await elapsed.textContent(), /^查询耗时: \d+\.\d{3}s（查询中）$/);
  assert.equal(await page.evaluate(() => window.__APP__.activeView().tab.page), null);
  const first = await elapsed.textContent();
  const statusButton = page.locator('.statusbar-toggle-btn').first();
  // Existing status switches are divs; make this fixture focusable to detect DOM replacement.
  await statusButton.evaluate(button => { button.tabIndex = 0; });
  await statusButton.focus();
  await page.waitForFunction(initial => document.querySelector('.statusbar-query-elapsed')?.textContent !== initial, first);
  assert(await statusButton.evaluate(button => button === document.activeElement));
  await page.evaluate(() => window.__STATUS_RESOLVE__(0));
  await page.waitForFunction(() => !window.__APP__.activeView().loading);
  const final = await elapsed.textContent();
  assert.match(final, /^查询耗时: \d+\.\d{3}s$/);
  const frozen = await page.evaluate(async () => {
    const cell = document.querySelector('.statusbar-query-elapsed');
    const before = cell.textContent;
    await new Promise(resolve => setTimeout(resolve, 300));
    return { before, after: cell.textContent, timer: window.__APP__.activeView().queryTimer };
  });
  assert.equal(frozen.before, frozen.after); assert.equal(frozen.timer, null);

  await page.evaluate(() => {
    const view = window.__APP__.activeView();
    view.tab.filters = [
      { logic: 'AND', conds: [{ column: 'tradeType', operator: '=', value: '129' }, { column: 'tradeType', operator: '=', value: '130', logic: 'OR' }] },
      { logic: 'AND', conds: [{ column: 'applyStatus', operator: '=', value: '3' }, { column: 'tradeStatus', operator: '=', value: '3', logic: 'OR' }] },
    ];
    view.renderFooter(view.tab.page);
  });
  const summary = page.locator('.grid-filter-summary');
  assert.equal(await summary.textContent(), '筛选: (tradeType=129 OR tradeType=130) AND (applyStatus=3 OR tradeStatus=3)');
  await page.evaluate(() => {
    const view = window.__APP__.activeView();
    view.tab.filters[1].conds.push({ column: 'tradeStatus', operator: '=', value: '完整中文筛选条件'.repeat(60), logic: 'OR' });
    view.renderFooter(view.tab.page);
  });
  const geometry = await summary.evaluate(button => {
    const css = getComputedStyle(button), rect = button.getBoundingClientRect();
    return { tag: button.tagName, title: button.title, text: button.textContent, border: css.borderWidth, background: css.backgroundColor, shadow: css.boxShadow,
      maxWidth: css.maxWidth, width: rect.width, clientWidth: button.clientWidth, scrollWidth: button.scrollWidth, overflow: css.overflow, ellipsis: css.textOverflow };
  });
  assert.equal(geometry.tag, 'BUTTON'); assert.equal(geometry.border, '0px'); assert.equal(geometry.background, 'rgba(0, 0, 0, 0)'); assert.equal(geometry.shadow, 'none');
  assert.equal(geometry.maxWidth, '640px'); assert.equal(geometry.width, 640);
  assert.equal(geometry.title, geometry.text); assert(geometry.scrollWidth > geometry.clientWidth);
  assert.equal(geometry.overflow, 'hidden'); assert.equal(geometry.ellipsis, 'ellipsis');
  await summary.hover();
  await summary.focus();
  await page.keyboard.press('Tab'); await page.keyboard.press('Shift+Tab');
  assert(await summary.evaluate(button => button === document.activeElement && button.matches(':focus-visible')));
  assert(await summary.evaluate(button => Number.parseFloat(getComputedStyle(button).outlineWidth) >= 2));
  await summary.press('Enter');
  assert(await page.locator('.filter-bar').isVisible());
  await page.evaluate(() => {
    const view = window.__APP__.activeView();
    const app = window.__APP__; app.__statusCommands = [];
    app.runCommand = command => app.__statusCommands.push(command);
    view.tab.filters = []; view.tab.orderBy = 'tradeType'; view.tab.ascending = false;
    view.tab.order_rules = [{ column: 'tradeType', ascending: false }]; view.renderFooter(view.tab.page);
  });
  await summary.press('Enter');
  assert.deepEqual(await page.evaluate(() => window.__APP__.__statusCommands), ['sort']);

  await page.evaluate(() => { void window.__APP__.activeView().reload(); });
  await page.waitForFunction(() => window.__STATUS_REQUESTS__.length === 2);
  assert.match(await elapsed.textContent(), /（查询中）$/);
  await page.evaluate(() => window.__STATUS_REQUESTS__[1].reject(new Error('fixture failure')));
  await page.waitForFunction(() => !window.__APP__.activeView().loading);
  assert.match(await elapsed.textContent(), /^查询耗时: \d+\.\d{3}s$/);
  assert.equal(await page.evaluate(() => window.__APP__.activeView().queryTimer), null);

  await page.evaluate(() => {
    const view = window.__APP__.activeView();
    view.tab.filters = [{ conds: [{ column: 'tradeType', operator: '=', value: '129' }, { column: 'tradeType', operator: '=', value: '130', logic: 'OR' }] },
      { conds: [{ column: 'applyStatus', operator: '=', value: '3' }, { column: 'tradeStatus', operator: '=', value: '3', logic: 'OR' }] }];
    view.tab.orderBy = null; view.tab.order_rules = []; view.renderFooter(view.tab.page);
  });
  await page.setViewportSize({ width: 1024, height: 768 });
  const narrow = await page.locator('.grid-footer').evaluate(footer => ({ footerRight: footer.getBoundingClientRect().right,
    pagerRight: footer.querySelector('.grid-pager').getBoundingClientRect().right, scrollWidth: document.documentElement.scrollWidth, viewport: window.innerWidth }));
  assert(narrow.pagerRight <= narrow.viewport); assert(narrow.scrollWidth <= narrow.viewport);
  await page.setViewportSize({ width: 2150, height: 1246 });
  await page.evaluate(() => { void window.__APP__.activeView().reload(); });
  await page.waitForFunction(() => window.__STATUS_REQUESTS__.length === 3);
  await page.evaluate(() => window.__STATUS_RESOLVE__(2));
  await page.waitForFunction(() => !window.__APP__.activeView().loading);
  await mkdir(path.join(root, 'artifacts'), { recursive: true });
  const screenshot = path.join(root, 'artifacts/data-grid-status.png');
  await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ mode: 'production-browser-mocked-ipc', passed: ['首次结果返回前可见并持续计时', '计时刷新不打断状态栏焦点', '成功/失败停止计时并保留最终值', '筛选摘要无外框且最大宽度由320px增至640px', '截断省略与完整title悬浮文本', '键盘焦点/Enter与仅排序入口', '1024px布局不溢出'], final, geometry: { ...geometry, text: undefined, title: undefined }, narrow, screenshot }, null, 2));
} finally {
  try { if (browser) await browser.close(); }
  finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
}
