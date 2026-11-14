import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { execFile } from 'node:child_process';
import { launchOwnedDesignerBrowser, designerPage, finishSave } from './table-designer-fixture.mjs';
let browser, browserServer, browserProcess;
const diagnosticsRoot = path.resolve('artifacts', `table-designer-metadata-diagnostics-${randomUUID()}`);
mkdirSync(diagnosticsRoot, { recursive: true });
console.log(`Metadata diagnostics: ${diagnosticsRoot}; real browser + mock IPC; native commands: 0`);
const lifecycle = { nativeCommandCount: 0, ownerNodePid: process.pid, primary_errors: [], cleanup_errors: [], graceful_errors: [], phases: [] };
const recordLifecycle = phase => {
  lifecycle.phases.push({ phase, at: new Date().toISOString() });
  lifecycle.ownedProcess = browserProcess ? { pid: browserProcess.pid, exitCode: browserProcess.exitCode, signalCode: browserProcess.signalCode } : null;
  lifecycle.clientConnected = browser?.isConnected() ?? false;
  writeFileSync(path.join(diagnosticsRoot, 'browser-lifecycle.json'), JSON.stringify(lifecycle, null, 2));
};
const ownedExited = () => browserProcess && (browserProcess.exitCode !== null || browserProcess.signalCode !== null);
const launchStartedAt = Date.now();
function ownedCleanupCommand(executable, args, timeout, env = process.env) {
  return new Promise((resolve, reject) => {
    execFile(executable, args, { windowsHide: true, timeout, killSignal: 'SIGKILL', maxBuffer: 1024 * 1024, env }, (error, stdout, stderr) => {
      if (error) { error.message += `; stderr=${stderr.trim()}`; reject(error); }
      else resolve(stdout);
    });
  });
}
async function ownedBrowserRows() {
  const profile = lifecycle.browserProfile;
  assert.ok(profile?.includes('playwright_'), 'The tracked browser must have a unique automation profile');
  const command = `$ErrorActionPreference='Stop'; $profile=$env:NAVKIT_METADATA_BROWSER_PROFILE; $rows=@(Get-CimInstance Win32_Process -Filter "Name='chrome.exe' OR Name='msedge.exe' OR Name='chrome_crashpad_handler.exe'" -OperationTimeoutSec 15 | ForEach-Object { [pscustomobject]@{ pid=[int]$_.ProcessId; ppid=[int]$_.ParentProcessId; name=$_.Name; created=$_.CreationDate.ToUniversalTime().ToString('o'); profileMatch=([string]$_.CommandLine).Contains($profile) } }); ConvertTo-Json -InputObject $rows -Compress`;
  const powershell = path.win32.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const rows = JSON.parse(await ownedCleanupCommand(powershell, ['-NoProfile', '-NonInteractive', '-Command', command], 25_000, { ...process.env, NAVKIT_METADATA_BROWSER_PROFILE: profile }));
  assert.ok(Array.isArray(rows), 'Process identity verification must return a complete array');
  return rows;
}
function trackedBrowserTree(rows, previous = []) {
  const identities = new Map(previous.map(row => [row.pid, row.created]));
  const ids = new Set([browserProcess.pid]);
  const found = new Map();
  for (const row of rows) if (row.profileMatch || identities.get(row.pid) === row.created) {
    ids.add(row.pid); found.set(row.pid, row);
  }
  let changed = true;
  while (changed) {
    changed = false;
    for (const row of rows) if (ids.has(row.ppid) && !ids.has(row.pid)) {
      ids.add(row.pid); found.set(row.pid, row); changed = true;
    }
  }
  return [...found.values()];
}
async function confirmOwnedExit() {
  if (ownedExited()) return;
  let removeListeners;
  try {
    await bounded(new Promise((resolve, reject) => {
      const exited = () => resolve();
      const failed = error => reject(error);
      browserProcess.once('exit', exited);
      browserProcess.once('error', failed);
      removeListeners = () => { browserProcess.off('exit', exited); browserProcess.off('error', failed); };
      if (ownedExited()) exited();
    }), 15_000, 'owned browser process exit');
  } finally { removeListeners?.(); }
}
before(async () => {
  recordLifecycle('launch:before');
  try {
    const owned = await launchOwnedDesignerBrowser(server => {
      browserServer = server;
      browserProcess = server.process();
      lifecycle.browserExecutable = browserProcess.spawnfile;
      lifecycle.browserProfile = browserProcess.spawnargs.find(arg => arg.startsWith('--user-data-dir='))?.slice('--user-data-dir='.length);
      assert.ok(browserProcess?.pid, 'The browser server must expose its owned child process');
      browserProcess.once('exit', (code, signal) => {
        lifecycle.processExit = { code, signal, at: new Date().toISOString() };
        recordLifecycle('owned-process:exit');
      });
      recordLifecycle('launch:owned-process');
    });
    browser = owned.browser;
    recordLifecycle('launch:connected');
  } catch (error) {
    lifecycle.primary_errors.push({ stage: 'before', message: String(error) });
    recordLifecycle('launch:error');
    throw error;
  }
}, { timeout: 90_000 });
after(async () => {
  lifecycle.contextsBeforeClose = browser?.contexts().map((context, index) => ({
    index, pages: context.pages().map(page => ({ url: page.url(), closed: page.isClosed() })),
  })) ?? [];
  browser?.once('disconnected', () => {
    lifecycle.clientDisconnectedAt = new Date().toISOString();
    recordLifecycle('client:disconnected');
  });
  recordLifecycle('cleanup:before');
  if (!browserServer) { recordLifecycle('cleanup:no-owned-server'); return; }
  let treeBeforeKill = [];
  if (process.platform === 'win32') {
    try {
      treeBeforeKill = trackedBrowserTree(await ownedBrowserRows());
      lifecycle.treeBeforeClose = treeBeforeKill;
    } catch (error) { lifecycle.cleanup_errors.push({ stage: 'owned-tree-before-close', message: String(error) }); }
  }
  recordLifecycle('cleanup:server-close:before');
  const serverClose = browserServer.close();
  // Windows Chromium can finish its own shutdown more than 18s after the transport
  // disconnects. Keep the exit/tree/error assertions, but do not race that shutdown
  // with a 5s forced taskkill (whose children may already be exiting).
  try { await bounded(serverClose, 30_000, 'browser server graceful close'); }
  catch (error) { lifecycle.graceful_errors.push({ resource: 'server', message: String(error) }); }
  lifecycle.closeMode = 'graceful';
  recordLifecycle('cleanup:server-close:finished');
  if (!ownedExited()) {
    recordLifecycle('cleanup:owned-kill:before');
    try {
      assert.equal(browserServer.process(), browserProcess, 'Only the exact tracked browser child may be terminated');
      if (process.platform === 'win32') {
        const rows = await ownedBrowserRows();
        const root = rows.find(row => row.pid === browserProcess.pid);
        if (!ownedExited()) {
          assert.ok(root, 'The owned root process must exist before termination');
          assert.equal(root.ppid, process.pid, 'The browser root must belong to this test process');
          assert.equal(root.name.toLowerCase(), path.basename(browserProcess.spawnfile).toLowerCase());
          assert.equal(root.profileMatch, true, 'The browser root must use the exact recorded automation profile');
          assert.ok(Date.parse(root.created) >= launchStartedAt - 2000 && Date.parse(root.created) <= Date.now(), 'The root identity must belong to this launch');
          treeBeforeKill = trackedBrowserTree(rows, treeBeforeKill);
          lifecycle.treeBeforeKill = treeBeforeKill;
          lifecycle.closeMode = 'owned-async-taskkill';
          recordLifecycle('cleanup:owned-kill:verified');
          const taskkill = path.win32.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'taskkill.exe');
          lifecycle.taskkillOutput = (await ownedCleanupCommand(taskkill, ['/PID', String(browserProcess.pid), '/T', '/F'], 10_000)).trim();
        }
      } else {
        lifecycle.closeMode = 'owned-server-kill';
        await bounded(browserServer.kill(), 5000, 'owned browser server kill');
      }
    } catch (error) { lifecycle.cleanup_errors.push({ stage: 'owned-kill', message: String(error) }); }
    recordLifecycle('cleanup:owned-kill:after');
  }
  try { await confirmOwnedExit(); }
  catch (error) { lifecycle.cleanup_errors.push({ stage: 'owned-process-exit', message: String(error) }); }
  recordLifecycle('cleanup:owned-exit:checked');
  recordLifecycle('cleanup:client-close:before');
  const clientClose = browser?.close() ?? Promise.resolve();
  try { await bounded(clientClose, 5000, 'browser client graceful close'); }
  catch (error) { lifecycle.graceful_errors.push({ resource: 'client', message: String(error) }); }
  recordLifecycle('cleanup:graceful-finished');
  if (process.platform === 'win32') {
    try {
      lifecycle.remainingOwnedTree = trackedBrowserTree(await ownedBrowserRows(), treeBeforeKill);
      assert.deepEqual(lifecycle.remainingOwnedTree, [], 'No recorded browser child or unique-profile process may remain');
    } catch (error) { lifecycle.cleanup_errors.push({ stage: 'owned-tree-verification', message: String(error) }); }
  }
  try { await bounded(Promise.all([clientClose, serverClose]), 5000, 'browser transport and temporary directory cleanup'); }
  catch (error) { lifecycle.cleanup_errors.push({ stage: 'transport-and-temporary-cleanup', message: String(error) }); }
  if (browser?.isConnected()) lifecycle.cleanup_errors.push({ stage: 'client-disconnect', message: 'Browser client remained connected after owned cleanup' });
  lifecycle.processExitConfirmed = !!ownedExited();
  recordLifecycle('cleanup:finished');
  assert.equal(lifecycle.processExitConfirmed, true, 'The exact browser server child process must have exited');
  assert.deepEqual(lifecycle.cleanup_errors, [], 'Owned browser cleanup must complete without unresolved errors');
  assert.deepEqual(lifecycle.graceful_errors, [], 'Browser server and client closure must complete without errors');
}, { timeout: 150_000 });
const tab = (page, name) => page.getByRole('tab', { name, exact: true }).click();
let nextCase = 0;
async function bounded(operation, milliseconds, label) {
  let timer;
  try {
    return await Promise.race([operation, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds}ms`)), milliseconds);
    })]);
  } finally { clearTimeout(timer); }
}
async function diagnosedPage(t, run) {
  const directory = path.join(diagnosticsRoot, String(++nextCase).padStart(2, '0'));
  mkdirSync(directory);
  const report = { test: t.name, nativeCommandCount: 0, phases: [] };
  const record = (phase, details = {}) => {
    report.phase = phase;
    report.phases.push({ phase, at: new Date().toISOString(), ...details });
    writeFileSync(path.join(directory, 'phases.json'), JSON.stringify(report, null, 2));
  };
  let page, originalEvaluate, capturePromise, closePromise, rejectAbort, sequence = 0;
  const aborted = new Promise((_, reject) => { rejectAbort = reject; });
  aborted.catch(() => {});
  const capture = error => capturePromise ??= (async () => {
    report.error = { name: error.name, message: error.message, stack: error.stack };
    lifecycle.primary_errors.push({ stage: t.name, ...report.error });
    recordLifecycle('case:failure');
    record('failure:capture', { waitingAt: report.phase });
    if (!page || page.isClosed()) { record('failure:page-unavailable'); return; }
    const observations = await Promise.allSettled([
      bounded(originalEvaluate(() => ({
        readyState: document.readyState,
        busy: globalThis.designer?.draftBusy?.(),
        loading: globalThis.designer?.loading,
        saving: globalThis.designer?.saving,
        preview: globalThis.designer?.preview,
        draft: globalThis.designer?.draft,
        dialogs: [...document.querySelectorAll('[role="dialog"]')].map(node => ({ label: node.getAttribute('aria-label'), text: node.textContent })),
        fixture: globalThis.fixture && {
          calls: fixture.calls ?? null,
          previews: fixture.previews, executions: fixture.executions, grants: fixture.grants,
          notices: fixture.notices, referenceCalls: fixture.referenceCalls,
          referenceCatalogCalls: fixture.referenceCatalogCalls ?? null,
        },
      })), 1500, 'failure state capture'),
      bounded(page.content(), 1500, 'failure DOM capture'),
      page.screenshot({ path: path.join(directory, 'failure.png'), fullPage: true, timeout: 1500 }),
    ]);
    for (const [index, observation] of observations.entries()) {
      const label = ['state', 'dom', 'screenshot'][index];
      if (observation.status === 'rejected') {
        report[`${label}Error`] = String(observation.reason);
      } else if (index === 0) {
        writeFileSync(path.join(directory, 'state.json'), JSON.stringify(observation.value, null, 2));
      } else if (index === 1) {
        writeFileSync(path.join(directory, 'failure.html'), observation.value);
      }
    }
    record('failure:captured');
  })();
  const closePage = () => {
    if (!page || page.isClosed()) return Promise.resolve();
    return closePromise ??= (async () => {
      record('cleanup:page-close:before');
      await bounded(page.close(), 5000, 'owned page close');
      record('cleanup:page-close:after');
    })();
  };
  const onAbort = () => {
    const error = t.signal.reason instanceof Error ? t.signal.reason : new Error(`Test aborted while ${report.phase}`);
    void capture(error).catch(error => console.error('Metadata failure capture failed:', error));
    rejectAbort(error);
  };
  t.signal.addEventListener('abort', onAbort, { once: true });
  t.after(async () => {
    t.signal.removeEventListener('abort', onAbort);
    if (capturePromise) await capturePromise;
    await closePage();
  }, { timeout: 8000 });
  try {
    record('fixture:create:before');
    const observedBrowser = {
      async newPage(options) {
        page = await browser.newPage(options);
        originalEvaluate = page.evaluate.bind(page);
        page.evaluate = async (fn, arg) => {
          const label = `evaluate:${++sequence}:${String(fn).slice(0, 180)}`;
          record(`${label}:before`);
          try {
            const value = await Promise.race([originalEvaluate(fn, arg), aborted]);
            record(`${label}:after`);
            return value;
          } catch (error) { record(`${label}:error`, { message: String(error) }); throw error; }
        };
        if (t.signal.aborted) { await closePage(); throw t.signal.reason; }
        return page;
      },
    };
    await Promise.race([designerPage(observedBrowser), aborted]);
    record('fixture:ready');
    await Promise.race([run(page), aborted]);
    record('assertions:passed');
  } catch (error) {
    await capture(error);
    throw error;
  } finally {
    t.signal.removeEventListener('abort', onAbort);
    if (capturePromise) await capturePromise;
    await closePage();
  }
}

test('unfinished fillfactor stays in the real input and captured draft and cannot silently clear an option', { timeout: 30_000 }, async t => diagnosedPage(t, async page => {
  await tab(page, '选项');
  const fillfactor = page.getByRole('textbox', { name: '填充因子', exact: true });
  await fillfactor.fill('1e');
  assert.equal(await fillfactor.inputValue(), '1e');
  assert.equal(await page.evaluate(() => designer.captureDraft().payload.draft.options.fillfactor), '1e');
  await page.evaluate(async () => designer.saveChanges());
  assert.equal(await page.evaluate(() => fixture.previews.length), 0);
  assert.equal(await page.evaluate(() => fixture.executions.length), 0);
  assert.equal(await page.evaluate(() => designer.draftDirty()), true);
  assert.ok((await page.evaluate(() => fixture.notices)).some(([kind, text]) => kind === 'error' && text.includes('填充因子必须')));
  await fillfactor.fill('80');
  await page.evaluate(() => { globalThis.saving = designer.saveChanges(); });
  await page.getByRole('dialog', { name: '保存表结构', exact: true }).waitFor(); await finishSave(page);
  assert.equal(await page.evaluate(() => fixture.executions[0].design.options.fillfactor), 80);
  assert.equal(await page.evaluate(() => designer.draftDirty()), false);
}));

test('invalid foreign-key metadata preserves legacy values and only a real namespace, table and ordered column can be saved', { timeout: 30_000 }, async t => diagnosedPage(t, async page => {
  await page.evaluate(async () => {
    fixture.snapshot.design.foreign_keys = [{ name: 'legacy_fk', columns: ['id'], ref_schema: 'missing_schema', ref_table: 'missing_table', ref_columns: ['old_column'], on_update: null, on_delete: null }];
    await designer.reloadAfterLeave();
  });
  await tab(page, '外键'); await page.waitForFunction(() => designer.namespaces.values !== null);
  const namespace = page.getByRole('combobox', { name: '外键 1 引用库', exact: true });
  const table = page.getByRole('combobox', { name: '外键 1 引用表', exact: true });
  assert.equal(await namespace.inputValue(), 'missing_schema'); assert.equal(await table.inputValue(), 'missing_table');
  assert.match(await page.getByRole('tabpanel').textContent(), /引用库「missing_schema」已失效/);
  await page.getByRole('textbox', { name: '外键 1 名称', exact: true }).fill('edited_fk');
  await page.evaluate(async () => designer.saveChanges()); assert.equal(await page.evaluate(() => fixture.previews.length), 0);
  await namespace.selectOption('db'); await page.waitForFunction(() => designer.referenceTables.get('db')?.values !== null && !designer.referenceTables.get('db')?.loading);
  assert.equal(await table.inputValue(), 'missing_table'); assert.equal(await table.locator('option[value="parents_view"]').count(), 0);
  await page.evaluate(async () => designer.saveChanges()); assert.equal(await page.evaluate(() => fixture.previews.length), 0);
  await table.selectOption('parents'); await page.waitForFunction(() => fixture.referenceCalls.length === 1 && !!designer.references.get(JSON.stringify(['db', 'parents']))?.columns);
  const column = page.getByRole('combobox', { name: '外键 1 引用字段', exact: true });
  assert.equal(await column.inputValue(), 'old_column');
  await page.evaluate(async () => designer.saveChanges()); assert.equal(await page.evaluate(() => fixture.previews.length), 0);
  assert.equal(await page.evaluate(() => designer.draft.foreign_keys[0].ref_columns[0]), 'old_column');
  await column.fill('parent_b'); await page.evaluate(() => { globalThis.saving = designer.saveChanges(); });
  await page.getByRole('dialog', { name: '保存表结构', exact: true }).waitFor(); await finishSave(page);
  assert.deepEqual(await page.evaluate(() => fixture.executions[0].design.foreign_keys[0]), { name: 'edited_fk', columns: ['id'], ref_schema: 'db', ref_table: 'parents', ref_columns: ['parent_b'], on_update: null, on_delete: null });
}));

test('namespace, table and column responses from an earlier baseline cannot overwrite reloaded metadata', { timeout: 30_000 }, async t => diagnosedPage(t, async page => {
  const state = await page.evaluate(async () => {
    const release = {};
    __api.listDatabases = () => new Promise(resolve => { release.namespaces = resolve; });
    __api.listTables = () => new Promise(resolve => { release.tables = resolve; });
    __api.getTableSchema = () => new Promise(resolve => { release.columns = resolve; });
    const fk = { name: 'pending', columns: ['id'], ref_schema: 'db', ref_table: 'parents', ref_columns: ['old'] };
    const pending = [designer.loadNamespaces(), designer.loadReferenceTables('db'), designer.loadReference(fk)];
    await designer.reloadAfterLeave();
    __api.listDatabases = async () => [['db', 'database']];
    __api.listTables = async () => [['parents', 'table']];
    __api.getTableSchema = async () => ({ columns: [{ name: 'fresh_column' }] });
    await Promise.all([designer.loadNamespaces(), designer.loadReferenceTables('db'), designer.loadReference(fk)]);
    release.namespaces([['stale_schema', 'database']]); release.tables([['stale_table', 'table']]); release.columns({ columns: [{ name: 'stale_column' }] });
    await Promise.all(pending);
    return { namespaces: designer.namespaces.values, tables: designer.referenceTables.get('db').values, columns: designer.references.get(JSON.stringify(['db', 'parents'])).columns };
  });
  assert.deepEqual(state, { namespaces: ['db'], tables: ['parents'], columns: ['fresh_column'] });
  await tab(page, '外键'); await page.evaluate(() => designer.addForeignKey());
  const namespace = page.getByRole('combobox', { name: '外键 1 引用库', exact: true });
  assert.equal(await namespace.locator('option[value="stale_schema"]').count(), 0);
  assert.equal(await page.getByRole('combobox', { name: '外键 1 引用表', exact: true }).locator('option[value="stale_table"]').count(), 0);
  assert.equal(await page.evaluate(() => fixture.executions.length), 0);
}));
