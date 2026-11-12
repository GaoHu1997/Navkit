import test from 'node:test';
import assert from 'node:assert/strict';
import { createBatchTransport, flush, loadService, requestFor } from './batch-execution-fixture.mjs';

const { BatchExecutionService } = await loadService();
function harness(options = {}) {
  const source = createBatchTransport(), timers = new Map(); let sequence = 0;
  const service = new BatchExecutionService(source, { schedule(callback, delay) { assert.equal(delay, 400); timers.set(++sequence, callback); return sequence; }, clearSchedule(id) { timers.delete(id); }, ...options });
  return { source, service, timers, async tick() { const callbacks = [...timers.values()]; timers.clear(); callbacks.forEach(callback => callback()); await flush(); } };
}
const executionId = h => h.service.list()[0].context.execution_id;
const count = (h, method) => h.source.calls.filter(call => call[0] === method).length;
const log = (sequence, extra = {}) => ({ sequence, level: 'success', phase: 'execute', statement: sequence, row: sequence, message: 'RAW_DB_ERROR secret-token', sql_template: 'INSERT INTO items(id) VALUES (?)', ...extra });

test('延迟登记期间停止：得到 context 先 cancel，run 永远为零，ack 不结束任务', async () => {
  const h = harness(); h.source.mode = 'delayed-register'; const record = h.service.start(requestFor());
  assert.equal(record.registering, true); await h.service.cancel(record.id); assert.equal(count(h, 'run'), 0);
  h.source.releaseRegistration(); await flush();
  assert.deepEqual(h.source.calls.map(call => call[0]), ['register', 'cancel']); assert.equal(h.service.get(record.id).busy, true);
  await h.tick(); assert.equal(count(h, 'run'), 0); assert.equal(count(h, 'status'), 1);
  h.source.setSnapshot(executionId(h), { state: 'cancelled', pending: false, transaction: 'rolled_back' }); await h.tick();
  assert.equal(h.service.get(record.id).busy, false); assert.equal(h.timers.size, 0);
});

test('已运行任务的 terminal ack 与 pending 终态均不解锁；排空后才释放', async () => {
  const h = harness(), record = h.service.start(requestFor('generated_rows')); await flush();
  await h.service.cancel(record.id); assert.equal(h.service.get(record.id).busy, true); assert.equal(h.service.get(record.id).state, 'running');
  h.source.setSnapshot(executionId(h), { state: 'cancelled', pending: true, transaction: 'rolled_back' }); await h.tick();
  assert.equal(h.service.get(record.id).busy, true); assert.ok(h.timers.size);
  h.source.setSnapshot(executionId(h), { state: 'cancelled', pending: false, transaction: 'rolled_back', elapsed: 3000 }); await h.tick();
  assert.equal(h.service.get(record.id).busy, false); assert.equal(h.timers.size, 0); assert.equal(count(h, 'run'), 1);
});

test('run 传输异常只查原 context；unknown 迟到排空保持未知，绝不重放', async () => {
  const h = harness(); h.source.mode = 'run-reject'; const record = h.service.start(requestFor()); await flush();
  assert.equal(h.service.get(record.id).unknown, true); assert.equal(h.service.get(record.id).busy, true);
  h.source.setSnapshot(executionId(h), { state: 'unknown', pending: true, transaction: 'unknown' }); await h.tick();
  h.source.setSnapshot(executionId(h), { state: 'unknown', pending: false, transaction: 'unknown', elapsed: 4000 }); await h.tick();
  const result = h.service.get(record.id); assert.equal(result.unknown, true); assert.equal(result.busy, false);
  assert.equal(count(h, 'register'), 1); assert.equal(count(h, 'run'), 1); assert.equal(h.timers.size, 0);
  for (const call of h.source.calls.filter(call => call[0] === 'status')) assert.deepEqual(call[1], result.context);
  assert.match(result.notice, /RAW_DB_ERROR secret-token/);
});

for (const kind of ['sql_file', 'generated_rows']) test(`${kind} 模板保留，消息原文可见，400ms 增量去重不恢复已截断日志`, async () => {
  const h = harness({ logLimit: 2 }); const record = h.service.start(requestFor(kind)); await flush();
  h.source.setSnapshot(executionId(h), { logs: [log(1), log(2), log(3, { sql_template: "SELECT 'sensitive-value', 123 /* secret-token */" })] }); await h.tick();
  let result = h.service.get(record.id);
  assert.deepEqual(result.logs.map(item => item.sequence), [2, 3]); assert.equal(result.locallyDropped, 1);
  assert.match(result.logs[0].sql_template, /INSERT INTO items/); assert.equal(result.logs[0].message, 'RAW_DB_ERROR secret-token'); assert.doesNotMatch(result.logs[1].sql_template, /sensitive-value|secret-token/);
  // 模拟重复/迟到的完整增量包，不使用 fixture 的 afterSequence 过滤。
  h.source.batchExecutionStatus = async (context, cursor) => { h.source.calls.push(['status', context, cursor]); return h.source.tasks.get(context.execution_id).snapshot; };
  await h.tick(); result = h.service.get(record.id); assert.equal(result.locallyDropped, 1); assert.deepEqual(result.logs.map(item => item.sequence), [2, 3]);
  assert.equal(h.source.calls.filter(call => call[0] === 'status').at(-1)[2], 3);
});

test('后端已脱敏参数模板保留 PostgreSQL 绑定标记，不保存参数值', async () => {
  const h = harness(), record = h.service.start(requestFor('generated_rows')); await flush();
  h.source.setSnapshot(executionId(h), { logs: [log(1, { sql_template: 'INSERT INTO items (id) VALUES ($1)' })] }); await h.tick();
  assert.equal(h.service.get(record.id).logs[0].sql_template, 'INSERT INTO items (id) VALUES ($1)');
});

test('默认 300 条 / 96 KiB ring；后端截断序号保留，源大小来自 ticket', async () => {
  const h = harness(), record = h.service.start(requestFor()); await flush();
  h.source.setSnapshot(executionId(h), { logs: Array.from({ length: 350 }, (_, i) => log(i + 1, { sql_template: '表'.repeat(2048) })), dropped: 5 }); await h.tick();
  const result = h.service.get(record.id); assert.ok(result.logs.length <= 300); assert.ok(result.locallyDropped > 0);
  const bytes = result.logs.reduce((total, item) => total + (item.message.length + (item.sql_template?.length ?? 0) + 96) * 3, 0);
  assert.ok(bytes <= 96 * 1024); assert.equal(result.droppedBeforeSequence, 5); assert.equal(result.sourceBytes, 8192);
});

test('仅解除订阅不终止宿主；同一任务继续轮询且可停止', async () => {
  const h = harness(), record = h.service.start(requestFor()); await flush(); let updates = 0;
  const detach = h.service.subscribe(() => updates++); detach(); await h.tick(); await h.service.cancel(record.id);
  assert.equal(updates, 0); assert.equal(count(h, 'register'), 1); assert.equal(count(h, 'run'), 1); assert.equal(count(h, 'cancel'), 1);
});

test('迟到的 run 初始快照不能覆盖已经排空的终态', async () => {
  const h = harness(); h.source.mode = 'delayed-run'; const record = h.service.start(requestFor()); await flush();
  h.source.setSnapshot(executionId(h), { state: 'succeeded', pending: false, elapsed: 3000 }); await h.tick();
  h.source.releaseRun(); await flush(); assert.equal(h.service.get(record.id).busy, false); assert.equal(h.service.get(record.id).state, 'succeeded');
});

for (const invalid of ['context', 'count', 'kind']) test(`拒绝不可信 ${invalid} 快照且保持资源锁`, async () => {
  const h = harness(), record = h.service.start(requestFor()); await flush();
  const snapshot = h.source.setSnapshot(executionId(h), { state: 'succeeded', pending: false });
  if (invalid === 'context') snapshot.context.owner_id = 'other-owner';
  if (invalid === 'count') snapshot.affected_rows = -1;
  if (invalid === 'kind') snapshot.kind = 'generated_rows';
  await h.tick(); assert.equal(h.service.get(record.id).busy, true); assert.equal(h.service.get(record.id).state, 'running');
});

test('授权上下文不一致时 fail closed，不调用 run', async () => {
  const h = harness(), register = h.source.registerBatchExecution;
  h.source.registerBatchExecution = async request => ({ ...await register(request), namespace: 'other' });
  const record = h.service.start(requestFor()); await flush(); assert.equal(count(h, 'run'), 0); assert.equal(h.service.get(record.id).state, 'failed');
});

test('终态失败在顶部和日志显示数据库原文，并保留错误码/耗时', async () => {
  const h = harness(), record = h.service.start(requestFor()); await flush();
  h.source.setSnapshot(executionId(h), { state: 'failed', pending: false, transaction: 'rolled_back', elapsed: 30000,
    logs: [log(1, { level: 'error', message: 'RAW_DB_ERROR secret-token', elapsed_ms: 812, affected_rows: 0, error_code: '1062' })] });
  await h.tick();
  const result = h.service.get(record.id);
  assert.equal(result.state, 'failed'); assert.equal(result.busy, false);
  assert.match(result.notice, /执行失败/); assert.match(result.notice, /回滚已确认/);
  const entry = result.logs.find(item => item.sequence === 1);
  assert.equal(entry.error_code, '1062'); assert.equal(entry.elapsed_ms, 812); assert.equal(entry.message, 'RAW_DB_ERROR secret-token');
  assert.match(result.notice, /RAW_DB_ERROR secret-token/);
  assert.equal(result.snapshot.outcome.error.message, 'RAW_DB_ERROR secret-token');
});

test('超时终态明确报告执行时限与停止状态', async () => {
  const h = harness(), record = h.service.start(requestFor()); await flush();
  h.source.setSnapshot(executionId(h), { state: 'timed_out', pending: false, transaction: 'none', elapsed: 35000, budgetMs: 30000 });
  await h.tick();
  const result = h.service.get(record.id);
  assert.equal(result.state, 'timed_out'); assert.match(result.notice, /执行超时/); assert.match(result.notice, /30\.0 秒/);
});

test('finish 错误日志保留后端提供的真实错误原文', async () => {
  const h = harness(), record = h.service.start(requestFor()); await flush();
  h.source.setSnapshot(executionId(h), { state: 'failed', pending: false, transaction: 'rolled_back', elapsed: 30000,
    logs: [{ sequence: 1, level: 'error', phase: 'finish', statement: null, row: null, message: "任务失败；原因：Duplicate entry 'customer@example.com' for key 'users.email'", sql_template: null }] });
  await h.tick();
  const entry = h.service.get(record.id).logs.find(item => item.sequence === 1);
  assert.match(entry.message, /customer@example.com/); assert.match(entry.message, /users.email/); assert.doesNotMatch(entry.message, /数据库详情已隐藏/);
});
