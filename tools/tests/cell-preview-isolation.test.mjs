import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const source = fs.readFileSync(path.join(root, 'src/core/App.ts'), 'utf8');
const ast = ts.createSourceFile('App.ts', source, ts.ScriptTarget.ES2021, true);
const classes = ast.statements.filter(node => ts.isClassDeclaration(node) && ['App', 'TabView', 'DataTabView', 'QueryTabView'].includes(node.name?.text));
const renderer = ast.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === 'renderCellPreview');
const menuSource = fs.readFileSync(path.join(root, 'src/ui/menu.ts'), 'utf8');
const menuAst = ts.createSourceFile('menu.ts', menuSource, ts.ScriptTarget.ES2021, true);
const menu = menuAst.statements.find(node => ts.isVariableStatement(node) && node.declarationList.declarations.some(declaration => declaration.name.getText(menuAst) === 'TEXT_FORMAT_MENU'));
const production = [menu.getText(menuAst), ...classes.map(node => node.getText(ast)), renderer.getText(ast)].map(text => text.replace(/^export /, '')).join('\n');

// Production preview state, renderers, guards and DataTabView constructor execute below.
// Only unrelated filter/grid/sidebar painting and the app shell are fixtures; no layout/native DB claim.
class Element {
  constructor(tag = 'div') {
    this.tagName = tag; this.children = []; this.style = {}; this.dataset = {}; this.listeners = new Map(); this.className = ''; this._text = '';
    this.classList = {
      add: (...names) => { this.className = [...new Set([...this.className.split(' ').filter(Boolean), ...names])].join(' '); },
      contains: name => this.className.split(' ').includes(name),
      toggle: (name, force) => { const names = new Set(this.className.split(' ').filter(Boolean)); if (force) names.add(name); else names.delete(name); this.className = [...names].join(' '); },
    };
  }
  appendChild(child) { child.parentNode = this; this.children.push(child); return child; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  replaceChildren(...children) { this.children.forEach(child => { child.parentNode = null; }); this.children = []; this._text = ''; this.append(...children); }
  set innerHTML(value) { this.replaceChildren(); this._html = value; }
  get innerHTML() { return this._html ?? ''; }
  set textContent(value) { this.replaceChildren(); this._text = String(value); }
  get textContent() { return this._text + this.children.map(child => child.textContent).join(''); }
  contains(node) { return node === this || this.children.some(child => child.contains(node)); }
  setAttribute(name, value) { (this.attributes ??= {})[name] = value; }
  addEventListener(name, handler) { this.listeners.set(name, [...(this.listeners.get(name) ?? []), handler]); }
  click() { for (const handler of this.listeners.get('click') ?? []) handler({ target: this }); }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); this.parentNode = null; }
  querySelector(selector) { const cls = selector.slice(1); for (const child of this.children) { if (child.className.split(' ').includes(cls)) return child; const nested = child.querySelector(selector); if (nested) return nested; } return null; }
}
function fixture() {
  const listeners = new Map(); let selection = null;
  const document = { createElement: tag => new Element(tag), addEventListener(name, handler) { listeners.set(name, [...(listeners.get(name) ?? []), handler]); }, removeEventListener() {} };
  class Grid {
    constructor() { this.el = document.createElement('div'); this.rows = [['A']]; this.columns = [{ name: 'value' }]; this.focusedCell = { row: 0, col: 0 }; this.editingActive = false; }
    dispose() {}
  }
  const context = vm.createContext({ document, window: { getSelection: () => selection }, TextEncoder, crypto: globalThis.crypto, DataGrid: Grid, ColumnSidebar: class { constructor() { this.el = document.createElement('aside'); } }, console });
  vm.runInContext(ts.transpile(production + '\nObject.assign(globalThis, { App, DataTabView, QueryTabView });', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  // These two painters are unrelated to preview; retain the real constructor and all preview methods/state.
  context.DataTabView.prototype.buildFilterBar = () => document.createElement('div');
  context.DataTabView.prototype.renderBar = () => {};
  const app = Object.create(context.App.prototype);
  Object.assign(app, { views: new Map(), tabs: [], activeTabId: null, renderStatus() {}, notifyToolbar() {}, renderTabs() {}, copyContext: null, copyOwner: null });
  function data(id, kind = 'table') {
    const tab = { id, kind, filters: [], orderBy: null, ascending: true, connectionId: id, schema: 'main', object: 'same_name', page: { columns: [], rows: [], total: 0 } };
    app.tabs.push(tab);
    const view = new context.DataTabView(app, tab); app.views.set(id, view); return view;
  }
  const a = data('table-a'), b = data('table-b'), viewData = data('view-data', 'view');
  const queryTab = { id: 'query', kind: 'query' }; app.tabs.push(queryTab);
  const query = Object.create(context.QueryTabView.prototype);
  Object.assign(query, { app, tab: queryTab, el: document.createElement('div'), grid: new Grid(), cellPreviewEl: document.createElement('div'), resultPanelHost: document.createElement('div'), controlledResult: { hasFocusedCell: () => true, focusedCellValue: () => 'query value' } });
  query.el.appendChild(query.cellPreviewEl); app.views.set(queryTab.id, query);
  const otherTab = { id: 'objects', kind: 'objects' }; app.tabs.push(otherTab); app.views.set(otherTab.id, { el: document.createElement('div') });
  function focus(id) { app.activeTabId = id; const active = app.views.get(id); active?.updateCellPreview?.(); }
  function selectionChanged(value) { selection = value; for (const listener of listeners.get('selectionchange') ?? []) listener(); query.updateCellPreview(); }
  focus(a.tab.id);
  return { app, a, b, viewData, query, focus, selectionChanged, document, data };
}
const hidden = view => assert.equal(view.cellPreviewEl.style.display, 'none');
const body = view => view.cellPreviewEl.querySelector('.cell-preview-body');

test('focusing a table cell does not open preview until that table explicitly enables it', () => {
  const f = fixture();
  assert.equal(f.a.previewFormat, 'text'); assert.equal(f.a.previewPinned, false); hidden(f.a);
  f.app.setTextFormat('hex');
  assert.equal(f.a.previewPinned, true); assert.equal(f.a.cellPreviewEl.style.display, ''); assert.equal(body(f.a).textContent, '41');
  assert.equal(f.b.previewPinned, false); assert.equal(f.app.textFormatValue, 'hex');
});

test('switching to query or other pages cannot inherit or enable the table preview, including focused controlled results', () => {
  const f = fixture(); f.app.setTextFormat('hex');
  for (const id of ['query', 'objects']) {
    f.focus(id); assert.equal(f.app.textPreviewActive(), false); assert.equal(f.app.textFormatValue, 'text');
    f.app.setTextFormat('web'); f.app.closeTextPreview();
    f.query.updateCellPreview(); hidden(f.query); assert.equal(f.a.previewFormat, 'hex'); assert.equal(f.a.previewPinned, true);
  }
  f.focus(f.a.tab.id); assert.equal(f.app.textPreviewActive(), true); assert.equal(body(f.a).textContent, '41');
});

test('different tables and view data own independent switches and formats, even for same object name', () => {
  const f = fixture(); f.app.setTextFormat('hex');
  f.focus(f.b.tab.id); hidden(f.b); assert.equal(f.app.textFormatValue, 'text'); f.app.setTextFormat('text');
  assert.equal(body(f.b).textContent, 'A');
  f.focus(f.viewData.tab.id); hidden(f.viewData); f.app.setTextFormat('image'); assert.equal(body(f.viewData).textContent, '（非图像数据）');
  f.focus(f.a.tab.id); assert.equal(f.app.textFormatValue, 'hex'); assert.equal(body(f.a).textContent, '41');
  f.focus(f.b.tab.id); assert.equal(f.app.textFormatValue, 'text'); assert.equal(f.app.textPreviewActive(), true);
});

test('close button actually hides the current preview despite existing focused cell or selected preview text', () => {
  const f = fixture(); f.app.setTextFormat('text');
  const close = f.a.cellPreviewEl.querySelector('.cell-preview-close');
  f.selectionChanged({ anchorNode: body(f.a), isCollapsed: false, toString: () => 'A' });
  close.click(); hidden(f.a); assert.equal(f.app.textPreviewActive(), false);
  f.selectionChanged(null); hidden(f.a); f.focus(f.b.tab.id); f.focus(f.a.tab.id); hidden(f.a);
});

test('old preview close button closes its original table only, not a newly active table', () => {
  const f = fixture(); f.app.setTextFormat('hex'); const oldClose = f.a.cellPreviewEl.querySelector('.cell-preview-close');
  f.focus(f.b.tab.id); f.app.setTextFormat('text'); oldClose.click();
  assert.equal(f.a.previewPinned, false); hidden(f.a); assert.equal(f.b.previewPinned, true); assert.equal(body(f.b).textContent, 'A');
});

test('global selectionchange cannot repaint hidden table previews with the active table format', () => {
  const f = fixture(); f.app.setTextFormat('hex'); f.focus(f.b.tab.id); f.app.setTextFormat('text');
  f.selectionChanged({ anchorNode: f.a.cellPreviewEl, isCollapsed: false, toString: () => 'old text' });
  hidden(f.a); assert.equal(f.a.previewFormat, 'hex'); assert.equal(body(f.b).textContent, 'A'); hidden(f.query);
  f.selectionChanged(null); f.focus(f.a.tab.id); assert.equal(body(f.a).textContent, '41');
});

test('opening or changing preview only refreshes the owning active table', () => {
  const f = fixture(); const updates = [];
  f.b.updateCellPreview = () => updates.push('b'); f.query.updateCellPreview = () => updates.push('query');
  f.app.setTextFormat('hex'); f.app.closeTextPreview(); assert.deepEqual(updates, []);
});

test('enabled preview still requires selected content and does not fall back to the first cell', () => {
  const f = fixture(); f.a.grid.focusedCell = null; f.app.setTextFormat('hex'); hidden(f.a); assert.equal(f.app.textPreviewActive(), true);
  f.selectionChanged({ anchorNode: f.a.grid.el, isCollapsed: false, toString: () => 'B' }); assert.equal(body(f.a).textContent, '42');
  f.selectionChanged(null); hidden(f.a); assert.equal(f.app.textPreviewActive(), true);
});

test('destroying an owner and reopening it creates a default closed preview, without affecting another table', () => {
  const f = fixture(); f.app.setTextFormat('hex'); f.focus(f.b.tab.id); f.app.setTextFormat('text');
  f.app.removeTabNow(f.a.tab.id);
  const reopened = f.data('table-a'); f.focus(reopened.tab.id); hidden(reopened); assert.equal(reopened.previewPinned, false); assert.equal(reopened.previewFormat, 'text');
  assert.equal(f.b.previewPinned, true);
});
