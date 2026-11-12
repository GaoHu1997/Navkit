// New feature desktop acceptance. Always pin a freshly built custom-protocol exe.
// NAVKIT_EXE=<absolute exe> NAVKIT_EXE_SHA256=<sha256> node tools/tests/object-mutation-desktop-native.mjs
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { createServer } from 'node:net';
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import { DatabaseSync } from 'node:sqlite';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const source = process.env.NAVKIT_EXE, expected = process.env.NAVKIT_EXE_SHA256;
assert.ok(source && path.isAbsolute(source), 'Explicit current native executable is required');
assert.match(expected || '', /^[a-f0-9]{64}$/, 'Pin executable SHA256; no old executable fallback');
const directory = path.join(root, 'tools/test-artifacts', `object-mutation-native-${randomUUID()}`);
await mkdir(directory, { recursive: true });
const report = { started: new Date().toISOString(), directory, source, sha256: expected, checks: [], ui: [], cleanup_errors: [], errors: [] };
const persist = () => writeFile(path.join(directory, 'report.json'), JSON.stringify(report, null, 2) + '\n');
const check = text => { report.checks.push(text); console.log(`PASS ${text}`); };
const hash = data => createHash('sha256').update(data).digest('hex');
const delay = ms => new Promise(r => setTimeout(r, ms));
const q = name => `"${name.replaceAll('"', '""')}"`;
const id = randomUUID(), owner = `object-acceptance:${randomUUID()}`, target = '用户 " 审计';
const object = { connection_id: id, catalog: null, schema: 'main', kind: 'trigger', name: target, identity_arguments: null, parent: { schema: 'main', kind: 'table', name: 'items' } };
let child, browser, page, failure;
async function freePort() { const s = createServer(); await new Promise((ok, no) => { s.once('error', no); s.listen(0, '127.0.0.1', ok); }); const port = s.address().port; await new Promise((ok, no) => s.close(e => e ? no(e) : ok())); return port; }
async function ipc(command, args = {}) {
  const response = await page.evaluate(async ({ command, args }) => {
    try { return { ok: true, value: await window.__TAURI_INTERNALS__.invoke(command, args) }; }
    catch (e) { return { ok: false, error: typeof e === 'string' ? { message: e } : { message: e.message, code: e.code } }; }
  }, { command, args });
  if (!response.ok) throw Object.assign(new Error(response.error.message), { code: response.error.code });
  return response.value;
}
async function grant(sql, ownerId = owner) { return ipc('issue_execution_grant', { request: { execution_id: randomUUID(), connection_id: id, namespace: 'main', owner_id: ownerId, sql, timeout_ms: 15000, origin: 'object-desktop-fixture' } }); }
async function sql(text) { const result = await ipc('execute_sql_v2', { request: await grant(text) }); assert.equal(result.state, 'succeeded', JSON.stringify(result)); assert.equal(result.resources_pending, false); for (const set of result.result.sets) assert.equal(set.ok, true, JSON.stringify(set)); return result.result; }
async function scalar(text) { const result = await sql(text); assert.equal(result.sets.length, 1); assert.equal(result.sets[0].rows.length, 1); return result.sets[0].rows[0][0]; }
async function present() { return scalar(`SELECT count(*) FROM main.sqlite_schema WHERE type='trigger' AND name='${target.replaceAll("'", "''")}'`); }
async function navigate() {
  await page.evaluate(({ id }) => window.__APP__.openObjectsTab(id, 'main', 'trigger'), { id });
  await page.waitForFunction(() => document.querySelector('.object-browser')?.getAttribute('aria-busy') === 'false');
  await page.locator('.object-browser td[data-col]').filter({ hasText: target }).first().click();
  await page.waitForFunction(() => { const b = document.querySelector('[data-command="object-delete"]'); return b && !b.disabled; });
  const selected = await page.evaluate(() => { const app = window.__APP__; return app.views.get(app.activeTab.id)?.selectedObject()?.descriptor?.object; });
  assert.deepEqual(selected, object, 'Actual grid selection must retain exact connection, trigger and parent identity');
}
async function openDrop() { await navigate(); await page.locator('[data-command="object-delete"]').click(); const dlg = page.getByRole('dialog', { name: '受控删除触发器', exact: true }); await dlg.getByRole('button', { name: '读取删除预览' }).waitFor(); await page.waitForFunction(() => [...document.querySelectorAll('.dialog-footer button')].some(b => b.textContent === '读取删除预览' && !b.disabled)); return dlg; }
async function preview(dlg) { await dlg.getByRole('button', { name: '读取删除预览' }).click(); await dlg.locator('pre').filter({ hasText: '后端计划：' }).waitFor(); assert.equal(await dlg.getByRole('button', { name: '确认删除' }).isEnabled(), false); assert.equal(await present(), '1', 'baseline/preview cannot delete'); }
function seedTriggerDatabase(file, initial = false) {
  // Only this run's private fixture is seeded externally. No product SQL bypass is installed.
  assert.equal(path.dirname(file), directory); const db = new DatabaseSync(file);
  try {
    if (initial) db.exec('CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT); CREATE TABLE audit(kind TEXT); CREATE VIEW kept_view AS SELECT id,value FROM items; CREATE TRIGGER view_only INSTEAD OF INSERT ON kept_view BEGIN SELECT 1; END; CREATE TRIGGER sibling AFTER INSERT ON items BEGIN INSERT INTO audit VALUES(\'sibling\'); END;');
    db.exec(`CREATE TRIGGER ${q(target)} AFTER INSERT ON items BEGIN INSERT INTO audit VALUES('target'); END`);
    if (initial) db.exec("INSERT INTO items VALUES(1,'kept')");
  } finally { db.close(); }
}
async function waitForExit(child, ms) {
  if (child.exitCode !== null || child.signalCode !== null) return true;
  return new Promise(resolve => {
    const finish = done => { clearTimeout(timer); child.off('exit', exited); resolve(done); };
    const exited = () => finish(true); const timer = setTimeout(() => finish(false), ms);
    child.once('exit', exited);
  });
}
async function bounded(promise, ms) { let timer; try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('bounded shutdown IPC timeout')), ms); })]); } finally { clearTimeout(timer); } }
async function shutDown() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  try { if (page && !page.isClosed()) { await bounded(page.evaluate(async () => { window.__APP__.settings.confirm_on_close = false; await window.__TAURI_INTERNALS__.invoke('plugin:window|close', { label: 'main' }); }), 5000); } } catch (e) { report.close_notice = String(e); }
  await waitForExit(child, 15000);
  if (child.exitCode === null && child.signalCode === null) { child.kill(); await waitForExit(child, 10000); }
  if (child.exitCode === null && child.signalCode === null) {
    const kill = spawn('taskkill', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true });
    await new Promise((ok, no) => { kill.once('error', no); kill.once('exit', code => code === 0 || child.exitCode !== null ? ok() : no(new Error(`owned taskkill ${code}`))); });
    await waitForExit(child, 5000);
  }
  assert.ok(child.exitCode !== null || child.signalCode !== null, 'exact launched child must exit before cleanup');
}
try {
  assert.equal(hash(await readFile(source)), expected);
  const exe = path.join(directory, 'navkit.exe'); await copyFile(source, exe); assert.equal(hash(await readFile(exe)), expected);
  const port = await freePort(); report.port = port;
  child = spawn(exe, [], { cwd: directory, env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-allow-origins=*`, WEBVIEW2_USER_DATA_FOLDER: path.join(directory, 'webview-data') }, stdio: ['ignore', 'pipe', 'pipe'] }); report.pid = child.pid;
  child.stdout.on('data', data => { report.stdout = ((report.stdout || '') + data).slice(-50000); }); child.stderr.on('data', data => { report.stderr = ((report.stderr || '') + data).slice(-50000); });
  child.once('error', e => { failure = e; });
  const until = Date.now() + 60000;
  while (!browser && Date.now() < until) { if (failure) throw failure; assert.equal(child.exitCode, null, 'new binary must launch'); try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 1500 }); } catch { await delay(200); } }
  assert.ok(browser, 'own native WebView CDP endpoint unavailable'); page = browser.contexts().flatMap(c => c.pages())[0]; assert.ok(page); page.setDefaultTimeout(15000);
  page.on('pageerror', e => report.errors.push(String(e))); await page.waitForFunction(() => !!window.__APP__ && !!window.__TAURI_INTERNALS__?.invoke);
  report.url = page.url(); assert.match(report.url, /^http:\/\/tauri\.localhost|^tauri:/); assert.equal(await page.evaluate(() => Boolean(window.__PREVIEW_MOCK__)), false); assert.deepEqual(await ipc('list_connections'), []);
  check('Fresh SHA-pinned custom-protocol native binary, isolated portable storage, real IPC and no preview mock');
  const config = { id, name: '临时对象删除验收', kind: 'sqlite', host: '', port: 0, user: '', password: '', database: '', file: path.join(directory, 'fixture.sqlite'), params: '', group: '', read_only: false, keep_alive: false };
  seedTriggerDatabase(config.file, true); report.fixture_seed = 'Node native SQLite, run-owned fixture only; feature assertions use unmodified Tauri IPC';
  await ipc('save_connection', { config, password: '' }); assert.equal((await ipc('open_connection', { id })).ok, true);
  await page.reload(); await page.waitForFunction(({ id }) => window.__APP__?.connectionById(id), { id });
  const catalogue = await ipc('list_object_catalog', { id, catalog: null, schema: 'main', kinds: ['trigger'], cursor: null });
  assert.equal(catalogue.objects.find(o => o.object.name === target).actions.drop.level, 'limited'); assert.equal(catalogue.objects.find(o => o.object.name === 'view_only').actions.drop.level, 'unsupported');
  const dlg = await openDrop(); assert.equal(await dlg.getByRole('button', { name: '确认删除' }).isEnabled(), false); await preview(dlg);
  report.ui.push({ phase: 'preview', text: await dlg.innerText() }); await page.screenshot({ path: path.join(directory, '01-controlled-preview.png') });
  await dlg.getByRole('checkbox').check(); await dlg.getByRole('button', { name: '确认删除' }).click(); await dlg.waitFor({ state: 'hidden', timeout: 20000 });
  assert.equal(await present(), '0'); assert.equal(await scalar('SELECT count(*) FROM items'), '1'); assert.equal(await scalar("SELECT count(*) FROM main.sqlite_schema WHERE name='sibling' AND type='trigger'"), '1');
  await sql("INSERT INTO items VALUES(2,'post drop')"); assert.equal(await scalar("SELECT count(*) FROM audit WHERE kind='target'"), '1'); assert.equal(await scalar("SELECT count(*) FROM audit WHERE kind='sibling'"), '2');
  const logs = await ipc('list_sql_log', { limit: 200 }); assert.ok(JSON.stringify(logs).includes('OBJECT_MUTATION_DROP'), 'Actual toolbar dialog must use typed C1 marker');
  assert.equal(await scalar('SELECT count(*) FROM kept_view'), '2'); assert.equal(await scalar("SELECT count(*) FROM main.sqlite_schema WHERE name='view_only' AND type='trigger'"), '1');
  await ipc('close_connection', { id }); assert.equal((await ipc('open_connection', { id })).ok, true);
  assert.equal(await present(), '0', 'Committed native DROP must persist across physical connection close/reopen'); assert.equal(await scalar('SELECT count(*) FROM items'), '2');
  const savedFile = JSON.parse(await readFile(path.join(directory, 'data/connections.json'), 'utf8'));
  report.connection_persistence = { bytes: JSON.stringify(savedFile).length, containsRunId: JSON.stringify(savedFile).includes(id) }; assert.equal(report.connection_persistence.containsRunId, true);
  await page.screenshot({ path: path.join(directory, '02-after-ui-delete.png') }); check('Actual object grid selection → toolbar → baseline → preview → checkbox → typed native DROP; physical reopen preserves absence, parent/view/sibling and behavior');
  // A changed complete directory rejects the editor-opening baseline, never rebinds it at save.
  seedTriggerDatabase(config.file);
  const stale = await openDrop(); await preview(stale); await sql('CREATE TABLE changed_catalog(sentinel TEXT)'); await stale.getByRole('checkbox').check(); await stale.getByRole('button', { name: '确认删除' }).click();
  await stale.getByRole('status').filter({ hasText: '禁止重放' }).waitFor(); assert.equal(await stale.getByRole('button', { name: '确认删除' }).isEnabled(), false); assert.equal(await present(), '1'); report.ui.push({ phase: 'stale', text: await stale.innerText() }); await page.screenshot({ path: path.join(directory, '03-stale-rejected.png') }); await stale.getByRole('button', { name: '关闭', exact: true }).click(); check('Real directory change after preview rolls back, retains exact trigger, locks replay and permits closing settled failure');
  const base = await ipc('read_object_mutation_baseline', { object, ownerId: owner });
  const draft = { baseline_id: base.baseline_id, object, source_fingerprint: base.source_fingerprint, base_revision: base.base_revision, operation: 'drop' };
  const plan = await ipc('preview_object_mutation', { draft, ownerId: owner }); const work = { plan_id: plan.plan_id, review_digest: plan.review_digest, draft, confirm_drop: true };
  const noGrant = { execution_id: randomUUID(), connection_id: id, namespace: 'main', owner_id: owner, sql: plan.request_sql, timeout_ms: 15000 };
  await assert.rejects(ipc('execute_object_mutation', { request: noGrant, work }), e => e.code === 'OBJECT_MUTATION_NOT_REGISTERED'); assert.equal(await present(), '1');
  const stopped = await grant(plan.request_sql); stopped.cancel_before_send = true; const cancelled = await ipc('execute_object_mutation', { request: stopped, work }); assert.equal(cancelled.outcome.effects, 'no_effect'); assert.equal(cancelled.outcome.state, 'cancelled'); assert.equal(cancelled.outcome.resources_pending, false); assert.equal(await present(), '1');
  check('Native IPC rejects missing grant before SQL; genuine signed cancel-before-send has settled zero effects');
  await ipc('close_connection', { id });
  check('Connection close confirmed before portable child exit'); report.passed = true;
} catch (error) { failure = error; report.passed = false; report.error = { message: error.message || String(error), code: error.code, stack: error.stack }; }
finally {
  try { await shutDown(); } catch (e) { report.cleanup_errors.push(String(e)); }
  if (browser) try { await browser.close(); } catch (e) { report.cleanup_errors.push(String(e)); }
  report.child = child ? { pid: child.pid, exit: child.exitCode, signal: child.signalCode } : null;
  if (report.errors.length || report.cleanup_errors.length) { report.passed = false; failure ||= new Error('Desktop produced script/cleanup errors'); }
  report.finished = new Date().toISOString(); await persist(); console.log(`EVIDENCE=${path.join(directory, 'report.json')}`);
}
if (failure) throw failure;
console.log(`NATIVE_OBJECT_CHECKS=${report.checks.length} PASS`);
