// 软件更新对话框（需求 1.2）：检查更新 → 下载 → 重启并安装；无新版本提示。
// IPC 走 __PREVIEW_MOCK__；任务注册与对话框共用同一 bundle 实例。
import test from 'node:test';
import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import { build } from 'esbuild';

const root = fileURLToPath(new URL('../..', import.meta.url));
const built = await build({
  stdin: {
    contents: 'export { openUpdateDialog } from "./src/dialogs/UpdateDialog.ts"; export { taskCenter } from "./src/core/backgroundTasks.ts";',
    resolveDir: root, loader: 'ts',
  },
  bundle: true, write: false, platform: 'node', format: 'cjs', loader: { '.css': 'empty' }, logLevel: 'silent',
});

// ---- 最小 DOM 桩（与 data-dictionary-dialog.test.mjs 相同）----
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
  get value() { return this.tagName === 'SELECT' ? (this.children.find(option => option.selected)?.value ?? this.children[0]?.value ?? '') : this._value; }
  set value(value) {
    this._value = String(value);
    if (this.tagName === 'SELECT') for (const option of this.children) option.selected = option.value === this._value;
  }
  set textContent(value) { this._text = String(value); this._html = ''; this.children = []; }
  get textContent() { return this._text + this._html.replace(/<[^>]*>/g, '') + this.children.map(child => child.textContent).join(''); }
  set innerHTML(value) { this._html = String(value); this._text = ''; this.children = []; }
  get innerHTML() { return this._html; }
  appendChild(child) { child.remove(); this.children.push(child); child.parentNode = this; return child; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  prepend(...children) { for (const child of [...children].reverse()) { child.remove(); this.children.unshift(child); child.parentNode = this; } }
  replaceChildren(...children) { for (const child of [...this.children]) child.parentNode = null; this.children = []; for (const child of children) this.appendChild(child); }
  insertBefore(child, before) {
    child.remove(); const index = this.children.indexOf(before);
    this.children.splice(index < 0 ? this.children.length : index, 0, child); child.parentNode = this;
  }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); this.parentNode = null; }
  get firstChild() { return this.children[0] ?? null; }
  get isConnected() { return this === this.ownerDocument?.body || Boolean(this.parentNode?.isConnected); }
  get options() { return this.tagName === 'SELECT' ? this.children : []; }
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  hasAttribute(name) { return this.attributes.has(name) || ['disabled', 'hidden'].includes(name) && Boolean(this[name]); }
  getAttribute(name) {
    if (this.attributes.has(name)) return this.attributes.get(name);
    if (name === 'disabled' || name === 'hidden') return this[name] ? '' : null;
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

const manifest = {
  version: '1.1.0', notes: '新增数据字典导出与关系图', pub_date: '2026-11-12',
  url: 'https://example.test/navkit-1.1.0.exe', sha256: 'a'.repeat(64), size: 15 * 1024 * 1024,
};

function harness(overrides = {}) {
  const calls = [];
  const mock = {
    about_info: () => ({ product: 'NavKit', version: '1.0.0' }),
    update_check: () => { calls.push(['update_check']); return { current_version: '1.0.0', update_available: true, manifest }; },
    update_download: () => { calls.push(['update_download']); return 'D:/navkit/data/update/navkit-new.exe'; },
    update_status: () => ({ phase: 'idle', done: 0, total: 0, speed: 0, message: '', staged_file: null }),
    update_apply: () => { calls.push(['update_apply']); return; },
    ...overrides,
  };
  const document = { activeElement: null, createElement(tag) { const element = new Element(tag); element.ownerDocument = document; return element; } };
  document.body = document.createElement('body'); document.activeElement = document.body;
  const microtasks = [], module = { exports: {} };
  const context = vm.createContext({
    module, exports: module.exports, document, HTMLElement: Element, crypto: webcrypto, Error,
    queueMicrotask: callback => microtasks.push(callback),
    setTimeout: callback => microtasks.push(callback),
    clearTimeout: () => {},
    console,
    window: { innerWidth: 1280, innerHeight: 900, addEventListener() {}, removeEventListener() {}, __PREVIEW_MOCK__: mock },
  });
  vm.runInContext(built.outputFiles[0].text, context, { filename: 'update-dialog.cjs' });
  const settle = async () => {
    while (microtasks.length) microtasks.shift()();
    for (let turn = 0; turn < 6; turn++) await Promise.resolve();
  };
  return { ...module.exports, document, settle, calls };
}

test('打开对话框显示当前版本（about_info）', async () => {
  const { openUpdateDialog, settle, document } = harness();
  const dlg = openUpdateDialog();
  await settle();
  const versionInput = dlg.body.querySelector('input[aria-label="当前版本"]') ?? dlg.body.querySelectorAll('input')[0];
  assert.equal(versionInput.value, '1.0.0');
  const checkBtn = findBtn(dlg, '检查更新');
  assert.ok(checkBtn, '应有检查更新按钮');
  document.body.appendChild(dlg.el);
});

test('检查更新发现新版本 → 显示版本号与下载按钮；下载完成 → 重启并安装', async () => {
  const { openUpdateDialog, settle, calls, taskCenter } = harness();
  taskCenter.reset();
  const dlg = openUpdateDialog();
  await settle();
  const checkBtn = findBtn(dlg, '检查更新');
  const downloadBtn = findBtn(dlg, '下载更新');
  const applyBtn = findBtn(dlg, '重启并安装');
  assert.equal(downloadBtn.hidden, true, '检查前不显示下载按钮');
  await checkBtn.emit('click');
  await settle();
  const info = dlg.body.children[1];
  assert.match(info.textContent, /1\.1\.0/);
  assert.match(info.textContent, /新增数据字典导出与关系图/);
  assert.equal(downloadBtn.hidden, false, '发现新版本后显示下载按钮');
  await downloadBtn.emit('click');
  await settle();
  assert.ok(calls.some(c => c[0] === 'update_download'));
  assert.equal(applyBtn.hidden, false, '下载完成后显示重启并安装');
  assert.match(dlg.body.children[1].textContent, /已下载完成/);
  assert.match(dlg.body.children[1].textContent, /换名替换/);
  const updateTask = taskCenter.snapshot().tasks.find(task => task.kind === 'update');
  assert.ok(updateTask, '下载应在任务中心登记 update 任务');
  assert.equal(updateTask.status, 'success');
});

test('已是最新版本时不显示下载按钮', async () => {
  const { openUpdateDialog, settle } = harness({
    update_check: () => ({ current_version: '1.0.0', update_available: false, manifest: null }),
  });
  const dlg = openUpdateDialog();
  await settle();
  await findBtn(dlg, '检查更新').emit('click');
  await settle();
  assert.match(dlg.body.children[1].textContent, /已是最新版本/);
  assert.equal(findBtn(dlg, '下载更新').hidden, true);
});

test('检查失败显示错误信息且可重试', async () => {
  const { openUpdateDialog, settle } = harness({
    update_check: () => { throw new Error('清单请求失败: timeout'); },
  });
  const dlg = openUpdateDialog();
  await settle();
  await findBtn(dlg, '检查更新').emit('click');
  await settle();
  assert.match(dlg.body.textContent, /清单请求失败/);
  assert.equal(findBtn(dlg, '检查更新').disabled, false, '失败后按钮可重试');
});

test('update_status 就绪时直接进入可安装状态', async () => {
  const { openUpdateDialog, settle } = harness({
    update_status: () => ({ phase: 'ready', done: 0, total: 0, speed: 0, message: '', staged_file: 'D:/x/navkit-new.exe' }),
  });
  const dlg = openUpdateDialog();
  await settle();
  assert.equal(findBtn(dlg, '重启并安装').hidden, false, '已有就绪包时直接可安装');
});

function findBtn(dlg, text) {
  return dlg.body.querySelectorAll('button').find(button => button.textContent === text)
    ?? dlg.footer.querySelectorAll('button').find(button => button.textContent === text);
}
