import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
const executable = process.env.NAVKIT_PARALLEL_TEST_EXE;
assert(executable, 'NAVKIT_PARALLEL_TEST_EXE must name the just-compiled private test executable');
const inventory = spawnSync(executable, ['--list'], { encoding: 'utf8', timeout: 60000, windowsHide: true });
assert.equal(inventory.error, undefined); assert.equal(inventory.status, 0, inventory.stderr);
const tests = inventory.stdout.split(/\r?\n/).filter(line => line.endsWith(': test')).map(line => line.slice(0, -6));
const modules = [
  ['structure_sync::tests::', 18], ['design::tests::', 20], ['table_objects::tests::', 7],
  ['schema::relational_keys::tests::', 6], ['transfer_stream::tests::', 22], ['scheduler::migration::tests::', 15],
];
let total = 0;
for (const [prefix, expected] of modules) {
  const actual = tests.filter(name => name.startsWith(prefix));
  assert.equal(actual.length, expected, `${prefix} must contain this source generation, not an old executable`);
  const excluded = tests.filter(name => name.includes(prefix) && !name.startsWith(prefix));
  const args = [prefix, '--test-threads=1', ...excluded.flatMap(name => ['--skip', name])];
  const result = spawnSync(executable, args, { encoding: 'utf8', timeout: 240000, windowsHide: true });
  process.stdout.write(result.stdout ?? ''); process.stderr.write(result.stderr ?? '');
  assert.equal(result.error, undefined, `${prefix}: ${result.error}`);
  assert.equal(result.status, 0, `${prefix}: exit ${result.status}`);
  const receipt = /test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;/.exec(result.stdout);
  assert(receipt, `${prefix} must report a completed real run`);
  assert.equal(Number(receipt[1]), expected); assert.equal(Number(receipt[2]), 0); assert.equal(Number(receipt[3]), 0);
  total += expected;
}
console.log(JSON.stringify({ suite: 'parallel-rust-fresh-20261110', inventory: tests.length, passed: total, failed: 0, ignored: 0 }));
