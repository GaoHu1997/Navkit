import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { build } from 'esbuild';
export async function moduleAt(entry) {
  const result = await build({ entryPoints: [entry], bundle: true, platform: 'node', format: 'esm', write: false, logLevel: 'silent' });
  return import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
}
export class Element {
  constructor(tag = 'div') {
    this.tagName = tag; this.children = []; this.dataset = {}; this.style = { setProperty(name, value) { this[name] = value; }, removeProperty(name) { delete this[name]; } }; this.attributes = {}; this.listeners = {};
    this.className = ''; this.scrollTop = 0; this.clientHeight = 400; this.value = ''; this.inert = false; this.isConnected = true;
    this.classList = {
      add: (...items) => { this.className = [...new Set([...this.className.split(' ').filter(Boolean), ...items])].join(' '); },
      remove: (...items) => { this.className = this.className.split(' ').filter(x => !items.includes(x)).join(' '); },
      contains: item => this.className.split(' ').includes(item),
      toggle: (item, force) => { const yes = force ?? !this.classList.contains(item); yes ? this.classList.add(item) : this.classList.remove(item); },
    };
  }
  set innerHTML(value) { this.children = []; this._html = value; }
  get innerHTML() { return this._html ?? ''; }
  get firstChild() { return this.children[0] ?? null; }
  appendChild(child) { child.parentElement = this; this.children.push(child); return child; }
  append(...children) { children.forEach(child => this.appendChild(typeof child === 'string' ? Object.assign(new Element('#text'), { textContent: child }) : child)); }
  remove() { this.isConnected = false; if (this.parentElement) this.parentElement.children = this.parentElement.children.filter(child => child !== this); }
  setAttribute(key, value) { this.attributes[key] = value; }
  getAttribute(key) { return this.attributes[key] ?? null; }
  removeAttribute(key) { delete this.attributes[key]; }
  addEventListener(name, fn) { (this.listeners[name] ??= []).push(fn); }
  removeEventListener(name, fn) { this.listeners[name] = (this.listeners[name] ?? []).filter(f => f !== fn); }
  fire(name, props = {}) { for (const fn of this.listeners[name] ?? []) fn({ target: this, preventDefault() {}, stopPropagation() {}, ...props }); }
  matches(selector) {
    const cls = selector.match(/\.([\w-]+)/)?.[1], tag = selector.match(/^([\w]+)/)?.[1], attr = selector.match(/\[data-(\w+)(?:="([^"]*)")?\]/);
    return (!tag || tag === this.tagName) && (!cls || this.classList.contains(cls)) && (!attr || (this.dataset[attr[1]] !== undefined && (attr[2] === undefined || this.dataset[attr[1]] === attr[2])));
  }
  closest(selector) { return this.matches(selector) ? this : this.parentElement?.closest(selector) ?? null; }
  querySelectorAll(selector) { return this.children.flatMap(child => [...(child.matches?.(selector) ? [child] : []), ...(child.querySelectorAll?.(selector) ?? [])]); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  contains(node) { return node === this || this.children.some(child => child.contains?.(node)); }
  getBoundingClientRect() { return { left: 100, right: 200, top: 30, bottom: 430 }; }
  focus() { this.focused = true; }
  blur() { this.fire('blur'); }
  select() {}
}
export function harness(extra = {}) {
  const document = { addEventListener() {}, removeEventListener() {}, body: new Element('body'), activeElement: null, createElement: tag => new Element(tag), createTextNode: text => Object.assign(new Element('#text'), { textContent: text }), createDocumentFragment: () => new Element('fragment') };
  const window = new Element('window'); window.close = () => { window.closed = true; };
  class SqlEditor {
    constructor(options) { this.options = options; this.text = options.value ?? ''; this.el = new Element(); }
    getSql() { return this.text; }
    setSql(text) { this.text = text; this.options?.onChange?.(text); }
    focus() { this.focusCount = (this.focusCount ?? 0) + 1; }
    destroy() { this.disposed = true; }
    hasSelection() { return !!this.selection; }
    getSelection() { return this.selection || this.text; }
    setCompletionData() {}
  }
  const context = vm.createContext({ exports: {}, document, window, HTMLElement: Element, HTMLInputElement: class extends Element {},
    SqlEditor, crypto: globalThis.crypto, structuredClone, TextEncoder, setTimeout, clearTimeout, console,
    iconSvgByName: () => '', svg: () => '', renderCellPreview() {}, GRID_CONTEXT_MENU: [],
    ICON_PLUS_LINE: {}, ICON_MINUS_LINE: {}, ICON_CHECK: {}, ICON_CROSS: {}, ICON_PAGE_FIRST: {}, ICON_PAGE_PREV: {}, ICON_PAGE_NEXT: {}, ICON_PAGE_LAST: {}, ICON_MORE_DOTS: {}, ICON_COLUMNS_GRID: {}, ICON_VIEW_LIST: {},
    ...extra,
  });
  const grid = fs.readFileSync('src/ui/DataGrid.ts', 'utf8').replace(/^import .*;$/gm, '').replace('export class DataGrid', 'class DataGrid');
  vm.runInContext(ts.transpile(grid + '\nglobalThis.DataGrid = DataGrid;', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  const source = fs.readFileSync('src/core/App.ts', 'utf8');
  const ast = ts.createSourceFile('App.ts', source, ts.ScriptTarget.ES2021, true);
  const names = ['App', 'TabView', 'DataTabView', 'QueryTabView', 'SqlDesignTabView', 'StructureTabView', 'ObjectsTabView'];
  const classes = ast.statements.filter(node => ts.isClassDeclaration(node) && names.includes(node.name?.text)).map(node => node.getText(ast).replace(/^export /, '')).join('\n');
  vm.runInContext(ts.transpile(classes + '\nObject.assign(globalThis, {' + names.join(',') + '});', { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None }), context);
  return { context, document, window };
}
export const plain = value => JSON.parse(JSON.stringify(value));
export const tick = () => new Promise(resolve => setTimeout(resolve, 0));
export function deferred() { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
