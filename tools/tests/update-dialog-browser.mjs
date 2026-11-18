// STORY-UPD-001：实际 UpdateDialog + 实际 CSS 的浏览器验收，隔离模拟 IPC，不启动服务器。
import assert from 'node:assert/strict';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import { build } from 'esbuild';

const out = fileURLToPath(new URL('../../artifacts/update-dialog-reference/', import.meta.url));
await mkdir(out, { recursive: true });
const built = await build({
  stdin: { contents: 'import { openUpdateDialog } from "./src/dialogs/UpdateDialog.ts"; window.openUpdateTestDialog = openUpdateDialog;', resolveDir: process.cwd(), loader: 'ts' },
  bundle: true, write: false, format: 'esm', loader: { '.css': 'empty' }, logLevel: 'silent',
});
const baseCss = await readFile(new URL('../../src/styles/base.css', import.meta.url), 'utf8');
const updateCss = await readFile(new URL('../../src/styles/update-dialog.css', import.meta.url), 'utf8');
const report = { mode: 'actual-component-and-css-with-isolated-mock-ipc', status: 'running', checks: [], images: [], pageErrors: [], sourceHashes: {} };
for (const file of ['src/dialogs/UpdateDialog.ts', 'src/styles/update-dialog.css', 'src/core/updatePreferences.ts']) {
  report.sourceHashes[file] = createHash('sha256').update(await readFile(file)).digest('hex');
}
const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
const page = await browser.newPage({ viewport: { width: 960, height: 600 }, deviceScaleFactor: 1 });
page.on('pageerror', error => report.pageErrors.push(error.message));
await page.route('https://navkit.test/**', async route => {
  const path = new URL(route.request().url()).pathname;
  if (path === '/bundle.js') return route.fulfill({ contentType: 'text/javascript', body: built.outputFiles[0].text });
  if (path === '/styles.css') return route.fulfill({ contentType: 'text/css', body: baseCss + '\n' + updateCss });
  return route.fulfill({ contentType: 'text/html', body: '<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><link rel="stylesheet" href="/styles.css"></head><body><button id="trigger">检查更新</button><script type="module" src="/bundle.js"></script></body></html>' });
});
await page.addInitScript(() => {
  window.updateCalls = [];
  window.fixtureManifest = { version: '3.18.2', pub_date: '2026-11-12T08:00:00Z', notes: '- 第一项改进\n- 第二项修复', url: 'https://example.test/update.exe', size: 100, sha256: 'a'.repeat(64) };
  window.fixturePhase = 'idle';
  window.fixtureLatest = false;
  window.fixtureError = false;
  window.__PREVIEW_MOCK__ = {
    about_info: () => ({ version: '1.0.2' }),
    update_check: () => {
      window.updateCalls.push('check');
      if (window.fixtureError) throw new Error('隔离测试：网络失败');
      return { current_version: '1.0.2', update_available: !window.fixtureLatest, manifest: window.fixtureManifest };
    },
    update_status: () => ({ phase: window.fixturePhase, done: 40, total: 100, speed: 10, staged_file: window.fixturePhase === 'ready' ? 'D:/test-staged.exe' : null }),
    update_download: () => { window.updateCalls.push('download'); window.fixturePhase = 'downloading'; return new Promise(resolve => { window.completeUpdateDownload = () => { window.fixturePhase = 'ready'; resolve('D:/test-staged.exe'); }; }); },
    update_apply: () => { window.updateCalls.push('apply'); },
  };
});
const dialog = () => page.locator('.dialog.update-dialog');
const open = async () => {
  await page.locator('#trigger').focus();
  await page.evaluate(() => window.testDialog = window.openUpdateTestDialog());
  await dialog().waitFor();
  await page.waitForFunction(() => document.querySelector('.update-dialog')?.dataset.state !== 'checking');
};
const close = async () => { await page.keyboard.press('Escape'); assert.equal(await dialog().count(), 0); };
const capture = async name => {
  const file = `${out}/${name}.png`;
  await dialog().screenshot({ path: file });
  report.images.push({ name, file });
};
try {
  await page.goto('https://navkit.test/', { waitUntil: 'networkidle' });
  await page.waitForFunction(() => typeof window.openUpdateTestDialog === 'function');
  await open();
  const metrics = await dialog().evaluate(element => {
    const rect = element.getBoundingClientRect();
    return { width: rect.width, height: rect.height, background: getComputedStyle(element).backgroundColor,
      primary: getComputedStyle(element.querySelector('.btn.primary')).backgroundColor,
      footer: element.querySelector('.dialog-footer').getBoundingClientRect().height,
      notesHidden: element.querySelector('.ud-notes').hidden,
      overflow: element.scrollWidth - element.clientWidth };
  });
  report.metrics = metrics;
  assert.equal(metrics.width, 640); assert.equal(metrics.height, 257);
  assert.equal(metrics.background, 'rgb(255, 255, 255)'); assert.equal(metrics.primary, 'rgb(20, 23, 28)');
  assert.equal(metrics.notesHidden, true); assert.ok(metrics.overflow <= 1);
  assert.equal(await dialog().getByRole('checkbox').isChecked(), false);
  await dialog().locator('.ud-hero').click();
  await capture('available'); report.checks.push('640×257 白底紧凑布局、日期和黑色主按钮');

  await dialog().getByRole('button', { name: /查看更新内容/ }).click();
  await dialog().locator('.ud-notes').waitFor({ state: 'visible' });
  assert.equal(await dialog().locator('.ud-notes-list li').count(), 2);
  await dialog().getByRole('button', { name: /查看更新内容/ }).click();
  await dialog().getByRole('checkbox').check();
  assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem('navkit.updateprefs.v1')).autoUpdate), true);
  assert.equal(await page.evaluate(() => window.updateCalls.includes('apply')), false);
  await dialog().getByRole('button', { name: '最小化', exact: true }).click();
  assert.equal(await dialog().locator('.dialog-body').isVisible(), false);
  await dialog().getByRole('button', { name: '还原', exact: true }).click();
  assert.equal(await dialog().locator('.dialog-body').isVisible(), true);
  await close();
  assert.ok(await page.locator('#trigger').evaluate(element => document.activeElement === element));
  report.checks.push('说明展开、复选框持久化、最小化还原、Escape 和焦点恢复');

  await open(); assert.equal(await dialog().getByRole('checkbox').isChecked(), true);
  await dialog().getByRole('button', { name: '下载更新', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.update-dialog')?.dataset.state === 'downloading');
  await capture('downloading');
  await page.evaluate(() => window.completeUpdateDownload());
  await page.waitForFunction(() => document.querySelector('.update-dialog')?.dataset.state === 'ready');
  await capture('ready');
  assert.equal(await page.evaluate(() => window.updateCalls.includes('apply')), false);
  await close(); report.checks.push('下载→就绪，自动更新开启也不立即安装');

  await page.evaluate(() => { window.fixturePhase = 'idle'; window.fixtureError = true; });
  await open(); assert.equal(await dialog().getAttribute('data-state'), 'error');
  await capture('error');
  await page.evaluate(() => { window.fixtureError = false; window.fixtureLatest = true; });
  await dialog().getByRole('button', { name: '重试', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.update-dialog')?.dataset.state === 'latest');
  await capture('latest'); await close(); report.checks.push('错误就地展示、重试恢复和最新版');

  await page.evaluate(() => { window.fixtureLatest = false; window.fixtureManifest.version = '3.18.2-' + 'long-version-'.repeat(8); window.fixtureManifest.pub_date = ''; });
  await page.setViewportSize({ width: 375, height: 440 }); await open();
  const narrow = await dialog().evaluate(element => ({ width: element.getBoundingClientRect().width, overflow: element.scrollWidth - element.clientWidth, footerVisible: element.querySelector('.dialog-footer').getBoundingClientRect().bottom <= innerHeight }));
  assert.ok(narrow.width <= 343 && narrow.overflow <= 1 && narrow.footerVisible);
  await capture('narrow');
  await dialog().getByRole('button', { name: '跳过此版本', exact: true }).click();
  assert.equal(await dialog().count(), 0);
  assert.match(await page.evaluate(() => JSON.parse(localStorage.getItem('navkit.updateprefs.v1')).skippedVersion), /^3\.18\.2-long/);
  report.checks.push('窄屏长版本/缺失日期无溢出、跳过实际持久化');
  assert.deepEqual(report.pageErrors, []); report.status = 'passed';
} catch (error) { report.status = 'failed'; report.error = error.stack; throw error; }
finally { await writeFile(`${out}/report.json`, JSON.stringify(report, null, 2)); await browser.close(); }
console.log(JSON.stringify(report, null, 2));
