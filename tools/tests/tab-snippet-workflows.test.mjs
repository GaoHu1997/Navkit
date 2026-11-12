import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
const read = file => fs.readFileSync(new URL('../../' + file, import.meta.url), 'utf8');
const appText = read('src/core/App.ts'), source = ts.createSourceFile('App.ts', appText, ts.ScriptTarget.Latest, true);
const appClass = source.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === 'App');
const members = names => appClass.members.filter(node => names.includes(node.name?.getText(source))).map(node => node.getText(source)).join('\n');
class Element {
  constructor(tag, doc) { this.tagName = tag; this.doc = doc; this.children = []; this.style = {}; this.dataset = {}; this.attributes = {}; this.listeners = {}; this.value = ''; this.className = ''; this.classList = { add: (...xs) => { this.className += ' ' + xs.join(' '); }, contains: x => this.className.split(' ').includes(x) }; }
  get parentElement() { return this.parent; }
  appendChild(child) { child.remove(); this.children.push(child); child.parent = this; return child; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  replaceChildren(...children) { this.children.forEach(child => { child.parent = null; }); this.children = []; this.append(...children); }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter(child => child !== this); this.parent = null; }
  set innerHTML(text) { this.replaceChildren(); this.html = text; }
  get innerHTML() { return this.html ?? ''; }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return this.attributes[name] ?? null; }
  removeAttribute(name) { delete this.attributes[name]; }
  addEventListener(name, action) { (this.listeners[name] ??= []).push(action); }
  fire(name, patch = {}) { for (const action of this.listeners[name] ?? []) action({ target: this, preventDefault() {}, stopPropagation() {}, ...patch }); }
  matches(selector) { if (selector === 'div') return this.tagName === 'div'; if (selector.startsWith('.')) return selector.slice(1).split('.').every(name => this.classList.contains(name)); return false; }
  closest(selector) { return this.matches(selector) ? this : this.parent?.closest(selector); }
  querySelectorAll(selector) { return this.children.flatMap(child => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  focus() { this.doc.activeElement = this; }
  scrollIntoView() {}
}
const flatten = node => [node, ...node.children.flatMap(flatten)];
function fixture() {
  const document = { activeElement: null, createElement: tag => new Element(tag, document) };
  const dialogs = [], stored = new Map(); let storageError = false;
  class Dialog { constructor(options) { this.body = document.createElement('div'); this.el = this.body; this.options = options; options.render(this.body); dialogs.push(this); } }
  class QueryTabView { constructor() { this.inserted = []; this.editor = { insertText: text => this.inserted.push(text) }; } }
  const env = vm.createContext({ exports: {}, document, Dialog, QueryTabView, Date, Math, console,
    localStorage: { getItem: key => stored.get(key) ?? null, setItem: (key, value) => { if (storageError) throw new Error('disk unavailable'); stored.set(key, value); } },
    window: { prompt: () => { throw new Error('原生名称弹窗不允许'); } }, iconSvgByName: () => '', ICON_CROSS: {}, svg: () => '' });
  const run = text => vm.runInContext(ts.transpile(text, { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None }), env);
  run(read('src/core/snippets.ts').replace(/^export /gm, ''));
  run(read('src/ui/TabStrip.ts').replace(/^import .*;$/gm, '').replace(/^export /gm, '') + '\nglobalThis.TabStrip = TabStrip;');
  run('globalThis.App = class { ' + members(['makeTab', 'openObjectsTab', 'openSpecialTab', 'cmdCodeSnippet']) + ' };');
  const app = new env.App();
  Object.assign(app, { connections: [{ id: 'a', name: '连接A' }, { id: 'b', name: '连接B' }], tabs: [], views: new Map(), openDbs: new Set(), openConnections: new Set(), settings: { rows_per_page: 100 }, syncConnIcons() {}, renderTabs() {}, toast() {} });
  return { app, env, document, dialogs, QueryTabView, setStorageError: value => { storageError = value; } };
}
test('备份与表/视图复用每连接对象页，跨库切换销毁旧类型视图且保留查询草稿', () => {
  const h = fixture(), { app } = h;
  app.openObjectsTab('a', 'one', 'table'); const tab = app.tabs[0], id = tab.id; let disposed = 0;
  app.views.set(id, { destroy() { disposed++; } });
  const query = { id: 'query', kind: 'query', dirty: true, sql: 'SELECT draft' }; app.tabs.push(query);
  app.openSpecialTab('backup', 'a', 'one'); assert.equal(app.tabs.length, 2); assert.equal(tab.id, id); assert.equal(tab.kind, 'backup'); assert.equal(disposed, 1);
  app.views.set(id, { destroy() { disposed++; } }); app.openSpecialTab('backup', 'a', 'two'); assert.equal(disposed, 1); assert.equal(tab.schema, 'two');
  app.openObjectsTab('a', 'two', 'view'); assert.equal(disposed, 2); assert.equal(tab.kind, 'objects'); assert.equal(tab.objectType, 'view');
  app.openSpecialTab('backup', 'b', 'other'); assert.equal(app.tabs[1].connectionId, 'b'); assert.equal(app.tabs.at(-1), query); assert.equal(query.sql, 'SELECT draft');
});
test('对象/表/查询/设计 Tab 显示@数据库，不改写业务名称，连接级工具不加虚假库名', () => {
  const h = fixture(), strip = new h.env.TabStrip();
  for (const kind of ['objects', 'backup', 'table', 'query', 'structure', 'viewdesign', 'routinedesign', 'result', 'users', 'datasync', 'schedule']) {
    const tab = { id: 't', kind, title: '同名对象', connectionName: '连接', schema: '库二' }, node = strip.buildTab(tab);
    const title = node.querySelector('.tab-title').textContent;
    assert.equal(title, ['users', 'datasync', 'schedule'].includes(kind) ? '同名对象' : '同名对象@库二'); assert.equal(tab.title, '同名对象'); assert.match(node.title, /连接/);
  }
});
test('代码段列表新增行命名，空名/重名/存储失败保留输入，Enter保存，Escape只取消本行', () => {
  const h = fixture(), tab = { id: 'q', kind: 'query' }, view = new h.QueryTabView();
  h.app.activeTab = tab; h.app.views.set(tab.id, view); h.app.cmdCodeSnippet(); const dlg = h.dialogs[0];
  const nodes = () => flatten(dlg.body), button = name => nodes().find(node => node.tagName === 'button' && node.textContent === name);
  const input = () => nodes().find(node => node.attributes['aria-label'] === '新增代码段名称');
  button('＋ 新增代码段').fire('click'); const name = input(); assert.equal(h.document.activeElement, name); assert.equal(nodes().filter(node => node.className === 'snippet-name-draft').length, 1);
  button('＋ 新增代码段').fire('click'); assert.equal(input(), name); name.fire('keydown', { key: 'Enter' }); assert.equal(name.attributes['aria-invalid'], 'true');
  name.value = 'SELECT 查询'; name.fire('keydown', { key: 'Enter' }); assert.equal(input(), name);
  name.value = '我的 SQL'; h.setStorageError(true); name.fire('keydown', { key: 'Enter' }); assert.equal(input(), name); assert(nodes().some(node => node.textContent?.includes('保存失败')));
  h.setStorageError(false); const sql = nodes().find(node => node.tagName === 'textarea'); sql.value = 'SELECT 99'; sql.fire('input');
  name.fire('keydown', { key: 'Enter', isComposing: true }); assert.equal(input(), name); name.fire('keydown', { key: 'Enter' }); assert.equal(input(), undefined);
  dlg.options.buttons[0].onClick(); assert.equal(view.inserted.at(-1), '\nSELECT 99'); assert(nodes().some(node => node.textContent === '我的 SQL'));
  button('＋ 新增代码段').fire('click'); input().fire('keydown', { key: 'Escape' }); assert.equal(input(), undefined); assert.equal(h.dialogs.length, 1);
});
