import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';

// 当前产品模块和样式 + 明确隔离的 IPC fixture，不连接或写入任何真实数据库。
const built = await build({ stdin: { contents: 'export { openImportWizard } from "./src/dialogs/ImportExportDialog.ts";', resolveDir: process.cwd(), loader: 'ts' }, bundle: true, write: false, platform: 'browser', format: 'iife', globalName: 'ImportFixture', logLevel: 'silent' });
const css = await readFile('src/styles/base.css', 'utf8');

test('导入向导：真实浏览器预览布局与目标回显回归（隔离 IPC）', async t => {
  const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 850 } });
    page.setDefaultTimeout(10000);
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('https://import-fixture.test/**', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><html lang="zh-CN"><body></body></html>' }));
    await page.goto('https://import-fixture.test/');
    await page.addStyleTag({ content: css });
    await page.addScriptTag({ content: built.outputFiles[0].text });
    await page.evaluate(() => {
      window.f = { tableMode: '', csvMode: '', tableGates: [], csvGates: [], csvCalls: [], imports: [] };
      const f = window.f;
      window.__PREVIEW_MOCK__ = {
        list_databases: () => [['main', ''], ['other', ''], ['third', '']],
        list_tables: ({ schemaName }) => {
          if (f.tableMode === 'wait') return new Promise((resolve, reject) => f.tableGates.push({ schemaName, resolve, reject }));
          if (f.tableMode === 'error') throw Error('fixture metadata offline');
          if (f.tableMode === 'empty') return [];
          return schemaName === 'main' ? [['t_comment', 'table'], ['alternate', 'table'], ['a_view', 'view']] : [[schemaName + '_table', 'table']];
        },
        preview_csv: args => {
          f.csvCalls.push(args);
          if (f.csvMode === 'wait') return new Promise((resolve, reject) => f.csvGates.push({ resolve, reject }));
          if (f.csvMode === 'error') throw Error('fixture CSV unreadable');
          if (f.csvMode === 'empty') return [[], []];
          if (f.csvMode === 'header-only') return [['id', 'note'], []];
          if (f.csvMode === 'wide') return [Array.from({ length: 8 }, (_, i) => 'very_long_column_' + i), Array.from({ length: 10 }, (_, i) => Array.from({ length: 8 }, () => String(i) + '_long_sample_'.repeat(50)))];
          return [['id', 'note'], Array.from({ length: 10 }, (_, i) => [String(i + 1), i === 0 ? '备注1' : i === 1 ? String.fromCharCode(92) + 'N' : i === 2 ? '' : i + '_sample_note'])];
        },
        get_table_schema: args => {
          f.metaCalls ??= []; f.metaGates ??= []; f.metaCalls.push(args);
          if (f.metaMode === 'wait') return new Promise((resolve, reject) => f.metaGates.push({ args, resolve, reject }));
          return { columns: [{ name: 'id' }, { name: 'note' }] };
        },
        run_import_task: ({ opts }) => { f.imports.push(opts); return 10; },
        transfer_task_progress: ({ taskId }) => ({ task_id: taskId, status: 'completed', processed: 10, total: 10, message: 'fixture' }),
      };
      window.connection = { id: 'fixture', name: 'SQLite 验收', kind: 'sqlite', file: 'fixture.db', read_only: false };
    });
    const open = async (tableMode = '', csvMode = '', table = 't_comment') => page.evaluate(({ tableMode, csvMode, table }) => {
      window.dialog?.close();
      Object.assign(window.f, { tableMode, csvMode, tableGates: [], csvGates: [], csvCalls: [], imports: [], metaMode: '', metaGates: [], metaCalls: [] });
      window.dialog = ImportFixture.openImportWizard(window.connection, 'main', table, () => {});
    }, { tableMode, csvMode, table });
    const next = () => page.getByRole('button', { name: '下一步', exact: true }).click();
    const back = () => page.getByRole('button', { name: '上一步', exact: true }).click();
    const file = () => page.locator('.dialog .form-control-line input');
    const table = () => page.getByRole('combobox', { name: '目标表', exact: true });
    const summary = () => page.locator('input[aria-label="已选择的目标表"]');
    const waitTables = () => page.waitForFunction(() => document.querySelector('select[aria-label="目标表"]')?.value === 't_comment');
    const waitPreview = () => page.locator('.import-preview-table').waitFor();
    const enterCheck = async () => { await next(); await next(); await next(); };
    await mkdir('.tmp/import-wizard', { recursive: true });

    await t.test('预览铺满、行号与空字符串、编码与列名开关联动', async () => {
      await open(); await waitTables();
      await page.getByText('选择 CSV 文件后，在此预览前 10 行数据。', { exact: true }).waitFor();
      await file().fill('C:/fixture/navkit_accept_t_comment.csv'); await file().press('Tab'); await waitPreview();
      assert.equal(await page.locator('.import-preview-table tbody tr').count(), 10);
      assert.equal(await page.locator('.import-preview-table tbody tr').nth(1).locator('td').nth(1).textContent(), String.fromCharCode(92) + 'N');
      assert.equal(await page.locator('.import-preview-table .is-empty').textContent(), '空字符串');
      const layout = await page.locator('.import-preview').evaluate(el => ({ width: el.clientWidth, parentWidth: el.parentElement.clientWidth, height: el.clientHeight }));
      assert.ok(layout.width >= layout.parentWidth - 2, JSON.stringify(layout));
      assert.ok(layout.height >= 180, JSON.stringify(layout));
      await page.locator('.dialog').screenshot({ path: '.tmp/import-wizard/preview.png' });
      await page.locator('.form-grid select').last().selectOption('gbk');
      await page.waitForFunction(() => f.csvCalls.at(-1)?.encoding === 'gbk');
      await page.getByRole('checkbox', { name: '第一行包含列名' }).uncheck();
      await page.waitForFunction(() => document.querySelector('.import-preview-table thead th:nth-child(2)')?.textContent === 'column1');
      assert.equal(await page.locator('.import-preview-table tbody tr').first().locator('td').first().textContent(), 'id');
      await page.getByRole('checkbox', { name: '第一行包含列名' }).check(); await waitPreview();
    });

    await t.test('多列与长值只在预览内部滚动，窄窗口不撑宽，表头固定', async () => {
      await open('', 'wide'); await file().fill('C:/fixture/wide.csv'); await file().press('Tab'); await waitPreview();
      for (const width of [1200, 760, 480]) {
        await page.setViewportSize({ width, height: 850 });
        const layout = await page.evaluate(() => {
          const body = document.querySelector('.import-preview-body'), pane = document.querySelector('.wizard-panel:not([hidden]) .wizard-panel-inner');
          return { scrolls: body.scrollWidth > body.clientWidth, paneFits: pane.scrollWidth <= pane.clientWidth + 1, tableLayout: getComputedStyle(body.querySelector('table')).tableLayout, sticky: getComputedStyle(body.querySelector('thead th')).position };
        });
        assert.deepEqual(layout, { scrolls: true, paneFits: true, tableLayout: 'fixed', sticky: 'sticky' });
      }
      await page.setViewportSize({ width: 1200, height: 850 });
    });

    await t.test('空文件、只有列名、错误与非 CSV 都有明确状态，刷新可恢复', async () => {
      await open('', 'empty'); await file().fill('C:/fixture/empty.csv'); await file().press('Tab');
      await page.getByText('文件为空，没有可预览的数据。', { exact: true }).waitFor();
      await page.evaluate(() => f.csvMode = 'header-only'); await page.getByRole('button', { name: '刷新', exact: true }).click();
      await page.getByText('只有列名，没有数据行。', { exact: true }).waitFor();
      await page.evaluate(() => f.csvMode = 'error'); await page.getByRole('button', { name: '刷新', exact: true }).click();
      await page.locator('.import-preview-message.is-error').waitFor();
      assert.match(await page.locator('.import-preview-message').textContent(), /fixture CSV unreadable/);
      await page.evaluate(() => f.csvMode = ''); await page.getByRole('button', { name: '刷新', exact: true }).click(); await waitPreview();
      await page.locator('.form-grid select').first().selectOption('json');
      await page.getByText('JSON 和 SQL 文件不提供表格预览，可继续下一步。', { exact: true }).waitFor();
      assert.equal(await page.locator('.import-preview-table').count(), 0);
    });

    await t.test('迟到 CSV 响应不能覆盖最新文件或非 CSV 状态', async () => {
      await open('', 'wait'); await file().fill('C:/fixture/old.csv'); await file().press('Tab');
      await page.waitForFunction(() => f.csvGates.length === 1);
      await file().fill('C:/fixture/new.csv'); await file().press('Tab');
      await page.waitForFunction(() => f.csvGates.length === 2);
      await page.evaluate(() => f.csvGates[1].resolve([['new'], [['latest']]])); await waitPreview();
      await page.evaluate(() => f.csvGates[0].resolve([['old'], [['stale']]]));
      assert.equal(await page.locator('.import-preview-table thead th').nth(1).textContent(), 'new');
      await page.getByRole('button', { name: '刷新', exact: true }).click();
      await page.waitForFunction(() => f.csvGates.length === 3);
      await page.locator('.form-grid select').first().selectOption('sql');
      await page.evaluate(() => f.csvGates[2].resolve([['old'], [['stale']]]));
      assert.equal(await page.locator('.import-preview-table').count(), 0);
      assert.equal(await page.locator('.import-preview').getAttribute('aria-busy'), 'false');
    });

    await t.test('异步默认目标不需要手动 change，检查页立即回显；手动切换与新表名都同步', async () => {
      await open(); await waitTables();
      // 即使检查页尚未进入，异步设置默认表也必须同步只读值。
      assert.equal(await summary().inputValue(), 't_comment');
      await file().fill('C:/fixture/comments.csv'); await file().press('Tab'); await enterCheck();
      assert.equal(await summary().inputValue(), 't_comment');
      await page.locator('.dialog').screenshot({ path: '.tmp/import-wizard/check.png' });
      await back(); await back(); await table().selectOption('alternate'); await next(); await next();
      assert.equal(await summary().inputValue(), 'alternate');
      await back(); await back(); await page.getByRole('checkbox', { name: '目标表不存在时创建' }).check();
      const name = page.getByPlaceholder('新表名', { exact: true }); await name.fill('  new_comments  ');
      assert.equal(await summary().inputValue(), 'new_comments');
      await next(); await next(); assert.equal(await summary().inputValue(), 'new_comments');
      await back(); await back(); await page.getByRole('checkbox', { name: '目标表不存在时创建' }).uncheck();
      assert.equal(await summary().inputValue(), 'alternate');
    });

    await t.test('数据库级入口自动选择第一张真实表，不假装回显不存在的表', async () => {
      await open('', '', ''); await waitTables(); assert.equal(await summary().inputValue(), 't_comment');
      await open('empty');
      await page.waitForFunction(() => document.querySelector('select[aria-label="目标表"]')?.textContent.includes('无可用表'));
      assert.equal(await summary().inputValue(), '');
      await next(); await next(); await page.getByText('请选择目标表或勾选新建并填写表名', { exact: true }).waitFor();
      await open('error'); await next(); await page.getByText(/表元数据读取失败/).waitFor(); assert.equal(await summary().inputValue(), '');
    });

    await t.test('元数据未完成时不进入映射；跨模式迟到响应不串表，失败时清空旧目标', async () => {
      await open('wait'); await page.waitForFunction(() => f.tableGates.length === 1);
      await next(); await next(); await page.getByText('目标表仍在读取，请稍后再试', { exact: true }).waitFor();
      await page.evaluate(() => f.tableGates[0].resolve([['t_comment', 'table']])); await waitTables();
      const schema = page.locator('select[aria-label^="目标"]').first();
      await schema.selectOption('other'); await page.waitForFunction(() => f.tableGates.length === 2);
      assert.equal(await summary().inputValue(), '');
      await schema.selectOption('third'); await page.waitForFunction(() => f.tableGates.length === 3);
      await page.evaluate(() => f.tableGates[2].resolve([['third_table', 'table']]));
      await page.waitForFunction(() => document.querySelector('select[aria-label="目标表"]')?.value === 'third_table');
      await page.evaluate(() => f.tableGates[1].resolve([['stale_other_table', 'table']]));
      assert.equal(await summary().inputValue(), 'third_table');
      await schema.selectOption('other'); await page.waitForFunction(() => f.tableGates.length === 4);
      await page.evaluate(() => f.tableGates[3].reject(Error('fixture failed')));
      await page.getByText(/表元数据读取失败/).waitFor(); assert.equal(await summary().inputValue(), '');
    });

    await t.test('检查页目标与实际导入 payload 一致（仅隔离 fixture 写入）', async () => {
      await open(); await waitTables();
      await file().fill('C:/fixture/comments.json'); await file().press('Tab');
      await page.locator('.form-grid select').first().selectOption('json');
      await next(); await table().selectOption('alternate'); await next(); await next();
      const displayed = await summary().inputValue();
      await next(); await page.waitForFunction(() => f.imports.length === 1);
      assert.deepEqual(await page.evaluate(() => ({ table: f.imports[0].table, schema: f.imports[0].schema, file: f.imports[0].file })), { table: displayed, schema: 'main', file: 'C:/fixture/comments.json' });
    });

    await t.test('旧 CSV 映射不能覆盖新文件映射或进入实际导入 payload', async () => {
      await open('', 'wait'); await waitTables();
      await file().fill('C:/fixture/old.csv'); await file().press('Tab');
      await page.waitForFunction(() => f.csvGates.length === 1);
      await page.evaluate(() => f.csvGates[0].resolve([['id'], [['1']]])); await waitPreview();
      await next(); await next(); await page.waitForFunction(() => f.csvGates.length === 2);
      await back(); await back();
      await file().fill('C:/fixture/new.csv'); await file().press('Tab');
      await page.waitForFunction(() => f.csvGates.length === 3);
      await page.evaluate(() => f.csvGates[2].resolve([['note', 'id'], [['new', '2']]])); await waitPreview();
      await next(); await next(); await page.waitForFunction(() => f.csvGates.length === 4);
      await page.evaluate(() => f.csvGates[3].resolve([['note', 'id'], [['new', '2']]]));
      await page.getByLabel('note 对应目标列', { exact: true }).waitFor();
      await page.evaluate(() => f.csvGates[1].resolve([['old_column'], [['old']]]));
      assert.equal(await page.getByLabel('old_column 对应目标列', { exact: true }).count(), 0);
      assert.equal(await page.evaluate(() => f.metaCalls.length), 1, '旧 CSV 回执不应再启动目标元数据读取');
      await next(); await next(); await page.waitForFunction(() => f.imports.length === 1);
      assert.deepEqual(await page.evaluate(() => ({ file: f.imports[0].file, mapping: f.imports[0].column_mapping })), { file: 'C:/fixture/new.csv', mapping: [{ source: 'note', target: 'note' }, { source: 'id', target: 'id' }] });
    });

    await t.test('换目标后的旧结构回执不覆盖新映射；旧 finally 不提前解锁新请求', async () => {
      await open(); await waitTables();
      await file().fill('C:/fixture/comments.csv'); await file().press('Tab'); await waitPreview();
      await page.evaluate(() => f.metaMode = 'wait');
      await next(); await next(); await page.waitForFunction(() => f.metaGates.length === 1);
      await back(); await table().selectOption('alternate'); await next();
      await page.waitForFunction(() => f.metaGates.length === 2);
      await page.evaluate(() => f.metaGates[0].resolve({ columns: [{ name: 'stale_target' }] }));
      await next(); await next();
      assert.equal(await page.evaluate(() => f.imports.length), 0);
      await page.getByText('列映射仍在读取，不能导入', { exact: true }).waitFor();
      await page.evaluate(() => f.metaGates[1].resolve({ columns: [{ name: 'fresh_target' }] }));
      await page.waitForFunction(() => [...document.querySelectorAll('select[aria-label$="对应目标列"] option')].some(option => option.value === 'fresh_target'));
      assert.equal(await page.locator('select[aria-label$="对应目标列"] option[value="stale_target"]').count(), 0);
      assert.deepEqual(await page.evaluate(() => f.metaCalls.map(call => call.table)), ['t_comment', 'alternate']);
    });

    await t.test('关闭后迟到映射不再读取目标或写回已关闭对话框', async () => {
      await open('', 'wait'); await waitTables();
      await file().fill('C:/fixture/closed.csv'); await file().press('Tab');
      await page.waitForFunction(() => f.csvGates.length === 1);
      await page.evaluate(() => f.csvGates[0].resolve([['id'], [['1']]])); await waitPreview();
      await next(); await next(); await page.waitForFunction(() => f.csvGates.length === 2);
      await page.evaluate(() => { dialog.close(); f.csvGates[1].resolve([['late'], [['1']]]); });
      assert.equal(await page.evaluate(() => f.metaCalls.length), 0);
      assert.equal(await page.evaluate(() => dialog.body.querySelectorAll('select[aria-label$="对应目标列"]').length), 0);
    });

    await page.evaluate(() => dialog.close());
    assert.deepEqual(errors, []);
  } finally { await browser.close(); }
});
