import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../', import.meta.url));
// Narrow post-review maintenance: only files read and reviewed in this task.
for (const relative of [
  'docs/completed/PRODUCT-STATE.md',
  'docs/active/README.md',
  'docs/active/mysql-cancel-unknown-20261115.md',
  'artifacts/stream-preview-eval-20261115/HANDOFF.md',
]) {
  const file = path.join(root, relative);
  const previous = fs.readFileSync(file, 'utf8');
  let next = previous.replace(/\]\(([^\n()]+):(\d+)\)/g, ']($1#L$2)');
  if (relative.endsWith('/HANDOFF.md') && !next.includes('主代理独立复核')) {
    next += '\n## 主代理独立复核\n\n最终交付以[REPORT.md](REPORT.md#L75)与[lead-review.json](lead-review.json#L1)为准：原始证据7/7核验通过；同一哈希二进制在主代理默认环境复跑47通过/11失败/4 ignored，多项缓存私有目录权限拒绝导致断言失败，未申请权限修复。此前CLI58通过/1失败仍保留为其环境记录，不覆盖主代理失败。前端仍缺tsc依赖。运行时和流式前端未改，暂不合并/发布。\n';
  }
  if (relative.endsWith('/mysql-cancel-unknown-20261115.md') && !next.includes('主代理默认环境')) {
    next += '\n## 补充验收边界\n\n主代理默认环境独立复跑出现11项失败，其中多项明确伴随缓存目录私有权限设置被Windows拒绝；三个范围合计47通过/11失败/4 ignored。这与上述真实MySQL取消Unknown不是同一已确认根因。被拒绝目录位于独立工作树测试产物下，未修改权限或重试读取被拒绝目录；审批不可用，权限机制未进一步诊断。见[独立复核](../../artifacts/stream-preview-eval-20261115/lead-review.json#L1)。修复/发布前需分别消除或确认这两类验收缺口，不降级缓存私有权限保护或Unknown策略。\n';
  }
  if (next !== previous) fs.writeFileSync(file, next);
}
console.log('Normalized reviewed local line links and retained independent failures.');
