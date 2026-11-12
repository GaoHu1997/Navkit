import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

// The former round1 HTTP provisioner was a disposable acceptance artifact.
// Keep the GBK pipe regression against the current acceptance report's ACTUAL
// console serialization, without executing its service/evidence provisioning.
const fixture = fileURLToPath(new URL('../acceptance/simulation/odbc/verify-report.py', import.meta.url));
const probe = String.raw`
import ast,json,os,pathlib,sys
source=pathlib.Path(sys.argv[1]).read_text(encoding='utf-8')
module=ast.parse(source)
main=next(node for node in module.body if isinstance(node,ast.FunctionDef) and node.name=='main')
outputs=[node for node in ast.walk(main) if isinstance(node,ast.Expr) and isinstance(node.value,ast.Call) and isinstance(node.value.func,ast.Name) and node.value.func.id=='print' and node.value.args and isinstance(node.value.args[0],ast.Call) and isinstance(node.value.args[0].func,ast.Attribute) and isinstance(node.value.args[0].func.value,ast.Name) and node.value.args[0].func.value.id=='json' and node.value.args[0].func.attr=='dumps']
assert len(outputs)==1, 'Execute the unique real report JSON output, not a copied serializer'
audit=json.loads(os.environ['NAVKIT_FIXTURE_TRANSPORT_SAMPLE'])
exec(compile(ast.Module(body=outputs,type_ignores=[]),sys.argv[1],'exec'),{'json':json,'audit':audit})
`;

test('actual acceptance report preserves full Unicode through a GBK-to-UTF8 Windows pipe', () => {
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
