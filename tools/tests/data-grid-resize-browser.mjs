// 窗口尺寸变化回归：网格可视行必须随视口重算，分页 limit 必须保持用户设置不变。
// 全部使用显式 fixture IPC，不触碰真实数据库。
import assert from 'node:assert/strict';
import { mkdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer } from 'node:http';
import { chromium } from 'playwright-core';
const sleepMs = ms => new Promise(r => setTimeout(r, ms));

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
const ROWS_PER_PAGE = 200;
const TOTAL = 1000;
try {
  browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  const page = await browser.newPage({ viewport: { width: 1538, height: 820 } });
  page.setDefaultTimeout(10000);
  page.setDefaultNavigationTimeout(30000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  // 只截掉预览桩里的定时自动演示，避免它切走页签。
  const previewSource = await readFile(new URL('../../public/preview-bootstrap.js', import.meta.url), 'utf8');
  const demo = previewSource.indexOf('// 自动演示：');
  assert.ok(demo > 0);
  await page.route('**/preview-bootstrap.js*', route => route.fulfill({ contentType: 'text/javascript', body: previewSource.slice(0, demo) + '\n} catch (e) { throw e; }\n' }));
  await page.addInitScript(({ rowsPerPage, total }) => {
    let mock;
    window.__PAGE_REQUESTS__ = [];
    Object.defineProperty(window, '__PREVIEW_MOCK__', { configurable: true, get: () => mock, set: value => {
      mock = value;
      const getSettings = value.get_settings;
      mock.get_settings = () => ({ ...getSettings(), rows_per_page: rowsPerPage });
      mock['plugin:event|listen'] = () => 1;
      mock['plugin:event|unlisten'] = () => null;
      mock.list_editing_drafts = () => [];
      mock.page_table = args => {
        const req = args.req;
        window.__PAGE_REQUESTS__.push({ offset: req.offset, limit: req.limit });
        const rows = Array.from({ length: Math.max(0, Math.min(req.limit, total - req.offset)) }, (_, i) => [String(req.offset + i + 1), `记录 ${req.offset + i + 1}`]);
        return { columns: [
          { name: 'id', type: 'INTEGER', nullable: false, primary_key: true, auto_increment: false },
          { name: 'name', type: 'TEXT', nullable: true, primary_key: false, auto_increment: false },
        ], rows, total, primary_keys: ['id'], elapsed_ms: 0 };
      };
    } });
  }, { rowsPerPage: ROWS_PER_PAGE, total: TOTAL });
  await page.goto(`http://127.0.0.1:${server.address().port}/preview.html`, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(n => window.__APP__?.connections?.length && window.__APP__.settings.rows_per_page === n, ROWS_PER_PAGE);
  await page.evaluate(() => window.__APP__.openTableTab('c1', 'dbswitch', 'resize_fixture', 'table'));
  await page.waitForFunction(n => window.__APP__.activeView()?.grid?.rows.length === n, ROWS_PER_PAGE);

  /** 采样当前网格状态：视口高、渲染行数、行高、滚动总高、分页 limit。 */
  const sample = () => page.evaluate(() => {
    const view = window.__APP__.activeView();
    const scroll = view.grid.el.querySelector('.grid-scroll');
    const dataRows = [...scroll.querySelectorAll('tbody tr[data-row]')];
    const firstHeight = dataRows[0]?.getBoundingClientRect().height ?? 0;
    const headerHeight = scroll.querySelector('thead')?.getBoundingClientRect().height ?? 0;
    return {
      viewportHeight: scroll.clientHeight,
      renderedRows: dataRows.length,
      rowHeight: firstHeight,
      headerHeight,
      scrollHeight: scroll.scrollHeight,
      scrollTop: scroll.scrollTop,
      pageLimit: view.tab.pageLimit,
      pageOffset: view.tab.pageOffset,
      rowsInGrid: view.grid.rows.length,
      spacers: scroll.querySelectorAll('tbody tr.grid-spacer').length,
      previewDisplay: getComputedStyle(view.cellPreviewEl).display,
    };
  });

  const wide = await sample();
  // 每页条数来自设置，与窗口大小无关。
  assert.equal(wide.pageLimit, ROWS_PER_PAGE);
  assert.equal(wide.rowsInGrid, ROWS_PER_PAGE);
  assert.ok(wide.rowHeight > 20 && wide.rowHeight < 40, `行高应取 CSS 真实值，实测 ${wide.rowHeight}`);
  // 可视行必须填满视口（旧实现在这里会因为行高常量 19.2 与真实 26 不符而留白）
  assert.ok(wide.renderedRows * wide.rowHeight >= wide.viewportHeight - wide.rowHeight * 2,
    `可视行应填满视口：${wide.renderedRows} 行 × ${wide.rowHeight}px < 视口 ${wide.viewportHeight}px`);
  // 滚动总高应与数据行数一致（不能低于视口）
  assert.ok(wide.scrollHeight >= wide.viewportHeight - 2, `滚动高度不足：${wide.scrollHeight}`);

  // 窗口变高：可视行增加，pageLimit 不变，不重新请求数据
  const requestsBefore = await page.evaluate(() => window.__PAGE_REQUESTS__.length);
  await page.setViewportSize({ width: 1538, height: 1200 });
  // ResizeObserver 回调在下一帧才重算，等渲染行数真正跟上，而不是等布局尺寸
  await page.waitForFunction(n => window.__APP__.activeView().grid.el.querySelectorAll('tbody tr[data-row]').length > n, wide.renderedRows);
  const tall = await sample();
  assert.equal(tall.pageLimit, ROWS_PER_PAGE);
  assert.ok(tall.renderedRows > wide.renderedRows, `窗口变高后可视行应增加：${wide.renderedRows} → ${tall.renderedRows}`);
  assert.ok(tall.renderedRows * tall.rowHeight >= tall.viewportHeight - tall.rowHeight * 2);
  assert.equal(await page.evaluate(() => window.__PAGE_REQUESTS__.length), requestsBefore, '改变窗口大小不应触发重新分页查询');

  // 窗口变矮：可视行减少（旧实现缩小时不回收，行数停留在上一次的高度）
  await page.setViewportSize({ width: 1100, height: 560 });
  await page.waitForFunction(n => window.__APP__.activeView().grid.el.querySelectorAll('tbody tr[data-row]').length < n, tall.renderedRows);
  const small = await sample();
  assert.equal(small.pageLimit, ROWS_PER_PAGE);
  assert.ok(small.renderedRows < tall.renderedRows, `窗口变矮后可视行应减少：${tall.renderedRows} → ${small.renderedRows}`);
  assert.ok(small.renderedRows * small.rowHeight >= small.viewportHeight - small.rowHeight * 2);

  // 滚动总高在整个过程中都应与数据行数匹配（占位行撑高，而不是 tbody padding）
  for (const s of [wide, tall, small]) {
    const expected = s.rowsInGrid * s.rowHeight + s.headerHeight;
    assert.ok(Math.abs(s.scrollHeight - expected) <= s.rowHeight * 1.5,
      `滚动高度应与 ${s.rowsInGrid} 行 + 表头吻合：期望≈${expected}，实际 ${s.scrollHeight}`);
  }

  // 翻页到最后一页并滚到底：最后一行必须真正可见（占位行高度算错时会出现滚不到底）
  await page.evaluate(() => { const view = window.__APP__.activeView(); view.tab.pageOffset = 800; return view.reload(); });
  await page.waitForFunction(() => window.__APP__.activeView().tab.pageOffset === 800);
  await page.waitForFunction(n => window.__APP__.activeView()?.grid?.rows.length === n, ROWS_PER_PAGE);
  await page.evaluate(() => { const s = window.__APP__.activeView().grid.el.querySelector('.grid-scroll'); s.scrollTop = s.scrollHeight; });
  await page.waitForFunction(n => {
    const scroll = window.__APP__.activeView().grid.el.querySelector('.grid-scroll');
    const last = scroll.querySelector(`tbody tr[data-row="${n - 1}"]`);
    if (!last) return false;
    const box = last.getBoundingClientRect(), view = scroll.getBoundingClientRect();
    return box.top >= view.top - 1 && box.bottom <= view.bottom + 1;
  }, ROWS_PER_PAGE);

  // 单元格面板：未聚焦时不占高度（旧实现恒挂一条 42px 空面板）
  const footerHeightBefore = await page.evaluate(() => window.__APP__.activeView().grid.el.getBoundingClientRect().height);
  assert.equal(small.previewDisplay, 'none');
  const previewHidden = await page.evaluate(() => {
    const view = window.__APP__.activeView();
    return { display: getComputedStyle(view.cellPreviewEl).display, height: view.cellPreviewEl.getBoundingClientRect().height };
  });
  assert.equal(previewHidden.display, 'none');
  assert.equal(previewHidden.height, 0);
  const footerHeightAfter = await page.evaluate(() => window.__APP__.activeView().grid.el.getBoundingClientRect().height);
  assert.equal(footerHeightAfter, footerHeightBefore);

  // 选中驱动：单击单元格即选中 → 高亮并展开内容面板；没有选中时面板不占高度。
  await page.locator('.grid-table tbody tr[data-row]').first().locator('td').nth(1).click();
  await sleepMs(200);
  const afterPlainClick = await page.evaluate(() => {
    const view = window.__APP__.activeView();
    return { display: getComputedStyle(view.cellPreviewEl).display, height: view.cellPreviewEl.getBoundingClientRect().height };
  });
  assert.notEqual(afterPlainClick.display, 'none', '单击选中单元格后应展开内容面板');
  assert.ok(afterPlainClick.height >= 40, `单击选中后面板应展开，实测 ${afterPlainClick.height}`);
  // 进入编辑（双击）→ 编辑态全选 → 面板展开
  await page.locator('.grid-table tbody tr[data-row]').first().locator('td').nth(1).dblclick();
  await page.waitForFunction(() => getComputedStyle(window.__APP__.activeView().cellPreviewEl).display !== 'none', null, { timeout: 8000 });
  const previewShown = await page.evaluate(() => window.__APP__.activeView().cellPreviewEl.getBoundingClientRect().height);
  assert.ok(previewShown >= 40, `进入编辑后面板应展开，实测 ${previewShown}`);
  await page.keyboard.press('Escape');
  await sleepMs(200);
  const afterEscape = await page.evaluate(() => ({ display: getComputedStyle(window.__APP__.activeView().cellPreviewEl).display, height: window.__APP__.activeView().cellPreviewEl.getBoundingClientRect().height }));
  assert.ok(afterEscape.height >= 40, 'Escape 退出编辑后焦点单元格仍选中，面板保持展开');

  await mkdir(path.join(root, 'artifacts'), { recursive: true });
  await page.screenshot({ path: path.join(root, 'artifacts/data-grid-resize.png'), timeout: 10000, animations: 'disabled' });
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ mode: 'production-browser-mocked-ipc', passed: [
    '窗口变大/变小后可视行数随视口重算',
    '分页 limit 始终等于设置值，缩放不触发重新查询',
    '滚动总高与数据行数相符（占位行撑高）',
    '最后一页滚到底可见最后一行',
    '未选中时不占高度；单击选中即展开；进入编辑展开；Escape 退出编辑仍保持选中预览',
  ], samples: { wide, tall, small }, screenshot: 'artifacts/data-grid-resize.png' }, null, 2));
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
