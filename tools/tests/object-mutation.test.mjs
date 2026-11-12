import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const output = await build({ entryPoints: ['src/core/object-mutation.ts'], bundle: true, platform: 'node', format: 'esm', write: false, logLevel: 'silent' });
const { ObjectMutationSession, objectDropReason } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);
const sha = c => c.repeat(64);
const target = () => ({ connection_id: 'connection', catalog: null, schema: 'main', kind: 'trigger', name: 'audit', identity_arguments: null, parent: { schema: 'main', kind: 'table', name: 'items' } });
function fixture() {
  const calls = [];
  const baseline = { baseline_id: crypto.randomUUID(), object: target(), source_fingerprint: sha('a'), base_revision: sha('b'), backend_object_id: 'cookie/root/ddl', metadata_state: 'known', expires_in_ms: 120000 };
  const preview = d => { const id = crypto.randomUUID(), digest = sha('c'); return { plan_id: id, baseline_id: d.baseline_id, object: structuredClone(d.object), source_fingerprint: d.source_fingerprint, base_revision: d.base_revision, review_digest: digest, request_sql: `OBJECT_MUTATION_DROP ${id} ${digest}`, expires_in_ms: 100000, confirmation_required: true, transaction: 'transactional' }; };
  const port = {
    readObjectMutationBaseline: async (object, owner) => { calls.push(['read', object, owner]); return baseline; },
    previewObjectMutation: async (draft, owner) => { calls.push(['preview', draft, owner]); return preview(draft); },
    issueGrant: async request => { calls.push(['grant', request]); return { ...request, execution_id: crypto.randomUUID(), execution_grant: 'server-grant' }; },
    executeObjectMutation: async (request, work) => { calls.push(['execute', request, work]); return { outcome: { execution_id: request.execution_id, state: 'succeeded', effects: 'completed', transaction_state: 'committed', resources_pending: false, result: null, error: null, deadline: {} }, receipt: { plan_id: work.plan_id, baseline_id: baseline.baseline_id, object: target(), backend_object_id: baseline.backend_object_id, source_fingerprint: baseline.source_fingerprint, before_revision: baseline.base_revision, after_revision: sha('d'), status: 'applied', verified: true, transaction: 'committed', mismatches: [] } }; },
    cancelExecution: async context => { calls.push(['cancel', context]); return { execution_id: context.execution_id, accepted: true, state: 'cancel_requested', message: 'requested' }; },
    executionStatus: async context => ({ execution_id: context.execution_id, state: 'unknown', resources_pending: true, deadline: {} }),
  };
  return { calls, baseline, preview, port, session: new ObjectMutationSession(target(), 'page', port, () => {}) };
}
async function prepared(f) { await f.session.load(); await f.session.prepare(); }
const deferred = () => { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; };
test('native trigger requires complete parent/main identity and mandatory grant port', () => {
  const f = fixture();
  for (const change of [{ kind: 'function' }, { catalog: 'foreign' }, { schema: 'temp' }, { parent: null }, { identity_arguments: '' }, { name: 'sqlite_internal' }, { parent: { schema: 'main', kind: 'view', name: 'v' } }]) assert.ok(objectDropReason({ ...target(), ...change }, false));
  assert.ok(objectDropReason(target(), true));
  assert.throws(() => new ObjectMutationSession(target(), 'page', { ...f.port, issueGrant: undefined }, () => {}), /C1/);
  assert.deepEqual(f.calls, []); f.session.dispose();
});
test('baseline and preview copies cannot mutate authority; submission is once with server grant', async () => {
  const f = fixture(); await prepared(f);
  f.session.baseline.object.name = 'foreign'; f.session.preview.request_sql = 'DROP TABLE items';
  const response = await f.session.submit(true); assert.equal(response.receipt.verified, true);
  const [_, request, work] = f.calls.find(c => c[0] === 'execute'); assert.equal(work.draft.object.name, 'audit'); assert.match(request.sql, /^OBJECT_MUTATION_DROP /); assert.equal(request.execution_grant, 'server-grant');
  await assert.rejects(f.session.submit(true), /禁止重放/); assert.equal(f.calls.filter(c => c[0] === 'execute').length, 1); f.session.dispose();
});
for (const [label, mutate] of [['object', p => ({ ...p, object: { ...p.object, name: 'foreign' } })], ['parent', p => ({ ...p, object: { ...p.object, parent: { ...p.object.parent, name: 'foreign' } } })], ['source', p => ({ ...p, source_fingerprint: sha('d') })], ['baseline', p => ({ ...p, baseline_id: crypto.randomUUID() })], ['version', p => ({ ...p, base_revision: sha('d') })], ['SQL', p => ({ ...p, request_sql: 'DROP TRIGGER audit' })], ['confirmation', p => ({ ...p, confirmation_required: false })]]) {
  test(`preview ${label} mismatch never issues grant or deletion`, async () => {
    const f = fixture(); f.port.previewObjectMutation = async d => mutate(f.preview(d)); await f.session.load(); await assert.rejects(f.session.prepare()); assert.deepEqual(f.calls.map(c => c[0]), ['read']); assert.equal(f.session.locked, false); f.session.dispose();
  });
}
test('preview failure retains baseline and permits explicit preview retry, not silent reload', async () => {
  const f = fixture(); await f.session.load(); const base = f.session.baseline; f.port.previewObjectMutation = async () => { throw new Error('offline'); }; await assert.rejects(f.session.prepare(), /offline/);
  f.port.previewObjectMutation = async d => f.preview(d); await f.session.prepare(); assert.equal(f.session.baseline.baseline_id, base.baseline_id); assert.equal(f.calls.filter(c => c[0] === 'read').length, 1); f.session.dispose();
});
test('re-preview invalidates the previous review immediately and after a failed read', async () => {
  const f = fixture(); await prepared(f); const read = deferred(); f.port.previewObjectMutation = () => read.promise;
  const pending = f.session.prepare(); assert.equal(f.session.busy, true); assert.equal(f.session.preview, null); await assert.rejects(f.session.submit(true));
  read.resolve({ ...f.preview({ ...f.session.baseline, operation: 'drop' }), request_sql: 'bad SQL' }); await assert.rejects(pending);
  assert.equal(f.session.busy, false); assert.equal(f.session.preview, null); await assert.rejects(f.session.submit(true)); assert.deepEqual(f.calls.map(c => c[0]), ['read', 'preview']); f.session.dispose();
});
test('unchecked confirmation sends no grant and no SQL', async () => { const f = fixture(); await prepared(f); await assert.rejects(f.session.submit(false)); assert.deepEqual(f.calls.map(c => c[0]), ['read', 'preview']); f.session.dispose(); });
test('lost commit receipt locks replay and retains opening evidence even after resource status', async () => {
  const f = fixture(); await prepared(f); f.port.executeObjectMutation = async () => { f.calls.push(['execute']); throw new Error('lost acknowledgement'); };
  await assert.rejects(f.session.submit(true), /lost/); assert.equal(f.session.locked, true); assert.equal(f.session.execution.state, 'unknown'); assert.equal(f.session.baseline.object.name, 'audit'); await assert.rejects(f.session.submit(true)); await assert.rejects(f.session.load()); assert.equal(f.calls.filter(c => c[0] === 'execute').length, 1); f.session.dispose();
});
for (const [label, mutate] of [['missing', r => ({ ...r, receipt: null })], ['wrong parent', r => ({ ...r, receipt: { ...r.receipt, object: { ...r.receipt.object, parent: { schema: 'main', kind: 'table', name: 'other' } } } })], ['unverified', r => ({ ...r, receipt: { ...r.receipt, verified: false } })], ['resource pending', r => ({ ...r, outcome: { ...r.outcome, resources_pending: true } })], ['unknown', r => ({ ...r, outcome: { ...r.outcome, state: 'unknown', transaction_state: 'unknown' } })]]) {
  test(`success with ${label} proof locks rather than replaying`, async () => { const f = fixture(); await prepared(f); const original = f.port.executeObjectMutation; f.port.executeObjectMutation = async (...args) => mutate(await original(...args)); await assert.rejects(f.session.submit(true)); assert.equal(f.session.locked, true); await assert.rejects(f.session.submit(true)); f.session.dispose(); });
}
test('proved rollback still consumes plan and requires explicit fresh target review', async () => {
  const f = fixture(); await prepared(f); f.port.executeObjectMutation = async request => { f.calls.push(['execute']); return { outcome: { execution_id: request.execution_id, state: 'failed', effects: 'no_effect', transaction_state: 'rolled_back', resources_pending: false, result: null, error: { message: 'rolled back' }, deadline: {} }, receipt: null }; };
  await assert.rejects(f.session.submit(true), /rolled back/); assert.equal(f.session.locked, true); assert.equal(f.session.execution.busy, false); await assert.rejects(f.session.prepare()); f.session.dispose();
});
test('duplicate execution rejection is never converted into no-send', async () => {
  const f = fixture(); await prepared(f); f.port.executeObjectMutation = async () => { throw { code: 'EXECUTION_ID_DUPLICATE', message: 'same ID may already have applied' }; };
  try { await assert.rejects(f.session.submit(true)); assert.equal(f.session.locked, true); assert.equal(f.session.execution.state, 'unknown'); assert.equal(f.session.execution.busy, true); } finally { f.session.dispose(); }
});
test('explicit IPC no-registration proof releases controller rather than polling forever', async () => {
  const f = fixture(); await prepared(f); f.port.executeObjectMutation = async () => { throw { code: 'OBJECT_MUTATION_NOT_REGISTERED', message: 'expired plan; no SQL' }; };
  try { await assert.rejects(f.session.submit(true)); assert.equal(f.session.execution.state, 'failed'); assert.equal(f.session.execution.busy, false); assert.equal(f.session.locked, false); assert.equal(f.calls.filter(c => c[0] === 'execute').length, 0); } finally { f.session.dispose(); }
});
test('late grant context rejection is proven no-send, not an immortal unknown execution', async () => {
  const f = fixture(); await prepared(f); const grant = deferred(), started = deferred(); let alive = true;
  f.port.issueGrant = request => { started.resolve(request); return grant.promise; };
  f.port.executeObjectMutation = async () => { if (!alive) throw { code: 'OBJECT_MUTATION_NOT_REGISTERED', message: 'page changed before IPC' }; throw new Error('unexpected'); };
  const pending = f.session.submit(true); const request = await started.promise; alive = false; grant.resolve({ ...request, execution_id: crypto.randomUUID(), execution_grant: 'grant' });
  try { await assert.rejects(pending); assert.equal(f.session.execution.busy, false); assert.equal(f.session.execution.state, 'failed'); assert.equal(f.session.locked, false); } finally { f.session.dispose(); }
});
test('cancel during late grant is attached to exactly the signed typed request', async () => {
  const f = fixture(); await prepared(f); const grant = deferred(), started = deferred(); f.port.issueGrant = request => { started.resolve(request); return grant.promise; };
  f.port.executeObjectMutation = async request => { f.calls.push(['execute', request]); assert.equal(request.cancel_before_send, true); return { outcome: { execution_id: request.execution_id, state: 'cancelled', effects: 'no_effect', transaction_state: 'none', resources_pending: false, error: { message: 'stopped before SQL' }, deadline: {}, result: null }, receipt: null }; };
  const pending = f.session.submit(true); const request = await started.promise; await f.session.cancel(); grant.resolve({ ...request, execution_grant: 'late-grant', execution_id: crypto.randomUUID() }); await assert.rejects(pending, /stopped/); assert.equal(f.calls.filter(c => c[0] === 'execute').length, 1); assert.equal(f.calls.filter(c => c[0] === 'cancel').length, 0); f.session.dispose();
});
