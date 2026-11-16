// 真实 Chromium + 产品组件/对话框。隔离 HTTP 页面，无数据库、原生 IPC 或新依赖。
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
import { installSearchableSelects } from './src/ui/SearchableSelect.ts';
import { Dialog } from './src/ui/Dialog.ts';
import { accessibleToolDialog } from './src/dialogs/toolDialog.ts';
window.fixture = {
  events: [], install: installSearchableSelects, dispose: installSearchableSelects(),
  openDialog() {
    let dlg = new Dialog({ title: '搜索测试', buttons: [{ label: '确认', primary: true, onClick: () => { window.fixture.submitted++; return false; } }],
      render(body) { body.innerHTML = '<label for="modal-select">模态字段</label><select id="modal-select"><option value="a">Alpha</option><option value="b">中文字段</option></select><button id="modal-next">下一项</button>'; } });
    accessibleToolDialog(dlg, '搜索测试', () => {});
    window.fixture.dialog = dlg;
  },
  submitted: 0,
};
document.addEventListener('input', e => { if (e.target instanceof HTMLSelectElement) fixture.events.push(['input', e.target.value]); });
document.addEventListener('change', e => { if (e.target instanceof HTMLSelectElement) fixture.events.push(['change', e.target.value]); });
`;
const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><link rel="stylesheet" href="/base.css"><link rel="stylesheet" href="/select.css"></head><body>
<main style="padding:40px; overflow:hidden; height:80px">
<button id="before">前一项</button><label for="field">字段</label>
<select id="field" style="width:160px"><option value="">请选择字段</option><option value="id">ID</option><option value="userName">用户名</option><option value="EMAIL_KEY" label="邮箱">ignored text</option><optgroup label="禁止选择" disabled><option value="blocked">Disabled</option></optgroup></select>
<button id="after">下一项</button><select id="multiple" multiple aria-label="多选"><option>A</option><option>B</option></select><select id="large" size="2" aria-label="列表"><option>A</option><option>B</option></select>
</main><script type="module" src="/fixture.js"></script></body></html>`;

before(async () => {
  const bundled = await build({ stdin: { contents: source, resolveDir: root, sourcefile: 'searchable-select-fixture.ts' }, bundle: true, write: false, format: 'esm', platform: 'browser' });
  const files = new Map([
    ['/', ['text/html', html]], ['/fixture.js', ['text/javascript', bundled.outputFiles[0].text]],
    ['/base.css', ['text/css', await readFile(path.join(root, 'src/styles/base.css'), 'utf8')]],
    ['/select.css', ['text/css', await readFile(path.join(root, 'src/ui/searchable-select.css'), 'utf8')]],
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
const search = page => page.locator('.searchable-select-search');
const options = page => page.locator('.searchable-select-option');
const open = async page => { await page.locator('#field').click(); await search(page).waitFor(); };
const closed = async page => assert.equal(await page.locator('.searchable-select-popover').count(), 0);
const events = page => page.evaluate(() => fixture.events);

test('保留原 DOM/闭合样式；label/value 搜索；搜索不提交；确认只派发一次 input/change', () => withPage(async page => {
  const before = await page.locator('#field').evaluate(el => { window.originalSelect = el; return [el.value, el.outerHTML, el.getBoundingClientRect().width, el.getBoundingClientRect().height]; });
  await open(page);
  assert.equal(await page.evaluate(() => originalSelect === document.getElementById('field')), true);
  assert.equal(await options(page).count(), 4, '占位不应当成为真实候选');
  await search(page).fill('email_key');
  assert.deepEqual(await options(page).allTextContents(), ['邮箱']);
  assert.equal(await page.locator('#field').inputValue(), '');
  assert.deepEqual(await events(page), []);
  await search(page).press('Enter');
  await closed(page);
  assert.equal(await page.locator('#field').inputValue(), 'EMAIL_KEY');
  assert.deepEqual(await events(page), [['input', 'EMAIL_KEY'], ['change', 'EMAIL_KEY']]);
  await open(page);
  await search(page).fill('邮箱');
  await options(page).click();
  assert.equal((await events(page)).length, 2, '同值不重复触发');
  const after = await page.locator('#field').evaluate(el => [el.getBoundingClientRect().width, el.getBoundingClientRect().height]);
  assert.deepEqual(after, before.slice(2));
  assert.equal(await page.locator('#field').getAttribute('aria-controls'), null);
}));

test('方向键/Home/End 跳过禁用分组；Enter 确认；Escape 撤销并返回可见焦点', () => withPage(async page => {
  await page.locator('#field').focus();
  await page.keyboard.press('ArrowDown');
  await search(page).press('End');
  assert.equal(await page.locator('.is-active').textContent(), '邮箱');
  await search(page).press('Home');
  assert.equal(await page.locator('.is-active').textContent(), 'ID');
  await search(page).press('ArrowDown');
  assert.equal(await page.locator('.is-active').textContent(), '用户名');
  await search(page).press('Enter');
  assert.equal(await page.locator('#field').inputValue(), 'userName');
  await page.keyboard.press('Enter');
  await search(page).fill('id');
  await search(page).press('Escape');
  assert.equal(await page.locator('#field').inputValue(), 'userName');
  assert.equal(await page.locator('#field').evaluate(el => el === document.activeElement && el.matches(':focus-visible') && getComputedStyle(el).outlineStyle !== 'none'), true);
  assert.equal((await events(page)).length, 2);
}));

test('空结果不允许自由文本；Tab/Shift+Tab 沿原 select 顺序退出', () => withPage(async page => {
  await open(page);
  await search(page).fill('不存在的字段');
  assert.equal(await options(page).count(), 0);
  assert.match(await page.getByRole('status').textContent(), /没有匹配选项/);
  await search(page).press('Enter');
  assert.equal(await page.locator('#field').inputValue(), '');
  await search(page).press('Tab');
  await closed(page);
  assert.equal(await page.locator('#after').evaluate(el => el === document.activeElement), true);
  await open(page);
  await search(page).press('Shift+Tab');
  assert.equal(await page.locator('#before').evaluate(el => el === document.activeElement), true);
  assert.deepEqual(await events(page), []);
}));

test('中文 IME Enter/Escape 不提交也不触发产品模态默认按钮', () => withPage(async page => {
  await page.evaluate(() => fixture.openDialog());
  await page.locator('#modal-select').click();
  await search(page).evaluate(el => {
    el.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    el.value = '中文'; el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', isComposing: true, bubbles: true }));
    el.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', keyCode: 229, bubbles: true }));
  });
  assert.equal(await page.locator('#modal-select').inputValue(), 'a');
  assert.equal(await page.evaluate(() => fixture.submitted), 0);
  assert.equal(await search(page).count(), 1);
  await search(page).evaluate(el => el.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true })));
  await search(page).press('Enter');
  assert.equal(await page.locator('#modal-select').inputValue(), 'b');
  assert.equal(await page.evaluate(() => fixture.submitted), 0);
  assert.equal(await page.locator('[role=dialog]').count(), 1);
}));

test('禁用/fieldset/加载占位不绕过；空值真实候选保留；多选增强、size 列表框保持原生', () => withPage(async page => {
  await page.evaluate(() => { const el = document.getElementById('field'); el.disabled = true; el.replaceChildren(new Option('正在加载…', '')); });
  await page.locator('#field').dispatchEvent('click');
  await closed(page);
  await page.evaluate(() => {
    const el = document.getElementById('field'); el.disabled = false;
    el.replaceChildren(new Option('选择…', ''), new Option('（整库）', ''), new Option('中文表', 'table'));
  });
  await open(page);
  assert.deepEqual(await options(page).allTextContents(), ['（整库）', '中文表']);
  await search(page).press('Escape');
  // 多选列表框同样增强（点选切换、浮层保持打开）；size>1 且非 multiple 的列表框保持原生。
  await page.locator('#multiple').click(); await search(page).waitFor();
  await page.locator('.searchable-select-option').filter({ hasText: 'B' }).click();
  assert.equal(await page.locator('#multiple').evaluate(el => [...el.selectedOptions].map(o => o.value).join(',')), 'B');
  assert.equal(await page.locator('.searchable-select-popover').count(), 1, '多选后浮层保持打开');
  await search(page).press('Escape');
  await page.locator('#large').click(); await closed(page);
  assert.equal(await page.locator('#large').getAttribute('aria-controls'), null);
  await page.evaluate(() => { const set = document.createElement('fieldset'); set.disabled = true; document.body.append(set); set.append(document.getElementById('field')); });
  await page.locator('#field').dispatchEvent('click'); await closed(page);
}));

test('动态 select 自动命名和增强；选项替换/禁用/移除安全关闭且不提交', () => withPage(async page => {
  await page.evaluate(() => { const el = document.createElement('select'); el.id = 'dynamic'; el.title = '数据源'; el.add(new Option('Source A', 'a')); el.add(new Option('Source B', 'b')); document.body.append(el); });
  await page.waitForFunction(() => document.getElementById('dynamic').getAttribute('aria-label') === '数据源');
  await page.locator('#dynamic').click();
  assert.equal(await search(page).getAttribute('aria-label'), '搜索数据源');
  await page.evaluate(() => document.getElementById('dynamic').replaceChildren(new Option('New C', 'c')));
  await page.waitForFunction(() => !document.querySelector('.searchable-select-popover'));
  await page.locator('#dynamic').click();
  assert.deepEqual(await options(page).allTextContents(), ['New C']);
  await page.evaluate(() => { document.getElementById('dynamic').disabled = true; });
  await page.waitForFunction(() => !document.querySelector('.searchable-select-popover'));
  await page.evaluate(() => { document.getElementById('dynamic').disabled = false; });
  await page.locator('#dynamic').click();
  await page.evaluate(() => document.getElementById('dynamic').remove());
  await page.waitForFunction(() => !document.querySelector('.searchable-select-popover'));
  assert.deepEqual(await events(page), []);
}));

test('浮层处于 top-layer；长列表滚动；外部点击关闭，滚动/缩放时跟随锚点', () => withPage(async page => {
  await page.evaluate(() => { const el = document.getElementById('field'); el.replaceChildren(...Array.from({ length: 150 }, (_, i) => new Option('字段 ' + i, 'v' + i))); });
  await open(page);
  assert.equal(await page.locator('.searchable-select-popover').evaluate(el => el.matches(':popover-open')), true);
  await search(page).press('End');
  assert.equal(await page.locator('.is-active').textContent(), '字段 149');
  assert.equal(await page.locator('.searchable-select-list').evaluate(el => el.scrollHeight > el.clientHeight && el.scrollTop > 0), true);
  const last = await page.locator('.is-active').boundingBox();
  assert.equal(await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.classList.contains('searchable-select-option'), { x: last.x + 10, y: last.y + 10 }), true, '父 overflow:hidden 不应裁切候选');
  await page.locator('#after').click(); await closed(page);
  // 容器滚动不再直接关闭浮层：跟随锚点重新定位，滚出视口才关。
  await open(page);
  const before = await page.locator('.searchable-select-popover').boundingBox();
  await page.evaluate(() => document.dispatchEvent(new Event('scroll')));
  await page.waitForTimeout(100);
  assert.equal(await page.locator('.searchable-select-popover').count(), 1, '滚动后浮层仍在');
  assert.equal(await page.locator('.searchable-select-popover').evaluate(el => el.matches(':popover-open')), true);
  const after = await page.locator('.searchable-select-popover').boundingBox();
  assert.deepEqual(after, before, '锚点未动时位置保持');
  await page.setViewportSize({ width: 640, height: 480 });
  await page.waitForTimeout(100);
  assert.equal(await page.locator('.searchable-select-popover').count(), 1, '窗口缩放时跟随而不是关闭');
  await page.evaluate(() => document.querySelector('main').style.display = 'none');
  await page.waitForFunction(() => !document.querySelector('.searchable-select-popover'));
}));

test('真实产品模态浮层保持在模态内；Escape 不关闭模态；Tab 继续且陷阱仍生效', () => withPage(async page => {
  await page.evaluate(() => fixture.openDialog());
  await page.locator('#modal-select').click();
  assert.equal(await search(page).evaluate(el => !!el.closest('[role=dialog]')), true);
  await search(page).press('Escape');
  assert.equal(await page.locator('[role=dialog]').count(), 1);
  assert.equal(await page.locator('#modal-select').evaluate(el => el === document.activeElement), true);
  await page.keyboard.press('Enter');
  await search(page).press('Tab');
  assert.equal(await page.locator('#modal-next').evaluate(el => el === document.activeElement), true);
  await page.keyboard.press('Tab');
  await page.keyboard.press('Tab');
  assert.equal(await page.evaluate(() => !!document.activeElement.closest('[role=dialog]')), true);
}));

test('原生 HTML dialog top-layer 下搜索可交互', () => withPage(async page => {
  await page.evaluate(() => {
    const dlg = document.createElement('dialog'); dlg.id = 'native';
    dlg.innerHTML = '<label for="native-select">表</label><select id="native-select"><option value="a">表 A</option><option value="b">表 B</option></select><button>下一项</button>';
    document.body.append(dlg); dlg.showModal();
  });
  await page.locator('#native-select').click();
  assert.equal(await search(page).evaluate(el => el.closest('dialog')?.id), 'native');
  await search(page).fill('表 B');
  await options(page).click();
  assert.equal(await page.locator('#native-select').inputValue(), 'b');
  assert.equal(await page.locator('#native').evaluate(el => el.open), true);
}));

test('重复安装幂等；dispose 清理浮层/名称/监听并可再次安装，无观察器循环', () => withPage(async page => {
  await page.evaluate(() => { const el = document.createElement('select'); el.id = 'unnamed'; el.add(new Option('A', 'a')); document.body.append(el); });
  await page.waitForFunction(() => document.getElementById('unnamed').hasAttribute('aria-label'));
  assert.equal(await page.evaluate(() => fixture.install() === fixture.dispose), true);
  await open(page);
  const mutations = await page.evaluate(async () => {
    let count = 0; const observer = new MutationObserver(records => { count += records.length; });
    observer.observe(document.body, { subtree: true, childList: true, attributes: true });
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    observer.disconnect(); return count;
  });
  assert.ok(mutations < 5, `闲置浮层应稳定，实际 ${mutations} 次变更`);
  await page.evaluate(() => fixture.dispose()); await closed(page);
  assert.equal(await page.locator('#unnamed').getAttribute('aria-label'), null);
  await page.locator('#field').dispatchEvent('click'); await closed(page);
  await page.evaluate(() => { fixture.dispose = fixture.install(); });
  await open(page);
  await search(page).fill('用户名'); await search(page).press('Enter');
  assert.deepEqual(await events(page), [['input', 'userName'], ['change', 'userName']]);
}));

test('局部 root 外的浮层仍可键盘搜索；移除整个 root 会清理浮层', () => withPage(async page => {
  await page.evaluate(() => {
    fixture.dispose();
    const scope = document.createElement('section'); scope.id = 'scope';
    scope.innerHTML = '<label>数据源<select id="scoped"><option value="a">源 A</option><option value="b">源 B</option></select></label>';
    document.body.append(scope); fixture.dispose = fixture.install(scope);
  });
  await page.locator('#scoped').click();
  assert.equal(await search(page).getAttribute('aria-label'), '搜索数据源', '包裹 label 的候选文本不应混入名称');
  assert.equal(await search(page).evaluate(el => !el.closest('#scope')), true);
  await search(page).fill('源 B');
  await search(page).press('Enter');
  assert.equal(await page.locator('#scoped').inputValue(), 'b');
  await page.locator('#scoped').click();
  await page.evaluate(() => document.getElementById('scope').remove());
  await page.waitForFunction(() => !document.querySelector('.searchable-select-popover'));
  await page.locator('#field').dispatchEvent('click'); await closed(page);
}));

test('原生 dialog.close() 关闭其搜索浮层；禁用候选/忙状态/必填占位不会提交', () => withPage(async page => {
  await page.evaluate(() => {
    const dlg = document.createElement('dialog'); dlg.id = 'native-close';
    dlg.innerHTML = '<select id="native-close-select" aria-label="表"><option>A</option><option>B</option></select>';
    document.body.append(dlg); dlg.showModal();
  });
  await page.locator('#native-close-select').click();
  await page.evaluate(() => document.getElementById('native-close').close());
  await page.waitForFunction(() => !document.querySelector('.searchable-select-popover'));
  await open(page);
  // Playwright 的 locator.click 会尊重 aria-disabled；这里用真实鼠标验证防护。
  const disabledRow = await options(page).filter({ hasText: 'Disabled' }).boundingBox();
  await page.mouse.click(disabledRow.x + 12, disabledRow.y + 12);
  assert.equal(await search(page).count(), 1);
  assert.deepEqual(await events(page), []);
  await page.evaluate(() => document.getElementById('field').setAttribute('aria-busy', 'true'));
  await page.waitForFunction(() => !document.querySelector('.searchable-select-popover'));
  await page.locator('#field').dispatchEvent('click'); await closed(page);
  await page.evaluate(() => {
    const el = document.getElementById('field'); el.removeAttribute('aria-busy'); el.required = true;
    el.replaceChildren(new Option('自定义必填占位', ''), new Option('有效字段', 'valid'));
  });
  await open(page);
  assert.deepEqual(await options(page).allTextContents(), ['有效字段']);
  assert.deepEqual(await events(page), []);
}));

test('祖先 inert 立即关闭已开的搜索浮层，不能通过外置浮层绕过查询锁', () => withPage(async page => {
  await open(page);
  await search(page).fill('邮箱');
  await page.evaluate(() => document.querySelector('main').inert = true);
  await page.waitForFunction(() => !document.querySelector('.searchable-select-popover'));
  await page.locator('#field').dispatchEvent('click');
  await closed(page);
  assert.deepEqual(await events(page), []);
  assert.equal(await page.locator('#field').inputValue(), '');
  await page.evaluate(() => document.querySelector('main').inert = false);
  await open(page);
  await search(page).fill('邮箱'); await search(page).press('Enter');
  assert.equal(await page.locator('#field').inputValue(), 'EMAIL_KEY');
}));

test('多选跨搜索连续勾选/取消，选中标记与 ARIA 一致且事件不重复', () => withPage(async page => {
  await page.evaluate(() => {
    const el = document.getElementById('multiple');
    el.replaceChildren(new Option('表 Alpha', 'a'), new Option('表 Beta', 'b'), new Option('表 Gamma', 'g'));
    fixture.multiEvents = [];
    for (const type of ['input', 'change']) el.addEventListener(type, () => fixture.multiEvents.push([type, [...el.selectedOptions].map(o => o.value)]));
  });
  await page.locator('#multiple').click();
  assert.equal(await page.locator('.searchable-select-list').getAttribute('aria-multiselectable'), 'true');
  await search(page).fill('Beta');
  await search(page).press('Enter');
  assert.equal(await options(page).getAttribute('aria-selected'), 'true');
  assert.equal(await options(page).locator('.searchable-select-check').evaluate(el => getComputedStyle(el).visibility), 'visible');
  await search(page).fill('Alpha');
  await options(page).click();
  assert.equal(await search(page).evaluate(el => el === document.activeElement), true);
  await search(page).fill('Beta');
  assert.equal(await options(page).getAttribute('aria-selected'), 'true', '重新过滤仍保留勾选');
  await search(page).press('Enter');
  assert.equal(await options(page).locator('.searchable-select-check').evaluate(el => getComputedStyle(el).visibility), 'hidden');
  assert.deepEqual(await page.locator('#multiple').evaluate(el => [...el.selectedOptions].map(o => o.value)), ['a']);
  assert.deepEqual(await page.evaluate(() => fixture.multiEvents), [
    ['input', ['b']], ['change', ['b']], ['input', ['a', 'b']], ['change', ['a', 'b']], ['input', ['a']], ['change', ['a']],
  ]);
  await search(page).press('Escape');
  await closed(page);
  assert.equal(await page.locator('#multiple').evaluate(el => el === document.activeElement && el.matches(':focus-visible')), true);
}));

test('真实容器滚动浮层跟随锚点，列表滚动与鼠标高亮不会回跳', () => withPage(async page => {
  await page.evaluate(() => {
    const main = document.querySelector('main');
    main.style.cssText = 'margin:20px;height:240px;width:500px;overflow:auto;padding:0';
    const container = document.createElement('div'); container.style.height = '800px';
    const el = document.getElementById('field');
    el.style.marginTop = '120px';
    el.replaceChildren(...Array.from({ length: 100 }, (_, i) => new Option('字段 ' + i, 'v' + i)));
    main.replaceChildren(container); container.append(el);
  });
  await open(page);
  const before = await page.locator('.searchable-select-popover').boundingBox();
  await page.locator('main').evaluate(el => { el.scrollTop = 40; });
  await page.waitForFunction(() => {
    const anchor = document.getElementById('field').getBoundingClientRect();
    const popover = document.querySelector('.searchable-select-popover')?.getBoundingClientRect();
    return popover && Math.abs(popover.top - anchor.bottom - 4) < 1;
  });
  const after = await page.locator('.searchable-select-popover').boundingBox();
  assert.equal(Math.round(after.y - before.y), -40);
  await search(page).press('End');
  const list = page.locator('.searchable-select-list');
  const scrolled = await list.evaluate(el => el.scrollTop);
  await options(page).last().dispatchEvent('pointermove');
  assert.equal(await list.evaluate(el => el.scrollTop), scrolled, '鼠标经过不应自动滚动');
  await page.locator('main').evaluate(el => { el.scrollTop = 700; });
  await page.waitForFunction(() => !document.querySelector('.searchable-select-popover'));
  assert.deepEqual(await events(page), []);
}));

test('data-no-search 直接选择：无搜索框、点选即生效、键盘可用、滚轮不穿透背景', () => withPage(async page => {
  await page.evaluate(() => {
    const wrap = document.createElement('div'); wrap.id = 'scroller';
    wrap.style.cssText = 'height:160px;overflow:auto;margin:20px';
    const inner = document.createElement('div'); inner.style.height = '700px';
    const el = document.createElement('select'); el.id = 'plain'; el.setAttribute('data-no-search', '');
    el.setAttribute('aria-label', '生成方式');
    el.replaceChildren(...Array.from({ length: 40 }, (_, i) => new Option('方式 ' + i, 'm' + i)));
    inner.append(el); wrap.append(inner); document.body.append(wrap);
  });
  await page.locator('#plain').click();
  assert.equal(await page.locator('.searchable-select-search').count(), 0, '不渲染搜索输入框');
  assert.equal(await options(page).count(), 40);
  assert.equal(await page.locator('.searchable-select-list').getAttribute('aria-label'), '生成方式');
  assert.equal(await page.locator('.searchable-select-list').evaluate(el => el === document.activeElement), true, '焦点落在列表上');
  await page.keyboard.press('ArrowDown');
  assert.equal(await page.locator('.is-active').textContent(), '方式 1');
  await page.keyboard.press('Escape');
  await closed(page);
  assert.equal(await page.locator('#plain').evaluate(el => el === document.activeElement), true);
  assert.equal(await page.locator('#plain').getAttribute('aria-expanded'), null);
  // 点开即选：不需要过滤，点中候选就生效并关闭。
  await page.locator('#plain').click();
  await options(page).filter({ hasText: '方式 7' }).click();
  await closed(page);
  assert.equal(await page.locator('#plain').inputValue(), 'm7');
  assert.deepEqual(await events(page), [['input', 'm7'], ['change', 'm7']]);
  // 键盘打开后方向键 + Enter 选择；字符键不再触发浮层（保留原生 type-ahead）。
  await page.locator('#plain').focus();
  await page.keyboard.press('Enter');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await closed(page);
  assert.equal(await page.locator('#plain').inputValue(), 'm8');
  await page.keyboard.press('x');
  assert.equal(await page.locator('.searchable-select-popover').count(), 0, '字符键不打开无搜索下拉');
  // 滚轮只滚动候选列表，背后的滚动容器不动（曾表现为滚动穿透）。
  await page.locator('#plain').click();
  const list = page.locator('.searchable-select-list');
  const box = await list.boundingBox();
  await page.mouse.move(box.x + 20, box.y + 20);
  await page.mouse.wheel(0, 240);
  await page.waitForTimeout(120);
  assert.ok(await list.evaluate(el => el.scrollTop > 0), '候选列表应滚动');
  assert.equal(await page.locator('#scroller').evaluate(el => el.scrollTop), 0, '背景容器不应穿透滚动');
  await page.mouse.wheel(0, 5000);
  await page.waitForTimeout(120);
  assert.equal(await page.locator('#scroller').evaluate(el => el.scrollTop), 0, '滚到列表底部也不穿透');
}));
