// Real browser + production dialog. Only IPC and the long UI deadline are controlled.
import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { buildSync } from 'esbuild';
import { chromium } from 'playwright-core';

const root = new URL('../../', import.meta.url);
const bundle = buildSync({
  stdin: { contents: `export { openConnectionDialog } from './src/dialogs/ConnectionDialog'; export { driverApi } from './src/core/api';`, resolveDir: fileURLToPath(root) },
  bundle: true, write: false, platform: 'browser', format: 'iife', globalName: 'connectionUI',
  loader: { '.svg': 'dataurl', '.png': 'dataurl' }, logLevel: 'silent',
}).outputFiles[0].text;
const css = await readFile(new URL('src/styles/base.css', root), 'utf8');

async function withPage(run) {
  const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  try {
    const page = await browser.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.setContent('<!doctype html><html><body><button id="opener">新建</button></body></html>');
    await page.addStyleTag({ content: css });
    await page.evaluate(() => {
      window.__PREVIEW_MOCK__ = { list_driver_statuses: () => [] };
      window.calls = []; window.pending = []; window.saves = [];
      const nativeTimeout = window.setTimeout;
      // Opt-in deterministic expiry; production deadlines remain unmodified.
      window.setTimeout = (fn, ms, ...args) => nativeTimeout(fn, window.fastDeadline && ms === 32000 ? 60 : ms, ...args);
    });
    await page.addScriptTag({ content: bundle });
    await page.evaluate(() => {
      connectionUI.driverApi.testConnection = (...args) => {
        calls.push(args); return new Promise((resolve, reject) => pending.push({ resolve, reject }));
      };
      connectionUI.driverApi.options = async () => [];
      window.openForm = (existing = null, kind = 'mysql') => {
        document.querySelector('#opener').focus();
        window.dialog = connectionUI.openConnectionDialog(existing, [], value => saves.push(value), kind);
      };
      window.saved = { id: 'saved', name: 'QA', kind: 'mysql', host: 'localhost', port: 3306, user: 'root', password: 'ciphertext', database: '', file: '', params: '', group: '', read_only: false, keep_alive: false };
      openForm();
    });
    await run(page);
    assert.deepEqual(errors, [], 'No uncaught browser errors');
  } finally { await browser.close(); }
}
const status = page => page.getByRole('status');
const testButton = page => page.getByRole('button', { name: '测试连接', exact: true });
for (const kind of ['kingbase', 'clickhouse', 'tidb']) {
  test(`${kind} password supports typing, select-all replacement and draft preservation`, () => withPage(async page => {
    await page.evaluate(kind => { dialog.close(); openForm(null, kind); }, kind);
    const password = page.getByLabel('密码', { exact: true });
    assert.equal(await password.getAttribute('type'), 'password');
    await password.click(); await password.pressSequentially('first-secret_123');
    assert.equal(await password.inputValue(), 'first-secret_123');
    await password.press('ControlOrMeta+A'); await password.pressSequentially('replacement-secret_456');
    assert.equal(await password.inputValue(), 'replacement-secret_456');
    await page.getByRole('tab', { name: '高级', exact: true }).click();
    await page.getByRole('tab', { name: '常规', exact: true }).click();
    assert.equal(await password.inputValue(), 'replacement-secret_456');
    await testButton(page).click();
    assert.equal(await password.isDisabled(), true);
    assert.deepEqual(await page.evaluate(() => ({ kind: calls[0][0].kind, password: calls[0][0].password, useSaved: calls[0][2] })),
      { kind, password: 'replacement-secret_456', useSaved: false });
    await page.evaluate(() => pending[0].resolve({ ok: false, message: 'fixture authentication rejected' }));
    await page.waitForFunction(() => !document.querySelector('.dialog').hasAttribute('aria-busy'));
    assert.equal(await password.inputValue(), 'replacement-secret_456');
    await page.getByRole('button', { name: '保存', exact: true }).click();
    assert.equal(await page.evaluate(() => saves[0].password), 'replacement-secret_456');
  }));

  test(`${kind} retains saved ciphertext until password is explicitly replaced`, () => withPage(async page => {
    await page.evaluate(kind => { dialog.close(); openForm({ ...saved, kind }); }, kind);
    const password = page.getByLabel('密码', { exact: true });
    assert.equal(await password.inputValue(), '');
    assert.match(await password.getAttribute('placeholder'), /已保存密码/);
    await testButton(page).click();
    assert.equal(await page.evaluate(() => calls[0][0].password), '');
    assert.equal(await page.evaluate(() => calls[0][2]), true);
    await page.evaluate(() => pending[0].resolve({ ok: true }));
    await page.waitForFunction(() => !document.querySelector('.dialog').hasAttribute('aria-busy'));
    await page.getByRole('button', { name: '保存', exact: true }).click();
    assert.deepEqual(await page.evaluate(() => ({ password: saves[0].password, stored: saves[0].config.password })),
      { password: undefined, stored: 'ciphertext' });
    await page.evaluate(kind => openForm({ ...saved, kind }), kind);
    await password.click(); await password.pressSequentially('new-secret');
    await password.press('ControlOrMeta+A'); await password.pressSequentially('updated-secret');
    await testButton(page).click();
    assert.equal(await page.evaluate(() => calls[1][0].password), 'updated-secret');
    assert.equal(await page.evaluate(() => calls[1][2]), false);
    await page.evaluate(() => pending[1].resolve({ ok: true }));
    await page.waitForFunction(() => !document.querySelector('.dialog').hasAttribute('aria-busy'));
    await page.getByRole('button', { name: '保存', exact: true }).click();
    assert.equal(await page.evaluate(() => saves[1].password), 'updated-secret');
    assert.equal(await page.evaluate(() => saves[1].config.password), 'ciphertext');
  }));
}

const fillPassword = page => page.getByLabel('密码', { exact: true }).fill('test-secret');

for (const kind of ['mysql', 'postgresql', 'sqlserver']) test(`empty ${kind} password requires explicit intent without IPC`, () => withPage(async page => {
  await page.evaluate(kind => { dialog.close(); openForm(null, kind); }, kind);
  await testButton(page).click();
  await assert.match(await status(page).innerText(), /请输入密码/);
  assert.equal(await page.evaluate(() => calls.length), 0);
  assert.equal(await page.getByLabel('密码', { exact: true }).getAttribute('aria-invalid'), 'true');
  assert.equal(await page.getByRole('button', { name: '保存', exact: true }).isEnabled(), true);
  await page.getByRole('checkbox', { name: '使用空密码 / 无密码认证', exact: true }).check();
  await testButton(page).click();
  assert.equal(await page.evaluate(() => calls[0][0].password), '');
  await page.evaluate(() => pending[0].resolve({ ok: false, message: 'Access denied' }));
  await page.waitForFunction(() => !document.querySelector('.dialog').hasAttribute('aria-busy'));
  assert.match(await status(page).innerText(), /Access denied/);
}));

test('editing preserves encrypted saved password; explicit clearing requires confirmation', () => withPage(async page => {
  await page.evaluate(() => { dialog.close(); openForm(saved); });
  await testButton(page).click();
  assert.equal(await page.evaluate(() => calls[0][2]), true);
  await page.evaluate(() => pending[0].resolve({ ok: true, version: '8.0' }));
  await page.waitForFunction(() => !document.querySelector('.dialog').hasAttribute('aria-busy'));
  await fillPassword(page);
  await page.getByLabel('密码', { exact: true }).fill('');
  await testButton(page).click();
  assert.equal(await page.evaluate(() => calls.length), 1);
  assert.match(await status(page).innerText(), /请输入密码/);
}));

test('blank or invalid network fields fail locally without entering busy state', () => withPage(async page => {
  await fillPassword(page);
  for (const [label, value, message] of [['主机 / 地址', ' ', /主机/], ['端口', '', /端口/], ['端口', '0', /端口/], ['端口', '1.5', /整数/], ['端口', '65536', /端口/]]) {
    await page.getByLabel('主机 / 地址', { exact: true }).fill('localhost');
    await page.getByLabel('端口', { exact: true }).fill('3306');
    await page.getByLabel(label, { exact: true }).fill(value);
    await testButton(page).click(); assert.match(await status(page).innerText(), message);
    assert.equal(await page.evaluate(() => calls.length), 0);
    assert.equal(await testButton(page).isEnabled(), true);
  }
}));

test('repeat submission is ignored, stop restores inputs, old completion cannot replace retry', () => withPage(async page => {
  await fillPassword(page); await testButton(page).click();
  await page.evaluate(() => { const b = [...dialog.footer.querySelectorAll('button')].find(b => b.textContent === '测试连接'); b.dispatchEvent(new MouseEvent('click')); });
  assert.equal(await page.evaluate(() => calls.length), 1);
  assert.equal(await page.getByLabel('密码', { exact: true }).isDisabled(), true);
  await page.getByRole('button', { name: '停止等待', exact: true }).click();
  await page.waitForFunction(() => !document.querySelector('.dialog').hasAttribute('aria-busy'));
  assert.match(await status(page).innerText(), /已停止等待/);
  assert.equal(await page.getByLabel('密码', { exact: true }).inputValue(), 'test-secret');
  await testButton(page).click();
  await page.evaluate(() => pending[1].resolve({ ok: true, version: 'NEW_RESULT' }));
  await page.waitForFunction(() => document.querySelector('[role=status]').textContent.includes('NEW_RESULT'));
  await page.evaluate(() => pending[0].resolve({ ok: false, message: 'OLD_RESULT' }));
  assert.match(await status(page).innerText(), /NEW_RESULT/);
  await page.getByLabel('主机 / 地址', { exact: true }).fill('changed-host');
  assert.match(await status(page).innerText(), /已更改.*重新测试/);
}));

test('hung IPC expires, keeps draft and permits retry; late rejection is consumed', () => withPage(async page => {
  await page.evaluate(() => { window.fastDeadline = true; });
  await fillPassword(page); await testButton(page).click();
  await page.waitForFunction(() => document.querySelector('[role=status]').textContent.includes('超时'));
  assert.equal(await testButton(page).isEnabled(), true);
  assert.equal(await page.getByLabel('密码', { exact: true }).inputValue(), 'test-secret');
  await page.evaluate(() => { window.fastDeadline = false; });
  await testButton(page).click();
  await page.evaluate(() => pending[0].reject(new Error('late old rejection')));
  assert.equal(await page.evaluate(() => document.querySelector('.dialog').getAttribute('aria-busy')), 'true');
  await page.evaluate(() => pending[1].resolve({ ok: true }));
  await page.waitForFunction(() => document.querySelector('[role=status]').textContent.includes('连接成功'));
}));

test('advanced option diagnostics cannot keep connection test busy forever', () => withPage(async page => {
  await page.evaluate(() => { dialog.close(); openForm(saved); window.fastDeadline = true; connectionUI.driverApi.options = () => new Promise(() => {}); });
  await testButton(page).click(); await page.evaluate(() => pending[0].resolve({ ok: true }));
  await page.waitForFunction(() => document.querySelector('[role=status]').textContent.includes('超时'));
  assert.equal(await testButton(page).isEnabled(), true);
}));

for (const close of ['取消', '关闭', 'Escape']) test(`closing via ${close} ignores pending completion and leaves reopened dialog intact`, () => withPage(async page => {
  await fillPassword(page); await testButton(page).click();
  if (close === 'Escape') await page.keyboard.press('Escape');
  else await page.getByRole('button', { name: close, exact: true }).click();
  assert.equal(await page.locator('.dialog').count(), 0);
  assert.equal(await page.locator('#opener').evaluate(el => el === document.activeElement), true);
  await page.evaluate(() => { openForm(); pending[0].resolve({ ok: true, version: 'STALE' }); });
  assert.doesNotMatch(await status(page).innerText(), /STALE/);
  assert.equal(await testButton(page).isEnabled(), true);
}));

test('anonymous H2 is not forced to provide a password and saving does not require a test', () => withPage(async page => {
  await page.evaluate(() => { dialog.close(); openForm(null, 'h2'); });
  await testButton(page).click(); assert.equal(await page.evaluate(() => calls.length), 1);
  await page.evaluate(() => pending[0].resolve({ ok: true }));
  await page.waitForFunction(() => !document.querySelector('.dialog').hasAttribute('aria-busy'));
  await page.getByRole('button', { name: '保存', exact: true }).click();
  assert.equal(await page.evaluate(() => saves.length), 1);
}));
