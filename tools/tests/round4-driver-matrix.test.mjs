import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import { buildInventory, serializeInventory, parseCatalog, ROOT, ARTIFACT, OPERATIONS } from './round4-driver-matrix.mjs';

// Source inventory tests, NOT database integration or performance tests.
const report = buildInventory();
const byId = id => report.catalog.find(row => row.id === id);
const route = (id, routeId) => byId(id).routes.find(row => row.id === routeId);
const profile = id => Object.fromEntries(Object.entries(report.operationProfiles[id]).map(([op, rule]) => [op, report.rules[rule]]));
const status = (id, routeId, operation) => profile(route(id, routeId).operationProfile)[operation].status;
const readSource = path => readFileSync(resolve(ROOT, path), 'utf8');
const altered = (path, transform) => buildInventory({ readSource: file => file === path ? transform(readSource(file)) : readSource(file) });
const cli = resolve(ROOT, 'tools/tests/round4-driver-matrix.mjs');

test('covers exactly 101 unique Rust catalog IDs plus two generic routes, not 103 catalog products', () => {
  assert.equal(report.summary.catalogCount, 101);
  assert.equal(new Set(report.catalog.map(row => row.id)).size, 101);
  assert.equal(report.summary.genericBindingCount, 2);
  assert.equal(report.summary.routeCount, 206);
  assert.equal(report.catalog.flatMap(row => row.routes).length, 204);
  assert.equal(report.summary.operationCells, 206 * OPERATIONS.length);
  assert.deepEqual(report.catalog.map(row => row.id), parseCatalog(readSource('src-tauri/src/catalog.rs')).map(row => row.id));
  assert.deepEqual(report.summary.catalogByProtocol, {
    Clickhouse: 1, Http: 20, Mongo: 1, Mssql: 1, Mysql: 15,
    Native: 13, Odbc: 34, Postgres: 14, Redis: 1, Sqlite: 1,
  });
  assert.equal(byId('odbc'), undefined);
  assert.equal(byId('jdbc'), undefined);
});

test('every route covers every operation with a referenced rule, never implicit support', () => {
  const allowed = ['implemented', 'limited', 'unsupported', 'blocked'];
  for (const r of [...report.catalog.flatMap(row => row.routes), ...report.genericBindings]) {
    const operations = profile(r.operationProfile);
    assert.deepEqual(Object.keys(operations), [...OPERATIONS], r.operationProfile);
    assert.deepEqual(r.evidence, { status: 'not-run', observations: [] });
    for (const [operation, rule] of Object.entries(operations)) {
      assert.ok(rule && allowed.includes(rule.status), `${r.id}/${operation}`);
      assert.ok(rule.reason && rule.references.length > 0, `${r.id}/${operation}`);
      for (const ref of rule.references) assert.ok(report.references[ref], ref);
    }
    for (const ref of r.connectionReferences) assert.ok(report.references[ref], ref);
  }
});

test('wire transports are independent from catalog Protocol and Db::Http wrapper', () => {
  assert.equal(route('mysql', 'default').backend, 'Db::Mysql');
  assert.equal(route('oracle', 'odbc').backend, 'Db::Odbc');
  assert.equal(route('clickhouse', 'default').transport, 'http');
  assert.equal(route('elasticsearch', 'default').backend, 'Db::Http');
  assert.equal(route('salesforce', 'native-adapter').transport, 'http');
  assert.equal(route('h2', 'native-adapter').adapter, 'H2Db');
  assert.equal(route('h2', 'native-adapter').transport, 'jdbc');
  assert.equal(route('cassandra', 'native-adapter').adapter, 'CassandraDb');
  assert.equal(route('cassandra', 'native-adapter').transport, 'native');
  assert.equal(route('cassandra', 'native-adapter').backend, 'Db::Http');
});

test('managed and custom JDBC never inherit native/ODBC import, restore or sync capabilities', () => {
  for (const row of report.catalog) {
    const jdbc = row.routes.find(r => r.adapter === 'JdbcDb');
    assert.ok(jdbc, row.id);
    assert.equal(jdbc.backend, 'Db::Http', row.id);
    assert.equal(jdbc.transport, 'jdbc', row.id);
    for (const op of ['restore_sql', 'import_csv_json', 'import_sql', 'generate_rows', 'sql_file_controlled', 'full_sync_source', 'full_sync_target', 'diff_default']) {
      assert.equal(profile(jdbc.operationProfile)[op].status, 'unsupported', `${row.id}/${op}`);
    }
  }
  for (const id of ['oracle', 'db2', 'firebird', 'access', 'spanner', 'gbase', 'argodb', 'cache']) {
    assert.ok(route(id, 'managed-or-custom-jdbc'), id);
  }
  assert.ok(route('gbase8c', 'custom-jdbc'), 'No guessed managed GBase 8a driver for 8c');
  assert.match(route('oracle', 'managed-or-custom-jdbc').selection, /installed ODBC/);
  assert.equal(status('mysql', 'custom-jdbc', 'export_csv'), 'unsupported', 'export checks actual backend AND nominal protocol');
  assert.equal(status('oracle', 'managed-or-custom-jdbc', 'export_csv'), 'limited');
});

test('generic ODBC planned-kind rejection is not confused with generic JDBC early override', () => {
  const odbc = report.genericBindings.find(row => row.id === 'generic-odbc');
  const jdbc = report.genericBindings.find(row => row.id === 'generic-jdbc');
  assert.equal(odbc.backend, null);
  assert.equal(odbc.attemptedBackend, 'Db::Odbc');
  for (const op of OPERATIONS) assert.equal(profile(odbc.operationProfile)[op].status, 'blocked', op);
  assert.equal(jdbc.kindId, 'odbc');
  assert.equal(jdbc.backend, 'Db::Http');
  assert.equal(profile(jdbc.operationProfile).query.status, 'implemented');
  assert.equal(profile(jdbc.operationProfile).export_sql.status, 'unsupported');
});

test('nonrelational routes explicitly reject relational operations but retain native queries', () => {
  for (const [id, routeId] of [['redis', 'default'], ['mongodb', 'default'], ['mqtt', 'native-adapter'], ['elasticsearch', 'default'], ['cassandra', 'native-adapter'], ['kafka', 'native-adapter'], ['nacos', 'default'], ['hbase', 'native-adapter']]) {
    assert.equal(status(id, routeId, 'query'), 'implemented', id);
    for (const op of ['import_sql', 'import_csv_json', 'restore_sql', 'backup_sql', 'sql_file_controlled', 'generate_rows', 'full_sync_source', 'full_sync_target', 'incremental_sync', 'diff_default', 'diff_memory']) {
      assert.equal(status(id, routeId, op), 'unsupported', `${id}/${op}`);
    }
  }
  assert.equal(status('redis', 'default', 'export_csv'), 'unsupported');
  assert.equal(status('mongodb', 'default', 'export_sql'), 'unsupported');
  assert.equal(status('mqtt', 'native-adapter', 'export_csv'), 'unsupported');
  assert.equal(status('mongodb', 'default', 'export_json'), 'limited');
  assert.equal(status('elasticsearch', 'default', 'export_csv'), 'limited');
});

test('actual sync/diff dispatch is narrower than the protocol admission list', () => {
  assert.equal(status('sqlite', 'default', 'incremental_sync'), 'limited');
  assert.equal(status('mysql', 'default', 'incremental_sync'), 'unsupported');
  assert.equal(status('oracle', 'odbc', 'full_sync_target'), 'limited');
  assert.equal(status('oracle', 'odbc', 'diff_default'), 'limited');
  assert.equal(status('oracle', 'odbc', 'diff_bounded'), 'unsupported');
  assert.equal(status('sqlserver', 'default', 'diff_bounded'), 'unsupported');
  assert.equal(status('mysql', 'default', 'diff_bounded'), 'limited');
  assert.equal(status('sqlite', 'default', 'diff_memory'), 'limited');
  assert.equal(status('h2', 'native-adapter', 'incremental_sync'), 'unsupported');
  assert.equal(status('h2', 'native-adapter', 'import_sql'), 'unsupported');
  for (const id of ['salesforce', 'hbase']) {
    assert.equal(status(id, 'legacy-odbc', 'export_csv'), 'unsupported', 'ODBC cursor also checks nominal protocol');
    assert.equal(status(id, 'legacy-odbc', 'full_sync_source'), 'unsupported', 'Native catalog kind fails sync admission');
  }
});

test('runtime evidence defaults to not-run, harness type never invents observed results', () => {
  assert.deepEqual(report.summary.runtimeEvidence, { 'not-run': 206, mock: 0, real: 0 });
  assert.equal(report.evidencePolicy.default, 'not-run');
  assert.deepEqual(Object.keys(report.evidencePolicy.statuses), ['not-run', 'mock', 'real']);
  assert.equal(report.evidencePolicy.importedObservations, false);
  const perf = report.performanceValidation;
  assert.equal(perf.status, 'not-run');
  assert.deepEqual(perf.observedResults, []);
  assert.deepEqual(perf.requiredRealTargets, ['mysql/default', 'sqlite/default']);
  assert.deepEqual(perf.optionalRealTargets, ['h2/native-adapter']);
  assert.match(perf.scope, /Do not extrapolate/);
  assert.ok(perf.availableHarnesses.some(row => row.evidenceType === 'mock'));
  for (const h of perf.availableHarnesses) {
    assert.equal(h.status, 'not-run');
    assert.ok(h.prerequisites && ['real', 'mock'].includes(h.evidenceType));
    for (const ref of h.references) assert.ok(report.references[ref], ref);
  }
});

test('shared serializer opportunity spans eligible export routes, with JSON exceptions and no win claim', () => {
  const opportunity = report.performanceValidation.sharedPipelineOpportunity;
  const routes = [...report.catalog.flatMap(row => row.routes), ...report.genericBindings];
  const eligible = routes.filter(r => ['implemented', 'limited'].includes(profile(r.operationProfile).export_csv.status));
  assert.equal(opportunity.exportEligibleRoutes, eligible.length);
  assert.ok(opportunity.exportEligibleRoutes > 100);
  assert.equal(opportunity.status, 'unmeasured');
  assert.match(opportunity.scope, /NOT demonstrated wins/);
  assert.match(opportunity.exceptions, /Mongo JSON uses BSON Extended JSON/);
  assert.match(opportunity.validation, /before\/after.*warmup.*repeated rounds/);
});

test('catalog parser tolerates comments/literals but fails on duplicate/new/unparsed entries', () => {
  const original = readSource('src-tauri/src/catalog.rs');
  assert.equal(parseCatalog(original.replace('// ------------------------------------------------ 关系型数据库', '// k("fake", "Fake", 1, Protocol::Mysql, "relational"),')).length, 101);
  assert.throws(() => altered('src-tauri/src/catalog.rs', text => text.replace('k("mysql",', 'k("postgresql",')), /Duplicate catalog IDs/);
  assert.throws(() => altered('src-tauri/src/catalog.rs', text => text.replace('k("mysql",', 'k("extra", "Extra", 1, Protocol::Mysql, "relational"), k("mysql",')), /Expected 101/);
  assert.throws(() => parseCatalog(original.replace('k("mysql",', 'unknown("mysql",')), /Unparsed catalog entry/);
});

test('dispatch drift fails closed instead of inferring expanded capability from Protocol', () => {
  assert.throws(() => altered('src-tauri/src/transfer_export_cursor.rs', text => text.replace('Protocol::Http | Protocol::Native | Protocol::Odbc', 'Protocol::Http | Protocol::Native')), /Dispatch changed/);
  assert.throws(() => altered('src-tauri/src/batch_execution.rs', text => text.replaceAll('BATCH_NATIVE_REQUIRED', 'BATCH_ALL_DRIVERS')), /Dispatch changed/);
  assert.throws(() => altered('src-tauri/src/native_db.rs', text => text.replace('"mqtt"=>', '"other"=>')), /changed native adapter dispatch/);
});

test('artifact is byte reproducible with CRLF-normalized source hashes, independent of clock or host paths', () => {
  const generated = serializeInventory(report);
  assert.equal(generated, serializeInventory(buildInventory()));
  assert.equal(generated, serializeInventory(buildInventory({ readSource: path => readSource(path).replace(/\r?\n/g, '\r\n') })));
  assert.equal(generated, readFileSync(resolve(ROOT, ARTIFACT), 'utf8'));
  assert.equal(generated.includes(ROOT), false);
  assert.equal(generated.includes('generatedAt'), false);
  assert.ok(report.sources.length > 20);
  for (const file of report.sources) {
    assert.equal(file.sha256, createHash('sha256').update(readSource(file.path).replace(/\r\n/g, '\n')).digest('hex'), file.path);
    assert.ok(!file.path.startsWith('docs/') && !file.path.startsWith('artifacts/'), 'Do not import previous report assumptions');
  }
});

test('every reference resolves to an exact current source line and all catalog anchors identify their ID', () => {
  for (const [id, ref] of Object.entries(report.references)) {
    const line = readSource(ref.path).replace(/\r\n/g, '\n').split('\n')[ref.line - 1];
    assert.ok(line?.trim(), `${id}: ${ref.path}:${ref.line}`);
    assert.ok(ref.symbol, id);
  }
  for (const row of report.catalog) {
    const ref = row.catalogReference;
    const text = readSource(ref.path).replace(/\r\n/g, '\n').split('\n').slice(ref.line - 1, ref.line + 2).join('\n');
    assert.ok(text.includes(`"${row.id}"`), row.id);
  }
});

test('CLI defaults, check and stdout are deterministic from an unrelated working directory', () => {
  const options = { cwd: resolve(ROOT, '..'), encoding: 'utf8' };
  const expected = serializeInventory(report);
  assert.equal(execFileSync(process.execPath, [cli, '--stdout'], options), expected);
  assert.match(execFileSync(process.execPath, [cli], options), /Generated.*101 catalog IDs \+ 2 generic bindings/);
  assert.match(execFileSync(process.execPath, [cli, '--check'], options), /Verified.*runtime evidence NOT_RUN/);
  assert.throws(() => execFileSync(process.execPath, [cli, '--bogus'], { ...options, stdio: 'pipe' }), /Usage:/);
});
