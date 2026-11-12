import { spawnSync } from 'node:child_process';
const exe='src-tauri/target/debug/deps/navkit_lib-9db4d7a43149e559.exe';
const listing=spawnSync(exe,['--list'],{encoding:'utf8',timeout:60000});
if(listing.status!==0) throw Error(`fresh executable cannot load: ${listing.status}\n${listing.stderr}`);
for(const name of ['native_reverse_observation_does_not_authorize_online_engineering','disappeared_ipc_waiter_keeps_ordinary_and_activity_until_real_read_cleanup','actual_saved_sqlite_reverse_service_preserves_order_and_rejects_bad_owner_scope','builder_has_stable_ids_ordered_primary_key_and_valid_model_snapshot']) if(!listing.stdout.includes(name)) throw Error(`STALE_BINARY missing ${name}`);
const filters=['er_native_snapshot::tests','er_database::owned::tests','er_database::tests','er_commands::tests','er_execution::tests','controlled_execution::tests'];
let passed=0,ignored=0,failed=0;
for(const filter of filters){
 console.log(`\n=== ${filter} ===`);
 const r=spawnSync(exe,[filter,'--test-threads=1','--nocapture'],{encoding:'utf8',timeout:120000,maxBuffer:16*1024*1024});console.log(r.stdout);console.error(r.stderr);
 const m=r.stdout?.match(/test result: .*?(\d+) passed; (\d+) failed; (\d+) ignored/);
 if(r.status!==0||!m||Number(m[1])===0) failed++;
 if(m){passed+=Number(m[1]);ignored+=Number(m[3]);}
}
console.log(`ER_SCOPED_PASSED=${passed} FAILED_FILTERS=${failed} IGNORED_VENDOR_TESTS_NOT_VERIFIED=${ignored}`);process.exitCode=failed?1:0;
