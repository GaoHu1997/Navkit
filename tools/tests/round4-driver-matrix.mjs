#!/usr/bin/env node
/** Static dispatch inventory, NOT a vendor certification or a benchmark runner.
 * Run: node tools/tests/round4-driver-matrix.mjs [--check | --stdout]
 * No services, credentials, downloads, previous reports, or wall-clock inputs.
 */
import { createHash } from 'node:crypto';
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const ROOT = fileURLToPath(new URL('../../', import.meta.url));
export const ARTIFACT = 'artifacts/round4/driver-matrix.json';
export const OPERATIONS = Object.freeze([
  'query', 'browse', 'export_csv', 'export_json', 'export_sql',
  'import_csv_json', 'import_sql', 'sql_file_controlled', 'restore_sql',
  'backup_sql', 'generate_rows', 'full_sync_source', 'full_sync_target',
  'incremental_sync', 'diff_default', 'diff_bounded', 'diff_memory',
]);

function requireThat(ok, message) { if (!ok) throw new Error(message); }

// Small fail-closed scanner for the literal catalog and balanced Rust functions.
// Strings (including raw/byte strings), chars and comments cannot introduce fake arms.
function skipLiteral(text, i) {
  if (text.startsWith('//', i)) { const end = text.indexOf('\n', i); return end < 0 ? text.length : end; }
  if (text.startsWith('/*', i)) {
    let depth = 1; let end = i + 2;
    while (depth && end < text.length) {
      if (text.startsWith('/*', end)) { depth++; end += 2; }
      else if (text.startsWith('*/', end)) { depth--; end += 2; }
      else end++;
    }
    requireThat(!depth, 'Unclosed Rust block comment'); return end;
  }
  const raw = /^(?:br|r)(#*)"/.exec(text.slice(i));
  if (raw) {
    const end = text.indexOf(`"${raw[1]}`, i + raw[0].length);
    requireThat(end >= 0, 'Unclosed Rust raw string'); return end + 1 + raw[1].length;
  }
  if (text[i] === '"') {
    let end = i + 1;
    while (end < text.length) {
      if (text[end] === '\\') end += 2;
      else if (text[end++] === '"') return end;
    }
    throw new Error('Unclosed Rust string');
  }
  const char = /^'(?:\\(?:u\{[\da-fA-F]+\}|x[\da-fA-F]{2}|.)|[^'\\\r\n])'/u.exec(text.slice(i));
  return char ? i + char[0].length : i;
}
function closing(text, start) {
  const pairs = { '(': ')', '[': ']', '{': '}' }; const stack = [];
  requireThat(pairs[text[start]], 'Expected opening delimiter');
  for (let i = start; i < text.length; i++) {
    const end = skipLiteral(text, i); if (end !== i) { i = end - 1; continue; }
    if (pairs[text[i]]) stack.push(pairs[text[i]]);
    else if (')]}'.includes(text[i])) {
      requireThat(stack.pop() === text[i], 'Unbalanced Rust delimiters');
      if (!stack.length) return i;
    }
  }
  throw new Error('Unclosed Rust delimiters');
}
function args(text) {
  const parts = []; let start = 0;
  for (let i = 0; i < text.length; i++) {
    const end = skipLiteral(text, i); if (end !== i) { i = end - 1; continue; }
    if ('([{'.includes(text[i])) { i = closing(text, i); continue; }
    if (text[i] === ',') { parts.push(text.slice(start, i).trim()); start = i + 1; }
  }
  if (text.slice(start).trim()) parts.push(text.slice(start).trim());
  return parts;
}
export function parseCatalog(source) {
  const declaration = 'pub static CATALOG: &[KindMeta] = &[';
  const start = source.indexOf(declaration);
  requireThat(start >= 0, 'CATALOG declaration missing');
  const open = start + declaration.length - 1; const end = closing(source, open);
  const body = source.slice(open + 1, end); const entries = [];
  const string = value => { requireThat(/^"(?:[^"\\]|\\.)*"$/su.test(value), `Nonliteral catalog value: ${value}`); return JSON.parse(value); };
  for (let i = 0; i < body.length;) {
    const skipped = skipLiteral(body, i); if (skipped !== i) { i = skipped; continue; }
    if (/\s|,/.test(body[i])) { i++; continue; }
    const call = /^(k|odbc|http|native)\s*\(/.exec(body.slice(i));
    requireThat(call, `Unparsed catalog entry near ${body.slice(i, i + 50)}`);
    const a = i + call[0].lastIndexOf('('); const b = closing(body, a);
    const fields = args(body.slice(a + 1, b));
    const type = call[1]; const count = { k: 5, odbc: 6, http: 6, native: 5 }[type];
    requireThat(fields.length === count, `Unexpected ${type} catalog arguments`);
    const protocol = type === 'k' ? /^Protocol::(\w+)$/.exec(fields[3])?.[1] : { odbc: 'Odbc', http: 'Http', native: 'Native' }[type];
    requireThat(protocol && /^\d+$/.test(fields[2]), 'Unparsed catalog protocol/port');
    entries.push({ id: string(fields[0]), label: string(fields[1]), protocol,
      category: string(fields[type === 'k' ? 4 : 3]), adapter: type === 'http' ? string(fields[4]) : null,
      line: source.slice(0, open + 1 + i).split('\n').length });
    i = b + 1;
  }
  requireThat(new Set(entries.map(row => row.id)).size === entries.length, 'Duplicate catalog IDs');
  return entries;
}

export function buildInventory({ root = ROOT, readSource = path => readFileSync(resolve(root, path), 'utf8') } = {}) {
  const sources = new Map(); const references = {}; const rules = {};
  function source(path) {
    if (!sources.has(path)) sources.set(path, readSource(path).replace(/\r\n/g, '\n'));
    return sources.get(path);
  }
  function ref(id, path, needle, symbol = needle) {
    const text = source(path); const offset = text.indexOf(needle);
    requireThat(offset >= 0 && text.indexOf(needle, offset + needle.length) < 0, `Missing/ambiguous source anchor ${path}: ${needle}`);
    references[id] = { path, line: text.slice(0, offset).split('\n').length, symbol };
    return id;
  }
  const rust = (id, file, needle, symbol) => ref(id, `src-tauri/src/${file}.rs`, needle, symbol);
  function body(file, name) {
    const text = source(`src-tauri/src/${file}.rs`); const match = new RegExp(`\\bfn ${name}\\s*\\(`).exec(text);
    requireThat(match, `Missing Rust function ${file}::${name}`);
    const open = text.indexOf('{', match.index); return text.slice(open, closing(text, open) + 1);
  }
  function contract(file, name, patterns) {
    const text = body(file, name);
    for (const pattern of patterns) requireThat(pattern.test(text), `Dispatch changed: ${file}::${name}: ${pattern}`);
  }
  function rule(id, status, reason, refs) {
    requireThat(!rules[id], `Duplicate rule ${id}`);
    rules[id] = { status, reason, references: refs }; return id;
  }
  const catalog = parseCatalog(source('src-tauri/src/catalog.rs'));
  requireThat(catalog.length === 101, `Expected 101 catalog DbKind IDs, found ${catalog.length}; review dispatch coverage`);
  rust('kindProtocol', 'models', 'pub fn protocol(&self)', 'DbKind::protocol');
  rust('kindPlanned', 'models', 'pub fn is_planned(&self)', 'DbKind::is_planned');
  rust('open', 'db', 'pub async fn open(req:', 'db::open');
  rust('openDriver', 'db', 'async fn open_odbc_or_jdbc(', 'db::open_odbc_or_jdbc');
  rust('jdbcResolution', 'driver_manager', 'pub fn resolve_jdbc_driver(', 'driver_manager::resolve_jdbc_driver');
  rust('managedDefault', 'managed_jdbc', 'pub fn resolve_default(', 'managed_jdbc::resolve_default');
  rust('nativeConnect', 'native_db', 'pub(crate) async fn connect(', 'native_db::connect');
  rust('httpConnect', 'http_db', 'pub async fn connect(kind:', 'HttpDb::connect');
  rust('query', 'db', 'pub async fn execute(', 'db::execute');
  rust('jdbcBatch', 'jdbc_db', 'async fn run_sql_batch(', 'JdbcDb::run_sql_batch');
  rust('listTables', 'schema', 'pub async fn list_tables(', 'schema::list_tables');
  rust('pageTable', 'schema', 'pub async fn page_table(', 'schema::page_table');
  rust('export', 'transfer_export_cursor', 'async fn export_cursor_with_sql_schema(', 'transfer_export_cursor::export_cursor_with_sql_schema');
  rust('httpExport', 'transfer_http_export', 'pub(super) async fn export(', 'transfer_http_export::export');
  rust('httpPage', 'http_db', 'pub trait HttpAdapter:', 'HttpAdapter::export_page (default offset cursor)');
  rust('jdbcRows', 'jdbc_db', 'JDBC preview exceeds explicit paging safety bounds', 'JdbcDb::rows');
  rust('h2Rows', 'h2_db', 'async fn rows(&self, container:', 'H2Db::rows');
  rust('mqttExport', 'native_mq', 'async fn export_page(&self,_c:', 'MqttDb::export_page');
  rust('mongoExport', 'transfer_mongo_export', 'pub(super) async fn export(', 'transfer_mongo_export::export');
  rust('import', 'transfer', 'async fn execute_import_batches(', 'transfer::execute_import_batches');
  rust('importSql', 'transfer', 'async fn execute_prepared_script(', 'transfer::execute_prepared_script');
  rust('batch', 'batch_execution', 'async fn run_pinned(', 'batch_execution::run_pinned');
  rust('restore', 'transfer', 'async fn execute_one(', 'transfer::execute_one');
  rust('restoreSession', 'transfer_restore_stream', 'pub(super) async fn run_with_control(', 'transfer_restore_stream::run_with_control');
  rust('backup', 'transfer', 'async fn table_ddl(', 'transfer::table_ddl');
  rust('backupPolicy', 'transfer', 'pub async fn dump_database_with_policy(', 'transfer::dump_database_with_policy');
  rust('backupNative', 'transfer_backup_native', 'pub(super) fn mode(', 'transfer_backup_native::mode');
  rust('syncGate', 'sync', 'pub fn supported(', 'sync::supported (admission only)');
  rust('sync', 'sync', 'async fn synchronize_inner(', 'sync::synchronize_inner');
  rust('syncWrite', 'sync', 'async fn execute_sync_pages(', 'sync::execute_sync_pages');
  rust('syncRead', 'transfer', 'async fn fetch_rows_sql(', 'transfer::fetch_rows_sql');
  rust('diff', 'sync', 'async fn diff_with_policy(', 'sync::diff_with_policy');
  rust('diffCompat', 'sync_diff_compat', 'pub(super) async fn compare(', 'sync_diff_compat::compare');
  rust('diffBounded', 'sync_diff', 'async fn legacy_side(', 'sync_diff::legacy_side');
  rust('diffMemory', 'sync_diff', 'pub(super) async fn validate_memory_database(', 'sync_diff::validate_memory_database');
  rust('generate', 'data_generator', 'pub async fn insert_batch(', 'data_generator::insert_batch');

  // Reviewed positive AND negative arms. Drift must fail, never silently broaden support.
  contract('db', 'open', [/if !res\.url\.is_empty\(\)/, /return Ok\(Db::Http\(/, /if req\.kind\.is_planned\(\)/, /Protocol::Native => Ok\(Db::Http/, /Protocol::Odbc => open_odbc_or_jdbc/]);
  contract('models', 'is_planned', [/unwrap_or\(true\)/]);
  contract('driver_manager', 'resolve_jdbc_driver', [/resolve_versioned/, /resolve_driver/, /resolve_default/]);
  contract('transfer_export_cursor', 'export_cursor_with_sql_schema', [/Db::Sqlite\(pool\) if kind\.protocol\(\) == Protocol::Sqlite/, /Db::Mysql\(pool\) if kind\.protocol\(\) == Protocol::Mysql/, /Db::Odbc\(odb\) if kind\.protocol\(\) == Protocol::Odbc/, /Db::Http\(http\) if matches!\(kind\.protocol\(\), Protocol::Http \| Protocol::Native \| Protocol::Odbc\)/, /Db::Redis\(_\).*bail!/]);
  for (const [file, name] of [['transfer', 'execute_import_batches'], ['transfer', 'fetch_rows_sql'], ['sync', 'execute_sync_pages']]) {
    contract(file, name, [/Db::Sqlite\(/, /Db::Mysql\(/, /Db::Postgres\(/, /Db::Mssql\(_\) \| Db::ClickHouse\(_\) \| Db::Odbc\(_\)/, /_ => bail!/]);
    requireThat(!/Db::Http\(/.test(body(file, name)), `Review newly added Http route in ${file}::${name}`);
  }
  contract('transfer', 'execute_one', [/Db::Redis\(_\) \| Db::Mongo\(_\) \| Db::Http\(_\)/, /不支持 SQL 脚本还原/]);
  contract('transfer', 'table_ddl', [/Db::Redis\(_\) \| Db::Mongo\(_\) \| Db::Http\(_\) => Ok\(None\)/]);
  contract('transfer', 'execute_prepared_script', [/Db::Sqlite\(/, /Db::Mysql\(/, /Db::Postgres\(/, /_ => bail!/]);
  contract('batch_execution', 'run_pinned', [/Db::Sqlite\(/, /Db::Mysql\(/, /Db::Postgres\(/, /BATCH_NATIVE_REQUIRED/]);
  contract('sync', 'synchronize_inner', [/if !supported\(source_kind\) \|\| !supported\(target_kind\)/, /\(Db::Sqlite\(source\), Db::Sqlite\(target\)\) if source_kind\.is_sqlite\(\) && target_kind\.is_sqlite\(\)/]);
  contract('sync_diff', 'legacy_side', [/Db::Postgres\(/, /Db::Mysql\(/, /_=>bail!/]);
  const admitted = [...body('sync', 'supported').matchAll(/Protocol::(\w+)/g)].map(m => m[1]);
  requireThat(admitted.join(',') === 'Sqlite,Mysql,Postgres,Mssql,Clickhouse,Odbc', 'Review changed sync admission protocols');

  const q = rule('query', 'implemented', 'db::execute dispatch exists; query language, authorization and server behavior remain adapter-specific.', ['query']);
  const qb = rule('jdbc-query', 'implemented', 'Db::Http delegates ordered SQL batches to JdbcDb, not HTTP REST or ODBC generic_exec.', ['query', 'jdbcBatch']);
  const browse = rule('browse', 'limited', 'Real metadata/page dispatch exists; native containers/objects/keys are not necessarily SQL tables or exhaustive catalogs.', ['listTables', 'pageTable']);
  const exp = rule('sql-cursor-export', 'limited', 'CSV/JSON/SQL fixed cursor path exists; metadata, value types, engine/version and view gates can reject individual objects.', ['export']);
  const httpExp = rule('http-export', 'limited', 'CSV/JSON adapter export_page path; no custom WHERE. Cursor and model semantics vary; not a snapshot guarantee.', ['export', 'httpExport', 'httpPage']);
  const jdbcExp = rule('jdbc-export', 'limited', 'CSV/JSON via Db::Http default offset export_page. JdbcDb::rows caps limit at 10000 and offset+limit at 1000000; not unbounded export.', ['export', 'httpExport', 'httpPage', 'jdbcRows']);
  const h2Exp = rule('h2-export', 'limited', 'Native H2 adapter uses JDBC inside Db::Http and default offset export_page; rows clamps page size to 10000. Not native SQLx.', ['nativeConnect', 'export', 'h2Rows', 'httpPage']);
  const mongoExp = rule('mongo-export', 'limited', 'JSON preserves canonical Extended JSON; CSV uses union-column spool. Not relational SQL export.', ['export', 'mongoExport']);
  const noSqlExp = rule('no-http-sql-export', 'unsupported', 'Http/JDBC adapter export rejects SQL: no reliable typed columns/view DDL on this transfer path.', ['httpExport']);
  const noMongoSql = rule('no-mongo-sql-export', 'unsupported', 'MongoDB SQL export has no SQL restore semantics.', ['mongoExport']);
  const redisExp = rule('no-redis-table-export', 'unsupported', 'Redis key browsing is not a table export implementation.', ['export']);
  const mqttExp = rule('no-mqtt-history-export', 'unsupported', 'MQTT explicitly rejects full history export, including limit=0; live cache is not historical storage.', ['mqttExport']);
  const mismatchExp = rule('export-backend-kind-mismatch', 'unsupported', 'Export requires matching physical backend and nominal kind. Db::Http accepts only Http/Native/Odbc; legacy Db::Odbc requires Odbc, so Salesforce/HBase raw ODBC and native-kind JDBC overrides do not gain unmatched cursor branches.', ['open', 'export']);
  const imp = rule('native-import', 'limited', 'CSV/JSON writes use SQLx transaction; MySQL DDL/nontransactional effects are not guaranteed rollback. Object/type validation still applies.', ['import']);
  const genImp = rule('generic-import', 'limited', 'CSV/JSON generic_exec path is per-statement autocommit, without task transaction rollback.', ['import']);
  const noImp = rule('no-http-import', 'unsupported', 'No Db::Http/Redis/Mongo import batch arm; JDBC has Db::Http even when catalog protocol is Odbc.', ['import']);
  const sqlImp = rule('native-sql-import', 'limited', 'Prepared file SQL import is limited to physical SQLx SQLite/MySQL/Postgres; MySQL may commit DDL.', ['importSql']);
  const noSqlImp = rule('no-generic-sql-import', 'unsupported', 'execute_prepared_script has no generic or Http/JDBC implementation; restore and query batching are different entrypoints.', ['importSql']);
  const batch = rule('native-controlled-batch', 'limited', 'Owned pinned SQLx SQLite/MySQL/Postgres batch session with cancellation/drain; segmented recipes only SQLite.', ['batch']);
  const noBatch = rule('no-controlled-batch', 'unsupported', 'BATCH_NATIVE_REQUIRED: no owned single-session cancellation/drain contract for this actual backend.', ['batch']);
  const restore = rule('native-restore', 'limited', 'Detailed restore uses a dedicated physical SQLx session; reports committed prefixes/unknown effects, not whole-script atomicity.', ['restoreSession', 'restore']);
  const genRestore = rule('generic-restore', 'limited', 'Detailed restore generic compatibility executes per-statement through generic_exec; no dedicated task session/transaction guarantee.', ['restoreSession', 'restore']);
  const noRestore = rule('no-http-restore', 'unsupported', 'execute_one explicitly rejects Db::Http/Redis/Mongo. JDBC query batches do not enable SQL file restore.', ['restore']);
  const backup = rule('sql-backup', 'limited', 'Logical SQL backup/typed export path. SQLite file main has task snapshot; mysql_locked only exact mysql and pg_snapshot only exact postgresql, not compatible aliases. Other modes are compatibility, not certified whole-task snapshots.', ['backup', 'backupPolicy', 'backupNative', 'export']);
  const noBackup = rule('no-http-sql-backup', 'unsupported', 'Default Http/Redis/Mongo table_ddl returns None, or the physical-backend/kind export guard has no matching SQL data cursor. Browsing is not a restorable SQL backup.', ['backup', 'export', 'httpExport', 'mongoExport']);
  const generate = rule('native-generate', 'limited', 'Safe generated-row writes require physical SQLx SQLite/MySQL/Postgres; controlled path also checks pinned backend.', ['generate', 'batch']);
  const noGenerate = rule('no-generate', 'unsupported', 'Safe transactional data generator has no arm for this actual backend.', ['generate', 'batch']);
  const syncRead = rule('full-sync-source', 'limited', 'Protocol admission AND physical fetch_rows_sql path; paged reads, not a whole-source snapshot guarantee or vendor dialect certification.', ['syncGate', 'sync', 'syncRead']);
  const syncWrite = rule('full-sync-target', 'limited', 'SQLx target transaction path; MySQL DDL/nontransactional side effects remain possible. Both endpoints must independently qualify.', ['syncGate', 'sync', 'syncWrite']);
  const syncGeneric = rule('generic-sync-target', 'limited', 'Protocol-admitted generic_exec target; statements autocommit, partial writes are not rolled back. Both endpoints must qualify.', ['syncGate', 'syncWrite']);
  const noSync = rule('no-sync', 'unsupported', 'Protocol admission alone is insufficient: Http/JDBC has no full-sync read/write backend; Native/Http/Redis/Mongo kinds fail admission too.', ['syncGate', 'syncRead', 'syncWrite']);
  const incr = rule('sqlite-incremental', 'limited', 'Only physical native SQLite -> SQLite is admitted to bounded snapshot/typed incremental path; namespace/object gates apply.', ['sync']);
  const noIncr = rule('no-incremental', 'unsupported', 'Incremental sync explicitly rejects every non-native-SQLite pair before DDL/DML.', ['sync']);
  const diff = rule('compat-diff', 'limited', 'Default diff uses physical transfer reads/in-memory comparison; no all-driver bounded-memory claim. SQLite count-only can use bounded dispatch.', ['diff', 'diffCompat', 'syncRead']);
  const bound = rule('bounded-diff', 'limited', 'SQLite read snapshot or MySQL/Postgres legacy-side budget: <=2048 rows/16MiB per side; MySQL requires InnoDB. Not unrestricted streaming for network engines.', ['diff', 'diffBounded']);
  const noBound = rule('no-bounded-diff', 'unsupported', 'Bounded diff has no legacy_side backend for ODBC/SQL Server/ClickHouse/Http/JDBC; broader sync admission is not implementation.', ['diffBounded']);
  const memory = rule('sqlite-memory-diff', 'limited', 'Accounted-memory comparison only native file-backed SQLite on both endpoints; memory/temp/attached namespaces are not certified.', ['diffMemory']);
  const noMemory = rule('no-memory-diff', 'unsupported', 'Accounted-memory diff explicitly requires physical native file SQLite, not an ODBC/JDBC adapter or compatible label.', ['diffMemory']);
  const blocked = rule('generic-odbc-blocked', 'blocked', 'No catalog row for kind=odbc: is_planned defaults true. Custom ODBC does not early-return from db::open before planned rejection; custom JDBC does. Lower-level OdbcDb APIs do not prove application reachability.', ['kindProtocol', 'kindPlanned', 'open']);

  const nativeAdapters = {
    salesforce: ['http', 'salesforce_rest'], hbase: ['http', 'hbase_rest'], h2: ['jdbc', 'H2Db'],
    druid: ['http', 'ApiDb'], dynamodb: ['http', 'ApiDb'], etcd: ['http', 'ApiDb'], rqlite: ['http', 'ApiDb'],
    cassandra: ['native', 'CassandraDb'], nebulagraph: ['native', 'NebulaDb'], kafka: ['native', 'KafkaDb'],
    mqtt: ['native', 'MqttDb'], zookeeper: ['native', 'ZkDb'], rocketmq: ['native', 'RocketDb'],
  };
  const nativeText = body('native_db', 'connect');
  const nativeIDs = [...nativeText.matchAll(/"([a-z0-9_]+)"/g)].map(m => m[1]).filter(id => catalog.some(k => k.id === id));
  requireThat([...new Set(nativeIDs)].sort().join() === Object.keys(nativeAdapters).sort().join(), 'Review changed native adapter dispatch');
  const managedFiles = ['managed_jdbc', 'managed_vendor_assets', 'managed_mirror', 'managed_spanner'];
  const managed = new Set();
  for (const file of managedFiles) {
    const vendor = body(file, 'vendor');
    for (const match of vendor.matchAll(/kinds:\s*&\[([^\]]+)\]/g)) {
      for (const id of match[1].matchAll(/"([a-z0-9_]+)"/g)) managed.add(id[1]);
    }
  }
  const httpConnect = body('http_db', 'connect');
  for (const row of catalog) {
    if (row.protocol === 'Native') requireThat(nativeAdapters[row.id], `Unmapped native kind ${row.id}`);
    if (row.protocol === 'Http') requireThat(new RegExp(`"${row.adapter}"\\s*=>`).test(httpConnect), `Missing HTTP adapter ${row.id}`);
  }
  for (const id of managed) requireThat(catalog.some(row => row.id === id), `Managed JDBC absent from catalog: ${id}`);

  const profiles = {};
  function profile(row, backend, adapter) {
    const sqlx = ['Sqlite', 'Mysql', 'Postgres'].includes(backend);
    const generic = ['Mssql', 'ClickHouse', 'Odbc'].includes(backend);
    const admittedKind = admitted.includes(row.protocol);
    const sqlite = backend === 'Sqlite' && row.protocol === 'Sqlite';
    const exportHttp = backend === 'Http' && ['Http', 'Native', 'Odbc'].includes(row.protocol);
    const exportSqlBackend = (sqlx || generic) && ({ ClickHouse: 'Clickhouse' }[backend] ?? backend) === row.protocol;
    const id = `${backend}:${row.protocol}${backend === 'Http' ? `:${adapter}` : ''}`;
    if (profiles[id]) return id;
    const op = Object.fromEntries(OPERATIONS.map(key => [key, noSync]));
    op.query = adapter === 'JdbcDb' ? qb : q; op.browse = browse;
    let exportRule = exportSqlBackend ? exp : backend === 'Mongo' ? mongoExp : backend === 'Redis' ? redisExp : mismatchExp;
    if (backend === 'Http') exportRule = !exportHttp ? mismatchExp : adapter === 'MqttDb' ? mqttExp : adapter === 'JdbcDb' ? jdbcExp : adapter === 'H2Db' ? h2Exp : httpExp;
    op.export_csv = op.export_json = exportRule;
    op.export_sql = exportSqlBackend ? exp : backend === 'Mongo' ? noMongoSql : exportHttp && adapter !== 'MqttDb' ? noSqlExp : exportRule;
    op.import_csv_json = sqlx ? imp : generic ? genImp : noImp;
    op.import_sql = sqlx ? sqlImp : noSqlImp;
    op.sql_file_controlled = sqlx ? batch : noBatch;
    op.restore_sql = sqlx ? restore : generic ? genRestore : noRestore;
    op.backup_sql = exportSqlBackend ? backup : noBackup;
    op.generate_rows = sqlx ? generate : noGenerate;
    op.full_sync_source = admittedKind && (sqlx || generic) ? syncRead : noSync;
    op.full_sync_target = admittedKind && sqlx ? syncWrite : admittedKind && generic ? syncGeneric : noSync;
    op.incremental_sync = sqlite ? incr : noIncr;
    op.diff_default = admittedKind && (sqlx || generic) ? diff : noSync;
    op.diff_bounded = admittedKind && sqlx ? bound : noBound;
    op.diff_memory = sqlite ? memory : noMemory;
    profiles[id] = op; return id;
  }
  function route(row, id, transport, backend, adapter, selection) {
    return { id, transport, backend: `Db::${backend}`, adapter, selection,
      connectionReferences: ['open', ...(adapter === 'JdbcDb' ? ['openDriver', 'jdbcResolution', 'managedDefault'] : row.protocol === 'Native' ? ['nativeConnect'] : row.protocol === 'Http' ? ['httpConnect'] : [])],
      operationProfile: profile(row, backend, adapter), evidence: { status: 'not-run', observations: [] } };
  }
  const catalogRows = catalog.map(row => {
    const { id, label, protocol, category, line } = row;
    const routes = [];
    if (protocol === 'Odbc') {
      routes.push(route(row, 'odbc', 'odbc', 'Odbc', 'OdbcDb', 'Installed/managed vendor ODBC or explicit raw/custom ODBC, when JDBC is not selected; driver/server required.'));
    } else if (protocol === 'Native') {
      const [transport, adapter] = nativeAdapters[id];
      routes.push(route(row, 'native-adapter', transport, 'Http', adapter, 'Default built-in product adapter. Native is a catalog grouping, not the wire transport.'));
      if (['salesforce', 'hbase'].includes(id)) routes.push(route(row, 'legacy-odbc', 'odbc', 'Odbc', 'OdbcDb', 'Explicit legacy driver/DSN binding retains the separate ODBC path.'));
    } else {
      const backend = { Clickhouse: 'ClickHouse' }[protocol] ?? protocol;
      routes.push(route(row, 'default', ['Http', 'Clickhouse'].includes(protocol) ? 'http' : 'native', backend,
        row.adapter ?? backend, 'Default product route; protocol-compatible labels are not vendor certification.'));
    }
    routes.push(route(row, managed.has(id) ? 'managed-or-custom-jdbc' : 'custom-jdbc', 'jdbc', 'Http', 'JdbcDb',
      managed.has(id) ? 'Verified managed package may be selected if installed ODBC is not ready; explicit version/custom JDBC also possible. Missing package can select ODBC; corrupt selected package/Java errors must not silently fall back.' : 'Explicit validated structured custom JDBC binding overrides default before planned/protocol dispatch; same nominal kind is retained.'));
    return { id, label, category, declaredProtocol: protocol, catalogReference: { path: 'src-tauri/src/catalog.rs', line }, routes };
  });
  const genericJdbc = route({ protocol: 'Odbc' }, 'generic-jdbc', 'jdbc', 'Http', 'JdbcDb', 'kind=odbc (or another uncatalogued identity) with a valid explicit structured JDBC binding; not a 102nd catalog product.');
  const genericOdbc = { id: 'generic-odbc', kindId: 'odbc', transport: 'odbc', backend: null, attemptedBackend: 'Db::Odbc',
    selection: 'Raw DSN or structured custom ODBC binding; currently blocked at application db::open, not reported as implemented.',
    operationProfile: 'blocked-generic-odbc', connectionReferences: ['kindProtocol', 'kindPlanned', 'open'], evidence: { status: 'not-run', observations: [] } };
  profiles[genericOdbc.operationProfile] = Object.fromEntries(OPERATIONS.map(key => [key, blocked]));
  genericJdbc.kindId = 'odbc';

  rust('serializer', 'transfer', 'pub(crate) fn write_rows(', 'ExportSink::write_rows');
  rust('jsonSerializer', 'transfer_json_row', 'pub(super) fn write', 'transfer_json_row::write');
  rust('encoder', 'transfer_output', 'pub(super) struct StagedOutput', 'StagedOutput / FileEncoder');
  rust('httpJson', 'transfer_http_export', 'fn json_record_bytes(', 'transfer_http_export::json_record_bytes');
  rust('mongoJson', 'transfer_mongo_export', 'fn json_record_bytes(', 'transfer_mongo_export::json_record_bytes');
  rust('perfSqlite', 'sync_performance_tests', 'async fn sqlite_real_product_full_and_incremental(', 'sync::performance_tests::sqlite_real_product_full_and_incremental');
  rust('perfMysql', 'sync_performance_tests', 'async fn mysql_real_product_full(', 'sync::performance_tests::mysql_real_product_full');
  rust('realH2', 'h2_db', 'async fn real_hidden_h2_connection(', 'h2_db::tests::real_hidden_h2_connection');
  rust('realJdbc', 'commands_jdbc_namespace_tests', 'async fn generic_jdbc_session_namespaces_binary_paging_and_close_are_real(', 'commands_jdbc_namespace_tests::generic_jdbc_session_namespaces_binary_paging_and_close_are_real');
  rust('mockWire', 'transfer_export_protocol_tests', 'async fn mysql_wire_simulation_exports_views_without_weakening_innodb_and_keeps_one_data_select(', 'MySQL SQLx / local simulated server export test');
  rust('mockRest', 'native_rest_tests', 'async fn salesforce_actual_product_stateful_rest_contract(', 'Salesforce adapter / simulated REST server test');
  const allRoutes = [...catalogRows.flatMap(row => row.routes), genericOdbc, genericJdbc];
  const eligible = allRoutes.filter(r => ['limited', 'implemented'].includes(rules[profiles[r.operationProfile].export_csv].status));
  const summary = {
    catalogCount: catalogRows.length, genericBindingCount: 2, routeCount: allRoutes.length,
    catalogByProtocol: Object.fromEntries([...new Set(catalog.map(k => k.protocol))].sort().map(p => [p, catalog.filter(k => k.protocol === p).length])),
    operationCells: allRoutes.length * OPERATIONS.length,
    runtimeEvidence: { 'not-run': allRoutes.length, mock: 0, real: 0 },
  };
  return {
    schemaVersion: 1,
    scope: 'Source-derived application dispatch inventory. A reachable branch is NOT observed server support, test success, or measured performance.',
    regeneration: `node tools/tests/round4-driver-matrix.mjs; node tools/tests/round4-driver-matrix.mjs --check`,
    evidencePolicy: {
      statuses: { 'not-run': 'No operation was executed by this generator.', mock: 'Requires an executed simulation record; never vendor/real-engine evidence.', real: 'Requires an executed real-engine record bound to exact kind, route, operation and configuration.' },
      default: 'not-run', importedObservations: false,
      limitations: 'Harness source is only available validation coverage, not an execution record. No previous reports are read. Generator/tests do not benchmark databases.',
    },
    summary, operations: [...OPERATIONS], rules, operationProfiles: profiles,
    catalog: catalogRows, genericBindings: [genericOdbc, genericJdbc],
    customBindingPolicy: { jdbc: 'All catalog IDs can explicitly bind JDBC; actual Db::Http per-operation guards above still apply.', odbc: 'Structured custom ODBC requires declared Odbc; other nominal protocols reject rather than fall back. Salesforce/HBase retain a distinct raw driver/DSN legacy path, not a structured ODBC override.', references: ['open', 'openDriver', 'jdbcResolution'] },
    performanceValidation: {
      status: 'not-run', observedResults: [],
      requiredRealTargets: ['mysql/default', 'sqlite/default'], optionalRealTargets: ['h2/native-adapter'],
      scope: 'Real MySQL + file SQLite are the required measured product paths; H2 built-in adapter is optional JDBC conformance/measurement. Do not extrapolate to aliases, ODBC/JDBC vendors, REST, authentication, TLS, query speed, cancellation or all operations.',
      availableHarnesses: [
        { target: 'sqlite/default', evidenceType: 'real', status: 'not-run', operations: ['full_sync_source', 'full_sync_target', 'incremental_sync'], references: ['perfSqlite'], prerequisites: 'Explicit ignored Rust benchmark; isolated file SQLite, >=2000 rows and >=3 rounds, warmup and correctness checks.' },
        { target: 'mysql/default', evidenceType: 'real', status: 'not-run', operations: ['full_sync_source', 'full_sync_target'], references: ['perfMysql'], prerequisites: 'Explicit ignored Rust benchmark; NAVKIT_SYNC_MYSQL_URL for an authorized isolated local fixture, InnoDB. No credentials inferred or database contacted here.' },
        { target: 'h2/native-adapter', evidenceType: 'real', status: 'not-run', operations: ['query', 'browse'], references: ['realH2'], prerequisites: 'Java + official H2 JAR, copied isolated test executable. Existing test is conformance, NOT a throughput benchmark.' },
        { target: 'generic-jdbc', evidenceType: 'real', status: 'not-run', operations: ['query', 'browse'], references: ['realJdbc'], prerequisites: 'Ignored isolated Java/H2 explicit-binding test. Does not certify other JDBC vendors or perf.' },
        { target: 'mysql/default', evidenceType: 'mock', status: 'not-run', operations: ['export_csv', 'export_json', 'export_sql'], references: ['mockWire'], prerequisites: 'Actual SQLx client, local wire simulation; NOT a MySQL server benchmark.' },
        { target: 'salesforce/native-adapter', evidenceType: 'mock', status: 'not-run', operations: ['query', 'browse'], references: ['mockRest'], prerequisites: 'Actual adapter with stateful simulated HTTP server; NOT Salesforce cloud acceptance.' },
      ],
      sharedPipelineOpportunity: {
        status: 'unmeasured', exportEligibleRoutes: eligible.length,
        scope: 'All code-reachable export routes share staged output/encoding; relational native/ODBC/ClickHouse/SQL Server plus CSV replay for HTTP/JDBC/Mongo share ExportSink. These are cross-driver serializer investigation targets, NOT demonstrated wins.',
        references: ['serializer', 'jsonSerializer', 'encoder', 'export', 'httpExport', 'mongoExport'],
        exceptions: 'HTTP/JDBC JSON uses its own bounded JSON object writer; Mongo JSON uses BSON Extended JSON. Do not claim one shared JSON serializer or apply SQL batching to all formats/routes.',
        exceptionReferences: ['httpJson', 'mongoJson'],
        validation: 'Measure encode/write CPU and allocations separately from fetch, first-row latency, full output, peak RSS and correctness (Unicode, NULL/empty, binary, decimals, dynamic fields). Compare equivalent fixtures before/after with warmup and repeated rounds; gate by each route/format above. Record real and mock separately. No performance threshold or result is invented.',
      },
    },
    references,
    sources: [...sources].sort(([a], [b]) => a.localeCompare(b, 'en')).map(([path, text]) => ({ path, sha256: createHash('sha256').update(text).digest('hex'), normalization: 'CRLF to LF' })),
  };
}

export function serializeInventory(report) { return `${JSON.stringify(report, null, 2)}\n`; }
export function main(argv = process.argv.slice(2)) {
  requireThat(argv.length <= 1 && argv.every(arg => ['--check', '--stdout'].includes(arg)), 'Usage: node tools/tests/round4-driver-matrix.mjs [--check | --stdout]');
  const report = buildInventory(); const output = serializeInventory(report); const path = resolve(ROOT, ARTIFACT);
  if (argv.includes('--stdout')) { process.stdout.write(output); return; }
  if (argv.includes('--check')) requireThat(readFileSync(path, 'utf8') === output, `Stale artifact: run node tools/tests/round4-driver-matrix.mjs (${ARTIFACT})`);
  else { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, output, 'utf8'); }
  console.log(`${argv.includes('--check') ? 'Verified' : 'Generated'} ${ARTIFACT}: ${report.summary.catalogCount} catalog IDs + 2 generic bindings, ${report.summary.routeCount} routes, ${report.summary.operationCells} operation cells; runtime evidence NOT_RUN (${report.summary.runtimeEvidence['not-run']}).`);
}
if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
