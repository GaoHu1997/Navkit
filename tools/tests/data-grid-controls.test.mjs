import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import ts from 'typescript';

// 运行真实侧栏模块。仅模拟原生 DOM，不连接数据库或查询写入端口。
class Node {
  constructor(tag, doc) { this.tagName = tag; this.doc = doc; this.children = []; this.attributes = {}; this.dataset = {}; this.listeners = {}; this.value = ''; this.hidden = false; this.className = ''; }
  get classList() {
    const names = () => String(this.className).split(/\s+/).filter(Boolean);
    return {
      add: (...items) => { this.className = [...new Set([...names(), ...items])].join(' '); },
      remove: (...items) => { this.className = names().filter(name => !items.includes(name)).join(' '); },
      contains: item => names().includes(item),
    };
  }
  append(...nodes) { for (const node of nodes) { node.parentElement = this; this.children.push(node); } }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  addEventListener(name, listener) { (this.listeners[name] ??= []).push(listener); }
  fire(name, detail = {}) { const event = { target: this, preventDefault() {}, stopPropagation() {}, ...detail }; this.listeners[name]?.forEach(listener => listener(event)); }
  focus() { this.doc.activeElement = this; }
  querySelectorAll(selector) {
    const out = [];
    const walk = node => { for (const child of node.children) { if (instanceofmatches(child, selector)) out.push(child); walk(child); } };
    walk(this); return out;
  }
}
function instanceofmatches(child, selector) {
  if (!selector) return child.tagName === 'input' && child.type === 'checkbox';
  if (selector === 'input[type="checkbox"]') return child.tagName === 'input' && child.type === 'checkbox';
  if (selector.startsWith('.')) return String(child.className).split(/\s+/).includes(selector.slice(1));
  return child.tagName === selector;
}
function fixture(count = 300) {
  const doc = { createElement(tag) { return new Node(tag, this); } };
  const context = vm.createContext({ document: doc, exports: {} });
  const source = fs.readFileSync(new URL('../../src/ui/ColumnSidebar.ts', import.meta.url), 'utf8').replace('export class ColumnSidebar', 'class ColumnSidebar');
  vm.runInContext(ts.transpile(source + '\nglobalThis.ColumnSidebar = ColumnSidebar;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const columns = Array.from({ length: count }, (_, index) => ({ id: `c${index}`, name: index < 2 ? '重复别名' : `field_${index}` }));
  let hidden = new Set(); const changes = [];
  const sidebar = new context.ColumnSidebar('test-columns', () => columns, () => new Set(hidden), ids => { hidden = new Set(ids); changes.push([...ids]); sidebar.render(); });
  const trigger = doc.createElement('button'); sidebar.bind(trigger); const body = doc.createElement('div'); body.append(sidebar.el);
  return { doc, columns, sidebar, trigger, changes, hidden: () => hidden, checkboxes: () => sidebar.el.querySelectorAll() };
}

test('sidebar has 300 native named checkbox rows in a scrollable nonfloating layout', () => {
  const { sidebar, trigger, checkboxes } = fixture(); assert.equal(sidebar.el.hidden, true); assert.equal(trigger.attributes['aria-expanded'], 'false');
  sidebar.toggle(); assert.equal(checkboxes().length, 300); assert.equal(sidebar.el.tagName, 'aside'); assert.equal(sidebar.el.id, trigger.attributes['aria-controls']);
  assert.ok(checkboxes().every(input => input.parentElement.tagName === 'label' && input.parentElement.children[1].children[0].textContent));
  const css = fs.readFileSync(new URL('../../src/ui/data-grid-controls.css', import.meta.url), 'utf8');
  assert.match(css, /column-sidebar-list[^}]*overflow: auto/); assert.match(css, /column-sidebar\s*\{[^}]*border-left/);
  assert.doesNotMatch(css.match(/\.column-sidebar\s*\{[^}]*\}/)?.[0] ?? '', /position:\s*(absolute|fixed)/); assert.match(css, /min-height: 26px/);
});

test('duplicate names toggle only their stable column id and preserve checkbox focus', () => {
  const { sidebar, doc, checkboxes, hidden } = fixture(); sidebar.toggle(); const second = checkboxes()[1]; second.focus(); second.checked = false; second.fire('change');
  assert.deepEqual([...hidden()], ['c1']); assert.equal(checkboxes()[0].checked, true); assert.equal(checkboxes()[1].checked, false); assert.equal(doc.activeElement, second); assert.equal(checkboxes()[1], second);
});

test('search finds many columns and restore selects all columns even when search is active', () => {
  const { sidebar, hidden, checkboxes } = fixture(); sidebar.toggle(); const search = sidebar.el.children[1]; search.value = 'field_2'; search.fire('input');
  assert.equal(checkboxes().length, 111); assert.ok(checkboxes().every(input => input.dataset.columnId.startsWith('c2')));
  const input = checkboxes()[0]; input.checked = false; input.fire('change'); assert.equal(hidden().size, 1);
  sidebar.el.children[2].fire('click'); assert.equal(hidden().size, 0); assert.ok(checkboxes().every(input => input.checked));
  search.value = 'not-found'; search.fire('input'); assert.equal(checkboxes().length, 0); assert.equal(sidebar.el.children[3].children[0].textContent, '没有匹配字段');
  search.value = ''; search.fire('input'); assert.equal(checkboxes().length, 300);
});

test('field comments render below names as small muted text and update when metadata arrives', () => {
  const { sidebar, columns, checkboxes } = fixture(3);
  columns[1].comment = '交易类型\n<script>plain text</script>';
  columns[2].comment = '   ';
  sidebar.toggle();
  const second = checkboxes()[1], text = second.parentElement.children[1], note = text.children[1];
  assert.equal(text.children[0].textContent, '重复别名');
  assert.equal(note.tagName, 'small'); assert.equal(note.className, 'column-sidebar-comment');
  assert.equal(note.textContent, columns[1].comment); assert.equal(second.attributes['aria-describedby'], note.id);
  assert.equal(second.attributes['aria-label'], '重复别名'); assert.equal(checkboxes()[2].parentElement.children[1].children.length, 1);
  columns[1].comment = '更新后的备注'; sidebar.render();
  assert.equal(checkboxes()[1].parentElement.children[1].children[1].textContent, '更新后的备注');
  const css = fs.readFileSync(new URL('../../src/ui/data-grid-controls.css', import.meta.url), 'utf8');
  assert.match(css, /column-sidebar-text[^}]*flex-direction: column/);
  assert.match(css, /column-sidebar-comment[^}]*color: var\(--text-secondary\)/);
});

test('field rows expose a trailing drag handle that reorders only through the supplied callback', () => {
  const doc = { createElement(tag) { return new Node(tag, this); } };
  const context = vm.createContext({ document: doc, exports: {} });
  const source = fs.readFileSync(new URL('../../src/ui/ColumnSidebar.ts', import.meta.url), 'utf8').replace('export class ColumnSidebar', 'class ColumnSidebar');
  vm.runInContext(ts.transpile(source + '\nglobalThis.ColumnSidebar = ColumnSidebar;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const columns = [{ id: 'a', name: 'alpha' }, { id: 'b', name: 'beta' }, { id: 'c', name: 'gamma' }];
  const moves = [];
  const sidebar = new context.ColumnSidebar('test-columns', () => columns, () => new Set(), () => {}, (from, to) => { moves.push([from, to]); });
  sidebar.toggle();
  const items = sidebar.el.children[3].children;
  assert.equal(items.length, 3);
  // 拖拽块是每行最后一个子元素，且不是勾选框本身。
  for (const item of items) {
    const handle = item.children.at(-1);
    assert.equal(handle.className, 'column-sidebar-drag');
    assert.equal(handle.draggable, true);
    assert.equal(handle.dataset.columnId, item.dataset.columnId);
    assert.match(handle.attributes['aria-label'], /调整 .* 字段顺序/);
  }
  items[0].children.at(-1).fire('dragstart', { dataTransfer: { setData() {}, effectAllowed: '' } });
  items[2].fire('dragover', { dataTransfer: { dropEffect: '' } });
  items[2].fire('drop');
  assert.deepEqual(moves, [['a', 'c']]);
  assert.equal(items[0].classList.contains('dragging'), false);
});

test('actual browser field rows highlight, separate and align controls with the name above comments', { timeout: 30000 }, async t => {
  const { chromium } = await import('playwright-core');
  const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  t.after(() => browser.close());
  const page = await browser.newPage({ viewport: { width: 800, height: 560 }, deviceScaleFactor: 1 });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  const css = ['../../src/styles/base.css', '../../src/ui/query-result-panel.css', '../../src/ui/data-grid-controls.css']
    .map(file => fs.readFileSync(new URL(file, import.meta.url), 'utf8')).join('\n');
  const source = fs.readFileSync(new URL('../../src/ui/ColumnSidebar.ts', import.meta.url), 'utf8').replace('export class ColumnSidebar', 'class ColumnSidebar');
  const script = ts.transpile(source + '\nglobalThis.ColumnSidebar = ColumnSidebar;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ES2022 });
  const output = new URL('../../artifacts/column-sidebar-layout/', import.meta.url); fs.mkdirSync(output, { recursive: true });
  for (const host of ['data-grid-controls', 'qr-panel']) for (const sortable of [true, false]) {
    await page.setContent('<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"></head><body><button id="trigger">显示字段</button><main id="host" style="display:flex;flex-direction:row;width:760px;height:480px"><div style="flex:1;min-width:0"></div></main></body></html>');
    await page.addStyleTag({ content: css }); await page.addScriptTag({ content: script, type: 'module' });
    await page.evaluate(({ host, sortable }) => {
      document.getElementById('host').className = host;
      const columns = [{ id: 'id', name: 'id' }, { id: 'trade_type', name: 'trade_type', comment: '交易类型' },
        { id: 'description', name: 'description_with_a_very_long_field_name_that_must_truncate', comment: '这是多行备注，复选框与拖拽块应当保持在字段名首行。\n' + 'long_comment_'.repeat(10) }];
      let hidden = new Set(); globalThis.moves = [];
      const reorder = (from, to) => { moves.push([from, to]); const at = columns.findIndex(column => column.id === from), target = columns.findIndex(column => column.id === to); columns.splice(target, 0, columns.splice(at, 1)[0]); };
      globalThis.sidebar = new ColumnSidebar('layout-columns', () => columns, () => new Set(hidden), ids => { hidden = new Set(ids); }, sortable ? reorder : undefined);
      document.getElementById('host').append(sidebar.el); sidebar.toggle(document.getElementById('trigger'));
    }, { host, sortable });
    const rows = page.locator('.column-sidebar-list > *');
    const metrics = await rows.evaluateAll(elements => elements.map(element => {
      const name = element.querySelector('.column-sidebar-name'), input = element.querySelector('input'), handle = element.querySelector('.column-sidebar-drag');
      const rect = node => node.getBoundingClientRect(), center = node => rect(node).top + rect(node).height / 2;
      const note = element.querySelector('.column-sidebar-comment'), style = getComputedStyle(element);
      return { checkboxOffset: center(input) - center(name), handleOffset: handle ? center(handle) - center(name) : null,
        checkboxSize: [rect(input).width, rect(input).height], borderWidth: style.borderBottomWidth, borderColor: style.borderBottomColor,
        height: rect(element).height, noteBelow: !note || rect(note).top >= rect(name).bottom,
        overflow: element.scrollWidth - element.clientWidth, truncates: name.scrollWidth > name.clientWidth };
    }));
    for (const [index, metric] of metrics.entries()) {
      assert.ok(Math.abs(metric.checkboxOffset) <= 0.5, `${host} row ${index}: checkbox centered on name`);
      if (sortable) assert.ok(Math.abs(metric.handleOffset) <= 0.5, `${host} row ${index}: handle centered on name`);
      assert.deepEqual(metric.checkboxSize, [16, 16]); assert.equal(metric.borderWidth, index < 2 ? '1px' : '0px');
      if (index < 2) assert.equal(metric.borderColor, 'rgb(232, 235, 239)');
      assert.equal(metric.noteBelow, true); assert.ok(metric.overflow <= 1);
    }
    assert.ok(metrics[0].height < metrics[1].height && metrics[1].height < metrics[2].height); assert.equal(metrics[2].truncates, true);
    const background = index => rows.nth(index).evaluate(element => getComputedStyle(element).backgroundColor);
    await page.mouse.move(0, 0); assert.equal(await background(1), 'rgba(0, 0, 0, 0)');
    for (const selector of ['.column-sidebar-name', '.column-sidebar-comment', ...(sortable ? ['.column-sidebar-drag'] : [])]) {
      await rows.nth(1).locator(selector).hover(); assert.equal(await background(1), 'rgb(241, 243, 246)', `${host}: full row feedback on ${selector}`);
    }
    if (host === 'data-grid-controls' && sortable) await page.locator('.column-sidebar').screenshot({ path: fileURLToPath(new URL('hover.png', output)) });
    await page.mouse.move(0, 0); assert.equal(await background(1), 'rgba(0, 0, 0, 0)');
    const checkboxes = page.locator('.column-sidebar-row input'); await checkboxes.nth(1).focus();
    assert.equal(await background(1), 'rgb(241, 243, 246)');
    assert.equal(await checkboxes.nth(1).evaluate(element => getComputedStyle(element).outlineStyle), 'solid');
    await page.keyboard.press('Space'); assert.equal(await checkboxes.nth(1).isChecked(), false);
    assert.equal(await checkboxes.nth(1).evaluate(element => document.activeElement === element), true);
    await checkboxes.nth(2).uncheck(); assert.equal(await checkboxes.nth(0).isDisabled(), true);
    await page.getByRole('button', { name: '全选 / 恢复全部字段' }).click(); assert.equal(await page.locator('.column-sidebar-row input:checked').count(), 3);
    if (sortable) {
      const handle = rows.nth(1).locator('.column-sidebar-drag'); await handle.focus(); await page.keyboard.press('Alt+ArrowUp');
      assert.deepEqual(await page.evaluate(() => moves), [['trade_type', 'id']]);
      assert.equal(await page.evaluate(() => document.activeElement.dataset.columnId), 'trade_type');
      await rows.nth(0).locator('.column-sidebar-drag').dispatchEvent('dragstart'); await rows.nth(2).dispatchEvent('dragover'); await rows.nth(2).hover();
      assert.equal(await background(2), 'rgb(242, 247, 254)', 'drop target stays distinct from hover');
      assert.match(await rows.nth(2).evaluate(element => getComputedStyle(element).boxShadow), /inset/);
      await rows.nth(2).dispatchEvent('drop'); assert.deepEqual(await page.evaluate(() => moves), [['trade_type', 'id'], ['trade_type', 'description']]);
    }
    await page.getByRole('searchbox', { name: '搜索字段' }).fill('missing'); assert.equal(await rows.textContent(), '没有匹配字段');
    await page.getByRole('searchbox', { name: '搜索字段' }).fill(''); assert.equal(await rows.count(), 3);
    await page.getByRole('searchbox', { name: '搜索字段' }).focus(); await page.keyboard.press('Escape');
    assert.equal(await page.locator('.column-sidebar').isVisible(), false); assert.equal(await page.locator('#trigger').evaluate(element => document.activeElement === element), true);
  }
  assert.deepEqual(errors, []);
});

test('last visible column is disabled while hidden columns remain available; Escape returns focus', () => {
  const { sidebar, trigger, doc, checkboxes } = fixture(2); sidebar.toggle(); assert.equal(doc.activeElement, sidebar.el.children[1]);
  const second = checkboxes()[1]; second.checked = false; second.fire('change'); assert.equal(checkboxes()[0].disabled, true); assert.equal(checkboxes()[1].disabled, false);
  sidebar.el.fire('keydown', { key: 'Escape' }); assert.equal(sidebar.el.hidden, true); assert.equal(trigger.attributes['aria-expanded'], 'false'); assert.equal(doc.activeElement, trigger);
});
