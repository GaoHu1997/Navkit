// 明确选择逐轮证据生成五档对照；不写 docs、不推断产品接入/回归结论。
import fs from 'node:fs';
import path from 'node:path';

const selectedManifest = process.argv[2];
if (!selectedManifest) {
  console.error('需要显式 manifest 路径：node tools/tests/stream-preview-bench-report.mjs <manifest-标记.json>');
  process.exit(2);
}
const manifestPath = path.resolve(selectedManifest);
const dir = path.dirname(manifestPath);
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
const match = path.basename(manifestPath).match(/^manifest-(.*)\.json$/);
if (!match) throw new Error('manifest 文件名必须为 manifest-<标记>.json');
const tag = match[1];
const records = fs.readFileSync(path.join(dir, `runs-${tag}.jsonl`), 'utf8')
  .split(/\r?\n/).filter(Boolean).map(JSON.parse);
const median = values => {
  const sorted = values.filter(v => typeof v === 'number' && Number.isFinite(v)).sort((a, b) => a - b);
  if (!sorted.length) return null;
  return sorted.length % 2 ? sorted[Math.floor(sorted.length / 2)] : (sorted[sorted.length / 2 - 1] + sorted[sorted.length / 2]) / 2;
};
const fmt = n => n == null ? '未实测' : n.toFixed(2);
const sizes = [10000, 100000, 1000000, 10000000, 100000000];
const group = (n, control, mode) => records.filter(r => (r.range_upper_bound ?? r.source_rows) === n
  && r.limit_200_control === control && r.mode === mode && r.round > 0 && r.state === 'Succeeded');
const rows = [], controls = [], summary = [], variation = [];
for (const n of sizes) {
  const old = group(n, false, 'old'), stream = group(n, false, 'stream');
  const oldMs = median(old.map(r => r.final_ms));
  const first = median(stream.map(r => r.first_200_available_ms));
  const streamMs = median(stream.map(r => r.final_ms));
  const cache = old.concat(stream);
  const integrity = !cache.length ? '未实测' : cache.some(r => r.cache_truncated)
    ? `截断；${[...new Set(cache.map(r => r.cached_rows))].join('/')} 行；完整导出拒绝` : `完整 ${n} 行`;
  const paired = stream.map(s => { const o = old.find(o => o.round === s.round); return o ? s.final_ms - o.final_ms : null; });
  const range = values => values.length ? `${Math.min(...values).toFixed(2)}–${Math.max(...values).toFixed(2)}` : '未实测';
  const saved = oldMs == null || first == null ? null : oldMs - first;
  rows.push(`| ${n.toLocaleString('en-US')} | ${fmt(oldMs)} | ${fmt(first)} | ${fmt(saved)} | ${fmt(streamMs)} | ${integrity} | ${old.length}/${stream.length} |`);
  variation.push(`| ${n.toLocaleString('en-US')} | ${range(old.map(r => r.final_ms))} | ${range(stream.map(r => r.final_ms))} | ${fmt(median(paired))} |`);
  const o = group(n, true, 'old'), s = group(n, true, 'stream');
  const tableRows = [...new Set(o.concat(s).map(r => r.source_rows))];
  controls.push(`| ${n.toLocaleString('en-US')} | ${tableRows.length ? tableRows.join('/') : '未实测'} | ${fmt(median(o.map(r => r.final_ms)))} | ${fmt(median(s.map(r => r.first_200_available_ms)))} | ${fmt(median(s.map(r => r.final_ms)))} | ${o.length}/${s.length} |`);
  summary.push({ rows: n, oldFirstAndFinalMs: oldMs, streamFirst200Ms: first, streamFinalMs: streamMs,
    firstPageSavedMs: saved, pairedFullStreamMinusOldMs: median(paired), oldRuns: old.length, streamRuns: stream.length, integrity });
}
const peak = key => records.reduce((p, r) => Math.max(p, Number(r[key] ?? 0)), 0);
const reviewPath = path.join(dir, 'lead-review.json');
const review = fs.existsSync(reviewPath) ? JSON.parse(fs.readFileSync(reviewPath, 'utf8')) : null;
const report = `# NavKit 旧回执与流式首屏：所选 MySQL 服务证据

证据基线 ${manifest.baseline}；构建 ${records[0]?.build_mode ?? '未测'}；二进制 SHA-256 ${manifest.binarySha256}。

仅描述此 manifest 的实验，不声称当前 main 的UI、取消、构建或发布已验收，不覆盖当前服务器分页 + 已看页缓存的产品决策。

## SQL真实输出N行（毫秒）

| 输出行数 | 旧首屏/最终回执 | 流式200行 | 首屏净省时 | 流式最终回执 | 缓存完整性 | 成功轮数旧/流式 |
|---:|---:|---:|---:|---:|---|---|
${rows.join('\n')}

各组仅统计 round>0 的 Succeeded 记录，round=0预热不入中位数；缺档未实测。失败/未完成记录仍保留在原始JSONL，不因报告筛选变成成功。

## 全量波动与配对差（毫秒）

| 输出行数 | 旧最小–最大 | 流式最小–最大 | 同轮流式减旧中位数 |
|---:|---:|---:|---:|
${variation.join('\n')}

配对差为正表示流式全量更慢；两组中位数相减不等于配对差。单轮没有置信区间，不外推其它机器、行宽或SQL。

## LIMIT200对照（毫秒）

| SQL范围id<=N | 实体表行数 | 旧最终200行 | 流式200行 | 流式最终回执 | 成功轮数旧/流式 |
|---:|---:|---:|---:|---:|---|
${controls.join('\n')}

实体表很大与真实输出全量是不同工作量。首屏需实际取得200行，1秒无行pending不算。成功终态的metadata与首屏来自同次执行，不重新查询；本工具不宣称产品界面已接入。

## 边界与证据身份

${manifest.schema ?? 'schema未提供'}；MySQL版本 ${manifest.mysqlVersion ?? '未提供'}；实际实体表行数 ${manifest.fixedTableRows ?? '未提供'}。
缓存政策：\`${JSON.stringify(manifest.cachePolicy ?? null)}\`。截断缓存不能导出完整文件，即使数据库响应已经排空。
Rust进程采样峰值RSS ${(peak('rss_peak_bytes') / 1024 ** 2).toFixed(2)} MiB / private ${(peak('private_peak_bytes') / 1024 ** 2).toFixed(2)} MiB；厂商服务CPU和内存未测，不宣称冷缓存。
实验状态 ${manifest.state ?? '未提供'}；库清理记录：\`${JSON.stringify(manifest.cleanup ?? [])}\`。

[manifest](manifest-${tag}.json)、[逐轮JSONL](runs-${tag}.jsonl)、[逐轮JSON](runs-${tag}.json)、[CSV](runs-${tag}.csv)。详细机器与止损条件见manifest；不自动套用旧快照的回归计数和前端构建结论。

## 显式复现

先设置 NAVKIT_MYSQL_HOST / NAVKIT_MYSQL_PORT / NAVKIT_MYSQL_USER / NAVKIT_MYSQL_PASSWORD，以及可选 NAVKIT_MYSQL_CLIENT；仅loopback。
使用 node tools/tests/stream-preview-bench.mjs --check-env 检查配置，不连接数据库。
明确构建新鲜release测试二进制后调用 node tools/tests/stream-preview-bench.mjs <二进制路径>；此操作会造测试库和运行基准，非普通验证。
再以显式manifest调用本报告工具；报告只写所选证据目录，不改docs。
${review ? `\n## 同目录独立复核原记录（历史，不替代当前验收）\n\n[lead-review.json](lead-review.json)\n\n\`\`\`json\n${JSON.stringify(review, null, 2)}\n\`\`\`\n` : ''}
`;
fs.writeFileSync(path.join(dir, 'REPORT.md'), report);
fs.writeFileSync(path.join(dir, 'summary.json'), JSON.stringify({ manifest: manifestPath, summary }, null, 2) + '\n');
console.log(JSON.stringify(summary, null, 2));
