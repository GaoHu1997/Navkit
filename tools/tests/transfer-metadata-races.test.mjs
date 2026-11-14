// Transfer dialogs metadata gates: real browser, memory bundle and isolated IPC only.
import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';
const compiled = await build({ stdin: { contents: 'export { openBackupDialog } from "./src/dialogs/BackupDialog.ts"; export { openSyncDialog } from "./src/dialogs/SyncDialog.ts";', resolveDir: process.cwd(), loader: 'ts' }, bundle: true, write: false, platform: 'browser', format: 'iife', globalName: 'TransferMetadataFixture', loader: { '.css': 'empty' }, logLevel: 'silent' });

test('备份和整库同步：目录等待/失败/旧源回执不得扩大或串库执行', async t => {
  const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  try {
    const withPage = async run => {
      const page = await browser.newPage(); const errors = []; page.on('pageerror', error => errors.push(error.message));
      try {
        await page.route('https://transfer-metadata.test/**', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><html lang="zh-CN"><body></body></html>' }));
        await page.goto('https://transfer-metadata.test/'); await page.addScriptTag({ content: compiled.outputFiles[0].text });
        await page.evaluate(() => {
          window.f = { backupCalls: [], syncCalls: [], tableCalls: [], gates: [], tableMode: '' };
          window.connections = ['a', 'b'].map(id => ({ id, name: id, kind: 'sqlite', params: '', file: 'unused.db', database: 'main', read_only: false }));
          window.__PREVIEW_MOCK__ = {
            ensure_connection: () => ({ ok: true }), list_databases: () => [['main', ''], ['other', '']], list_routines: () => [],
            list_tables: args => {
              f.tableCalls.push(args);
              if (f.tableMode === 'wait') return new Promise((resolve, reject) => f.gates.push({ args, resolve, reject }));
              if (f.tableMode === 'error') throw Error('isolated metadata failed');
              return f.tableMode === 'empty' ? [] : [['items', 'table']];
            },
            sync_type_mapping: () => [{ source: 'id', source_type: 'INTEGER', target_type: 'INTEGER' }],
            get_table_schema: () => ({ columns: [{ name: 'id' }], foreign_keys: [], indexes: [] }),
            preview_diff_memory: () => ({ effective_limit_bytes: 1024, requested_limit_bytes: 1024, source: 'fixture', mode: 'auto' }),
            create_backup: ({ opts }) => { f.backupCalls.push(opts); return { rows: 1, bytes: 1, path: 'fixture.sql', warnings: [] }; },
            run_sync_database: ({ req }) => { f.syncCalls.push(req); return []; },
          };
        });
        await run(page); assert.deepEqual(errors, []);
      } finally { await page.close(); }
    };
    await t.test('备份等待与失败阻断前进，重试成功后提交已核对的表而不是空数组', () => withPage(async page => {
      await page.evaluate(() => { f.tableMode = 'wait'; window.dialog = TransferMetadataFixture.openBackupDialog(connections[0], 'main', () => {}); });
      await page.waitForFunction(() => f.gates.length === 1);
      await page.getByRole('button', { name: '下一步', exact: true }).click();
      await page.getByText('对象列表仍在加载，请等待完成后核对备份范围。', { exact: true }).waitFor();
      assert.equal(await page.evaluate(() => f.backupCalls.length), 0);
      await page.evaluate(() => f.gates[0].reject(Error('isolated metadata failed')));
      await page.getByText(/对象列表加载失败：isolated metadata failed/).first().waitFor();
      await page.getByRole('button', { name: '下一步', exact: true }).click();
      assert.equal(await page.getByRole('button', { name: '开始备份', exact: true }).count(), 0);
      await page.evaluate(() => f.tableMode = ''); await page.getByRole('button', { name: '重试加载对象', exact: true }).click();
      await page.waitForFunction(() => !!document.querySelector('.backup-tree-leaf'));
      await page.getByRole('button', { name: '下一步', exact: true }).click();
      await page.getByRole('button', { name: '下一步', exact: true }).click();
      await page.getByRole('button', { name: '开始备份', exact: true }).click();
      await page.waitForFunction(() => f.backupCalls.length === 1);
      assert.deepEqual(await page.evaluate(() => f.backupCalls[0].tables), ['items']);
    }));
    await t.test('真实空库明确停止，不提交空选择作为全部表；关闭后目录回执不回填', () => withPage(async page => {
      await page.evaluate(() => { f.tableMode = 'empty'; window.dialog = TransferMetadataFixture.openBackupDialog(connections[0], 'main', () => {}); });
      await page.getByText('（该数据库没有可备份的表、视图、函数或存储过程）', { exact: true }).waitFor();
      await page.getByRole('button', { name: '下一步', exact: true }).click();
      await page.getByText('当前数据库没有可备份对象，不能创建空备份。', { exact: true }).waitFor();
      assert.equal(await page.evaluate(() => f.backupCalls.length), 0);
      await page.evaluate(() => { dialog.close(); f.tableMode = 'wait'; window.dialog = TransferMetadataFixture.openBackupDialog(connections[0], 'main', () => {}); });
      await page.waitForFunction(() => f.gates.length === 1);
      await page.evaluate(() => { dialog.close(); f.gates[0].resolve([['late', 'table']]); });
      assert.equal(await page.evaluate(() => dialog.body.querySelectorAll('.backup-tree-leaf').length), 0);
    }));
    await t.test('整库换源清单失败不借用旧源勾选；重试后只发送新源清单', () => withPage(async page => {
      await page.evaluate(() => { window.dialog = TransferMetadataFixture.openSyncDialog(connections, connections[0], { schema: 'main', table: 'items' }); });
      await page.waitForFunction(() => [...document.querySelectorAll('.sync-side select')].filter(el => el.options.length).length >= 4);
      await page.getByLabel('同步方式', { exact: true }).first().selectOption('database');
      const next = () => page.getByRole('button', { name: '下一步', exact: true }).click();
      const back = () => page.getByRole('button', { name: '上一步', exact: true }).click();
      await next(); await next(); await page.getByLabel('同步表 items', { exact: true }).waitFor();
      await back(); await back();
      await page.locator('.sync-side').first().locator('select').nth(1).selectOption('other');
      await page.evaluate(() => f.tableMode = 'error');
      await next(); await next();
      await page.getByRole('button', { name: '开始同步', exact: true }).click();
      await page.getByText(/源库表清单加载失败，不能同步/).waitFor();
      assert.equal(await page.evaluate(() => f.syncCalls.length), 0);
      assert.equal(await page.getByLabel('同步表 items', { exact: true }).count(), 0);
      await page.evaluate(() => f.tableMode = 'wait');
      await back(); await next(); await page.waitForFunction(() => f.gates.length >= 1);
      await page.evaluate(() => f.gates.at(-1).resolve([['fresh_items', 'table']]));
      await page.getByLabel('同步表 fresh_items', { exact: true }).waitFor();
      await page.getByRole('button', { name: '开始同步', exact: true }).click();
      await page.waitForFunction(() => f.syncCalls.length === 1);
      assert.deepEqual(await page.evaluate(() => ({ schema: f.syncCalls[0].source_schema, names: f.syncCalls[0].tables.map(table => table.source) })), { schema: 'other', names: ['fresh_items'] });
    }));
  } finally { await browser.close(); }
});
