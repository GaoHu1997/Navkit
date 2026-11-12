import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
async function load(path) {
  const output = await build({ entryPoints: [path], bundle: true, platform: 'node', format: 'esm', write: false, logLevel: 'silent' });
  return import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);
}
const { ObjectCatalogController, objectKey } = await load('src/core/object-catalog.ts');
const { RoutineEditorSession } = await load('src/core/routine-editor.ts');
const fingerprint = 'a'.repeat(64), otherFingerprint = 'b'.repeat(64);
const object = { connection_id: 'c', catalog: 'db', schema: 'public', kind: 'function', name: 'f', identity_arguments: '', parent: null };
const scope = () => ({ connection_id: 'c', catalog: null, schema: 'public', kinds: ['function'] });
const item = (changes = {}) => { const target = { ...object, ...changes }; return { object: target, object_key: objectKey(target), display_name: target.name, actions: {}, properties: {} }; };
const page = (objects, next_cursor = null) => ({ objects, revision: 'directory-1', completeness: 'complete', sections: { function: { state: 'known', reason: null } }, next_cursor });
test('unknown initial catalog resolves once and continuation uses the real catalog', async () => {
  const calls = [], controller = new ObjectCatalogController(scope(), { list: async (request, cursor) => { calls.push([request, cursor]); return cursor ? page([item({ name: 'g' })]) : page([item()], 'cursor-1'); } });
  await controller.load(); await controller.load(true);
  assert.equal(calls[0][0].catalog, null); assert.equal(calls[1][0].catalog, 'db'); assert.equal(calls[1][1], 'cursor-1'); assert.equal(controller.objects.length, 2);
});
test('mixed physical catalogs reject the whole page and preserve unresolved scope', async () => {
  let mixed = true;
  const controller = new ObjectCatalogController(scope(), { list: async request => { assert.equal(request.catalog, null); return mixed ? page([item(), item({ catalog: 'foreign' })]) : page([item({ catalog: 'valid' })]); } });
  await assert.rejects(controller.load(), /OBJECT_CATALOG_MISMATCH/); assert.equal(controller.objects.length, 0);
  mixed = false; await controller.load(); assert.equal(controller.objects[0].object.catalog, 'valid');
});
test('a rejected foreign object cannot poison catalog resolution for a valid retry', async () => {
  let bad = true;
  const controller = new ObjectCatalogController(scope(), { list: async request => { assert.equal(request.catalog, null); return page([item(bad ? { connection_id: 'foreign', catalog: 'evil' } : { catalog: 'valid' })]); } });
  await assert.rejects(controller.load(), /OBJECT_IDENTITY_MISMATCH/); bad = false; await controller.load(); assert.equal(controller.objects[0].object.catalog, 'valid');
});
test('a resolved SQLite null catalog cannot become a different catalog on continuation', async () => {
  const controller = new ObjectCatalogController(scope(), { list: async (_, cursor) => cursor ? page([item({ name: 'g' })]) : page([item({ catalog: null })], 'next') });
  await controller.load(); await assert.rejects(controller.load(true), /OBJECT_IDENTITY_MISMATCH/); assert.equal(controller.objects.length, 1); assert.equal(controller.objects[0].object.catalog, null);
});
test('an empty first page does not invent a catalog and later identity still resolves', async () => {
  const calls = [], controller = new ObjectCatalogController(scope(), { list: async (request, cursor) => { calls.push(request.catalog); return cursor ? page([item()]) : page([], 'next'); } });
  await controller.load(); await controller.load(true); assert.deepEqual(calls, [null, null]); assert.equal(controller.objects[0].object.catalog, 'db');
});
test('catalog controller clones the caller scope before asynchronous reads', async () => {
  const input = scope(), calls = [], controller = new ObjectCatalogController(input, { list: async request => { calls.push(request); return page([item()]); } });
  input.connection_id = 'wrong'; input.kinds.push('trigger'); await controller.load(); assert.equal(calls[0].connection_id, 'c'); assert.deepEqual(calls[0].kinds, ['function']);
});
test('an explicitly selected catalog is never inferred from a foreign result', async () => {
  const controller = new ObjectCatalogController({ ...scope(), catalog: 'selected' }, { list: async () => page([item()]) });
  await assert.rejects(controller.load(), /OBJECT_IDENTITY_MISMATCH/); assert.equal(controller.objects.length, 0);
});
const snapshot = () => ({ object: { connection_id: 'c', schema: 'public', name: 'f', kind: 'function', identity_arguments: '' }, revision: 'open-revision', definition: 'original', source_fingerprint: fingerprint, completeness: 'complete', overloads: 1, backend_object_id: '12', creation_context: {}, security_completeness: 'complete', replacement_reason: '' });
function fixture() {
  const log = [];
  const port = {
    preview: async draft => { log.push(['preview', structuredClone(draft)]); return { object: structuredClone(draft.object), base_revision: draft.base_revision, source_fingerprint: draft.source_fingerprint, request_sql: 'ROUTINE_REPLACEMENT backend-marker', executable: true, warnings: [], steps: [] }; },
    issueGrant: async request => { log.push(['grant', structuredClone(request)]); return { ...request, execution_id: crypto.randomUUID(), execution_grant: 'real-fixture-grant' }; },
    execute: async (draft, request) => { log.push(['execute', structuredClone(draft), request]); return { outcome: result(request), receipt: receipt(draft) }; },
    cancelExecution: async request => { log.push(['cancel', request]); return { execution_id: request.execution_id, accepted: true, state: 'cancel_requested', message: 'requested' }; },
    executionStatus: async request => ({ execution_id: request.execution_id, state: 'unknown', resources_pending: true, deadline: {} }),
  };
  return { port, log };
}
function result(request, changes = {}) { return { execution_id: request.execution_id, state: 'succeeded', effects: 'completed', transaction_state: 'committed', resources_pending: false, result: null, error: null, deadline: {}, ...changes }; }
function receipt(draft, changes = {}) { return { status: 'succeeded', verified: true, transaction: 'committed', original_revision: draft.base_revision, revision: 'saved-revision', definition: draft.definition, mismatches: [], ...changes }; }
test('production routine session requires a C1 grant port before any preview', () => {
  const { port, log } = fixture(); assert.throws(() => new RoutineEditorSession(snapshot(), 'page', { ...port, issueGrant: undefined }), /C1授权/); assert.deepEqual(log, []);
});
test('verified routine save uses the immutable opening baseline and updates it only after readback', async () => {
  const { port, log } = fixture(), loaded = snapshot(), session = new RoutineEditorSession(loaded, 'page', port);
  loaded.revision = 'foreign-new'; loaded.source_fingerprint = otherFingerprint;
  assert.equal(await session.save('edited'), true); assert.equal(log[0][1].base_revision, 'open-revision'); assert.equal(log[0][1].source_fingerprint, fingerprint);
  assert.equal(log[2][2].owner_id, 'page'); assert.equal(log[2][2].sql, 'ROUTINE_REPLACEMENT backend-marker'); assert.equal(session.loadedSnapshot.revision, 'saved-revision');
  await session.save('second edit'); assert.equal(log[3][1].base_revision, 'saved-revision'); assert.equal(session.effectsUnknown, false); session.dispose();
});
test('changed preview source is rejected before grant or execute and preserves opening evidence', async () => {
  const { port, log } = fixture(), preview = port.preview; port.preview = async draft => ({ ...await preview(draft), source_fingerprint: otherFingerprint });
  const session = new RoutineEditorSession(snapshot(), 'page', port); await assert.rejects(session.save('edit'), /ROUTINE_SOURCE_CHANGED/);
  assert.deepEqual(log.map(row => row[0]), ['preview']); assert.equal(session.effectsUnknown, false); assert.equal(session.loadedSnapshot.source_fingerprint, fingerprint); session.dispose();
});
test('a confirmed no_effect failure allows a new explicit attempt without inventing unknown writes', async () => {
  const { port, log } = fixture(); port.execute = async (draft, request) => { log.push(['execute', draft, request]); return { outcome: result(request, { state: 'failed', effects: 'no_effect', transaction_state: 'none', error: { message: 'read only refusal', code: 'READ_ONLY_VIOLATION', retryable: false } }), receipt: null }; };
  const session = new RoutineEditorSession(snapshot(), 'page', port); await assert.rejects(session.save('edit'), /read only refusal/); assert.equal(session.effectsUnknown, false); assert.equal(session.busy, false);
  await assert.rejects(session.save('explicit retry'), /read only refusal/); assert.equal(log.filter(row => row[0] === 'preview').length, 2); session.dispose();
});
test('success without a verified receipt remains unknown even when effects claim no_effect', async () => {
  const { port, log } = fixture(); port.execute = async (_, request) => ({ outcome: result(request, { effects: 'no_effect' }), receipt: null });
  const session = new RoutineEditorSession(snapshot(), 'page', port); await assert.rejects(session.save('edit'), /完整提交/); assert.equal(session.effectsUnknown, true);
  await assert.rejects(session.save('retry'), /禁止重放/); assert.equal(log.filter(row => row[0] === 'preview').length, 1); session.dispose();
});
test('a receipt from a different definition revision cannot clear pending edits', async () => {
  const { port } = fixture(); port.execute = async (draft, request) => ({ outcome: result(request), receipt: receipt(draft, { original_revision: 'different' }) });
  const session = new RoutineEditorSession(snapshot(), 'page', port); await assert.rejects(session.save('draft'), /完整提交/); assert.equal(session.effectsUnknown, true); assert.equal(session.loadedSnapshot.definition, 'original'); assert.equal(session.capture().pending_draft.definition, 'draft'); session.dispose();
});
test('lost commit acknowledgement blocks replay and retains the exact submitted draft', async () => {
  const { port, log } = fixture(); port.execute = async (draft, request) => { log.push(['execute', draft, request]); throw new Error('lost commit acknowledgement'); };
  const session = new RoutineEditorSession(snapshot(), 'page', port); await assert.rejects(session.save('submitted SQL'), /lost commit/); assert.equal(session.effectsUnknown, true);
  await assert.rejects(session.save('repeat')); assert.equal(log.filter(row => row[0] === 'execute').length, 1); assert.equal(session.capture().pending_draft.definition, 'submitted SQL'); session.dispose();
});
test('stop during routine preview performs zero grant, cancel and execute IPC', async () => {
  const { port, log } = fixture(); let release;
  port.preview = draft => new Promise(resolve => { log.push(['preview', draft]); release = () => resolve({ object: draft.object, base_revision: draft.base_revision, source_fingerprint: fingerprint, request_sql: 'ROUTINE_REPLACEMENT marker', executable: true, warnings: [] }); });
  const session = new RoutineEditorSession(snapshot(), 'page', port), pending = session.save('edit'); await session.stop(); release(); await assert.rejects(pending, /准备已停止/); assert.deepEqual(log.map(row => row[0]), ['preview']); assert.equal(session.effectsUnknown, false); session.dispose();
});
test('a pre-grant rejection retains the baseline but does not require unknown-result recovery', async () => {
  const { port, log } = fixture(); port.issueGrant = async () => { log.push(['grant']); throw new Error('endpoint changed before signing'); };
  const session = new RoutineEditorSession(snapshot(), 'page', port); await assert.rejects(session.save('edit'), /endpoint changed/); assert.equal(session.effectsUnknown, false); assert.equal(session.busy, false); assert.equal(session.loadedSnapshot.revision, 'open-revision'); session.dispose();
});
