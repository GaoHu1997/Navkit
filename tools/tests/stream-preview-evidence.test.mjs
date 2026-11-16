import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../', import.meta.url));
const dir = path.join(root, 'artifacts/stream-preview-eval-20261115');
const records = fs.readFileSync(path.join(dir, 'runs-latest.jsonl'), 'utf8').trim().split(/\r?\n/).map(JSON.parse);
const manifest = JSON.parse(fs.readFileSync(path.join(dir, 'manifest-latest.json'), 'utf8'));
const summary = JSON.parse(fs.readFileSync(path.join(dir, 'summary.json'), 'utf8')).summary;
const median = xs => { const a = xs.slice().sort((a,b)=>a-b); return a.length % 2 ? a[(a.length-1)/2] : (a[a.length/2-1]+a[a.length/2])/2; };
const group = (n, mode) => records.filter(r => r.range_upper_bound === n && !r.limit_200_control && r.mode === mode && r.round > 0);

test('all 54 original records are real successful release runs with one SQL log and full output count', () => {
  assert.equal(records.length, 54);
  for (const r of records) {
    assert.equal(r.state, 'Succeeded'); assert.equal(r.build_mode, 'release');
    assert.equal(r.source_rows, 100000000); assert.equal(r.sql_log_count, 1);
    assert.equal(r.total_drained_rows, r.output_rows); assert.equal(r.resources_pending, false);
    assert.equal(r.stop_reason, null);
    assert.equal(r.sql, `SELECT id,payload FROM bench_rows WHERE id<=${r.range_upper_bound} ORDER BY id${r.limit_200_control ? ' LIMIT 200' : ''}`);
    assert.equal(r.output_rows, r.limit_200_control ? 200 : r.range_upper_bound);
  }
});
test('preview is 200 real rows while execution remains running, not a one-second pending response', () => {
  for (const r of records.filter(r=>r.mode==='stream')) {
    assert.equal(r.first_200_source, 'running_preview'); assert.equal(r.preview_state, 'Running');
    assert.equal(r.preview_first_id, 1); assert.equal(r.preview_last_id, 200);
    assert.equal(r.first_200_available_ms, r.preview_200_ms);
    assert.ok(r.preview_200_ms > 0 && r.preview_200_ms <= r.final_ms);
  }
});
test('five scales have paired expected rounds and independently calculated medians', () => {
  for (const n of [10000,100000,1000000,10000000,100000000]) {
    const old=group(n,'old'), stream=group(n,'stream'); const rounds=n<=1000000?3:1;
    assert.equal(old.length, rounds); assert.equal(stream.length, rounds);
    const expected=summary.find(s=>s.rows===n); assert.ok(expected);
    assert.equal(median(old.map(r=>r.final_ms)), expected.oldFirstAndFinalMs);
    assert.equal(median(stream.map(r=>r.first_200_available_ms)), expected.streamFirst200Ms);
    assert.equal(median(stream.map(r=>r.final_ms)), expected.streamFinalMs);
    assert.equal(expected.firstPageSavedMs, expected.oldFirstAndFinalMs-expected.streamFirst200Ms);
  }
});
test('cache is bounded and paired old/stream results have identical cached data boundaries', () => {
  for (const r of records) {
    assert.ok(r.cached_bytes <= 536870912); assert.equal(r.cached_first_id, 1);
    assert.equal(r.cached_last_id, r.cached_rows);
    const peer=records.find(p=>p.range_upper_bound===r.range_upper_bound && p.limit_200_control===r.limit_200_control && p.round===r.round && p.mode!==r.mode);
    assert.ok(peer); assert.equal(r.cached_rows,peer.cached_rows); assert.equal(r.cached_bytes,peer.cached_bytes);
    assert.equal(r.complete_export_allowed,!r.cache_truncated);
  }
});
test('100-million outputs are drained but truncated cache cannot be called complete', () => {
  for (const r of group(100000000,'old').concat(group(100000000,'stream'))) {
    assert.equal(r.total_drained_rows,100000000); assert.equal(r.cached_rows,17124437);
    assert.equal(r.cache_truncated,true); assert.equal(r.complete_export_allowed,false);
  }
});
test('LIMIT 200 controls are distinguished from output-N and excluded warmups', () => {
  for (const r of records.filter(r=>r.limit_200_control)) {
    assert.equal(r.output_rows,200); assert.equal(r.cached_rows,200); assert.equal(r.cache_truncated,false);
  }
  for (const n of [10000,100000,1000000,10000000,100000000]) assert.equal(records.filter(r=>r.range_upper_bound===n && r.round===0).length,2);
});
test('binary hash, source seed, real MySQL version, cache policy and owned-database removal match evidence', () => {
  const hash=crypto.createHash('sha256').update(fs.readFileSync(manifest.binary)).digest('hex');
  assert.equal(hash,manifest.binarySha256); assert.equal(manifest.mysqlVersion,'8.0.44');
  assert.equal(manifest.fixedTableRows,100000000); assert.deepEqual(manifest.seed[0].aggregate,[100000000,1,100000000,16,16]);
  assert.deepEqual(manifest.cachePolicy,{singleSetBytes:536870912,globalBytes:2147483648});
  assert.equal(manifest.state,'completed'); assert.ok(manifest.cleanup.every(c=>c.state==='removed' && c.verifiedAbsent));
  assert.ok(new Date(manifest.buildCompleted) < new Date(manifest.queriesStartedAfterSeedAndBuild));
});
