/**
 * 全功能桌面验收运行器（IPC + GUI + 逐步截图）。
 *
 * 用法:  node tools/acceptance/e2e/manual-runner-20261112.mjs [--no-build] [--with-vite]
 *
 * 形态：debug exe（devUrl → Vite dev server :5183）+ 真实 Rust IPC。
 *  - Vite 已在 5183 运行时直接复用；否则 --with-vite 自动拉起。
 *  - 隔离运行时（临时目录），配置/备份全部写入临时目录，不污染工作区 data/。
 *
 * 职责：
 *  1. 准备 MySQL 夹具库 navkit_manual（testdata/mysql_fixture.sql 重定向库名）
 *  2. 启动真实桌面（WebView2 CDP 9235）
 *  3. 运行 manual-tests-20261112.mjs 全量用例
 *  4. 清理（杀进程 / 删库 / 删临时目录）
 *
 * 证据目录: docs/active/evidence/manual-20261112/
 */
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn, execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const label = 'manual-20261112';
const evidence = path.join(root, 'docs', 'active', 'evidence', label);
fs.mkdirSync(evidence, { recursive: true });
const runtime = fs.mkdtempSync(path.join(os.tmpdir(), `navkit-${label}-`));
const port = 9235;
const manifest = { label, root, evidence, runtime, port, started: new Date().toISOString(), steps: [] };
const persist = () => fs.writeFileSync(path.join(evidence, 'runner.json'), JSON.stringify(manifest, null, 2));
persist();
console.log(`EVIDENCE=${evidence}\nRUNTIME=${runtime}`);

const noBuild = process.argv.includes('--no-build');
const withVite = process.argv.includes('--with-vite');

async function command(labelCmd, exe, args, env = process.env) {
  const log = fs.openSync(path.join(evidence, `${labelCmd}.log`), 'w');
  const child = spawn(exe, args, { cwd: root, env, stdio: ['ignore', log, log], shell: exe === 'npm.cmd' });
  const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', resolve); });
  fs.closeSync(log);
  manifest.steps.push({ label: labelCmd, code }); persist();
  console.log(`${labelCmd}: ${code}`);
  return code;
}

// ---- MySQL 客户端 ------------------------------------------------------------
const mysqlExe = 'C:\\Program Files\\MySQL\\MySQL Server 8.0\\bin\\mysql.exe';
const MYSQL = { host: '127.0.0.1', port: 3306, user: 'root', password: '123456', db: 'navkit_manual' };
const my = (sql, input) => execFileSync(mysqlExe,
  ['-h', MYSQL.host, '-P', String(MYSQL.port), '-u', MYSQL.user, `-p${MYSQL.password}`, '--default-character-set=utf8mb4', '-N', '-e', sql],
  { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 });
/** 通过 stdin 执行大段 SQL（-e 与 stdin 互斥，mysql -e 不读标准输入）。 */
const myFile = (db, input) => execFileSync(mysqlExe,
  ['-h', MYSQL.host, '-P', String(MYSQL.port), '-u', MYSQL.user, `-p${MYSQL.password}`, '--default-character-set=utf8mb4', ...(db ? [db] : [])],
  { encoding: 'utf8', input, maxBuffer: 256 * 1024 * 1024 });

let app;
let vite;
let dbCreated = false;
function terminateOwnProcess() {
  if (app?.pid && app.exitCode === null) { try { execFileSync('taskkill', ['/PID', String(app.pid), '/T', '/F'], { stdio: 'ignore' }); } catch {} }
}
for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => { terminateOwnProcess(); process.exit(130); });
process.once('exit', terminateOwnProcess);

try {
  // ---- 构建（可用 --no-build 跳过） ------------------------------------------
  if (!noBuild) {
    const be = await command('cargo-build', 'cargo', ['build', '--manifest-path', path.join(root, 'src-tauri', 'Cargo.toml')]);
    if (be !== 0) throw Error('cargo build failed');
  } else if (!fs.existsSync(path.join(root, 'src-tauri', 'target', 'debug', 'navkit.exe'))) {
    throw Error('debug exe missing; run without --no-build');
  }

  // ---- Vite dev server（devUrl 目标） ----------------------------------------
  let viteUp = false;
  try { const r = await fetch('http://localhost:5183/index.html'); viteUp = r.ok; } catch {}
  if (!viteUp && withVite) {
    const log = fs.openSync(path.join(evidence, 'vite-dev.log'), 'w');
    vite = spawn('npm.cmd', ['run', 'dev'], { cwd: root, stdio: ['ignore', log, log], detached: false });
    for (let i = 0; i < 60; i++) {
      try { const r = await fetch('http://localhost:5183/index.html'); if (r.ok) { viteUp = true; break; } } catch {}
      await new Promise(r => setTimeout(r, 500));
    }
  }
  if (!viteUp) throw Error('Vite dev server 未就绪（http://localhost:5183）；请先 npm run dev 或加 --with-vite');
  console.log('Vite dev server ready on :5183');

  // ---- MySQL 夹具库 ----------------------------------------------------------
  const fixtureSql = fs.readFileSync(path.join(root, 'testdata', 'mysql_fixture.sql'), 'utf8')
    .replaceAll('navkit_accept', MYSQL.db);
  my(`DROP DATABASE IF EXISTS \`${MYSQL.db}\`; CREATE DATABASE \`${MYSQL.db}\` CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;`);
  myFile(MYSQL.db, fixtureSql);
  dbCreated = true;
  const tableCount = my(`SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='${MYSQL.db}';`).trim();
  console.log(`MySQL fixture ready: ${MYSQL.db} tables=${tableCount}`);

  // ---- 隔离运行时 ------------------------------------------------------------
  fs.copyFileSync(path.join(root, 'src-tauri', 'target', 'debug', 'navkit.exe'), path.join(runtime, 'navkit.exe'));
  fs.copyFileSync(path.join(root, 'testdata', 'demo.db'), path.join(runtime, 'manual.db'));
  fs.copyFileSync(path.join(root, 'testdata', 'demo.db'), path.join(runtime, 'sync_source.db'));
  fs.copyFileSync(path.join(root, 'testdata', 'demo.db'), path.join(runtime, 'sync_target.db'));
  fs.copyFileSync(path.join(root, 'testdata', 'import_stmts.sql'), path.join(runtime, 'run_stmts.sql'));
  fs.copyFileSync(path.join(root, 'testdata', 'import_utf8.csv'), path.join(runtime, 'import_utf8.csv'));
  fs.mkdirSync(path.join(runtime, 'out'), { recursive: true });
  fs.mkdirSync(path.join(runtime, 'data'), { recursive: true });

  // ---- 启动真实桌面 ----------------------------------------------------------
  const stdout = fs.openSync(path.join(evidence, 'desktop.log'), 'w');
  app = spawn(path.join(runtime, 'navkit.exe'), [], {
    cwd: runtime,
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
      WEBVIEW2_USER_DATA_FOLDER: path.join(runtime, 'data', 'cache', 'webview'),
    },
    stdio: ['ignore', stdout, stdout],
  });
  manifest.appPid = app.pid; persist();
  for (let i = 0; i < 120; i++) {
    if (app.exitCode !== null) throw Error(`Desktop exited before CDP: ${app.exitCode}`);
    try { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) break; } catch {}
    if (i === 119) throw Error('WebView2 CDP unavailable after 60s');
    await new Promise(r => setTimeout(r, 500));
  }
  console.log(`Desktop CDP ready pid=${app.pid}`);

  // ---- 运行全量用例 ----------------------------------------------------------
  const testEnv = { ...process.env, NAVKIT_E2E_EVID: evidence, NAVKIT_E2E_RUNTIME: runtime, NAVKIT_E2E_PID: String(app.pid), NAVKIT_MYSQL_DB: MYSQL.db };
  const exit = await command('manual-tests', 'node',
    [path.join(root, 'tools', 'acceptance', 'e2e', 'manual-tests-20261112.mjs')], testEnv);
  manifest.testExit = exit;
  if (exit !== 0) process.exitCode = 1;
} catch (error) {
  manifest.error = String(error);
  console.error(error);
  process.exitCode = 1;
} finally {
  terminateOwnProcess();
  if (vite && vite.exitCode === null) { try { vite.kill(); } catch {} }
  if (dbCreated) { try { my(`DROP DATABASE IF EXISTS \`${MYSQL.db}\`;`); manifest.steps.push({ label: 'mysql-cleanup', status: 'PASS' }); } catch (e) { manifest.steps.push({ label: 'mysql-cleanup', status: 'FAIL', error: String(e) }); } }
  try { fs.rmSync(runtime, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 }); manifest.steps.push({ label: 'runtime-cleanup', status: 'PASS' }); } catch (e) { manifest.steps.push({ label: 'runtime-cleanup', status: 'FAIL', error: String(e) }); }
  manifest.finished = new Date().toISOString(); persist();
}
