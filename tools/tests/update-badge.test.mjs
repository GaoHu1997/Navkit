// 标题栏「更新」徽标：三态切换（隐藏 / 更新 / 下载中 / 重启）与点击行为。
// IPC 走 __PREVIEW_MOCK__（与桌面 IPC 同名命令），组件内部静态依赖 api.call。
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { build } from 'esbuild';

const built = await build({
  stdin: { contents: 'export { UpdateBadge } from "./src/ui/UpdateBadge.ts"; export * from "./src/core/updatePreferences.ts";', resolveDir: process.cwd(), loader: 'ts' },
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
  const intervals = [];
  const storage = new Map();
  const sandbox = {
    document, console,
    localStorage: { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) },
    setTimeout: fn => { timers.push(fn); return timers.length; },
    clearTimeout() {},
    clearInterval(id) { intervals[id - 1] = null; }, setInterval: fn => { intervals.push(fn); return intervals.length; },
  };
  sandbox.window = sandbox;
  sandbox.__PREVIEW_MOCK__ = mock;
  return { sandbox, timers, intervals };
}

async function instantiate(mock) {
  const { sandbox, timers, intervals } = makeEnv(mock);
  sandbox.exports = {}; sandbox.module = { exports: sandbox.exports };
  vm.runInNewContext(built.outputFiles[0].text, sandbox, { filename: 'update-badge.cjs' });
  const { UpdateBadge } = sandbox.module.exports;
  const notify = [];
  const badge = new UpdateBadge((kind, text) => notify.push({ kind, text }));
  return { badge, sandbox, timers, intervals, notify, ...sandbox.module.exports };
}

const flush = async () => { for (let i = 0; i < 20; i += 1) await Promise.resolve(); };

const fixtureManifest = { version: '1.2.0', notes: '', pub_date: '', url: '', sha256: 'a'.repeat(64), size: 10 };
test('自动更新开启后后台下载，但运行期间不安装', async () => {
  let downloads = 0, applies = 0;
  const env = await instantiate({
    update_check: () => ({ update_available: true, manifest: fixtureManifest }),
    update_status: () => ({ phase: 'idle' }),
    update_download: () => { downloads++; return 'D:/staged.exe'; },
    update_apply: () => { applies++; },
  });
  env.saveUpdatePreferences({ autoUpdate: true });
  env.badge.start(); await env.timers[0](); await flush();
  assert.equal(downloads, 1); assert.equal(applies, 0);
  assert.equal(env.badge.el.dataset.phase, 'ready');
  assert.equal(env.readUpdatePreferences().pendingInstall.version, '1.2.0');
});
test('跳过只抑制同版本后台提示和自动下载，新版本正常可见', async () => {
  let version = '1.2.0', downloads = 0;
  const env = await instantiate({
    update_check: () => ({ update_available: true, manifest: { ...fixtureManifest, version } }),
    update_status: () => ({ phase: 'idle' }),
    update_download: () => { downloads++; return 'D:/staged.exe'; },
  });
  env.saveUpdatePreferences({ autoUpdate: true }); env.skipUpdateVersion('1.2.0');
  await env.badge['check'](); await flush();
  assert.equal(env.badge.el.hidden, true); assert.equal(downloads, 0);
  version = '1.2.1'; await env.badge['check'](); await flush();
  assert.equal(downloads, 1); assert.equal(env.badge.el.dataset.phase, 'ready');
});
test('弹窗跳过当前版本时，已经显示的后台徽标立即隐藏', async () => {
  const env = await instantiate({ update_status: () => ({ phase: 'idle' }) });
  env.badge.start(); env.badge.setAvailable(fixtureManifest);
  env.skipUpdateVersion('1.2.0');
  assert.equal(env.badge.el.hidden, true);
});

test('初始为隐藏，不占位', async () => {
  const { badge } = await instantiate({});
  assert.equal(badge.el.hidden, true);
  assert.equal(badge.el.dataset.phase, 'hidden');
  assert.equal(badge.el.textContent, '更新');
});

test('发现新版本：显示绿色「更新」，并触发一次检查', async () => {
  const calls = [];
  const { badge, timers } = await instantiate({
    update_check: async () => { calls.push('check'); return { current_version: '1.0.0', update_available: true, manifest: { version: '1.2.0', notes: '', pub_date: '', url: '', sha256: 'a'.repeat(64), size: 10 } }; },
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
    update_check: async () => ({ current_version: '1.0.0', update_available: true, manifest: { version: '1.2.0', notes: '', pub_date: '', url: '', sha256: 'a'.repeat(64), size: 10 } }),
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


function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function assertBusy(badge, phase) {
  assert.equal(badge.el.dataset.phase, phase);
  assert.equal(badge.el.disabled, true);
  assert.equal(badge.el.getAttribute('aria-busy'), 'true');
  const loading = badge.el.children[0].children.find(child => child.className === 'ub-loading');
  assert.equal(loading.getAttribute('aria-hidden'), 'true');
  assert.equal(loading.children[0].className, 'ub-spinner');
}
function assertIdle(badge, phase) {
  assert.equal(badge.el.dataset.phase, phase);
  assert.equal(badge.el.disabled, false);
  assert.equal(badge.el.getAttribute('aria-busy'), 'false');
}

test('更新点击同步进入忙碌，下载期间重复事件只调用一次 IPC', async () => {
  const pending = deferred(); let calls = 0;
  const { badge } = await instantiate({ update_download: () => { calls++; return pending.promise; } });
  badge.setAvailable(fixtureManifest); assertIdle(badge, 'available');
  badge.el.dispatch('click'); assertBusy(badge, 'downloading');
  for (let i = 0; i < 10; i++) badge.el.dispatch('click');
  await badge['download'](); await flush(); assert.equal(calls, 1);
  badge['enterDownloading'](40, 100); assertBusy(badge, 'downloading');
  assert.match(badge.el.title, /40%/);
  pending.resolve('staged'); await flush(); assertIdle(badge, 'ready');
});

test('下载失败解除忙碌，用户可重试并在成功后看到重启', async () => {
  const first = deferred(); let calls = 0;
  const { badge, notify } = await instantiate({ update_download: () => ++calls === 1 ? first.promise : 'staged' });
  badge.setAvailable(fixtureManifest); badge.el.dispatch('click'); await flush();
  first.reject(new Error('网络失败')); await flush();
  assertIdle(badge, 'available'); assert.match(notify[0].text, /网络失败/);
  badge.el.dispatch('click'); assertBusy(badge, 'downloading'); await flush();
  assert.equal(calls, 2); assertIdle(badge, 'ready');
});

test('重启同步锁定，程序派发与直接重入均只安装一次，成功回执后仍忙碌', async () => {
  const pending = deferred(); let calls = 0;
  const { badge } = await instantiate({ update_apply: () => { calls++; return pending.promise; } });
  badge['setReady'](); badge.el.dispatch('click'); assertBusy(badge, 'applying');
  for (let i = 0; i < 10; i++) badge.el.dispatch('click');
  await badge['apply'](); await flush(); assert.equal(calls, 1);
  badge['setReady'](); badge.setAvailable(fixtureManifest); badge['enterDownloading'](30, 100);
  assertBusy(badge, 'applying'); assert.match(badge.el.getAttribute('aria-label'), /正在重启/);
  pending.resolve(); await flush(); badge.el.dispatch('click'); await flush();
  assert.equal(calls, 1); assertBusy(badge, 'applying');
});

test('重启失败恢复可点击状态和错误提示，重试成功后保持锁定', async () => {
  const first = deferred(); let calls = 0;
  const { badge, notify } = await instantiate({ update_apply: () => ++calls === 1 ? first.promise : undefined });
  badge['setReady'](); badge.el.dispatch('click'); await flush();
  first.reject(new Error('文件占用')); await flush();
  assertIdle(badge, 'ready'); assert.match(badge.el.title, /文件占用/);
  assert.match(badge.el.getAttribute('aria-label'), /失败.*重试/); assert.match(notify[0].text, /文件占用/);
  badge.el.dispatch('click'); assertBusy(badge, 'applying'); await flush(); assert.equal(calls, 2);
});

test('迟到启动 ready 回执不能提前解锁正在下载的更新', async () => {
  const status = deferred(), download = deferred();
  const { badge } = await instantiate({ update_status: () => status.promise, update_download: () => download.promise });
  badge.start(); badge.setAvailable(fixtureManifest); badge.el.dispatch('click'); await flush();
  status.resolve({ phase: 'ready' }); await flush(); assertBusy(badge, 'downloading');
  download.resolve('staged'); await flush(); assertIdle(badge, 'ready'); badge.stop();
});

test('在途轮询 ready 回执和迟到下载成功不能解除重启锁定', async () => {
  const status = deferred(), download = deferred(), apply = deferred(); let applies = 0;
  const { badge, intervals } = await instantiate({
    update_status: () => status.promise,
    update_download: () => download.promise,
    update_apply: () => { applies++; return apply.promise; },
  });
  badge.setAvailable(fixtureManifest); badge.el.dispatch('click'); await flush();
  const poll = intervals.find(Boolean); poll(); await flush();
  badge['setReady'](); badge.el.dispatch('click'); await flush(); assertBusy(badge, 'applying');
  status.resolve({ phase: 'ready' }); download.resolve('staged'); await flush();
  assertBusy(badge, 'applying'); badge.el.dispatch('click'); await flush(); assert.equal(applies, 1);
  apply.resolve(); await flush(); assertBusy(badge, 'applying');
});

test('停止后迟到启动状态不显示或解锁徽标', async () => {
  const status = deferred();
  const { badge } = await instantiate({ update_status: () => status.promise });
  badge.start(); badge.stop(); status.resolve({ phase: 'ready' }); await flush();
  assertIdle(badge, 'hidden');
});
