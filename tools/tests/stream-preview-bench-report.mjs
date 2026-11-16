// 从逐轮原始证据生成五档对照；缺档明确未实测，不外推。
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../',import.meta.url));
const dir = path.join(root,'artifacts/stream-preview-eval-20261115');
const manifestPath = process.argv[2] || fs.readdirSync(dir).filter(f=>/^manifest-.*\.json$/.test(f)).sort().map(f=>path.join(dir,f)).at(-1);
if (!manifestPath) throw new Error('没有实测 manifest');
const manifest = JSON.parse(fs.readFileSync(manifestPath,'utf8'));
const tag = path.basename(manifestPath).match(/^manifest-(.*)\.json$/)[1];
const file = path.join(dir,`runs-${tag}.jsonl`);
const records = fs.existsSync(file) ? fs.readFileSync(file,'utf8').split('\n').filter(Boolean).map(JSON.parse) : [];
const median = values => {
  const sorted = values.filter(v=>typeof v==='number' && Number.isFinite(v)).sort((a,b)=>a-b);
  if (!sorted.length) return null;
  return sorted.length%2 ? sorted[Math.floor(sorted.length/2)] : (sorted[sorted.length/2-1]+sorted[sorted.length/2])/2;
};
const fmt = n => n === null || n === undefined ? '未实测' : n.toFixed(2);
const sizes = [10_000,100_000,1_000_000,10_000_000,100_000_000];
const grouped = (n,control,mode) => records.filter(r=>(r.range_upper_bound??r.source_rows)===n && r.limit_200_control===control && r.mode===mode && r.round>0 && r.state==='Succeeded');
const rows = [], controls = [], summary=[], variation=[];
for (const n of sizes) {
  const old = grouped(n,false,'old'), stream = grouped(n,false,'stream');
  const oldMs = median(old.map(r=>r.final_ms)), first = median(stream.map(r=>r.first_200_available_ms));
  const streamMs = median(stream.map(r=>r.final_ms));
  const cache = old.concat(stream);
  const integrity = !cache.length ? '未实测' : cache.some(r=>r.cache_truncated)
    ? `截断；${[...new Set(cache.map(r=>r.cached_rows))].join('/')} 行；完整导出拒绝`
    : `完整 ${n} 行`;
  const reason = old.length && stream.length ? '' : manifest.error || '本档未运行';
  rows.push(`| ${n.toLocaleString('en-US')} | ${fmt(oldMs)} | ${fmt(first)} | ${fmt(oldMs===null||first===null?null:oldMs-first)} | ${fmt(oldMs)} | ${fmt(streamMs)} | ${integrity} | ${old.length}/${stream.length}${reason?'；'+reason:''} |`);
  const o = grouped(n,true,'old'), s = grouped(n,true,'stream');
  const tableRows=[...new Set(o.concat(s).map(r=>r.source_rows))];
  controls.push(`| ${n.toLocaleString('en-US')} | ${tableRows.length?tableRows.map(v=>v.toLocaleString('en-US')).join('/'):'未实测'} | ${fmt(median(o.map(r=>r.final_ms)))} | ${fmt(median(s.map(r=>r.first_200_available_ms)))} | ${fmt(median(s.map(r=>r.final_ms)))} | ${o.length}/${s.length} |`);
  const pairedFull = stream.map(s=>{const o=old.find(o=>o.round===s.round);return o?s.final_ms-o.final_ms:null;});
  const range = values => values.length ? `${Math.min(...values).toFixed(2)}–${Math.max(...values).toFixed(2)}` : '未实测';
  variation.push(`| ${n.toLocaleString('en-US')} | ${range(old.map(r=>r.final_ms))} | ${range(stream.map(r=>r.final_ms))} | ${fmt(median(pairedFull))} |`);
  summary.push({rows:n, oldFirstAndFinalMs:oldMs, streamFirst200Ms:first, streamFinalMs:streamMs,
    firstPageSavedMs:oldMs===null||first===null?null:oldMs-first, pairedFullStreamMinusOldMs:median(pairedFull),
    oldRuns:old.length, streamRuns:stream.length, integrity});
}
const peak = (key) => records.reduce((p,r)=>Math.max(p,Number(r[key]??0)),0);
const largest=summary.find(r=>r.rows===100_000_000);
const limitOld=median(grouped(100_000_000,true,'old').map(r=>r.final_ms));
const limitStream=median(grouped(100_000_000,true,'stream').map(r=>r.first_200_available_ms));
const decision=largest?.firstPageSavedMs!==null
  ? `判断：SQL编辑器真实输出百万及以上结果时值得提前展示；亿行输出本次首屏提前${(largest.firstPageSavedMs/1000).toFixed(2)}秒。普通分页不因实体表大而需要流式：同一亿行表LIMIT200旧回执${fmt(limitOld)}ms、流式200行${fmt(limitStream)}ms。流式不保证全量更快，本次亿档全量增加${(largest.pairedFullStreamMinusOldMs/1000).toFixed(2)}秒。正式前端尚未实施，厂商取消回归缺口需另行诊断。`
  : '判断尚不完整：缺少亿行输出对照，不外推未实测的数字。';
const md = `# NavKit 旧回执与流式首屏：真实 MySQL 服务实测

基线 ${manifest.baseline}；${records[0]?.build_mode??'未测'} 测试二进制 SHA-256 ${manifest.binarySha256}。所有代码、二进制与产物在独立 worktree；留档补丁未应用，没有替换用户运行的程序。

${decision}

本表为 Rust 命令服务层，不含 IPC、WebView 或 DOM 绘制。旧服务直到最终回执才可获得首屏；流式首屏必须实际取得200行，1秒无行 pending 不算。快速终结时从同次 spool 读取200行并单列来源，原始记录保留 preview_200_ms 与 first_200_source。

## 真实输出 N 行（毫秒）

| SQL 输出行数 | 旧可获得首屏 | 流式实际200行 | 净省时 | 旧完整读取回执 | 流式完整读取终态 | 缓存完整性 | 实测轮数旧/流式 |
|---:|---:|---:|---:|---:|---:|---|---|
${rows.join('\n')}

前三档各三轮交替 AB/BA，后两档各一轮；数值为各组中位数，净省时为两个中位数之差。round=0 是 LIMIT200 预热与正确性检查，不纳入统计。大档单轮没有置信区间，不外推到其他机器、行宽、SQL 或数据库。

## 波动与全量完成开销（毫秒）

| 输出行数 | 旧全量最小–最大 | 流式全量最小–最大 | 同轮流式减旧耗时差的中位数 |
|---:|---:|---:|---:|
${variation.join('\n')}

配对差为正代表流式全量更慢。分别取两组中位数再相减不等于配对差；百万档存在明显轮间变化，不能仅按两组中位数宣称流式吞吐提升。

## 同一实体表 LIMIT 200 对照（毫秒）

| SQL范围 id<=N | 实体表实际行数 | 旧最终200行 | 流式实际200行 | 流式最终回执 | 实测轮数旧/流式 |
|---:|---:|---:|---:|---:|---|
${controls.join('\n')}

亿行表取200行和SQL真实输出亿行是不同工作量。${manifest.fixedTableRows?`最终轮所有档共用同一张 ${manifest.fixedTableRows.toLocaleString('en-US')} 行固定实体表`:'初轮使用同一张按档增长的实体表'}，输出 SQL 为 SELECT id,payload FROM bench_rows WHERE id<=N ORDER BY id，对照加 LIMIT 200；不使用 CTE 生成计时。原始记录 source_rows 是实体表行数，range_upper_bound 是SQL的N，output_rows 是真实输出行数。

## 方法、资源与边界

${manifest.schema}；payload 固定16 ASCII字节。一次性 grant 在 submit 前颁发；旧 execute_sql_service 和流式 register_streaming_execution→launch→execute_registered_sql_service 都使用正式 AppState、物理连接登记、MySQL SQLx 产品驱动、相同命名空间池与产品物理连接生命周期及 query_spool 路径。正式MySQL服务每次执行后关闭固定物理连接，不宣称重复使用同一条热TCP连接。每轮对同次 SQL 验证日志一次、总数、缓存首末行、终态资源状态；成功流式终态 rows 为空，首页/末页取自 spool。每轮 drop_spool 并断言文件真实删除，防止512MiB/2GiB配额串扰。

缓存配额未修改。缓存截断仍会完整排空数据库响应，但缺失尾部不支持完整分页/导出；ensure_complete 必须拒绝，不能把总读取行数当成缓存完整性。完整缓存的末行 id=N；截断时末行是 cached_last_id，source COUNT/MIN/MAX 与 total_drained_rows 独立保留，不冒称缺失尾页可读。

客户端内存每100ms用 Windows K32GetProcessMemoryInfo 采样；本批最大采样RSS ${(peak('rss_peak_bytes')/1024**2).toFixed(2)} MiB、private ${(peak('private_peak_bytes')/1024**2).toFixed(2)} MiB。GetProcessTimes CPU仅含Rust测试进程；MySQL服务CPU与内存未测。终态轮询1ms存在系统调度误差，首屏等待使用通知而非1秒假首屏。每轮同时保存 final_ms 和 drained_ms，资源排空才继续下一轮。

单轮30分钟、客户端private4GiB、系统可用内存2GiB、D盘空闲40GiB止损。播种每批最多10万行、每档最多30分钟；播种及验证时间在manifest单列，不计入查询。共享MySQL不重启、不清空缓存、不改全局配置，无冷缓存声明；本机其他负载未被控制。

## 验证与已知失败

相关release回归 execution_tests 38项、execution_stream 13项、query_spool 6项通过。已有真实MySQL SELECT1 门控用例1项通过；真实MySQL定向取消用例两次失败，实际Unknown、期望TimedOut，不能算厂商取消验收通过。独立重跑后该UUID剩余MySQL会话为0且库已删除；这不替代回执时刻的资源状态断言。日志：[regression-live-mysql-1.log](regression-live-mysql-1.log:17)、[regression-live-mysql-cancel-recheck.log](regression-live-mysql-cancel-recheck.log:17)、[regression-live-mysql-cancel-recheck.json](regression-live-mysql-cancel-recheck.json:1)。根因待单独诊断，性能分支未改运行时。

release构建与cargo check --lib --offline通过。npm run build因隔离目录缺离线TypeScript/Rollup依赖未通过；文档全局检查有52条基线旧产物链接失效，新增入口另行核验，不称所有闸门全绿。

初轮先实测1万/10万及后续小档；亿档因为基准误把SpoolSet.total缓存计数当成完整读取计数而失败，失败全量时刻未写出，不计入此表。已修正为结果total=SQL输出N、SpoolSet.total=文件缓存行数。初轮失败已保留 [failed-spool-evidence.json](failed-spool-evidence.json:1)，512MiB失败缓存与临时夹具按精确所有权清理。最终轮在同一张完整固定表重新测五档，查询开始前等待新鲜release构建完成，构建负载不混入查询计时。

## 复现与证据

1. 在基线 ${manifest.baseline} 的独立 worktree 添加本分支测试模块；cargo test --manifest-path src-tauri/Cargo.toml --lib --release --offline --no-run。
2. node tools/tests/stream-preview-bench.mjs <当前worktree内新鲜release测试exe绝对路径>。凭据来自已授权本机fixture，仅loopback，密码只经环境与夹具密文使用；每批拥有新的UUID库，结束删除精确库名。
3. node tools/tests/stream-preview-bench-report.mjs <manifest路径> 重新生成此表。

逐轮数据：[runs-${tag}.jsonl](runs-${tag}.jsonl:1)、[runs-${tag}.json](runs-${tag}.json:1)、[runs-${tag}.csv](runs-${tag}.csv:1)；机器/版本/schema/SQL/造表时间/进程参数/库清理：[manifest-${tag}.json](manifest-${tag}.json:1)。各stage日志含非零1项性能test通过与逐轮断言。五档数据缺失时明确“未实测”，没有按比例推算。

实现入口：[stream_preview_bench_tests.rs](../../src-tauri/src/stream_preview_bench_tests.rs:1)、[stream-preview-bench.mjs](../../tools/tests/stream-preview-bench.mjs:1)。本报告只评估现有服务，未实施流式前端或其他UI。

跑批的Python仅负责创建独立夹具、启动Rust测试与保存证据，没有Python/MySQL数据库驱动性能值。Node原生子进程输出捕获失败的首轮manifest及诊断保留在同一artifact目录；有效查询记录仅来自后续真实Rust服务成功运行。
`;
const reviewPath = path.join(dir, 'lead-review.json');
const reviewNote = fs.existsSync(reviewPath) ? `\n\n## 主代理独立复核（保留不同执行环境结果）\n\n同一release二进制哈希已核对，独立证据校验7/7通过，54条原始记录、五档中位数、SQL一次、首屏200行、缓存边界与库清理记录一致。这是对原始证据的复核，不是重新跑五档MySQL性能。\n\n主代理默认环境复跑：execution_tests为30通过/9失败/3 ignored，execution_stream为11通过/2失败/1 ignored，query_spool为6/6；三个不重叠筛选合计47通过/11失败/4 ignored。多项失败伴随缓存目录私有权限设置返回Windows os error 5；原因未进一步诊断或修复，不将其直接判为流式新缺陷，也不隐藏失败。先前CLI环境58通过/1失败不替代本轮结果。\n\n性能脚本NAVKIT_SPOOL_ISOLATION=0，CLI实库回归脚本为1，只改变测试缓存目录选择，不关闭私有权限保护。主代理未更改被拒绝目录权限。真实MySQL取消Unknown缺口仍未修复；本轮没有实库环境变量，ignored不算通过。前端构建仍因隔离目录缺tsc依赖未通过，前端源未改。\n\n证据：[lead-review.json](lead-review.json#L1)、[执行复跑](lead-regression-execution.log#L2)、[流式复跑](lead-regression-stream.log#L1)、[缓存复跑](lead-regression-spool.log#L1)、[原始证据校验](lead-evidence-validation.log#L1)、[前端构建](lead-frontend-build.log#L1)。不满足合并/发布的充分验收条件。\n` : '';
const finalMd = (md + reviewNote).replace(/\]\(([^\n()]+):(\d+)\)/g, ']($1#L$2)');
fs.writeFileSync(path.join(dir,'REPORT.md'),finalMd);
fs.writeFileSync(path.join(dir,'summary.json'),JSON.stringify({manifest:manifestPath,summary},null,2)+'\n');
console.log(JSON.stringify(summary,null,2));
