import { chromium } from 'playwright-core';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const base = process.env.NAVKIT_TREE_TEST_URL || 'http://localhost:5183';
const browser = await chromium.launch({ executablePath: process.env.NAVKIT_EDGE_PATH || 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', headless: true });
const page = await browser.newPage({ viewport: { width: 700, height: 650 } });
const errors = [];
page.on('pageerror', error => errors.push(error.message));
const checks = [];
try {
  await page.goto(`${base}/tools/tests/tree-fixture.html`);
  await page.waitForFunction(() => !!window.treeRegression);
  await page.evaluate(() => localStorage.clear());
  await page.reload();
  const row = id => page.locator(`.tree-node[data-id="${id}"]`);
  const snapshot = () => page.evaluate(() => window.treeRegression.connections.map(c => ({ id: c.id, group: c.group })));
  const moveCount = () => page.evaluate(() => window.treeRegression.moves.length);
  const expectMoves = async count => page.waitForFunction(count => window.treeRegression.moves.length === count && document.querySelector('.tree-scroll').getAttribute('aria-busy') !== 'true', count);

  await row('conn:a').dragTo(row('group:folder'));
  await expectMoves(1);
  assert.equal((await snapshot()).find(c => c.id === 'a').group, 'folder'); checks.push('real pointer drop into folder');
  await row('conn:a').dragTo(row('conn:c'), { targetPosition: { x: 100, y: 3 } });
  await expectMoves(2);
  assert.deepEqual((await snapshot()).filter(c => c.group === 'folder').map(c => c.id), ['a', 'c', 'd']); checks.push('connection upper half = before');
  await row('conn:a').dragTo(row('conn:d'), { targetPosition: { x: 100, y: 21 } });
  await expectMoves(3);
  assert.deepEqual((await snapshot()).filter(c => c.group === 'folder').map(c => c.id), ['c', 'd', 'a']); checks.push('connection lower half = after');
  await row('conn:a').dragTo(page.locator('.tree-root-drop'));
  await expectMoves(4);
  assert.deepEqual((await snapshot()).filter(c => !c.group).map(c => c.id), ['b', 'a']); checks.push('dedicated root drop');
  const beforeInvalid = await moveCount();
  await row('conn:a').dragTo(row('conn:c/db/main'));
  assert.equal(await moveCount(), beforeInvalid); checks.push('database child rejects drops');
  await row('conn:a').dragTo(row('conn:a'));
  assert.equal(await moveCount(), beforeInvalid); checks.push('self drop rejected');

  await row('conn:a').focus(); await page.keyboard.press('Control+x');
  await row('group:empty').focus(); await page.keyboard.press('Control+v'); await expectMoves(5);
  assert.equal((await snapshot()).find(c => c.id === 'a').group, 'empty'); checks.push('keyboard cross-group move');
  await row('conn:a').focus(); await page.keyboard.press('Control+x');
  await row('conn:d').focus(); await page.keyboard.press('Control+Shift+v'); await expectMoves(6);
  assert.deepEqual((await snapshot()).filter(c => c.group === 'folder').map(c => c.id), ['c', 'd', 'a']); checks.push('keyboard insert after');
  await page.keyboard.press('Alt+ArrowUp'); await expectMoves(7);
  assert.deepEqual((await snapshot()).filter(c => c.group === 'folder').map(c => c.id), ['c', 'a', 'd']); checks.push('keyboard same-group reorder');
  await row('conn:a').focus(); await page.keyboard.press('Control+x');
  await page.locator('.tree-root-drop').focus(); await page.keyboard.press('Enter'); await expectMoves(8);
  assert.equal((await snapshot()).find(c => c.id === 'a').group, ''); checks.push('root button Enter moves to ungrouped');
  await row('conn:a').focus(); await page.keyboard.press('Control+x'); await page.keyboard.press('Escape');
  await row('group:folder').focus(); await page.keyboard.press('Control+v');
  assert.equal(await moveCount(), 8); checks.push('Escape cancels keyboard movement');
  const externalBlocked = await page.evaluate(() => {
    const dataTransfer = new DataTransfer(); dataTransfer.items.add(new File(['external'], 'external.txt'));
    const target = document.querySelector('.tree-scroll');
    const over = new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer });
    const drop = new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer });
    target.dispatchEvent(over); target.dispatchEvent(drop);
    return over.defaultPrevented && drop.defaultPrevented && window.treeRegression.moves.length === 8;
  });
  assert.equal(externalBlocked, true); checks.push('external files cannot move or navigate WebView');
  const persisted = await snapshot();
  await page.reload(); await page.waitForFunction(() => !!window.treeRegression);
  assert.deepEqual(await snapshot(), persisted); checks.push('fixture IPC response survives page reload');

  // 在保存之前取真实 CSS 落点截图，验证可见插入线而不只检查类名。
  await page.evaluate(() => {
    const source = document.querySelector('[data-id="conn:b"]');
    const target = document.querySelector('[data-id="conn:a"]');
    const dataTransfer = new DataTransfer();
    source.dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer }));
    const bounds = target.getBoundingClientRect();
    target.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer, clientY: bounds.top + 3 }));
  });
  assert.equal(await row('conn:a').evaluate(el => el.classList.contains('drop-before')), true);
  assert.equal(await row('conn:a').evaluate(el => getComputedStyle(el, '::before').height), '2px');
  fs.mkdirSync(path.join(root, 'artifacts'), { recursive: true });
  const screenshot = path.join(root, 'artifacts/tree-drag-sort.png');
  await page.screenshot({ path: screenshot });
  await row('conn:b').dispatchEvent('dragend');
  checks.push('visible 2px insertion indicator and screenshot');

  await page.evaluate(() => { window.treeRegression.fail = true; });
  const prior = await snapshot();
  await row('conn:b').focus(); await page.keyboard.press('Control+x');
  await row('group:folder').focus(); await page.keyboard.press('Control+v');
  await page.waitForFunction(() => document.querySelector('.tree-move-status').textContent.includes('移动失败'));
  assert.deepEqual(await snapshot(), prior);
  assert.equal(await page.locator('.tree-root-drop').isDisabled(), false); checks.push('save errors announced without changing data');
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ passed: checks.length, checks, screenshot, url: page.url(), browserErrors: errors }, null, 2));
} finally { await browser.close(); }
