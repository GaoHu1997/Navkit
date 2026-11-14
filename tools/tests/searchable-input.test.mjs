// 真实 Chromium：input[list] 的搜索浮层（SearchableInput）。无数据库、原生 IPC 或新依赖。
import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';

const root = fileURLToPath(new URL('../../', import.meta.url));
let browser, server, origin;
const source = `
import { installSearchableInputs } from './src/ui/SearchableInput.ts';
window.fixture = { events: [], install: installSearchableInputs, dispose: installSearchableInputs() };
document.addEventListener('input', e => { if (e.target instanceof HTMLInputElement) fixture.events.push(['input', e.target.value]); });
document.addEventListener('change', e => { if (e.target instanceof HTMLInputElement) fixture.events.push(['change', e.target.value]); });
document.addEventListener('submit', e => { fixture.events.push(['submit']); e.preventDefault(); });
`;
const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><link rel="stylesheet" href="/base.css"><link rel="stylesheet" href="/input.css"></head><body>
<main style="padding:40px">
<form id="form"><label for="table">目标表</label>
<input id="table" list="tables" placeholder="可输入新表名"><datalist id="tables"><option value="items"></option><option value="orders"></option><option value="user_profile"></option></datalist>
<label for="columns">字段</label><input id="columns" list="cols" data-search-token="," value="id, na">
<datalist id="cols"><option value="id"></option><option value="name"></option><option value="note"></option></datalist>
<button id="submit" type="submit">提交</button></form>
</main><script type="module" src="/fixture.js"></script></body></html>`;

before(async () => {
  const bundled = await build({ stdin: { contents: source, resolveDir: root, sourcefile: 'searchable-input-fixture.ts' }, bundle: true, write: false, format: 'esm', platform: 'browser' });
  const files = new Map([
    ['/', ['text/html', html]], ['/fixture.js', ['text/javascript', bundled.outputFiles[0].text]],
    ['/base.css', ['text/css', await readFile(path.join(root, 'src/styles/base.css'), 'utf8')]],
    ['/input.css', ['text/css', await readFile(path.join(root, 'src/ui/searchable-select.css'), 'utf8')]],
  ]);
  server = createServer((req, res) => {
    const file = files.get(new URL(req.url, 'http://127.0.0.1').pathname);
    res.writeHead(file ? 200 : 404, { 'Content-Type': `${file?.[0] ?? 'text/plain'}; charset=utf-8` });
    res.end(file?.[1] ?? '未找到');
  });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  origin = `http://127.0.0.1:${server.address().port}`;
  const executablePath = process.env.BROWSER_EXE || ['C:/Program Files/Google/Chrome/Application/chrome.exe', 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe'].find(existsSync);
  browser = await chromium.launch({ executablePath, headless: true });
}, { timeout: 45000 });
after(async () => { try { await browser?.close(); } finally { if (server) await new Promise(resolve => server.close(resolve)); } });

async function withPage(run) {
  const context = await browser.newContext({ viewport: { width: 800, height: 600 } });
  const page = await context.newPage();
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  try {
    await page.goto(origin);
    await page.waitForFunction(() => !!window.fixture);
    await run(page);
    assert.deepEqual(errors, []);
  } finally { await context.close(); }
}

const panel = page => page.locator('.searchable-input-popover');
const options = page => page.locator('.searchable-input-popover .searchable-select-option');
const empty = page => page.locator('.searchable-input-popover .searchable-select-empty');
const events = page => page.evaluate(() => fixture.events);
const openTable = async page => { await page.locator('#table').click(); await panel(page).waitFor(); };
const closed = async page => assert.equal(await panel(page).count(), 0);

test('点击输入框弹出全部建议；输入过滤；点击候选写入并只派发一次 input/change', () => withPage(async page => {
  await openTable(page);
  assert.deepEqual(await options(page).allTextContents(), ['items', 'orders', 'user_profile']);
  assert.equal(await page.locator('#table').getAttribute('list'), null, '打开期间摘掉 list，避免原生弹层叠加');
  await page.keyboard.type('ord');
  assert.deepEqual(await options(page).allTextContents(), ['orders']);
  await options(page).click();
  await closed(page);
  assert.equal(await page.locator('#table').inputValue(), 'orders');
  assert.equal(await page.locator('#table').getAttribute('list'), 'tables', '关闭后恢复 list');
  assert.equal(await page.locator('#table').getAttribute('aria-expanded'), null);
  // 浮层里的逐字输入本身就是真实输入事件；点击候选后再补一次最终值。
  assert.deepEqual((await events(page)).slice(-2), [['input', 'orders'], ['change', 'orders']]);
}));

test('自由输入不拦截；无匹配时提示仍可手动输入', () => withPage(async page => {
  await page.locator('#table').fill('brand_new');
  await panel(page).waitFor();
  assert.equal(await options(page).count(), 0);
  assert.match(await empty(page).textContent(), /继续手动输入/);
  await page.locator('#submit').click();
  assert.equal(await page.locator('#table').inputValue(), 'brand_new');
  await closed(page);
  assert.deepEqual((await events(page)).filter(([kind]) => kind === 'submit'), [['submit']]);
}));

test('Enter 不劫持表单提交；方向键选择后 Enter 才采纳', () => withPage(async page => {
  await openTable(page);
  await page.keyboard.press('Enter');
  assert.deepEqual(await events(page).then(list => list.filter(([kind]) => kind === 'submit')), [['submit']], '未输入过滤词且未移动选中项时 Enter 走原路径');
  await page.keyboard.type('user');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  assert.equal(await page.locator('#table').inputValue(), 'user_profile');
  assert.deepEqual((await events(page)).slice(-2), [['input', 'user_profile'], ['change', 'user_profile']]);
}));

test('部分匹配的新表名 Enter 保持自由输入；明确导航后才采纳建议', () => withPage(async page => {
  await page.locator('#table').fill('order');
  await panel(page).waitFor();
  assert.deepEqual(await options(page).allTextContents(), ['orders']);
  await page.keyboard.press('Enter');
  assert.equal(await page.locator('#table').inputValue(), 'order', '新表名不能被改成已有表 orders');
  assert.deepEqual((await events(page)).filter(([kind]) => kind === 'submit'), [['submit']]);
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  assert.equal(await page.locator('#table').inputValue(), 'orders');
  assert.deepEqual((await events(page)).slice(-2), [['input', 'orders'], ['change', 'orders']]);
}));

test('多值输入框只过滤与替换逗号后的最后一段', () => withPage(async page => {
  await page.locator('#columns').click();
  await panel(page).waitFor();
  assert.deepEqual(await options(page).allTextContents(), ['name'], '只按最后一段 "na" 过滤（"note" 不含 na）');
  await page.keyboard.type('m');
  assert.deepEqual(await options(page).allTextContents(), ['name'], '继续输入后按 "nam" 过滤');
  await options(page).click();
  assert.equal(await page.locator('#columns').inputValue(), 'id, name');
}));

test('Escape 关闭不改值且焦点留在输入框；外部点击与 Tab 也关闭', () => withPage(async page => {
  await openTable(page);
  await page.keyboard.type('item');
  await page.keyboard.press('Escape');
  await closed(page);
  assert.equal(await page.locator('#table').inputValue(), 'item');
  assert.equal(await page.locator('#table').evaluate(el => el === document.activeElement), true);
  await openTable(page);
  await page.keyboard.press('Tab');
  await closed(page);
  await openTable(page);
  await page.locator('main').click({ position: { x: 5, y: 5 } });
  await closed(page);
}));

test('datalist 选项被替换时刷新；输入框禁用时关闭；dispose 后不再拦截', () => withPage(async page => {
  await openTable(page);
  await page.evaluate(() => {
    const list = document.getElementById('tables');
    list.replaceChildren(new Option('custom_a'), new Option('custom_b'));
  });
  assert.deepEqual(await options(page).allTextContents(), ['custom_a', 'custom_b']);
  await page.evaluate(() => { document.getElementById('table').disabled = true; });
  await closed(page);
  await page.evaluate(() => { const el = document.getElementById('table'); el.disabled = false; el.value = ''; fixture.dispose(); });
  await page.locator('#table').click();
  assert.equal(await panel(page).count(), 0, 'dispose 后交回原生 datalist');
  assert.equal(await page.locator('#table').getAttribute('list'), 'tables');
}));

test('重复安装返回同一清理函数，重复调用安全', () => withPage(async page => {
  assert.equal(await page.evaluate(() => fixture.install() === fixture.install()), true);
  await page.evaluate(() => { fixture.dispose(); fixture.dispose(); });
  await page.locator('#table').click();
  assert.equal(await panel(page).count(), 0);
}));

test('Tab 聚焦后可方向键开启建议；输入法确认不提交也不采纳建议', () => withPage(async page => {
  await page.locator('#table').focus();
  await closed(page);
  await page.keyboard.press('ArrowDown');
  await panel(page).waitFor();
  assert.deepEqual(await options(page).allTextContents(), ['items', 'orders', 'user_profile']);
  await page.locator('#table').dispatchEvent('compositionstart');
  await page.locator('#table').dispatchEvent('keydown', { key: 'Enter', bubbles: true });
  await page.locator('#table').dispatchEvent('keydown', { key: 'Escape', bubbles: true });
  assert.equal(await panel(page).count(), 1);
  assert.equal(await page.locator('#table').inputValue(), '');
  assert.deepEqual(await events(page), []);
  await page.locator('#table').dispatchEvent('compositionend');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  assert.equal(await page.locator('#table').inputValue(), 'orders');
  assert.deepEqual(await events(page), [['input', 'orders'], ['change', 'orders']]);
}));

test('输入框重复点击与真实鼠标拖选文字不会关闭建议或产生业务事件', () => withPage(async page => {
  await page.locator('#table').fill('user');
  await panel(page).waitFor();
  await page.locator('#table').click();
  assert.equal(await panel(page).count(), 1);
  await page.evaluate(() => { fixture.events = []; });
  const box = await page.locator('#table').boundingBox();
  await page.mouse.move(box.x + 5, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width - 5, box.y + box.height / 2, { steps: 8 });
  await page.mouse.up();
  assert.equal(await panel(page).count(), 1);
  assert.equal(await page.locator('#table').evaluate(el => el.selectionEnd > el.selectionStart), true);
  assert.equal(await page.locator('#table').inputValue(), 'user');
  assert.deepEqual(await events(page), []);
  await page.keyboard.type('ord');
  assert.deepEqual(await options(page).allTextContents(), ['orders']);
  await options(page).click();
  assert.equal(await page.locator('#table').inputValue(), 'orders');
}));

test('真实滚动时建议浮层跟随输入框，鼠标高亮不会回滚列表', () => withPage(async page => {
  await page.evaluate(() => {
    const main = document.querySelector('main');
    main.style.cssText = 'margin:20px;height:240px;width:500px;overflow:auto;padding:0';
    const container = document.createElement('div'); container.style.height = '800px';
    const input = document.getElementById('table'); input.style.marginTop = '120px';
    const list = document.getElementById('tables');
    list.replaceChildren(...Array.from({ length: 100 }, (_, i) => new Option('table_' + i)));
    main.replaceChildren(container); container.append(input, list);
  });
  await openTable(page);
  const before = await panel(page).boundingBox();
  await page.locator('main').evaluate(el => { el.scrollTop = 40; });
  await page.waitForFunction(() => {
    const anchor = document.getElementById('table').getBoundingClientRect();
    const popover = document.querySelector('.searchable-input-popover')?.getBoundingClientRect();
    return popover && Math.abs(popover.top - anchor.bottom - 4) < 1;
  });
  assert.equal(Math.round((await panel(page).boundingBox()).y - before.y), -40);
  await page.keyboard.press('End');
  const list = page.locator('.searchable-input-popover .searchable-select-list');
  const scrolled = await list.evaluate(el => el.scrollTop);
  await options(page).last().dispatchEvent('pointermove');
  assert.equal(await list.evaluate(el => el.scrollTop), scrolled);
  await page.locator('main').evaluate(el => { el.scrollTop = 700; });
  await page.waitForFunction(() => !document.querySelector('.searchable-input-popover'));
  assert.equal(await page.locator('#table').getAttribute('list'), 'tables');
  assert.deepEqual(await events(page), []);
}));
