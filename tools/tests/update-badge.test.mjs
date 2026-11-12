// 标题栏「更新」徽标：三态切换（隐藏 / 更新 / 下载中 / 重启）与点击行为。
// IPC 走 __PREVIEW_MOCK__（与桌面 IPC 同名命令），组件内部静态依赖 api.call。
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { build } from 'esbuild';

const built = await build({
  stdin: { contents: 'export { UpdateBadge } from "./src/ui/UpdateBadge.ts";', resolveDir: process.cwd(), loader: 'ts' },
  bundle: true, write: false, platform: 'node', format: 'cjs',
  loader: { '.css': 'empty' }, external: ['@tauri-apps/api/event'], logLevel: 'silent',
});

class El {
  constructor(tag = 'div') {
    this.tagName = tag.toUpperCase(); this.children = []; this.parentNode = null;
    this.style = {}; this.dataset = {}; this.attrs = new Map(); this.listeners = new Map();
    this.className = ''; this.hidden = false; this.disabled = false; this.type = ''; this.title = '';
    this._text = '';
  }
  setAttribute(k, v) { this.attrs.set(k, String(v)); }
  getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  addEventListener(type, fn) { if (!this.listeners.has(type)) this.listeners.set(type, []); this.listeners.get(type).push(fn); }
  dispatch(type) { for (const fn of this.listeners.get(type) ?? []) fn({ preventDefault() {}, stopPropagation() {} }); }
  append(...kids) { for (const kid of kids) { kid.parentNode = this; this.children.push(kid); } }
  set textContent(v) { this._text = String(v); }
  get textContent() { return this._text + this.children.map(c => c.textContent).join(''); }
  find(tag) { for (const c of this.children) { if (c.tagName === tag.toUpperCase()) return c; const hit = c.find(tag); if (hit) return hit; } return null; }
}

function makeEnv(mock) {
  const document = { createElement: t => new El(t), createElementNS: (_ns, t) => new El(t) };
  const timers = [];
  const sandbox = {
    document, console,
    setTimeout: fn => { timers.push(fn); return timers.length; },
    clearInterval() {}, setInterval: () => 0,
  };
  sandbox.window = sandbox;
  sandbox.__PREVIEW_MOCK__ = mock;
  return { sandbox, timers };
}

async function instantiate(mock) {
  const { sandbox, timers } = makeEnv(mock);
  sandbox.exports = {}; sandbox.module = { exports: sandbox.exports };
  vm.runInNewContext(built.outputFiles[0].text, sandbox, { filename: 'update-badge.cjs' });
  const { UpdateBadge } = sandbox.module.exports;
  const notify = [];
  const badge = new UpdateBadge((kind, text) => notify.push({ kind, text }));
  return { badge, sandbox, timers, notify };
}

const flush = async () => { for (let i = 0; i < 6; i += 1) await Promise.resolve(); };

test('初始为隐藏，不占位', async () => {
  const { badge } = await instantiate({});
  assert.equal(badge.el.hidden, true);
  assert.equal(badge.el.dataset.phase, 'hidden');
  assert.equal(badge.el.textContent, '更新');
});

test('发现新版本：显示绿色「更新」，并触发一次检查', async () => {
  const calls = [];
  const { badge, timers } = await instantiate({
    update_check: async () => { calls.push('check'); return { current_version: '1.0.0', update_available: true, manifest: { version: '1.2.0', notes: '', pub_date: '', url: '', sha256: '', size: 10 } }; },
    update_status: async () => ({ phase: 'idle', done: 0, total: 0, speed: 0, message: '', staged_file: null }),
  });
  badge.start();
  assert.equal(timers.length, 1, '首次检查走延迟定时器');
  await timers[0]();
  await flush();
  assert.deepEqual(calls, ['check']);
  assert.equal(badge.el.dataset.phase, 'available');
  assert.equal(badge.el.hidden, false);
  assert.equal(badge.el.textContent, '更新');
});

test('无新版本：保持隐藏', async () => {
  const { badge, timers } = await instantiate({
    update_check: async () => ({ current_version: '1.0.0', update_available: false, manifest: null }),
    update_status: async () => ({ phase: 'idle', done: 0, total: 0, speed: 0, message: '', staged_file: null }),
  });
  badge.start();
  await timers[0]();
  await flush();
  assert.equal(badge.el.dataset.phase, 'hidden');
  assert.equal(badge.el.hidden, true);
});

test('已有就绪安装包：启动即为橘黄「重启」', async () => {
  const { badge } = await instantiate({
    update_status: async () => ({ phase: 'ready', done: 100, total: 100, speed: 0, message: '', staged_file: 'x' }),
  });
  badge.start();
  await flush();
  assert.equal(badge.el.dataset.phase, 'ready');
  assert.equal(badge.el.textContent, '重启');
});

test('点击下载 → 进入进度态 → 完成后变「重启」；再次点击调用安装', async () => {
  let applied = 0;
  const { badge } = await instantiate({
    update_check: async () => ({ current_version: '1.0.0', update_available: true, manifest: { version: '1.2.0', notes: '', pub_date: '', url: '', sha256: '', size: 10 } }),
    update_status: async () => ({ phase: 'downloading', done: 40, total: 100, speed: 0, message: '', staged_file: null }),
    update_download: async () => 'staged',
    update_apply: async () => { applied += 1; },
  });
  badge['check']();
  await flush();
  assert.equal(badge.el.dataset.phase, 'available');

  badge.el.dispatch('click');
  await flush();
  assert.equal(badge.el.dataset.phase, 'ready');
  assert.equal(badge.el.textContent, '重启');

  badge.el.dispatch('click');
  await flush();
  assert.equal(applied, 1);
});
