// Node 原生子进程输出在本机出现空捕获；Python仅管理进程/造表，性能计时仍在正式Rust服务。
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const script = fileURLToPath(new URL('./stream-preview-bench.py',import.meta.url));
const result = spawnSync(process.env.NAVKIT_PYTHON || 'python',[script,...process.argv.slice(2)],{stdio:'inherit',windowsHide:true});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
