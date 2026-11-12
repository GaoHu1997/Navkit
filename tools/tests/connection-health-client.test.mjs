import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';

const output = await build({ entryPoints: ['src/core/connection-health-client.ts'], bundle: true, platform: 'node', format: 'esm', write: false, logLevel: 'silent' });
const { ConnectionHealthMonitor } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const settle = () => new Promise(done => setImmediate(done));
const health = (patch = {}) => ({ connection_id: 'conn', generation: 'generation-1', state: 'healthy', enabled: true, resources_pending: false, interval_secs: 180, timeout_secs: 5, next_due_in_ms: 180000, probe_skipped_reason: null, last_error: null, ...patch });
function fixture() {
  const timers = new Map(), calls = [], offline = []; let sequence = 0;
  const port = { view: async id => { calls.push(['view', id]); return health(); }, check: async id => { calls.push(['check', id]); return health(); }, offline: (id, message) => offline.push([id, message]), schedule(callback, delay) { const id = ++sequence; timers.set(id, { callback, delay }); return id; }, cancel(id) { timers.delete(id); } };
  const monitor = new ConnectionHealthMonitor(port);
  const fire = () => { assert.equal(timers.size, 1); const [id, timer] = timers.entries().next().value; timers.delete(id); timer.callback(); return timer.delay; };
  return { monitor, port, timers, calls, offline, fire };
}

test('starts with metadata only and respects a three-minute server cadence', async () => {
  const f = fixture(); f.monitor.start('conn'); await settle();
  assert.deepEqual(f.calls, [['view', 'conn']]); assert.equal([...f.timers.values()][0].delay, 180000);
  f.fire(); await settle(); assert.deepEqual(f.calls, [['view', 'conn'], ['check', 'conn']]); assert.equal(f.timers.size, 1); f.monitor.stopAll(); assert.equal(f.timers.size, 0);
});
test('duplicate starts and an in-flight probe cannot overlap requests', async () => {
  const f = fixture(), pending = deferred(); f.port.check = id => { f.calls.push(['check', id]); return pending.promise; };
  f.monitor.start('conn'); f.monitor.start('conn'); await settle(); f.fire(); f.monitor.start('conn'); await settle();
  assert.equal(f.timers.size, 0); assert.equal(f.calls.filter(c => c[0] === 'check').length, 1);
  pending.resolve(health()); await settle(); assert.equal(f.timers.size, 1); f.monitor.stopAll();
});
test('pending resource cleanup is observed with metadata and never sends another probe', async () => {
  const f = fixture(); f.port.view = async id => { f.calls.push(['view', id]); return health({ resources_pending: true, next_due_in_ms: null }); };
  f.monitor.start('conn'); await settle(); assert.equal(f.fire(), 5000); await settle(); assert.equal(f.fire(), 5000); await settle();
  assert.equal(f.calls.filter(c => c[0] === 'check').length, 0); assert.equal(f.calls.length, 3); f.monitor.stopAll();
});
test('a busy endpoint uses bounded retry cadence without a tight loop', async () => {
  const f = fixture(); f.port.view = async () => health({ probe_skipped_reason: 'manual_transaction', next_due_in_ms: 0 });
  f.monitor.start('conn'); await settle(); assert.equal([...f.timers.values()][0].delay, 1000); f.monitor.stopAll();
});
for (const patch of [{ enabled: false }, { probe_skipped_reason: 'unsupported' }, { probe_skipped_reason: 'disabled' }]) {
  test(`disabled or unsupported connections schedule no SQL ${JSON.stringify(patch)}`, async () => {
    const f = fixture(); f.port.view = async () => health(patch); f.monitor.start('conn'); await settle(); assert.equal(f.timers.size, 0); assert.equal(f.offline.length, 0); f.monitor.stopAll();
  });
}
test('an offline result clears the watch and reports failure once', async () => {
  const f = fixture(); f.port.check = async () => health({ state: 'offline', last_error: 'bounded failure' });
  f.monitor.start('conn'); await settle(); f.fire(); await settle(); assert.deepEqual(f.offline, [['conn', 'bounded failure']]); assert.equal(f.timers.size, 0); f.monitor.stopAll();
});
test('stopping and restarting cannot let the previous generation change the new timer', async () => {
  const f = fixture(), pending = deferred(); let views = 0;
  f.port.view = async () => ++views === 1 ? pending.promise : health({ generation: 'generation-2', next_due_in_ms: 60000 });
  f.monitor.start('conn'); f.monitor.stop('conn'); f.monitor.start('conn'); await settle();
  pending.resolve(health({ state: 'offline' })); await settle(); assert.equal(f.offline.length, 0); assert.equal(f.timers.size, 1); assert.equal([...f.timers.values()][0].delay, 60000); f.monitor.stopAll();
});
test('a server generation change never reuses an old timer or emits a stale failure', async () => {
  const f = fixture(); f.port.check = async () => health({ generation: 'generation-2', state: 'offline' }); f.monitor.start('conn'); await settle(); f.fire(); await settle(); assert.equal(f.timers.size, 0); assert.equal(f.offline.length, 0);
});
test('disconnecting during a probe prevents late acknowledgements from scheduling work', async () => {
  const f = fixture(), pending = deferred(); f.port.check = () => pending.promise; f.monitor.start('conn'); await settle(); f.fire(); f.monitor.stopAll(); pending.resolve(health()); await settle(); assert.equal(f.timers.size, 0); assert.equal(f.offline.length, 0);
});
for (const patch of [{ connection_id: 'other' }, { generation: '' }, { enabled: 'true' }, { resources_pending: 'false' }, { interval_secs: 9 }, { timeout_secs: 61 }, { interval_secs: 10, timeout_secs: 11 }, { next_due_in_ms: null }, { next_due_in_ms: -1 }, { next_due_in_ms: Infinity }, { next_due_in_ms: 3600001 }, { state: 'connected' }]) {
  test(`invalid server health proof cannot schedule SQL ${JSON.stringify(patch)}`, async () => {
    const f = fixture(); f.port.view = async () => health(patch); f.monitor.start('conn'); await settle(); assert.equal(f.timers.size, 0); assert.equal(f.offline.length, 0); f.monitor.stopAll();
  });
}
test('IPC failure stops polling until a deliberate connection start', async () => {
  const f = fixture(); f.port.view = async () => { throw new Error('disconnected IPC'); }; f.monitor.start('conn'); await settle(); assert.equal(f.timers.size, 0); assert.equal(f.offline.length, 0);
  f.port.view = async () => health(); f.monitor.start('conn'); await settle(); assert.equal(f.timers.size, 1); f.monitor.stopAll();
});
