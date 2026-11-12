import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
const output = await build({ entryPoints: ['src/core/structure-sync.ts'], bundle: true, platform: 'node', format: 'esm', write: false, logLevel: 'silent' });
const { StructureSyncSession } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);
const hex = n => n.repeat(64);
const sourceRevision = hex('a'), targetRevision = hex('b'), digest = hex('c');
const sourceFingerprint = hex('d'), targetFingerprint = hex('e');
const input = () => ({ source_connection: 'source', source_schema: 'main', source_table: 'source table', target_connection: 'target', target_schema: 'main', target_table: 'target table', columns: [['id', 'id'], ['value', 'value']], allow_drop: false, source_revision: '', target_revision: '', review_digest: '' });
const preview = () => {
  const binding = { principal: 'actual-window', owner: 'page', source_connection_id: 'source', source_fingerprint: sourceFingerprint, source_schema: 'main', source_table: 'source table', target_connection_id: 'target', target_fingerprint: targetFingerprint, target_schema: 'main', target_table: 'target table' };
  const identity = (name, table, rootpage) => ({ database: { kind: 'sqlite-file', identity: name, canonical_path: `C:/test/${name}.sqlite` }, schema: 'main', table, rootpage });
  return { target_exists: true, changes: [{ kind: 'add_column', object: 'value', before: null, after: 'TEXT', risk: 'safe' }], warnings: [], alterable: true, unsupported: [], source_revision: sourceRevision, target_revision: targetRevision, summary: 'one safe change', final_design: { schema: 'main', table: 'target table', columns: [], indexes: [] }, steps: [{ label: 'add value', sql: 'ALTER TABLE "target table" ADD COLUMN "value" TEXT' }], review_digest: digest, review_binding: structuredClone(binding), request_sql: `STRUCTURE_SYNC_REVIEW ${hex('f')}`,
    source_identity: identity('source-file', 'source table', 2), target_identity: identity('target-file', 'target table', 3), same_database: false,
    source_consistency: 'two_owned_read_snapshots_not_cross_database_atomic', observation_digest: hex('9'), observation_binding: structuredClone(binding), observation_only: false };
};
function observation(report, patch = {}) { Object.assign(report, { changes: [], steps: [], request_sql: '', review_digest: '', review_binding: null, observation_only: true }, patch); if (!report.target_exists) { report.target_identity = null; report.target_revision = ''; } return report; }
const receipt = report => { const { request_sql, ...reviewed } = report; return { report: structuredClone(reviewed), receipt: { steps: report.steps.map(step => ({ ...step, status: 'applied', error: null })), verified: true, unverified: false, mismatches: [], transaction: 'committed', schema: { table: 'target table', database: 'main', columns: [], indexes: [], foreign_keys: [], row_count: null } } }; };
const outcome = (request, patch = {}) => ({ execution_id: request.execution_id, state: 'succeeded', result: null, error: null, deadline: { execution_id: request.execution_id, budget_ms: 60000, elapsed_ms: 1, deadline_reached: false, stop_confirmed: false }, effects: 'completed', transaction_state: 'committed', resources_pending: false, ...patch });
function fixture(t) {
  const log = [], current = input(), report = preview();
  const port = {
    current: () => current,
    preview: async (request, owner) => { log.push(['preview', structuredClone(request), owner]); return structuredClone(report); },
    issueGrant: async request => { log.push(['grant', structuredClone(request)]); return { ...request, execution_id: crypto.randomUUID(), execution_grant: 'real-contract-test-grant' }; },
    execute: async (request, syncRequest) => { log.push(['execute', structuredClone(request), structuredClone(syncRequest)]); return { outcome: outcome(request), receipt: receipt(report) }; },
    cancelExecution: async request => { log.push(['cancel', structuredClone(request)]); return { execution_id: request.execution_id, accepted: true, state: 'cancel_requested', message: 'request only' }; },
    executionStatus: async request => ({ execution_id: request.execution_id, state: 'unknown', deadline: {}, resources_pending: true }),
  };
  const session = new StructureSyncSession('page', port);
  t.after(() => session.dispose());
  return { session, port, log, current, report };
}
const sent = log => log.filter(item => item[0] === 'execute');
const grants = log => log.filter(item => item[0] === 'grant');

test('structure preview binds both revisions, full mapping, owner and backend marker without executing', async t => {
  const { session, log } = fixture(t); await session.preview();
  assert.equal(session.canApply, true); assert.equal(grants(log).length, 0); assert.equal(sent(log).length, 0);
  await session.apply(); const [, request, syncRequest] = sent(log)[0];
  assert.equal(request.owner_id, 'page'); assert.equal(request.connection_id, 'target'); assert.equal(request.namespace, 'main'); assert.equal(request.sql, preview().request_sql);
  assert.equal(syncRequest.source_revision, sourceRevision); assert.equal(syncRequest.target_revision, targetRevision); assert.equal(syncRequest.review_digest, digest);
  assert.deepEqual(syncRequest.columns, input().columns); assert.equal(session.canApply, false); assert.equal(session.effectsUnknown, false);
});
for (const [name, mutate] of [
  ['source connection', value => value.source_connection = 'other'], ['source namespace', value => value.source_schema = 'other'], ['source table', value => value.source_table = 'other'],
  ['target connection', value => value.target_connection = 'other'], ['target namespace', value => value.target_schema = 'other'], ['target table', value => value.target_table = 'other'],
  ['mapping order', value => value.columns.reverse()], ['implicit mapping collision', value => value.columns[1][1] = 'id'], ['drop option', value => value.allow_drop = true],
]) test(`editing ${name} after preview prevents grant and write`, async t => {
  const { session, current, log } = fixture(t); await session.preview(); mutate(current);
  assert.equal(session.canApply, false); await assert.rejects(session.apply(true), /REVIEW_CHANGED/); assert.equal(grants(log).length, 0); assert.equal(sent(log).length, 0);
});
test('mutating the returned preview does not replace the frozen SQL or final request', async t => {
  const { session, log } = fixture(t); const report = await session.preview(); report.request_sql = 'DROP TABLE x'; report.steps[0].sql = 'DROP TABLE x'; report.review_binding.owner = 'foreign';
  await session.apply(); assert.equal(sent(log)[0][1].sql, preview().request_sql); assert.equal(sent(log)[0][2].review_digest, digest);
});
test('context change during grant prevents any execute IPC', async t => {
  const { session, current, port, log } = fixture(t); await session.preview();
  const issue = port.issueGrant; port.issueGrant = async request => { const grant = await issue(request); current.target_table = 'changed during grant'; return grant; };
  await assert.rejects(session.apply(), /REVIEW_CHANGED/); assert.equal(grants(log).length, 1); assert.equal(sent(log).length, 0); assert.equal(session.effectsUnknown, false);
});
for (const [name, mutate] of [
  ['missing source proof', report => report.review_binding.source_fingerprint = ''], ['foreign page', report => report.review_binding.owner = 'foreign'], ['foreign target', report => report.review_binding.target_table = 'other'],
  ['wrong final table', report => report.final_design.table = 'other'], ['bad marker', report => report.request_sql = 'ALTER TABLE x DROP COLUMN y'], ['missing steps', report => delete report.steps],
  ['nonboolean metadata', report => report.alterable = 'true'], ['empty SQL step', report => report.steps[0].sql = ''], ['missing source revision', report => report.source_revision = ''],
]) test(`invalid ${name} preview never authorizes execution`, async t => {
  const { session, report, log } = fixture(t); mutate(report); await assert.rejects(session.preview(), /STRUCTURE_PREVIEW/); assert.equal(session.canApply, false); assert.equal(sent(log).length, 0); assert.equal(grants(log).length, 0);
});
test('readonly, unavailable and unchanged metadata remain visible without an apply plan', async t => {
  for (const patch of [{ alterable: false, unsupported: ['metadata unknown'] }, { target_exists: false }, { changes: [], steps: [], request_sql: '', review_digest: '' }]) {
    const { session, report, log } = fixture(t); observation(report, patch); const shown = await session.preview(); assert.equal(shown.summary, report.summary); assert.equal(session.canApply, false); await assert.rejects(session.apply(), /REVIEW_CHANGED/); assert.equal(grants(log).length, 0);
  }
});
test('destructive changes need explicit consent before grant', async t => {
  const { session, current, log } = fixture(t); current.allow_drop = true; await session.preview(); await assert.rejects(session.apply(), /CONFIRMATION_REQUIRED/); assert.equal(grants(log).length, 0); await session.apply(true); assert.equal(sent(log).length, 1);
});
for (const [name, mutate] of [
  ['foreign digest', value => value.report.review_digest = hex('1')], ['foreign owner', value => value.report.review_binding.owner = 'foreign'], ['foreign principal', value => value.report.review_binding.principal = 'foreign'],
  ['different source endpoint', value => value.report.review_binding.source_fingerprint = hex('1')], ['different target endpoint', value => value.report.review_binding.target_fingerprint = hex('1')],
  ['changed source revision', value => value.report.source_revision = hex('1')], ['changed target revision', value => value.report.target_revision = hex('1')],
  ['unverified schema', value => value.receipt.verified = false], ['unverified flag', value => value.receipt.unverified = true], ['mismatch', value => value.receipt.mismatches.push('extra trigger')],
  ['wrong transaction', value => value.receipt.transaction = 'implicit'], ['wrong target schema', value => value.receipt.schema.database = 'other'], ['wrong target table', value => value.receipt.schema.table = 'other'],
  ['altered executed SQL', value => value.receipt.steps[0].sql = 'DROP TABLE x'], ['skipped step', value => value.receipt.steps[0].status = 'skipped'], ['missing step', value => value.receipt.steps = []],
]) test(`invalid ${name} receipt locks replay`, async t => {
  const { session, port, log } = fixture(t); port.execute = async (request, syncRequest) => { log.push(['execute', request, syncRequest]); const value = receipt(preview()); mutate(value); return { outcome: outcome(request), receipt: value }; };
  await session.preview(); await assert.rejects(session.apply(), /RECEIPT_INVALID/); assert.equal(session.effectsUnknown, true); await assert.rejects(session.apply(), /未知/); assert.equal(sent(log).length, 1); assert.equal(grants(log).length, 1);
});
test('success outcome without committed transaction is not a verified success', async t => {
  const { session, port } = fixture(t); port.execute = async request => ({ outcome: outcome(request, { transaction_state: 'none' }), receipt: receipt(preview()) }); await session.preview(); await assert.rejects(session.apply(), /RECEIPT_INVALID/); assert.equal(session.effectsUnknown, true);
});
test('unknown or pending response cannot release the write lock', async t => {
  for (const patch of [{ state: 'unknown', transaction_state: 'unknown' }, { resources_pending: true }]) {
    const { session, port, log } = fixture(t); port.execute = async (request, syncRequest) => { log.push(['execute', request, syncRequest]); return { outcome: outcome(request, patch), receipt: receipt(preview()) }; };
    await session.preview(); await assert.rejects(session.apply(), /未知/); assert.equal(session.effectsUnknown, true); await assert.rejects(session.apply(), /未知/); assert.equal(sent(log).length, 1);
  }
});
test('lost commit acknowledgement keeps the same execution quarantined without replay', async t => {
  const { session, port, log } = fixture(t); port.execute = async (request, syncRequest) => { log.push(['execute', request, syncRequest]); throw new Error('lost commit ACK'); };
  await session.preview(); await assert.rejects(session.apply(), /lost commit ACK/); assert.equal(session.effectsUnknown, true); assert.equal(session.busy, true); await assert.rejects(session.preview(), /排空/); await assert.rejects(session.apply(), /未知/); assert.equal(sent(log).length, 1);
});
test('confirmed rollback rejects the stale plan while retaining proof of no effect', async t => {
  const { session, port, log } = fixture(t); port.execute = async (request, syncRequest) => { log.push(['execute', request, syncRequest]); return { outcome: outcome(request, { state: 'failed', effects: 'no_effect', transaction_state: 'rolled_back', error: { code: 'STRUCTURE_ROLLED_BACK', message: 'rolled back', retryable: false } }), receipt: null }; };
  await session.preview(); await assert.rejects(session.apply(), /rolled back/); assert.equal(session.effectsUnknown, false); assert.equal(session.canApply, false); await assert.rejects(session.apply(), /REVIEW_CHANGED/); assert.equal(sent(log).length, 1);
});
test('stopping pending preview discards its late response and sends no execution', async t => {
  const { session, port, log } = fixture(t); let resolve; port.preview = () => new Promise(done => { resolve = done; }); const pending = session.preview(); await session.stop(); resolve(preview()); await assert.rejects(pending, /REVIEW_CHANGED/); assert.equal(session.canApply, false); assert.equal(grants(log).length, 0);
});
test('stop during grant is carried to C1 as cancel_before_send', async t => {
  const { session, port, log } = fixture(t); let resolveGrant, grantedRequest;
  port.issueGrant = request => { grantedRequest = request; return new Promise(done => { resolveGrant = done; }); };
  port.execute = async (request, syncRequest) => { log.push(['execute', request, syncRequest]); return { outcome: outcome(request, { state: 'cancelled', effects: 'no_effect', transaction_state: 'none' }), receipt: null }; };
  await session.preview(); const pending = session.apply(); await session.stop(); resolveGrant({ ...grantedRequest, execution_id: crypto.randomUUID(), execution_grant: 'grant' }); await assert.rejects(pending, /未成功/); assert.equal(sent(log).length, 1); assert.equal(sent(log)[0][1].cancel_before_send, true); assert.equal(session.effectsUnknown, false);
});
test('disposed session ignores late metadata without sending or unlocking a new operation', async t => {
  const { session, port, log } = fixture(t); let resolve; port.preview = () => new Promise(done => { resolve = done; }); const pending = session.preview(); session.dispose(); resolve(preview()); await assert.rejects(pending, /REVIEW_CHANGED/); await assert.rejects(session.apply(), /未知/); assert.equal(sent(log).length, 0);
});

test('explicit no-change observation reconciles local evidence without authorizing or rewriting C1 Unknown', async t => {
  const { session, port, log, report } = fixture(t);
  port.execute = async (request, syncRequest) => { log.push(['execute', request, syncRequest]); return { outcome: outcome(request, { state: 'unknown', effects: 'unknown', transaction_state: 'unknown' }), receipt: null }; };
  await session.preview(); await assert.rejects(session.apply(), /未知/); assert.equal(session.execution.state, 'unknown'); assert.equal(session.busy, false);
  observation(report); await session.preview(); assert.equal(session.effectsUnknown, false); assert.equal(session.execution.state, 'unknown'); assert.equal(session.canApply, false);
  await assert.rejects(session.apply(), /REVIEW_CHANGED/); assert.equal(grants(log).length, 1); assert.equal(sent(log).length, 1);
});
for (const [name, mutate] of [
  ['missing digest', value => value.observation_digest = ''], ['foreign owner', value => value.observation_binding.owner = 'other'],
  ['wrong table identity', value => value.target_identity.table = 'other'], ['unproved physical identity', value => value.target_identity.database.identity = ''],
  ['false same-database claim', value => value.same_database = true], ['negative rootpage', value => value.target_identity.rootpage = -1],
]) test(`invalid ${name} observation never clears unknown writes`, async t => {
  const { session, port, report, log } = fixture(t); port.execute = async request => ({ outcome: outcome(request, { state: 'unknown', transaction_state: 'unknown', effects: 'unknown' }), receipt: null });
  await session.preview(); await assert.rejects(session.apply(), /未知/); observation(report); mutate(report);
  await assert.rejects(session.preview(), /STRUCTURE_PREVIEW/); assert.equal(session.effectsUnknown, true); assert.equal(session.canApply, false); assert.equal(grants(log).length, 1);
});
for (const [name, mutate] of [
  ['different principal', value => value.observation_binding.principal = 'different-window'], ['changed endpoint', value => value.observation_binding.target_fingerprint = hex('1')],
  ['different physical file', value => value.target_identity.database.identity = 'replacement-file'], ['recreated target rootpage', value => value.target_identity.rootpage = 19],
  ['partial metadata', value => value.unsupported.push('metadata unknown')], ['missing target', value => { value.target_exists = false; value.target_identity = null; value.target_revision = ''; }],
]) test(`${name} read-only observation cannot reconcile earlier target effects`, async t => {
  const { session, port, report, log } = fixture(t); port.execute = async request => ({ outcome: outcome(request, { state: 'unknown', transaction_state: 'unknown', effects: 'unknown' }), receipt: null });
  await session.preview(); await assert.rejects(session.apply(), /未知/); observation(report); mutate(report);
  await assert.rejects(session.preview(), /RECONCILIATION_INCOMPLETE/); assert.equal(session.effectsUnknown, true); assert.equal(session.canApply, false); assert.equal(grants(log).length, 1);
});
test('an observation never bypasses C1 pending resource drainage', async t => {
  const { session, port, report, log } = fixture(t); port.execute = async request => ({ outcome: outcome(request, { state: 'unknown', resources_pending: true, effects: 'unknown', transaction_state: 'unknown' }), receipt: null });
  await session.preview(); await assert.rejects(session.apply(), /未知/); observation(report); const calls = log.length;
  await assert.rejects(session.preview(), /排空/); assert.equal(log.length, calls); assert.equal(session.effectsUnknown, true); assert.equal(session.canApply, false);
});

const dialogSource = ts.createSourceFile('SyncDialog.ts', readFileSync(new URL('../../src/dialogs/SyncDialog.ts', import.meta.url), 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
const dialogNode = dialogSource.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === 'openSyncDialog');
function findFunction(name) { let found; const visit = node => { if (ts.isFunctionDeclaration(node) && node.name?.text === name) found = node; ts.forEachChild(node, visit); }; visit(dialogNode); assert.ok(found, `production ${name} must exist`); return found; }
function dialogMethod(name, context) { const js = ts.transpileModule(`${findFunction(name).getText(dialogSource)}\nglobalThis.method = ${name};`, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None } }).outputText; vm.runInNewContext(js, context); return context.method; }
test('real wizard finish guards unknown structure writes before any data sync IPC', async () => {
  let finish; const visit = node => { if (ts.isPropertyAssignment(node) && node.name.getText(dialogSource) === 'onFinish') finish = node.initializer; ts.forEachChild(node, visit); }; visit(dialogNode); assert.ok(finish);
  const errors = [], context = { structureSession: { busy: false, effectsUnknown: true }, wizard: { showError: message => errors.push(message) }, syncApi: new Proxy({}, { get: () => () => assert.fail('unknown structure cannot dispatch another sync') }), busy: false };
  vm.runInNewContext(ts.transpileModule(`globalThis.finish = ${finish.getText(dialogSource)};`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText, context); await context.finish(); assert.equal(errors.length, 1);
});
test('real preview handler rejects closed, active and database mode before metadata IPC', async () => {
  for (const patch of [{ structureClosed: true }, { busy: true }, { structureSession: { busy: true } }, { isDbMode: () => true }]) {
    const context = { busy: false, structureClosed: false, structureSession: { busy: false }, isDbMode: () => false, structureSyncRequest: () => assert.fail('must guard before reading context'), ...patch }; await dialogMethod('previewStructure', context)();
  }
});
test('real sync UI freezes context and finish while unknown, retaining stop and explicit review', () => {
  const input = { disabled: false }, apply = { disabled: false, title: '' }, busyStates = [];
  const context = { structureClosed: false, structureSession: { busy: false, effectsUnknown: true, canApply: false }, structureLockedControls: new Set(), dlg: { body: { querySelectorAll: () => input.disabled ? [] : [input], querySelector: () => apply } }, wizard: { setBusy: value => busyStates.push(value) }, busy: false, structureStop: { disabled: false } };
  const update = dialogMethod('syncStructureUi', context); update(); assert.equal(input.disabled, true); assert.equal(apply.disabled, true); assert.equal(busyStates.at(-1), true); assert.equal(context.structureStop.disabled, true);
  context.structureSession.effectsUnknown = false; update(); assert.equal(input.disabled, false); assert.equal(busyStates.at(-1), false); assert.equal(context.structureLockedControls.size, 0);
});
