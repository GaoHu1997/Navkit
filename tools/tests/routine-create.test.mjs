import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const output = await build({ entryPoints: ['src/core/routine-create.ts'], bundle: true, platform: 'node', format: 'esm', write: false, logLevel: 'silent' });
const { RoutineCreationSession } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);
const fp = 'a'.repeat(64), otherFp = 'b'.repeat(64);
const object = () => ({ connection_id: 'conn', schema: 'public', kind: 'function', name: 'fresh_name', identity_arguments: '' });
const baseline = (changes = {}) => ({ baseline_id: crypto.randomUUID(), object: object(), source_fingerprint: fp, base_revision: 'absence-revision', completeness: 'complete', absent: true, expires_in_ms: 120000, strategy: 'create_transactional', warnings: [], ...changes });
const outcome = (request, changes = {}) => ({ execution_id: request.execution_id, state: 'succeeded', effects: 'completed', transaction_state: 'committed', resources_pending: false, result: null, error: null, deadline: {}, ...changes });
function snapshot(draft) { return { object: structuredClone(draft.object), source_fingerprint: draft.source_fingerprint, completeness: 'complete', overloads: 1, revision: 'created-revision', definition: draft.definition, backend_object_id: 'real-object', creation_context: {}, security_completeness: 'complete', replacement_reason: '' }; }
function receipt(draft, transaction = 'committed') { return { baseline_id: draft.baseline_id, object: structuredClone(draft.object), source_fingerprint: draft.source_fingerprint, status: 'applied', verified: true, transaction, steps: [{ label: 'create', sql: draft.definition, status: 'applied', error: null }], snapshot: snapshot(draft), warnings: [] }; }
function fixture(base = baseline()) {
  const calls = [];
  const preview = draft => ({ baseline_id: draft.baseline_id, object: structuredClone(draft.object), source_fingerprint: draft.source_fingerprint, base_revision: draft.base_revision, steps: [{ label: 'create', sql: draft.definition }], request_sql: `ROUTINE_CREATION ${'c'.repeat(64)}`, strategy: base.strategy, executable: true, expires_in_ms: 100000, warnings: [] });
  const port = {
    preview: async draft => { calls.push(['preview', structuredClone(draft)]); return preview(draft); },
    issueGrant: async request => { calls.push(['grant', structuredClone(request)]); return { ...request, execution_id: crypto.randomUUID(), execution_grant: 'fixture-grant' }; },
    execute: async (draft, request) => { calls.push(['execute', structuredClone(draft), structuredClone(request)]); return { outcome: outcome(request), receipt: receipt(draft, base.strategy === 'create_transactional' ? 'committed' : 'implicit_commit_acknowledged') }; },
    cancelExecution: async request => { calls.push(['cancel', request]); return { execution_id: request.execution_id, accepted: true, state: 'cancel_requested', message: 'requested' }; },
    executionStatus: async request => ({ execution_id: request.execution_id, state: 'unknown', resources_pending: true, deadline: {} }),
  };
  return { base, calls, port, preview };
}
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
test('create requires a complete absence proof, exact zero signature and mandatory C1 grant', () => {
  const f = fixture();
  for (const bad of [{ absent: false }, { completeness: 'unknown' }, { source_fingerprint: null }, { expires_in_ms: 0 }, { expires_in_ms: 120001 }, { object: { ...object(), identity_arguments: null } }, { strategy: 'unchecked' }]) assert.throws(() => new RoutineCreationSession(baseline(bad), 'page', f.port));
  assert.throws(() => new RoutineCreationSession(f.base, 'page', { ...f.port, issueGrant: undefined }), /C1授权/); assert.deepEqual(f.calls, []);
});
test('create freezes the opening absence proof and uses the server issued execution identity', async () => {
  const f = fixture(), session = new RoutineCreationSession(f.base, 'page', f.port); f.base.base_revision = 'foreign'; f.base.object.name = 'wrong';
  assert.equal(await session.save('CREATE FUNCTION fresh_name() RETURNS INT AS $$ SELECT 1 $$ LANGUAGE SQL'), true);
  const draft = f.calls[0][1], request = f.calls[2][2]; assert.equal(draft.base_revision, 'absence-revision'); assert.equal(draft.object.name, 'fresh_name'); assert.equal(request.owner_id, 'page'); assert.match(request.sql, /^ROUTINE_CREATION [a-f0-9]{64}$/); assert.equal(request.execution_grant, 'fixture-grant'); assert.notEqual(request.execution_id, f.calls[1][1].execution_id);
  assert.equal(session.createdSnapshot.revision, 'created-revision'); await assert.rejects(session.save('replay'), /不能重复执行CREATE/); assert.equal(f.calls.filter(c => c[0] === 'execute').length, 1); session.dispose();
});
for (const [label, mutate] of [['source', p => ({ ...p, source_fingerprint: otherFp })], ['object', p => ({ ...p, object: { ...p.object, name: 'foreign' } })], ['revision', p => ({ ...p, base_revision: 'foreign' })], ['marker', p => ({ ...p, request_sql: 'CREATE FUNCTION arbitrary' })], ['baseline', p => ({ ...p, baseline_id: crypto.randomUUID() })]]) {
  test(`changed preview ${label} rejects before grant and write`, async () => {
    const f = fixture(); f.port.preview = async draft => { f.calls.push(['preview', draft]); return mutate(f.preview(draft)); };
    const session = new RoutineCreationSession(f.base, 'page', f.port); await assert.rejects(session.save('definition')); assert.deepEqual(f.calls.map(c => c[0]), ['preview']); assert.equal(session.effectsUnknown, false); session.dispose();
  });
}
test('a preview failure preserves a dirty draft and allows an explicit retry without renewing its proof', async () => {
  const f = fixture(); let fail = true; const original = f.port.preview; f.port.preview = async draft => { if (fail) throw new Error('database offline'); return original(draft); };
  const session = new RoutineCreationSession(f.base, 'page', f.port); await assert.rejects(session.save('draft'), /database offline/); assert.equal(session.needsRecheck, false); assert.equal(session.effectsUnknown, false); assert.equal(f.calls.length, 0);
  fail = false; assert.equal(await session.save('draft'), true); assert.equal(f.calls[0][1].baseline_id, f.base.baseline_id); session.dispose();
});
test('an explicit no-effect execution failure cannot silently reuse a potentially consumed proof', async () => {
  const f = fixture(); f.port.execute = async (_, request) => { f.calls.push(['execute']); return { outcome: outcome(request, { state: 'failed', effects: 'no_effect', transaction_state: 'rolled_back', error: { message: 'rolled back', code: 'ROUTINE_CREATION_ROLLED_BACK' } }), receipt: null }; };
  const session = new RoutineCreationSession(f.base, 'page', f.port); await assert.rejects(session.save('draft'), /rolled back/); assert.equal(session.effectsUnknown, false); assert.equal(session.needsRecheck, true); await assert.rejects(session.save('retry'), /重新核对目标/); assert.equal(f.calls.filter(c => c[0] === 'execute').length, 1); session.dispose();
});
test('lost commit acknowledgement retains evidence and blocks replay until explicit real target recheck', async () => {
  const f = fixture(); f.port.execute = async draft => { f.calls.push(['execute', draft]); throw new Error('lost commit acknowledgement'); };
  const session = new RoutineCreationSession(f.base, 'page', f.port); await assert.rejects(session.save('submitted SQL'), /lost commit/); assert.equal(session.effectsUnknown, true); assert.equal(session.capture().pending_draft.definition, 'submitted SQL');
  await assert.rejects(session.save('replay'), /禁止重放/); assert.equal(f.calls.filter(c => c[0] === 'execute').length, 1); session.dispose();
});
for (const [label, mutate] of [['missing', () => null], ['foreign source', r => ({ ...r, source_fingerprint: otherFp })], ['foreign snapshot', r => ({ ...r, snapshot: { ...r.snapshot, object: { ...r.snapshot.object, name: 'foreign' } } })], ['unverified', r => ({ ...r, verified: false })], ['wrong transaction', r => ({ ...r, transaction: 'implicit_commit_acknowledged' })]]) {
  test(`success with ${label} receipt stays unknown without a created snapshot`, async () => {
    const f = fixture(); f.port.execute = async (draft, request) => ({ outcome: outcome(request), receipt: mutate(receipt(draft)) }); const session = new RoutineCreationSession(f.base, 'page', f.port);
    await assert.rejects(session.save('draft'), /完整提交/); assert.equal(session.effectsUnknown, true); assert.equal(session.createdSnapshot, null); await assert.rejects(session.save('replay'), /禁止重放/); session.dispose();
  });
}
test('MySQL creation accepts only its explicit implicit commit acknowledgement', async () => {
  const f = fixture(baseline({ strategy: 'create_implicit_commit' })), session = new RoutineCreationSession(f.base, 'page', f.port); assert.equal(await session.save('CREATE FUNCTION fresh_name() RETURNS INT RETURN 1'), true); assert.equal(session.createdSnapshot.definition.includes('RETURN 1'), true); session.dispose();
});
test('stop during preview sends neither grant nor execution nor cancel IPC', async () => {
  const f = fixture(), ready = deferred(); f.port.preview = draft => { f.calls.push(['preview', draft]); return ready.promise; }; const session = new RoutineCreationSession(f.base, 'page', f.port), pending = session.save('draft');
  await session.stop(); ready.resolve(f.preview(f.calls[0][1])); await assert.rejects(pending, /准备已停止/); assert.deepEqual(f.calls.map(c => c[0]), ['preview']); assert.equal(session.effectsUnknown, false); session.dispose();
});
test('stop during grant freezes the signed payload and asks C1 to cancel before sending SQL', async () => {
  const f = fixture(), ready = deferred(), started = deferred(); f.port.issueGrant = request => { f.calls.push(['grant', request]); started.resolve(); return ready.promise; };
  f.port.execute = async (_, request) => { f.calls.push(['execute', request]); assert.equal(request.cancel_before_send, true); return { outcome: outcome(request, { state: 'cancelled', effects: 'no_effect', transaction_state: 'none', error: { message: 'cancelled' } }), receipt: null }; };
  const session = new RoutineCreationSession(f.base, 'page', f.port), pending = session.save('draft'); await started.promise; await session.stop(); const request = f.calls.find(c => c[0] === 'grant')[1]; ready.resolve({ ...request, execution_id: crypto.randomUUID(), execution_grant: 'grant' }); await assert.rejects(pending, /cancelled/); assert.equal(f.calls.filter(c => c[0] === 'cancel').length, 0); assert.equal(session.effectsUnknown, false); session.dispose();
});
test('only an explicitly obtained proof for the same endpoint and object can clear a consumed baseline', () => {
  const f = fixture(), session = new RoutineCreationSession(f.base, 'page', f.port); assert.throws(() => session.replaceBaseline(baseline({ source_fingerprint: otherFp })), /另一例程或端点/); assert.throws(() => session.replaceBaseline(baseline({ object: { ...object(), name: 'foreign' } })), /另一例程或端点/);
  const next = baseline({ base_revision: 'fresh-absence' }); session.replaceBaseline(next); assert.equal(session.loadedBaseline.base_revision, 'fresh-absence'); assert.equal(session.needsRecheck, false); assert.equal(session.capture().pending_draft, null); session.dispose();
});
