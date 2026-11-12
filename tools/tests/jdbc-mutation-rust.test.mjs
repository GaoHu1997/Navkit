import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// This runner deliberately never invokes Cargo. The caller supplies a finished,
// freshly compiled test executable; every writable fixture gets its own copied
// executable and portable storage below the OS temp directory, never user data.
const executable = process.env.NAVKIT_PL20_TEST_EXE;
const jar = process.env.NAVKIT_H2_JAR || path.resolve('tools/acceptance/datasources/h2/h2-2.3.232.jar');
const java = process.env.NAVKIT_JAVA;
const outputLimit = 1024 * 1024;

async function run(file, args, options = {}) {
  const child = spawn(file, args, { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], ...options });
  let stdout = '', stderr = '', expired = false, excessive = false;
  const deadline = setTimeout(() => { expired = true; child.kill(); }, 120_000);
  const collect = field => data => {
    if (field === 'stdout') stdout += data.toString('utf8'); else stderr += data.toString('utf8');
    if (Buffer.byteLength(stdout) + Buffer.byteLength(stderr) > outputLimit) { excessive = true; child.kill(); }
  };
  child.stdout.on('data', collect('stdout'));
  child.stderr.on('data', collect('stderr'));
  try {
    const result = await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('close', (code, signal) => resolve({ code, signal }));
    });
    assert.equal(expired, false, `real Rust fixture exceeded its deadline\n${stdout}\n${stderr}`);
    assert.equal(excessive, false, 'real Rust fixture exceeded bounded output');
    assert.equal(result.code, 0, `real Rust fixture exited ${result.code}/${result.signal}\n${stdout}\n${stderr}`);
    return { stdout, stderr };
  } finally { clearTimeout(deadline); }
}

const missing = !executable ? 'unverified: set NAVKIT_PL20_TEST_EXE to a finished fresh lib test executable'
  : !fs.existsSync(jar) ? 'unverified: official H2 2.3.232 JAR is unavailable'
  : !java ? 'unverified: set NAVKIT_JAVA to the real Java 11+ executable' : false;

test('real Rust/JVM/H2 typed plans plus unchanged native and command regression assertions', { skip: missing, timeout: 700_000 }, async t => {
  assert.equal(fs.lstatSync(executable).isFile(), true, 'regular finished test executable required');
  assert.equal(fs.lstatSync(jar).isFile(), true, 'official H2 JAR must be a regular file');
  assert.equal(fs.lstatSync(java).isFile(), true, 'Java must be a real regular executable');
  const absoluteExecutable = fs.realpathSync(executable), absoluteJar = fs.realpathSync(jar), absoluteJava = fs.realpathSync(java);
  const env = { ...process.env, NAVKIT_H2_JAR: absoluteJar, NAVKIT_JAVA: absoluteJava };
  const listing = await run(absoluteExecutable, ['--list'], { env });
  const names = listing.stdout.split(/\r?\n/).map(line => /^([A-Za-z0-9_:]+): test$/.exec(line)?.[1]).filter(Boolean);
  assert.ok(names.length > 0, 'libtest must actually list the finished executable');
  const cases = [
    ['http_db::relational::real_tests::real_native_h2_rust_bridge_owned_typed_row_plans', 'navkit-pl20-command-tests-'],
    ['http_db::relational::real_tests::real_generic_jdbc_rust_bridge_owned_typed_row_plans', 'navkit-pl20-command-tests-'],
    ['http_db::relational::real_tests::real_saved_explicit_jdbc_binding_routes_without_native_or_odbc_fallback', 'navkit-pl20-command-tests-'],
    ['mutation::jdbc::real_tests::real_native_h2_shared_c2_registry_and_backend_service', 'navkit-pl20-c2-tests-'],
    ['mutation::jdbc::real_tests::real_generic_jdbc_shared_c2_registry_and_backend_service', 'navkit-pl20-c2-tests-'],
    ['commands::h2_namespace_tests::h2_memory_queries_metadata_and_schemas_share_one_real_session', 'navkit-command-tests-h2-'],
    ['commands::jdbc_namespace_tests::generic_jdbc_session_namespaces_binary_paging_and_close_are_real', 'navkit-jdbc-command-tests-'],
    ['::real_h2_jdbc_schema_browse_transaction_auth', 'navkit-pl20-native-tests-'],
  ];
  for (const [requested, prefix] of cases) {
    const matches = names.filter(name => requested.startsWith('::') ? name.endsWith(requested) : name === requested);
    assert.equal(matches.length, 1, `exactly one existing Rust test must match ${requested}, never silently run zero tests`);
    const name = matches[0];
    await t.test(name, { timeout: 130_000 }, async () => {
      const root = fs.mkdtempSync(path.join(os.tmpdir(), prefix));
      const safeRoot = fs.realpathSync(root), temp = fs.realpathSync(os.tmpdir());
      assert.equal(path.dirname(safeRoot).toLowerCase(), temp.toLowerCase(), 'never stage or delete outside the OS temp directory');
      assert.ok(path.basename(safeRoot).startsWith(prefix), 'unique owned fixture root required');
      const copy = path.join(safeRoot, 'navkit-pl20-tests.exe');
      try {
        fs.copyFileSync(absoluteExecutable, copy, fs.constants.COPYFILE_EXCL);
        if (process.platform !== 'win32') fs.chmodSync(copy, 0o700);
        const result = await run(copy, ['--ignored', '--exact', name, '--nocapture', '--test-threads=1'], { env });
        assert.match(result.stdout, /running 1 test\r?\n/, 'a real test must actually execute');
        assert.match(result.stdout, /test result: ok\. 1 passed; 0 failed; 0 ignored;/, 'ignored registration is not a service PASS');
        assert.ok(result.stdout.includes(name), 'actual Rust test identity must be visible');
        t.diagnostic(result.stdout.trim());
        if (result.stderr.trim()) t.diagnostic(result.stderr.trim());
      } finally {
        // Only the freshly created, prefix-validated private fixture root is ours.
        // Test-side JVM ownership closes stdin/sidecars and H2 files before exit.
        fs.rmSync(safeRoot, { recursive: true, force: false });
      }
    });
  }
});
