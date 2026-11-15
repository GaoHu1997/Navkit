// 软件更新对话框（需求 1.2）：打开即自动检查 → 下载 → 重启并安装。
// 关键回归点：不再有「检查更新」按钮，弹窗打开就自动请求 update_check。
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
  version: '1.1.0', notes: '新增数据字典导出与关系图\n- 修复导入进度停滞\n\n* 启动更快', pub_date: '2026-11-12',
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
    for (let turn = 0; turn < 12; turn++) await Promise.resolve();
  };
  return { ...module.exports, document, settle, calls };
}

test('打开对话框即自动检查，无需手动点击', async () => {
  const { openUpdateDialog, settle, calls, document } = harness();
  const dlg = openUpdateDialog();
  await settle();
  assert.deepEqual(calls.filter(c => c[0] === 'update_check').length, 1, '打开即应发起一次 update_check');
  assert.equal(findBtn(dlg, '检查更新'), undefined, '不应再出现手动「检查更新」按钮');
  assert.match(dlg.body.textContent, /发现新版本 1\.1\.0/);
  assert.match(dlg.body.textContent, /已安装/);
  assert.match(dlg.body.textContent, /最新版/);
  assert.match(dlg.body.textContent, /更新包 15\.0 MB/);
  document.body.appendChild(dlg.el);
});

test('更新说明按行拆成列表，去掉项目符号', async () => {
  const { openUpdateDialog, settle } = harness();
  const dlg = openUpdateDialog();
  await settle();
  const items = dlg.body.querySelectorAll('li').map(li => li.textContent);
  assert.deepEqual(items, ['新增数据字典导出与关系图', '修复导入进度停滞', '启动更快']);
});

test('发现新版本 → 下载并安装 → 就绪可重启', async () => {
  const { openUpdateDialog, settle, calls, taskCenter } = harness();
  taskCenter.reset();
  const dlg = openUpdateDialog();
  await settle();
  assert.equal(findBtn(dlg, '稍后提醒').hidden, false);
  await findBtn(dlg, '下载并安装').emit('click');
  await settle();
  assert.ok(calls.some(c => c[0] === 'update_download'));
  assert.match(dlg.body.textContent, /已就绪/);
  assert.match(dlg.body.textContent, /SHA-256 校验通过/);
  assert.equal(findBtn(dlg, '重启并安装').hidden, false);
  const updateTask = taskCenter.snapshot().tasks.find(task => task.kind === 'update');
  assert.ok(updateTask, '下载应在任务中心登记 update 任务');
  assert.equal(updateTask.status, 'success');
});

test('已是最新版本：显示版本对比，主按钮为完成', async () => {
  const { openUpdateDialog, settle } = harness({
    update_check: () => ({ current_version: '1.0.0', update_available: false, manifest: null }),
  });
  const dlg = openUpdateDialog();
  await settle();
  assert.match(dlg.body.textContent, /已是最新版本/);
  assert.match(dlg.body.textContent, /当前版本 1\.0\.0/);
  assert.equal(findBtn(dlg, '下载并安装'), undefined, '无新版本时不应有下载入口');
  assert.ok(findBtn(dlg, '完成'));
  assert.ok(findBtn(dlg, '重新检查'));
});

test('检查失败：显示错误并可重试', async () => {
  let attempts = 0;
  const { openUpdateDialog, settle } = harness({
    update_check: () => { attempts += 1; if (attempts === 1) throw new Error('清单请求失败: timeout'); return { current_version: '1.0.0', update_available: false, manifest: null }; },
  });
  const dlg = openUpdateDialog();
  await settle();
  assert.match(dlg.body.textContent, /检查更新失败/);
  assert.match(dlg.body.textContent, /清单请求失败/);
  const retry = findBtn(dlg, '重试');
  assert.ok(retry, '失败后应提供重试按钮');
  assert.equal(retry.disabled, false);
  await retry.emit('click');
  await settle();
  assert.equal(attempts, 2);
  assert.match(dlg.body.textContent, /已是最新版本/);
});

test('已有就绪安装包时直接进入可安装状态，不重复检查', async () => {
  const { openUpdateDialog, settle, calls } = harness({
    update_status: () => ({ phase: 'ready', done: 100, total: 100, speed: 0, message: '', staged_file: 'D:/x/navkit-new.exe' }),
  });
  const dlg = openUpdateDialog();
  await settle();
  assert.equal(findBtn(dlg, '重启并安装').hidden, false, '已有就绪包时直接可安装');
  assert.equal(calls.some(c => c[0] === 'update_check'), false, '就绪包存在时不必再检查');
});

test('下载中打开对话框：直接显示进度，不再发起检查', async () => {
  const { openUpdateDialog, settle, calls } = harness({
    update_status: () => ({ phase: 'downloading', done: 40, total: 100, speed: 2048, message: '', staged_file: null }),
  });
  const dlg = openUpdateDialog();
  await settle();
  assert.match(dlg.body.textContent, /正在下载/);
  assert.match(dlg.body.textContent, /40 B \/ 100 B/);
  assert.equal(calls.some(c => c[0] === 'update_check'), false);
  assert.ok(findBtn(dlg, '后台下载'));
});

function findBtn(dlg, text) {
  return dlg.body.querySelectorAll('button').find(button => button.textContent === text)
    ?? dlg.footer.querySelectorAll('button').find(button => button.textContent === text);
}
