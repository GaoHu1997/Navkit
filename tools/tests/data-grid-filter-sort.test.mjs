import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
class Element {
  constructor(tag = 'div') {
    this.tagName = tag; this.children = []; this.dataset = {}; this.style = {}; this.attributes = {};
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
  remove() { if (this.parentElement) this.parentElement.children = this.parentElement.children.filter(c => c !== this); }
  setAttribute(k, v) { this.attributes[k] = v; }
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
  const document = { body: new Element('body'), createElement: tag => new Element(tag), createTextNode: text => Object.assign(new Element('#text'), { textContent: text }), createDocumentFragment: () => new Element('fragment') };
  const window = new Element('window');
  const api = { pageTable: async () => ({ columns: [], rows: [], total: 0, primary_keys: [], elapsed_ms: 0 }) };
  const context = vm.createContext({ exports: {}, document, window, api, setTimeout, console, iconSvgByName: () => '', svg: () => '', renderCellPreview() {}, GRID_CONTEXT_MENU: [],
    ICON_PLUS_LINE: {}, ICON_MINUS_LINE: {}, ICON_CHECK: {}, ICON_CROSS: {}, ICON_PAGE_FIRST: {}, ICON_PAGE_PREV: {}, ICON_PAGE_NEXT: {}, ICON_PAGE_LAST: {}, ICON_MORE_DOTS: {}, ICON_COLUMNS_GRID: {}, ICON_VIEW_LIST: {},
    TabView: class { constructor(app, tab) { this.app = app; this.tab = tab; this.el = document.createElement('div'); } },
  });
  const gridSource = fs.readFileSync(path.join(root, 'src/ui/DataGrid.ts'), 'utf8').replace(/^import .*;$/gm, '').replace('export class DataGrid', 'class DataGrid');
  vm.runInContext(ts.transpile(gridSource + '\nglobalThis.DataGrid = DataGrid;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const appSource = fs.readFileSync(path.join(root, 'src/core/App.ts'), 'utf8');
  const viewSource = appSource.slice(appSource.indexOf('class DataTabView extends'), appSource.indexOf('/** 查询视图（运行条'));
  vm.runInContext(ts.transpile(viewSource + '\nglobalThis.DataTabView = DataTabView;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const columns = ['id', 'team', 'name'].map(name => ({ name, type: 'text', nullable: true, primary_key: false, auto_increment: false }));
  const tab = { filters: [], orderBy: null, ascending: true, pageOffset: 4, pageLimit: 20, connectionId: 'test', schema: '', object: 'items', selectedRows: new Set(), page: { columns, rows: [], total: 0, primary_keys: [] } };
  const notices = [];
  const app = { toast: (...args) => notices.push(args), renderStatus() {}, handleColumnSelected() {}, showGridLinesState: () => false, skipNullState: () => false };
  const view = new context.DataTabView(app, tab);
  return { context, document, window, api, tab, view, notices, columns, makeView: () => new context.DataTabView(app, tab) };
}
test('filter/sort toolbar entries do not depend on row count or pending state', () => {
  const source = fs.readFileSync(path.join(root, 'src/core/App.ts'), 'utf8');
  for (const id of ['filter', 'sort']) {
    const entry = source.match(new RegExp('\\{ id: "' + id + '", label:[^}]+\\}'))?.[0];
    assert.ok(entry, `missing ${id} toolbar entry`);
    assert.ok(!entry.includes('disabled:'), `${id} must remain available for zero-result recovery`);
  }
});

const plain = value => JSON.parse(JSON.stringify(value));

test('filter editor: one line = one bracketed group, case sensitivity; empty strings remain conditions', () => {
  const { view, tab } = harness();
  view.reload = async () => {};
  view.focusFilter();
  const sensitive = view.el.querySelector('.fb-case');
  assert.equal(sensitive.tagName, 'input'); assert.equal(sensitive.type, 'checkbox');
  sensitive.checked = true; sensitive.fire('change');
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
  const val = view.filterRowsEl.querySelector('.fb-val');
  assert.equal(val.textContent, '');
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
  assert.equal(op.textContent, '=');
  view.filterLines[0].conds[0].op = 'is null';
  view.renderFilterLines();
  assert.equal(view.filterRowsEl.querySelector('.fb-op-txt').textContent, 'IS NULL');
  assert.equal(view.filterRowsEl.querySelector('.fb-val').style.display, 'none', 'IS NULL 无值输入');
});

test('line and in-line logic survive tab reopen; missing logic falls back', () => {
  const { view, tab, makeView } = harness();
  view.reload = async () => {};
  view.focusFilter();
  view.addLineCond(0);   // 行1 两条
  view.addFilterLine();  // 行2
  view.addLineCond(1);   // 行2 两条
  const joins = view.filterRowsEl.querySelectorAll('.fb-join-text');
  assert.equal(joins.length, 3, '行1内 + 行2行间 + 行2内');
  assert.equal(joins[1].textContent, 'AND');
  joins[1].fire('click');
  assert.equal(joins[1].textContent, 'OR');
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
  view.focusFilter(); view.addFilterLine(); view.addFilterLine();
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
    { conds: [{ column: 'id', operator: '=', value: '3', case_sensitive: false }] },
    { logic: 'AND', conds: [
      { column: 'team', operator: 'like', value: 'b%', case_sensitive: false },
      { column: 'name', operator: 'is null', value: '', case_sensitive: false, logic: 'OR' },
    ] },
    { logic: 'AND', conds: [{ column: 'note', operator: '=', value: 'x', case_sensitive: false }] },
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
  second.children.find(c => c.textContent === '▲').fire('click');
  assert.equal(tab.orderBy, null, '未应用时不得提前改变后端排序状态');
  view.focusSort(); view.focusSort();
  assert.equal(view.sortRows[0].column, 'team', '重新打开不丢失草稿');
  view.applySort();
  assert.deepEqual(plain(tab.order_rules), [{ column: 'team', ascending: false }, { column: 'id', ascending: true }]);
  assert.equal(tab.orderBy, 'team');
  assert.equal(view.sortRowsEl.children[0].children.find(c => c.textContent === '▲').disabled, true);
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
