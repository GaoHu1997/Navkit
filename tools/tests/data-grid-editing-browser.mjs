// 真实 Chromium DOM 回归；source 模式使用当前 DataGrid，--production 使用 dist + 显式 IPC fixture。
// 两种模式均使用自有临时 HTTP server，不连接数据库，不依赖已有 5183 服务。
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer as createHttpServer } from 'node:http';
import { createServer as createViteServer } from 'vite';
import { chromium } from 'playwright-core';

const root = fileURLToPath(new URL('../../', import.meta.url));
const production = process.argv.includes('--production');
const columns = [
  ['id', 'INTEGER'], ['name', 'TEXT'], ['day', 'DATE'], ['clock', 'TIME(6)'],
  ['stamp', 'TIMESTAMP(6) WITH TIME ZONE'], ['nil', 'TEXT'], ['empty', 'TEXT'],
].map(([name, type]) => ({ name, type, nullable: name !== 'id', primary_key: name === 'id', auto_increment: false }));
const rows = Array.from({ length: 200 }, (_, i) => [String(i + 1), `记录 ${i + 1}`, '2026-11-10', '12:34:56.654321-05:00', '2026-11-10 12:34:56.123456+08:00', null, '']);
const fixtureHtml = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>DataGrid 编辑回归</title></head>
<body><div id="frame" style="height:322px;display:flex;flex-direction:column;width:1120px"><div id="preview" style="height:0;flex:none"></div></div><button id="outside">离开编辑</button><div id="icons"></div>
<script type="module">
import '/src/styles/base.css';
import { DataGrid } from '/src/ui/DataGrid.ts';
import { ICON_COLUMN_MINI, ICON_INDEX_MINI, ICON_FK_MINI, ICON_TRIGGER_MINI, svg } from '/src/ui/icons.ts';
const columns = ${JSON.stringify(columns)}, initialRows = ${JSON.stringify(rows)};
const frame = document.querySelector('#frame'), preview = document.querySelector('#preview');
let fullRows = structuredClone(initialRows), visible = columns.map((_, i) => i);
window.commits = [];
const data = () => ({ columns: visible.map(i => columns[i]), rows: fullRows.map(row => visible.map(i => row[i])), lined: true, readOnly: false });
const grid = window.grid = new DataGrid({
  onEditStart: (row, col) => grid.beginEdit(row, col, grid.rows[row][col]),
  onEditBegin: () => { preview.style.height = grid.editingActive ? '42px' : '0'; },
  onEditEnd: (row, col, value) => { window.commits.push({ row, col, originalCol: visible[col], value }); fullRows[row][visible[col]] = value; grid.setData(data()); },
  onColumnClick: () => {},
});
frame.appendChild(grid.el);
window.resetGrid = (options = {}) => { fullRows = structuredClone(initialRows); visible = options.visible ?? columns.map((_, i) => i); window.commits.length = 0; grid.setData({ ...data(), ...options }); grid.el.querySelector('.grid-scroll').scrollTop = 0; };
window.getFullRows = () => fullRows;
window.resetGrid();
document.querySelector('#icons').innerHTML = [ICON_COLUMN_MINI, ICON_INDEX_MINI, ICON_FK_MINI, ICON_TRIGGER_MINI].map(def => svg(def)).join('');
</script></body></html>`;

let server, browser, closeServer;
try {
  if (production) {
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
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
    closeServer = async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); };
  } else {
    const vite = await createViteServer({ root, configFile: false, optimizeDeps: { noDiscovery: true, include: [] }, server: { middlewareMode: true, watch: null, hmr: false } });
    server = createHttpServer((request, response) => {
      if (request.url === '/__grid_fixture') {
        response.setHeader('Content-Type', 'text/html; charset=utf-8'); response.end(fixtureHtml); return;
      }
      vite.middlewares(request, response, () => { response.writeHead(404); response.end(); });
    });
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
    closeServer = async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); await vite.close(); };
  }
  const port = server.address().port;
  console.log(`临时浏览器测试服务器已就绪：${port}`);
  browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  console.log('Chromium 已启动');
  const page = await browser.newPage({ viewport: { width: 1280, height: 640 } });
  console.log('浏览器页面已创建');
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on('pageerror', error => { errors.push(error.message); console.error(error.message); });
  await mkdir(path.join(root, 'artifacts'), { recursive: true });
  const settle = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const passed = [];
  const cell = (row, col) => page.locator(`.grid-table tbody tr[data-row="${row}"] td[data-col="${col}"]`);
  const text = () => page.locator('.grid-cell-input');
  const picker = () => page.locator('.grid-cell-picker');
  const open = async (row, col) => { await cell(row, col).dblclick(); await text().waitFor(); await settle(); };
  const commitValue = async expected => {
    await page.keyboard.press('Enter'); await settle();
    const commits = await page.evaluate(() => window.commits);
    assert.equal(commits.length, 1, 'Enter 只提交一次');
    assert.equal(commits[0].value, expected);
    await page.evaluate(() => window.grid.el.querySelector('.grid-scroll').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', repeat: true, bubbles: true })));
    assert.equal(await page.evaluate(() => window.grid.editingActive), false, '持续按住 Enter 不能重新打开编辑器');
  };
  if (production) {
    const source = await readFile(path.join(root, 'public/preview-bootstrap.js'), 'utf8');
    const demo = source.indexOf('// 自动演示：');
    assert.ok(demo > 0);
    await page.route('**/preview-bootstrap.js*', route => route.fulfill({ contentType: 'text/javascript', body: source.slice(0, demo) + '\n} catch (e) { throw e; }\n' }));
    await page.addInitScript(({ columns, rows }) => {
      let mock;
      window.fixturePageRequests = [];
      Object.defineProperty(window, '__PREVIEW_MOCK__', { configurable: true, get: () => mock, set: value => {
        mock = value;
        const settings = value.get_settings;
        mock.get_settings = () => ({ ...settings(), rows_per_page: 200 });
        mock.get_draft_recovery_enabled = () => false;
        mock['plugin:event|listen'] = () => 1; mock['plugin:event|unlisten'] = () => null; mock.list_editing_drafts = () => [];
        mock.page_table = ({ req }) => { window.fixturePageRequests.push({ offset: req.offset, limit: req.limit }); return { columns, rows: structuredClone(rows), total: 200, primary_keys: ['id'], elapsed_ms: 0 }; };
      } });
    }, { columns, rows });
    await page.goto(`http://127.0.0.1:${port}/preview.html`, { waitUntil: 'domcontentloaded', timeout: 20000 });
    await page.waitForFunction(() => window.__APP__?.connections?.length);
    await page.evaluate(() => window.__APP__.openTableTab('c1', 'dbswitch', 'editing_fixture', 'table'));
    await page.waitForFunction(() => window.__APP__.activeView()?.grid?.rows.length === 200);
    await page.evaluate(() => { window.grid = window.__APP__.activeView().grid; });
    await open(0, 1);
    assert.equal(await page.evaluate(() => window.grid.editingActive), true);
    assert.equal(await page.evaluate(() => document.activeElement === window.grid.el.querySelector('.grid-cell-input')), true);
    assert.ok(await page.evaluate(() => window.__APP__.activeView().cellPreviewEl.getBoundingClientRect().height) >= 40);
    await text().fill('实际产品页草稿'); await text().click();
    await page.setViewportSize({ width: 1200, height: 480 }); await settle();
    assert.equal(await text().inputValue(), '实际产品页草稿');
    await page.keyboard.press('Enter'); await settle();
    assert.equal(await cell(0, 1).textContent(), '实际产品页草稿');
    await open(0, 4);
    assert.equal(await picker().getAttribute('type'), 'datetime-local');
    await picker().click({ position: { x: 4, y: 10 } });
    assert.equal(await page.evaluate(() => window.grid.editingActive), true);
    await picker().fill('2026-12-11T13:35:57');
    assert.equal(await text().inputValue(), '2026-12-11 13:35:57.123456+08:00');
    await page.screenshot({ path: path.join(root, 'artifacts/data-grid-editing-picker.png'), animations: 'disabled' });
    await text().click(); await page.keyboard.press('Escape'); await settle();
    assert.equal(await cell(0, 4).textContent(), rows[0][4]);
    const requests = await page.evaluate(() => window.fixturePageRequests);
    assert.ok(requests.every(req => req.limit === 200));
    passed.push('产品 DataTabView 双击展开预览后编辑保持、点击和 resize 不丢焦点、Enter 进入草稿', '产品日期时间 picker 保留小数秒/offset，Escape 取消');
  } else {
    await page.goto(`http://127.0.0.1:${port}/__grid_fixture`, { waitUntil: 'domcontentloaded', timeout: 20000 });
    console.log('DataGrid fixture 页面已加载');
    await page.waitForFunction(() => window.grid?.rows.length === 200);
    const contextMenus = await page.evaluate(async () => {
      const { DataGrid } = await import('/src/ui/DataGrid.ts');
      const results = [];
      for (const dedicated of [true, false]) {
        const calls = [], order = [];
        const host = document.createElement('div'); host.style.cssText = 'height:180px;width:500px;display:flex';
        const testGrid = new DataGrid({
          ...(dedicated ? { onHeaderContextMenu: (...args) => { calls.push(['header', ...args]); order.push('header'); } } : {}),
          onContextMenu: (...args) => { calls.push(['generic', ...args]); order.push('generic'); },
        });
        host.appendChild(testGrid.el); document.body.appendChild(host);
        try {
          testGrid.setData({ columns: [{ name: 'team', type: 'TEXT', nullable: true, primary_key: false, auto_increment: false }], rows: [['A']] });
          await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
          const scroll = testGrid.el.querySelector('.grid-scroll');
          const header = testGrid.el.querySelector('th[data-col="0"] .grid-th-name');
          const cell = testGrid.el.querySelector('td[data-col="0"]');
          const row = testGrid.el.querySelector('tbody tr[data-row="0"] .grid-row-header');
          scroll.addEventListener('contextmenu', () => order.push('scroll'));
          const dispatch = target => {
            const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: 34, clientY: 56 });
            target.dispatchEvent(e); return { prevented: e.defaultPrevented, stopped: e.cancelBubble };
          };
          const headerEvent = dispatch(header);
          const afterHeader = { calls: calls.slice(), order: order.slice() };
          const cellEvent = dispatch(cell), rowEvent = dispatch(row), blankEvent = dispatch(scroll);
          results.push({ dedicated, headerEvent, afterHeader, cellEvent, rowEvent, blankEvent, calls, order });
        } finally { testGrid.dispose(); host.remove(); }
      }
      return results;
    });
    const [dedicated, fallback] = contextMenus;
    assert.deepEqual(dedicated.afterHeader, { calls: [['header', 'team', 0, 34, 56]], order: ['header'] }, '真实列头冒泡链只分派专属回调一次，不进入滚动容器的通用菜单');
    assert.deepEqual(fallback.afterHeader, { calls: [['generic', 34, 56, -1, -1]], order: ['generic', 'scroll'] }, '没有专属列头回调时，原通用菜单应继续收到 header 事件');
    for (const result of contextMenus) {
      assert.deepEqual(result.calls.slice(1), [['generic', 34, 56, 0, 0], ['generic', 34, 56, 0, -1], ['generic', 34, 56, -1, -1]]);
      assert.ok([result.headerEvent, result.cellEvent, result.rowEvent, result.blankEvent].every(event => event.prevented), '四类右键均阻止浏览器默认菜单');
      assert.deepEqual(result.order.slice(result.dedicated ? 1 : 2), ['generic', 'scroll', 'generic', 'scroll', 'generic', 'scroll']);
    }
    passed.push('真实 DOM 冒泡：专属列头菜单只分派一次且不被 generic 覆盖；单元格/行/空白区与无专属列头的 fallback 均保留');
    const reset = async options => { await page.evaluate(options => window.resetGrid(options), options); await settle(); };
    const sample = () => page.evaluate(() => {
      const scroll = window.grid.el.querySelector('.grid-scroll');
      const rendered = [...scroll.querySelectorAll('tbody tr[data-row]')];
      return { height: scroll.clientHeight, rowHeight: rendered[0].getBoundingClientRect().height, rows: rendered.length, scrollHeight: scroll.scrollHeight, headerHeight: scroll.querySelector('thead').getBoundingClientRect().height, indices: rendered.map(row => Number(row.dataset.row)) };
    });
    await open(0, 1);
    await text().fill('仍在编辑');
    await page.evaluate(() => {
      window.savedInput = document.activeElement; window.savedRow = window.savedInput.closest('tr'); window.editorWasRemoved = false;
      window.editorObserver = new MutationObserver(records => { for (const record of records) for (const node of record.removedNodes) if (node === window.savedRow || node.contains?.(window.savedInput)) window.editorWasRemoved = true; });
      window.editorObserver.observe(window.grid.el.querySelector('tbody'), { childList: true, subtree: true });
    });
    await text().click(); await text().dblclick();
    await page.evaluate(() => { document.querySelector('#frame').style.height = '210px'; }); await settle();
    assert.equal(await page.evaluate(() => document.activeElement === window.savedInput && window.savedInput.isConnected && window.savedRow === window.savedInput.closest('tr')), true);
    assert.equal(await text().inputValue(), '仍在编辑');
    for (const scrollTop of [1800, 3600, 5200, 900, 0]) {
      await page.evaluate(scrollTop => { window.grid.el.querySelector('.grid-scroll').scrollTop = scrollTop; }, scrollTop); await settle();
      const current = await sample();
      assert.ok(current.rows <= Math.ceil(current.height / current.rowHeight) + 17, `虚拟行保持有界：${JSON.stringify(current)}`);
      assert.equal(current.indices.filter(i => i === 0).length, 1);
      assert.ok(Math.abs(current.scrollHeight - (200 * current.rowHeight + current.headerHeight)) <= current.rowHeight);
      assert.equal(await page.evaluate(() => document.activeElement === window.savedInput && window.savedInput.isConnected), true);
    }
    assert.equal(await page.evaluate(() => window.editorWasRemoved), false, '编辑行在整个重建过程中不能脱离 DOM');
    await page.evaluate(() => window.editorObserver.disconnect());
    await commitValue('仍在编辑');
    passed.push('200 行、受限高度、预览展开、resize/scroll 中活编辑行始终原位连接且不丢焦点', '可视窗口外只保留一个编辑行、spacer 总高度正确、输入点击/双击不重启');

    await reset(); await open(0, 1); await text().fill('应取消'); await page.keyboard.press('Escape'); await settle();
    assert.deepEqual(await page.evaluate(() => window.commits), []); assert.equal(await cell(0, 1).textContent(), rows[0][1]);
    await open(0, 1); await text().fill('真正离开'); await page.locator('#outside').click(); await settle();
    assert.deepEqual(await page.evaluate(() => window.commits.map(c => c.value)), ['真正离开']);
    passed.push('Escape 取消，真正离开组件只提交一次');

    for (const [col, value, expected] of [[5, undefined, null], [5, '', ''], [6, undefined, ''], [1, '', '']]) {
      await reset(); await open(0, col);
      if (value === '' && col === 5) await page.keyboard.press('Backspace');
      else if (value !== undefined) await text().fill(value);
      await commitValue(expected);
    }
    passed.push('未修改 NULL 保持 null；明确清空和既有空串保持空串');
    for (const [col, type, picked, expected] of [
      [2, 'date', '2026-12-11', '2026-12-11'],
      [3, 'time', '13:35:57', '13:35:57.654321-05:00'],
      [4, 'datetime-local', '2026-12-11T13:35:57', '2026-12-11 13:35:57.123456+08:00'],
    ]) {
      await reset(); await open(0, col);
      assert.equal(await picker().getAttribute('type'), type);
      assert.ok(await text().getAttribute('aria-label')); assert.ok(await picker().getAttribute('aria-label'));
      await picker().click({ position: { x: 4, y: 10 } }); await settle(); assert.equal(await page.evaluate(() => window.grid.editingActive), true);
      await picker().fill(picked); assert.equal(await text().inputValue(), expected);
      await text().click(); assert.equal(await page.evaluate(() => window.commits.length), 0);
      await commitValue(expected);
    }
    await reset(); await open(0, 4); await commitValue(rows[0][4]);
    await reset(); await open(0, 4); await picker().fill(''); assert.equal(await text().inputValue(), rows[0][4]); await page.keyboard.press('Escape');
    for (const value of ['-25:12:34.123456', '127:00:00', '23:59:59.999999+08:00']) {
      await reset(); await open(0, 3); await text().fill(value); await commitValue(value);
    }
    passed.push('date/time/datetime-local 原生控件与文本互切不提交，小数秒/offset/原始分隔符不丢失', '负数及超过24小时的 MySQL TIME 可直接输入，清空 picker 不清原文');

    for (const options of [{ visible: [0, 5, 3] }, { rows: [['新页', '新记录']] }, { readOnly: true }]) {
      await reset(); await open(0, 1); await text().fill('旧编辑不能提交');
      await page.evaluate(options => window.resetGrid(options), options); await settle();
      assert.equal(await page.evaluate(() => window.grid.editingActive), false); assert.deepEqual(await page.evaluate(() => window.commits), []);
    }
    await reset({ visible: [0, 5, 3] }); await open(0, 2); assert.equal(await picker().getAttribute('type'), 'time'); await text().fill('-30:20:10.123456'); await commitValue('-30:20:10.123456');
    assert.equal(await page.evaluate(() => window.commits[0].originalCol), 3);
    assert.equal(await page.evaluate(() => window.getFullRows()[0][5]), null);
    passed.push('setData 换页/列映射变化/只读切换使旧编辑失效，隐藏列后编辑提交正确原始列');

    await reset(); await open(0, 1); await text().fill('picker 外也能强制提交'); await page.evaluate(() => window.grid.flushEditing()); await settle();
    assert.equal(await page.evaluate(() => window.commits.length), 1);
    await reset(); await open(0, 4); await picker().focus(); await page.evaluate(() => window.grid.flushEditing()); await settle(); assert.equal(await page.evaluate(() => window.commits.length), 1);
    await reset(); await open(0, 1); await page.evaluate(() => window.grid.setState({ selectedRows: new Set([1]) })); await settle(); assert.equal(await page.evaluate(() => document.activeElement.classList.contains('grid-cell-input')), true); await page.keyboard.press('Escape');
    await reset({ readOnly: true }); await cell(0, 1).dblclick(); assert.equal(await page.evaluate(() => window.grid.editingActive), false); assert.equal(await page.evaluate(() => window.grid.beginEdit(0, 1, '禁止')), null);
    passed.push('flushEditing 在文本或 picker 焦点下同步提交一次，状态更新保留编辑，只读网格不能 beginEdit');

    await reset();
    await page.evaluate(() => { window.grid.setState({ selectedRows: new Set([0, 1]), dirtyCells: new Set(['0:2']), rowFlags: { 1: 'delete' } }); window.grid.selectedColumn = 1; });
    await settle(); await page.mouse.move(1200, 600);
    const colors = await page.evaluate(() => {
      const color = selector => getComputedStyle(window.grid.el.querySelector(selector));
      return { row: color('tr[data-row="0"] td[data-col="0"]').backgroundColor, column: color('tr[data-row="2"] td[data-col="1"]').backgroundColor,
        intersection: color('tr[data-row="0"] td[data-col="1"]').boxShadow, dirty: color('tr[data-row="0"] td[data-col="2"]').backgroundColor,
        oddRow: color('tr[data-row="1"] td[data-col="0"]').backgroundColor, oddColumn: color('tr[data-row="3"] td[data-col="1"]').backgroundColor,
        deleted: color('tr[data-row="1"] td[data-col="0"]').textDecorationLine, header: color('th[data-col="1"]').backgroundColor };
    });
    assert.notEqual(colors.row, colors.column); assert.equal(colors.header, colors.column); assert.notEqual(colors.intersection, 'none'); assert.notEqual(colors.dirty, colors.row); assert.equal(colors.deleted, 'line-through');
    assert.equal(colors.oddRow, colors.row); assert.equal(colors.oddColumn, colors.column);
    const iconFrames = await page.locator('#icons svg').evaluateAll(icons => icons.map(svg => ({ box: svg.getAttribute('viewBox'), fill: svg.querySelector('rect').getAttribute('fill'), stroke: svg.querySelector('rect').getAttribute('stroke'), frame: ['x', 'y', 'width', 'height', 'rx', 'stroke-width'].map(name => svg.querySelector('rect').getAttribute(name)) })));
    assert.equal(iconFrames.length, 4); assert.ok(iconFrames.every(icon => icon.box === '0 0 16 16' && icon.fill && icon.stroke)); assert.ok(iconFrames.every(icon => JSON.stringify(icon.frame) === JSON.stringify(iconFrames[0].frame)));
    passed.push('行/列/交叉层级与 dirty/deleted 状态共存，四个 MINI 图标具有相同16网格外框');
  }
  const screenshot = `artifacts/data-grid-editing-${production ? 'production' : 'source'}.png`;
  await page.screenshot({ path: path.join(root, screenshot), animations: 'disabled' });
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ mode: production ? 'production-browser-fixture-ipc' : 'source-browser-real-dom', passed, screenshot }, null, 2));
} catch (error) {
  console.error(error);
  throw error;
} finally {
  try { await browser?.close(); } finally { await closeServer?.(); }
}
