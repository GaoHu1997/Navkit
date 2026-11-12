// 真实监控采样与控制器，使用可控时钟和只读 API 样例；不执行真实数据库操作。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import { build, transform } from 'esbuild';

const root = fileURLToPath(new URL('../..', import.meta.url));
const built = await build({
  stdin: { contents: 'export { sampleServerMonitor, supportsProcessList, currentProcessSql, ServerMonitorController } from "./src/dialogs/ServerMonitorDialog.ts";', resolveDir: root, loader: 'ts' },
  bundle: true, write: false, platform: 'node', format: 'cjs', loader: { '.css': 'empty' }, logLevel: 'silent',
});
function harness() {
  const timers = new Map(); let nextTimer = 0, time = 0;
  const module = { exports: {} };
  const context = vm.createContext({ module, exports: module.exports, Error, Date,
    performance: { now: () => ++time },
    setTimeout(callback, delay) { const id = ++nextTimer; timers.set(id, { callback, delay }); return id; },
    clearTimeout(id) { timers.delete(id); },
  });
  vm.runInContext(built.outputFiles[0].text, context);
  return { ...module.exports, timers,
    async settle() { for (let index = 0; index < 12; index++) await Promise.resolve(); },
    tick() { const [id, timer] = timers.entries().next().value; timers.delete(id); timer.callback(); },
  };
}
const plain = value => JSON.parse(JSON.stringify(value));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const connection = (kind = 'mysql') => ({ id: 'c1', name: '本地连接', kind });
const process = (patch = {}) => ({ id: '941', user: 'app', host: 'localhost:52346', db: 'navkit_accept', command: 'Query', state: 'executing', elapsed_secs: 12, sql: 'SELECT\r\n  id, username\nFROM t_user;', is_self: false, ...patch });
const snapshot = (id = 'c1') => ({ connectionId: id, capturedAt: id, elapsedMs: 1, processes: [], processError: null });

test('协议目录决定是否有会话概念，兼容协议和自定义协议沿用既有目录', () => {
  const h = harness();
  for (const kind of ['mysql', 'mariadb', 'custom_mysql', 'postgresql', 'custom_pg', 'sqlserver', 'clickhouse', 'oracle']) assert.equal(h.supportsProcessList(kind), true, kind);
  for (const kind of ['sqlite', 'h2', 'redis', 'mongo', 'unknown-kind']) assert.equal(h.supportsProcessList(kind), false, kind);
});
test('空闲会话不显示数据库返回的上一条 SQL，仍保留完整的活动原文', () => {
  const h = harness(), row = process();
  assert.equal(h.currentProcessSql(row), row.sql);
  for (const command of ['Sleep', 'sleeping', '空闲', '事务中空闲', 'idle in transaction', 'idle in transaction (aborted)', 'AWAITING COMMAND', 'dormant']) assert.equal(h.currentProcessSql(process({ command, state: '' })), '', command);
  for (const state of ['idle', 'idle（等待 ClientRead）', 'idle in transaction', 'sleeping']) assert.equal(h.currentProcessSql(process({ state })), '', state);
  assert.equal(h.currentProcessSql(process({ sql: ' \r\n\t' })), '');
  assert.equal(h.currentProcessSql(process({ state: 'Waiting for table metadata lock' })), row.sql);
  assert.equal(h.currentProcessSql(process({ command: '正在执行', state: 'active（等待 Lock）' })), row.sql);
});
test('每次采样只读当前连接，会话 SQL 清理不修改源数据，也不请求任何额外指标', async () => {
  const h = harness(), calls = [], sourceRows = [process(), process({ id: '945', command: 'Sleep', state: '', elapsed_secs: 360, sql: 'SELECT FINISHED' })];
  const result = await h.sampleServerMonitor(connection(), {
    listDbProcesses: async (...args) => { calls.push(args); return sourceRows; },
    serverInfo: () => assert.fail('不读取基本信息'), executeSql: () => assert.fail('不执行指标 SQL'),
  });
  assert.deepEqual(calls, [['c1']]); assert.equal(result.connectionId, 'c1');
  assert.equal(result.processes.length, 2); assert.equal(result.processes[0].sql, sourceRows[0].sql);
  assert.equal(result.processes[1].sql, ''); assert.equal(result.processes[1].elapsed_secs, null);
  assert.equal(sourceRows[1].sql, 'SELECT FINISHED'); assert.equal(sourceRows[1].elapsed_secs, 360);
  assert.equal(result.processError, null); assert.ok(result.elapsedMs >= 0);
  assert.equal(result.server, undefined); assert.equal(result.metrics, undefined);
});
test('结束后的 SQL 从下一次采样移除，但对应连接不会被删除', async () => {
  const h = harness(); let row = process();
  const source = { listDbProcesses: async () => [row] };
  const first = await h.sampleServerMonitor(connection(), source);
  row = process({ command: '空闲', state: 'idle（等待 ClientRead）' });
  const second = await h.sampleServerMonitor(connection(), source);
  assert.equal(first.processes[0].sql, row.sql); assert.equal(second.processes.length, 1);
  assert.equal(second.processes[0].id, '941'); assert.equal(second.processes[0].sql, '');
});
test('不支持与权限失败不是零会话；没有接口时也就地返回真实错误', async () => {
  const h = harness();
  const unsupported = await h.sampleServerMonitor(connection('sqlite'), { listDbProcesses: () => assert.fail('嵌入式库不发请求') });
  assert.equal(unsupported.processes, null); assert.match(unsupported.processError, /不支持/);
  const denied = await h.sampleServerMonitor(connection(), { listDbProcesses: async () => { throw new Error('permission denied'); } });
  assert.equal(denied.processes, null); assert.equal(denied.processError, 'permission denied');
  const missing = await h.sampleServerMonitor(connection(), {});
  assert.equal(missing.processes, null); assert.match(missing.processError, /listDbProcesses/);
  const empty = await h.sampleServerMonitor(connection(), { listDbProcesses: async () => [] });
  assert.deepEqual(plain(empty.processes), []); assert.equal(empty.processError, null);
});
test('手动刷新不重叠，重复刷新只合并为一个后续请求', async () => {
  const h = harness(), gates = [deferred(), deferred()], received = []; let calls = 0;
  const controller = new h.ServerMonitorController(() => gates[calls++].promise, result => received.push(result.connectionId));
  const first = controller.refresh(); await controller.refresh(); await controller.refresh(); assert.equal(calls, 1);
  gates[0].resolve(snapshot('first')); await first; assert.equal(calls, 2);
  gates[1].resolve(snapshot('queued')); await h.settle(); assert.deepEqual(received, ['first', 'queued']); assert.equal(h.timers.size, 0); controller.dispose();
});
test('连接切换先使旧响应失效，等待在途读请求完成后才采样新连接', async () => {
  const h = harness(), old = deferred(), next = deferred(), requested = [], received = []; let id = 'old';
  const controller = new h.ServerMonitorController(() => { requested.push(id); return id === 'old' ? old.promise : next.promise; }, result => received.push(result.connectionId));
  controller.start(5); id = 'new'; controller.start(3);
  assert.deepEqual(requested, ['old']); old.resolve(snapshot('old')); await h.settle(); assert.deepEqual(requested, ['old', 'new']);
  next.resolve(snapshot('new')); await h.settle(); assert.deepEqual(received, ['new']);
  assert.deepEqual([...h.timers.values()].map(timer => timer.delay), [3000]); controller.dispose(); assert.equal(h.timers.size, 0);
});
test('暂停丢弃尚未完成的读响应和排队刷新，不取消后端 Promise，手动刷新仍可用', async () => {
  const h = harness(), gate = deferred(), received = []; let calls = 0;
  const controller = new h.ServerMonitorController(() => ++calls === 1 ? gate.promise : Promise.resolve(snapshot('manual')), result => received.push(result.connectionId));
  controller.start(5); await controller.refresh(); controller.stop(); gate.resolve(snapshot('old')); await h.settle();
  assert.deepEqual(received, []); assert.equal(calls, 1); assert.equal(h.timers.size, 0);
  await controller.refresh(); assert.deepEqual(received, ['manual']); assert.equal(h.timers.size, 0); controller.dispose();
});
test('关闭后迟到成功或失败都不发布、不再排队或安排计时器', async () => {
  for (const fail of [false, true]) {
    const h = harness(), gate = deferred(), received = []; let calls = 0;
    const controller = new h.ServerMonitorController(() => { calls++; return gate.promise; }, result => received.push(result));
    controller.start(2); await controller.refresh(); controller.dispose();
    fail ? gate.reject(new Error('late failure')) : gate.resolve(snapshot()); await h.settle(); await controller.refresh();
    assert.equal(calls, 1); assert.equal(received.length, 0); assert.equal(h.timers.size, 0);
  }
});
test('读错误保留错误语义，默认间隔和边界有效，下一次计时采样可恢复', async () => {
  const h = harness(), received = []; let fail = true;
  const controller = new h.ServerMonitorController(async () => { if (fail) throw new Error('offline'); return snapshot('recovered'); }, result => received.push(result));
  controller.start(NaN); await h.settle(); assert.equal(received[0].processError, 'offline'); assert.equal(received[0].processes, null);
  assert.deepEqual([...h.timers.values()].map(timer => timer.delay), [5000]);
  fail = false; h.tick(); await h.settle(); assert.equal(received[1].connectionId, 'recovered');
  controller.start(1); await h.settle(); assert.deepEqual([...h.timers.values()].map(timer => timer.delay), [2000]);
  controller.start(999); await h.settle(); assert.deepEqual([...h.timers.values()].map(timer => timer.delay), [300000]); controller.dispose();
});
test('局部样式只消费现有令牌，保持单表自然高度与按需全文，不引入大指标区或动画', async () => {
  const css = await readFile(new URL('../../src/styles/server-monitor-dialog.css', import.meta.url), 'utf8');
  const base = await readFile(new URL('../../src/styles/base.css', import.meta.url), 'utf8');
  const source = await readFile(new URL('../../src/dialogs/ServerMonitorDialog.ts', import.meta.url), 'utf8');
  assert.deepEqual((await transform(css, { loader: 'css', logLevel: 'silent' })).warnings, []);
  const tokens = new Set([...base.matchAll(/(--[\w-]+)\s*:/g)].map(match => match[1]));
  for (const match of css.matchAll(/var\((--[\w-]+)/g)) assert.ok(tokens.has(match[1]), match[1]);
  assert.doesNotMatch(css, /#[0-9a-f]{3,8}\b|@keyframes|transition\s*:/i);
  assert.doesNotMatch(source, /source\.serverInfo|source\.executeSql|role", "tab|monitorQuery/);
  // 窗口尺寸固定：会话行数变化不再让对话框缩放（用户要求）。
  assert.match(source, /new Dialog\(\{ title: "数据库监控", width: 1240, height: 640/);
  assert.match(source, /processes:\s*ProcessInfo\[\] \| null/); assert.match(source, /data/);
});
