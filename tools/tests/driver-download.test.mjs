import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

// Compile the real picker functions; no copied UI/download implementation.
const path = new URL('../../src/dialogs/ConnectionPickerDialog.ts', import.meta.url);
const source = fs.readFileSync(path, 'utf8');
const ast = ts.createSourceFile(path.pathname, source, ts.ScriptTarget.ES2021, true);
const functions = ast.statements.filter(ts.isFunctionDeclaration).map(node => node.getText(ast).replace(/^export\s+/, '')).join('\n');
// Load only the actual self-contained action icons and svg renderer, not copied icon stubs.
const iconsPath = new URL('../../src/ui/icons.ts', import.meta.url);
const iconsSource = fs.readFileSync(iconsPath, 'utf8');
const iconsAst = ts.createSourceFile(iconsPath.pathname, iconsSource, ts.ScriptTarget.ES2021, true);
const iconNames = new Set(['ICON_DOWNLOAD', 'ICON_EXTERNAL_LINK', 'ICON_SPINNER', 'ICON_GRID_VIEW', 'ICON_LIST_VIEW']);
const iconNodes = iconsAst.statements.filter(node =>
  ts.isFunctionDeclaration(node) && node.name?.text === 'svg' ||
  ts.isVariableStatement(node) && node.declarationList.declarations.some(declaration => iconNames.has(declaration.name.getText(iconsAst))));
assert.equal(iconNodes.length, iconNames.size + 1, 'Every actual icon binding and svg renderer must be present');
const iconFunctions = iconNodes.map(node => node.getText(iconsAst).replace(/^export\s+/, '')).join('\n');
// Execute the real session cache in each fresh VM, including its preload and subscription logic.
const statusPath = new URL('../../src/core/driverStatus.ts', import.meta.url);
const statusSource = fs.readFileSync(statusPath, 'utf8');
const statusAst = ts.createSourceFile(statusPath.pathname, statusSource, ts.ScriptTarget.ES2021, true);
const statusBindings = statusAst.statements.filter(node => !ts.isImportDeclaration(node))
  .map(node => node.getText(statusAst).replace(/^export\s+/, '')).join('\n');
const compiled = ts.transpileModule(statusBindings + '\n' + iconFunctions + '\n' + functions + '\nglobalThis.openConnectionPicker = openConnectionPicker; globalThis.makeDriverAction = makeDriverAction;', { compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None } }).outputText;
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const settle = () => new Promise(resolve => setImmediate(resolve));
class Element {
  constructor(tag, document) {
    this.tagName = tag; this.document = document; this.children = []; this.parentElement = null;
    this.style = {}; this.dataset = {}; this.attributes = new Map(); this.listeners = new Map(); this.disabled = false; this.isConnected = true; this._class = '';
    this.classList = {
      add: (...values) => { this._class = [...new Set([...this._class.split(' ').filter(Boolean), ...values])].join(' '); },
      remove: (...values) => { this._class = this._class.split(' ').filter(value => !values.includes(value)).join(' '); },
      toggle: (value, force) => { const enabled = force ?? !this._class.split(' ').includes(value); enabled ? this.classList.add(value) : this.classList.remove(value); },
    };
  }
  set className(value) { this._class = value; } get className() { return this._class; }
  set innerHTML(value) {
    this._html = value;
    // A browser blurs a focused descendant when replacement removes it.
    if (this.document.activeElement !== this && this.contains(this.document.activeElement)) this.document.activeElement = null;
    for (const child of this.children) child.parentElement = null;
    this.children = [];
  } get innerHTML() { return this._html; }
  set textContent(value) { this._text = value; } get textContent() { return this._text ?? ''; }
  append(...children) { for (const child of children) { child.parentElement = this; this.children.push(child); } }
  appendChild(child) { this.append(child); return child; }
  setAttribute(name, value) { this.attributes.set(name, value); }
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  remove() {
    if (this.contains(this.document.activeElement)) this.document.activeElement = null;
    if (this.parentElement) this.parentElement.children = this.parentElement.children.filter(child => child !== this);
    this.parentElement = null;
  }
  removeAttribute(name) { this.attributes.delete(name); }
  addEventListener(name, callback) { if (!this.listeners.has(name)) this.listeners.set(name, []); this.listeners.get(name).push(callback); }
  focus() { this.document.activeElement = this; }
  contains(element) { return !!element && (this === element || this.children.some(child => child.contains(element))); }
  matches(selector) {
    if (selector.startsWith('.')) return selector.slice(1).split('.').every(name => this.className.split(' ').includes(name));
    if (selector === 'button[data-select-kind]') return this.tagName === 'button' && !!this.dataset.selectKind;
    return this.tagName === selector;
  }
  querySelectorAll(selector) { const selectors = selector.split(',').map(part => part.trim()); return this.children.flatMap(child => [...(selectors.some(part => child.matches(part)) ? [child] : []), ...child.querySelectorAll(selector)]); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  emit(type) {
    if (this.disabled) return;
    let stopped = false;
    const event = { stopPropagation() { stopped = true; }, preventDefault() {}, target: this };
    for (let node = this; node; node = stopped ? null : node.parentElement) for (const callback of node.listeners.get(type) ?? []) callback(event);
  }
}
const kinds = [
  { id: 'mysql', label: 'MySQL', category: 'relational', planned: false, note: '' },
  { id: 'duckdb', label: 'DuckDB', category: 'relational', protocol: 'odbc', planned: false, note: '' },
  { id: 'h2', label: 'H2', category: 'relational', protocol: 'native', planned: false, note: '' },
  { id: 'dameng', label: '达梦', category: 'relational', protocol: 'odbc', planned: false, note: '需授权' },
  { id: 'future', label: 'Future', category: 'relational', planned: true, note: '规划中' },
];
function state(kind, status, can_download = false, message = status) { return { kind, status, can_download, architecture: 'x86_64', message, library: null, download_path: null }; }
const initial = () => [state('mysql', 'builtin'), state('duckdb', 'missing', true), state('dameng', 'manual', false, '需厂商授权；没有公开直链'), state('future', 'unsupported')];
function harness(defaultKind = 'mysql') {
  const document = { activeElement: null, createElement: tag => new Element(tag, document) };
  let dlg;
  class Dialog {
    constructor(options) {
      this.options = options; this.body = document.createElement('div'); this.footer = document.createElement('div'); this.closeCallbacks = []; dlg = this;
      // The production constructor renders synchronously before building footer buttons.
      options.render(this.body, this);
      for (const option of options.buttons ?? []) {
        const button = document.createElement('button'); button.className = option.primary ? 'btn primary' : 'btn'; button.textContent = option.label;
        button.addEventListener('click', () => { const result = option.onClick?.(this); if (result !== false) this.close(); }); this.footer.append(button);
      }
    }
    onClose(callback) { this.closeCallbacks.push(callback); }
    close() { for (const callback of this.closeCallbacks) callback(); }
  }
  const loads = [deferred()]; const downloads = []; const picks = [];
  const driverApi = {
    statuses() { const request = loads.at(-1); return request.promise; },
    list() { return Promise.resolve([]); },
    download(kind) { const request = deferred(); downloads.push({ kind, ...request }); return request.promise; },
  };
  const context = vm.createContext({ document, Dialog, driverApi, DB_KINDS: kinds, DB_CATEGORIES: [{ id: 'relational', label: '关系数据库' }],
    kindLabel: id => kinds.find(kind => kind.id === id)?.label ?? id, kindIcon: id => id, iconSvgByName: () => '',
    prepareConnectionDialog() {}, openDriverManager() {},
  });
  vm.runInContext(compiled, context, { filename: path.pathname });
  context.openConnectionPicker(kind => picks.push(kind), defaultKind);
  const button = kind => dlg.body.querySelectorAll('button').find(button => button.dataset.driverKind === kind);
  const select = kind => dlg.body.querySelectorAll('button').find(button => button.dataset.selectKind === kind);
  const byClass = className => dlg.body.querySelectorAll('.' + className)[0];
  return { context, document, dlg, loads, downloads, picks, button, select, byClass, nextButton: () => dlg.footer.querySelectorAll('.btn.primary')[0], nextLoad() { const request = deferred(); loads.push(request); return request; } };
}
async function ready() { const h = harness(); h.loads[0].resolve(initial()); await settle(); return h; }

test('unknown driver state is disabled, never falsely ready', () => {
  const h = harness(); assert.equal(h.button('duckdb').tagName, 'button'); assert.equal(h.button('duckdb').disabled, true);
  assert.equal(h.button('duckdb').dataset.mode, 'busy'); assert.match(h.button('duckdb').attributes.get('aria-label'), /检查当前架构的完整本地驱动/); assert.match(h.button('duckdb').innerHTML, /<svg[\s>]/);
});
test('synchronous Dialog render uses the initialized constructor instance and preserves native builtin access', () => {
  const h = harness(); assert.equal(h.nextButton().disabled, false); h.nextButton().emit('click'); assert.deepEqual(h.picks, ['mysql']);
});
test('unknown or failed ODBC/H2 detection cannot bypass the initial footer or double click', async () => {
  for (const kind of ['duckdb', 'h2']) {
    const h = harness(kind); assert.equal(h.nextButton().disabled, true); assert.match(h.nextButton().title, /尚未确认可用/);
    h.select(kind).emit('dblclick'); h.nextButton().emit('click'); assert.deepEqual(h.picks, []);
    h.loads[0].reject(new Error('IPC unavailable')); await settle(); assert.equal(h.nextButton().disabled, true); assert.deepEqual(h.picks, []);
  }
});
test('confirmed missing and manual drivers cannot bypass next while builtin selection restores it', async () => {
  const h = await ready();
  for (const kind of ['duckdb', 'dameng']) { h.select(kind).emit('click'); assert.equal(h.nextButton().disabled, true); h.nextButton().emit('click'); h.select(kind).emit('dblclick'); assert.deepEqual(h.picks, []); }
  h.select('mysql').emit('click'); assert.equal(h.nextButton().disabled, false); h.nextButton().emit('click'); assert.deepEqual(h.picks, ['mysql']);
});
test('supported missing drivers have native original-tag actions, builtin and planned do not', async () => {
  const h = await ready(); const button = h.button('duckdb');
  assert.equal(button.type, 'button'); assert.ok(button.className.includes('conn-kind-tag')); assert.equal(button.dataset.mode, 'download'); assert.match(button.attributes.get('aria-label'), /从官方源直接下载并校验完整性/); assert.match(button.innerHTML, /<svg[\s>]/);
  assert.equal(h.button('mysql'), undefined); assert.equal(h.button('future'), undefined);
  assert.equal(h.select('future').disabled, true);
});
test('selection and download buttons are siblings: no nested button in grid or list', async () => {
  const h = await ready();
  for (const layout of ['grid', 'list']) {
    if (layout === 'list') h.dlg.body.querySelectorAll('button').find(button => button.title === '列表视图').emit('click');
    const action = h.button('duckdb'); assert.equal(action.parentElement.tagName, 'div'); assert.equal(action.parentElement, h.select('duckdb').parentElement);
    assert.equal(action.parentElement.parentElement.querySelectorAll('button').filter(button => button.querySelectorAll('button').length).length, 0);
  }
});
test('download click and double click neither select nor open a connection; directly calls IPC', async () => {
  const h = await ready(); const action = h.button('duckdb'); action.emit('click'); action.emit('dblclick');
  assert.deepEqual(h.downloads.map(request => request.kind), ['duckdb']); assert.deepEqual(h.picks, []);
  assert.equal(h.select('mysql').attributes.get('aria-pressed'), 'true');
  assert.equal(h.select('duckdb').attributes.get('aria-pressed'), 'false');
  assert.equal(h.button('duckdb').dataset.mode, 'busy'); assert.equal(h.button('duckdb').attributes.get('aria-disabled'), 'true'); assert.match(h.button('duckdb').attributes.get('aria-label'), /正在下载并校验官方驱动/);
  h.button('duckdb').emit('click'); assert.equal(h.downloads.length, 1);
});
test('manual action explains authorization honestly and never downloads or opens a webpage', async () => {
  const h = await ready(); assert.equal(h.button('dameng').dataset.mode, 'manual'); assert.match(h.button('dameng').attributes.get('aria-label'), /获取驱动：无法自动下载.*需厂商授权/); h.button('dameng').emit('click');
  assert.match(h.byClass('conn-driver-feedback').textContent, /需厂商授权/); assert.equal(h.downloads.length, 0); assert.equal(h.picks.length, 0);
});
test('successful download rechecks complete status and restores keyboard focus to selection', async () => {
  const h = await ready(); const originalSelect = h.select('duckdb'); const originalAction = h.button('duckdb');
  originalAction.focus(); originalAction.emit('click');
  assert.equal(h.document.activeElement, originalAction, 'Busy refresh keeps the focused original trigger attached');
  assert.equal(h.select('duckdb'), originalSelect);
  assert.equal(h.document.activeElement.dataset.driverKind, 'duckdb');
  const recheck = h.nextLoad(); h.downloads[0].resolve({ ...state('duckdb', 'ready', false, '官方完整驱动验证通过'), download_path: 'data/managed-drivers/duckdb' });
  await settle(); recheck.resolve(initial().map(entry => entry.kind === 'duckdb' ? state('duckdb', 'ready') : entry)); await settle();
  assert.equal(h.button('duckdb'), undefined); assert.equal(h.document.activeElement, originalSelect);
  assert.equal(h.select('duckdb'), originalSelect, 'Completion and subscription refresh never detach the selection button');
  assert.match(h.byClass('conn-driver-feedback').textContent, /官方完整驱动验证通过/);
});
test('mouse activation establishes focus before replacing a download button and background refresh retains it', async () => {
  const h = await ready();
  assert.equal(h.document.activeElement, null);
  h.button('duckdb').emit('click');
  assert.equal(h.document.activeElement, h.button('duckdb'));
  const recheck = h.nextLoad();
  const statuses = initial().map(entry => entry.kind === 'duckdb' ? state('duckdb', 'ready') : entry);
  h.downloads[0].resolve(state('duckdb', 'ready')); await settle();
  assert.equal(h.document.activeElement, h.select('duckdb'));
  recheck.resolve(statuses); await settle();
  assert.equal(h.document.activeElement, h.select('duckdb'));
});
test('download completion and background refresh preserve focus explicitly moved to search', async () => {
  const h = await ready(); h.button('duckdb').emit('click');
  const search = h.byClass('conn-picker-search'); search.focus();
  const recheck = h.nextLoad();
  h.downloads[0].resolve(state('duckdb', 'ready')); await settle();
  assert.equal(h.document.activeElement, search);
  recheck.resolve(initial().map(entry => entry.kind === 'duckdb' ? state('duckdb', 'ready') : entry)); await settle();
  assert.equal(h.document.activeElement, search);
});
test('recommended driver status is not overwritten by a missing historical version', async () => {
  const h = harness('h2');
  h.loads[0].resolve([state('h2', 'ready'), state('h2', 'missing', true)]); await settle();
  assert.equal(h.button('h2'), undefined); assert.equal(h.nextButton().disabled, false);
});
test('a ready historical version cannot bypass the missing default driver', async () => {
  const h = harness('h2');
  h.loads[0].resolve([state('h2', 'missing', true), state('h2', 'ready')]); await settle();
  assert.equal(h.button('h2').dataset.mode, 'download'); assert.equal(h.nextButton().disabled, true);
  h.select('h2').emit('dblclick'); assert.deepEqual(h.picks, []);
});
test('download failure stays visible, preserves focus and permits retry without fake success', async () => {
  const h = await ready(); h.button('duckdb').focus(); h.button('duckdb').emit('click'); h.downloads[0].reject(new Error('SHA-256 mismatch')); await settle();
  assert.match(h.byClass('conn-driver-feedback').textContent, /SHA-256 mismatch/); assert.equal(h.button('duckdb').dataset.mode, 'download'); assert.match(h.button('duckdb').attributes.get('aria-label'), /从官方源直接下载并校验完整性/);
  assert.equal(h.document.activeElement, h.button('duckdb')); h.button('duckdb').emit('click'); assert.equal(h.downloads.length, 2);
});
test('a late completion after closing cannot overwrite dialog feedback or pick a kind', async () => {
  const h = await ready(); h.button('duckdb').emit('click'); const feedback = h.byClass('conn-driver-feedback'); const before = feedback.textContent;
  h.dlg.close(); h.downloads[0].resolve(state('duckdb', 'ready', false, 'late result')); await settle(); assert.equal(feedback.textContent, before); assert.equal(h.picks.length, 0);
});
test('driver detection failure is visible and never grants availability', async () => {
  const h = harness(); h.loads[0].reject(new Error('IPC unavailable')); await settle();
  assert.match(h.byClass('conn-driver-feedback').textContent, /IPC unavailable/); assert.equal(h.button('duckdb').disabled, true);
});
test('selection buttons retain keyboard semantics and live feedback has accessible status', async () => {
  const h = await ready(); h.select('duckdb').focus(); h.select('duckdb').emit('click');
  assert.equal(h.select('duckdb').attributes.get('aria-pressed'), 'true'); assert.equal(h.select('mysql').attributes.get('aria-pressed'), 'false');
  assert.equal(h.byClass('conn-driver-feedback').attributes.get('role'), 'status'); assert.equal(h.byClass('conn-driver-feedback').attributes.get('aria-live'), 'polite');
  assert.match(h.button('duckdb').attributes.get('aria-label'), /^DuckDB：下载驱动：从官方源直接下载并校验完整性$/); assert.match(h.button('duckdb').innerHTML, /aria-hidden="true"/);
});
