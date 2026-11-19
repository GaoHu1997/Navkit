// Backup wizard acceptance with delayed fixture IPC; never touches a live database.
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';

const root = fileURLToPath(new URL('../../', import.meta.url));
const bundle = await build({
  stdin: { contents: 'export { openBackupDialog } from "./src/dialogs/BackupDialog.ts"; export { taskCenter } from "./src/core/backgroundTasks.ts";', resolveDir: root, loader: 'ts' },
  bundle: true, write: false, platform: 'browser', format: 'iife', globalName: 'backupFixture', loader: { '.css': 'empty' }, logLevel: 'silent',
});
const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1024, height: 800 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.setContent('<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"></head><body><button id="trigger">打开备份</button></body></html>');
  await page.addStyleTag({ path: path.join(root, 'src/styles/base.css') });
  await page.addStyleTag({ path: path.join(root, 'src/styles/backup-dialog.css') });
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  await page.evaluate(() => {
    window.requests = [];
    window.completed = [];
    window.__PREVIEW_MOCK__ = {
      list_tables: () => [['demo', 'table']], list_routines: () => [],
      create_backup: args => new Promise((resolve, reject) => requests.push({ args, resolve, reject })),
    };
    window.openFixture = (failCallback = false) => {
      document.querySelector('#trigger').focus();
      window.fixtureDialog = backupFixture.openBackupDialog({ id: 'c1', name: '本地 MySQL', kind: 'mysql', database: 'demo' }, 'demo', (rows, bytes) => {
        completed.push([rows, bytes]); if (failCallback) throw new Error('list-refresh fixture');
      });
    };
    openFixture();
  });
  const dialog = page.locator('.backup-dialog');
  const next = dialog.locator('.wizard-actions button').nth(1);
  const back = dialog.getByRole('button', { name: '上一步', exact: true });
  const close = dialog.locator('.dialog-footer > button');
  const toExecution = async () => { await next.click(); await next.click(); await next.getByText('开始备份').waitFor(); };
  const waitIdle = () => page.waitForFunction(() => document.querySelector('.wizard-body').getAttribute('aria-busy') === 'false');
  await toExecution();
  const fieldsBefore = await dialog.locator('.backup-execution input').evaluateAll(inputs => inputs.map(input => input.value));
  await next.click();
  assert.equal(await next.isDisabled(), true);
  assert.equal(await back.isDisabled(), true);
  await next.dispatchEvent('click');
  assert.equal(await page.evaluate(() => requests.length), 1, 'running submit must be idempotent');
  const savedPath = 'D:\\真实备份目录\\demo_20261113_1411.sql';
  await page.evaluate(saved => requests[0].resolve({ rows: 3, bytes: 1434, path: saved, warnings: [] }), savedPath);
  await waitIdle();
  assert.equal(await next.textContent(), '备份已完成');
  assert.equal(await next.isDisabled(), true);
  assert.equal(await back.isDisabled(), false, 'success must allow going back');
  assert.equal(await close.evaluate(button => button.classList.contains('primary')), true);
  assert.equal(await next.evaluate(button => button.classList.contains('primary')), false);
  const status = dialog.getByRole('status');
  assert.equal(await status.getAttribute('aria-live'), 'polite');
  assert.match(await status.textContent(), /备份成功.*已备份 3 行数据，文件大小 1\.4 KB/);
  assert.equal(await dialog.locator('.backup-success-location').textContent(), `存储位置：${savedPath}`);
  assert.equal(await status.locator('input, button').count(), 0, 'path must be plain text without copy action');
  assert.deepEqual(await dialog.locator('.backup-execution input').evaluateAll(inputs => inputs.map(input => input.value)), fieldsBefore);
  assert.equal(await page.evaluate(() => document.activeElement.textContent), '关闭', 'focus transfers away from disabled submit');
  assert.equal(await page.evaluate(() => backupFixture.taskCenter.snapshot().tasks[0].status), 'success');
  await next.dispatchEvent('click');
  assert.equal(await page.evaluate(() => requests.length), 1, 'success must not re-submit');
  const screenshot = path.join(root, 'artifacts/backup-dialog-success.png');
  await mkdir(path.dirname(screenshot), { recursive: true });
  await dialog.screenshot({ path: screenshot });

  await back.click();
  assert.equal(await dialog.locator('.backup-status.is-success').count(), 0);
  assert.equal(await close.evaluate(button => button.classList.contains('primary')), false);
  const options = dialog.locator('.wizard-panel:not([hidden])');
  await options.locator('input[type="checkbox"]').nth(1).uncheck();
  await options.locator('input[type="text"]').nth(1).fill('changed');
  await next.click();
  assert.equal(await next.textContent(), '开始备份');
  assert.equal(await next.isDisabled(), false);
  await next.click();
  await page.waitForFunction(() => requests.length === 2);
  assert.equal(await page.evaluate(() => requests[1].args.opts.include_data), false);
  assert.equal(await page.evaluate(() => requests[1].args.opts.file_name), 'changed');
  await page.evaluate(() => requests[1].resolve({ rows: 0, bytes: 900, path: 'D:/backups/changed.sql', warnings: [] }));
  await waitIdle();
  // Step-header navigation also clears completion, not just the back button.
  await dialog.locator('.wizard-step').first().click();
  assert.equal(await next.textContent(), '下一步');
  assert.equal(await dialog.locator('.backup-status.is-success').count(), 0);
  await toExecution();
  await next.click();
  await page.evaluate(() => requests[2].reject(new Error('disk-full fixture')));
  await waitIdle();
  assert.equal(await next.isDisabled(), false, 'failed backup remains retryable');
  assert.match(await status.textContent(), /备份失败.*disk-full fixture/);
  await next.click();
  const longPath = 'D:/长目录/' + '实际文件夹/'.repeat(20) + '<special>&.sql';
  await page.evaluate(saved => requests[3].resolve({ rows: 9, bytes: 1024, path: saved, warnings: [] }), longPath);
  await waitIdle();
  assert.equal(await dialog.locator('.backup-success-location').textContent(), `存储位置：${longPath}`);
  const bounds = await dialog.locator('.backup-success-location').evaluate(el => ({ width: el.clientWidth, scroll: el.scrollWidth }));
  assert.ok(bounds.scroll <= bounds.width + 1, 'long path must wrap, not be truncated');
  // 成功块改为「左图标 + 右文案、下方路径」，不再撑出滚动条，也不再显示关闭提示
  const overflow = await dialog.locator('.wizard-body').evaluate(el => ({ scroll: el.scrollHeight, client: el.clientHeight }));
  assert.ok(overflow.scroll <= overflow.client + 1, `success panel must not add a scrollbar: ${JSON.stringify(overflow)}`);
  assert.equal(await dialog.locator('.backup-success-hint').count(), 0);
  assert.doesNotMatch(await dialog.innerText(), /可关闭此窗口/);
  const successLayout = await dialog.locator('.backup-success-main').evaluate(el => {
    const icon = el.querySelector('.backup-success-icon').getBoundingClientRect();
    const title = el.querySelector('.backup-success-title').getBoundingClientRect();
    const location = el.parentElement.querySelector('.backup-success-location').getBoundingClientRect();
    return { iconLeft: icon.left, iconRight: icon.right, titleLeft: title.left, titleBottom: title.bottom, locationTop: location.top, locationLeft: location.left };
  });
  assert.ok(successLayout.titleLeft > successLayout.iconRight, 'icon stays left of the text');
  assert.ok(successLayout.locationTop > successLayout.titleBottom, 'storage path sits below the icon and text');
  await close.click();
  assert.equal(await page.locator('#trigger').evaluate(button => button === document.activeElement), true);
  await page.evaluate(() => openFixture(true));
  await toExecution(); await next.click();
  await page.evaluate(() => requests[4].resolve({ rows: 1, bytes: 42, path: 'D:/backups/callback.sql', warnings: [] }));
  await waitIdle();
  assert.equal(await next.isDisabled(), true, 'callback error cannot unlock a published backup');
  assert.equal(await dialog.locator('.backup-success-title').textContent(), '备份成功');
  await close.click();
  await page.evaluate(() => openFixture());
  await toExecution(); await next.click(); await close.click();
  await page.evaluate(() => requests[5].resolve({ rows: 1, bytes: 12, path: 'D:/backups/background.sql', warnings: [] }));
  await page.waitForFunction(() => completed.length === 5);
  assert.equal(await page.locator('.backup-dialog').count(), 0, 'close does not resurrect background task dialog');
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ status: 'passed', scenarios: ['success panel and real path', 'running and completed deduplication', 'back and step-header reset', 'changed options re-submit', 'failure retry', 'long path plain-text wrapping', 'focus and callback failure', 'close during background backup'], screenshot }, null, 2));
} finally { await browser.close(); }
