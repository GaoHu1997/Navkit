import assert from 'node:assert/strict';
import { test, before, after } from 'node:test';
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdtemp, rm, access } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';

// Real H2 JDBC tests, not mock IPC/DOM. Files and database names are isolated.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const jar = process.env.NAVKIT_H2_JAR || path.join(root, 'tools/acceptance/datasources/h2/h2-2.3.232.jar');
const java = process.env.NAVKIT_JAVA || (process.env.JAVA_HOME
  ? path.join(process.env.JAVA_HOME, 'bin', process.platform === 'win32' ? 'java.exe' : 'java') : 'java');
const b64 = (value) => Buffer.from(value, 'utf8').toString('base64');
const v = (kind, value) => kind === 'null' ? { kind } : { kind, value };
let temporary, support, available = true;
const clients = [];

before(async () => {
  try { await access(jar); } catch { available = false; return; }
  temporary = await mkdtemp(path.join(os.tmpdir(), 'navkit-pl20-java-'));
  support = await readFile(path.join(root, 'src-tauri/src/JdbcMutationSupport.java'), 'utf8');
});
after(async () => {
  for (const client of clients) await client.close();
  if (temporary && path.basename(temporary).startsWith('navkit-pl20-java-')) await rm(temporary, { recursive: true, force: true });
});

class Sidecar {
  constructor(child) {
    this.child = child; this.buffer = ''; this.queue = []; this.stderr = ''; this.session = ''; this.chain = Promise.resolve();
    this.exit = new Promise(resolve => child.once('close', (code) => { this.closed = true; this.exitCode = code; resolve(code); }));
    child.stdout.setEncoding('utf8'); child.stdout.on('data', chunk => {
      this.buffer += chunk;
      for (;;) {
        const end = this.buffer.indexOf('\n'); if (end < 0) break;
        const line = this.buffer.slice(0, end); this.buffer = this.buffer.slice(end + 1);
        const waiter = this.queue.shift(); if (waiter) { clearTimeout(waiter.timer); waiter.resolve(JSON.parse(line)); }
        else this.unexpected = line;
      }
    });
    child.stderr.setEncoding('utf8'); child.stderr.on('data', chunk => { this.stderr = (this.stderr + chunk).slice(-4000); });
    child.on('error', error => { for (const waiter of this.queue.splice(0)) { clearTimeout(waiter.timer); waiter.reject(error); } });
    child.once('close', code => { for (const waiter of this.queue.splice(0)) { clearTimeout(waiter.timer); waiter.reject(new Error(`Java exit ${code}: ${this.stderr}`)); } });
  }
  request(line) {
    const run = () => new Promise((resolve, reject) => {
      if (this.closed) { reject(new Error('sidecar already closed')); return; }
      const timer = setTimeout(() => { this.child.kill(); reject(new Error(`Real Java request deadline: ${this.stderr}`)); }, 30000);
      this.queue.push({ resolve, reject, timer }); this.child.stdin.write(line + '\n');
    });
    const result = this.chain.then(run); this.chain = result.catch(() => {}); return result;
  }
  async query(sql, schema = '') {
    const result = await this.request(`query\t${b64(sql)}\t10\t${b64(schema)}`);
    if (result.error) throw new Error(result.error); return result;
  }
  async envelope(action, payload = {}, session = this.session) {
    const request_id = randomUUID();
    const result = await this.request(`v2\t${b64(JSON.stringify({ protocol: 2, request_id, session_id: session, action, payload }))}`);
    assert.equal(result.protocol, 2); assert.equal(result.request_id, request_id);
    if (session) assert.equal(result.session_id, session);
    else assert.match(result.session_id, /^[0-9a-f-]{36}$/);
    return result;
  }
  async command(action, payload = {}) {
    const envelope = await this.envelope(action, payload);
    if (envelope.error) throw new Error(JSON.stringify(envelope.error)); return envelope.result;
  }
  async metadata(schema, name) {
    return this.command('metadata', { object: { catalog: this.catalog, schema, name } });
  }
  plan(metadata, kind, changes = [], keys = [], original_values = []) {
    return { operation_id: randomUUID(), session_id: this.session, object: metadata.object, base_revision: metadata.revision,
      kind, keys, original_values, changes, read_only: false, timeout_secs: 10 };
  }
  async close() {
    if (this.closed) return;
    this.child.stdin.end();
    const timer = setTimeout(() => this.child.kill(), 5000);
    await this.exit; clearTimeout(timer);
  }
}
async function open(kind, options = '') {
  const mainName = kind === 'native' ? 'NavkitH2' : 'NavkitJdbc';
  const source = await readFile(path.join(root, `src-tauri/src/${mainName}.java`), 'utf8');
  const directory = await mkdtemp(path.join(temporary, `${kind}-`));
  const target = path.join(directory, `${mainName}.java`);
  await writeFile(target, source + '\n' + support);
  const child = spawn(java, ['-Dfile.encoding=UTF-8', '--class-path', jar, target], { cwd: directory, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  const client = new Sidecar(child); clients.push(client);
  const url = `jdbc:h2:mem:pl20_${randomUUID().replaceAll('-', '')};DB_CLOSE_DELAY=-1${options}`;
  const connect = kind === 'native' ? `connect\t${b64(url)}\t${b64('sa')}\t` : `connect\t${b64(url)}\t${b64('sa')}\t\t${b64('org.h2.Driver')}\t10`;
  const response = await client.request(connect); assert.equal(response.error, undefined, `actual Java connect: ${response.error}`);
  const capabilities = await client.command('capabilities');
  client.session = capabilities.session_id; client.catalog = capabilities.catalog; client.capabilities = capabilities;
  return client;
}

for (const kind of ['native', 'jdbc']) {
  test(`real ${kind} H2: PreparedStatement types, ownership, CAS, keys, metadata and refusal`, { timeout: 120000 }, async t => {
    if (!available) { t.skip('official H2 JAR unavailable: real database tests NOT VERIFIED'); return; }
    const db = await open(kind);
    assert.equal(db.capabilities.product, 'H2'); assert.equal(db.capabilities.row_write, true);
    assert.equal(db.capabilities.ddl_write, false);
    await db.query(`CREATE TABLE TYPED (ID BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, BIG BIGINT, AMT DECIMAL(38,12), TXT VARCHAR(200), NIL VARCHAR(20), EMPTY VARCHAR(20), BIN VARBINARY(64), B BOOL, D DATE, TM TIME(9), TS TIMESTAMP(9), TZ TIMESTAMP(9) WITH TIME ZONE, UID UUID, Q INTEGER DEFAULT 7 CHECK (Q>=0), G INTEGER GENERATED ALWAYS AS (Q*2))`, 'PUBLIC');
    let metadata = await db.metadata('PUBLIC', 'TYPED');
    const values = [
      ['BIG', v('integer', '9223372036854775807')], ['AMT', v('decimal', '12345678901234567890.123456789012')],
      ['TXT', v('text', `中文;O'Reilly\\尾\nemoji🙂`)], ['NIL', v('null')], ['EMPTY', v('text', '')], ['BIN', v('binary', 'AP8BgA==')],
      ['B', v('bool', true)], ['D', v('date', '2026-02-28')], ['TM', v('time', '12:34:56.123456789')],
      ['TS', v('timestamp', '2026-11-09T12:34:56.123456789')], ['TZ', v('timestamptz', '2026-11-09T12:34:56.123456789+05:30')],
      ['UID', v('uuid', 'b792cb0f-e22f-4de8-bc73-7656bf6fe221')],
    ];
    let insert;
    await t.test('lossless BIGINT/DECIMAL/NULL/empty/binary/Unicode/date/nanoseconds/timezone plus generated keys', async () => {
      assert.equal(metadata.columns.find(c => c.name === 'ID').auto_increment, true);
      assert.equal(metadata.columns.find(c => c.name === 'G').generated, true);
      assert.ok(metadata.constraints.length); assert.match(metadata.revision, /^[0-9a-f]{64}$/);
      insert = await db.command('row_plan', db.plan(metadata, 'insert', values));
      assert.equal(insert.status, 'applied', JSON.stringify(insert)); assert.equal(insert.verified, true);
      assert.equal(insert.affected_rows, '1'); assert.equal(insert.matched_rows, '1'); assert.equal(insert.transaction, 'committed');
      assert.deepEqual(insert.generated_keys, [['ID', v('integer', '1')]]);
      for (const [name, expected] of values) assert.deepEqual(new Map(insert.readback).get(name), expected, name);
      assert.deepEqual(new Map(insert.readback).get('G'), v('integer', '14'));
      const fresh = await db.metadata('PUBLIC', 'TYPED'); assert.equal(fresh.revision, metadata.revision, 'identity base/cardinality must not invalidate structure');
    });
    await t.test('true no-op is proven on locked row, affected=0, not guessed from driver count', async () => {
      const result = await db.command('row_plan', db.plan(metadata, 'update', [['TXT', values[2][1]]], [['ID', v('integer', '1')]], [['AMT', values[1][1]]]));
      assert.equal(result.status, 'unchanged', JSON.stringify(result)); assert.equal(result.verified, true);
      assert.equal(result.affected_rows, '0'); assert.equal(result.matched_rows, '1');
    });
    await t.test('old values/zero match refuse commit and preserve original row', async () => {
      const result = await db.command('row_plan', db.plan(metadata, 'update', [['TXT', v('text', 'not-written')]], [['ID', v('integer', '1')]], [['TXT', v('text', 'stale')]]));
      assert.equal(result.status, 'conflict'); assert.equal(result.transaction, 'rolled_back'); assert.equal(result.affected_rows, '0');
      assert.equal(result.matched_rows, '0'); assert.equal(result.verified, false);
      assert.equal((await db.query('SELECT TXT FROM TYPED WHERE ID=1', 'PUBLIC')).rows[0][0], values[2][1].value);
    });
    await t.test('readonly/generated/identity/precision mismatch do not change data', async () => {
      const savedReadonly = db.plan(metadata, 'update', [['TXT', v('text', 'unsafe')]], [['ID', v('integer', '1')]]); savedReadonly.read_only = true;
      const readonly = await db.envelope('row_plan', savedReadonly); assert.equal(readonly.error.code, 'READ_ONLY_VIOLATION');
      for (const change of [['G', v('integer', '99')], ['ID', v('integer', '99')], ['BIG', v('integer', '9223372036854775808')], ['AMT', v('decimal', '1.1234567890123')], ['NIL', v('text', 'x'.repeat(21))]]) {
        const result = await db.command('row_plan', db.plan(metadata, 'update', [change], [['ID', v('integer', '1')]]));
        assert.equal(result.verified, false, JSON.stringify(result)); assert.equal(result.affected_rows, '0'); assert.equal(result.transaction, 'rolled_back');
      }
      assert.equal((await db.query('SELECT ID,Q,G FROM TYPED', 'PUBLIC')).rows[0][0], '1');
    });
    await t.test('unique candidate key and changed nonidentity primary key read back exact target', async () => {
      await db.query(`CREATE TABLE KEYED (ID BIGINT PRIMARY KEY, BUSINESS VARCHAR(50) UNIQUE NOT NULL, TXT VARCHAR(50)); INSERT INTO KEYED VALUES(9007199254740993,'one','before')`, 'PUBLIC');
      const m = await db.metadata('PUBLIC', 'KEYED'); assert.ok(m.unique_keys.some(key => key.join(',') === 'BUSINESS'));
      let result = await db.command('row_plan', db.plan(m, 'update', [['TXT', v('text', 'after')]], [['BUSINESS', v('text', 'one')]], [['TXT', v('text', 'before')]]));
      assert.equal(result.verified, true, JSON.stringify(result));
      result = await db.command('row_plan', db.plan(m, 'update', [['ID', v('integer', '9007199254740995')]], [['ID', v('integer', '9007199254740993')]], [['TXT', v('text', 'after')]]));
      assert.equal(result.verified, true, JSON.stringify(result)); assert.deepEqual(new Map(result.readback).get('ID'), v('integer', '9007199254740995'));
    });
    await t.test('same-name/quoted/Unicode objects and concurrent namespace requests do not leak', async () => {
      await db.query('CREATE SCHEMA OTHER; CREATE TABLE OTHER.TYPED(ID BIGINT PRIMARY KEY,TXT VARCHAR(50)); INSERT INTO OTHER.TYPED VALUES(1,\'OTHER\'); CREATE TABLE "中文_带%号"("键" BIGINT PRIMARY KEY,"值" VARCHAR(20))', 'PUBLIC');
      const [publicMeta, otherMeta, unicodeMeta] = await Promise.all([db.metadata('PUBLIC', 'TYPED'), db.metadata('OTHER', 'TYPED'), db.metadata('PUBLIC', '中文_带%号')]);
      assert.notEqual(publicMeta.revision, otherMeta.revision); assert.equal(unicodeMeta.columns[0].name, '键');
      const result = await db.command('row_plan', db.plan(otherMeta, 'update', [['TXT', v('text', 'ONLY-OTHER')]], [['ID', v('integer', '1')]]));
      assert.equal(result.verified, true, JSON.stringify(result));
      const [publicRows, otherRows] = await Promise.all([db.query('SELECT TXT FROM TYPED', 'PUBLIC'), db.query('SELECT TXT FROM TYPED', 'OTHER')]);
      assert.equal(publicRows.rows[0][0], values[2][1].value); assert.equal(otherRows.rows[0][0], 'ONLY-OTHER');
      assert.equal((await db.query('SELECT TXT FROM TYPED')).rows[0][0], values[2][1].value);
    });
    await t.test('metadata revisions, absent keys, stale sessions and ordinary SQL injection are rejected', async () => {
      await db.query('CREATE TABLE NO_KEY(ID INT,TXT VARCHAR(30)); INSERT INTO NO_KEY VALUES(1,\'same\'),(1,\'same\')', 'PUBLIC');
      const noKey = await db.metadata('PUBLIC', 'NO_KEY');
      const missing = await db.command('row_plan', db.plan(noKey, 'delete', [], [['ID', v('integer', '1')]])); assert.equal(missing.verified, false); assert.equal(missing.affected_rows, '0');
      assert.equal((await db.query('SELECT COUNT(*) FROM NO_KEY', 'PUBLIC')).rows[0][0], '2');
      const changed = await db.metadata('PUBLIC', 'TYPED'); await db.query('ALTER TABLE TYPED ADD EXTRA INTEGER DEFAULT 0', 'PUBLIC');
      const old = await db.command('row_plan', db.plan(changed, 'update', [['TXT', v('text', 'unsafe')]], [['ID', v('integer', '1')]])); assert.equal(old.status, 'conflict'); assert.equal(old.verified, false);
      const stale = db.plan(changed, 'update', [], [['ID', v('integer', '1')]]); stale.session_id = randomUUID();
      assert.equal((await db.envelope('row_plan', stale)).error.code, 'STALE_CONTEXT');
      const unexpected = { ...db.plan(changed, 'update', [], []), sql: 'DROP TABLE TYPED' };
      assert.equal((await db.envelope('row_plan', unexpected)).error.code, 'INVALID_REQUEST');
      assert.equal((await db.query('SELECT COUNT(*) FROM TYPED', 'PUBLIC')).rows[0][0], '1');
    });
    await t.test('view definition/constraints are real but view writes remain unsupported', async () => {
      await db.query('CREATE VIEW V_TYPED AS SELECT ID,TXT FROM TYPED', 'PUBLIC');
      const view = await db.metadata('PUBLIC', 'V_TYPED'); assert.equal(view.table_type, 'VIEW'); assert.match(view.definition, /CREATE.*VIEW[\s\S]*SELECT/);
      const result = await db.command('row_plan', db.plan(view, 'delete', [], [['ID', v('integer', '1')]])); assert.equal(result.verified, false);
    });
    await t.test('verified delete commits once and missing-row repeat is conflict', async () => {
      metadata = await db.metadata('PUBLIC', 'TYPED');
      const result = await db.command('row_plan', db.plan(metadata, 'delete', [], [['ID', v('integer', '1')]], [['TXT', values[2][1]]]));
      assert.equal(result.verified, true, JSON.stringify(result)); assert.equal(result.affected_rows, '1');
      assert.equal((await db.query('SELECT COUNT(*) FROM TYPED', 'PUBLIC')).rows[0][0], '0');
      const again = await db.command('row_plan', db.plan(metadata, 'delete', [], [['ID', v('integer', '1')]])); assert.equal(again.status, 'conflict'); assert.equal(again.affected_rows, '0');
    });
    await db.close();
  });
  test(`real ${kind} H2 compatibility mode is not mislabeled supported`, { timeout: 60000 }, async t => {
    if (!available) { t.skip('official H2 JAR unavailable: NOT VERIFIED'); return; }
    const db = await open(kind, ';MODE=MySQL');
    assert.equal(db.capabilities.row_write, false); assert.equal(db.capabilities.metadata, false); assert.match(db.capabilities.reason, /compatibility modes/);
    assert.equal((await db.query('SELECT 1')).rows[0][0], '1');
    const result = await db.envelope('metadata', { object: { catalog: db.catalog, schema: 'PUBLIC', name: 'MISSING' } });
    assert.equal(result.error.code, 'UNSUPPORTED_CAPABILITY'); await db.close();
  });
}

test('real H2 fault injection: counts, races, generated keys, commit acknowledgements and strict codec', { timeout: 120000 }, async t => {
  if (!available) { t.skip('official H2 JAR unavailable: fault/transaction tests NOT VERIFIED'); return; }
  const source = await readFile(path.join(root, 'tools/tests/JdbcMutationFaultTests.java'), 'utf8');
  const directory = await mkdtemp(path.join(temporary, 'faults-'));
  const target = path.join(directory, 'JdbcMutationFaultTests.java');
  await writeFile(target, source + '\n' + support);
  const child = spawn(java, ['-Dfile.encoding=UTF-8', '--class-path', jar, target], { cwd: directory, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  let stdout = '', stderr = '';
  child.stdout.setEncoding('utf8'); child.stdout.on('data', chunk => { stdout += chunk; });
  child.stderr.setEncoding('utf8'); child.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-12000); });
  const result = new Promise((resolve, reject) => {
    child.once('error', reject); child.once('close', code => resolve(code));
  });
  const deadline = setTimeout(() => child.kill(), 90000);
  try {
    const code = await result;
    assert.equal(code, 0, `actual JDBC fault test exit ${code}:\n${stdout}\n${stderr}`);
    const count = stdout.match(/PL20_FAULT_TESTS_PASS (\d+)/);
    assert.ok(count && Number(count[1]) >= 100, `fault and real object-generation/ABA assertions not all executed: ${stdout}`);
    for (const evidence of ['no-replay count-unknown', 'locked no-op', 'concurrent metadata', 'concurrent row CAS', 'real trigger rewrite',
      'lost commit acknowledgement', 'missing rollback acknowledgement', 'postcommit cleanup', 'BigDecimal', 'incorrect-key rollback', 'partial-frame rejection',
      'physical Table generation', 'uncertified schema-object generation']) {
      assert.ok(stdout.includes(evidence), `missing executed fault evidence: ${evidence}\n${stdout}`);
    }
    t.diagnostic(stdout.trim());
  } finally { clearTimeout(deadline); if (child.exitCode === null) child.kill(); }
});
