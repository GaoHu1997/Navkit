import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
async function load(relative) {
  const { outputFiles } = await build({ entryPoints: [path.join(root, relative)], bundle: true, platform: 'node', format: 'esm', target: 'es2021', write: false });
  return import(`data:text/javascript;base64,${Buffer.from(outputFiles[0].text).toString('base64')}`);
}
const { QueryResultModel, compareQueryValues } = await load('src/core/query-result-model.ts');
const { validateQueryEditability, readOnlyQueryEditability, queryActionReason } = await load('src/core/query-editability.ts');
const integer = value => ({ kind: 'integer', value });
const decimal = value => ({ kind: 'decimal', value });
const text = value => ({ kind: 'text', value });
const binary = value => ({ kind: 'binary', value });
const ctx = { source_execution_id: 'source-execution', result_index: 0, generation: 'source-generation' };
function fixture() {
  const columns = ['id', 'label', 'amount', 'payload'].map((name, i) => ({ id: `column-${i}`, name, data_type: ['INTEGER', 'TEXT', 'TEXT', 'BLOB'][i] }));
  const evidence = { ...ctx, result_token: 'opaque-result', mode: 'single_table', reason: '', schema: 'main', table: 'items', metadata_revision: 'a'.repeat(64),
    columns: columns.map((col, i) => ({ result_column_id: col.id, result_index: i, name: col.name, base_column: col.name, data_type: col.data_type, writable: i !== 0, key: i === 0, generated: false, reason: i ? '' : '定位键只读' })),
    key_column_ids: ['column-0'], actions: { update: { supported: true, reason: '' }, delete: { supported: true, reason: '' }, insert: { supported: false, reason: '缺少必填投影' } } };
  return { ...ctx, columns, total_rows: 500_000, editability: evidence, rows: [
    { token: 'view-a', row_token: 'opaque-a', values: [integer('1'), text('alpha'), decimal('9007199254740993.000000000000000001'), binary('AAD/')] },
    { token: 'view-b', row_token: 'opaque-b', values: [integer('2'), text('Beta'), decimal('9007199254740992.999999999999999999'), binary('')] },
    { token: 'view-c', row_token: 'opaque-c', values: [integer('3'), text('alpha'), { kind: 'null' }, binary('AA==')] },
  ] };
}
const compare = (a, b) => compareQueryValues(a, b).comparison;

test('exact numeric comparison keeps i64/u64/decimal precision, signs, exponents and stable equal values', () => {
  assert.ok(compare(integer('9007199254740993'), integer('9007199254740992')) > 0);
  assert.ok(compare(integer('18446744073709551615'), integer('9223372036854775807')) > 0);
  assert.ok(compare(decimal('9007199254740993.000000000000000001'), decimal('9007199254740993.000000000000000000')) > 0);
  assert.ok(compare(decimal('-1e-1000'), decimal('0')) < 0);
  assert.ok(compare(decimal('1e1000'), decimal('9e999')) > 0);
  assert.equal(compare(decimal('-0.000'), integer('0')), 0);
  assert.equal(compare(decimal('1.0000'), decimal('10e-1')), 0);
  assert.equal(compareQueryValues(integer('2'), decimal('2.0')).fallback, null);
});

test('nanosecond temporal ordering compares UTC instants and validates local dates/times', () => {
  const z = value => ({ kind: 'timestamptz', value });
  assert.equal(compare(z('2024-02-29T15:30:00.123456789+05:30'), z('2024-02-29T10:00:00.123456789Z')), 0);
  assert.ok(compare(z('2024-02-29T10:00:00.123456790Z'), z('2024-02-29T10:00:00.123456789Z')) > 0);
  assert.equal(compare({ kind: 'time', value: '01:02:03' }, { kind: 'time', value: '01:02:03.000000000' }), 0);
  assert.ok(compare({ kind: 'time', value: '01:02:03.000000001' }, { kind: 'time', value: '01:02:03' }) > 0);
  assert.equal(compare({ kind: 'timestamp', value: '2024-02-29 01:02:03' }, { kind: 'timestamp', value: '2024-02-29T01:02:03.000' }), 0);
  assert.ok(compareQueryValues(z('2023-02-29T00:00:00Z'), z('2024-02-29T00:00:00Z')).fallback);
  assert.ok(compareQueryValues(integer('2'), text('2')).fallback);
});

test('multi-column sorting preserves token identity, explicit NULL position and stable ties', () => {
  const model = new QueryResultModel(fixture());
  model.setSorts([{ column_id: 'column-1', ascending: true, nulls: 'last' }, { column_id: 'column-2', ascending: false, nulls: 'first' }]);
  assert.deepEqual(model.view().tokens, ['view-b', 'view-c', 'view-a']);
  model.setSorts([{ column_id: 'column-2', ascending: false, nulls: 'first' }]);
  assert.deepEqual(model.view().tokens, ['view-c', 'view-a', 'view-b']);
  model.setSorts([{ column_id: 'column-2', ascending: true, nulls: 'last' }]);
  assert.deepEqual(model.view().tokens, ['view-b', 'view-a', 'view-c']);
  model.setSorts([{ column_id: 'column-1', ascending: true, nulls: 'last' }]);
  assert.deepEqual(model.view().tokens, ['view-b', 'view-a', 'view-c']);
});

test('loaded-only AND/OR filters use exact values and distinguish NULL from empty text', () => {
  const model = new QueryResultModel(fixture());
  model.setFilters([{ logic: 'OR', conditions: [{ column_id: 'column-1', operator: 'contains', value: text('ALP') }, { column_id: 'column-2', operator: 'is_null' }] },
    { conditions: [{ column_id: 'column-0', operator: 'gt', value: integer('1') }] }]);
  const view = model.view();
  assert.deepEqual(view.tokens, ['view-c']); assert.equal(view.loaded, 3); assert.equal(view.total, 500_000); assert.match(view.scope, /仅已加载 3 行/);
  model.setFilters([{ conditions: [{ column_id: 'column-2', operator: 'lt', value: decimal('9007199254740993') }] }]);
  assert.deepEqual(model.view().tokens, ['view-b']);
  model.setFilters([{ conditions: [{ column_id: 'column-2', operator: 'ne', value: { kind: 'null' } }] }]);
  assert.deepEqual(model.view().tokens, []);
  model.setFilters([{ conditions: [{ column_id: 'column-1', operator: 'eq', value: text('') }] }]);
  assert.equal(model.view().visible, 0);
});

test('edits, delete marks and hidden dirty rows stay attached to opaque tokens through filter/sort', () => {
  const snapshot = fixture(), model = new QueryResultModel(snapshot);
  model.setSorts([{ column_id: 'column-2', ascending: true, nulls: 'last' }]);
  model.stage('view-a', 'column-1', text("changed');DELETE FROM x;--"));
  model.markDeleted('view-c');
  model.setFilters([{ conditions: [{ column_id: 'column-0', operator: 'eq', value: integer('2') }] }]);
  assert.equal(model.view().hidden_dirty, 2); assert.equal(model.dirty, true);
  assert.deepEqual(model.changes(), [
    { result_token: 'opaque-result', row_token: 'opaque-a', operation: 'update', changes: [['column-1', text("changed');DELETE FROM x;--")]] },
    { result_token: 'opaque-result', row_token: 'opaque-c', operation: 'delete', changes: [] },
  ]);
  assert.deepEqual(snapshot.rows[0].values[1], text('alpha'));
  model.discard('view-a'); assert.equal(model.view().dirty, 1);
  model.discard(); assert.equal(model.dirty, false);
});

test('copy takes a detached visible/selected mapping and never leaks hidden rows or columns', () => {
  const model = new QueryResultModel(fixture());
  model.select(['view-a', 'view-b', 'view-c']);
  model.setSorts([{ column_id: 'column-0', ascending: false, nulls: 'last' }]);
  model.setFilters([{ conditions: [{ column_id: 'column-0', operator: 'lt', value: integer('3') }] }]);
  model.setHiddenColumns(['column-0']);
  const snapshot = model.copySnapshot();
  assert.deepEqual(snapshot.columns.map(c => c.name), ['label', 'amount', 'payload']);
  assert.deepEqual(snapshot.rows.map(row => row[0].value), ['Beta', 'alpha']);
  assert.deepEqual(snapshot.rows[1][2], { kind: 'binary', value: 'AAD/', encoding: 'base64' });
  assert.throws(() => model.copySnapshot(undefined, { token: 'view-a', column_id: 'column-0' }), /隐藏/);
  assert.throws(() => model.copySnapshot(undefined, { token: 'view-c', column_id: 'column-1' }), /隐藏/);
  model.stage('view-a', 'column-1', text('new')); assert.equal(snapshot.rows[1][0].value, 'alpha');
  assert.ok(model.copySnapshot().generation > snapshot.generation);
});

test('stale context, write acknowledgement, refresh and missing capabilities preserve local drafts', () => {
  const model = new QueryResultModel(fixture());
  model.stage('view-a', 'column-1', text('pending'));
  assert.throws(() => model.replace({ ...fixture(), generation: 'new' }), /未保存/);
  model.requireRefreshAfterWrite();
  assert.throws(() => model.changes(), /重新执行/); assert.throws(() => model.stage('view-b', 'column-1', text('x')), /重新执行/);
  assert.equal(model.values('view-a')[1].value, 'pending'); assert.equal(model.dirty, true);
  model.discard();
  const snapshot = fixture(); snapshot.generation = 'replacement'; snapshot.editability.generation = 'replacement';
  model.replace(snapshot); assert.equal(model.dirty, false); assert.deepEqual(model.context, { ...ctx, generation: 'replacement' });
  assert.throws(() => model.stage('view-a', 'column-0', integer('9')), /只读/);
  assert.throws(() => model.stage('missing', 'column-1', text('x')), /token/);
  snapshot.editability = readOnlyQueryEditability(model.context, '缺少键');
  const readOnly = new QueryResultModel(snapshot); assert.throws(() => readOnly.stage('view-a', 'column-1', text('x')), /缺少键/);
});

test('runtime typed values reject unknown tags, unsafe numbers, malformed binary and invalid dates', async t => {
  const bad = [{ kind: 'unknown', value: 'x' }, { kind: 'null', value: '' }, { kind: 'integer', value: 1 }, integer('18446744073709551616'),
    { kind: 'int', value: 9007199254740992 }, { kind: 'float', value: Infinity }, { kind: 'bool', value: 'false' }, decimal('1e1001'),
    binary('AA=A'), binary('AB=='), { kind: 'date', value: '2023-02-29' }, { kind: 'time', value: '24:00:00' },
    { kind: 'timestamp', value: '2024-02-30T00:00:00' }, { kind: 'timestamptz', value: '2024-01-01T00:00:00+18:01' },
    { kind: 'uuid', value: 'not-uuid' }, { kind: 'json', value: '{bad' }, { kind: 'text', value: false }, { kind: 'text', value: 'x', surprise: true }];
  for (const [i, value] of bad.entries()) await t.test(String(i), () => {
    const snapshot = fixture(); snapshot.rows[0].values[1] = value; assert.throws(() => new QueryResultModel(snapshot), /类型化/);
    const model = new QueryResultModel(fixture()); assert.throws(() => model.stage('view-a', 'column-1', value), /类型化/); assert.equal(model.dirty, false);
  });
});

test('server evidence is complete, detached and exact for result identity and column origin', async t => {
  const evidence = fixture().editability;
  const result = validateQueryEditability(evidence, ctx, 4); result.columns[1].writable = false;
  assert.equal(evidence.columns[1].writable, true); assert.equal(queryActionReason(evidence, 'insert'), '缺少必填投影');
  const bad = [proof => { proof.generation = 'foreign'; }, proof => { proof.result_token = null; }, proof => { proof.schema = 'temp'; },
    proof => { proof.metadata_revision = 'none'; }, proof => { proof.columns.pop(); }, proof => { proof.columns[1].result_index = 0; },
    proof => { proof.columns[1].result_column_id = proof.columns[0].result_column_id; }, proof => { proof.columns[1].base_column = 'ID'; },
    proof => { proof.columns[1].generated = true; }, proof => { proof.columns[0].writable = true; }, proof => { proof.key_column_ids = []; },
    proof => { proof.actions.delete = { supported: false, reason: '' }; }, proof => { proof.columns.forEach(col => { col.writable = false; }); }];
  for (const [i, mutate] of bad.entries()) await t.test(String(i), () => { const value = structuredClone(evidence); mutate(value); assert.throws(() => validateQueryEditability(value, ctx, 4), /证据/); });
  const malformedReadOnly = readOnlyQueryEditability(ctx, '只读'); malformedReadOnly.actions.update.supported = true;
  assert.throws(() => validateQueryEditability(malformedReadOnly, ctx, 4), /证据/);
  const columnMismatch = fixture(); columnMismatch.columns[1].id = 'foreign'; assert.throws(() => new QueryResultModel(columnMismatch), /列 ID/);
});

test('insert metadata remains exact and detached while older evidence cannot acquire implicit key permissions', () => {
  const evidence = fixture().editability;
  for (const [index, column] of evidence.columns.entries()) Object.assign(column, { insertable: true, nullable: index !== 0, auto_increment: false, default_value: index === 1 ? "'server default'" : null });
  const validated = validateQueryEditability(evidence, ctx, 4);
  assert.equal(validated.columns[0].key, true); assert.equal(validated.columns[0].writable, false); assert.equal(validated.columns[0].insertable, true);
  assert.equal(validated.columns[0].nullable, false); assert.equal(validated.columns[0].auto_increment, false); assert.equal(validated.columns[0].default_value, null);
  assert.equal(validated.columns[1].default_value, "'server default'"); validated.columns[1].default_value = 'client edit'; assert.equal(evidence.columns[1].default_value, "'server default'");
  const old = validateQueryEditability(fixture().editability, ctx, 4);
  for (const name of ['insertable', 'nullable', 'auto_increment', 'default_value']) assert.equal(name in old.columns[0], false);
  const model = new QueryResultModel({ ...fixture(), editability: evidence }); assert.throws(() => model.stage('view-a', 'column-0', integer('8')), /只读/);
});

test('present insert flags and default metadata reject lossy coercion or undefined instead of guessing', async t => {
  const invalid = [
    ['insertable', 'true'], ['insertable', 1], ['insertable', null], ['insertable', undefined],
    ['nullable', 'false'], ['nullable', 0], ['nullable', null], ['nullable', undefined],
    ['auto_increment', 'false'], ['auto_increment', 0], ['auto_increment', null], ['auto_increment', undefined],
    ['default_value', 0], ['default_value', false], ['default_value', {}], ['default_value', undefined],
  ];
  for (const [index, [field, value]] of invalid.entries()) await t.test(`${field}-${index}`, () => {
    const snapshot = fixture(); snapshot.editability.columns[0][field] = value;
    assert.throws(() => validateQueryEditability(snapshot.editability, ctx, 4), /证据/);
    assert.throws(() => new QueryResultModel(snapshot), /证据/);
  });
});

test('invalid filters/sorts and detached values cannot corrupt model mappings', () => {
  const model = new QueryResultModel(fixture());
  for (const groups of [[{ logic: 'X', conditions: [] }], [{ conditions: [{ column_id: 'unknown', operator: 'is_null' }] }], [{ conditions: [{ column_id: 'column-1', operator: 'eq' }] }]]) assert.throws(() => model.setFilters(groups));
  assert.throws(() => model.setSorts([{ column_id: 'column-1', ascending: 'false', nulls: 'first' }]));
  assert.throws(() => model.setSorts([{ column_id: 'column-1', ascending: true, nulls: 'last' }, { column_id: 'column-1', ascending: false, nulls: 'first' }]));
  const values = model.values('view-a'); values[0].value = '999'; assert.equal(model.values('view-a')[0].value, '1');
  const duplicate = fixture(); duplicate.rows[1].token = duplicate.rows[0].token; assert.throws(() => new QueryResultModel(duplicate), /token/);
  const sameServerRow = fixture(); sameServerRow.rows[1].row_token = sameServerRow.rows[0].row_token; assert.throws(() => new QueryResultModel(sameServerRow), /token/);
  const total = fixture(); total.total_rows = 2; assert.throws(() => new QueryResultModel(total), /总行数/);
});
