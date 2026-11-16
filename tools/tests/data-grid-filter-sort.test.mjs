import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import './data-grid-controls.test.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
class Element {
  constructor(tag = 'div') {
    this.tagName = tag; this.children = []; this.dataset = {}; this.style = { setProperty(name, value) { this[name] = value; } }; this.attributes = {};
    this.listeners = {}; this.scrollTop = 0; this.clientHeight = 400; this.value = ''; this.className = '';
    this.classList = {
      add: (...xs) => { this.className = [...new Set([...this.className.split(' ').filter(Boolean), ...xs])].join(' '); },
      remove: (...xs) => { this.className = this.className.split(' ').filter(x => !xs.includes(x)).join(' '); },
      contains: x => this.className.split(' ').includes(x),
      toggle: (x, force) => { const yes = force ?? !this.classList.contains(x); yes ? this.classList.add(x) : this.classList.remove(x); },
    };
  }
  set innerHTML(value) { this.children = []; this._html = value; }
  get innerHTML() { return this._html ?? ''; }
  appendChild(child) { child.parentElement = this; this.children.push(child); return child; }
  append(...children) { children.forEach(c => this.appendChild(c)); }
  replaceChildren(...children) { this.children = []; this.append(...children); }
  contains(node) { return node === this || this.children.some(child => child.contains(node)); }
  remove() { if (this.parentElement) this.parentElement.children = this.parentElement.children.filter(c => c !== this); }
  setAttribute(k, v) { this.attributes[k] = v; }
  getAttribute(k) { return this.attributes[k] ?? null; }
  removeAttribute(k) { delete this.attributes[k]; }
  replaceWith(node) {
    const p = this.parentElement;
    if (!p) return;
    const idx = p.children.indexOf(this);
    if (idx >= 0) p.children[idx] = node;
    node.parentElement = p;
  }
  addEventListener(name, fn) { (this.listeners[name] ??= []).push(fn); }
  removeEventListener(name, fn) { this.listeners[name] = (this.listeners[name] ?? []).filter(f => f !== fn); }
  fire(name, props = {}) { for (const fn of this.listeners[name] ?? []) fn({ target: this, preventDefault() {}, stopPropagation() {}, ...props }); }
  matches(selector) {
    const cls = selector.match(/\.([\w-]+)/)?.[1];
    const tag = selector.match(/^([\w]+)/)?.[1];
    const attr = selector.match(/\[data-(\w+)(?:="([^"]*)")?\]/);
    return (!tag || tag === this.tagName) && (!cls || this.classList.contains(cls)) &&
      (!attr || (this.dataset[attr[1]] !== undefined && (attr[2] === undefined || this.dataset[attr[1]] === attr[2])));
  }
  closest(selector) { return this.matches(selector) ? this : this.parentElement?.closest(selector) ?? null; }
  querySelectorAll(selector) {
    return this.children.flatMap(c => [...(c.matches?.(selector) ? [c] : []), ...(c.querySelectorAll?.(selector) ?? [])]);
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  getBoundingClientRect() { return { left: 100, right: 200, top: 30, bottom: 430 }; }
  focus() { this.focused = true; }
  select() {}
}
function harness() {
  const document = { addEventListener() {}, removeEventListener() {}, body: new Element('body'), createElement: tag => new Element(tag), createTextNode: text => Object.assign(new Element('#text'), { textContent: text }), createDocumentFragment: () => new Element('fragment') };
  const window = new Element('window');
  window.getSelection = () => null;
  const api = { pageTable: async () => ({ columns: [], rows: [], total: 0, primary_keys: [], elapsed_ms: 0 }) };
  document.querySelector = selector => document.body.querySelector(selector);
  let frameId = 0;
  const frames = new Map();
  const flushFrame = () => { const batch = [...frames]; frames.clear(); for (const [, callback] of batch) callback(); };
  const context = vm.createContext({ exports: {}, document, window, api, performance, setInterval, clearInterval, setTimeout, queueMicrotask, requestAnimationFrame: fn => { const id = ++frameId; frames.set(id, fn); return id; }, cancelAnimationFrame: id => frames.delete(id), crypto: globalThis.crypto, console, iconSvgByName: name => `<svg aria-hidden="true" data-icon="${name}"></svg>`, svg: () => '', renderCellPreview() {}, GRID_CONTEXT_MENU: [],
    ICON_PLUS_LINE: {}, ICON_MINUS_LINE: {}, ICON_CHECK: {}, ICON_CROSS: {}, ICON_PAGE_FIRST: {}, ICON_PAGE_PREV: {}, ICON_PAGE_NEXT: {}, ICON_PAGE_LAST: {}, ICON_MORE_DOTS: {}, ICON_GRID_VIEW: {}, ICON_FORM_VIEW: {},
    TabView: class { constructor(app, tab) { this.app = app; this.tab = tab; this.el = document.createElement('div'); } },
  });
  const sidebarSource = fs.readFileSync(path.join(root, 'src/ui/ColumnSidebar.ts'), 'utf8').replace('export class ColumnSidebar', 'class ColumnSidebar');
  vm.runInContext(ts.transpile(sidebarSource + '\nglobalThis.ColumnSidebar = ColumnSidebar;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const gridSource = fs.readFileSync(path.join(root, 'src/ui/DataGrid.ts'), 'utf8').replace(/^import .*;$/gm, '').replace('export class DataGrid', 'class DataGrid');
  vm.runInContext(ts.transpile(gridSource + '\nglobalThis.DataGrid = DataGrid;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const filterControls = fs.readFileSync(path.join(root, 'src/ui/TableFilterControls.ts'), 'utf8').replace(/^export /gm, '');
  vm.runInContext(ts.transpile(filterControls, { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const appSource = fs.readFileSync(path.join(root, 'src/core/App.ts'), 'utf8');
  const viewSource = appSource.slice(appSource.indexOf('class DataTabView extends'), appSource.indexOf('/** 查询视图（运行条'));
  vm.runInContext(ts.transpile(viewSource + '\nglobalThis.DataTabView = DataTabView;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const columns = ['id', 'team', 'name'].map(name => ({ name, type: 'text', nullable: true, primary_key: false, auto_increment: false }));
  const tab = { filters: [], orderBy: null, ascending: true, pageOffset: 4, pageLimit: 20, connectionId: 'test', schema: '', object: 'items', selectedRows: new Set(), page: { columns, rows: [], total: 0, primary_keys: [] } };
  const notices = [];
  const app = { toast: (...args) => notices.push(args), renderStatus() {}, updateTableQueryElapsed() {}, updateInfoPanelPublic() {}, notifyToolbar() {}, handleColumnSelected() {}, showGridLinesState: () => false, skipNullState: () => false, textPreviewActive: () => false, renderPreviewPanel: () => {} };
  const view = new context.DataTabView(app, tab);
  return { context, document, window, api, tab, view, notices, columns, frames, flushFrame, makeView: () => new context.DataTabView(app, tab) };
}
test('grid absolute row numbers include page offset while data indices remain local', () => {
  const { context, columns } = harness();
  const grid = new context.DataGrid();
  const rows = Array.from({ length: 200 }, (_, i) => [String(i), 'team', 'name']);
  grid.setData({ columns, rows, rowNumberOffset: 200 });
  assert.equal(grid.buildRow(0).children[0].textContent, '201');
  assert.equal(grid.buildRow(199).children[0].textContent, '400');
  assert.equal(grid.buildRow(199).dataset.row, '199');
  assert.equal(grid.buildRow(199).children[1].dataset.row, '199');
  grid.setData({ columns, rows, rowNumberOffset: 100000, rowFlags: { 0: 'dirty', 1: 'delete', 2: 'insert' } });
  assert.equal(grid.buildRow(0).children[0].textContent, '●100001');
  assert.equal(grid.buildRow(1).children[0].textContent, '✕100002');
  assert.equal(grid.buildRow(2).children[0].textContent, '+100003');
  assert(Number.parseInt(grid.table.style['--grid-row-header-width']) >= 58);
  grid.setData({ columns, rows, rowNumberOffset: 0, rowFlags: {} }); assert.equal(grid.buildRow(0).children[0].textContent, '1');
  grid.dispose();
});

test('filter/sort toolbar entries do not depend on row count or pending state', () => {
  const source = fs.readFileSync(path.join(root, 'src/core/App.ts'), 'utf8');
  for (const id of ['filter', 'sort']) {
    const entry = source.match(new RegExp('\\{ id: "' + id + '", label:[^}]+\\}'))?.[0];
    assert.ok(entry, `missing ${id} toolbar entry`);
    assert.ok(!entry.includes('disabled:'), `${id} must remain available for zero-result recovery`);
  }
});

const plain = value => JSON.parse(JSON.stringify(value));

test('filter editor: zero initial conditions, sensitive grouped filters and explicit empty strings', () => {
  const { view, tab } = harness();
  view.reload = async () => {};
  view.focusFilter();
  assert.equal(view.filterLines.length, 0);
  assert.equal(view.el.querySelector('.fb-case'), null);
  view.addFilterLine();
  view.filterLines[0].conds[0].touched = true;
  // 同一行追加条件 → 整行括为一组，行内连接方式默认 OR
  view.addLineCond(0);
  view.filterLines[0].conds[1].val = 'abc';
  const joins = view.filterRowsEl.querySelectorAll('.fb-join-text');
  assert.equal(joins.length, 1, '首行无行间连接方式，只有行内一个');
  assert.equal(joins[0].textContent, 'OR');
  // 连接方式点击就地切换
  joins[0].fire('click');
  assert.equal(joins[0].textContent, 'AND');
  assert.equal(view.filterLines[0].conds[1].logic, 'AND');
  joins[0].fire('click');
  view.applyFilters();
  assert.equal(tab.filters.length, 1);
  assert.equal(tab.filters[0].conds.length, 2);
  assert.equal(tab.filters[0].conds[0].value, '');
  assert.equal(tab.filters[0].conds[1].logic, 'OR');
  assert.ok(tab.filters[0].conds.every(c => c.case_sensitive));
  assert.equal(tab.pageOffset, 0);
});

test('clicking value shows an inline input; Enter commits, Esc cancels; operators show SQL only', () => {
  const { view, tab } = harness();
  view.reload = async () => {};
  view.focusFilter();
  view.addFilterLine();
  const val = view.filterRowsEl.querySelector('.fb-val');
  assert.equal(val.textContent, '<?>'); assert.equal(val.tagName, 'button'); assert.equal(val.type, 'button');
  val.fire('click');
  const input = view.filterRowsEl.querySelector('input.fb-edit');
  assert.ok(input, '点击值后出现无边框输入框');
  input.value = 'abc';
  input.fire('keydown', { key: 'Escape' });
  assert.equal(view.filterLines[0].conds[0].val, '', 'Esc 不保存');
  assert.equal(view.filterRowsEl.querySelector('input.fb-edit'), null, '输入框已收起');
  // 再次点击输入并回车 → 提交且显示回文本
  view.filterRowsEl.querySelector('.fb-val').fire('click');
  const input2 = view.filterRowsEl.querySelector('input.fb-edit');
  input2.value = 'abc';
  input2.fire('keydown', { key: 'Enter' });
  assert.equal(view.filterLines[0].conds[0].val, 'abc');
  assert.equal(view.filterRowsEl.querySelector('input.fb-edit'), null, '输入完毕不再有输入框');
  assert.equal(view.filterRowsEl.querySelector('.fb-val').textContent, 'abc');
  // 操作符只显示 SQL 文本（无中文）
  const op = view.filterRowsEl.querySelector('.fb-op-txt');
  assert.equal(op.value, '='); assert.equal(op.children.find(option => option.value === '=').textContent, '=');
  view.filterLines[0].conds[0].op = 'is null';
  view.renderFilterLines();
  assert.equal(view.filterRowsEl.querySelector('.fb-op-txt').value, 'is null');
  assert.equal(view.filterRowsEl.querySelector('.fb-val').style.display, 'none', 'IS NULL 无值输入');
});

test('line and in-line logic survive tab reopen; missing logic falls back', () => {
  const { view, tab, makeView } = harness();
  view.reload = async () => {};
  view.focusFilter();
  view.addFilterLine();
  view.addLineCond(0);   // 行1 两条
  view.addFilterLine();  // 行2
  view.addLineCond(1);   // 行2 两条
  const joins = view.filterRowsEl.querySelectorAll('.fb-join-text');
  assert.equal(joins.length, 3, '行1内 + 行2行间 + 行2内');
  assert.equal(joins[1].textContent, 'AND');
  joins[1].fire('click');
  assert.equal(joins[1].textContent, 'OR');
  view.filterLines.forEach(line => line.conds.forEach(condition => { condition.touched = true; }));
  view.applyFilters();
  assert.deepEqual(plain(tab.filters.map(l => l.logic ?? null)), [null, 'OR']);
  assert.deepEqual(plain(tab.filters.map(l => l.conds.map(c => c.logic ?? null))), [[null, 'OR'], [null, 'OR']]);
  // 重新打开标签页：行间 / 行内连接方式原样恢复
  const reopened = makeView();
  assert.equal(reopened.filterLines.length, 2);
  assert.equal(reopened.filterLines[1].logic, 'OR');
  assert.equal(reopened.filterLines[0].conds[1].logic, 'OR');
  // 旧数据：行内 logic 缺省回退 OR
  const legacy = harness();
  legacy.tab.filters = [{ conds: [{ column: 'id', operator: '=', value: '1' }, { column: 'team', operator: '=', value: 'a' }] }];
  const legacyView = legacy.makeView();
  assert.deepEqual(plain(legacyView.filterLines[0].conds.map(c => c.logic ?? null)), [null, 'OR']);
});

test('line groups build a = 1 AND (b = 2 OR c = 3) AND d = 4', () => {
  const { view, tab } = harness();
  view.reload = async () => {};
  view.focusFilter(); view.addFilterLine(); view.addFilterLine(); view.addFilterLine();
  Object.assign(view.filterLines[0].conds[0], { col: 'id', op: '=', val: '3' });
  Object.assign(view.filterLines[1].conds[0], { col: 'team', op: 'like', val: 'b%' });
  view.addLineCond(1);
  Object.assign(view.filterLines[1].conds[1], { col: 'name', op: 'is null', val: '' });
  Object.assign(view.filterLines[2].conds[0], { col: 'note', op: '=', val: 'x' });
  // 括号只在多条件行出现
  const brackets = view.filterRowsEl.querySelectorAll('.fb-bracket');
  assert.equal(brackets.length, 2);
  view.applyFilters();
  assert.deepEqual(plain(tab.filters), [
    { conds: [{ column: 'id', operator: '=', value: '3', case_sensitive: true }] },
    { logic: 'AND', conds: [
      { column: 'team', operator: 'like', value: 'b%', case_sensitive: true },
      { column: 'name', operator: 'is null', value: '', case_sensitive: true, logic: 'OR' },
    ] },
    { logic: 'AND', conds: [{ column: 'note', operator: '=', value: 'x', case_sensitive: true }] },
  ]);
});

test('multi-sort add/remove, priority, direction and drafts are independent of applied state', () => {
  const { view, tab } = harness();
  view.reload = async () => {};
  view.focusSort(); view.addSortRow(); view.addSortRow();
  assert.deepEqual(plain(view.sortRows.map(r => r.column)), ['id', 'team']);
  const second = view.sortRowsEl.children[1];
  const dir = second.querySelector('.fb-dir'); dir.fire('click');
  assert.equal(view.sortRows[1].ascending, false, '点击方向文本切换升 / 降序');
  assert.equal(dir.textContent, 'ASC');
  second.children.find(c => c.title === '上移').fire('click');
  assert.equal(tab.orderBy, null, '未应用时不得提前改变后端排序状态');
  view.focusSort(); view.focusSort();
  assert.equal(view.sortRows[0].column, 'team', '重新打开不丢失草稿');
  view.applySort();
  assert.deepEqual(plain(tab.order_rules), [{ column: 'team', ascending: false }, { column: 'id', ascending: true }]);
  assert.equal(tab.orderBy, 'team');
  assert.equal(view.sortRowsEl.children[0].children.find(c => c.title === '上移').disabled, true);
  view.sortRowsEl.children[0].children.find(c => c.textContent === '×').fire('click');
  assert.equal(view.sortRows.length, 1);
  view.sortRows = []; view.applySort();
  assert.deepEqual(plain(tab.order_rules), []); assert.equal(tab.orderBy, null);
});

test('pending edits block filter/sort without mutating applied state', () => {
  const { view, tab, notices } = harness();
  view.filterLines = [{ conds: [{ col: 'name', op: '=', val: 'abc' }] }];
  view.sortRows = [{ column: 'name', ascending: false }];
  view.pendingUpdates.set(0, new Map([['name', 'new']]));
  view.applyFilters(); view.applySort();
  assert.equal(tab.filters.length, 0); assert.equal(tab.orderBy, null); assert.equal(tab.pageOffset, 4);
  assert.equal(notices.length, 2);
});

test('latest page response wins; request carries logic and multi-sort even for empty results', async () => {
  const { view, tab, api, columns } = harness();
  const pending = []; api.pageTable = req => new Promise(resolve => pending.push({ req, resolve }));
  view.applyData = () => {};
  tab.filter_logic = 'OR'; tab.order_rules = [{ column: 'team', ascending: false }, { column: 'id', ascending: true }];
  const older = view.reload(); const newer = view.reload();
  assert.equal(pending[1].req.filter_logic, 'OR'); assert.equal(pending[1].req.order_rules.length, 2);
  pending[1].resolve({ columns, rows: [], total: 0, primary_keys: [] }); await newer;
  pending[0].resolve({ columns, rows: [['old']], total: 1, primary_keys: [] }); await older;
  assert.equal(tab.page.total, 0); assert.equal(view.el.attributes['aria-busy'], 'false');
});

test('resize handle has reliable cursor, fixed drag feedback, min width, cleanup and no accidental header action', async () => {
  const { context, document, columns } = harness();
  let clicks = 0;
  const grid = new context.DataGrid({ onSort: () => clicks++ });
  grid.setData({ columns, rows: [] });
  const handle = grid.el.querySelector('.grid-column-resize-handle');
  const th = handle.parentElement;
  assert.equal(handle.style.cursor, 'col-resize'); assert.equal(handle.attributes.role, 'separator');
  grid.onHeaderHover({ target: th, clientX: 199 }); assert.equal(th.style.cursor, 'col-resize');
  document.body.style.cursor = 'wait';
  grid.onHeaderMouseDown({ target: handle, button: 0, clientX: 199, preventDefault() {} });
  const guide = document.body.querySelector('.grid-resizer');
  assert.equal(guide.style.position, 'fixed'); assert.equal(guide.style.background, 'var(--accent-500)');
  assert.equal(document.body.querySelector('.grid-resize-overlay').style.cursor, 'col-resize');
  grid.onMouseMove({ clientX: 229 });
  assert.equal(grid.colWidths[0], 126); assert.equal(guide.children[0].textContent, '126 px');
  grid.onMouseMove({ clientX: -999 }); assert.equal(grid.colWidths[0], 40);
  grid.onMouseUp(); grid.onHeaderClick({ target: handle });
  assert.equal(clicks, 0); assert.equal(document.body.style.cursor, 'wait');
  assert.equal(document.body.querySelector('.grid-resizer'), null);
  await new Promise(resolve => setTimeout(resolve, 1));
  grid.onHeaderClick({ target: th }); assert.equal(clicks, 1);
  grid.onResizeKeyDown({ target: handle, key: 'ArrowRight', shiftKey: false, preventDefault() {}, stopPropagation() {} });
  assert.equal(grid.colWidths[0], 48);
  grid.onHeaderMouseDown({ target: handle, button: 0, clientX: 199, preventDefault() {} });
  grid.dispose(); assert.equal(document.body.querySelector('.grid-resizer'), null);
});

test('multiple header sort markers show priority and direction', () => {
  const { context, columns } = harness();
  const grid = new context.DataGrid();
  grid.setData({ columns, rows: [], orderRules: [{ column: 'team', ascending: false }, { column: 'id', ascending: true }] });
  const headers = grid.el.querySelectorAll('th[data-col]');
  assert.equal(headers[1].attributes['aria-sort'], 'descending');
  assert.equal(headers[0].querySelector('.grid-sort').title, '优先级 2，升序');
});

test('header context menu prevents bubbling and generic redispatch without replacing the frozen cell context', () => {
  const { context, columns } = harness();
  const header = [], generic = [];
  const grid = new context.DataGrid({ onHeaderContextMenu: (...args) => header.push(args), onContextMenu: (...args) => generic.push(args) });
  grid.setData({ columns, rows: [['1', 'A', 'alice']] });
  const target = grid.el.querySelectorAll('th[data-col]')[1].querySelector('.grid-th-name');
  const previousTarget = { row: 0, col: 2 }, previousCopy = { generation: grid.copyGeneration, rows: [['alice']] };
  grid.contextTarget = previousTarget; grid.frozenContext = previousCopy;
  const event = { target, clientX: 34, clientY: 56, prevented: false, stopped: false,
    preventDefault() { this.prevented = true; }, stopPropagation() { this.stopped = true; } };
  grid.onHeaderContextMenu(event);
  assert.equal(event.prevented, true); assert.equal(event.stopped, true);
  assert.deepEqual(header, [['team', 1, 34, 56]]);
  grid.onContextMenu(event); // 防御性直接分派也不能覆盖专属菜单。
  assert.deepEqual(generic, []);
  assert.equal(grid.contextTarget, previousTarget); assert.equal(grid.frozenContext, previousCopy);
  grid.dispose();
});

test('cell, row and blank context menus retain generic routing, and headers without a dedicated callback fall back', () => {
  const { context, columns } = harness();
  const generic = [];
  const grid = new context.DataGrid({ onContextMenu: (...args) => generic.push(args) });
  grid.setData({ columns, rows: [['1', 'A', 'alice']] });
  const event = target => ({ target, clientX: 34, clientY: 56, prevented: false, stopped: false,
    preventDefault() { this.prevented = true; }, stopPropagation() { this.stopped = true; } });
  const headerEvent = event(grid.el.querySelectorAll('th[data-col]')[1].querySelector('.grid-th-name'));
  grid.onHeaderContextMenu(headerEvent);
  assert.equal(headerEvent.prevented, false); assert.equal(headerEvent.stopped, false);
  grid.onContextMenu(headerEvent);
  assert.equal(headerEvent.prevented, true);
  for (const target of [grid.tbody.querySelector('td[data-col="1"]'), grid.tbody.querySelector('.grid-row-header'), grid.scroll]) {
    const e = event(target); grid.onContextMenu(e); assert.equal(e.prevented, true); assert.equal(e.stopped, false);
  }
  assert.deepEqual(generic, [[34, 56, -1, -1], [34, 56, 0, 1], [34, 56, 0, -1], [34, 56, -1, -1]]);
  grid.dispose();
});

function paginationFixture(total = 65, offset = 0) {
  const h = harness();
  h.tab.pageOffset = offset;
  h.tab.page.total = total;
  h.view.renderFooter(h.tab.page);
  const pager = () => h.view.footer.children.find(c => c.className === 'grid-pager');
  return { ...h, pager, button: title => pager().children.find(c => c.title === title) };
}

test('pagination is right-aligned with native controls; view switcher sits at the far right after a divider', () => {
  const { view, tab, pager } = paginationFixture();
  tab.filters = [{ conds: [{ column: 'name', operator: '=', value: 'Alice' }] }];
  view.renderFooter(tab.page);
  assert.equal(pager().className, 'grid-pager');
  assert.equal(pager().tagName, 'nav');
  assert.equal(pager().attributes['aria-label'], '表格分页');
  assert.ok(view.footer.children.findIndex(c => c.className === 'grid-footer-spacer') < view.footer.children.indexOf(pager()));
  assert.ok(view.footer.children.indexOf(pager()) >= 0);
  const switchWrap = view.footer.children.find(c => c.className === 'view-switch');
  assert.ok(switchWrap, '页脚应包含视图切换');
  assert.deepEqual(switchWrap.children.map(c => c.title), ['网格视图', '表单视图']);
  assert.equal(view.footer.children.at(-1), switchWrap);
  assert.ok(view.footer.children.some(c => c.className === 'grid-footer-divider'));
  assert.ok(view.footer.children.some(c => c.textContent?.startsWith('筛选:')));
  assert.deepEqual(pager().children.filter(c => c.tagName === 'button').map(c => c.attributes['aria-label']), ['第一页', '上一页', '下一页', '最后一页']);
  assert.ok(pager().children.filter(c => c.tagName === 'button').every(c => c.type === 'button'));
  const position = pager().children.find(c => c.tagName === 'form');
  assert.equal(position.children[0].attributes['aria-label'], '当前页码');
  assert.equal(position.children[0].inputMode, 'numeric');
  assert.equal(position.children[1].textContent, '/ 4 页');
  assert.ok(!view.footer.children.some(c => c.title === '更多'));
  assert.ok(!pager().children.some(c => c.title === '更多'));
});

test('data pages render the grid by default, preserving multiple selected rows and cell drafts', () => {
  const { view, tab } = harness();
  tab.page.rows = [['1', 'red', 'Alice'], ['2', 'blue', 'Bob']];
  tab.page.total = 2;
  tab.selectedRows = new Set([0, 1]);
  view.pendingUpdates.set(1, new Map([['name', 'Updated Bob']]));
  view.applyData();
  assert.deepEqual(plain(view.grid.rows), [['1', 'red', 'Alice'], ['2', 'blue', 'Updated Bob']]);
  assert.deepEqual([...tab.selectedRows], [0, 1]);
  assert.equal(view.grid.el.style.display || '', '');
  assert.equal(view.el.querySelectorAll('.data-form-view-host').length, 1);
  assert.equal(view.formHost.hidden, true);
  assert.equal(typeof view.viewMode, 'function');
  assert.ok('formIndex' in view);
  assert.equal(view.footer.children.at(-1).className, 'view-switch');
  assert.ok(view.footer.children.some(c => c.className === 'grid-pager'));
});

test('pagination disables boundary controls and all controls for empty or single-page results', () => {
  const { view, tab, button, pager } = paginationFixture();
  let reloads = 0;
  view.reload = async () => { reloads++; };
  assert.equal(button('第一页').disabled, true);
  assert.equal(button('上一页').disabled, true);
  assert.equal(button('下一页').disabled, false);
  assert.equal(button('最后一页').disabled, false);
  button('第一页').fire('click'); button('上一页').fire('click');
  assert.equal(reloads, 0);
  tab.pageOffset = 60;
  view.renderFooter(tab.page);
  assert.equal(button('第一页').disabled, false);
  assert.equal(button('上一页').disabled, false);
  assert.equal(button('下一页').disabled, true);
  assert.equal(button('最后一页').disabled, true);
  button('下一页').fire('click'); button('最后一页').fire('click');
  assert.equal(reloads, 0);
  for (const total of [0, 1, 20]) {
    tab.pageOffset = 0; tab.page.total = total;
    view.renderFooter(tab.page);
    assert.ok(pager().children.filter(c => c.tagName === 'button').every(c => c.disabled));
    assert.equal(pager().children.find(c => c.tagName === 'form').children[1].textContent, '/ 1 页');
  }
});

test('pagination first, previous, next and last buttons update the page offset once', () => {
  const { view, tab, button } = paginationFixture();
  let reloads = 0;
  view.reload = async () => { reloads++; view.renderFooter(tab.page); };
  button('下一页').fire('click'); assert.equal(tab.pageOffset, 20);
  button('最后一页').fire('click'); assert.equal(tab.pageOffset, 60);
  button('上一页').fire('click'); assert.equal(tab.pageOffset, 40);
  button('第一页').fire('click'); assert.equal(tab.pageOffset, 0);
  assert.equal(reloads, 4);
});

test('page input rejects invalid values, skips the current page, supports submit and clamps to the last page', () => {
  const { view, tab, pager } = paginationFixture();
  let reloads = 0;
  view.reload = async () => { reloads++; view.renderFooter(tab.page); };
  const position = () => pager().children.find(c => c.tagName === 'form');
  const input = () => position().children[0];
  for (const value of ['', '0', '-1', '1.5', 'abc', 'Infinity']) {
    input().value = value; input().fire('change');
    assert.equal(input().value, '1'); assert.equal(tab.pageOffset, 0);
  }
  input().value = '1'; input().fire('change');
  assert.equal(reloads, 0);
  input().value = '3';
  let prevented = false;
  position().fire('submit', { preventDefault() { prevented = true; } });
  assert.equal(prevented, true); assert.equal(tab.pageOffset, 40); assert.equal(reloads, 1);
  input().value = '999'; input().fire('change');
  assert.equal(tab.pageOffset, 60); assert.equal(input().value, '4'); assert.equal(reloads, 2);
});

test('pending edits still block pagination buttons and direct page jumps', () => {
  const { view, tab, pager, button, notices } = paginationFixture();
  let reloads = 0;
  view.reload = async () => { reloads++; };
  view.pendingUpdates.set(0, new Map([['name', 'new']]));
  view.renderFooter(tab.page);
  button('下一页').fire('click');
  const input = pager().children.find(c => c.tagName === 'form').children[0];
  input.value = '3'; input.fire('change');
  assert.equal(tab.pageOffset, 0); assert.equal(input.value, '1'); assert.equal(reloads, 0);
  assert.equal(notices.length, 2);
  assert.ok(notices.every(n => n[1] === '存在未提交的修改，请先提交或回滚'));
});

test('filter + inserts the next group and ()+ appends to the current group; hint stays visible', () => {
  const { view } = harness(); view.focusFilter();
  assert.equal(view.filterLines.length, 0); assert.equal(view.filterPlaceholderEl.style.display, '');
  assert.equal(view.filterBarEl.querySelectorAll('button').some(button => button.textContent.includes('添加条件行')), false);
  view.filterRowsEl.querySelector('button').fire('click');
  assert.equal(view.filterLines.length, 1); assert.equal(view.filterLines[0].conds[0].col, 'id');
  let row = view.filterRowsEl.children[0];
  row.children.find(button => button.textContent === '()＋').fire('click');
  assert.equal(view.filterLines.length, 1); assert.equal(view.filterLines[0].conds.length, 2);
  row = view.filterRowsEl.children[0]; row.children.find(button => button.textContent === '＋').fire('click');
  view.filterLines[1].conds[0].val = 'last group';
  view.filterRowsEl.children[0].children.find(button => button.textContent === '＋').fire('click');
  assert.equal(view.filterLines.length, 3); assert.equal(view.filterLines[1].conds[0].val, ''); assert.equal(view.filterLines[2].conds[0].val, 'last group');
  assert.equal(view.filterPlaceholderEl.style.display, '');
  view.focusFilter(); view.focusFilter(); assert.equal(view.filterLines.length, 3);
});

test('untouched placeholder and blur send no empty condition; Enter explicitly accepts empty string', () => {
  const { view, tab, notices } = harness(); let reloads = 0; view.reload = async () => { reloads++; };
  view.focusFilter(); view.addFilterLine(); view.applyFilters();
  assert.equal(tab.filters.length, 0); assert.equal(tab.pageOffset, 4); assert.equal(reloads, 0); assert.match(notices.at(-1)[1], /尚未填写/);
  view.filterRowsEl.querySelector('.fb-val').fire('click'); view.filterRowsEl.querySelector('input.fb-edit').fire('blur');
  view.applyFilters(); assert.equal(reloads, 0); assert.equal(view.filterLines[0].conds[0].touched, false);
  view.filterRowsEl.querySelector('.fb-val').fire('click');
  const input = view.filterRowsEl.querySelector('input.fb-edit'); assert.equal(input.placeholder, '<?>'); input.fire('keydown', { key: 'Enter' });
  assert.equal(reloads, 1); assert.equal(tab.filters[0].conds[0].value, ''); assert.equal(tab.filters[0].conds[0].case_sensitive, true);
  assert.equal(view.filterRowsEl.querySelector('.fb-val').textContent, '""');
});

test('clearing a typed value is explicit and NULL operators do not require a touched value', () => {
  const { view, tab } = harness(); view.reload = async () => {}; view.focusFilter(); view.addFilterLine();
  view.filterRowsEl.querySelector('.fb-val').fire('click'); let input = view.filterRowsEl.querySelector('input.fb-edit'); input.value = 'typed'; input.fire('input'); input.value = ''; input.fire('input'); input.fire('blur');
  view.applyFilters(); assert.equal(tab.filters[0].conds[0].value, '');
  view.addFilterLine(); const nullCondition = view.filterLines[1].conds[0]; nullCondition.op = 'is not null'; view.applyFilters();
  assert.equal(tab.filters[1].conds[0].operator, 'is not null'); assert.equal(nullCondition.touched, false);
});

test('value blur leaves the pressed group add button in the DOM and operator selection keeps focus', () => {
  const { view } = harness(); view.focusFilter(); view.addFilterLine();
  const row = view.filterRowsEl.children[0], plus = row.children.find(button => button.textContent === '＋');
  view.filterRowsEl.querySelector('.fb-val').fire('click'); const input = view.filterRowsEl.querySelector('input.fb-edit'); input.value = 'committed on blur'; input.fire('input'); input.fire('blur');
  assert.equal(view.filterRowsEl.children[0], row); assert.ok(row.children.includes(plus)); plus.fire('click'); assert.equal(view.filterLines.length, 2);
  const op = view.filterRowsEl.querySelector('.fb-op-txt'); op.focus(); op.value = 'is null'; op.fire('change');
  assert.equal(view.filterRowsEl.querySelector('.fb-op-txt'), op); assert.equal(op.focused, true); assert.equal(view.filterRowsEl.querySelector('.fb-val').style.display, 'none');
});

test('old insensitive filters are restored as explicit values and every reload forces sensitivity', async () => {
  const h = harness(); h.tab.filters = [{ conds: [{ column: 'name', operator: 'in', value: 'a,A', case_sensitive: false }, { column: 'team', operator: '=', value: '', case_sensitive: false }] }];
  const view = h.makeView(); let request; h.api.pageTable = async value => { request = value; return { columns: h.columns, rows: [], total: 0, primary_keys: [] }; };
  assert.equal(view.filterLines[0].conds[1].touched, true);
  await view.reload(); assert.ok(request.filters[0].conds.every(condition => condition.case_sensitive === true));
  view.reload = async () => {}; view.applyFilters(); assert.ok(h.tab.filters[0].conds.every(condition => condition.case_sensitive === true)); assert.equal(h.tab.filters[0].conds[1].value, '');
});

test('header ASC/DESC applies immediately, preserves priorities, resets pagination and rejects invalid columns', () => {
  const { view, tab } = harness(); let reloads = 0; view.reload = async () => { reloads++; };
  view.sortRows = [{ column: 'id', ascending: true }, { column: 'name', ascending: true }]; view.applyHeaderSort('id', 'desc');
  assert.equal(reloads, 1); assert.equal(tab.pageOffset, 0); assert.deepEqual(plain(tab.order_rules), [{ column: 'id', ascending: false }, { column: 'name', ascending: true }]);
  assert.equal(view.sortRowsEl.querySelector('.fb-dir').textContent, 'DESC');
  view.applyHeaderSort('missing', 'asc'); assert.equal(reloads, 1);
  view.applyHeaderSort('name', 'remove'); assert.equal(reloads, 2); assert.equal(tab.order_rules.length, 1);
  let menu; view.app.showContextMenu = entries => { menu = entries; }; view.grid.cb.onHeaderContextMenu('id', 0, 5, 10);
  assert.match(menu.find(item => item.id === 'header-sort-asc').label, /ASC/); assert.match(menu.find(item => item.id === 'header-sort-desc').label, /DESC/);
  assert.match(menu.find(item => item.id === 'header-pin-toggle').label, /固定列到最左侧/);
});

test('loading, manual transactions and newly flushed cell drafts block applied filter/sort changes', () => {
  for (const state of ['loading', 'transaction', 'editing']) {
    const { view, tab } = harness(); view.filterLines = [{ conds: [{ col: 'name', op: '=', val: 'x' }] }]; view.sortRows = [{ column: 'team', ascending: false }];
    const beforeSort = plain(view.sortRows);
    if (state === 'loading') view.loading = true;
    if (state === 'transaction') view.transactionOwnedByThisTab = () => true;
    if (state === 'editing') view.grid.flushEditing = () => view.pendingUpdates.set(0, new Map([['name', 'draft']]));
    view.applyHeaderSort('id', 'asc'); view.applySort(); view.applyFilters();
    assert.equal(tab.orderBy, null); assert.equal(tab.filters.length, 0); assert.equal(tab.pageOffset, 4); assert.deepEqual(plain(view.sortRows), beforeSort);
  }
});

test('sort choices exclude duplicates and add control is below the priority list', () => {
  const { view } = harness(); view.focusSort(); view.addSortRow(); view.addSortRow();
  const second = view.sortRowsEl.children[1], field = second.querySelector('select'); assert.deepEqual(field.children.map(option => option.value), ['team', 'name']);
  field.value = 'id'; field.fire('change'); assert.equal(view.sortRows[1].column, 'team');
  assert.match(second.children.find(button => button.title === '上移').innerHTML, /dbn_prior/);
  assert.match(second.children.find(button => button.title === '下移').innerHTML, /dbn_next/);
  const pane = view.sortRowsEl.parentElement; assert.ok(pane.children.indexOf(view.sortRowsEl) < pane.children.findIndex(child => child.children.some(button => button.textContent === '＋ 添加排序字段')));
});

test('table column sidebar works for empty results and flushes hidden editable column drafts by name', () => {
  const { view, tab, document } = harness(); const trigger = document.createElement('button'); view.toggleColumnSidebar(trigger);
  assert.equal(view.columnSidebar.el.hidden, false); assert.equal(trigger.getAttribute('aria-expanded'), 'true'); assert.equal(view.columnToolDisabled(), false);
  assert.equal(view.columnSidebar.el.parentElement.children[0].children[0], view.grid.el);
  assert.equal(view.loadingOverlay.parentElement.children[0], view.grid.el);
  tab.page.rows = [['1', 'red', 'Alice']]; tab.page.total = 1; view.applyData();
  let flushes = 0; view.grid.flushEditing = () => { flushes++; view.pendingUpdates.set(0, new Map([['team', 'draft-red']])); };
  view.toggleColumn('team'); assert.equal(flushes, 1); assert.deepEqual(plain(view.grid.rows), [['1', 'Alice']]);
  assert.equal(view.pendingUpdates.get(0).get('team'), 'draft-red'); view.toggleColumn('team'); assert.equal(view.grid.rows[0][1], 'draft-red');
  view.toggleColumn('id'); view.toggleColumn('team'); view.toggleColumn('name'); assert.deepEqual([...view.hiddenColumns()], ['id', 'team']);
  view.columnSidebar.el.fire('keydown', { key: 'Escape' }); assert.equal(view.columnSidebar.el.hidden, true); assert.equal(trigger.focused, true); assert.equal(trigger.getAttribute('aria-expanded'), 'false');
});

// 同时运行 App 的真实列信息构建成员，避免只检查通知回调而漏掉侧栏字段身份。
function bindColumnInfo({ context, view, tab, columns }) {
  const source = ts.createSourceFile('App.ts', fs.readFileSync(path.join(root, 'src/core/App.ts'), 'utf8'), ts.ScriptTarget.Latest, true);
  const appClass = source.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === 'App');
  const names = ['handleColumnSelected', 'renderTableInfoPanel'];
  const members = names.map(name => appClass.members.find(member => member.name?.getText(source) === name).getText(source)).join('\n');
  context.mapTableStatus = () => [];
  vm.runInContext(ts.transpile(`globalThis.ColumnInfoHost = class { ${members} };`, { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const host = new context.ColumnInfoHost();
  Object.assign(view.app, {
    handleColumnSelected: host.handleColumnSelected, renderTableInfoPanel: host.renderTableInfoPanel,
    views: new Map([[tab.id, view]]), infoRequest: 0, connectionById: () => undefined, connectionRef: () => [],
    tableMetaCache: new Map([[`${tab.connectionId}/${tab.schema}/${tab.object}`, { status: [], schema: { columns, indexes: [], foreign_keys: [] }, ddl: '' }]]),
    infoPanel: { setTableState(state) { this.state = state; } },
  });
}

for (const hidden of ['id', 'name', 'team']) test(`column identity: hide/restore ${hidden} around selected team`, () => {
  const h = harness(), { view, tab, document } = h;
  tab.kind = 'table'; tab.id = 'selected-column-regression';
  tab.page.rows = [['1', 'red', 'Alice']]; tab.page.total = 1; view.applyData();
  document.body.append(view.el); bindColumnInfo(h);
  view.grid.onHeaderClick({ target: view.grid.el.querySelectorAll('th[data-col]')[1] });
  assert.equal(view.app.infoPanel.state.column.name, 'team');
  const checkSelection = expected => {
    const index = view.grid.selectedColumn;
    assert.equal(index, expected === null ? -1 : view.grid.columns.findIndex(column => column.name === expected));
    assert.equal(view.app.infoPanel.state.column?.name ?? null, expected, '列信息立即随真实列身份刷新');
    const highlightedHeaders = view.grid.el.querySelectorAll('th[data-col]').filter(cell => cell.classList.contains('col-selected'));
    const highlightedCells = view.grid.el.querySelectorAll('td[data-col]').filter(cell => cell.classList.contains('col-selected'));
    assert.deepEqual(highlightedHeaders.map(cell => cell.dataset.name), expected === null ? [] : [expected]);
    assert.deepEqual(highlightedCells.map(cell => Number(cell.dataset.col)), expected === null ? [] : [index]);
    view.sortRows = []; view.filterLines = []; view.addSortRow(); view.addFilterLine(); view.addLineCond(0);
    assert.equal(view.sortRows[0].column, expected ?? 'id', '默认排序字段沿用实际选中列');
    assert.deepEqual(plain(view.filterLines[0].conds.map(condition => condition.col)), [expected ?? 'id', expected ?? 'id'], '默认筛选字段身份一致');
  };
  view.toggleColumn(hidden);
  const expected = hidden === 'team' ? null : 'team';
  checkSelection(expected);
  // 使用实际双击、编辑器和 flush 链，验证隐藏列后的草稿只写回对应字段。
  const editName = expected ?? 'name', editIndex = view.grid.columns.findIndex(column => column.name === editName);
  const cell = view.grid.el.querySelectorAll('td[data-col]').find(cell => Number(cell.dataset.col) === editIndex);
  view.grid.onDoubleClick({ target: cell });
  const input = view.grid.el.querySelector('.grid-cell-input');
  assert.ok(document.body.contains(input)); assert.equal(input.value, editName === 'team' ? 'red' : 'Alice');
  input.value = `draft ${editName}`; input.fire('input');
  view.toggleColumn(hidden); // 恢复列时 flush 活输入，然后重映当前的选择（-1 不恢复旧索引）。
  assert.deepEqual(plain([...view.pendingUpdates.get(0)]), [[editName, `draft ${editName}`]]);
  assert.equal(tab.page.rows[0][0], '1'); assert.equal(view.grid.rows[0][h.columns.findIndex(column => column.name === editName)], `draft ${editName}`);
  checkSelection(expected);
  view.grid.dispose();
});

for (const guard of ['composition lifecycle', 'isComposing', 'keyCode 229']) test(`filter IME: ${guard} keeps Enter/Escape connected; explicit Enter runs once`, async () => {
  const { view, tab, api, document, columns } = harness();
  const requests = [];
  api.pageTable = async request => { requests.push(request); return { columns, rows: [], total: 0, primary_keys: [] }; };
  document.body.append(view.el); view.focusFilter(); view.addFilterLine();
  view.filterRowsEl.querySelector('.fb-val').fire('click');
  const input = view.filterRowsEl.querySelector('input.fb-edit');
  input.value = '中文候选'; input.fire('input');
  if (guard === 'composition lifecycle') input.fire('compositionstart');
  const flags = guard === 'isComposing' ? { isComposing: true } : guard === 'keyCode 229' ? { keyCode: 229 } : { isComposing: false };
  for (const key of ['Enter', 'Escape']) {
    let prevented = false;
    input.fire('keydown', { key, ...flags, preventDefault() { prevented = true; } });
    assert.equal(prevented, false, '候选键保持输入法默认行为');
    assert.ok(document.body.contains(input), `${key} 不移除活输入`);
    assert.equal(view.filterLines[0].conds[0].val, ''); assert.equal(tab.filters.length, 0); assert.equal(requests.length, 0);
  }
  input.fire('compositionend'); input.value = '中文';
  input.fire('keydown', { key: 'Enter', isComposing: false, keyCode: 13 });
  input.fire('keydown', { key: 'Enter', isComposing: false }); input.fire('blur');
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(document.body.contains(input), false); assert.equal(requests.length, 1);
  assert.equal(requests[0].filters[0].conds[0].value, '中文'); assert.equal(tab.filters[0].conds[0].value, '中文');
});

test('filter IME: Escape after compositionend cancels without applying the candidate', () => {
  const { view, api, document } = harness(); let requests = 0; api.pageTable = async () => { requests++; };
  document.body.append(view.el); view.focusFilter(); view.addFilterLine();
  view.filterRowsEl.querySelector('.fb-val').fire('click'); const input = view.filterRowsEl.querySelector('input.fb-edit');
  input.fire('compositionstart'); input.value = '取消候选'; input.fire('keydown', { key: 'Escape' });
  assert.ok(document.body.contains(input)); input.fire('compositionend'); input.fire('keydown', { key: 'Escape' });
  assert.equal(document.body.contains(input), false); assert.equal(requests, 0); assert.equal(view.filterLines[0].conds[0].val, '');
});

for (const action of ['sort', 'pagination']) test(`busy editor: delayed ${action} IPC rejects old-page editing and preserves next-page drafts`, async () => {
  const { view, tab, api, document, columns, notices } = harness();
  document.body.append(view.el); tab.pageOffset = 0;
  tab.page.rows = [['1', 'red', 'Alice']]; tab.page.total = 40; view.applyData();
  const oldPage = tab.page, requests = [];
  const reload = view.reload.bind(view);
  let reloading;
  view.reload = (...args) => (reloading = reload(...args));
  api.pageTable = async request => {
    requests.push(request);
    await new Promise(resolve => setTimeout(resolve, 300));
    return { columns, rows: [['2', 'blue', 'Bob']], total: 40, primary_keys: [] };
  };
  if (action === 'sort') { view.sortRows = [{ column: 'team', ascending: false }]; view.applySort(); }
  else view.footer.querySelectorAll('button').find(button => button.title === '下一页').fire('click');
  assert.equal(view.loading, true); assert.equal(requests.length, 1);
  assert.equal(requests[0].offset, action === 'sort' ? 0 : 20);
  const oldCell = view.grid.el.querySelectorAll('td[data-col]')[1];
  view.grid.onDoubleClick({ target: oldCell });
  view.grid.lastFocused = { row: 0, col: 1 };
  view.grid.onKeyDown({ key: 'F2', preventDefault() {} }); view.grid.onKeyDown({ key: 'Enter', preventDefault() {} });
  assert.equal(view.grid.editingActive, false); assert.equal(view.grid.el.querySelector('.grid-cell-input'), null);
  assert.equal(tab.page, oldPage); assert.equal(view.pendingUpdates.size, 0);
  assert.equal(notices.length, 3); assert.ok(notices.every(notice => /载入或提交/.test(notice[1])));
  assert.equal(typeof reloading?.then, 'function');
  await reloading;
  assert.equal(view.loading, false); assert.equal(view.pendingUpdates.size, 0); assert.deepEqual(plain(tab.page.rows), [['2', 'blue', 'Bob']]);
  view.grid.onDoubleClick({ target: view.grid.el.querySelectorAll('td[data-col]')[1] });
  const input = view.grid.el.querySelector('.grid-cell-input');
  assert.ok(document.body.contains(input)); assert.equal(input.value, 'blue');
  input.value = '新页草稿'; input.fire('input');
  // 提交中的第二次编辑请求不能触发 beginEdit/endEdit，原活输入保持连接和值。
  view.committing = true; view.grid.onDoubleClick({ target: view.grid.el.querySelectorAll('td[data-col]')[2] });
  assert.ok(document.body.contains(input)); assert.equal(input.value, '新页草稿'); assert.equal(view.editingCol, 'team');
  view.committing = false;
  // 排序尝试会在发请求前落本地草稿，并因 pending 拒绝请求；不能丢值或写到旧页。
  view.applyHeaderSort('name', 'asc');
  assert.equal(requests.length, 1); assert.deepEqual(plain([...view.pendingUpdates.get(0)]), [['team', '新页草稿']]);
  assert.equal(view.grid.rows[0][1], '新页草稿'); assert.deepEqual(plain(oldPage.rows), [['1', 'red', 'Alice']]);
  view.grid.dispose();
});

function queryTimingFixture(t) {
  const h = harness();
  let now = 0, nextId = 0;
  const timers = new Map(), updates = [];
  h.context.performance = { now: () => now };
  h.context.setInterval = (fn, ms) => { assert.equal(ms, 50); timers.set(++nextId, fn); return nextId; };
  h.context.clearInterval = id => timers.delete(id);
  h.tab.id = 'timed-table';
  h.view.app.updateTableQueryElapsed = (id, text) => updates.push({ id, text });
  h.view.applyData = () => {};
  t.after(() => h.view.destroy());
  return { ...h, timers, updates, advance(ms) { now += ms; [...timers.values()].forEach(fn => fn()); } };
}

test('query timer appears before results, ticks, freezes on success and resets for a new query', async t => {
  const h = queryTimingFixture(t), pending = [];
  h.api.pageTable = () => new Promise(resolve => pending.push(resolve));
  const first = h.view.reload();
  assert.equal(h.view.statusInfo().elapsed, '查询耗时: 0.000s（查询中）');
  assert.equal(h.view.loadingLabelEl.textContent, '正在查询… ');
  assert.equal(h.view.loadingTimerEl.textContent, '0.000 s');
  assert.equal(h.view.loadingTimerEl.attributes['aria-hidden'], 'true');
  const timerNode = h.view.loadingTimerEl;
  assert.equal(h.timers.size, 1);
  h.advance(350);
  assert.equal(h.view.loadingTimerEl, timerNode);
  assert.equal(h.view.loadingTimerEl.textContent, '0.350 s');
  assert.deepEqual(h.updates.at(-1), { id: 'timed-table', text: '查询耗时: 0.350s（查询中）' });
  pending[0]({ columns: h.columns, rows: [], total: 0, primary_keys: [], elapsed_ms: 1 });
  await first;
  assert.equal(h.view.statusInfo().elapsed, '查询耗时: 0.350s');
  assert.equal(h.timers.size, 0, 'backend elapsed_ms must not replace the visible elapsed time');
  h.advance(5000); assert.equal(h.view.statusInfo().elapsed, '查询耗时: 0.350s');
  const second = h.view.reload(); assert.equal(h.view.statusInfo().elapsed, '查询耗时: 0.000s（查询中）');
  assert.equal(h.view.loadingTimerEl.textContent, '0.000 s');
  h.advance(120); pending[1]({ columns: h.columns, rows: [], total: 0, primary_keys: [] }); await second;
  assert.equal(h.view.statusInfo().elapsed, '查询耗时: 0.120s');
});

test('query failure stops timing and pending-edit rejection does not start or reset it', async t => {
  const h = queryTimingFixture(t); let reject;
  h.api.pageTable = () => new Promise((_, fail) => { reject = fail; });
  const query = h.view.reload(); h.advance(700); reject(new Error('query failed')); await query;
  assert.equal(h.view.statusInfo().elapsed, '查询耗时: 0.700s'); assert.equal(h.timers.size, 0);
  h.advance(1000); assert.equal(h.view.statusInfo().elapsed, '查询耗时: 0.700s');
  h.view.pendingUpdates.set(0, new Map([['name', 'draft']])); await h.view.reload();
  assert.equal(h.timers.size, 0); assert.equal(h.view.statusInfo().elapsed, '查询耗时: 0.700s');
});

test('stale responses cannot stop a newer query timer; destroy cancels ticks and late results', async t => {
  const h = queryTimingFixture(t), pending = [];
  h.api.pageTable = () => new Promise(resolve => pending.push(resolve));
  const older = h.view.reload(); h.advance(200); const newer = h.view.reload();
  assert.equal(h.timers.size, 1); h.advance(100);
  pending[0]({ columns: h.columns, rows: [], total: 0, primary_keys: [] }); await older;
  assert.equal(h.timers.size, 1); assert.equal(h.view.statusInfo().elapsed, '查询耗时: 0.100s（查询中）');
  h.advance(400); pending[1]({ columns: h.columns, rows: [], total: 0, primary_keys: [] }); await newer;
  assert.equal(h.view.statusInfo().elapsed, '查询耗时: 0.500s');
  const last = h.view.reload(); h.advance(100); const updatesBeforeClose = h.updates.length;
  h.view.destroy(); assert.equal(h.timers.size, 0);
  pending[2]({ columns: h.columns, rows: [], total: 0, primary_keys: [] }); await last;
  h.advance(1000); assert.equal(h.updates.length, updatesBeforeClose);
});

test('filter summary is a borderless 640px native button with its full grouped expression as hover text', () => {
  const { view, tab } = harness(), commands = [];
  view.app.runCommand = command => commands.push(command);
  tab.filters = [{ conds: [{ column: 'id', operator: '=', value: '129' }, { column: 'id', operator: '=', value: '130', logic: 'OR' }] },
    { conds: [{ column: 'name', operator: 'like', value: 'long condition '.repeat(60) }] }];
  view.renderFooter(tab.page);
  const summary = view.footer.querySelector('.grid-filter-summary');
  assert.equal(summary.tagName, 'button'); assert.equal(summary.type, 'button');
  assert.match(summary.textContent, /\(id=129 OR id=130\) AND name~/);
  assert.equal(summary.title, summary.textContent); assert.ok(summary.title.length > 640);
  assert.ok(summary.attributes['aria-label'].includes(summary.title));
  summary.fire('click'); assert.deepEqual(commands, ['filter']);
  const css = fs.readFileSync(path.join(root, 'src/ui/data-grid-controls.css'), 'utf8');
  assert.match(css, /\.grid-filter-summary\s*\{[^}]*max-width: 640px/);
  assert.match(css, /\.grid-filter-summary\s*\{[^}]*border: 0/);
  assert.match(css, /\.grid-filter-summary\s*\{[^}]*background: transparent/);
  assert.match(css, /\.grid-filter-summary\s*\{[^}]*text-overflow: ellipsis/);
});


// 大表首屏契约：无COUNT、可见状态、未知总数分页、统计与网格草稿隔离。
test('first page paints before automatic metadata-only total count and repeated activation does not duplicate it', async () => {
  const { view, tab, api, columns, flushFrame, frames } = harness();
  tab.page = null; tab.pageOffset = 0;
  const pending = []; api.pageTable = req => new Promise(resolve => pending.push({ req, resolve }));
  const loading = view.reload();
  assert.equal(pending.length, 1); assert.equal(pending[0].req.include_total, false);
  assert.equal(view.loadStatus.hidden, true);
  assert.equal(view.loadingOverlay.hidden, false); assert.equal(view.loadingLabelEl.textContent, '正在查询… '); assert.match(view.loadingTimerEl.textContent, /^\d+\.\d{3} s$/);
  assert.equal(view.loadingOverlay.attributes.role, 'status'); assert.equal(view.el.attributes['aria-busy'], 'true');
  view.activate(); view.activate(); assert.equal(pending.length, 1);
  pending[0].resolve({ columns, rows: Array.from({ length: 20 }, () => ['1', 'red', 'Alice']), total: 21, total_exact: false, has_more: true, primary_keys: [] });
  await loading;
  assert.equal(view.loadStatus.hidden, true); assert.equal(view.loading, false);
  assert.match(view.statusInfo().pageText, /总数统计中/); assert.doesNotMatch(view.statusInfo().pageText, /共 21 条/);
  assert.equal(view.footer.querySelectorAll('button').find(b => b.title === '下一页').disabled, false);
  assert.equal(view.footer.querySelectorAll('button').find(b => b.title.startsWith('最后一页')).disabled, true);
  assert.equal(view.grid.rows.length, 20); assert.equal(pending.length, 1);
  view.activate(); view.activate(); assert.equal(frames.size, 1);
  flushFrame(); assert.equal(pending.length, 1); // 第一帧先绘制，不执行 COUNT。
  flushFrame(); assert.equal(pending.length, 2);
  assert.equal(pending[1].req.include_total, true); assert.equal(pending[1].req.limit, 0); assert.equal(pending[1].req.offset, 0);
  view.activate(); view.activate(); assert.equal(pending.length, 2);
  pending[1].resolve({ columns: [], rows: [], primary_keys: [], total: 1000000, total_exact: true });
  await new Promise(resolve => setImmediate(resolve));
  assert.match(view.statusInfo().pageText, /共 1000000 条/);
  assert.match(view.footer.querySelector('.grid-total-count').textContent, /共 1000000 条/);
  assert.match(view.footer.querySelector('.grid-pager').children.find(n => n.tagName === 'form').children[1].textContent, /50000 页/);
  assert.equal(view.grid.rows.length, 20); assert.equal(frames.size, 0);
  view.destroy();
});

test('unknown totals use has_more rather than inferred page count and recover from an empty deep page', () => {
  const { view, tab, button, pager } = paginationFixture();
  Object.assign(tab.page, { rows: Array.from({ length: 20 }, () => ['1']), total: 21, total_exact: false, has_more: true });
  view.reload = async () => {};
  view.renderFooter(tab.page); button('下一页').fire('click'); assert.equal(tab.pageOffset, 20);
  tab.page.total = 0; tab.page.rows = []; tab.page.has_more = false; tab.pageOffset = 10000;
  view.renderFooter(tab.page);
  assert.equal(button('下一页').disabled, true); assert.equal(button('上一页').disabled, false);
  assert.match(pager().children.find(n => n.tagName === 'form').children[1].textContent, /总数未统计/);
  button('第一页').fire('click'); assert.equal(tab.pageOffset, 0);
  Object.assign(tab.page, { total: 40, total_exact: true, has_more: false }); tab.pageOffset = 20;
  view.renderFooter(tab.page); assert.equal(button('下一页').disabled, true);
});

test('unknown total page input does not clamp to the observed lower bound', () => {
  const { view, tab, pager } = paginationFixture();
  Object.assign(tab.page, { total: 21, total_exact: false, has_more: true });
  let requests = 0; view.reload = async () => { requests++; };
  view.renderFooter(tab.page);
  const input = pager().children.find(n => n.tagName === 'form').children[0];
  input.value = '50'; input.fire('change'); assert.equal(tab.pageOffset, 980); assert.equal(requests, 1);
  input.value = String(Number.MAX_SAFE_INTEGER); input.fire('change'); assert.equal(requests, 1);
});

test('explicit total count sends metadata-only count and never reapplies the grid or discards drafts', async () => {
  const { view, tab, api } = harness();
  Object.assign(tab.page, { total: 21, total_exact: false, has_more: true, rows: [['1', 'red', 'Alice']] });
  view.applyData = () => assert.fail('count must not rerender grid');
  let resolveCount; const requests = [];
  api.pageTable = req => { requests.push(req); return new Promise(resolve => { resolveCount = resolve; }); };
  const original = tab.page;
  const counting = view.countTotal();
  assert.equal(requests[0].include_total, true); assert.equal(requests[0].limit, 0); assert.equal(requests[0].offset, 0);
  assert.equal(view.loading, false); assert.equal(view.countingTotal, true);
  view.pendingUpdates.set(0, new Map([['name', 'draft']]));
  resolveCount({ total: 1000000, total_exact: true, columns: [], rows: [], primary_keys: [] }); await counting;
  assert.equal(tab.page, original); assert.equal(tab.page.total, 1000000); assert.equal(view.pendingUpdates.get(0).get('name'), 'draft');
  assert.equal(view.countingTotal, false); assert.match(view.statusInfo().pageText, /共 1000000 条/);
  view.grid.dispose();
});

test('late count cannot overwrite a newly filtered page', async () => {
  const { view, tab, api, columns } = harness();
  Object.assign(tab.page, { total: 21, total_exact: false, has_more: true });
  let resolveCount; api.pageTable = () => new Promise(resolve => { resolveCount = resolve; });
  const counting = view.countTotal();
  api.pageTable = async () => ({ columns, rows: [], total: 0, total_exact: true, has_more: false, primary_keys: [] });
  tab.filters = [{ conds: [{ column: 'name', operator: '=', value: 'no-match' }] }]; await view.reload();
  resolveCount({ total: 1000000, total_exact: true }); await counting;
  assert.equal(tab.page.total, 0); assert.equal(view.countingTotal, false); view.grid.dispose();
});

test('expired count re-enables statistics on the new unknown-total page; destroy ignores late count', async () => {
  const { view, tab, api, columns } = harness();
  Object.assign(tab.page, { total: 21, total_exact: false, has_more: true });
  let resolveCount; api.pageTable = () => new Promise(resolve => { resolveCount = resolve; });
  const older = view.countTotal();
  api.pageTable = async () => ({ columns, rows: [['2', 'blue', 'Bob']], total: 21, total_exact: false, has_more: true, primary_keys: [] });
  await view.reload();
  assert.equal(view.footer.querySelector('.grid-count-total').disabled, true);
  resolveCount({ total: 1000000, total_exact: true }); await older;
  assert.equal(tab.page.total, 21); assert.equal(view.footer.querySelector('.grid-count-total').disabled, false);
  api.pageTable = () => new Promise(resolve => { resolveCount = resolve; });
  const latest = view.countTotal(); view.destroy();
  view.renderFooter = () => assert.fail('late count must not render a destroyed view');
  resolveCount({ total: 1000000, total_exact: true }); await latest;
  assert.equal(tab.page.total, 21);
});

test('failed initial page exposes an inline error, clears busy state and permits retry', async () => {
  const { view, tab, api } = harness(); tab.page = null;
  api.pageTable = async () => { throw new Error('offline'); }; await view.reload();
  assert.equal(view.loading, false); assert.equal(view.el.attributes['aria-busy'], 'false');
  assert.equal(view.loadStatus.hidden, false); assert.match(view.loadStatus.textContent, /offline/);
  api.pageTable = async () => ({ columns: [], rows: [], total: 0, total_exact: true, has_more: false, primary_keys: [] }); await view.reload();
  assert.equal(view.loadStatus.hidden, true); view.grid.dispose();
});

test('form navigation crosses page boundaries without a count and disables only the unknown last record', async () => {
  const { view, tab, api, columns } = harness(); tab.pageOffset = 0;
  Object.assign(tab.page, { rows: Array.from({ length: 20 }, (_, i) => [String(i), 'red', 'Alice']), total: 21, total_exact: false, has_more: true });
  view.formIndex = 19; let pending;
  api.pageTable = req => new Promise(resolve => { pending = { req, resolve }; });
  const nav = view.buildRecordNav(tab.page);
  assert.equal(nav.children.find(b => b.title.startsWith('最后一条')).disabled, true);
  const next = nav.children.find(b => b.title === '下一条'); assert.equal(next.disabled, false); next.fire('click');
  assert.equal(pending.req.offset, 20); assert.equal(pending.req.include_total, false);
  pending.resolve({ columns, rows: [['20', 'blue', 'Bob']], total: 21, total_exact: true, has_more: false, primary_keys: [] });
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(view.formIndex, 0); assert.equal(tab.pageOffset, 20); assert.equal(view.buildRecordNav(tab.page).children.find(b => b.title === '下一条').disabled, true);
  view.grid.dispose();
});

function unknownPage(columns, total = 21, hasMore = true, rowCount = 20) {
  return { columns, rows: Array.from({ length: rowCount }, (_, i) => [String(i), 'red', 'Alice']), total, total_exact: false, has_more: hasMore, primary_keys: [] };
}

for (const action of ['page', 'sort']) {
  test(`exact snapshot survives ${action} without another count; explicit refresh invalidates it`, async () => {
    const { view, tab, api, columns, flushFrame } = harness(); tab.pageOffset = 0;
    const requests = [];
    api.pageTable = async req => { requests.push(req); return req.include_total ? { total: 100, total_exact: true } : unknownPage(columns); };
    await view.reload(); flushFrame(); flushFrame(); await new Promise(resolve => setImmediate(resolve));
    assert.equal(tab.page.total, 100);
    if (action === 'page') tab.pageOffset = 20; else tab.orderBy = 'name';
    await view.reload(action); flushFrame(); flushFrame();
    assert.equal(tab.page.total, 100); assert.equal(requests.filter(req => req.include_total).length, 1);
    await view.reload(); assert.equal(tab.page.total_exact, false);
    flushFrame(); flushFrame(); await new Promise(resolve => setImmediate(resolve));
    assert.equal(requests.filter(req => req.include_total).length, 2); view.destroy();
  });
}

test('new short-tail evidence wins over an older count from the same filter', async () => {
  const { view, tab, api, columns, flushFrame } = harness(); tab.pageOffset = 0;
  api.pageTable = async () => unknownPage(columns); await view.reload();
  let resolveCount; api.pageTable = () => new Promise(resolve => { resolveCount = resolve; });
  const count = view.countTotal(); tab.pageOffset = 20;
  api.pageTable = async () => ({ ...unknownPage(columns, 21, false, 1), total_exact: true });
  await view.reload('page'); resolveCount({ total: 100, total_exact: true }); await count;
  flushFrame(); flushFrame(); assert.equal(tab.page.total, 21); assert.equal(tab.page.total_exact, true); view.destroy();
});

test('current page evidence rejects a count older than externally inserted rows', async () => {
  const { view, tab, api, columns } = harness(); tab.pageOffset = 0;
  api.pageTable = async () => unknownPage(columns); await view.reload();
  let resolveCount; api.pageTable = () => new Promise(resolve => { resolveCount = resolve; });
  const count = view.countTotal(); tab.pageOffset = 20;
  api.pageTable = async () => unknownPage(columns, 41); await view.reload('page');
  resolveCount({ total: 21, total_exact: true }); await count;
  assert.equal(tab.page.total_exact, false); assert.match(view.totalError, /过期/);
  assert.equal(view.footer.querySelectorAll('button').find(b => b.title === '下一页').disabled, false); view.destroy();
});

test('automatic count failure is inline, never retries itself, and manual retry preserves drafts', async () => {
  const { view, tab, api, columns, flushFrame, frames, notices } = harness(); tab.pageOffset = 0;
  api.pageTable = async () => unknownPage(columns); await view.reload();
  let calls = 0; api.pageTable = async () => { calls++; throw new Error('count offline'); };
  view.pendingUpdates.set(0, new Map([['name', 'draft']]));
  flushFrame(); flushFrame(); await new Promise(resolve => setImmediate(resolve));
  assert.equal(calls, 1); assert.match(view.totalError, /count offline/); assert.equal(notices.length, 0);
  view.activate(); flushFrame(); flushFrame(); assert.equal(calls, 1); assert.equal(frames.size, 0);
  api.pageTable = async () => ({ total: 100, total_exact: true }); await view.countTotal();
  assert.equal(tab.page.total, 100); assert.equal(view.totalError, ''); assert.equal(view.pendingUpdates.get(0).get('name'), 'draft'); view.destroy();
});

test('transaction, busy connection and unknown write block counts; transaction cleanup permits a new count', async () => {
  const { view, tab, api, columns, flushFrame } = harness(); tab.pageOffset = 0;
  api.pageTable = async () => unknownPage(columns); await view.reload();
  let calls = 0, active = true, busy = false;
  view.app.manualTransactionFor = () => ({ active }); view.app.manualTransactionIsBusy = () => busy;
  api.pageTable = async () => { calls++; return { total: 100, total_exact: true }; };
  flushFrame(); flushFrame(); await view.countTotal(); assert.equal(calls, 0);
  active = false; busy = true; await view.countTotal(); assert.equal(calls, 0);
  busy = false; view.mutationEffectsUnknown = 'unconfirmed'; await view.countTotal(); assert.equal(calls, 0);
  view.mutationEffectsUnknown = null; view.clearTransactionState(); view.scheduleTotalCount();
  flushFrame(); flushFrame(); await new Promise(resolve => setImmediate(resolve)); assert.equal(calls, 1); assert.equal(tab.page.total, 100); view.destroy();
});

for (const outcome of ['applied', 'conflict', 'unknown']) {
  test(`real mutation execution invalidates totals and ignores the pre-write count (${outcome})`, async () => {
    const { view, tab, api, columns } = harness(); tab.pageOffset = 0;
    api.pageTable = async () => unknownPage(columns); await view.reload();
    let resolveCount; api.pageTable = () => new Promise(resolve => { resolveCount = resolve; });
    const count = view.countTotal();
    api.previewRowMutation = async () => ({ plan_id: 'plan' });
    api.executeRowMutation = async () => {
      assert.equal(view.totalSnapshot, null); assert.equal(tab.page.total_exact, false);
      return { plan_id: 'plan', status: outcome === 'unknown' ? 'failed' : outcome, transaction: outcome === 'unknown' ? 'unknown' : outcome === 'applied' ? 'committed' : 'rolled_back', affected: outcome === 'applied' ? 1 : 0, verified: outcome === 'applied', mismatches: [] };
    };
    const draft = { connection_id: tab.connectionId, schema: tab.schema, table: tab.object, kind: 'delete' };
    if (outcome === 'applied') await view.runMutation(draft); else await assert.rejects(view.runMutation(draft));
    resolveCount({ total: 100, total_exact: true }); await count;
    assert.equal(tab.page.total_exact, false); assert.equal(view.totalSnapshot, null);
    if (outcome === 'unknown') assert.match(view.totalText(), /待核对/);
    view.destroy();
  });
}

for (const partialFailure of [false, true]) {
  test(`real save schedules a fresh count after leaving committing state (partial failure: ${partialFailure})`, async () => {
    const { view, tab, api, columns, context, flushFrame } = harness(); tab.pageOffset = 0;
    context.toDbValue = value => value === null ? { kind: 'null' } : { kind: 'text', value };
    api.pageTable = async () => unknownPage(columns); await view.reload();
    api.pageTable = async () => ({ total: 100, total_exact: true }); await view.countTotal();
    let executes = 0, counts = 0;
    api.previewRowMutation = async () => ({ plan_id: 'plan' });
    api.executeRowMutation = async () => {
      assert.equal(view.committing, true); assert.equal(tab.page.total_exact, false); executes++;
      const failed = partialFailure && executes === 2;
      return { plan_id: 'plan', status: failed ? 'conflict' : 'applied', transaction: failed ? 'rolled_back' : 'committed', affected: failed ? 0 : 1, verified: !failed, mismatches: [] };
    };
    api.pageTable = async req => {
      if (req.include_total) { assert.equal(view.committing, false); counts++; return { total: 98, total_exact: true }; }
      return unknownPage(columns);
    };
    view.pendingDeletes.add(0); view.pendingDeletes.add(1);
    await view.commitEdits(); assert.equal(view.committing, false); assert.equal(tab.page.total_exact, false);
    assert.equal(view.pendingDeletes.size, partialFailure ? 1 : 0);
    flushFrame(); flushFrame(); await new Promise(resolve => setImmediate(resolve));
    assert.equal(executes, 2); assert.equal(counts, 1); assert.equal(tab.page.total, 98); view.destroy();
  });
}

test('destroy cancels either queued animation frame before any count IPC', async () => {
  for (const afterFirstFrame of [false, true]) {
    const { view, tab, api, columns, flushFrame, frames } = harness(); tab.pageOffset = 0;
    api.pageTable = async () => unknownPage(columns); await view.reload();
    let calls = 0; api.pageTable = async () => { calls++; return { total: 100, total_exact: true }; };
    if (afterFirstFrame) flushFrame(); view.destroy(); flushFrame(); flushFrame();
    assert.equal(calls, 0); assert.equal(frames.size, 0);
  }
});

test('pinning columns reorders only the display mapping, keeps widths/drafts and restores source order on unpin', () => {
  const { view, tab, columns } = harness();
  tab.page.rows = [['1', 'red', 'Alice']]; view.applyData();
  const original = plain(tab.page);
  view.grid.colWidths[2] = 180; view.grid.userSizedCols.add(2);
  view.grid.selectedColumn = 2;
  view.grid.flushEditing = () => view.pendingUpdates.set(0, new Map([['name', 'draft-name']]));
  view.togglePinnedColumn('name');
  assert.deepEqual(plain(view.grid.columns.map(c => c.name)), ['name', 'id', 'team']);
  assert.deepEqual(plain(view.grid.rows), [['draft-name', '1', 'red']]);
  assert.equal(view.grid.selectedColumn, 0); assert.equal(view.grid.colWidths[0], 180);
  assert.equal(view.grid.getOptions().dirtyCells.has('0:0'), true);
  assert.deepEqual(plain(tab.page), original);
  view.togglePinnedColumn('id');
  const headers = view.grid.el.querySelectorAll('th[data-col]');
  assert.equal(headers[0].style.left, '0px'); assert.equal(headers[1].style.left, '180px');
  assert.equal(headers[1].classList.contains('grid-cell-pinned-edge'), true);
  assert.equal(view.grid.buildRow(0).children[0].dataset.col, '0');
  view.toggleColumn('name'); assert.equal(view.grid.columns[0].name, 'id');
  view.toggleColumn('name'); assert.equal(view.grid.columns[0].name, 'name');
  view.togglePinnedColumn('name'); view.togglePinnedColumn('id');
  assert.deepEqual(plain(view.grid.columns.map(c => c.name)), columns.map(c => c.name));
  assert.equal(view.grid.colWidths[2], 180); assert.equal(view.grid.getOptions().dirtyCells.has('0:2'), true);
  assert.equal(view.grid.el.querySelectorAll('.grid-cell-pinned').length, 0);
  view.loading = true; view.togglePinnedColumn('team'); assert.equal(view.pinnedCols.size, 0);
  view.destroy();
});

test('field sidebar loads real metadata without COUNT and ignores metadata after disposal', async () => {
  const { view, api, columns } = harness(); let resolve; const calls = [];
  api.getTableSchema = (...args) => { calls.push(args); return new Promise(yes => { resolve = yes; }); };
  view.toggleColumnSidebar(); const load = view.loadColumnMetadata();
  assert.equal(calls.length, 1); assert.equal(calls[0][3], false);
  resolve({ columns: columns.map(c => ({ ...c, comment: c.name === 'team' ? '所属团队' : null })) }); await load;
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(view.columnComments.get('team'), '所属团队');
  assert.equal(view.columnSidebar.el.querySelector('small').textContent, '所属团队');
  view.columnMetadataLoaded = false;
  const late = view.loadColumnMetadata(); view.destroy();
  view.columnSidebar.render = () => assert.fail('disposed sidebar must not rerender');
  resolve({ columns: columns.map(c => ({ ...c, comment: 'late' })) }); await late;
  assert.equal(view.columnComments.get('team'), '所属团队');
});

test('all successful reload actions hide the centered overlay and never leave a completion notice', async () => {
  for (const action of ['reload', 'filter', 'sort', 'page']) {
    const { view, tab, api, columns } = harness(); let resolve;
    api.pageTable = () => new Promise(yes => { resolve = yes; });
    const task = view.reload(action);
    assert.equal(view.loadingOverlay.hidden, false); assert.equal(view.loadStatus.hidden, true);
    assert.equal(view.grid.el.inert, true); assert.equal(view.columnSidebar.el.inert, true);
    resolve({ columns, rows: [], total: 0, total_exact: true, has_more: false, primary_keys: [] }); await task;
    assert.equal(view.loadingOverlay.hidden, true); assert.equal(view.loadingLabelEl.textContent, ''); assert.equal(view.loadingTimerEl.textContent, '');
    assert.equal(view.loadStatus.hidden, true); assert.equal(view.loadStatus.textContent, '');
    assert.equal(view.grid.el.inert, false); assert.equal(tab.page.rows.length, 0); view.destroy();
  }
});

test('table information waits for first-page data and requests metadata without a hidden row count', async () => {
  const h = harness(), { view, tab, api, columns } = h;
  bindColumnInfo(h); view.app.tableMetaCache.clear(); view.app.activeTab = tab; tab.page = null;
  const calls = [];
  api.tableStatus = async () => { calls.push('status'); return []; };
  api.getTableDdl = async () => { calls.push('ddl'); return 'CREATE TABLE items(id INTEGER)'; };
  api.getTableSchema = async (...args) => { calls.push(['schema', ...args]); return { columns, indexes: [], foreign_keys: [] }; };
  view.app.updateInfoPanelPublic = () => view.app.renderTableInfoPanel(tab);
  view.app.renderTableInfoPanel(tab); assert.deepEqual(calls, []);
  api.pageTable = async req => { calls.push(['page', req.include_total]); return { columns, rows: [], total: 0, total_exact: true, has_more: false, primary_keys: [] }; };
  await view.reload(); await new Promise(resolve => setTimeout(resolve, 0));
  assert.deepEqual(calls, [['page', false], 'status', 'ddl', ['schema', tab.connectionId, tab.schema, tab.object, false]]);
  assert.equal(view.app.tableMetaCache.size, 1); view.destroy();
});
