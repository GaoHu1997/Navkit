import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
const executable = process.env.NAVKIT_PARALLEL_TEST_EXE;
assert(executable, 'NAVKIT_PARALLEL_TEST_EXE must name this generation of the isolated Rust test executable');
const inventory = spawnSync(executable, ['--list'], { encoding: 'utf8', timeout: 60000, windowsHide: true });
assert.equal(inventory.error, undefined); assert.equal(inventory.status, 0, inventory.stderr);
const tests = inventory.stdout.split(/\r?\n/).filter(line => line.endsWith(': test')).map(line => line.slice(0, -6));
const suites = [
  ['query_lineage::tests::', 10], ['query_result_store::tests::', 6], ['query_result_mutation::tests::', 5], ['query_commands::tests::', 8],
  ['transfer::transfer_output::tests::', process.platform === 'win32' ? 8 : 9],
  ['transfer::transfer_sql_reader::tests::', 13], ['transfer::transfer_streaming_tests::', 10], ['transfer::transfer_spool_tests::', 8],
];
const receipts = [];
for (const [prefix, expected] of suites) {
  const actual = tests.filter(name => name.startsWith(prefix));
  console.log(JSON.stringify({ prefix, expected, inventory: actual.length, names: actual }));
  if (actual.length !== expected) { receipts.push({ prefix, ok: false, reason: 'wrong source generation or test prefix' }); continue; }
  const excluded = tests.filter(name => name.includes(prefix) && !name.startsWith(prefix));
  const result = spawnSync(executable, [prefix, '--test-threads=1', ...excluded.flatMap(name => ['--skip', name])], { encoding: 'utf8', timeout: 240000, windowsHide: true });
  process.stdout.write(result.stdout ?? ''); process.stderr.write(result.stderr ?? '');
  const match = /test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;/.exec(result.stdout ?? '');
  const receipt = { prefix, exit: result.status, error: result.error?.message ?? null,
    passed: match ? Number(match[2]) : null, failed: match ? Number(match[3]) : null, ignored: match ? Number(match[4]) : null };
  receipt.ok = !result.error && result.status === 0 && receipt.passed === expected && receipt.failed === 0 && receipt.ignored === 0;
  receipts.push(receipt);
}
console.log(JSON.stringify({ suite: 'query-transfer-real-20261110', inventory: tests.length, receipts }));
assert(receipts.every(receipt => receipt.ok), 'Every fresh real suite must complete with the expected tests and zero ignored cases');
