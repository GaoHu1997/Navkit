// Pagination browser acceptance uses explicit fixture IPC; no database writes.
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
    const filename = path.resolve(dist, '.' + pathname);
    if (pathname === '/' || pathname === '/preview.html') {
      response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
      response.end(html);
      return;
    }
    if (!filename.startsWith(dist + path.sep)) { response.writeHead(403); response.end(); return; }
    const content = await readFile(filename);
    const type = { '.js': 'application/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png' }[path.extname(filename)] || 'application/octet-stream';
    response.writeHead(200, { 'Content-Type': type }); response.end(content);
  } catch { response.writeHead(404); response.end(); }
});
await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
let browser;
try {
browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
const page = await browser.newPage({ viewport: { width: 1538, height: 820 } });
page.setDefaultTimeout(10000);
page.setDefaultNavigationTimeout(30000);
const errors = [];
page.on('pageerror', error => errors.push(error.message));
// Suppress only the preview's timed automatic demo, which otherwise switches tabs.
const previewSource = await readFile(new URL('../../public/preview-bootstrap.js', import.meta.url), 'utf8');
const demo = previewSource.indexOf('// 自动演示：');
assert.ok(demo > 0);
await page.route('**/preview-bootstrap.js*', route => route.fulfill({ contentType: 'text/javascript', body: previewSource.slice(0, demo) + '\n} catch (e) { throw e; }\n' }));
await page.addInitScript(() => {
  let mock;
  window.__PAGINATION_REQUESTS__ = [];
  window.__PAGINATION_TOTAL__ = 65;
  Object.defineProperty(window, '__PREVIEW_MOCK__', { configurable: true, get: () => mock, set: value => {
    mock = value;
    const getSettings = value.get_settings;
    mock.get_settings = () => ({ ...getSettings(), rows_per_page: 20 });
    mock['plugin:event|listen'] = () => 1;
    mock['plugin:event|unlisten'] = () => null;
    mock.list_editing_drafts = () => [];
    mock.page_table = args => {
      const req = args.req;
      window.__PAGINATION_REQUESTS__.push(req.offset);
      const total = window.__PAGINATION_TOTAL__;
      const rows = Array.from({ length: Math.max(0, Math.min(req.limit, total - req.offset)) }, (_, i) => [String(req.offset + i + 1), i === 0 ? 'Alice 中文' : `记录 ${req.offset + i + 1}`]);
      return { columns: [
        { name: 'id', type: 'INTEGER', nullable: false, primary_key: true, auto_increment: false },
        { name: 'name', type: 'TEXT', nullable: true, primary_key: false, auto_increment: false },
      ], rows, total, primary_keys: ['id'], elapsed_ms: 0 };
    };
  } });
});
const waitForPage = async number => {
  await page.waitForFunction(n => {
    const view = window.__APP__.activeView();
    return view?.tab?.pageOffset === (n - 1) * 20 && view.footer.querySelector('input')?.value === String(n);
  }, number);
};
  await page.goto(`http://127.0.0.1:${server.address().port}/preview.html`, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.__APP__?.connections?.length && window.__APP__.settings.rows_per_page === 20);
  await page.evaluate(() => window.__APP__.openTableTab('c1', 'dbswitch', 'pagination_fixture', 'table'));
  const pager = page.getByRole('navigation', { name: '表格分页', exact: true });
  await pager.waitFor();
  await waitForPage(1);
  const footerView = pager.locator('..');
  assert.equal(await footerView.getByRole('button', { name: /^(网格视图|表单视图)$/ }).count(), 0);
  assert.equal(await page.locator('.record-form, .record-nav').count(), 0);
  assert.equal(await page.locator('.statusbar-toggle-btn').count(), 2);
  assert.equal(await page.locator('.grid-wrap:visible').count(), 1);
  assert.equal(await pager.getByRole('button').count(), 4);
  assert.equal(await pager.getByRole('button', { name: '更多', exact: true }).count(), 0);
  assert(await pager.getByRole('button', { name: '第一页', exact: true }).isDisabled());
  assert(await pager.getByRole('button', { name: '上一页', exact: true }).isDisabled());
  assert.equal(await pager.locator('.grid-page-count').textContent(), '/ 4 页');
  const geometry = await pager.evaluate(nav => {
    const rect = nav.getBoundingClientRect(), footer = nav.parentElement.getBoundingClientRect();
    const controls = [...nav.querySelectorAll('button,input')].map(el => ({ height: el.getBoundingClientRect().height, center: el.getBoundingClientRect().y + el.getBoundingClientRect().height / 2 }));
    const css = getComputedStyle(nav);
    return { rightGap: footer.right - rect.right, lastControl: nav.nextElementSibling === null, separatorWidth: css.borderRightWidth, controls, outerBorders: [css.borderTopWidth, css.borderLeftWidth, css.borderBottomWidth] };
  });
  assert.equal(geometry.lastControl, true);
  assert.equal(geometry.rightGap, 8);
  assert.equal(geometry.separatorWidth, '0px');
  assert.ok(geometry.controls.every(c => c.height === 22));
  assert.ok(geometry.controls.every(c => Math.abs(c.center - geometry.controls[0].center) < 1));
  assert.deepEqual(geometry.outerBorders, ['0px', '0px', '0px']);
  await pager.getByRole('button', { name: '下一页', exact: true }).click(); await waitForPage(2);
  await pager.getByRole('button', { name: '最后一页', exact: true }).click(); await waitForPage(4);
  assert(await pager.getByRole('button', { name: '下一页', exact: true }).isDisabled());
  assert(await pager.getByRole('button', { name: '最后一页', exact: true }).isDisabled());
  await pager.getByRole('button', { name: '上一页', exact: true }).click(); await waitForPage(3);
  await pager.getByRole('button', { name: '第一页', exact: true }).click(); await waitForPage(1);
  await pager.getByLabel('当前页码', { exact: true }).fill('3');
  await pager.getByLabel('当前页码', { exact: true }).press('Enter'); await waitForPage(3);
  assert.deepEqual(await page.evaluate(() => window.__PAGINATION_REQUESTS__), [0, 20, 60, 40, 0, 40]);
  await page.evaluate(() => {
    const view = window.__APP__.activeView();
    view.pendingUpdates.set(0, new Map([['name', '修改后']]));
    view.renderFooter(view.tab.page);
  });
  await pager.getByRole('button', { name: '下一页', exact: true }).click();
  assert.equal(await page.evaluate(() => window.__APP__.activeView().tab.pageOffset), 40);
  assert.equal(await page.evaluate(() => window.__PAGINATION_REQUESTS__.length), 6);
  await page.evaluate(() => { const view = window.__APP__.activeView(); view.pendingUpdates.clear(); view.renderFooter(view.tab.page); });
  await pager.getByLabel('当前页码', { exact: true }).focus();
  await page.keyboard.press('Tab');
  assert(await pager.getByRole('button', { name: '下一页', exact: true }).evaluate(button => button === document.activeElement && button.matches(':focus-visible')));
  const focus = await pager.getByRole('button', { name: '下一页', exact: true }).evaluate(button => ({ width: getComputedStyle(button).outlineWidth, style: getComputedStyle(button).outlineStyle }));
  assert.ok(Number.parseFloat(focus.width) >= 2); assert.equal(focus.style, 'solid');
  await mkdir(path.join(root, 'artifacts'), { recursive: true });
  const footer = await pager.locator('..').boundingBox();
  await page.screenshot({ path: path.join(root, 'artifacts/data-grid-pagination.png'), clip: { x: footer.x, y: footer.y - 60, width: footer.width, height: footer.height + 60 }, timeout: 10000, animations: 'disabled' });
  await page.setViewportSize({ width: 1024, height: 768 });
  const narrow = await pager.evaluate(nav => ({ right: nav.getBoundingClientRect().right, viewport: window.innerWidth }));
  assert.ok(narrow.right <= narrow.viewport);
  await page.evaluate(async () => { window.__PAGINATION_TOTAL__ = 0; const view = window.__APP__.activeView(); view.tab.pageOffset = 0; await view.reload(); });
  await waitForPage(1);
  assert.equal(await pager.locator('.grid-page-count').textContent(), '/ 1 页');
  assert.equal(await pager.locator('button:disabled').count(), 4);
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ mode: 'production-browser-mocked-ipc', passed: ['纯网格数据页，无表单视图和切换入口，分页右对齐', '一致的22px控件高度，无更多菜单或多重外框', '首尾页禁用及空结果', '四个翻页按钮与Enter页码跳转', '未提交修改保护', '键盘焦点与1024px宽度'], geometry, screenshot: 'artifacts/data-grid-pagination.png' }, null, 2));
} catch (error) {
  console.error(error);
  throw error;
} finally {
  try { if (browser) await browser.close(); }
  finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
}
