import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

// Execute the ACTUAL fixture script's final output statement without provisioning services.
// Windows Python normally writes GBK to pipes while pwsh decodes UTF-8. ASCII JSON is
// a transport encoding only: all original Unicode, quotes, binary escapes and nulls remain.
const fixture = fileURLToPath(new URL('../acceptance/datasources/round1-http-fixtures.py', import.meta.url));
const probe = String.raw`
import ast,json,os,pathlib,sys
source=pathlib.Path(sys.argv[1]).read_text(encoding='utf-8')
module=ast.parse(source)
output=module.body[-1]
assert isinstance(output,ast.Expr) and isinstance(output.value,ast.Call)
assert isinstance(output.value.func,ast.Name) and output.value.func.id=='print'
fixtures=json.loads(os.environ['NAVKIT_FIXTURE_TRANSPORT_SAMPLE'])
exec(compile(ast.Module(body=[output],type_ignores=[]),sys.argv[1],'exec'),{'json':json,'fixtures':fixtures})
`;

test('real HTTP fixture JSON output preserves full Unicode through a GBK-to-UTF8 Windows pipe', () => {
  const sentinel = "中文;O'Reilly\\尾\u0000😀";
  const expected = [{ expected: sentinel, nested: { value: sentinel, nullable: null, labels: { sentinel } } }];
  const result = spawnSync('python', ['-c', probe, fixture], {
    env: { ...process.env, PYTHONIOENCODING: 'gbk', PYTHONUTF8: '0', NAVKIT_FIXTURE_TRANSPORT_SAMPLE: JSON.stringify(expected) },
    timeout: 10000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr?.toString('utf8'));
  assert.ok(result.stdout.every(byte => byte < 128), 'JSON transport must not contain undecodable GBK Unicode bytes');
  assert.deepEqual(JSON.parse(result.stdout.toString('utf8')), expected, 'Transport escaping must restore every original character and null');
});
