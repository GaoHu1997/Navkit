// Run: node --test tools/acceptance/object-tools.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const temp = await mkdtemp(join(tmpdir(), 'navkit-object-tools-'));
const modules = {};
for (const [name, source] of Object.entries({ generator: 'src/core/dataGenerator.ts', details: 'src/core/objectDetails.ts', monitor: 'src/dialogs/ServerMonitorDialog.ts', history: 'src/dialogs/HistoryLogDialog.ts' })) {
  const outfile = join(temp, name + '.mjs');
  await build({ entryPoints: [join(root, source)], outfile, bundle: true, platform: 'node', format: 'esm', logLevel: 'silent' });
  modules[name] = await import(pathToFileURL(outfile).href);
}
process.on('exit', () => { void rm(temp, { recursive: true, force: true }); });
const { generateDataPreview, defaultGeneratorFields, MAX_GENERATED_ROWS } = modules.generator;
const { loadObjectDetails } = modules.details;
const { ServerMonitorController, sampleServerMonitor, supportsProcessList, currentProcessSql } = modules.monitor;
const { filterHistoryLog, sqlLogRows } = modules.history;
const column = (name, type, patch = {}) => ({ name, type, nullable: false, default: null, primary_key: false, auto_increment: false, comment: null, length: null, precision: null, scale: null, ...patch });
const structure = (columns, patch = {}) => ({ table: 'generated', database: 'main', columns, indexes: [], foreign_keys: [], comment: null, engine: null, collation: null, row_count: null, ...patch });
const connection = { id: 'test', name: 'SQLite test', kind: 'sqlite', host: '', port: 0, user: '', password: '', database: '', file: 'test.sqlite', params: '', group: '', read_only: false, keep_alive: false };
const source = (patch = {}) => ({
  tableStatus: async () => [], getTableSchema: async () => structure([]),
  getViewDefinition: async () => '', getRoutineDefinition: async () => '',
  listObjects: async () => [], listRoutines: async () => [],
  serverInfo: async () => ({ product: 'SQLite', version: '3.test', host: '', port: 0, charset: '', collation: '', uptime: null, extra: [] }),
  listBackups: async () => [], ...patch,
});

test('生成器按真实列类型生成25行，保留非空和唯一主键，默认/自增列省略', () => {
  const schema = structure([
    column('id', 'INTEGER', { primary_key: true, auto_increment: true }), column('code', 'varchar(12)', { primary_key: true }),
    column('amount', 'decimal(6,2)', { precision: 6, scale: 2 }), column('flag', 'boolean'), column('created', 'date'),
    column('payload', 'json'), column('optional', 'text', { nullable: true, default: "'database default'" }),
  ]);
  const result = generateDataPreview(schema, defaultGeneratorFields(schema), 25, 'seed', new Date('2026-01-01T00:00:00Z'));
  assert.equal(result.rows.length, 25); assert(!result.columns.includes('id')); assert(!result.columns.includes('optional'));
  assert.equal(new Set(result.rows.map(row => row[0])).size, 25); assert(result.rows.every(row => row.every(v => v !== null)));
  assert.equal(result.rows[0][2], '0'); assert.equal(result.rows[1][2], '1'); assert.equal(result.rows[0][3], '2026-01-01');
  assert.deepEqual(JSON.parse(result.rows[24][4]), { generated: true, index: 25 });
});
test('生成器拒绝条数、重复主键、非空NULL、越界和超长值', () => {
  const schema = structure([column('id', 'tinyint', { primary_key: true }), column('name', 'varchar(3)')]);
  const fields = defaultGeneratorFields(schema);
  assert.throws(() => generateDataPreview(schema, fields, MAX_GENERATED_ROWS + 1), /条数/);
  assert.throws(() => generateDataPreview(schema, fields, 1.5), /整数/);
  assert.throws(() => generateDataPreview(schema, [{ name: 'id', mode: 'constant', value: '1' }, { name: 'name', mode: 'constant', value: 'ok' }], 2), /重复主键/);
  assert.throws(() => generateDataPreview(schema, [{ name: 'id', mode: 'null', value: '' }, { name: 'name', mode: 'auto', value: '' }], 1), /NULL/);
  assert.throws(() => generateDataPreview(schema, [{ name: 'id', mode: 'sequence', value: '127' }, { name: 'name', mode: 'auto', value: '' }], 2), /范围/);
  assert.throws(() => generateDataPreview(schema, [{ name: 'id', mode: 'sequence', value: '1' }, { name: 'name', mode: 'constant', value: 'abcd' }], 1), /长度/);
});
test('固定值中的注入串仅为数据，生成器不会创建SQL', () => {
  const schema = structure([column('note', 'text')]); const injection = "x'); DROP TABLE generated; --";
  const result = generateDataPreview(schema, [{ name: 'note', mode: 'constant', value: injection }], 1);
  assert.deepEqual(result, { columns: ['note'], rows: [[injection]] }); assert.equal(result.sql, undefined);
});
test('未知必填类型及外键不伪造引用', () => {
  const schema = structure([column('geo', 'geometry'), column('parent', 'integer')], { foreign_keys: [{ name: 'fk', columns: ['parent'], ref_table: 'parent', ref_columns: ['id'] }] });
  assert(defaultGeneratorFields(schema).every(f => f.mode === 'constant'));
  assert.throws(() => generateDataPreview(schema, defaultGeneratorFields(schema), 2), /固定值/);
});
test('对象详情显示真实元数据与错误，SQLite整库大小不当作单表大小或排序规则', async () => {
  const state = await loadObjectDetails(connection, 'main', 'items', 'table', {}, source({
    getTableSchema: async () => structure([column('id', 'INTEGER', { primary_key: true })]),
    tableStatus: async () => [['Data_length', '40960'], ['Collation', 'UTF-8'], ['Create_options', '10 pages × 4096 bytes']],
  }));
  assert(state.fields.some(f => f.label.includes('非单表') && f.value === '40960'));
  assert(!state.fields.some(f => f.label === '排序规则' || f.label.startsWith('行数')));
  const error = await loadObjectDetails(connection, 'main', 'missing', 'view', {}, source({ getViewDefinition: async () => { throw new Error('view missing'); } }));
  assert(error.fields.some(f => f.value === 'view missing'));
  assert(!error.fields.some(f => f.label === '定义'));
});
test('视图、例程和备份详情来自具体对应接口', async () => {
  const view = await loadObjectDetails(connection, 'main', 'v', 'view', {}, source({ getViewDefinition: async () => 'CREATE VIEW v AS SELECT 1' }));
  assert(view.fields.some(f => f.value === 'CREATE VIEW v AS SELECT 1'));
  const routine = await loadObjectDetails({ ...connection, kind: 'mysql' }, 'db', 'proc', 'procedure', {}, source({ getRoutineDefinition: async (id, db, name, type) => `${id}/${db}/${name}/${type}` }));
  assert(routine.fields.some(f => f.value === 'test/db/proc/procedure'));
  const backup = await loadObjectDetails(connection, 'main', 'backup.sql', 'query', { category: 'backup', backupName: 'backup.sql' }, source({ listBackups: async () => [{ name: 'backup.sql', modified: '2026-11-05', size: 321, comment: 'real' }] }));
  assert(backup.fields.some(f => f.value === '321')); assert(backup.fields.some(f => f.value === 'real'));
});
test('数据库详情包含连接配置、真实服务器信息和仅表的估计汇总', async () => {
  const cfg = { ...connection, kind: 'mysql', name: 'remote', file: '', host: 'db.example', port: 3307, user: 'reader', database: 'default_db' };
  const object = (name, type, rows, dataLength) => ({ name, type, rows, dataLength, autoIncrement: 0, updateTime: '', engine: '', comment: '' });
  const state = await loadObjectDetails(cfg, 'selected_db', 'selected_db', 'database', {}, source({
    serverInfo: async () => ({ product: 'MySQL', version: '8.4.real', host: 'db.example', port: 3307, charset: 'utf8mb4', collation: 'utf8mb4_0900_ai_ci', uptime: '123', extra: [] }),
    listObjects: async () => [object('a', 'table', 12, 4096), object('b', 'table', 30, 8192), object('v', 'view', 999, 999999)],
  }));
  const value = label => state.fields.find(f => f.label === label)?.value;
  assert.equal(value('连接类型'), 'MySQL'); assert.equal(value('主机（配置）'), 'db.example'); assert.equal(value('端口（配置）'), '3307');
  assert.equal(value('用户名（配置）'), 'reader'); assert.equal(value('默认数据库（配置）'), 'default_db');
  assert.equal(value('服务器版本'), '8.4.real'); assert.equal(value('服务器字符集'), 'utf8mb4'); assert.equal(value('服务器排序规则'), 'utf8mb4_0900_ai_ci');
  assert.equal(value('表总行数（服务器估计）'), '42'); assert.equal(value('表总数据大小（字节；服务器估计）'), '12288');
  assert.match(value('统计说明'), /不是 COUNT/);
});
test('数据库统计缺失和服务器错误不变成虚构零值，SQLite占位大小不汇总', async () => {
  const state = await loadObjectDetails({ ...connection, kind: 'postgresql' }, 'public', 'public', 'schema', {}, source({
    serverInfo: async () => { throw new Error('server unavailable'); },
    listObjects: async () => [{ name: 'unknown', type: 'table', rows: -1, dataLength: NaN }],
  }));
  assert(!state.fields.some(f => f.label === '表总行数（服务器估计）'));
  assert(state.fields.some(f => f.label.endsWith('状态') && f.value.includes('未生成总计')));
  assert(state.fields.some(f => f.value === 'server unavailable'));
  const sqlite = await loadObjectDetails(connection, 'main', 'main', 'database', {}, source({
    serverInfo: async () => ({ product: 'SQLite', version: '3.real', charset: 'UTF-8', collation: '', uptime: null, extra: [] }),
    listObjects: async () => [{ name: 'a', type: 'table', rows: 5, dataLength: 0 }],
  }));
  assert(sqlite.fields.some(f => f.label === '表总行数（对象列表统计）' && f.value === '5'));
  assert(!sqlite.fields.some(f => f.label.includes('总') && f.label.includes('大小')));
  assert(!sqlite.fields.some(f => f.label === '服务器字符集'));
});
test('函数详情有所属上下文及实测源码行数和字符长度，不臆测参数', async () => {
  const definition = 'CREATE FUNCTION f()\r\n-- 中文\nRETURN 1;';
  const state = await loadObjectDetails({ ...connection, kind: 'mysql' }, 'db', 'f', 'function', {}, source({ getRoutineDefinition: async () => definition }));
  const value = label => state.fields.find(f => f.label === label)?.value;
  assert.equal(value('所属连接'), connection.name); assert.equal(value('所属数据库'), 'db'); assert.equal(value('对象类型'), '函数');
  const pg = await loadObjectDetails({ ...connection, kind: 'postgresql' }, 'public', 'f', 'function', {}, source({ getRoutineDefinition: async () => definition }));
  assert.equal(pg.fields.find(field => field.label === '所属模式')?.value, 'public');
  assert(!pg.fields.some(field => field.label === '所属数据库'));
  assert.equal(value('源码行数'), '3'); assert.equal(value('源码长度（字符）'), String(Array.from(definition).length)); assert.equal(value('定义'), definition);
  assert(!state.fields.some(f => /参数|返回类型/.test(f.label)));
  const failed = await loadObjectDetails(connection, 'main', 'f', 'function', {}, source({ getRoutineDefinition: async () => { throw new Error('not supported'); } }));
  assert(!failed.fields.some(f => /源码行数|源码长度/.test(f.label)));
});
test('备份组无backupName显示文件列表总大小和真实最新mtime，不用组名找文件', async () => {
  const files = [{ name: 'z-old.sql', modified: '2026-01-01 12:00:00', size: 100, comment: 'old' }, { name: 'a-new.sql', modified: '2026-11-05 15:00:00', size: 250, comment: 'new' }];
  const state = await loadObjectDetails(connection, 'main', '备份', 'folder', { category: 'backup' }, source({ listBackups: async () => files }));
  const value = label => state.fields.find(f => f.label === label)?.value;
  assert.equal(state.subtitle, '备份分组'); assert.equal(value('备份文件数量'), '2'); assert.equal(value('备份总大小（字节）'), '350');
  assert.equal(value('最新修改时间'), '2026-11-05 15:00:00'); assert.equal(value('最新修改的备份'), 'a-new.sql'); assert.equal(state.list.length, 2);
  assert(!state.fields.some(f => f.value.includes('不存在')));
  const empty = await loadObjectDetails(connection, 'main', '备份', 'folder', { category: 'backup' }, source());
  assert(empty.fields.some(f => f.label === '备份文件数量' && f.value === '0')); assert(!empty.fields.some(f => f.label === '最新修改时间'));
  const missing = await loadObjectDetails(connection, 'main', '备份', 'folder', { category: 'backup', backupName: 'missing.sql' }, source({ listBackups: async () => files }));
  assert(missing.fields.some(f => f.value === '备份文件不存在或已删除'));
  const unavailable = await loadObjectDetails(connection, 'main', '备份', 'folder', { category: 'backup' }, source({ listBackups: async () => { throw new Error('backups unavailable'); } }));
  assert(!unavailable.fields.some(f => f.label === '备份文件数量')); assert(unavailable.fields.some(f => f.value === 'backups unavailable'));
});
test('列索引详情有明确父表上下文，触发器不能冒充同名函数源码', async () => {
  const metadata = source({
    getTableSchema: async () => structure([column('id', 'INTEGER', { primary_key: true })], { indexes: [{ name: 'pk_items', columns: ['id'], primary: true, unique: true, method: 'BTREE' }] }),
    listRoutines: async () => [['trigger', 'same_name']], getRoutineDefinition: async () => { throw new Error('must not query wrong routine'); },
  });
  const col = await loadObjectDetails(connection, 'main', 'id', 'column', { parentTable: 'items' }, metadata);
  assert(col.fields.some(f => f.label === '类型' && f.value === 'INTEGER'));
  const index = await loadObjectDetails(connection, 'main', 'pk_items', 'index', { parentTable: 'items' }, metadata);
  assert(index.fields.some(f => f.label === '唯一索引' && f.value === '是'));
  const trigger = await loadObjectDetails(connection, 'main', 'same_name', 'trigger', {}, metadata);
  assert(trigger.fields.some(f => f.value === '对象在元数据列表中'));
  assert(!trigger.fields.some(f => f.value.includes('must not query')));
});
test('SQL 历史保留原文，结果只取真实 ok 而不从 SQL 或错误文字推断', () => {
  const sqlRows = sqlLogRows([{ ts: '2026-11-07 10:00:00', connection_id: 'c1', connection_name: '本地 MySQL', kind: 'mysql', database: 'shop', sql: 'SELECT * FROM orders', ok: false, elapsed_ms: 120, rows: -1, message: 'permission denied', origin: 'query' }]);
  assert.equal(sqlRows[0].outcome, 'error'); assert.equal(sqlRows[0].category, 'SQL');
  assert.equal(filterHistoryLog(sqlRows, 'orders', 'all', 'error').length, 1);
  assert.equal(filterHistoryLog(sqlRows, '', 'all', 'success').length, 0);
  const original = "SELECT 'failed <script>alert(1)</script>'\r\nFROM orders\n  WHERE id = 1;";
  const successful = sqlLogRows([{ ...sqlRows[0].detail, sql: original, ok: true, message: 'failed 字样不是错误状态' }]);
  assert.equal(successful[0].outcome, 'success'); assert.ok(successful[0].raw.includes(original));
});
test('监控只读当前会话；不请求基本信息或指标，失败与不支持不伪装成零连接', async () => {
  const calls = [];
  const processSource = {
    serverInfo: () => assert.fail('不能请求服务器基本信息'),
    executeSql: () => assert.fail('不能执行协议指标 SQL'),
    listDbProcesses: async id => { calls.push(id); throw new Error('permission denied'); },
  };
  assert.equal(supportsProcessList('custom_mysql'), true); assert.equal(supportsProcessList('sqlite'), false);
  const unsupported = await sampleServerMonitor(connection, processSource);
  assert.equal(unsupported.processes, null); assert.match(unsupported.processError, /不支持/); assert.deepEqual(calls, []);
  const snapshot = await sampleServerMonitor({ ...connection, kind: 'mysql' }, processSource);
  assert.equal(snapshot.processes, null); assert.equal(snapshot.processError, 'permission denied'); assert.deepEqual(calls, ['test']);
  assert.equal(snapshot.server, undefined); assert.equal(snapshot.metrics, undefined);
  assert.equal(currentProcessSql({ command: '空闲', state: 'idle（等待 ClientRead）', sql: 'SELECT previous_statement' }), '');
  assert.equal(currentProcessSql({ command: '正在执行', state: 'active（等待 Lock）', sql: 'SELECT current_statement' }), 'SELECT current_statement');
});
test('监控不重叠；stop丢弃旧响应，手動刷新排队后恢复', async () => {
  let resolveSample; let calls = 0; const received = [];
  const controller = new ServerMonitorController(() => { calls++; return new Promise(resolve => { resolveSample = resolve; }); }, result => received.push(result));
  const first = controller.refresh(); void controller.refresh(); assert.equal(calls, 1);
  controller.stop(); resolveSample({ capturedAt: 'old' }); await first; assert.equal(received.length, 0); assert.equal(calls, 1);
  const second = controller.refresh(); controller.stop(); void controller.refresh(); resolveSample({ capturedAt: 'stopped' }); await second;
  assert.equal(calls, 3); assert.equal(received.length, 0); resolveSample({ capturedAt: 'fresh' }); await new Promise(resolve => setTimeout(resolve, 0));
  assert.deepEqual(received.map(s => s.capturedAt), ['fresh']); controller.dispose(); await controller.refresh(); assert.equal(calls, 3);
});
