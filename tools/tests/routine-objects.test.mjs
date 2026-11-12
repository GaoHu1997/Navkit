import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

async function module(path) {
  const output = await build({ entryPoints: [path], bundle: true, platform: 'node', format: 'esm', write: false, logLevel: 'silent' });
  return import(`data:text/javascript;base64,${Buffer.from(`${output.outputFiles[0].text}\n//# sourceURL=${path}.test.js`).toString('base64')}`);
}
const { objectKey, objectAction, ObjectCatalogController, RoutineEditorBaseline } = await module('src/core/object-catalog.ts');
const { parameterArgument, RoutineCallController } = await module('src/core/routine-call.ts');
const { RoutineCallDialog } = await module('src/dialogs/RoutineCallDialog.ts');
const object = { connection_id: 'conn', catalog: 'db', schema: 'public', kind: 'function', name: 'echo', identity_arguments: 'value text', parent: null };
const id = 'd1e3a677-3d33-4f5c-bc61-ce45d7c80431';
const fingerprint = 'a'.repeat(64), newFingerprint = 'b'.repeat(64);
const parameter = (type = 'text', other = {}) => ({ ordinal: 1, name: 'value', mode: 'in', data_type: type, type_schema: 'pg_catalog', type_name: type, has_default: false, variadic: false, ...other });
const metadata = (parameters = [parameter()], other = {}) => ({ object: structuredClone(object), backend_object_id: '123', revision: 'meta-a', source_fingerprint: fingerprint, parameters, result_type: 'text', definition_revision: 'def-a', completeness: 'complete', call: { level: 'limited', reason: 'typed binding' }, notes: [], ...other });
const outcome = (request, state = 'succeeded', pending = false) => ({ execution_id: request.execution_id, state, result: null, error: null, deadline: { execution_id: request.execution_id, budget_ms: null, elapsed_ms: 10, deadline_reached: false, stop_confirmed: false }, effects: state === 'unknown' ? 'unknown' : 'completed', transaction_state: 'none', resources_pending: pending });
function port(meta = metadata()) {
  const log = [];
  return {
    log, metadata: async target => { log.push(['metadata', target]); return structuredClone(meta); },
    prepare: async (draft, owner) => { log.push(['prepare', structuredClone(draft), owner]); return { plan_id: id, source_fingerprint: draft.source_fingerprint, metadata_revision: draft.metadata_revision, object: structuredClone(draft.object), sql_preview: 'SELECT "public"."echo"($1::pg_catalog.text)', parameter_count: 1, expires_in_ms: 120000, warnings: [] }; },
    issueGrant: async request => { log.push(['grant', structuredClone(request)]); return { ...request, execution_id: id, execution_grant: 'one-use-grant' }; },
    execute: async (plan, request) => { log.push(['execute', plan, structuredClone(request)]); return { outcome: outcome(request), call: { sets: [], outputs: [], warnings: [] } }; },
    cancelExecution: async context => { log.push(['cancel', context]); return { execution_id: context.execution_id, accepted: true, state: 'cancel_requested', message: 'requested only' }; },
    executionStatus: async context => ({ execution_id: context.execution_id, state: 'unknown', deadline: {}, resources_pending: true }),
  };
}
function descriptor(target = object) { return { object: structuredClone(target), object_key: objectKey(target), display_name: target.name, backend_object_id: null, owner: null, enabled: null, definition_readable: true, actions: { read: { level: 'supported', reason: '' } }, properties: {} }; }
const page = (items = [], revision = 'r1', next_cursor = null) => ({ objects: items, completeness: 'complete', sections: { function: { state: 'known', reason: null } }, revision, next_cursor });
const scope = { connection_id: 'conn', catalog: 'db', schema: 'public', kinds: ['function'] };

test('identity distinguishes schema, catalog, signature, parent and unknown versus zero args', () => {
  const variants = [object, { ...object, schema: 'other' }, { ...object, catalog: 'other' }, { ...object, identity_arguments: 'integer' }, { ...object, identity_arguments: '' }, { ...object, identity_arguments: null }, { ...object, parent: { schema: 'public', kind: 'table', name: 't' } }];
  assert.equal(new Set(variants.map(objectKey)).size, variants.length);
  assert.equal(objectKey(object), '["conn","db","public","function","echo","value text",null]');
});
test('catalog failed refresh retains old visible objects but never claims known-empty', async () => {
  let fail = false;
  const controller = new ObjectCatalogController(scope, { list: async () => { if (fail) throw new Error('permission denied'); return page([descriptor()]); } });
  await controller.load(); fail = true;
  await assert.rejects(controller.load(), /permission denied/);
  assert.equal(controller.objects.length, 1); assert.equal(controller.sections.function.state, 'unknown'); assert.equal(controller.busy, false);
});
test('catalog page continuation is bound to revision and refuses duplicate identity', async () => {
  let response = page([descriptor()], 'r1', 'cursor');
  const controller = new ObjectCatalogController(scope, { list: async (_, cursor) => { assert.equal(cursor, response.revision === 'r1' && response.next_cursor === 'cursor' && controller.objects.length === 0 ? null : 'cursor'); return response; } });
  await controller.load(); response = page([descriptor()], 'r1');
  await assert.rejects(controller.load(true), /DUPLICATE/);
  response = page([], 'r2'); await assert.rejects(controller.load(true), /STALE/);
  assert.equal(controller.objects.length, 1);
});
test('wrong connection/schema object cannot enter the catalog map', async () => {
  const controller = new ObjectCatalogController(scope, { list: async () => page([descriptor({ ...object, schema: 'evil' })]) });
  await assert.rejects(controller.load(), /IDENTITY_MISMATCH/); assert.equal(controller.objects.length, 0);
});
test('missing writer capability and read-only routine invocation are explicitly unsupported', () => {
  assert.equal(objectAction(descriptor(), 'drop', false).level, 'unsupported');
  const item = descriptor(); item.actions.call = { level: 'limited', reason: 'native' };
  assert.equal(objectAction(item, 'call', true).level, 'unsupported'); assert.equal(objectAction(item, 'read', true).level, 'supported');
});
test('editor saves against open-time immutable revision, not a newly fetched current revision', () => {
  const snapshot = { object: { connection_id: 'conn', schema: 'public', name: 'echo', kind: 'function', identity_arguments: 'value text' }, definition: 'old', revision: 'opened-revision', source_fingerprint: fingerprint, overloads: 1, completeness: 'complete' };
  const editor = new RoutineEditorBaseline(snapshot); snapshot.revision = 'server-new'; snapshot.source_fingerprint = newFingerprint; snapshot.object.name = 'different';
  assert.deepEqual(editor.draft('draft'), { object: { connection_id: 'conn', schema: 'public', name: 'echo', kind: 'function', identity_arguments: 'value text' }, base_revision: 'opened-revision', source_fingerprint: fingerprint, definition: 'draft', allow_drop_recreate: false });
  assert.throws(() => new RoutineEditorBaseline({ ...snapshot, overloads: 2 }), /单对象/);
  assert.throws(() => new RoutineEditorBaseline({ ...snapshot, completeness: 'unknown' }), /基线/);
});
test('editor requires the loaded endpoint proof and refuses legacy or malformed snapshots', () => {
  const snapshot = { object: { connection_id: 'conn', schema: 'public', name: 'echo', kind: 'function', identity_arguments: 'value text' }, definition: 'same definition', revision: 'same revision', overloads: 1, completeness: 'complete' };
  for (const source_fingerprint of [undefined, null, '', 'endpoint', 'A'.repeat(64)]) assert.throws(() => new RoutineEditorBaseline({ ...snapshot, source_fingerprint }), /ROUTINE_SOURCE_REQUIRED/);
  const old = new RoutineEditorBaseline({ ...snapshot, source_fingerprint: fingerprint }), current = new RoutineEditorBaseline({ ...snapshot, source_fingerprint: newFingerprint });
  assert.equal(old.draft('changed').source_fingerprint, fingerprint); assert.equal(current.draft('changed').source_fingerprint, newFingerprint); assert.equal(old.revision, current.revision);
});
for (const [type, raw, kind, value] of [
  ['bigint', '9223372036854775807', 'integer', '9223372036854775807'],
  ['numeric(38,20)', '001.000000000000123400', 'decimal', '001.000000000000123400'],
  ['text', '', 'text', ''], ['text', "'; DROP TABLE t; --中文😀", 'text', "'; DROP TABLE t; --中文😀"],
  ['bytea', '0x0041FF', 'binary', 'AEH/'], ['timestamptz', '2026-11-09T12:00:01.123+08:00', 'timestamptz', '2026-11-09T12:00:01.123+08:00'],
]) test(`routine typed input preserves ${type}`, () => assert.deepEqual(parameterArgument(parameter(type), { mode: 'value', text: raw }), { mode: 'value', value: { kind, value } }));
test('SQL NULL, empty text and omitted database DEFAULT are three different inputs', () => {
  assert.deepEqual(parameterArgument(parameter(), { mode: 'null', text: '' }), { mode: 'value', value: { kind: 'null' } });
  assert.deepEqual(parameterArgument(parameter(), { mode: 'value', text: '' }), { mode: 'value', value: { kind: 'text', value: '' } });
  assert.deepEqual(parameterArgument(parameter('text', { has_default: true }), { mode: 'default', text: '' }), { mode: 'default' });
  assert.throws(() => parameterArgument(parameter('text', { has_default: null }), { mode: 'default', text: '' }), /未知/);
  assert.throws(() => parameterArgument(parameter('text', { mode: 'out' }), { mode: 'value', text: '' }), /OUT/);
});
test('call preparation keeps actual page, exact object and open-time parameter metadata revision', async () => {
  const source = port(), controller = new RoutineCallController(object, 'page-a', source);
  await controller.load(); controller.setInput(1, { mode: 'value', text: "'; --not SQL" }); await controller.prepare();
  const prepared = source.log.find(item => item[0] === 'prepare');
  assert.equal(prepared[2], 'page-a'); assert.equal(prepared[1].source_fingerprint, fingerprint); assert.equal(prepared[1].metadata_revision, 'meta-a'); assert.deepEqual(prepared[1].arguments[0].argument.value, { kind: 'text', value: "'; --not SQL" });
  await controller.execute(5000);
  const sent = source.log.find(item => item[0] === 'execute'); assert.equal(sent[1], id); assert.equal(sent[2].sql, `ROUTINE_CALL_PLAN ${id}`); assert.equal(sent[2].execution_grant, 'one-use-grant'); assert.equal(sent[2].owner_id, 'page-a'); assert.equal(sent[2].timeout_ms, 5000); assert.equal(controller.busy, false); controller.dispose();
});
test('metadata without the loaded endpoint proof cannot prepare, grant or execute', async () => {
  for (const source_fingerprint of [undefined, null, '', 'endpoint']) {
    const source = port(metadata([], { source_fingerprint })), controller = new RoutineCallController(object, 'page-a', source);
    await assert.rejects(controller.load(), /ROUTINE_SOURCE_REQUIRED/); assert.equal(controller.metadata, null); assert.throws(() => controller.draft());
    assert.deepEqual(source.log.map(item => item[0]), ['metadata']); controller.dispose();
  }
});
test('identical routine revision from a changed endpoint cannot replace the loaded source during preview', async () => {
  const source = port(); source.prepare = async draft => { source.log.push(['prepare', structuredClone(draft)]); return { plan_id: id, metadata_revision: draft.metadata_revision, object: structuredClone(draft.object), source_fingerprint: newFingerprint }; };
  const controller = new RoutineCallController(object, 'page-a', source); await controller.load();
  await assert.rejects(controller.prepare(), /ROUTINE_SOURCE_CHANGED/); assert.equal(controller.preview, null);
  assert.equal(controller.draft().source_fingerprint, fingerprint); assert.deepEqual(source.log.map(item => item[0]), ['metadata', 'prepare']);
  await assert.rejects(controller.execute(), /先预览/); controller.dispose();
});
test('mutating an accepted preview endpoint cannot reach C1 grant or SQL execution', async () => {
  const source = port(), controller = new RoutineCallController(object, 'page-a', source); await controller.load(); await controller.prepare();
  controller.preview.source_fingerprint = newFingerprint; await assert.rejects(controller.execute(), /ROUTINE_SOURCE_CHANGED/);
  assert.deepEqual(source.log.map(item => item[0]), ['metadata', 'prepare']); controller.dispose();
});
test('source metadata is cloned at load and explicit reload is required to use a changed endpoint', async () => {
  const original = metadata(), source = port(), controller = new RoutineCallController(object, 'page-a', { ...source, metadata: async () => original });
  await controller.load(); original.source_fingerprint = newFingerprint; assert.equal(controller.draft().source_fingerprint, fingerprint);
  await controller.load(); assert.equal(controller.draft().source_fingerprint, newFingerprint); assert.equal(controller.draft().metadata_revision, 'meta-a'); controller.dispose();
});
test('changed input invalidates the previous one-use preview', async () => {
  const controller = new RoutineCallController(object, 'page-a', port()); await controller.load(); await controller.prepare(); controller.setInput(1, { mode: 'null', text: '' });
  await assert.rejects(controller.execute(), /先预览/); assert.equal(controller.preview, null); controller.dispose();
});
test('OUT inputs are omitted and INOUT must provide an initial typed value', async () => {
  const controller = new RoutineCallController(object, 'page-a', port(metadata([parameter('integer', { mode: 'inout' }), parameter('text', { ordinal: 2, mode: 'out' })])));
  await controller.load(); assert.throws(() => controller.draft()); controller.setInput(1, { mode: 'value', text: '42' });
  assert.deepEqual(controller.draft().arguments, [{ ordinal: 1, argument: { mode: 'value', value: { kind: 'integer', value: '42' } } }]); controller.dispose();
});
test('metadata identity mismatch or unreadable permissions never becomes a zero-parameter call', async () => {
  for (const source of [port(metadata([], { object: { ...object, name: 'wrong' } })), { ...port(), metadata: async () => { throw new Error('permission denied'); } }]) {
    const controller = new RoutineCallController(object, 'page-a', source); await assert.rejects(controller.load()); assert.equal(controller.metadata, null); assert.throws(() => controller.draft()); controller.dispose();
  }
});
test('read-only/unsupported metadata cannot be previewed and grant is mandatory', async () => {
  const controller = new RoutineCallController(object, 'page-a', port(metadata([], { call: { level: 'unsupported', reason: 'read only' } })));
  await controller.load(); await assert.rejects(controller.prepare(), /read only/); controller.dispose();
  assert.throws(() => new RoutineCallController(object, 'page-a', { ...port(), issueGrant: undefined }), /C1授权/);
});
test('wrong plan object cannot execute, even with apparently valid plan UUID', async () => {
  const source = port(); source.prepare = async draft => ({ plan_id: id, object: { ...object, schema: 'wrong' }, metadata_revision: draft.metadata_revision });
  const controller = new RoutineCallController(object, 'page-a', source); await controller.load(); await assert.rejects(controller.prepare(), /不匹配/); assert.equal(controller.preview, null); controller.dispose();
});
test('unknown outcome does not display late details or permit repeat execution while C1 drains', async () => {
  const source = port(); source.execute = async (_, request) => ({ outcome: outcome(request, 'unknown', true), call: { sets: [{ rows: [['unsafe late value']] }], outputs: [], warnings: [] } });
  const controller = new RoutineCallController(object, 'page-a', source); await controller.load(); await controller.prepare(); const result = await controller.execute();
  assert.equal(result.state, 'unknown'); assert.equal(controller.response, null); assert.equal(controller.busy, true); await assert.rejects(controller.prepare(), /进行/); controller.dispose();
});
test('requesting stop during grant carries C1 cancel-before-send, not a false completed cancellation', async () => {
  const source = port(); let release;
  source.issueGrant = request => new Promise(resolve => { release = () => resolve({ ...request, execution_id: id, execution_grant: 'grant' }); });
  source.execute = async (_, request) => { source.log.push(['execute', request]); return { outcome: outcome(request, 'cancelled', false), call: null }; };
  const controller = new RoutineCallController(object, 'page-a', source); await controller.load(); await controller.prepare(); const pending = controller.execute(); await controller.cancel();
  assert.equal(controller.execution.state, 'cancel_requested'); release(); await pending; assert.equal(source.log.find(item => item[0] === 'execute')[1].cancel_before_send, true); controller.dispose();
});
// Actual dialog methods execute with minimal DOM observations; this is not a browser/E2E claim.
function node() { return { children: [], attrs: {}, style: {}, textContent: '', hidden: false, disabled: false, focus() { this.focused = true; }, append(...items) { this.children.push(...items); }, replaceChildren(...items) { this.children = items; }, setAttribute(k, v) { this.attrs[k] = v; }, removeAttribute(k) { delete this.attrs[k]; } }; }
function withDocument(document, callback) { const old = globalThis.document; globalThis.document = document; try { return callback(); } finally { globalThis.document = old; } }
test('dialog keyboard method traps Tab/Shift-Tab and Escape requests close', () => {
  const first = node(), last = node(); let prevented = 0, closed = 0;
  const fixture = { dialog: { el: { querySelectorAll: () => [first, last], contains: () => true } }, close() { closed++; } };
  first.getAttribute = last.getAttribute = () => null;
  withDocument({ activeElement: last }, () => RoutineCallDialog.prototype.handleKeydown.call(fixture, { key: 'Tab', shiftKey: false, preventDefault() { prevented++; } }));
  withDocument({ activeElement: first }, () => RoutineCallDialog.prototype.handleKeydown.call(fixture, { key: 'Tab', shiftKey: true, preventDefault() { prevented++; } }));
  RoutineCallDialog.prototype.handleKeydown.call(fixture, { key: 'Escape', preventDefault() { prevented++; } });
  assert.equal(first.focused, true); assert.equal(last.focused, true); assert.equal(closed, 1); assert.equal(prevented, 3);
});
test('dialog refuses blank/invalid execution budget instead of treating blank as unlimited', async () => {
  for (const value of ['', ' ', '-1', '1.2', 'Infinity', '86400001']) {
    let executed = false, error;
    const fixture = { budget: { value }, controller: { execute: async () => { executed = true; } }, cancelButton: { disabled: true }, alive: true, showError(cause) { error = cause.message; } };
    await RoutineCallDialog.prototype.execute.call(fixture); assert.equal(executed, false); assert.match(error, /预算/);
  }
});
test('dialog actual preview method focuses inline parameter error and does not send invalid integer', async () => {
  const input = node(), field = { input, error: node() }; let sent = false;
  const fixture = { fields: new Map([[1, field]]), controller: { metadata: metadata([parameter('bigint')]), input: () => ({ mode: 'value', text: '' }), prepare: async () => { sent = true; } } };
  await RoutineCallDialog.prototype.prepare.call(fixture); assert.equal(sent, false); assert.equal(input.focused, true); assert.equal(input.attrs['aria-invalid'], 'true'); assert.equal(field.error.hidden, false);
});
test('dialog typed output renderer preserves large integer, decimal text, SQL NULL and hostile text', () => {
  const fixture = { result: node() };
  const response = { outcome: outcome({ execution_id: id }), call: { sets: [{ columns: [{ name: 'a', data_type: 'numeric' }], rows: [[{ kind: 'integer', value: '9223372036854775807' }], [{ kind: 'decimal', value: '0.0000000000000012300' }], [{ kind: 'null' }], [{ kind: 'text', value: '<script>not HTML</script>' }]], affected: 0 }], outputs: [], warnings: [] } };
  withDocument({ createElement: () => node() }, () => RoutineCallDialog.prototype.renderResult.call(fixture, response));
  const texts = []; function walk(n) { if (n.textContent) texts.push(n.textContent); n.children?.forEach(walk); } walk(fixture.result);
  for (const value of ['9223372036854775807', '0.0000000000000012300', 'NULL', '<script>not HTML</script>']) assert(texts.includes(value));
});
