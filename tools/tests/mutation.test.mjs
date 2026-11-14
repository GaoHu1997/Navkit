import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { build } from 'esbuild';

const bundle = await build({ entryPoints: ['src/core/mutation.ts'], bundle: true, platform: 'node', format: 'esm', write: false, logLevel: 'silent' });
const { toDbValue } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const plain = value => JSON.parse(JSON.stringify(value));

for (const [raw, type, expected] of [
  ['9223372036854775807', 'BIGINT', { kind: 'integer', value: '9223372036854775807' }],
  ['18446744073709551615', 'BIGINT UNSIGNED', { kind: 'integer', value: '18446744073709551615' }],
  ['-9223372036854775808', 'int8', { kind: 'integer', value: '-9223372036854775808' }],
  ['-0.000000000000123400', 'DECIMAL(38,20)', { kind: 'decimal', value: '-0.000000000000123400' }],
  ['001.0000', 'numeric(20,4)', { kind: 'decimal', value: '001.0000' }],
  ['', 'varchar(255)', { kind: 'text', value: '' }],
  [null, 'integer', { kind: 'null' }],
  ['0x0041FF', 'BLOB', { kind: 'binary', value: 'AEH/' }],
  ['0x', 'bytea', { kind: 'binary', value: '' }],
  ['2026-11-09T12:00:01.123+08:00', 'timestamp with time zone', { kind: 'timestamptz', value: '2026-11-09T12:00:01.123+08:00' }],
  ["'; DROP TABLE x; --中文😀", 'text', { kind: 'text', value: "'; DROP TABLE x; --中文😀" }],
]) {
  test(`typed grid value preserves ${type}: ${JSON.stringify(raw)}`, () => assert.deepEqual(toDbValue(raw, type), expected));
}

for (const [raw, type] of [
  ['', 'int'], ['   ', 'decimal'], ['1.2', 'integer'], ['1e3', 'bigint'],
  ['9223372036854775808', 'bigint'], ['-1', 'BIGINT UNSIGNED'], ['NaN', 'numeric'],
  ['Infinity', 'double'], ['1e9999', 'decimal'], ['1x', 'float'], ['?', 'boolean'],
  ['plain ASCII bytes', 'binary'], ['0xABC', 'bytea'],
  ['2026-11-09 12:00:00', 'timestamptz'], ['{invalid}', 'json'],
]) {
  test(`invalid known value is rejected, never silently coerced: ${type} ${JSON.stringify(raw)}`, () => assert.throws(() => toDbValue(raw, type)));
}

test('large binary conversion is chunked and round-trips bytes', () => {
  const hex = '0x' + '0041FF'.repeat(50000);
  const value = toDbValue(hex, 'varbinary');
  const decoded = Buffer.from(value.value, 'base64');
  assert.equal(decoded.length, 150000);
  assert.deepEqual([...decoded.subarray(0, 6)], [0, 65, 255, 0, 65, 255]);
});

// Execute the actual methods, not a reimplementation; small fixtures need no fake browser.
const appSource = fs.readFileSync('src/core/App.ts', 'utf8');
const ast = ts.createSourceFile('App.ts', appSource, ts.ScriptTarget.Latest, true);
const dataClass = ast.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === 'DataTabView');
function viewMethod(name, context = {}) {
  const names = Array.isArray(name) ? name : [name];
  const methods = names.map(name => {
    const method = dataClass.members.find(node => ts.isMethodDeclaration(node) && node.name?.getText(ast) === name);
    assert(method, `actual DataTabView.${name} exists`);
    return method;
  });
  const source = `class TestView { ${methods.map(method => method.getText(ast)).join('\n')} }\nglobalThis.TestView = TestView;`;
  const sandbox = vm.createContext({ toDbValue, ...context });
  vm.runInContext(ts.transpile(source, { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None }), sandbox);
  const view = new sandbox.TestView();
  // Focused method fixtures do not instantiate the view's DOM/count lifecycle.
  view.invalidateTotal = () => {};
  view.scheduleTotalCount = () => {};
  return view;
}

test('grid edits distinguish SQL NULL from empty text and can restore NULL', () => {
  const view = viewMethod('applyEdit');
  view.tab = { page: { columns: [{ name: 'text' }], rows: [[null]] } };
  view.pendingDeletes = new Set(); view.pendingUpdates = new Map(); view.pendingInsertEdits = new Map(); view.txAppliedDeletes = new Set();
  view.originalRowCount = () => 1; view.applyData = () => {}; view.app = { renderStatus() {}, toast() {} };
  view.applyEdit(0, 'text', '');
  assert.equal(view.pendingUpdates.get(0).get('text'), '');
  view.applyEdit(0, 'text', null);
  assert.equal(view.pendingUpdates.size, 0);
});

test('mutation preview and execution retain the same page owner and target', async () => {
  const calls = [];
  const api = {
    previewRowMutation: async draft => { calls.push(['preview', plain(draft)]); return { plan_id: 'opaque-plan' }; },
    executeRowMutation: async (...args) => { calls.push(['execute', args]); return { plan_id: 'opaque-plan', status: 'applied', transaction: 'committed', affected: 1, verified: true, mismatches: [] }; },
  };
  const view = viewMethod('runMutation', { api });
  view.tab = { id: 'page-a', connectionId: 'conn', schema: 'public', object: 'items' };
  const draft = { connection_id: 'conn', schema: 'public', table: 'items', kind: 'update', changes: [['id', toDbValue('9007199254740993', 'bigint')]] };
  await view.runMutation(draft);
  assert.equal(calls[0][1].owner_id, 'page-a');
  assert.deepEqual(calls[0][1].changes, draft.changes);
  assert.deepEqual(calls[1], ['execute', ['opaque-plan', 'page-a']]);
});

test('a page target changed while previewing never executes the stale plan', async () => {
  let executed = false;
  const view = viewMethod('runMutation', { api: {
    previewRowMutation: async () => { view.tab.connectionId = 'other'; return { plan_id: 'plan' }; },
    executeRowMutation: async () => { executed = true; },
  } });
  view.tab = { id: 'page-a', connectionId: 'conn', schema: '', object: 'items' };
  await assert.rejects(view.runMutation({ connection_id: 'conn', table: 'items', kind: 'delete' }), /页面目标已变化/);
  assert.equal(executed, false);
});

test('conflicts retain a meaningful backend reason instead of claiming saved', async () => {
  const view = viewMethod('runMutation', { api: {
    previewRowMutation: async () => ({ plan_id: 'plan' }),
    executeRowMutation: async () => ({ plan_id: 'plan', status: 'conflict', transaction: 'rolled_back', affected: 0, verified: false, mismatches: ['原值定位命中 2 行；已回滚'] }),
  } });
  view.tab = { id: 'page-a', connectionId: 'conn', schema: '', object: 'items' };
  await assert.rejects(view.runMutation({ connection_id: 'conn', table: 'items', kind: 'delete' }), /2 行.*已回滚/);
});

function mutationView(api, methods = ['runMutation']) {
  const view = viewMethod(methods, { api, crypto: { randomUUID: () => 'stable-row-token' } });
  view.tab = { id: 'page-a', connectionId: 'conn', schema: 'main', object: 'items', selectedRows: new Set([0]),
    page: { columns: [{ name: 'id', type: 'integer', auto_increment: true }, { name: 'body', type: 'text' }, { name: 'note', type: 'text' }], primary_keys: ['id'], rows: [['7', null, 'original']] } };
  view.app = { draftChanged() {}, renderStatus() {}, toast() {} };
  view.pendingUpdates = new Map(); view.pendingDeletes = new Set(); view.pendingInserts = []; view.pendingInsertEdits = new Map();
  view.rowDraftTokens = new WeakMap();
  return view;
}
const validReceipt = { plan_id: 'plan', status: 'applied', transaction: 'committed', affected: 1, verified: true, mismatches: [] };

test('lost commit response freezes the draft and never issues a second plan or write', async () => {
  let previews = 0, writes = 0;
  const view = mutationView({
    previewRowMutation: async () => { previews++; return { plan_id: 'plan' }; },
    executeRowMutation: async () => { writes++; throw new Error('connection lost after commit'); },
  }, ['runMutation', 'captureDraft']);
  view.pendingUpdates.set(0, new Map([['body', 'changed']]));
  const draft = { connection_id: 'conn', schema: 'main', table: 'items', kind: 'update' };
  await assert.rejects(view.runMutation(draft), /结果未知.*禁止/);
  await assert.rejects(view.runMutation(draft), /结果未知.*禁止/);
  assert.equal(previews, 1); assert.equal(writes, 1);
  const snapshot = plain(view.captureDraft());
  assert.match(snapshot.payload.execution_effects_unknown, /connection lost/);
  assert.deepEqual(snapshot.payload.rows[0].changes, [['body', 'changed']]);
  assert.equal(snapshot.baseline.rows[0].identity.requires_validation, true);
});

for (const [index, receipt] of [undefined, {}, { ...validReceipt, plan_id: 'other-plan' }, { ...validReceipt, affected: 0 }, { ...validReceipt, transaction: 'none' }].entries()) {
  test(`invalid or mismatched mutation receipt ${index} is unknown, never retryable`, async () => {
    let writes = 0;
    const view = mutationView({ previewRowMutation: async () => ({ plan_id: 'plan' }), executeRowMutation: async () => { writes++; return receipt; } });
    const draft = { connection_id: 'conn', table: 'items', kind: 'insert' };
    await assert.rejects(view.runMutation(draft), /协议校验.*结果未知/);
    await assert.rejects(view.runMutation(draft), /结果未知.*禁止/);
    assert.equal(writes, 1);
  });
}

test('an explicit pre-execution expired plan failure does not mark database effects unknown', async () => {
  const view = mutationView({ previewRowMutation: async () => ({ plan_id: 'plan' }),
    executeRowMutation: async () => { throw Object.assign(new Error('plan expired before write'), { code: 'PLAN_EXPIRED' }); } });
  await assert.rejects(view.runMutation({ connection_id: 'conn', table: 'items', kind: 'insert' }), /plan expired/);
  assert.equal(view.mutationEffectsUnknown, undefined);
});

test('an invalid preview never reaches the execute IPC', async () => {
  let writes = 0;
  const view = mutationView({ previewRowMutation: async () => ({}), executeRowMutation: async () => { writes++; } });
  await assert.rejects(view.runMutation({ connection_id: 'conn', table: 'items', kind: 'insert' }), /未发送写入/);
  assert.equal(writes, 0); assert.equal(view.mutationEffectsUnknown, undefined);
});

for (const blocked of ['committing', 'mutationEffectsUnknown']) {
  test(`actual grid edit, insertion and deletion methods are frozen by ${blocked}`, async () => {
    const view = mutationView({}, ['applyEdit', 'insertRow', 'deleteSelectedRows']);
    view[blocked] = blocked === 'committing' ? true : 'unconfirmed commit';
    view.originalRowCount = () => 1;
    view.applyData = () => { assert.fail('frozen grid must not mutate local row state'); };
    view.applyEdit(0, 'body', 'new value'); await view.insertRow(); await view.deleteSelectedRows();
    assert.equal(view.pendingUpdates.size, 0); assert.equal(view.pendingDeletes.size, 0); assert.equal(view.pendingInserts.length, 0);
  });
}

for (const explicitNull of [false, true]) {
  test(`actual sparse insert keeps server defaults and explicit NULL (${explicitNull})`, async () => {
    const view = mutationView({}, ['commitEdits', 'columnType']);
    const drafts = [], busy = [];
    view.pendingInserts = [[null, null, null]];
    if (explicitNull) view.pendingInsertEdits.set(0, new Map([['note', null]]));
    view.hasPending = () => true; view.transactionOwnedByThisTab = () => false;
    view.renderFooter = () => { busy.push(view.committing); };
    view.reload = async () => {};
    view.runMutation = async draft => { assert.equal(view.committing, true); drafts.push(plain(draft)); return validReceipt; };
    await view.commitEdits();
    assert.equal(drafts.length, 1); assert.equal(drafts[0].kind, 'insert');
    assert.deepEqual(drafts[0].changes, explicitNull ? [['note', { kind: 'null' }]] : []);
    assert.equal(view.pendingInserts.length, 0); assert.deepEqual(busy, [true, false]);
  });
}

