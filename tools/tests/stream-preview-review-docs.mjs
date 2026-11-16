// 历史审查的只读入口；不再将旧结果或旧链接写回当前 docs。
import fs from 'node:fs';
import path from 'node:path';

const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== '--historical-evidence') {
  console.error('只读历史工具：需要 --historical-evidence <证据目录>；禁止默认写回当前文档。');
  process.exitCode = 2;
} else {
  const dir = path.resolve(args[1]);
  const review = JSON.parse(fs.readFileSync(path.join(dir, 'lead-review.json'), 'utf8'));
  const report = fs.readFileSync(path.join(dir, 'REPORT.md'), 'utf8');
  console.log(JSON.stringify({ mode: 'historical_read_only', directory: dir, review,
    reportPresent: report.length > 0, notice: '历史快照仅供参考，不代表当前 main 的验收结果；未修改文档。' }, null, 2));
}
