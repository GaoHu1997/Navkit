// 独立内存 bundle + 显式 IPC 替身；不读写用户数据库，不构建产品产物。
import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';
const built = await build({ stdin: { contents: 'export { openDataGeneratorDialog } from "./src/dialogs/DataGeneratorDialog.ts";', resolveDir: process.cwd(), loader: 'ts' }, bundle: true, write: false, platform: 'browser', format: 'iife', globalName: 'GeneratorMetadataFixture', loader: { '.css': 'empty' }, logLevel: 'silent' });

test('数据生成元数据与预览版本独立：修改条数/种子不丢连接和结构回执', async t => {
  const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  try {
    for (const stage of ['ensureConnection', 'listDatabases', 'listTables', 'getTableSchema']) {
      await t.test(stage, async () => {
        const page = await browser.newPage();
        const errors = []; page.on('pageerror', error => errors.push(error.message));
        try {
          await page.route('https://generator-metadata.test/**', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><html lang="zh-CN"><body></body></html>' }));
          await page.goto('https://generator-metadata.test/');
          await page.addScriptTag({ content: built.outputFiles[0].text });
          await page.evaluate(stage => {
            window.f = { calls: [], resolve: null };
            const column = { name: 'id', type: 'INTEGER', nullable: false, primary_key: true, auto_increment: false, default: null, length: null, precision: null, scale: null };
            const replies = { ensureConnection: { ok: true }, listDatabases: [['main', '']], listTables: [['items', 'table']], getTableSchema: { columns: [column], foreign_keys: [], indexes: [] } };
            const source = Object.fromEntries(Object.keys(replies).map(name => [name, (...args) => {
              f.calls.push([name, ...args]);
              return name === stage ? new Promise(resolve => { f.resolve = () => resolve(replies[name]); }) : Promise.resolve(replies[name]);
            }]));
            const connection = { id: 'isolated', name: 'fixture', kind: 'sqlite', params: '', file: 'unused-fixture.db', read_only: false };
            window.dialog = GeneratorMetadataFixture.openDataGeneratorDialog(connection, 'main', 'items', undefined, source);
          }, stage);
          await page.waitForFunction(() => !!f.resolve);
          await page.getByLabel('生成条数', { exact: true }).fill('17');
          await page.getByLabel('文本种子', { exact: true }).fill('changed-during-metadata');
          await page.evaluate(() => f.resolve());
          await page.waitForFunction(() => document.querySelector('[aria-label="生成预览"]')?.disabled === false || [...document.querySelectorAll('button')].some(button => button.textContent === '生成预览' && !button.disabled));
          await page.getByRole('button', { name: '下一步', exact: true }).click();
          await page.getByRole('button', { name: '下一步', exact: true }).click();
          await page.getByRole('button', { name: '生成预览', exact: true }).click();
          assert.match(await page.locator('[id^="generator-status-"]').textContent(), /预览已生成 17 行/);
          assert.deepEqual(await page.evaluate(() => f.calls.map(call => call[0])), ['ensureConnection', 'listDatabases', 'listTables', 'getTableSchema']);
          assert.deepEqual(errors, []);
        } finally { await page.close(); }
      });
    }
    await t.test('关闭后迟到连接响应不继续加载数据库', async () => {
      const page = await browser.newPage();
      try {
        await page.route('https://generator-metadata.test/**', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><html lang="zh-CN"><body></body></html>' }));
        await page.goto('https://generator-metadata.test/');
        await page.addScriptTag({ content: built.outputFiles[0].text });
        await page.evaluate(() => {
          window.calls = []; const source = { ensureConnection: () => new Promise(resolve => { window.resolveConnection = () => resolve({ ok: true }); }), listDatabases: () => { calls.push('databases'); return Promise.resolve([['main', '']]); } };
          window.dialog = GeneratorMetadataFixture.openDataGeneratorDialog({ id: 'isolated', name: 'fixture', kind: 'sqlite', params: '', read_only: false }, 'main', '', undefined, source);
          dialog.close(); resolveConnection();
        });
        assert.deepEqual(await page.evaluate(() => calls), []);
      } finally { await page.close(); }
    });
  } finally { await browser.close(); }
});
