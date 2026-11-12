import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

// 运行真实侧栏模块。仅模拟原生 DOM，不连接数据库或查询写入端口。
class Node {
  constructor(tag, doc) { this.tagName = tag; this.doc = doc; this.children = []; this.attributes = {}; this.dataset = {}; this.listeners = {}; this.value = ''; this.hidden = false; }
  append(...nodes) { for (const node of nodes) { node.parentElement = this; this.children.push(node); } }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  addEventListener(name, listener) { (this.listeners[name] ??= []).push(listener); }
  fire(name, detail = {}) { const event = { target: this, preventDefault() {}, stopPropagation() {}, ...detail }; this.listeners[name]?.forEach(listener => listener(event)); }
  focus() { this.doc.activeElement = this; }
  querySelectorAll() { return this.children.flatMap(child => [...(child.tagName === 'input' && child.type === 'checkbox' ? [child] : []), ...child.querySelectorAll()]); }
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
  assert.ok(checkboxes().every(input => input.parentElement.tagName === 'label' && input.parentElement.children[1].textContent));
  const css = fs.readFileSync(new URL('../../src/ui/data-grid-controls.css', import.meta.url), 'utf8');
  assert.match(css, /column-sidebar-list[^}]*overflow: auto/); assert.match(css, /column-sidebar\s*\{[^}]*border-left/);
  assert.doesNotMatch(css, /position:\s*(absolute|fixed)/); assert.match(css, /min-height: 26px/);
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

test('last visible column is disabled while hidden columns remain available; Escape returns focus', () => {
  const { sidebar, trigger, doc, checkboxes } = fixture(2); sidebar.toggle(); assert.equal(doc.activeElement, sidebar.el.children[1]);
  const second = checkboxes()[1]; second.checked = false; second.fire('change'); assert.equal(checkboxes()[0].disabled, true); assert.equal(checkboxes()[1].disabled, false);
  sidebar.el.fire('keydown', { key: 'Escape' }); assert.equal(sidebar.el.hidden, true); assert.equal(trigger.attributes['aria-expanded'], 'false'); assert.equal(doc.activeElement, trigger);
});
