#!/usr/bin/env node
// Runs real product sync code against explicit stateful simulations, never vendors.
// No user data or persisted connection settings are read. Child test failures,
// absent reports, incomplete cleanup and source changes all fail the aggregate.
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const arguments_ = process.argv.slice(2);
const requested = arguments_.find(arg => arg.startsWith('--protocols='))?.split('=')[1]?.split(',') ?? ['tds', 'clickhouse', 'odbc'];
const kinds = arguments_.find(arg => arg.startsWith('--odbc-kinds='))?.split('=')[1]?.split(',') ?? ['odbc', 'oracle', 'dameng', 'db2'];
if (arguments_.some(arg => !arg.startsWith('--protocols=') && !arg.startsWith('--odbc-kinds=')) || !requested.length || requested.some(p => !['tds', 'clickhouse', 'odbc'].includes(p)) || !kinds.length || kinds.some(k => !['odbc', 'oracle', 'dameng', 'db2'].includes(k)) || new Set(requested).size !== requested.length || new Set(kinds).size !== kinds.length) {
  console.error('Usage: node tools/acceptance/sync/run-simulations.mjs [--protocols=tds,clickhouse,odbc] [--odbc-kinds=odbc,oracle,dameng,db2]');
  process.exit(2);
}
const output = process.env.NAVKIT_SYNC_SIM_REPORT_DIR ?? path.join(root, 'tools', 'acceptance', 'sync', `simulated-${randomUUID()}`);
if (!path.isAbsolute(output)) throw new Error('NAVKIT_SYNC_SIM_REPORT_DIR must be absolute');
fs.mkdirSync(output, { recursive: true });
const tasks = [
  ...(requested.includes('tds') ? [{ label: 'tds-sqlserver', test: 'sync_simulated_tds', minimum: 8 }] : []),
  ...(requested.includes('clickhouse') ? [{ label: 'clickhouse-http_sql_subset', test: 'sync_simulated_clickhouse', minimum: 8 }] : []),
  ...(requested.includes('odbc') ? kinds.map(kind => ({ label: `odbc-${kind}`, test: 'sync_simulated_odbc', kind, minimum: ['oracle', 'dameng'].includes(kind) ? 6 : 7 })) : []),
];
const summaryPath = path.join(output, 'simulations-run.json');
const freshBuildLog = path.join(output, 'fresh-project-build.log');
for (const file of [summaryPath, freshBuildLog, ...tasks.flatMap(task => [path.join(output, `${task.label}.json`), path.join(output, `${task.label}.log`)])]) {
  if (fs.existsSync(file)) throw new Error(`Refusing to reuse existing evidence: ${file}. Choose a fresh report directory.`);
}
const env = {
  ...process.env,
  NAVKIT_PYTHON: process.env.NAVKIT_PYTHON ?? 'python',
  NAVKIT_SYNC_SIM_REPORT_DIR: output,
  NAVKIT_SIM_ODBC_DRIVER: process.env.NAVKIT_SIM_ODBC_DRIVER ?? path.join(root, 'tools', 'acceptance', 'simulation', 'odbc', 'runtime', 'navkit_sim_odbc.dll'),
  NAVKIT_SIM_ODBC_BACKEND: process.env.NAVKIT_SIM_ODBC_BACKEND ?? path.join(root, 'tools', 'acceptance', 'simulation', 'odbc', 'runtime', 'sim_duck_backend.dll'),
};
const tracked = [
  'src-tauri/src/sync.rs', 'src-tauri/src/transfer.rs', 'src-tauri/src/mssql.rs', 'src-tauri/src/odbc_db.rs', 'src-tauri/src/schema.rs', 'src-tauri/src/db.rs',
  'src-tauri/tests/e2e_sync_simulated.rs', 'src-tauri/tests/support/sync_simulated/mod.rs',
  'src-tauri/tests/support/sync_simulated/tds_sync_server.py', 'src-tauri/tests/support/sync_simulated/clickhouse_sync_server.py',
  'tools/acceptance/simulation/native/tds_server.py', 'tools/acceptance/sync/run-simulations.mjs',
];
function hashes() {
  return Object.fromEntries(tracked.map(relative => [relative, createHash('sha256').update(fs.readFileSync(path.join(root, relative))).digest('hex')]));
}
const artifactPaths = requested.includes('odbc') ? {
  NAVKIT_SIM_ODBC_DRIVER: env.NAVKIT_SIM_ODBC_DRIVER,
  NAVKIT_SIM_ODBC_BACKEND: env.NAVKIT_SIM_ODBC_BACKEND,
} : {};
function artifactHashes() {
  return Object.fromEntries(Object.entries(artifactPaths).map(([key, file]) => {
    try {
      const bytes = fs.readFileSync(file);
      return [key, { path: file, status: 'PRESENT', bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') }];
    } catch {
      return [key, { path: file, status: 'MISSING' }];
    }
  }));
}
const summary = {
  schema_version: 1, run_id: randomUUID(), timestamp_utc: new Date().toISOString(),
  evidence_type: 'ACTUAL_NAVKIT_SYNC_WITH_STATEFUL_PROTOCOL_SIMULATION', status: 'RUNNING',
  vendor_server_status: 'NOT_TESTED', vendor_real_pass: 0,
  source_hashes: hashes(), runtime_artifacts: artifactHashes(),
  runtime: { platform: process.platform, arch: process.arch, node: process.version },
  executions: [], counts: { suites: tasks.length, pass: 0, fail: 0, vendor_real_pass: 0 },
  limitations: ['Stateful simulations are not vendor database certification', 'Case counts are scenario bundles, not Rust test counts or individual SQL calls', 'Outer managed executor must cancel a native driver that never returns; incomplete cleanup is never a PASS'],
};
function persist() { fs.writeFileSync(summaryPath, JSON.stringify(summary, null, 2) + String.fromCharCode(10)); }
persist();
console.log(`Stateful simulation evidence: ${output}`);

async function execute(task) {
  const args = ['test', '--offline', '--manifest-path', 'src-tauri/Cargo.toml', '--test', 'e2e_sync_simulated', task.test, '--', '--ignored', '--test-threads=1', '--nocapture'];
  const log = fs.createWriteStream(path.join(output, `${task.label}.log`), { flags: 'wx' });
  const started = Date.now();
  console.log(`\n=== ${task.label}: ACTUAL SYNC / SIMULATED SERVER / VENDOR NOT TESTED ===`);
  const execution = { label: task.label, command: ['cargo', ...args], status: 'RUNNING', report: `${task.label}.json` };
  summary.executions.push(execution);
  persist();
  const result = await new Promise(resolve => {
    const child = spawn('cargo', args, { cwd: root, env: { ...env, ...(task.kind ? { NAVKIT_SYNC_SIM_ODBC_KIND: task.kind } : {}) }, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
    child.stdout.on('data', chunk => { process.stdout.write(chunk); log.write(chunk); });
    child.stderr.on('data', chunk => { process.stderr.write(chunk); log.write(chunk); });
    child.once('error', error => resolve({ code: null, error: error.message }));
    child.once('close', (code, signal) => resolve({ code, signal }));
  });
  await new Promise((resolve, reject) => { log.on('error', reject); log.end(resolve); });
  execution.exit_code = result.code;
  execution.elapsed_ms = Date.now() - started;
  try {
    const report = JSON.parse(fs.readFileSync(path.join(output, `${task.label}.json`), 'utf8'));
    const cases = Array.isArray(report.cases) ? report.cases : [];
    const passed = cases.filter(c => c.status === 'SIMULATED_SYNC_PASS').length;
    const failed = cases.length - passed;
    execution.counts = { pass: passed, fail: failed };
    execution.cleanup_success = report.cleanup?.success === true;
    execution.executed_binary = report.executed_binary;
    const binaryIdentified = typeof report.executed_binary?.path === 'string' && /^[0-9a-f]{64}$/.test(report.executed_binary?.sha256 ?? '');
    const valid = binaryIdentified && result.code === 0 && report.status === 'SIMULATED_SYNC_PASS' && report.vendor_server_status === 'NOT_TESTED' && report.vendor_real_pass === 0 && passed >= task.minimum && failed === 0 && execution.cleanup_success;
    execution.status = valid ? 'SIMULATED_SYNC_PASS' : 'SIMULATED_SYNC_FAIL';
    execution.errors = cases.filter(c => c.status !== 'SIMULATED_SYNC_PASS').map(c => ({ name: c.name, status: c.status, error: c.error }));
    if (!valid && execution.errors.length === 0) execution.errors.push({ error: result.error ?? 'Nonzero exit, insufficient executed cases, invalid report or incomplete cleanup' });
    summary.counts.pass += passed;
    summary.counts.fail += failed;
  } catch (error) {
    execution.status = 'SIMULATED_SYNC_FAIL';
    execution.errors = [{ error: result.error ?? `Missing or unreadable execution report: ${error.message}` }];
    summary.counts.fail += 1;
  }
  persist();
}
// Atomic source edits may preserve mtimes. Cargo must not certify a stale executable.
const cleanArgs = ['clean', '--offline', '--manifest-path', 'src-tauri/Cargo.toml', '-p', 'navkit'];
summary.fresh_project_build = { command: ['cargo', ...cleanArgs], status: 'RUNNING', log: 'fresh-project-build.log' };
persist();
const freshLog = fs.createWriteStream(freshBuildLog, { flags: 'wx' });
console.log('\n=== Clear only navkit package artifacts before compiling the real sync tests ===');
const freshResult = await new Promise(resolve => {
  const child = spawn('cargo', cleanArgs, { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
  child.stdout.on('data', chunk => { process.stdout.write(chunk); freshLog.write(chunk); });
  child.stderr.on('data', chunk => { process.stderr.write(chunk); freshLog.write(chunk); });
  child.once('error', error => resolve({ code: null, error: error.message }));
  child.once('close', (code, signal) => resolve({ code, signal }));
});
await new Promise((resolve, reject) => { freshLog.on('error', reject); freshLog.end(resolve); });
summary.fresh_project_build.exit_code = freshResult.code;
summary.fresh_project_build.status = freshResult.code === 0 ? 'PASS' : 'FAIL';
if (freshResult.error) summary.fresh_project_build.error = freshResult.error;
persist();
if (freshResult.code === 0) {
  for (const task of tasks) await execute(task);
} else {
  summary.counts.fail += 1;
}
summary.source_hashes_after = hashes();
summary.source_unchanged_during_run = JSON.stringify(summary.source_hashes) === JSON.stringify(summary.source_hashes_after);
summary.runtime_artifacts_after = artifactHashes();
summary.runtime_artifacts_unchanged_during_run = JSON.stringify(summary.runtime_artifacts) === JSON.stringify(summary.runtime_artifacts_after);
summary.runtime_artifacts_available = Object.values(summary.runtime_artifacts).every(artifact => artifact.status === 'PRESENT');
summary.executed_binary_unchanged_between_suites = new Set(summary.executions.map(execution => execution.executed_binary?.sha256)).size === 1;
summary.finished_utc = new Date().toISOString();
summary.status = summary.fresh_project_build.exit_code === 0 && summary.executions.length === tasks.length && summary.executions.every(e => e.status === 'SIMULATED_SYNC_PASS') && summary.source_unchanged_during_run && summary.runtime_artifacts_unchanged_during_run && summary.runtime_artifacts_available && summary.executed_binary_unchanged_between_suites ? 'SIMULATED_SYNC_PASS' : 'SIMULATED_SYNC_FAIL';
persist();
console.log(`\n${summary.status}: ${summary.counts.pass} scenario bundles passed, ${summary.counts.fail} failed; vendor_real_pass=0; ${summaryPath}`);
process.exitCode = summary.status === 'SIMULATED_SYNC_PASS' ? 0 : 1;
