// STORY-ER-LOAD-001：运行真实调度模块，不把 Promise 取消误当数据库 IPC 已终止。
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
const root = fileURLToPath(new URL('../../', import.meta.url));
const built = await build({ entryPoints: [root + 'src/core/erDesignReadQueue.ts'], bundle: true, write: false, platform: 'node', format: 'cjs', logLevel: 'silent' });
function harness() { const module = { exports: {} }; vm.runInNewContext(built.outputFiles[0].text, { module, exports: module.exports, Promise, Error }); return module.exports; }
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };

test('同端点最多两个真实在途；槽位保持到实际请求成功或失败', async () => {
  const api = harness(), a = deferred(), b = deferred(), c = deferred(); const sent = [];
  const task = (name, pending) => api.scheduleErDesignRead('endpoint', () => { sent.push(name); return pending.promise; }, () => {});
  const first = task('a', a), second = task('b', b), third = task('c', c);
  await settle(); assert.deepEqual(sent, ['a', 'b']); assert.equal(third.started, false);
  // 已发请求不能撤销或释放槽位，即使 UI 已切走。
  assert.equal(first.cancelQueued(), false); await settle(); assert.equal(sent.length, 2);
  a.resolve({ revision: 'a' }); await first.promise; await settle(); assert.deepEqual(sent, ['a', 'b', 'c']);
  b.reject(new Error('backend timeout after real cleanup')); await assert.rejects(second.promise, /backend timeout/);
  c.resolve({ revision: 'c' }); await third.promise;
});

test('撤销排队的旧选择不发送，重复撤销安全；不缓存跨 owner 的快照', async () => {
  const api = harness(), a = deferred(), b = deferred(); const sent = [];
  const first = api.scheduleErDesignRead('key', () => a.promise, () => {}), second = api.scheduleErDesignRead('key', () => b.promise, () => {});
  const discarded = api.scheduleErDesignRead('key', async () => { sent.push('discarded'); return {}; }, () => {});
  const rejection = assert.rejects(discarded.promise, api.ErDesignReadCancelled);
  assert.equal(discarded.cancelQueued(), true); assert.equal(discarded.cancelQueued(), false); await rejection; await settle();
  a.resolve({}); b.resolve({}); await Promise.all([first.promise, second.promise]); await settle(); assert.deepEqual(sent, []);
  for (const owner of ['one', 'two']) await api.scheduleErDesignRead('key', async () => { sent.push(owner); return { owner_id: owner }; }, () => {}).promise;
  assert.deepEqual(sent, ['one', 'two']);
});

test('started=true 必须意味着 run 已在同微任务执行，不存在不可撤销的预留窗口', async () => {
  const api = harness(), pending = deferred(); let sent = 0;
  const task = api.scheduleErDesignRead('key', () => { ++sent; return pending.promise; }, () => {});
  let observed;
  await new Promise(resolve => queueMicrotask(() => { observed = { started: task.started, sent, cancelled: task.cancelQueued() }; resolve(); }));
  assert.deepEqual(observed, { started: true, sent: 1, cancelled: false }); pending.resolve({}); await task.promise;
});

test('发送前同一事件内撤销，不会调用 onStart/run', async () => {
  const api = harness(); let calls = 0;
  const task = api.scheduleErDesignRead('key', async () => { ++calls; return {}; }, () => { ++calls; });
  const rejected = assert.rejects(task.promise, api.ErDesignReadCancelled); assert.equal(task.cancelQueued(), true); await rejected; await settle(); assert.equal(calls, 0);
});

test('onStart/run 同步失败也释放槽位；不同物理端点互不阻塞', async () => {
  const api = harness(), pending = deferred();
  const badStart = api.scheduleErDesignRead('key', async () => ({}), () => { throw new Error('start failed'); });
  const badRun = api.scheduleErDesignRead('key', () => { throw new Error('send failed'); }, () => {});
  const next = api.scheduleErDesignRead('key', async () => ({ ok: true }), () => {});
  const results = await Promise.allSettled([badStart.promise, badRun.promise, next.promise]);
  assert.deepEqual(results.map(r => r.status), ['rejected', 'rejected', 'fulfilled']);
  const old = api.scheduleErDesignRead('old', () => pending.promise, () => {});
  assert.deepEqual(await api.scheduleErDesignRead('new', async () => ({ ok: true }), () => {}).promise, { ok: true }); pending.resolve({}); await old.promise;
});
