// STORY-UPD-001：更新偏好、共享下载与工作区启动前自动安装。全部 IPC 隔离模拟。
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';

const built = await build({
  stdin: { contents: 'export * from "./src/core/updatePreferences.ts";', resolveDir: process.cwd(), loader: 'ts' },
  bundle: true, write: false, platform: 'node', format: 'cjs', logLevel: 'silent',
});
const key = 'navkit.updateprefs.v1';
const manifest = { version: '1.2.0', size: 20, notes: '', pub_date: '', sha256: 'a'.repeat(64), url: 'https://example.test/update.exe' };
const ready = { phase: 'ready', staged_file: 'D:/staged.exe' };
const approved = { autoUpdate: true, skippedVersion: '', pendingInstall: { version: '1.2.0', file: 'D:/staged.exe', sha256: 'a'.repeat(64) } };
function harness(value, mock = {}, storageFailure = false) {
  const storage = new Map(value === undefined ? [] : [[key, typeof value === 'string' ? value : JSON.stringify(value)]]);
  const calls = [];
  const sandbox = {
    module: { exports: {} }, exports: {}, console,
    localStorage: { getItem: key => storage.get(key) ?? null, setItem: (key, value) => { if (storageFailure) throw new Error('quota'); storage.set(key, value); } },
    window: { __PREVIEW_MOCK__: {
      update_status: () => { calls.push('status'); return ready; },
      update_download: () => { calls.push('download'); return 'D:/staged.exe'; },
      update_apply: () => { calls.push('apply'); }, ...mock,
    } },
  };
  vm.runInNewContext(built.outputFiles[0].text, sandbox);
  return { ...sandbox.module.exports, storage, calls };
}

test('自动更新默认关闭，损坏/错误类型设置不能产生授权', () => {
  for (const value of [undefined, 'invalid-json', 'null', '[]', { autoUpdate: 'true' }, { autoUpdate: 1 }]) {
    const env = harness(value);
    assert.equal(env.readUpdatePreferences().autoUpdate, false);
  }
});
test('勾选持久化，取消勾选撤回自动安装授权', () => {
  const env = harness();
  assert.equal(env.saveUpdatePreferences({ autoUpdate: true }), true);
  assert.equal(env.readUpdatePreferences().autoUpdate, true);
  assert.equal(JSON.parse(env.storage.get(key)).autoUpdate, true);
  env.saveUpdatePreferences({ autoUpdate: false });
  assert.equal(env.readUpdatePreferences().autoUpdate, false);
});
test('跳过精确版本（兼容 v 前缀），新版本不被跳过', () => {
  const env = harness(approved);
  env.skipUpdateVersion('v1.2.0');
  assert.equal(env.isUpdateSkipped('1.2.0'), true);
  assert.equal(env.isUpdateSkipped('1.2.1'), false);
  assert.equal(env.readUpdatePreferences().pendingInstall, null);
});
test('写入失败时跳过本会话有效，但不保留自动安装授权', () => {
  const env = harness(approved, {}, true);
  assert.equal(env.skipUpdateVersion('1.2.0'), false);
  assert.equal(env.isUpdateSkipped('1.2.0'), true);
  assert.equal(env.readUpdatePreferences().autoUpdate, false);
});
test('共享下载只请求一次，成功记录路径且不在运行期间安装', async () => {
  let resolve;
  const env = harness(undefined, { update_download: () => { env.calls.push('download'); return new Promise(done => { resolve = done; }); } });
  const a = env.downloadUpdate(manifest), b = env.downloadUpdate(manifest);
  assert.equal(a, b);
  assert.deepEqual(env.calls, ['download']);
  resolve('D:/staged.exe'); await a;
  assert.equal(env.readUpdatePreferences().pendingInstall.file, 'D:/staged.exe');
  assert.equal(env.calls.includes('apply'), false);
});
test('不同版本并发下载拒绝，失败不产生安装记录且允许重试', async () => {
  let reject;
  const env = harness(undefined, { update_download: () => new Promise((_, fail) => { reject = fail; }) });
  const first = env.downloadUpdate(manifest);
  await assert.rejects(env.downloadUpdate({ ...manifest, version: '2.0.0' }), /另一个版本/);
  reject(new Error('hash mismatch')); await assert.rejects(first, /hash mismatch/);
  assert.equal(env.readUpdatePreferences().pendingInstall, null);
  const next = env.downloadUpdate(manifest); reject(new Error('network')); await assert.rejects(next, /network/);
});
test('启动前仅安装此前成功下载的匹配包，消费记录避免循环安装', async () => {
  const env = harness(approved);
  assert.equal((await env.installPendingUpdateAtStartup()).restarting, true);
  assert.deepEqual(env.calls, ['status', 'apply']);
  assert.equal(env.readUpdatePreferences().pendingInstall, null);
  assert.equal((await env.installPendingUpdateAtStartup()).restarting, false);
  assert.deepEqual(env.calls, ['status', 'apply']);
});
for (const [name, preferences, status] of [
  ['未开启自动更新', { ...approved, autoUpdate: false }, ready],
  ['无成功下载记录', { ...approved, pendingInstall: null }, ready],
  ['跳过同版本', { ...approved, skippedVersion: '1.2.0' }, ready],
  ['包未就绪', approved, { phase: 'downloading', staged_file: null }],
  ['路径不匹配', approved, { phase: 'ready', staged_file: 'D:/unknown.exe' }],
]) {
  test(`启动安全门：${name}不安装`, async () => {
    const env = harness(preferences, { update_status: () => status });
    assert.equal((await env.installPendingUpdateAtStartup()).restarting, false);
    assert.equal(env.calls.includes('apply'), false);
  });
}
test('自动安装失败继续启动，不自动重复安装', async () => {
  const env = harness(approved, { update_apply: () => { throw new Error('permission denied'); } });
  const result = await env.installPendingUpdateAtStartup();
  assert.equal(result.restarting, false);
  assert.match(result.error, /自动安装失败/);
  assert.equal(env.readUpdatePreferences().pendingInstall, null);
});
test('无法消费安装记录时禁止安装', async () => {
  const env = harness(approved, {}, true);
  const result = await env.installPendingUpdateAtStartup();
  assert.equal(result.restarting, false);
  assert.match(result.error, /取消自动安装/);
  assert.equal(env.calls.includes('apply'), false);
});
test('下载及启动安装请求都绑定有效 SHA-256，旧无哈希记录不自动安装', async () => {
  let downloadArgs, applyArgs;
  const env = harness(approved, {
    update_download: args => { downloadArgs = args; return 'D:/staged.exe'; },
    update_apply: args => { applyArgs = args; },
  });
  await env.downloadUpdate(manifest);
  assert.equal(downloadArgs.expectedVersion, '1.2.0');
  assert.equal(downloadArgs.expectedSha256, 'a'.repeat(64));
  await env.installPendingUpdateAtStartup();
  assert.equal(applyArgs.expectedSha256, 'a'.repeat(64));
  const legacy = harness({ ...approved, pendingInstall: { version: '1.2.0', file: 'D:/staged.exe' } });
  assert.equal((await legacy.installPendingUpdateAtStartup()).restarting, false);
  assert.deepEqual(legacy.calls, []);
});
test('无效哈希清单不能下载，同版本不同哈希不共享下载', async () => {
  const env = harness();
  await assert.rejects(env.downloadUpdate({ ...manifest, sha256: '' }), /SHA-256/);
  assert.deepEqual(env.calls, []);
  let complete;
  const held = harness(undefined, { update_download: () => new Promise(resolve => { complete = resolve; }) });
  const a = held.downloadUpdate(manifest);
  await assert.rejects(held.downloadUpdate({ ...manifest, sha256: 'b'.repeat(64) }), /另一个版本或更新包/);
  complete('D:/staged.exe'); await a;
});

test('main 在 App 构造及工作区初始化之前执行启动安装检查', async () => {
  const source = await readFile(new URL('../../src/main.ts', import.meta.url), 'utf8');
  const guard = source.indexOf('await installPendingUpdateAtStartup()');
  assert.ok(guard > 0 && guard < source.indexOf('new App('));
  assert.match(source, /if \(update\.restarting\)\s*\{[\s\S]*?return;/);
});
