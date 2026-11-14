// Release candidate smoke: real Tauri IPC and file SQLite, isolated beside a copied exe.
import assert from 'node:assert/strict';
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { chromium } from 'playwright-core';

assert(process.env.NAVKIT_EXE, 'NAVKIT_EXE must explicitly select the freshly built candidate');
const source = path.resolve(process.env.NAVKIT_EXE);
const directory = path.resolve('artifacts', `release-native-${randomUUID()}`);
await mkdir(directory, { recursive: true });
const exe = path.join(directory, 'navkit.exe');
await copyFile(source, exe);
const report = { status: 'FAIL', source, directory, sha256: createHash('sha256').update(await readFile(exe)).digest('hex'), checks: [], errors: [] };
const port = Number(process.env.NATIVE_CDP_PORT || 9383);
assert(Number.isInteger(port) && port > 0 && port <= 65535);
const child = spawn(exe, [], { cwd: directory, stdio: 'inherit', env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` } });
let browser, page, startupError;
child.on('error', error => { startupError = error; });
const check = name => { report.checks.push(name); console.log(`PASS ${name}`); };
const invoke = async (command, args = {}) => {
  const reply = await page.evaluate(async ({ command, args }) => {
    try { return { ok: true, value: await window.__TAURI_INTERNALS__.invoke(command, args) }; }
    catch (error) { return { ok: false, error: typeof error === 'string' ? { message: error } : { ...error, message: error?.message ?? JSON.stringify(error) } }; }
  }, { command, args });
  if (!reply.ok) throw Object.assign(new Error(reply.error.message), reply.error);
  return reply.value;
};
const queryRows = async (id, sql) => {
  const result = await invoke('execute_sql', { id, database: 'main', sql });
  return result.sets[0]?.rows ?? [];
};
try {
  const deadline = Date.now() + 45000;
  while (Date.now() < deadline) {
    if (startupError) throw startupError;
    if (child.exitCode !== null) throw new Error(`Candidate exited before startup: ${child.exitCode}`);
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 1000 }); break; }
    catch { await new Promise(resolve => setTimeout(resolve, 200)); }
  }
  assert(browser, 'Candidate WebView2 did not start');
  page = browser.contexts()[0].pages()[0];
  page.on('pageerror', error => report.errors.push(error.message));
  await page.waitForFunction(() => window.__APP__, null, { timeout: 30000 });
  assert.equal(await page.evaluate(() => Boolean(window.__PREVIEW_MOCK__)), false);
  assert.deepEqual(await invoke('list_connections'), []);
  report.assets = await page.evaluate(() => [...document.scripts].map(script => script.src).filter(Boolean));
  check('Fresh candidate starts with real Tauri bridge and isolated empty data');

  for (const id of ['release-source', 'release-target']) {
    await invoke('save_connection', { config: { id, name: id, kind: 'sqlite', host: '', port: 0, user: '', password: '', database: 'main', file: path.join(directory, `${id}.sqlite`), params: '', group: '', read_only: false, keep_alive: false }, password: '' });
    assert.equal((await invoke('open_connection', { id })).ok, true);
  }
  await invoke('execute_sql', { id: 'release-source', database: 'main', sql: "CREATE TABLE items(id INTEGER PRIMARY KEY, base INTEGER DEFAULT 5, derived INTEGER GENERATED ALWAYS AS (base*2) STORED); INSERT INTO items(id,base) VALUES(1,7); INSERT INTO items(id) VALUES(2); CREATE TABLE desc_key(id INTEGER PRIMARY KEY DESC, value TEXT);" });
  assert.deepEqual(await queryRows('release-source', 'SELECT id,base,derived FROM items ORDER BY id'), [['1', '7', '14'], ['2', '5', '10']]);
  check('Native SQLite query preserves defaulted and generated values');

  const req = { connection_id: 'release-source', schema: 'main', table: 'items', offset: 0, limit: 1, include_total: false, order_by: 'id', ascending: true, filters: [], filter_logic: 'AND', order_rules: null };
  const paged = await invoke('page_table', { req });
  assert.equal(paged.rows.length, 1); assert.equal(paged.has_more, true); assert.equal(paged.total_exact, false);
  const count = await invoke('page_table', { req: { ...req, include_total: true, limit: 0 } });
  assert.equal(count.total, 2); assert.equal(count.total_exact, true); assert.equal(count.rows.length, 0);
  check('Native page-only and separate count-only contracts');

  const backup = await invoke('create_backup', { opts: { connection_id: 'release-source', schema: 'main', tables: ['items'], routines: [], include_structure: true, include_data: true, comment: 'release acceptance', file_name: 'generated-roundtrip', skip_incompatible_tables: false, source_consistency: 'compat' } });
  assert.equal(backup.rows, 2); assert(backup.bytes > 0);
  const dump = await readFile(backup.path, 'utf8');
  assert.match(dump, /GENERATED ALWAYS/);
  const targetBackupDirectory = path.join(directory, 'data', 'backups', 'release-target', 'main');
  await mkdir(targetBackupDirectory, { recursive: true });
  const restoreFile = 'restore.sql'; await writeFile(path.join(targetBackupDirectory, restoreFile), dump);
  const restored = await invoke('restore_backup_detailed', { connectionId: 'release-target', schemaName: 'main', file: restoreFile, taskId: randomUUID() });
  assert.equal(restored.failed, 0); assert.equal(restored.effects_unknown, false); assert.equal(restored.unknown, 0);
  assert.equal(restored.committed, restored.total); assert.equal(restored.cleanup_error, null);
  assert.deepEqual(await queryRows('release-target', 'SELECT id,base,derived FROM items ORDER BY id'), [['1', '7', '14'], ['2', '5', '10']]);
  check('Public backup and detailed restore roundtrip generated columns without false success');

  await assert.rejects(invoke('create_backup', { opts: { connection_id: 'release-source', schema: 'main', tables: ['missing'], routines: [], include_structure: true, include_data: true, comment: '', file_name: 'missing-selection', skip_incompatible_tables: false, source_consistency: 'compat' } }), error => error.message.includes('BACKUP_OBJECT_MISSING'));
  const backups = await invoke('list_backups', { connectionId: 'release-source', schemaName: 'main' });
  assert.equal(backups.length, 1);
  check('Missing explicit backup selection fails and never publishes an empty backup');

  const validRecipe = { version: 1, connection_id: 'release-source', schema: 'main', table: 'items', total: 1000, seed: 'release', frozen_time: '2026-11-14T08:00:00Z', schema_fingerprint: '', transaction_mode: 'atomic', fields: [{ name: 'id', mode: 'omit', value: '' }, { name: 'base', mode: 'constant', value: '9' }, { name: 'derived', mode: 'omit', value: '' }] };
  const sample = await invoke('preview_generated_recipe', { recipe: validRecipe, ownerId: 'release-smoke' });
  assert.equal(sample.rows.length, 20); assert.deepEqual(sample.columns, ['base']);
  assert(sample.rows.every(row => row.length === 1 && row[0] === '9'));
  assert.equal(sample.recipe.total, 1000); assert.match(sample.recipe.schema_fingerprint, /^[a-f0-9]{64}$/);
  assert.deepEqual(await queryRows('release-source', 'SELECT COUNT(*) FROM items'), [['2']]);
  check('Public large recipe preview passes typed registration and never inserts sample rows');

  const recipe = { version: 1, connection_id: 'release-source', schema: 'main', table: 'desc_key', total: 3, seed: 'release', frozen_time: '2026-11-14T08:00:00Z', schema_fingerprint: '', transaction_mode: 'atomic', fields: [{ name: 'id', mode: 'omit', value: '' }, { name: 'value', mode: 'constant', value: 'safe' }] };
  await assert.rejects(invoke('preview_generated_recipe', { recipe, ownerId: 'release-smoke' }), error => error.message.includes('主键') || error.message.includes('省略'));
  assert.deepEqual(await queryRows('release-source', 'SELECT COUNT(*) FROM desc_key'), [['0']]);
  check('Public recipe preview rejects the DESC primary key omission before writes');

  await page.reload({ waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.__APP__?.connectionsValue().length === 2);
  await page.evaluate(() => window.__APP__.openTableTab('release-source', 'main', 'items', 'table'));
  await page.waitForFunction(() => !window.__APP__.activeView()?.loading && window.__APP__.activeView()?.tab.page?.rows.length === 2);
  await page.screenshot({ path: path.join(directory, 'data-grid.png'), fullPage: true });
  check('Fresh embedded frontend opens actual SQLite table data');
  assert.deepEqual(report.errors, []);
  report.status = 'PASS';
} catch (error) {
  report.error = { message: error.message ?? String(error), stack: error.stack }; console.error(error); process.exitCode = 1;
} finally {
  if (browser) await browser.close().catch(() => {});
  if (child.exitCode === null) { const exited = new Promise(resolve => child.once('exit', resolve)); child.kill(); await exited; }
  await writeFile(path.join(directory, 'report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(`RESULT ${report.status}; report: ${path.join(directory, 'report.json')}`);
}
