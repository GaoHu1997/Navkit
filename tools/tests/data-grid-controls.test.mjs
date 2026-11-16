import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
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

test('last visible column is disabled while hidden columns remain available; Escape returns focus', () => {
  const { sidebar, trigger, doc, checkboxes } = fixture(2); sidebar.toggle(); assert.equal(doc.activeElement, sidebar.el.children[1]);
  const second = checkboxes()[1]; second.checked = false; second.fire('change'); assert.equal(checkboxes()[0].disabled, true); assert.equal(checkboxes()[1].disabled, false);
  sidebar.el.fire('keydown', { key: 'Escape' }); assert.equal(sidebar.el.hidden, true); assert.equal(trigger.attributes['aria-expanded'], 'false'); assert.equal(doc.activeElement, trigger);
});
