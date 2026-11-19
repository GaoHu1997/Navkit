// BUG-UPD-BUSY-001：实际徽标/CSS 与隔离 IPC，绝不下载或安装真实程序。
import assert from 'node:assert/strict';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import { build } from 'esbuild';

const out = fileURLToPath(new URL('../../artifacts/update-badge-busy/', import.meta.url));
await mkdir(out, { recursive: true });
const built = await build({
  stdin: { contents: 'import { UpdateBadge } from "./src/ui/UpdateBadge.ts"; window.badge = new UpdateBadge(); document.querySelector(".titlebar-tools").append(window.badge.el); window.badge.setAvailable(window.fixtureManifest);', resolveDir: process.cwd(), loader: 'ts' },
  bundle: true, write: false, format: 'esm', loader: { '.css': 'empty' }, logLevel: 'silent',
});
const css = (await readFile(new URL('../../src/styles/base.css', import.meta.url), 'utf8')) + '\n' +
  (await readFile(new URL('../../src/styles/update-badge.css', import.meta.url), 'utf8'));
const report = { mode: 'actual-component-and-css-with-isolated-mock-ipc', checks: [], pageErrors: [], status: 'running' };
const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
const page = await browser.newPage({ viewport: { width: 480, height: 140 }, deviceScaleFactor: 1 });
page.on('pageerror', error => report.pageErrors.push(error.message));
await page.route('https://navkit.test/**', route => {
  const path = new URL(route.request().url()).pathname;
  if (path === '/bundle.js') return route.fulfill({ contentType: 'text/javascript', body: built.outputFiles[0].text });
  if (path === '/styles.css') return route.fulfill({ contentType: 'text/css', body: css });
  return route.fulfill({ contentType: 'text/html', body: '<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><link rel="stylesheet" href="/styles.css"></head><body><div class="titlebar"><div class="titlebar-tools"></div></div><script type="module" src="/bundle.js"></script></body></html>' });
});
await page.addInitScript(() => {
  window.fixtureManifest = { version: '1.2.0', notes: '', pub_date: '', url: '', sha256: 'a'.repeat(64), size: 100 };
  window.calls = { downloads: 0, applies: 0 };
  window.__PREVIEW_MOCK__ = {
    update_status: () => ({ phase: 'idle' }),
    update_download: () => {
      window.calls.downloads++;
      return new Promise((resolve, reject) => { window.finishDownload = () => resolve('D:/isolated-staged.exe'); window.failDownload = () => reject(new Error('隔离测试：网络失败')); });
    },
    update_apply: () => {
      window.calls.applies++;
      return new Promise((resolve, reject) => { window.finishApply = resolve; window.failApply = () => reject(new Error('隔离测试：文件占用')); });
    },
  };
});
const badge = page.locator('.update-badge');
const phase = expected => page.waitForFunction(expected => window.badge.el.dataset.phase === expected, expected);
const capture = name => page.screenshot({ path: `${out}/${name}.png` });
const metrics = () => badge.evaluate(el => {
  const ub = el.querySelector('.ub'), loading = el.querySelector('.ub-loading'), spinner = el.querySelector('.ub-spinner');
  const rect = ub.getBoundingClientRect(), spin = spinner.getBoundingClientRect();
  return { width: rect.width, height: rect.height, disabled: el.disabled, busy: el.getAttribute('aria-busy'),
    display: getComputedStyle(loading).display, background: getComputedStyle(loading).backgroundColor,
    labelOpacity: getComputedStyle(el.querySelector('.ub-lb')).opacity, animation: getComputedStyle(spinner).animationName,
    spinnerWidth: spin.width, spinnerHeight: spin.height,
    centerX: Math.abs((spin.left + spin.right - rect.left - rect.right) / 2),
    centerY: Math.abs((spin.top + spin.bottom - rect.top - rect.bottom) / 2) };
});
const checkBusy = async expected => {
  await phase(expected);
  const m = await metrics();
  assert.equal(m.width, 54); assert.equal(m.height, 22); assert.equal(m.disabled, true); assert.equal(m.busy, 'true');
  assert.equal(m.display, 'grid'); assert.match(m.background, /rgba\(255, 255, 255, 0\.82\)/); assert.equal(m.labelOpacity, '0');
  assert.ok(m.centerX <= 0.1 && m.centerY <= 0.1); assert.equal(m.animation, 'ub-spin');
  return m;
};
const spamUserInput = async () => {
  const box = await badge.boundingBox();
  for (let i = 0; i < 4; i++) {
    await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
    await page.keyboard.press('Enter'); await page.keyboard.press('Space');
  }
  await page.evaluate(() => {
    for (let i = 0; i < 10; i++) window.badge.el.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  });
};
try {
  await page.goto('https://navkit.test/', { waitUntil: 'networkidle' });
  await phase('available'); assert.equal(await badge.isEnabled(), true); await capture('available');
  await badge.focus(); await page.keyboard.press('Enter');
  report.downloading = await checkBusy('downloading');
  await page.evaluate(() => window.badge.enterDownloading(40, 100));
  assert.match(await badge.getAttribute('title'), /40%/);
  assert.equal(await badge.locator('.ub-bar').evaluate(el => getComputedStyle(el).display), 'block');
  await spamUserInput(); assert.equal(await page.evaluate(() => window.calls.downloads), 1); await capture('downloading');
  report.checks.push('54×22 内部遮罩、加载环居中、边框进度保留、原生鼠标/键盘和重复事件禁用');

  await page.evaluate(() => window.failDownload()); await phase('available');
  assert.equal(await badge.isEnabled(), true); assert.equal(await badge.getAttribute('aria-busy'), 'false');
  assert.equal((await metrics()).display, 'none');
  await badge.click(); await checkBusy('downloading');
  await page.evaluate(() => window.finishDownload()); await phase('ready');
  assert.equal(await badge.isEnabled(), true); assert.equal(await badge.innerText(), '重启');
  report.checks.push('下载失败可重试，成功后恢复橙色重启');

  await badge.click(); report.applying = await checkBusy('applying');
  await spamUserInput(); assert.equal(await page.evaluate(() => window.calls.applies), 1); await capture('applying');
  await page.evaluate(() => window.failApply()); await phase('ready');
  assert.equal(await badge.isEnabled(), true); assert.match(await badge.getAttribute('title'), /文件占用/);
  await badge.click(); await phase('applying');
  await page.evaluate(() => { window.finishApply(); window.badge.setReady(); window.badge.enterDownloading(99, 100); });
  await spamUserInput(); assert.equal(await page.evaluate(() => window.calls.applies), 2); await checkBusy('applying');
  report.checks.push('重启失败可重试，成功和迟到状态均不解除安装锁定');

  await page.emulateMedia({ reducedMotion: 'reduce' });
  assert.equal((await metrics()).animation, 'none'); assert.equal((await metrics()).display, 'grid');
  assert.equal(await badge.isDisabled(), true);
  report.checks.push('减少动态效果偏好停转，静态加载环/遮罩与禁用保留');
  assert.deepEqual(report.pageErrors, []); report.status = 'passed';
} catch (error) { report.status = 'failed'; report.error = error.stack; throw error; }
finally { await writeFile(`${out}/report.json`, JSON.stringify(report, null, 2)); await browser.close(); }
console.log(JSON.stringify(report, null, 2));
