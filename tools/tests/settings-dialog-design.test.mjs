// 使用现有 esbuild + VM 加载方式，执行真实设置对话框与同步组件；不访问 IPC 或网络。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { webcrypto } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import { build, transform } from 'esbuild';

const root = fileURLToPath(new URL('../..', import.meta.url));
const built = await build({
  stdin: {
    contents: 'export { openSettingsDialog } from "./src/dialogs/SettingsDialog.ts"; export { createDatasourceSyncTab } from "./src/dialogs/DatasourceSyncTab.ts";',
    resolveDir: root, loader: 'ts',
  },
  bundle: true, write: false, platform: 'node', format: 'cjs', loader: { '.css': 'empty' }, logLevel: 'silent',
});

// 最小 DOM 仅模拟本对话框所用的原生节点、值与焦点；Dialog 和可访问辅助函数均来自产品源码。
class Element {
  constructor(tag = 'div') {
    this.tagName = tag.toUpperCase(); this.children = []; this.parentNode = null;
    this.style = {}; this.dataset = {}; this.attributes = new Map(); this.listeners = new Map();
    this.className = ''; this.id = ''; this._value = ''; this.checked = false;
    this.disabled = false; this.hidden = false; this.tabIndex = -1; this.scrollTop = 0;
    this._text = ''; this._html = '';
    this.classList = {
      contains: name => this.className.split(/\s+/).includes(name),
      add: (...names) => { this.className = [...new Set([...this.className.split(/\s+/).filter(Boolean), ...names])].join(' '); },
      remove: (...names) => { this.className = this.className.split(/\s+/).filter(name => !names.includes(name)).join(' '); },
      toggle: (name, force) => { const yes = force ?? !this.classList.contains(name); yes ? this.classList.add(name) : this.classList.remove(name); return yes; },
    };
  }
  get value() {
    if (this.tagName !== 'SELECT') return this._value;
    return this.children.find(option => option.selected)?.value ?? (this.explicitSelection ? '' : this.children[0]?.value ?? '');
  }
  set value(value) {
    this._value = String(value);
    if (this.tagName === 'SELECT') {
      this.explicitSelection = true;
      for (const option of this.children) option.selected = option.value === this._value;
    }
  }
  set textContent(value) { this._text = String(value); this._html = ''; this.children = []; }
  get textContent() { return this._text + this._html.replace(/<[^>]*>/g, '') + this.children.map(child => child.textContent).join(''); }
  set innerHTML(value) { this._html = String(value); this._text = ''; this.children = []; }
  get innerHTML() { return this._html; }
  appendChild(child) { child.remove(); this.children.push(child); child.parentNode = this; return child; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  insertBefore(child, before) {
    child.remove(); const index = this.children.indexOf(before);
    this.children.splice(index < 0 ? this.children.length : index, 0, child); child.parentNode = this;
  }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); this.parentNode = null; }
  get firstChild() { return this.children[0] ?? null; }
  get isConnected() { return this === this.ownerDocument?.body || Boolean(this.parentNode?.isConnected); }
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  getAttribute(name) {
    if (this.attributes.has(name)) return this.attributes.get(name);
    if (name.startsWith('data-')) return this.dataset[name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] ?? null;
    if (name === 'disabled' || name === 'hidden') return this[name] ? '' : null;
    if (name === 'id') return this.id || null;
    if (name === 'tabindex') return String(this.tabIndex);
    if (name === 'type') return this.type ?? null;
    return null;
  }
  removeAttribute(name) { this.attributes.delete(name); }
  addEventListener(name, handler) {
    const handlers = this.listeners.get(name) ?? []; handlers.push(handler); this.listeners.set(name, handlers);
  }
  async emit(name, detail = {}) {
    const event = { target: this, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...detail };
    for (const handler of this.listeners.get(name) ?? []) await handler(event);
    return event;
  }
  getClientRects() {
    for (let node = this; node; node = node.parentNode) if (node.hidden || node.style.display === 'none') return [];
    return this.isConnected ? [{}] : [];
  }
  focus() { if (this.getClientRects().length) this.ownerDocument.activeElement = this; }
  matches(selector) {
    return selector.split(',').some(part => {
      let candidate = part.trim();
      const excluded = [...candidate.matchAll(/:not\(([^)]+)\)/g)].map(match => match[1]);
      if (excluded.some(item => this.matches(item))) return false;
      candidate = candidate.replace(/:not\([^)]+\)/g, '');
      if (candidate.includes(':disabled') && !this.disabled) return false;
      candidate = candidate.replace(/:disabled/g, '');
      const tag = candidate.match(/^[\w-]+/); if (tag && this.tagName !== tag[0].toUpperCase()) return false;
      if ([...candidate.matchAll(/\.([\w-]+)/g)].some(match => !this.classList.contains(match[1]))) return false;
      const id = candidate.match(/#([\w-]+)/); if (id && this.id !== id[1]) return false;
      for (const match of candidate.matchAll(/\[([\w-]+)(?:=['"]?([^'"\]]+)['"]?)?\]/g)) {
        const value = this.getAttribute(match[1]);
        if (value === null || match[2] !== undefined && value !== match[2]) return false;
      }
      return true;
    });
  }
  querySelectorAll(selector) {
    const result = [];
    const visit = element => { for (const child of element.children) { if (child.matches(selector)) result.push(child); visit(child); } };
    visit(this); return result;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}

const settings = () => ({
  font_family: 'Microsoft YaHei UI', font_size: 12, grid_font_size: 12,
  rows_per_page: 1000, query_timeout_secs: 30, theme: 'light', max_recent: 10,
  auto_complete: true, confirm_on_close: true, show_system_tray: true,
  sql_font_family: 'Consolas', sql_font_size: 13.6, preserved_extra: '保留其他配置',
});
const config = (overrides = {}) => ({
  url: 'https://example.test/dav', user: 'navkit', auto_every: -1, remember_password: true,
  password_set: true, passphrase_set: true, headers_set: true, headers: 'Authorization: Bearer saved-demo\nX-Client: NavKit', last_sync: null, ...overrides,
});
const plain = value => JSON.parse(JSON.stringify(value));
function syncFixture(overrides = {}) {
  const calls = [];
  let savedConfig = config();
  const sync = {
    pause() { calls.push(['pause']); return () => calls.push(['release']); },
    async getConfig() { calls.push(['read']); return savedConfig; },
    async save(input) { calls.push(['sync-save', plain(input)]); return savedConfig = config({ ...savedConfig, ...input }); },
    async cancel() { calls.push(['cancel']); },
    ...overrides,
  };
  return { sync, calls };
}
function harness() {
  const document = { activeElement: null, createElement(tag) { const element = new Element(tag); element.ownerDocument = document; return element; } };
  document.body = document.createElement('body'); document.activeElement = document.body;
  const microtasks = [], module = { exports: {} };
  const context = vm.createContext({
    module, exports: module.exports, document, HTMLElement: Element, crypto: webcrypto, Error,
    queueMicrotask: callback => microtasks.push(callback),
    window: { innerWidth: 1280, innerHeight: 900, addEventListener() {}, removeEventListener() {} },
  });
  vm.runInContext(built.outputFiles[0].text, context, { filename: 'settings-dialog-production.cjs' });
  const settle = async () => {
    while (microtasks.length) microtasks.shift()();
    for (let turn = 0; turn < 4; turn++) await Promise.resolve();
  };
  return { ...module.exports, document, settle };
}
const control = (dialog, key) => dialog.body.querySelector(`[data-settings-field="${key}"]`);
const tab = (dialog, key) => dialog.body.querySelector(`[data-settings-tab="${key}"]`);
const confirm = dialog => dialog.footer.children.find(button => button.classList.contains('primary'));
const activePanel = dialog => dialog.body.querySelectorAll('[data-settings-panel]').filter(panel => !panel.hidden);
const change = async (element, value, event = 'input') => { element.value = value; await element.emit(event); };

test('四页保留全部 19 字段，每个控件关联标签与说明，初始值来自原配置', async () => {
  const h = harness(), f = syncFixture(), current = settings();
  const dialog = h.openSettingsDialog(current, () => {}, f.sync);
  await h.settle();
  const expected = [
    'rows_per_page', 'query_timeout_secs', 'connect_timeout_secs', 'pool_acquire_timeout_secs', 'sqlite_busy_timeout_secs', 'max_recent', 'auto_complete', 'confirm_on_close',
    'font_family', 'font_size', 'grid_font_size', 'sql_font_family', 'sql_font_size',
    'sync_url', 'sync_user', 'sync_password', 'sync_passphrase', 'sync_headers', 'sync_auto_every',
  ];
  assert.deepEqual(dialog.body.querySelectorAll('[data-settings-field]').map(input => input.dataset.settingsField), expected);
  assert.equal(dialog.body.querySelectorAll('.settings-field').length, 19);
  assert.deepEqual(dialog.body.querySelectorAll('[data-settings-tab]').map(button => button.textContent), ['常规', '字体', 'SQL 编辑器', 'WebDAV']);
  for (const key of expected) {
    const input = control(dialog, key);
    assert.ok(input.id);
    const label = dialog.body.querySelectorAll('label').find(item => item.htmlFor === input.id);
    assert.ok(label?.textContent.trim(), key + ' 缺少关联标签');
    const help = dialog.body.querySelector('#' + input.getAttribute('aria-describedby'));
    assert.ok(help?.textContent.trim(), key + ' 缺少关联说明');
  }
  assert.equal(new Set(expected.map(key => control(dialog, key).id)).size, 19);
  for (const key of expected.slice(0, 13)) {
    const input = control(dialog, key);
    assert.equal(input.type === 'checkbox' ? input.checked : input.value, typeof current[key] === 'boolean' ? current[key] : String(current[key] ?? ({ connect_timeout_secs: 30, pool_acquire_timeout_secs: 30, sqlite_busy_timeout_secs: 10 })[key]));
  }
  assert.equal(control(dialog, 'sync_url').required, true);
  assert.equal(dialog.el.getAttribute('role'), 'dialog');
  assert.equal(dialog.el.getAttribute('aria-modal'), 'true');
  dialog.close(); await h.settle();
});

test('无同步实例时仅有三页，WebDAV 初始页回退常规；有实例时尊重 initialTab', async () => {
  const h = harness();
  const dialog = h.openSettingsDialog(settings(), () => {}, undefined, 'sync');
  await h.settle();
  assert.equal(dialog.body.querySelectorAll('[data-settings-tab]').length, 3);
  assert.equal(dialog.body.querySelector('.settings-sync-panel'), null);
  assert.equal(h.document.activeElement, tab(dialog, 'general'));
  assert.equal(activePanel(dialog)[0].dataset.settingsPanel, 'general');
  dialog.close();
  const f = syncFixture();
  const syncDialog = h.openSettingsDialog(settings(), () => {}, f.sync, 'sync');
  await h.settle();
  assert.equal(h.document.activeElement, tab(syncDialog, 'sync'));
  assert.equal(activePanel(syncDialog)[0].dataset.settingsPanel, 'sync');
  syncDialog.close(); await h.settle();
});

test('侧导航使用原生按钮，方向键和 Home/End 同步选择、焦点、ARIA 与滚动位置', async () => {
  const h = harness(), dialog = h.openSettingsDialog(settings(), () => {});
  await h.settle();
  for (const button of dialog.body.querySelectorAll('[data-settings-tab]')) {
    assert.equal(button.tagName, 'BUTTON'); assert.equal(button.type, 'button');
    const panel = dialog.body.querySelector('#' + button.getAttribute('aria-controls'));
    assert.equal(panel.getAttribute('aria-labelledby'), button.id);
  }
  const actions = [['general', 'ArrowDown', 'font'], ['font', 'End', 'sql'], ['sql', 'ArrowRight', 'general'], ['general', 'ArrowUp', 'sql'], ['sql', 'Home', 'general']];
  for (const [from, key, to] of actions) {
    dialog.body.querySelector('.settings-content').scrollTop = 100;
    const event = await tab(dialog, from).emit('keydown', { key });
    assert.equal(event.defaultPrevented, true);
    assert.equal(h.document.activeElement, tab(dialog, to));
    assert.equal(activePanel(dialog).length, 1);
    assert.equal(activePanel(dialog)[0].dataset.settingsPanel, to);
    for (const button of dialog.body.querySelectorAll('[data-settings-tab]')) {
      assert.equal(button.getAttribute('aria-selected'), String(button === tab(dialog, to)));
      assert.equal(button.tabIndex, button === tab(dialog, to) ? 0 : -1);
    }
    assert.equal(dialog.body.querySelector('.settings-content').scrollTop, 0);
  }
  await tab(dialog, 'font').emit('click');
  assert.equal(activePanel(dialog)[0].dataset.settingsPanel, 'font');
  dialog.close();
});

test('字体和 SQL 样例即时响应，保持只读，并且预览不保存配置', async () => {
  const h = harness(), saved = [], dialog = h.openSettingsDialog(settings(), value => saved.push(value));
  const sample = key => dialog.body.querySelector(`[data-settings-preview="${key}"]`);
  await change(control(dialog, 'font_family'), 'Consolas', 'change');
  await change(control(dialog, 'font_size'), '14');
  await change(control(dialog, 'grid_font_size'), '15.5');
  await change(control(dialog, 'sql_font_family'), 'Courier New', 'change');
  await change(control(dialog, 'sql_font_size'), '16.5');
  assert.equal(sample('interface').style.fontFamily, 'Consolas');
  assert.equal(sample('interface').style.fontSize, '14px');
  assert.equal(sample('grid').style.fontFamily, 'Consolas');
  assert.equal(sample('grid').style.fontSize, '15.5px');
  assert.equal(sample('sql').style.fontFamily, 'Courier New');
  assert.equal(sample('sql').style.fontSize, '16.5px');
  for (const key of ['interface', 'grid', 'sql']) assert.equal(sample(key).querySelectorAll('input, select, textarea').length, 0);
  assert.match(sample('sql').textContent, /SELECT.*created_at\nFROM t_user/);
  assert.equal(saved.length, 0);
  dialog.close(); await h.settle();
});

test('同步保存完成后才保存全部设置，原默认值、开关和额外配置保留', async () => {
  const h = harness(), calls = [], current = settings();
  let resolveSave;
  const gate = new Promise(resolve => { resolveSave = resolve; });
  const f = syncFixture({ async save(input) { calls.push(['sync-start', plain(input)]); await gate; calls.push(['sync-finish']); return config(input); } });
  const dialog = h.openSettingsDialog(current, value => calls.push(['settings', plain(value)]), f.sync);
  await h.settle();
  const edits = { rows_per_page: '250', query_timeout_secs: '45', max_recent: '20', font_family: 'Arial', font_size: '14.5', grid_font_size: '15', sql_font_family: 'Courier New', sql_font_size: '16.5', sync_url: ' https://next.test/dav ', sync_user: 'next', sync_auto_every: '15' };
  for (const [key, value] of Object.entries(edits)) await change(control(dialog, key), value, key.includes('family') || key === 'sync_auto_every' ? 'change' : 'input');
  for (const key of ['auto_complete', 'confirm_on_close']) { control(dialog, key).checked = false; await control(dialog, key).emit('change'); }
  assert.match(dialog.body.querySelectorAll('.checkbox')[0].textContent, /已关闭/);
  const pending = confirm(dialog).emit('click');
  await h.settle();
  assert.deepEqual(calls.map(call => call[0]), ['sync-start']);
  assert.equal(dialog.el.isConnected, true);
  resolveSave(); await pending; await h.settle();
  assert.deepEqual(calls.map(call => call[0]), ['sync-start', 'sync-finish', 'settings']);
  assert.deepEqual(calls[2][1], { ...current, rows_per_page: 250, query_timeout_secs: 45, connect_timeout_secs: 30, pool_acquire_timeout_secs: 30, sqlite_busy_timeout_secs: 10, max_recent: 20, font_family: 'Arial', font_size: 14.5, grid_font_size: 15, sql_font_family: 'Courier New', sql_font_size: 16.5, auto_complete: false, confirm_on_close: false });
  assert.equal(calls[0][1].url, 'https://next.test/dav');
  assert.equal(calls[0][1].auto_every, 15);
  assert.equal(dialog.el.isConnected, false);
  assert.deepEqual(f.calls.map(call => call[0]), ['pause', 'read', 'cancel', 'release']);
});

test('查询0禁用预算；原有字体和行数回退保留', async () => {
  const h = harness(), saved = [], dialog = h.openSettingsDialog(settings(), value => saved.push(plain(value)));
  for (const [key, value] of Object.entries({ font_size: '', grid_font_size: '0', rows_per_page: '0', query_timeout_secs: '0', max_recent: '', sql_font_size: '0' })) await change(control(dialog, key), value);
  await confirm(dialog).emit('click');
  assert.deepEqual(Object.fromEntries(['font_size', 'grid_font_size', 'rows_per_page', 'query_timeout_secs', 'max_recent', 'sql_font_size'].map(key => [key, saved[0][key]])), { font_size: 12, grid_font_size: 12, rows_per_page: 1000, query_timeout_secs: 0, max_recent: 10, sql_font_size: 13 });
  assert.equal(saved[0].show_system_tray, true); assert.equal(saved[0].theme, 'light');
});

test('同步保存错误不关闭、不调用设置回调，重试成功才清理秘密', async () => {
  const h = harness(), saved = [], payloads = [];
  let fail = true;
  const f = syncFixture({ async save(input) { payloads.push(plain(input)); if (fail) throw new Error('同步配置保存失败'); return config(input); } });
  const dialog = h.openSettingsDialog(settings(), value => saved.push(value), f.sync);
  await h.settle();
  await change(control(dialog, 'sync_password'), 'new-password');
  await confirm(dialog).emit('click');
  assert.equal(saved.length, 0); assert.equal(dialog.el.isConnected, true);
  const error = dialog.body.querySelector('.dialog-error');
  assert.match(error.textContent, /同步配置保存失败/); assert.equal(error.getAttribute('role'), 'alert');
  assert.equal(control(dialog, 'sync_password').value, 'new-password');
  assert.equal(f.calls.some(call => call[0] === 'cancel'), false);
  fail = false; await confirm(dialog).emit('click'); await h.settle();
  assert.equal(payloads.length, 2); assert.equal(saved.length, 1);
  assert.equal(control(dialog, 'sync_password').value, ''); assert.equal(dialog.el.isConnected, false);
});

test('设置保存回调抛错时窗口保留，未修改的同步配置不重复保存', async () => {
  const h = harness(), f = syncFixture();
  const dialog = h.openSettingsDialog(settings(), () => { throw new Error('设置保存失败'); }, f.sync);
  await h.settle(); await confirm(dialog).emit('click');
  assert.equal(dialog.el.isConnected, true);
  assert.match(dialog.body.querySelector('.dialog-error').textContent, /设置保存失败/);
  assert.equal(f.calls.some(call => call[0] === 'sync-save'), false);
  dialog.close(); await h.settle();
});

test('执行预算边界和空值拒绝关联字段错误；异步设置失败保留窗口', async () => {
  for (const value of ['', '-1', '1.5', '86401', 'NaN', 'Infinity']) {
    const h = harness(), saved = [], dialog = h.openSettingsDialog(settings(), s => saved.push(s));
    await change(control(dialog, 'query_timeout_secs'), value); await confirm(dialog).emit('click'); await h.settle();
    assert.equal(saved.length, 0); assert.equal(dialog.el.isConnected, true); assert.equal(control(dialog, 'query_timeout_secs').getAttribute('aria-invalid'), 'true');
    assert.match(dialog.body.querySelector('.dialog-error').textContent, /范围内的整数/); dialog.close();
  }
  const h = harness(), dialog = h.openSettingsDialog(settings(), async () => { throw new Error('真实IPC保存失败'); });
  await confirm(dialog).emit('click'); await h.settle(); assert.equal(dialog.el.isConnected, true); assert.match(dialog.body.querySelector('.dialog-error').textContent, /真实IPC保存失败/); dialog.close();
});

test('WebDAV 请求头回显、未改或留空不重提交，密码与口令不回显，频率非法值回退暂停', async () => {
  const h = harness(), f = syncFixture({ async getConfig() { return config({ auto_every: 7, password: '禁止回显的密码', passphrase: '禁止回显的口令', headers: 'Authorization: Bearer saved-demo\nX-Client: NavKit' }); } });
  const component = h.createDatasourceSyncTab(f.sync); h.document.body.appendChild(component.el);
  await h.settle();
  const input = key => component.el.querySelector(`[data-sync-field="${key}"]`);
  const interval = input('auto_every');
  assert.deepEqual(interval.children.map(option => Number(option.value)), [-1, 3, 15, 30, 60]);
  assert.equal(interval.value, '-1');
  for (const key of ['password', 'passphrase']) {
    assert.equal(input(key).value, ''); assert.equal(input(key).autocomplete, 'off');
    assert.equal(input(key).placeholder, '已设置，留空保留');
  }
  assert.equal(input('headers').value, config().headers);
  assert.equal(input('headers').autocomplete, 'off');
  assert.equal(component.el.querySelector('.settings-sync-note'), null);
  assert.equal(input('password').type, 'password'); assert.equal(input('passphrase').type, 'password');
  await change(input('user'), 'changed'); await change(interval, '30', 'change');
  await component.saveIfNeeded();
  const request = f.calls.find(call => call[0] === 'sync-save')[1];
  assert.equal(request.auto_every, 30); assert.equal(request.remember_password, true);
  for (const key of ['password', 'passphrase', 'headers']) assert.equal(Object.hasOwn(request, key), false);
  await change(input('password'), 'replacement'); await change(input('passphrase'), 'key'); await change(input('headers'), 'Authorization: Bearer demo');
  await component.saveIfNeeded();
  const replacement = f.calls.filter(call => call[0] === 'sync-save').at(-1)[1];
  assert.deepEqual([replacement.password, replacement.passphrase, replacement.headers], ['replacement', 'key', 'Authorization: Bearer demo']);
  for (const key of ['password', 'passphrase']) assert.equal(input(key).value, '');
  assert.equal(input('headers').value, replacement.headers);
  await change(input('headers'), '  '); await component.saveIfNeeded();
  assert.equal(Object.hasOwn(f.calls.filter(call => call[0] === 'sync-save').at(-1)[1], 'headers'), false);
  assert.equal(input('headers').value, replacement.headers);
  assert.equal(component.el.querySelectorAll('button').length, 0);
  component.dispose(); component.dispose(); await h.settle();
  assert.equal(f.calls.filter(call => call[0] === 'cancel').length, 1);
  assert.equal(f.calls.filter(call => call[0] === 'release').length, 1);
  assert.equal(input('headers').value, '');
});

test('请求头保存后重新打开回显，地址变化不携带未修改的旧请求头', async () => {
  const h = harness(), f = syncFixture();
  let component = h.createDatasourceSyncTab(f.sync); h.document.body.appendChild(component.el); await h.settle();
  const headers = component.el.querySelector('[data-sync-field="headers"]');
  await change(headers, 'Authorization: Bearer changed-demo\nX-Client: Test'); await component.saveIfNeeded();
  component.dispose(); await h.settle();
  component = h.createDatasourceSyncTab(f.sync); h.document.body.appendChild(component.el); await h.settle();
  assert.equal(component.el.querySelector('[data-sync-field="headers"]').value, 'Authorization: Bearer changed-demo\nX-Client: Test');
  await change(component.el.querySelector('[data-sync-field="url"]'), 'https://another.test/dav'); await component.saveIfNeeded();
  assert.equal(Object.hasOwn(f.calls.filter(call => call[0] === 'sync-save').at(-1)[1], 'headers'), false);
  component.dispose(); await h.settle();
});

test('缺少 WebDAV 地址沿用原校验，失败后不保存设置或关闭', async () => {
  const h = harness(), saved = [], f = syncFixture();
  const dialog = h.openSettingsDialog(settings(), value => saved.push(value), f.sync);
  await h.settle(); await change(control(dialog, 'sync_url'), '  ');
  await confirm(dialog).emit('click');
  assert.equal(dialog.el.isConnected, true); assert.equal(saved.length, 0);
  assert.equal(control(dialog, 'sync_url').getAttribute('aria-invalid'), 'true');
  const error = dialog.body.querySelector('.dialog-error');
  assert.match(error.textContent, /请填写 WebDAV 基础目录地址/);
  assert.ok(control(dialog, 'sync_url').getAttribute('aria-describedby').split(/\s+/).includes(error.id));
  assert.equal(f.calls.some(call => call[0] === 'sync-save'), false);
  dialog.close(); await h.settle();
});

test('现有辅助原语约束焦点，Escape 关闭清理秘密并归还焦点，迟到读取不再填充', async () => {
  const h = harness();
  const trigger = h.document.createElement('button'); h.document.body.appendChild(trigger); trigger.focus();
  let resolveRead;
  const read = new Promise(resolve => { resolveRead = resolve; });
  const f = syncFixture({ getConfig: () => read, async cancel() { f.calls.push(['cancel']); throw new Error('取消失败仍需恢复计时'); } });
  const dialog = h.openSettingsDialog(settings(), () => {}, f.sync, 'font');
  await h.settle();
  const close = dialog.el.querySelector('.dialog-title-close'), last = dialog.footer.children.at(-1);
  close.focus(); await dialog.el.emit('keydown', { key: 'Tab', shiftKey: true });
  assert.equal(h.document.activeElement, last);
  await dialog.el.emit('keydown', { key: 'Tab', shiftKey: false });
  assert.equal(h.document.activeElement, close);
  await change(control(dialog, 'sync_password'), 'temporary');
  await change(control(dialog, 'sync_headers'), 'temporary');
  await dialog.el.emit('keydown', { key: 'Escape' });
  dialog.close(); await h.settle();
  assert.equal(dialog.el.isConnected, false); assert.equal(h.document.activeElement, trigger);
  for (const key of ['sync_password', 'sync_passphrase', 'sync_headers']) assert.equal(control(dialog, key).value, '');
  resolveRead(config({ url: 'https://late.test/dav' })); await h.settle();
  assert.equal(control(dialog, 'sync_url').value, '');
  assert.equal(f.calls.filter(call => call[0] === 'cancel').length, 1);
  assert.equal(f.calls.filter(call => call[0] === 'release').length, 1);
});

test('局部 CSS 可解析，选择器仅作用设置窗口，全部令牌实际存在并保留内部滚动', async () => {
  const css = await readFile(new URL('../../src/styles/settings-dialog.css', import.meta.url), 'utf8');
  const base = await readFile(new URL('../../src/styles/base.css', import.meta.url), 'utf8');
  const result = await transform(css, { loader: 'css', logLevel: 'silent' });
  assert.deepEqual(result.warnings, []);
  const tokens = new Set([...base.matchAll(/(--[\w-]+)\s*:/g)].map(match => match[1]));
  for (const match of css.matchAll(/var\((--[\w-]+)/g)) assert.ok(tokens.has(match[1]), '不存在的基础令牌：' + match[1]);
  const selectors = [...css.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{/g)].map(match => match[1].trim());
  for (const selector of selectors) assert.ok(selector.startsWith('.dialog.settings-dialog') || selector.startsWith('@media'), '越界选择器：' + selector);
  assert.doesNotMatch(css, /#[0-9a-f]{3,8}\b|@keyframes|transition\s*:/i);
  assert.match(css, /\.dialog\.settings-dialog\s*\{[^}]*min-width:\s*0/);
  assert.match(css, /\.settings-layout\s*\{[^}]*144px minmax\(0, 1fr\)/);
  assert.match(css, /\.settings-content\s*\{[^}]*min-height:\s*0[^}]*overflow:\s*auto/);
  assert.match(css, /\.settings-sync-panel \.settings-field\s*\{[^}]*grid-template-columns:\s*minmax\(0, 1fr\) 320px/);
  assert.doesNotMatch(css, /settings-sync-note/);
  assert.match(css, /@media\s*\(max-width:\s*760px\)[\s\S]*grid-template-columns:\s*minmax\(0, 1fr\)/);
});
