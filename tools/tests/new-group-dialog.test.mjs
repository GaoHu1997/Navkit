// Real Chromium layout regression using the production command, Dialog and CSS.
// IPC is mocked in memory; this is not native/database acceptance.
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import ts from 'typescript';
import { chromium } from 'playwright-core';

const root = new URL('../../', import.meta.url);
const appText = await readFile(new URL('src/core/App.ts', root), 'utf8');
const appSource = ts.createSourceFile('App.ts', appText, ts.ScriptTarget.Latest, true);
const appClass = appSource.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === 'App');
const command = appClass?.members.find(node => node.name?.getText(appSource) === 'cmdNewGroup');
assert.ok(command, 'Production cmdNewGroup must exist');
const dialogText = await readFile(new URL('src/ui/Dialog.ts', root), 'utf8');
const strippedDialog = dialogText.replace(/^import .*;\s*$/gm, '').replace(/^export /gm, '');
const css = await readFile(new URL('src/styles/base.css', root), 'utf8');
const source = ts.transpileModule(`
${strippedDialog}
globalThis.groupCalls = [];
const api = { addGroup: async name => { groupCalls.push(name); return [name]; } };
globalThis.app = new class {
  groups = []; refreshes = 0; notices = [];
  refreshTree() { this.refreshes++; }
  toast(kind, message) { this.notices.push({ kind, message }); }
  ${command.getText(appSource)}
};
`, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None } }).outputText;
const out = new URL('artifacts/new-group-dialog/', root);

async function withPage(run) {
  const browser = await chromium.launch({
    executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe',
    headless: true,
  });
  try {
    const page = await browser.newPage({ viewport: { width: 866, height: 452 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.setContent('<!doctype html><html><body></body></html>');
    await page.addStyleTag({ content: css });
    await page.addScriptTag({ content: 'const ICON_CROSS = ""; const svg = () => "";' + source });
    await run(page);
    assert.deepEqual(errors, [], 'No uncaught browser errors');
  } finally {
    await browser.close();
  }
}

for (const viewport of [
  { width: 866, height: 452 },
  { width: 320, height: 240 },
  { width: 1560, height: 980 },
]) test(`new group dialog fits and remains centered at ${viewport.width}x${viewport.height}`, async () => withPage(async page => {
  await page.setViewportSize(viewport);
  await page.evaluate(() => app.cmdNewGroup());
  const metrics = await page.locator('.dialog').evaluate(dialog => {
    const body = dialog.querySelector('.dialog-body');
    const row = body.querySelector('.field');
    const label = body.querySelector('.field-label');
    const input = body.querySelector('input');
    const box = element => {
      const r = element.getBoundingClientRect();
      return { x: r.x, y: r.y, width: r.width, height: r.height, right: r.right, bottom: r.bottom };
    };
    // 标签文字的实际着墨范围（label 是 right-aligned，元素盒不等于可见文字位置）。
    const range = document.createRange();
    range.selectNodeContents(label);
    const labelText = box({ getBoundingClientRect: () => range.getBoundingClientRect() });
    return {
      dialog: box(dialog), body: box(body), label: box(label), labelText, input: box(input),
      overflow: body.scrollWidth - body.clientWidth,
      focused: document.activeElement === input,
      paddingTop: parseFloat(getComputedStyle(body).paddingTop),
      paddingBottom: parseFloat(getComputedStyle(body).paddingBottom),
      paddingLeft: parseFloat(getComputedStyle(body).paddingLeft),
      rowMargin: parseFloat(getComputedStyle(row).marginBottom),
    };
  });
  assert.ok(metrics.overflow <= 1, `Content must not overflow: ${JSON.stringify(metrics)}`);
  assert.ok(metrics.body.right - metrics.input.right >= 16, 'Input and focus ring must keep right inset');
  // 左侧可见留白 = 标签文字到内容区左边缘；它应与右侧留白相同，避免“左空右挤”。
  const leftInset = metrics.labelText.x - metrics.body.x;
  const rightInset = metrics.body.right - metrics.input.right;
  assert.ok(Math.abs(leftInset - metrics.paddingLeft) <= 1.5, `Label text must sit at the body left padding: ${JSON.stringify(metrics)}`);
  assert.ok(Math.abs(leftInset - rightInset) <= 1.5, `Left and right whitespace must match: left=${leftInset} right=${rightInset}`);
  assert.equal(metrics.focused, true, 'Opening still focuses the input');
  assert.equal(metrics.paddingTop, metrics.paddingBottom, 'Body has balanced vertical padding');
  assert.equal(metrics.rowMargin, 0, 'Single field must not add a trailing row gap');
  assert.ok(Math.abs(metrics.dialog.x + metrics.dialog.width / 2 - viewport.width / 2) <= 1);
  assert.ok(Math.abs(metrics.dialog.y + metrics.dialog.height / 2 - viewport.height / 2) <= 1);
  assert.ok(metrics.dialog.x >= 16 && metrics.dialog.right <= viewport.width - 16);
  if (viewport.width === 866) {
    await mkdir(out, { recursive: true });
    await page.screenshot({ path: fileURLToPath(new URL('fixed.png', out)) });
  }
  await page.getByRole('button', { name: '取消', exact: true }).click();
  assert.equal(await page.locator('.dialog').count(), 0);
  assert.equal(await page.evaluate(() => groupCalls.length), 0, 'Cancel must not create a group');
}));

test('new group creation still trims the name, refreshes the tree and closes', async () => withPage(async page => {
  await page.evaluate(() => app.cmdNewGroup());
  await page.locator('.dialog input').fill('   ');
  await page.getByRole('button', { name: '确定', exact: true }).click();
  assert.equal(await page.locator('.dialog').count(), 1, 'Empty group must stay open');
  assert.equal(await page.evaluate(() => groupCalls.length), 0);
  await page.locator('.dialog input').fill('  开发环境  ');
  await page.getByRole('button', { name: '确定', exact: true }).click();
  await page.waitForFunction(() => app.refreshes === 1 && !document.querySelector('.dialog'));
  assert.deepEqual(await page.evaluate(() => ({ calls: groupCalls, groups: app.groups })), {
    calls: ['开发环境'], groups: ['开发环境'],
  });
}));

test('pressing Enter in the group name confirms the dialog', () => withPage(async page => {
  await page.evaluate(() => app.cmdNewGroup());
  const input = page.locator('.dialog input');
  await input.fill('开发环境');
  await input.press('Enter');
  await page.waitForFunction(() => app.refreshes === 1 && !document.querySelector('.dialog'));
  assert.deepEqual(await page.evaluate(() => groupCalls), ['开发环境'], 'Enter must submit through the primary action');
}));

test('Enter on an empty group name keeps the dialog open and creates nothing', () => withPage(async page => {
  await page.evaluate(() => app.cmdNewGroup());
  const input = page.locator('.dialog input');
  await input.fill('   ');
  await input.press('Enter');
  await page.waitForTimeout(50);
  assert.equal(await page.locator('.dialog').count(), 1, 'Blank name must keep the dialog open');
  assert.equal(await page.evaluate(() => groupCalls.length), 0);
}));

test('Enter while an IME composition is active must not submit', () => withPage(async page => {
  await page.evaluate(() => app.cmdNewGroup());
  await page.locator('.dialog input').fill('开发');
  const composed = await page.evaluate(() => {
    const input = document.querySelector('.dialog input');
    const event = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true, isComposing: true });
    input.dispatchEvent(event);
    return { prevented: event.defaultPrevented };
  });
  assert.equal(composed.prevented, false, 'Composition Enter must keep native behaviour');
  assert.equal(await page.locator('.dialog').count(), 1);
  assert.equal(await page.evaluate(() => groupCalls.length), 0);
}));

// 通用性 + 安全性：任意 Dialog 的文本框回车确认；多行文本回车不提交。
test('the shared Enter shortcut confirms any dialog with a text field but ignores textareas', () => withPage(async page => {
  await page.evaluate(() => {
    window.__submits = 0;
    window.__generic = new Dialog({
      title: '通用弹框', width: 380,
      render: body => {
        const text = textInput(); text.id = 'g-text';
        const area = document.createElement('textarea'); area.id = 'g-area';
        body.append(field('名称:', text), field('备注:', area));
      },
      buttons: [{ label: '确定', primary: true, onClick: () => { window.__submits++; } }, { label: '取消' }],
    });
  });
  await page.locator('#g-area').focus();
  await page.locator('#g-area').press('Enter');
  await page.waitForTimeout(50);
  assert.equal(await page.evaluate(() => window.__submits), 0, 'Textarea Enter inserts a newline, not submit');
  assert.equal(await page.locator('.dialog').count(), 1);

  await page.locator('#g-text').fill('abc');
  await page.locator('#g-text').press('Enter');
  await page.waitForFunction(() => window.__submits === 1 && !document.querySelector('.dialog'));
}));
