import { spawnSync } from 'node:child_process';
const exe = 'src-tauri/target/debug/deps/navkit_lib-9db4d7a43149e559.exe';
const listing = spawnSync(exe, ['--list'], { encoding: 'utf8', timeout: 60000 });
if (listing.status !== 0) { console.error(listing.stdout, listing.stderr); throw new Error(`fresh executable failed to load: ${listing.status}`); }
for (const name of ['production_window_page_and_saved_endpoint_changes_never_consume_original_grant', 'unknown_after_real_commit_keeps_background_connection_and_guards_until_drain', 'duplicate_or_registered_execution_is_not_a_no_send_proof', 'invalid_payload_does_not_consume_execution_grant']) {
  if (!listing.stdout.includes(name)) throw new Error(`STALE_BINARY: missing ${name}`);
}
const filters = [
  'object_mutation::tests', 'object_commands::tests', 'controlled_execution::tests',
  'table_design_execution::tests', 'table_workflows::tests', 'query_commands::tests',
  'scheduler_desktop::', 'sync::sync_stream::tests', 'ssh_tunnel::profiles::http_profile_tests', 'http_tunnel::production_tests',
];
let failures = 0, total = 0, ignored = 0;
for (const filter of filters) {
  console.log(`\n=== FRESH RUST FILTER ${filter} ===`);
  const result = spawnSync(exe, [filter, '--test-threads=1', '--nocapture'], { encoding: 'utf8', timeout: 240000, maxBuffer: 16 * 1024 * 1024 });
  console.log(result.stdout); console.error(result.stderr);
  const matched = result.stdout?.match(/test result: .*?(\d+) passed; (\d+) failed; (\d+) ignored/);
  const passed = matched ? Number(matched[1]) : 0;
  const skipped = matched ? Number(matched[3]) : 0; ignored += skipped;
  total += passed;
  const conditional = filter === 'http_tunnel::production_tests' && skipped > 0;
  if (conditional) console.log('NOT_VERIFIED: HTTPS production acceptance needs its isolated Java/H2 fixture; ignored is not a pass.');
  if (result.status !== 0 || !matched || (passed === 0 && !conditional)) { console.error(`FILTER_FAILURE status=${result.status} error=${result.error?.message ?? ''} filter=${filter}`); failures++; }
}
console.log(`FINAL_RUST_FILTERS=${filters.length} PASSED=${total} IGNORED_NOT_VERIFIED=${ignored} FAILED_FILTERS=${failures}`);
process.exitCode = failures ? 1 : 0;
