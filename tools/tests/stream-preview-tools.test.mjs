import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const python = process.env.NAVKIT_PYTHON || 'python';
const tool = name => path.join(root, 'tools/tests', name);
function cleanEnvironment() {
  const env = { ...process.env };
  for (const key of Object.keys(env)) if (/^(NAVKIT_|MYSQL_PWD$|NODE_TEST_CONTEXT$)/i.test(key)) delete env[key];
  return env;
}
const configured = () => ({ ...cleanEnvironment(), NAVKIT_MYSQL_HOST: '127.0.0.1', NAVKIT_MYSQL_PORT: '3306',
  NAVKIT_MYSQL_USER: 'test-only', NAVKIT_MYSQL_PASSWORD: 'test-config-not-a-real-password',
  // --check-env resolves this executable but never runs it or connects to MySQL.
  NAVKIT_MYSQL_CLIENT: process.execPath });
function run(executable, args, env = cleanEnvironment()) {
  const result = spawnSync(executable, args, { cwd: root, env, encoding: 'utf8', timeout: 15000, windowsHide: true });
  assert.ifError(result.error);
  return { ...result, text: (result.stdout || '') + (result.stderr || '') };
}

for (const script of ['stream-preview-bench.py', 'pagination-live-mysql.py']) {
  test(`${script}: source syntax parses without executing or writing bytecode`, () => {
    const result = run(python, ['-c', 'import ast, pathlib, sys; ast.parse(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))', tool(script)]);
    assert.equal(result.status, 0, result.text);
  });
  test(`${script}: missing explicit credentials fails before DB and filesystem work`, () => {
    const result = run(python, [tool(script), '--check-env']);
    assert.equal(result.status, 2, result.text);
    assert.match(result.text, /NAVKIT_MYSQL_PASSWORD/);
    assert.doesNotMatch(result.text, /Traceback/);
  });
  test(`${script}: explicit configuration reports actual baseline without contacting DB`, () => {
    const result = run(python, [tool(script), '--check-env'], configured());
    assert.equal(result.status, 0, result.text);
    const output = JSON.parse(result.stdout);
    assert.equal(output.mode, 'configuration_only');
    assert.match(output.baseline, /^[0-9a-f]{40}$/);
    const git = run('git', ['rev-parse', 'HEAD']);
    assert.equal(git.status, 0, git.text); assert.equal(output.baseline, git.stdout.trim());
    assert.equal(output.credentialSource, 'explicit_environment');
    assert.doesNotMatch(result.text, /test-config-not-a-real-password/);
  });
  test(`${script}: non-loopback and invalid port fail without contacting DB`, () => {
    const remote = run(python, [tool(script), '--check-env'], { ...configured(), NAVKIT_MYSQL_HOST: '192.0.2.1' });
    assert.equal(remote.status, 2); assert.match(remote.text, /loopback/);
    const port = run(python, [tool(script), '--check-env'], { ...configured(), NAVKIT_MYSQL_PORT: '65536' });
    assert.equal(port.status, 2); assert.match(port.text, /65535/);
  });
}

test('evidence test skips only when no explicit evidence directory is supplied', () => {
  const result = run(process.execPath, ['--test', '--test-reporter=tap', tool('stream-preview-evidence.test.mjs')]);
  assert.equal(result.status, 0, result.text);
  assert.match(result.text, /# skipped 1|# skip 1/);
  assert.match(result.text, /# pass 0/);
});
test('explicit missing evidence is a failure, never converted into a skip', () => {
  const missing = path.join(os.tmpdir(), `navkit-missing-evidence-${process.pid}-${Date.now()}`);
  assert.equal(fs.existsSync(missing), false);
  const result = run(process.execPath, ['--test', '--test-reporter=tap', tool('stream-preview-evidence.test.mjs')],
    { ...cleanEnvironment(), NAVKIT_STREAM_EVIDENCE_DIR: missing });
  assert.notEqual(result.status, 0, result.text);
  assert.match(result.text, /ENOENT/);
});
test('historical review and report do not implicitly read or overwrite current docs', () => {
  const review = run(process.execPath, [tool('stream-preview-review-docs.mjs')]);
  assert.equal(review.status, 2); assert.match(review.text, /historical-evidence/);
  const report = run(process.execPath, [tool('stream-preview-bench-report.mjs')]);
  assert.equal(report.status, 2); assert.match(report.text, /manifest/);
});
test('Node benchmark wrapper forwards the no-DB configuration gate', () => {
  const result = run(process.execPath, [tool('stream-preview-bench.mjs'), '--check-env'],
    { ...cleanEnvironment(), NAVKIT_PYTHON: python });
  assert.equal(result.status, 2, result.text);
  assert.match(result.text, /NAVKIT_MYSQL_PASSWORD/);
});
