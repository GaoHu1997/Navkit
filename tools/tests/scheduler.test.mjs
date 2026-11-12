import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import { build } from 'esbuild';
import { webcrypto } from 'node:crypto';

const built = await build({ entryPoints: ['src/core/scheduler.ts'], bundle: true, platform: 'node', format: 'esm', write: false, logLevel: 'silent' });
const scheduler = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString('base64')}`);
const uuid = () => webcrypto.randomUUID();
const plain = value => JSON.parse(JSON.stringify(value));
const action = (deps = []) => ({ id: uuid(), kind: 'sql', depends_on: deps, template_ref: uuid(), approved_scope_revision: '' });
const job = () => ({ ...scheduler.newScheduledJob('2026-01-01T00:00:07.123Z'), actions: [action()] });
const record = input => ({ job: { ...plain(input), revision: uuid(), actions: input.actions.map(a => ({ ...plain(a), approved_scope_revision: 'a'.repeat(64) })) }, schedule_revision: 'b'.repeat(64), next_run_utc: null, queued: null, blocked_run_id: null, paused_reason: null });

for (const expression of ['* * * * *', '0 9 * * *', '1,10-20/5 */6 * * 0,7', '0 0 13 * 1', '0 0 */2 * 1', '15/10 0-23/2 1-31 * 0-7']) {
  test(`strict five-field Cron accepts ${expression}`, () => scheduler.validateCron5(expression));
}
for (const expression of ['* * * *', '* * * * * *', '@daily', '*/0 * * * *', '60 * * * *', '0 24 * * *', '0 0 0 * *', '0 0 * 13 *', '0 0 * * 8', '1,,2 * * * *', '0 0 * * 6-1', '1x * * * *', '0 0 * * MON']) {
  test(`invalid Cron is rejected without browser execution: ${expression}`, () => assert.throws(() => scheduler.validateCron5(expression)));
}
test('new jobs are disabled and have no automatic browser trigger', () => {
  const result = job(); assert.equal(result.enabled, false); assert.equal(result.execution_mode, 'app_online');
  assert.equal(result.schedule.anchor_utc, '2026-01-01T00:00:07.123Z'); scheduler.validateScheduledJob(result);
});
test('interval anchor accepts nanoseconds without a JavaScript round-trip', () => {
  const s = { kind: 'interval', seconds: 60, anchor_utc: '2026-01-01T00:00:07.123456789Z' };
  scheduler.validateScheduleSpec(s); assert.equal(s.anchor_utc, '2026-01-01T00:00:07.123456789Z');
});
for (const anchor of ['2026-02-30T00:00:00Z', '2026-01-01 00:00:00', '2026-01-01T00:00:00+08:00', '1969-01-01T00:00:00Z', '2026-01-01T25:00:00Z']) {
  test(`invalid or ambiguous interval anchor is rejected: ${anchor}`, () => assert.throws(() => scheduler.validateScheduleSpec({ kind: 'interval', seconds: 60, anchor_utc: anchor })));
}
test('calendar display keeps explicit IANA and fixed DST policy', () => {
  const s = { kind: 'calendar', cron: '30 1 * * *', timezone: 'America/New_York', dst_gap: 'skip', dst_fold: 'first' };
  scheduler.validateScheduleSpec(s); assert.match(scheduler.scheduleDescription(s), /America\/New_York.*第一次/);
  assert.throws(() => scheduler.validateScheduleSpec({ ...s, timezone: 'China Standard Time' }));
  assert.throws(() => scheduler.validateScheduleSpec({ ...s, dst_fold: 'both' }));
});
test('DAG order respects prerequisites and keeps stable order for independent nodes', () => {
  const root = action(), second = action([root.id]), branch = action([root.id]), last = action([second.id, branch.id]);
  assert.deepEqual(scheduler.schedulerActionOrder([last, root, branch, second]), [root.id, branch.id, second.id, last.id]);
});
test('DAG rejects cycles, missing nodes, duplicate dependencies and self edges', () => {
  const a = action(), b = action([a.id]); a.depends_on = [b.id]; assert.throws(() => scheduler.schedulerActionOrder([a, b]), /包含环/);
  for (const deps of [[uuid()], [a.id], [b.id, b.id]]) assert.throws(() => scheduler.schedulerActionOrder([{ ...a, depends_on: deps }, b]));
});
test('deleting action removes dependencies and invalidates only affected approvals', () => {
  const a = action(), b = action([a.id]), c = action(); b.approved_scope_revision = c.approved_scope_revision = 'a'.repeat(64);
  const original = { ...job(), actions: [a, b, c] }; const changed = scheduler.removeSchedulerAction(original, a.id);
  assert.deepEqual(changed.actions[0].depends_on, []); assert.equal(changed.actions[0].approved_scope_revision, ''); assert.equal(changed.actions[1].approved_scope_revision, 'a'.repeat(64)); assert.equal(original.actions.length, 3);
});
test('64 action limit and exact integer budgets are enforced', () => {
  const draft = job(); draft.actions = Array.from({ length: 65 }, () => action()); assert.throws(() => scheduler.validateScheduledJob(draft));
  for (const timeout of [0, 1.5, 86_400_001, Number.NaN]) assert.throws(() => scheduler.validateScheduledJob({ ...job(), timeout_ms: timeout }));
});
test('save receipts accept reordered JSON keys but reject wrong identity or changed endpoint/template', () => {
  const draft = job(), result = record(draft); scheduler.assertSchedulerSaveReceipt(draft, result);
  result.job = Object.fromEntries(Object.entries(result.job).reverse()); scheduler.assertSchedulerSaveReceipt(draft, result);
  assert.throws(() => scheduler.assertSchedulerSaveReceipt(draft, { ...result, job: { ...result.job, id: uuid() } }));
  assert.throws(() => scheduler.assertSchedulerSaveReceipt(draft, { ...result, job: { ...result.job, actions: [{ ...result.job.actions[0], template_ref: uuid() }] } }));
});
test('unresolved outcomes and pending resources remain non-runnable', () => {
  const current = record(job()); assert.equal(scheduler.schedulerJobCanRun(current), true);
  assert.equal(scheduler.schedulerJobCanRun({ ...current, blocked_run_id: uuid() }), false);
  assert.equal(scheduler.schedulerJobCanRun({ ...current, paused_reason: 'SCHEDULE_SCOPE_CHANGED' }), false);
  assert.equal(scheduler.schedulerRunActive({ status: 'unknown', resources_pending: true }), true);
  assert.equal(scheduler.schedulerRunActive({ status: 'unknown', resources_pending: false }), false);
});
// ScheduleV2Dialog was removed by the workspace recovery. Its IPC/consent/save/dirty
// regressions now exercise production SchedulerPanel methods in scheduler-panel.test.mjs.
test('scheduler editor contains no browser due-job dispatcher or shell API', () => {
  const source = fs.readFileSync('src/ui/SchedulerPanel.ts', 'utf8');
  assert.doesNotMatch(source, /setInterval\s*\(|executeSql\s*\(|execute_sql|spawn\s*\(|schtasks|powershell/i);
  // setTimeout is intentionally allowed only for reading an already-active journal.
  assert.match(source, /bindings\.history\(id\)/);
});
