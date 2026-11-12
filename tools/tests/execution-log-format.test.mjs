import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import ts from 'typescript';

// 执行真实实现：与历史日志窗口共用 src/core/log-format.ts（纯函数、零依赖）。
const source = await readFile(new URL('../../src/core/log-format.ts', import.meta.url), 'utf8');
const exports = {};
vm.runInNewContext(ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS },
}).outputText, { exports, module: { exports }, Date, Number, String, RegExp });
const { formatLogTimestamp, formatLogTime, formatEpoch, sqlHistoryParts } = exports;

test('时间戳统一为本地 YYYY-MM-DD HH:mm:ss.SSS，ISO 与空值都稳定', () => {
  assert.equal(formatLogTimestamp('2026-11-08 12:41:09'), '2026-11-08 12:41:09.000');
  assert.equal(formatLogTimestamp(' 2026-11-08 12:41:09.7 '), '2026-11-08 12:41:09.700');
  const iso = formatLogTimestamp(new Date(2026, 10, 8, 12, 41, 9, 123).toISOString());
  assert.equal(iso, '2026-11-08 12:41:09.123');
  assert.equal(formatLogTimestamp(''), '');
  assert.equal(formatLogTimestamp('not-a-date'), 'not-a-date');
});

test('单条耗时统一三位小数，与样例 Time: 0.000s 一致', () => {
  assert.equal(formatLogTime(0), 'Time: 0.000s');
  assert.equal(formatLogTime(7), 'Time: 0.007s');
  assert.equal(formatLogTime(1234), 'Time: 1.234s');
  assert.equal(formatLogTime(0.4), 'Time: 0.000s');
});

test('epoch 毫秒转同款时间戳；0/负数/非数字返回空串', () => {
  const stamp = new Date(2026, 10, 11, 17, 41, 13, 566).getTime();
  assert.equal(formatEpoch(stamp), '2026-11-11 17:41:13.566');
  for (const value of [0, -5, null, undefined, Number.NaN, Infinity]) assert.equal(formatEpoch(value), '');
});

test('三段格式逐字节复刻样例：元信息 / SQL / Time / 可选 Error', () => {
  const ok = sqlHistoryParts({ ts: '2026-11-11 17:41:13.566', connection_name: 'localhost', database: 'navkit_accept', kind: 'mysql', sql: 'SELECT 1', ok: true, elapsed_ms: 7 });
  assert.deepEqual(JSON.parse(JSON.stringify(ok)), [
    { kind: 'meta', text: '[2026-11-11 17:41:13.566][localhost][navkit_accept][MYSQL]' },
    { kind: 'sql', text: 'SELECT 1' },
    { kind: 'time', text: 'Time: 0.007s' },
  ]);
  const failed = sqlHistoryParts({ ts: '2026-11-11 17:41:13.571', connection_name: 'localhost', database: '', kind: '', sql: 'ALTER TABLE t', ok: false, elapsed_ms: 0, message: 'Duplicate key name' });
  assert.equal(failed[0].text, '[2026-11-11 17:41:13.571][localhost][—][]');
  assert.deepEqual(JSON.parse(JSON.stringify(failed[3])), { kind: 'error', text: 'Error: Duplicate key name' });
  assert.equal(failed[2].text, 'Time: 0.000s');
});
