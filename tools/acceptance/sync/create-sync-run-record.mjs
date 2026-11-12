#!/usr/bin/env node
// Reconcile this dated dynamic run; these checks do not execute or count new sync cases.
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const evidence = path.join(root, 'tools/acceptance/sync');
const simulatedDir = path.join(evidence, 'simulated-final-clean-20261107');
const output = path.join(evidence, 'sync-run-record-final-20261107.json');
const digest = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const json = file => JSON.parse(fs.readFileSync(file, 'utf8'));
const require_ = (ok, message) => { if (!ok) throw new Error(message); };
const record = { schema_version: 1, evidence_type: 'RECONCILIATION_OF_ACTUAL_DYNAMIC_SYNC_RUN', timestamp_utc: new Date().toISOString(), status: 'RUNNING', checks: [], artifacts: [], limitations: ['This verifier aggregates prior actual executions; it is not an additional database scenario.', 'Protocol simulation does not certify the corresponding vendor databases.', 'Conditional ignored tests are not counted as passes.'] };
const artifact = file => {
  record.artifacts.push({ path: path.relative(root, file).replaceAll('\\', '/'), bytes: fs.statSync(file).size, sha256: digest(file) });
};
const check = (name, action) => {
  try { action(); record.checks.push({ name, status: 'PASS' }); }
  catch (error) { record.checks.push({ name, status: 'FAIL', error: error.message }); }
};
const executable = description => {
  require_(typeof description?.path === 'string' && /^[0-9a-f]{64}$/.test(description?.sha256 ?? ''), 'Actual executed binary must be identified');
  const file = path.resolve(description.path);
  require_(file.startsWith(path.join(root, 'src-tauri/target') + path.sep), 'Only owned target executables may be hashed');
  require_(digest(file) === description.sha256, 'Executed binary changed after the run');
};
let live, simulated;
check('real_database_9_directions_108_scenarios_and_cleanup', () => {
  const file = path.join(evidence, 'live-report.json'); live = json(file); artifact(file);
  require_(live.counts?.directions === 9 && live.counts?.homogeneous_directions === 3 && live.counts?.heterogeneous_directions === 6 && live.counts?.cases === 108 && live.counts?.pass === 108 && live.counts?.fail_or_not_run === 0, 'Incomplete or failed live matrix');
  require_(live.setup?.length === 6 && live.setup.every(row => row.status === 'LIVE_CONNECTED'), 'Every actual endpoint must have connected');
  require_(live.cleanup?.length === 7 && live.cleanup.every(row => row.success === true), 'Owned namespaces/files not fully cleaned');
  require_(live.rows?.length === 108 && live.rows.every(row => row.status === 'PASS'), 'A live case was skipped or failed');
  const directions = new Map();
  for (const row of live.rows) {
    const key = `${row.source}->${row.target}`;
    if (!directions.has(key)) directions.set(key, new Set());
    directions.get(key).add(row.case);
  }
  require_(directions.size === 9 && [...directions.values()].every(cases => cases.size === 12), 'Missing or duplicated direction cases');
  require_(live.source_unchanged_during_run === true, 'Live source changed in flight');
  executable(live.executed_binary);
  record.live = { counts: live.counts, setup: live.setup, executed_binary: live.executed_binary, all_cleanup_succeeded: true };
});
check('six_stateful_simulation_suites_42_scenarios_and_vendor_zero', () => {
  const file = path.join(simulatedDir, 'simulations-run.json'); simulated = json(file); artifact(file);
  require_(simulated.status === 'SIMULATED_SYNC_PASS' && simulated.counts?.suites === 6 && simulated.counts?.pass === 42 && simulated.counts?.fail === 0 && simulated.vendor_real_pass === 0 && simulated.vendor_server_status === 'NOT_TESTED', 'Incomplete simulations or false vendor certification');
  const expected = { 'tds-sqlserver': 8, 'clickhouse-http_sql_subset': 8, 'odbc-odbc': 7, 'odbc-oracle': 6, 'odbc-dameng': 6, 'odbc-db2': 7 };
  require_(simulated.executions?.length === 6, 'All six simulation suites must execute');
  for (const execution of simulated.executions) {
    require_(expected[execution.label] === execution.counts?.pass && execution.counts?.fail === 0 && execution.exit_code === 0 && execution.cleanup_success === true, `Failed/incomplete simulation: ${execution.label}`);
    const suiteFile = path.join(simulatedDir, `${execution.label}.json`); const suite = json(suiteFile); artifact(suiteFile);
    require_(suite.status === 'SIMULATED_SYNC_PASS' && suite.cases?.length === expected[execution.label] && suite.cases.every(row => row.status === 'SIMULATED_SYNC_PASS') && suite.cleanup?.success === true && suite.vendor_real_pass === 0 && suite.vendor_server_status === 'NOT_TESTED', `Invalid raw suite evidence: ${execution.label}`);
    executable(suite.executed_binary);
    artifact(path.join(simulatedDir, `${execution.label}.log`));
  }
  require_(simulated.fresh_project_build?.exit_code === 0 && simulated.fresh_project_build?.status === 'PASS', 'Required package artifact clean did not succeed');
  require_(simulated.executed_binary_unchanged_between_suites === true && simulated.source_unchanged_during_run === true && simulated.runtime_artifacts_available === true && simulated.runtime_artifacts_unchanged_during_run === true, 'Source, executed binary or DLL integrity failed');
  artifact(path.join(simulatedDir, 'fresh-project-build.log'));
  record.simulated = { counts: simulated.counts, all_cleanup_succeeded: true, executed_binary: simulated.executions[0].executed_binary, vendor_server_status: 'NOT_TESTED', vendor_real_pass: 0 };
});
check('current_source_matches_actual_live_and_simulated_runs', () => {
  require_(live && simulated, 'Both dynamic reports are required');
  const allowed = new Set(['src-tauri/src/sync.rs','src-tauri/src/transfer.rs','src-tauri/src/mssql.rs','src-tauri/src/odbc_db.rs','src-tauri/src/schema.rs','src-tauri/src/db.rs','src-tauri/tests/e2e_sync_live_matrix.rs','src-tauri/tests/e2e_sync_simulated.rs','src-tauri/tests/support/sync_simulated/mod.rs','src-tauri/tests/support/sync_simulated/tds_sync_server.py','src-tauri/tests/support/sync_simulated/clickhouse_sync_server.py','tools/acceptance/simulation/native/tds_server.py','tools/acceptance/sync/run-simulations.mjs']);
  require_(Object.keys(live.source_hashes ?? {}).length === 7, 'Old live test executable omitted a newly tracked source');
  const hashes = { ...simulated.source_hashes, ...live.source_hashes };
  for (const [relative, hash] of Object.entries(hashes)) {
    require_(allowed.has(relative), 'Unexpected source path');
    require_(digest(path.join(root, relative)) === hash, `Source changed after execution: ${relative}`);
    if (simulated.source_hashes[relative] && live.source_hashes[relative]) require_(simulated.source_hashes[relative] === live.source_hashes[relative], `Runs used different production source: ${relative}`);
  }
  for (const relative of ['src-tauri/tests/e2e_sync_diff_incremental.rs','src-tauri/tests/e2e_sync_regressions.rs','src-tauri/tests/transfer_sync_acceptance.rs']) hashes[relative] = digest(path.join(root, relative));
  record.source_hashes = hashes;
});
check('unchanged_existing_odbc_runtime_artifacts', () => {
  require_(simulated, 'Simulation report required');
  for (const [key, value] of Object.entries(simulated.runtime_artifacts)) {
    const filename = key === 'NAVKIT_SIM_ODBC_DRIVER' ? 'navkit_sim_odbc.dll' : 'sim_duck_backend.dll';
    const file = path.join(root, 'tools/acceptance/simulation/odbc/runtime', filename);
    require_(value.status === 'PRESENT' && digest(file) === value.sha256, 'ODBC artifact changed'); artifact(file);
  }
});
check('13_real_sqlite_precision_key_cancel_bulk_regressions', () => {
  const file = path.join(evidence, 'regressions-report.json'); const report = json(file); artifact(file);
  require_(report.counts?.total === 13 && report.counts?.pass === 13 && report.counts?.fail === 0 && report.rows?.length === 13 && report.rows.every(row => row.status === 'PASS' && row.cleanup_success === true), 'SQLite regression failed or cleanup incomplete');
  record.sqlite_regressions = report.counts;
});
check('rebuilt_rust_library_and_existing_sqlite_and_live_transfer_logs', () => {
  const coreFile = path.join(evidence, 'core-final-clean-20261107.log'); const liveFile = path.join(evidence, 'live-final-clean-20261107.log');
  const coreLog = fs.readFileSync(coreFile, 'utf8'); const liveLog = fs.readFileSync(liveFile, 'utf8'); artifact(coreFile); artifact(liveFile);
  for (const result of ['274 passed; 0 failed; 23 ignored','12 passed; 0 failed; 0 ignored','1 passed; 0 failed; 0 ignored','1 passed; 0 failed; 1 ignored']) require_(coreLog.includes(`test result: ok. ${result}`), `Missing actual Rust result: ${result}`);
  require_(coreLog.includes('binary_source_metadata_is_not_unknown_text') && coreLog.includes('numeric_description_retains_precision_scale_and_nullable_status'), 'New ODBC tests not in executed library binary');
  require_((liveLog.match(/test result: ok\. 1 passed; 0 failed; 0 ignored/g) ?? []).length === 2, 'Live matrix or transfer test did not execute');
  require_(!/^error:|test result: FAILED/m.test(coreLog + liveLog), 'Failure present in Rust logs');
  const binaries = [...(coreLog + liveLog).matchAll(/Running[^\n]*\((src-tauri[\\/]target[^\n)]*\.exe)\)/g)].map(match => path.resolve(root, match[1]));
  for (const file of new Set(binaries)) { require_(file.startsWith(path.join(root, 'src-tauri/target') + path.sep), 'Unexpected executable path'); artifact(file); }
  require_(new Set(binaries).size >= 5, 'Expected library and integration executables not identified');
  record.rust_library = { passed: 274, failed: 0, ignored: 23 };
  record.existing_sqlite_diff_incremental = { passed: 12, failed: 0 };
  record.transfer = { sqlite_passed: 1, live_mysql_pg_passed: 1, failed: 0 };
});
check('current_node_auxiliary_regression_167_pass', () => {
  const file = path.join(evidence, 'ui-final-20261107.log'); const log = fs.readFileSync(file, 'utf8'); artifact(file);
  require_(/^(?:#|ℹ)\s+pass 167\s*$/m.test(log) && /^(?:#|ℹ)\s+fail 0\s*$/m.test(log) && /^(?:#|ℹ)\s+skipped 0\s*$/m.test(log), 'Node regression incomplete/failed');
  record.node_auxiliary = { passed: 167, failed: 0, skipped: 0, gui_click_end_to_end: false };
});
artifact(fileURLToPath(import.meta.url));
record.status = record.checks.every(item => item.status === 'PASS') ? 'PASS' : 'FAIL';
fs.writeFileSync(output, JSON.stringify(record, null, 2) + String.fromCharCode(10), { flag: 'wx' });
console.log(`${record.status}: ${record.checks.filter(item => item.status === 'PASS').length}/${record.checks.length} evidence checks; ${output}`);
for (const item of record.checks.filter(item => item.status === 'FAIL')) console.error(`${item.name}: ${item.error}`);
process.exitCode = record.status === 'PASS' ? 0 : 1;
