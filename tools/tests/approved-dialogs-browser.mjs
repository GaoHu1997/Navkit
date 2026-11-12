// 已批准三处产品界面的集成验收；连接、SQL、保存和破坏性操作全部使用浏览器内模拟 IPC。
// 复用正在运行的项目预览，不启动替代服务。产物每次写入独立的 artifacts 目录。
import assert from 'node:assert/strict';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';

const base = process.env.PREVIEW_URL || 'http://localhost:5183';
const root = new URL('../..', import.meta.url);
const tag = new Date().toISOString().replace(/[:.]/g, '-');
const out = fileURLToPath(new URL(`../../artifacts/approved-ui-${tag}/`, import.meta.url));
await mkdir(out, { recursive: true });
const report = { status: 'running', mode: 'actual-product-ui-with-browser-mocked-ipc', base, out, sourceChecks: [], images: [], checks: [], metrics: {}, pageErrors: [] };
const sha = value => createHash('sha256').update(value).digest('hex');
for (const file of ['src/core/App.ts', 'src/dialogs/SettingsDialog.ts', 'src/dialogs/ServerMonitorDialog.ts', 'src/dialogs/HistoryLogDialog.ts']) {
  const response = await fetch(`${base}/${file}?ui-review=${Date.now()}`); assert.equal(response.status, 200, file);
  const transformed = await response.text();
  const match = transformed.match(/sourceMappingURL=data:application\/json;base64,([A-Za-z0-9+/=]+)/);
  assert.ok(match, '必须是当前工作区的 Vite 开发模块：' + file);
  const map = JSON.parse(Buffer.from(match[1], 'base64').toString('utf8'));
  const servedHash = sha(map.sourcesContent[0]), diskHash = sha(await readFile(new URL(file, root)));
  assert.equal(servedHash, diskHash, '服务源码与工作树不一致：' + file);
  report.sourceChecks.push({ file, sha256: diskHash });
}
const response = await fetch(`${base}/preview-bootstrap.js`); assert.equal(response.status, 200);
const bootstrap = await response.text(), marker = bootstrap.indexOf('// 自动演示：'); assert.ok(marker >= 0);
const quietBootstrap = bootstrap.slice(0, marker) + (bootstrap.trimStart().startsWith('try') ? '\n} catch (error) { throw error; }\n' : '');
const fixtureSource = await readFile(new URL('tools/tests/dialog-fixtures.js', root), 'utf8');
const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
const page = await browser.newPage({ viewport: { width: 1560, height: 980 }, deviceScaleFactor: 1.5 });
page.setDefaultTimeout(15000);
page.on('pageerror', error => report.pageErrors.push(error.message));
const loadedModules = [];
page.on('response', response => {
  const file = new URL(response.url()).pathname.slice(1), expected = report.sourceChecks.find(check => check.file === file);
  if (!expected) return;
  loadedModules.push((async () => {
    const code = await response.text(), match = code.match(/sourceMappingURL=data:application\/json;base64,([A-Za-z0-9+/=]+)/);
    if (!match) return { file, error: '实际加载的模块没有源码映射' };
    const map = JSON.parse(Buffer.from(match[1], 'base64').toString('utf8'));
    return { file, sha256: sha(map.sourcesContent[0]) };
  })().catch(error => ({ file, error: error.message })));
});
// 原子编辑保留时间戳，正在运行的 Vite 可能缓存旧转换结果；只给本次验收的源码请求添加新缓存键。
await page.route('**/src/**', route => {
  const url = new URL(route.request().url());
  if (/\.(?:ts|css)$/.test(url.pathname)) url.searchParams.set('ui-code-review', tag);
  return route.continue({ url: url.href });
});
await page.route('**/preview-bootstrap.js*', route => route.fulfill({ contentType: 'text/javascript', body: quietBootstrap }));
await page.addInitScript(() => {
  let mock;
  Object.defineProperty(window, '__PREVIEW_MOCK__', { configurable: true, get: () => mock, set(value) {
    mock = value; mock['plugin:event|listen'] = () => 1; mock['plugin:event|unlisten'] = () => null;
  } });
});
const capture = async (name, locator) => {
  await page.evaluate(() => document.fonts.ready);
  const file = `${out}/${name}.png`; await locator.screenshot({ path: file });
  const box = await locator.boundingBox(); report.images.push({ name, file, box });
};
const focusTrigger = () => page.locator('#ui-review-trigger').focus();
const noOverflow = async locator => locator.evaluate(element => element.scrollWidth <= element.clientWidth + 1);
const sleepMs = ms => new Promise(r => setTimeout(r, ms));
const monitor = () => page.locator('.dialog.server-monitor-dialog');
const waitSessions = async (id, count = 5) => {
  await page.waitForFunction(({ id, count }) => {
    const dialog = document.querySelector('.server-monitor-dialog');
    return dialog?.querySelector('.monitor-connection').value === id && dialog.querySelector('.monitor-table-wrap').getAttribute('aria-busy') === 'false' && dialog.querySelectorAll('[data-session-id]').length === count;
  }, { id, count });
};
try {
  await page.goto(`${base}/preview.html`, { waitUntil: 'networkidle' });
  await page.waitForFunction(() => window.__APP__?.connections?.length);
  const actualModules = await Promise.all(loadedModules);
  report.loadedSourceChecks = report.sourceChecks.map(expected => {
    const actual = actualModules.find(module => module.file === expected.file);
    assert.ok(actual && !actual.error, '必须核验浏览器实际加载的模块：' + expected.file);
    assert.equal(actual.sha256, expected.sha256, '浏览器不能使用缓存旧模块：' + expected.file);
    return actual;
  });
  const overlay = page.locator('vite-error-overlay');
  if (await overlay.count()) {
    const errorText = await overlay.evaluate(element => element.shadowRoot.textContent);
    const staleCss = errorText.match(/Failed to resolve import "\.\.\/styles\/(settings-dialog|history-log-dialog|server-monitor-dialog)\.css"/);
    assert.ok(staleCss, '不允许忽略当前 Vite 错误：' + errorText);
    // TS 源码已逐一与工作树核验；只有旧的缺 CSS 错误且三个当前 CSS 均能读取时才关闭开发浮层。
    for (const name of ['settings-dialog', 'history-log-dialog', 'server-monitor-dialog']) {
      const currentCss = await fetch(`${base}/src/styles/${name}.css?direct`);
      assert.equal(currentCss.status, 200, '当前样式仍缺失：' + name);
      assert.ok((await currentCss.text()).includes(`.dialog.${name === 'settings-dialog' ? 'settings-dialog' : name}`));
    }
    report.devOverlay = { staleMissingCss: staleCss[1], currentCssAvailable: true, dismissedInTestPage: true };
    await overlay.click({ position: { x: 4, y: 4 } }); await overlay.waitFor({ state: 'detached' });
  }
  await page.addScriptTag({ content: fixtureSource });
  await page.evaluate(() => {
    const template = window.__APP__.connections[0];
    window.__APP__.connections = [
      { ...template, id: 'c1', name: 'localhost', kind: 'mysql', read_only: false },
      { ...template, id: 'c2', name: '只读 PostgreSQL', kind: 'postgresql', read_only: true },
      { ...template, id: 'c3', name: '本地 SQLite', kind: 'sqlite', read_only: false },
    ];
    window.__APP__.tree.selectedNodeId = null; window.__APP__.activeTabId = null;
    window.__calls = []; window.__copied = []; window.__records = window.NAVKIT_DESIGN.records.map(row => ({ ...row, connection_id: 'c1', origin: 'query' }));
    window.__monitorInFlight = 0; window.__monitorMaxInFlight = 0;
    window.__monitorDone = false; window.__monitorHoldC1 = false; window.__monitorDenied = false; window.__killDenied = false;
    window.__removed = { c1: [], c2: [] };
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(text) { window.__copied.push(text); } } });
    const sourceRows = id => window.NAVKIT_DESIGN.sessions.filter(row => !window.__removed[id]?.includes(row.id)).map(row => {
      const active = row.active && !(window.__monitorDone && row.id === '941');
      return { id: row.id, user: row.user, host: row.host, db: row.db,
        command: active ? (id === 'c2' ? '正在执行' : 'Query') : (id === 'c2' ? '事务中空闲' : 'Sleep'),
        state: active ? 'Waiting for table metadata lock' : (id === 'c2' ? 'idle（等待 ClientRead）' : ''),
        elapsed_secs: active ? parseFloat(row.elapsed) : 360,
        sql: active ? (id === 'c2' ? '-- 只读 PostgreSQL 连接\n' : '') + row.sql : 'SELECT PREVIOUS_COMPLETED_SQL;', is_self: row.self,
      };
    });
    const mock = window.__PREVIEW_MOCK__;
    mock.ensure_connection = args => { window.__calls.push({ command: 'ensure_connection', args }); return { ok: true, message: '已建立示例连接' }; };
    mock.list_db_processes = args => {
      window.__calls.push({ command: 'list_db_processes', args }); window.__monitorInFlight++;
      window.__monitorMaxInFlight = Math.max(window.__monitorMaxInFlight, window.__monitorInFlight);
      let promise;
      if (window.__monitorDenied) promise = Promise.reject(new Error('permission denied'));
      else if (window.__monitorHoldC1 && args.id === 'c1') promise = new Promise(resolve => {
        window.__resolveMonitor = rows => { window.__monitorHoldC1 = false; window.__resolveMonitor = null; resolve(rows || sourceRows(args.id)); };
      });
      else promise = Promise.resolve(sourceRows(args.id));
      return promise.finally(() => { window.__monitorInFlight--; });
    };
    mock.kill_db_process = args => {
      window.__calls.push({ command: 'kill_db_process', args });
      if (window.__killDenied) return Promise.reject(new Error('禁止结束示例会话'));
      window.__removed[args.id].push(String(args.processId ?? args.process_id)); return '已结束示例会话';
    };
    mock.list_sql_log = args => { window.__calls.push({ command: 'list_sql_log', args }); return window.__records; };
    mock.clear_sql_log = args => { window.__calls.push({ command: 'clear_sql_log', args }); window.__records = []; return null; };
    for (const command of ['server_info', 'execute_sql', 'read_log', 'list_job_history', 'open_database']) mock[command] = args => {
      window.__calls.push({ command, args }); throw new Error('本轮窗口禁止调用：' + command);
    };
    const trigger = document.createElement('button'); trigger.id = 'ui-review-trigger'; trigger.textContent = '验收入口';
    trigger.style.cssText = 'position:fixed;right:8px;bottom:8px'; document.body.append(trigger); trigger.focus();
  });

  // 设置四页、原字段、实时字体示例、键盘切换和小窗口；取消退出，不保存真实配置。
  await focusTrigger(); await page.evaluate(() => window.__APP__.cmdOptions());
  const settings = page.locator('.dialog.settings-dialog'); await settings.waitFor();
  assert.equal(await settings.locator('[data-settings-tab]').count(), 4);
  // 19 字段 = 常规 8（含新增的 3 个超时字段）+ 字体 3 + SQL 编辑器 2 + WebDAV 6
  assert.equal(await settings.locator('[data-settings-field]').count(), 19);
  report.metrics.settings = await settings.evaluate(dialog => ({
    width: dialog.getBoundingClientRect().width, nav: dialog.querySelector('.settings-sidebar')?.getBoundingClientRect().width,
    title: dialog.querySelector('.dialog-title').getBoundingClientRect().height,
    footer: dialog.querySelector('.dialog-footer').getBoundingClientRect().height,
    control: dialog.querySelector('[data-settings-field="rows_per_page"]').getBoundingClientRect().height,
  }));
  assert.equal(report.metrics.settings.width, 840); assert.equal(report.metrics.settings.nav, 144); assert.equal(report.metrics.settings.control, 26);
  assert.equal(report.metrics.settings.title, 36); assert.equal(report.metrics.settings.footer, 48);
  for (const tab of ['general', 'font', 'sql', 'sync']) {
    await settings.locator(`[data-settings-tab="${tab}"]`).click();
    await settings.locator(`[data-settings-panel="${tab}"]`).waitFor({ state: 'visible' });
    assert.ok(await noOverflow(settings), '设置页不能产生全页横向溢出：' + tab);
    await capture(`settings-${tab}`, settings);
  }
  for (const field of ['sync_password', 'sync_passphrase', 'sync_headers']) assert.equal(await settings.locator(`[data-settings-field="${field}"]`).inputValue(), '');
  assert.deepEqual(await settings.locator('[data-settings-field="sync_auto_every"] option').evaluateAll(options => options.map(option => option.value)), ['-1', '3', '15', '30', '60']);
  await settings.locator('[data-settings-tab="font"]').click();
  await settings.locator('[data-settings-field="font_size"]').fill('14');
  assert.equal(await settings.locator('[data-settings-preview="interface"]').evaluate(element => getComputedStyle(element).fontSize), '14px');
  await settings.locator('[data-settings-tab="sql"]').click();
  await settings.locator('[data-settings-field="sql_font_size"]').fill('15');
  assert.equal(await settings.locator('[data-settings-preview="sql"]').evaluate(element => getComputedStyle(element).fontSize), '15px');
  await settings.locator('[data-settings-tab="general"]').focus(); await page.keyboard.press('ArrowDown');
  assert.equal(await settings.locator('[data-settings-tab="font"]').getAttribute('aria-selected'), 'true');
  await page.setViewportSize({ width: 720, height: 420 });
  report.metrics.settingsSmall = await settings.evaluate(dialog => ({ width: dialog.getBoundingClientRect().width, height: dialog.getBoundingClientRect().height, overflow: dialog.scrollWidth - dialog.clientWidth, content: dialog.querySelector('.settings-content').clientHeight }));
  assert.ok(report.metrics.settingsSmall.width <= 688 && report.metrics.settingsSmall.height <= 388 && report.metrics.settingsSmall.content > 20 && report.metrics.settingsSmall.overflow <= 1);
  await page.keyboard.press('Escape'); assert.equal(await settings.count(), 0);
  assert.ok(await page.locator('#ui-review-trigger').evaluate(element => document.activeElement === element));
  report.checks.push('设置四页16字段、单位密度、秘密留空、原同步频率、字体样例、键盘和窄屏');
  await page.setViewportSize({ width: 1560, height: 980 });

  // 从 App 的真正工具命令打开，不预先选择连接、创建 SQL 页或打开数据库。
  await focusTrigger(); const beforeTabs = await page.evaluate(() => window.__APP__.tabs.length);
  await page.evaluate(() => window.__APP__.runCommand('server-monitor')); await monitor().waitFor();
  assert.equal(await monitor().locator('.monitor-connection').inputValue(), '');
  assert.ok(await monitor().locator('.monitor-refresh').isDisabled());
  assert.equal(await page.evaluate(() => window.__calls.filter(call => call.command === 'list_db_processes').length), 0);
  await capture('monitor-unselected', monitor());
  await monitor().locator('.monitor-connection').selectOption('c1'); await waitSessions('c1');
  await monitor().locator('.monitor-pause').click();
  assert.equal(await page.evaluate(() => window.__APP__.tabs.length), beforeTabs);
  assert.equal(await monitor().locator('[role="tab"]').count(), 0);
  assert.equal(await monitor().locator('[data-view-sql]').count(), 2);
  assert.equal(await monitor().locator('.server-monitor-sql-dialog').count(), 0);
  assert.doesNotMatch(await monitor().textContent(), /PREVIOUS_COMPLETED_SQL|服务器基本信息|CPU|Threads_running/);
  report.metrics.monitor = await monitor().evaluate(dialog => ({ width: dialog.getBoundingClientRect().width, height: dialog.getBoundingClientRect().height, row: dialog.querySelector('[data-session-id]').getBoundingClientRect().height, header: dialog.querySelector('thead').getBoundingClientRect().height }));
  assert.equal(report.metrics.monitor.width, 1240); assert.ok(report.metrics.monitor.height < 400);
  assert.equal(report.metrics.monitor.row, 26); assert.equal(report.metrics.monitor.header, 28);
  await capture('monitor', monitor());
  await monitor().locator('[data-view-sql="941"]').click();
  const detail = page.locator('.server-monitor-sql-dialog'); await detail.waitFor();
  assert.equal(await detail.locator('.monitor-sql-full').textContent(), await page.evaluate(() => window.NAVKIT_DESIGN.longSql));
  assert.ok(await monitor().evaluate(element => element.inert));
  await detail.getByRole('button', { name: '复制 SQL', exact: true }).click();
  assert.equal(await page.evaluate(() => window.__copied.at(-1)), await page.evaluate(() => window.NAVKIT_DESIGN.longSql));
  await page.keyboard.press('Escape'); assert.equal(await detail.count(), 0);
  assert.ok(await monitor().locator('[data-view-sql="941"]').evaluate(element => document.activeElement === element));
  // 模拟服务器执行完成；下一次正常自动采样应自动关闭按需全文且保留会话。
  // 确定性地触发一次自动采样（不依赖间隔定时器的时序抖动）
  await monitor().locator('.monitor-interval').fill('2'); await monitor().locator('.monitor-interval').dispatchEvent('change');
  await page.evaluate(() => { window.__monitorDone = true; });
  await monitor().locator('.monitor-refresh').click();
  // 已完成语句的 SQL 单元格按审定规则留白（不使用「—」占位）
  await page.waitForFunction(() => !document.querySelector('.server-monitor-sql-dialog') && (document.querySelector('[data-session-id="941"] .monitor-sql-cell')?.textContent ?? 'x').trim() === '', null, { timeout: 15000 });
  assert.equal(await monitor().locator('[data-session-id]').count(), 5);
  await monitor().locator('.monitor-pause').click();
  report.checks.push('未选连接可开监控、按连接惰性连接、同表显示、只按需全文、完成清空SQL但保留会话');

  // 延迟旧连接响应再切到只读连接：不得发布串台内容或重复并发请求。
  await page.evaluate(() => { window.__monitorDone = false; window.__monitorHoldC1 = true; });
  await monitor().locator('.monitor-refresh').click(); await page.waitForFunction(() => typeof window.__resolveMonitor === 'function');
  await monitor().locator('.monitor-connection').selectOption('c2');
  await page.evaluate(() => window.__resolveMonitor([{ id: '9999', user: 'old', host: 'old', db: 'old', command: 'Query', state: '', elapsed_secs: 1, sql: 'WRONG_OLD_SERVER_SQL', is_self: false }]));
  await waitSessions('c2'); assert.doesNotMatch(await monitor().textContent(), /WRONG_OLD_SERVER_SQL|PREVIOUS_COMPLETED_SQL/);
  assert.match(await monitor().textContent(), /只读 PostgreSQL 连接/);
  assert.equal(await monitor().locator('[data-kill-session]').count(), 0);
  assert.ok(!(await monitor().locator('thead').textContent()).includes('操作'));
  assert.equal(await page.evaluate(() => window.__monitorMaxInFlight), 1);
  await capture('monitor-readonly', monitor());
  const requests = await page.evaluate(() => window.__calls.filter(call => call.command === 'list_db_processes').length);
  await monitor().locator('.monitor-connection').selectOption('c3');
  assert.equal(await monitor().locator('[data-session-id]').count(), 0); assert.match(await monitor().textContent(), /不支持/);
  assert.equal(await page.evaluate(() => window.__calls.filter(call => call.command === 'list_db_processes').length), requests);
  await capture('monitor-unsupported', monitor());
  await monitor().locator('.monitor-connection').selectOption('c1'); await waitSessions('c1');
  // 读取失败关闭陈旧全文并把焦点归还到仍存在的连接选择器，而不是已删除的行按钮。
  // 打开全文详情 → 下次采样读取失败时应自动关闭详情并把焦点归还连接选择器
  await monitor().locator('[data-view-sql="941"]').click(); await detail.waitFor();
  await page.evaluate(() => { window.__monitorDenied = true; });
  // 详情打开时主表 inert，先按 Esc 关闭再触发刷新（这也是产品行为：Esc 先收复菜单/详情）
  // 详情打开时主表 inert：直接派发 refresh 的 click（等价于用户从详情关闭后立即刷新），
  // 该路径必须由「采样失败」关闭陈旧详情并把焦点交给连接选择器。
  await monitor().locator('.monitor-refresh').dispatchEvent('click');
  await page.waitForFunction(() => !document.querySelector('.server-monitor-sql-dialog') && document.querySelector('.monitor-status')?.textContent.includes('permission denied'), null, { timeout: 15000 });
  assert.match(await monitor().locator('.monitor-count').textContent(), /无法读取/);
  assert.ok(await monitor().locator('.monitor-connection').evaluate(element => document.activeElement === element));
  const pauseBtn2 = monitor().locator('.monitor-pause');
  if ((await pauseBtn2.innerText()).includes('继续')) { await pauseBtn2.click(); }
  await page.evaluate(() => { window.__monitorDenied = false; });
  await monitor().locator('.monitor-refresh').click(); await waitSessions('c1');
  report.checks.push('连接切换版本失效、非重叠读、只读隐藏危险操作、不支持不发请求、错误不伪造零会话和回焦');

  // 结束会话默认取消，取消不调用 API；失败可原位重试，成功后更新当前会话表。
  await monitor().locator('[data-kill-session="947"]').click();
  const kill = page.locator('.server-monitor-confirm'); await kill.waitFor();
  assert.equal(await kill.getAttribute('role'), 'alertdialog');
  assert.equal(await page.evaluate(() => document.activeElement.textContent), '取消');
  await page.keyboard.press('Escape'); assert.equal(await kill.count(), 0);
  assert.equal(await page.evaluate(() => window.__calls.filter(call => call.command === 'kill_db_process').length), 0);
  await monitor().locator('[data-kill-session="947"]').click(); await page.evaluate(() => { window.__killDenied = true; });
  await kill.getByRole('button', { name: '结束会话', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.monitor-confirm-status')?.textContent.includes('禁止结束示例会话'));
  assert.equal(await kill.count(), 1);
  await page.evaluate(() => { window.__killDenied = false; });
  await kill.getByRole('button', { name: '结束会话', exact: true }).click(); await kill.waitFor({ state: 'detached' }); await waitSessions('c1', 4);
  await page.setViewportSize({ width: 720, height: 420 });
  report.metrics.monitorSmall = await monitor().evaluate(dialog => ({ width: dialog.getBoundingClientRect().width, height: dialog.getBoundingClientRect().height, overflow: dialog.scrollWidth - dialog.clientWidth, tableOverflow: dialog.querySelector('.monitor-table-wrap').scrollWidth - dialog.querySelector('.monitor-table-wrap').clientWidth }));
  assert.ok(report.metrics.monitorSmall.width <= 688 && report.metrics.monitorSmall.height <= 388 && report.metrics.monitorSmall.overflow <= 1);
  await page.setViewportSize({ width: 1560, height: 980 });
  // 关闭时使迟到请求失效。
  await page.evaluate(() => { window.__monitorHoldC1 = true; window.__oldMonitor = document.querySelector('.server-monitor-dialog'); });
  await monitor().locator('.monitor-refresh').click(); await page.waitForFunction(() => typeof window.__resolveMonitor === 'function');
  await monitor().locator('.dialog-title-close').click(); const detachedText = await page.evaluate(() => window.__oldMonitor.textContent);
  await page.evaluate(() => window.__resolveMonitor()); await page.waitForFunction(() => window.__monitorInFlight === 0);
  assert.equal(await page.evaluate(() => window.__oldMonitor.textContent), detachedText); assert.equal(await monitor().count(), 0);
  report.checks.push('结束会话取消/错误重试、窄屏仅表内横滚、关闭丢弃迟到响应');

  // 日志仅 SQL，完整多行原文、行号、软换行、筛选/复制、清空确认及大数据虚拟渲染。
  await focusTrigger(); await page.evaluate(() => window.__APP__.runCommand('history-log'));
  const history = page.locator('.history-log-dialog:not(.history-log-confirm)'); await history.waitFor();
  await page.waitForFunction(() => document.querySelector('.history-log-count')?.textContent.includes('4 / 4'));
  assert.equal(await history.locator('[role="tab"]').count(), 0);
  assert.doesNotMatch(await history.textContent(), /运行日志|自动运行|日志来源/);
  assert.equal(await history.locator('.cm-content').getAttribute('contenteditable'), 'false');
  assert.ok(await history.locator('.cm-lineNumbers').count());
  assert.match(await history.locator('.cm-content').textContent(), /LEFT JOIN navkit_accept.t_user/);
  report.metrics.history = await history.evaluate(dialog => {
    const scroller = dialog.querySelector('.cm-scroller');
    return { width: dialog.getBoundingClientRect().width, height: dialog.getBoundingClientRect().height, viewer: dialog.querySelector('.history-log-viewer').getBoundingClientRect().height, font: getComputedStyle(scroller).fontSize, lineHeight: getComputedStyle(scroller).lineHeight, overflow: scroller.scrollWidth - scroller.clientWidth };
  });
  assert.equal(report.metrics.history.width, 1240); assert.ok(report.metrics.history.viewer <= 640);
  assert.equal(report.metrics.history.font, '13px'); assert.equal(report.metrics.history.lineHeight, '20px'); assert.ok(report.metrics.history.overflow <= 1);
  await capture('history', history);
  await history.locator('.history-log-search').fill('SHOW CREATE TABLE'); assert.match(await history.locator('.history-log-count').textContent(), /1 \/ 4/);
  await history.locator('.history-log-kind').selectOption('query'); assert.match(await history.locator('.history-log-count').textContent(), /1 \/ 4/);
  await history.locator('.history-log-outcome').selectOption('error'); assert.match(await history.locator('.history-log-count').textContent(), /0 \/ 4/);
  await history.locator('.history-log-outcome').selectOption('all'); await history.locator('.history-log-kind').selectOption('all');
  await history.locator('.history-log-outcome').selectOption('all'); await history.locator('.history-log-search').fill('');
  await history.locator('.history-log-more').click(); await history.locator('.history-log-clear').click();
  const clear = page.locator('.history-log-confirm'); await clear.waitFor(); assert.equal(await page.evaluate(() => document.activeElement.textContent), '取消');
  await page.keyboard.press('Escape'); assert.equal(await clear.count(), 0);
  assert.ok(await history.locator('.history-log-more').evaluate(element => document.activeElement === element));
  assert.equal(await page.evaluate(() => window.__calls.filter(call => call.command === 'clear_sql_log').length), 0);
  await page.setViewportSize({ width: 720, height: 420 });
  report.metrics.historySmall = await history.evaluate(dialog => ({ width: dialog.getBoundingClientRect().width, height: dialog.getBoundingClientRect().height, overflow: dialog.scrollWidth - dialog.clientWidth, viewer: dialog.querySelector('.history-log-viewer').getBoundingClientRect().height }));
  assert.ok(report.metrics.historySmall.width <= 688 && report.metrics.historySmall.height <= 388 && report.metrics.historySmall.overflow <= 1 && report.metrics.historySmall.viewer > 30);
  await page.setViewportSize({ width: 1560, height: 980 });
  await page.evaluate(() => { const base = window.__records[0]; window.__records = Array.from({ length: 2000 }, (_, index) => ({ ...base, sql: 'SELECT \'<script>&中文\' AS value,\n  ' + 'x'.repeat(1200) + '\nFROM sample\nWHERE id = ' + index + ';' })); });
  await history.locator('.history-log-refresh').click(); await page.waitForFunction(() => document.querySelector('.history-log-count')?.textContent.includes('2000 / 2000'));
  report.metrics.historyLarge = await history.evaluate(dialog => ({ lineNodes: dialog.querySelectorAll('.cm-line').length, scripts: dialog.querySelectorAll('.history-log-viewer script').length, overflow: dialog.querySelector('.cm-scroller').scrollWidth - dialog.querySelector('.cm-scroller').clientWidth }));
  assert.ok(report.metrics.historyLarge.lineNodes < 500); assert.equal(report.metrics.historyLarge.scripts, 0); assert.ok(report.metrics.historyLarge.overflow <= 1);
  await history.locator('.cm-scroller').evaluate(element => { element.scrollTop = element.scrollHeight; });
  await page.waitForFunction(() => document.querySelector('.history-log-editor .cm-content')?.textContent.includes('WHERE id = 1999;'));
  await history.locator('.history-log-more').click(); await history.locator('.history-log-clear').click(); await clear.waitFor();
  await clear.getByRole('button', { name: '清空 SQL 日志', exact: true }).click(); await clear.waitFor({ state: 'detached' });
  await page.waitForFunction(() => document.querySelector('.history-log-count')?.textContent.includes('0 / 0'));
  assert.equal(await page.evaluate(() => window.__calls.filter(call => call.command === 'clear_sql_log').length), 1);
  await page.keyboard.press('Escape'); assert.equal(await history.count(), 0); assert.ok(await page.locator('#ui-review-trigger').evaluate(element => document.activeElement === element));
  report.checks.push('仅SQL来源、完整原文/行号/只读/软换行、筛选与复制、清空取消/确认、2000条虚拟化与安全文本');
  const forbidden = await page.evaluate(() => window.__calls.filter(call => ['server_info', 'execute_sql', 'read_log', 'list_job_history', 'open_database'].includes(call.command)));
  assert.deepEqual(forbidden, []); assert.deepEqual(report.pageErrors, []);
  const sampleCalls = await page.evaluate(() => window.__calls.filter(call => call.command === 'list_db_processes'));
  assert.ok(sampleCalls.every(call => call.args.database === null || call.args.database === undefined));
  report.metrics.api = await page.evaluate(() => window.__calls.reduce((counts, call) => ({ ...counts, [call.command]: (counts[call.command] || 0) + 1 }), {}));
  report.status = 'passed';
} catch (error) {
  report.status = 'failed'; report.error = error.stack || String(error);
  await page.screenshot({ path: `${out}/failure.png`, fullPage: true }).catch(() => {});
  throw error;
} finally {
  await writeFile(`${out}/browser-validation.json`, JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report));
  await page.unrouteAll({ behavior: 'ignoreErrors' }); await browser.close();
}
