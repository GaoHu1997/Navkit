/**
 * 发布前测试第一轮：桌面补充场景运行器（L6）。
 * 复用 desktop14-runner 的启动模式：debug exe + 隔离运行时 + CDP + 真实 MySQL/PG 临时命名空间。
 * 与 desktop14 的区别：默认跳过构建（--with-build 可开启），CDP 端口 9237，运行 round1-extra-tests.mjs。
 */
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import {randomUUID, createHash} from 'node:crypto';
import {spawn, execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../..');
const id=randomUUID();
const evidence=path.join(root,'docs','验收交付','发布前测试-20261110','证据',`round1-desktop-${id}`);
const runtime=fs.mkdtempSync(path.join(os.tmpdir(),`navkit-round1-${id}-`));
fs.mkdirSync(evidence,{recursive:true});
const namespace=`navkit_r1_${id.replaceAll('-','')}`;
const manifest={id,root,evidence,runtime,namespace,port:9237,started:new Date().toISOString(),builds:[],cleanup:[]};
const persist=()=>fs.writeFileSync(path.join(evidence,'runner.json'),JSON.stringify(manifest,null,2));
persist();
console.log(`EVIDENCE=${evidence}\nRUNTIME=${runtime}`);
async function command(label,exe,args,env=process.env){
  const log=fs.openSync(path.join(evidence,`${label}.log`),'w');
  const child=spawn(exe,args,{cwd:root,env,stdio:['ignore',log,log],shell:exe==='npm.cmd'});
  const code=await new Promise((resolve,reject)=>{child.once('error',reject);child.once('exit',resolve)});
  fs.closeSync(log); manifest.builds.push({label,code});persist();console.log(`${label}: ${code}`);
  return code;
}
const mysql='C:\\Program Files\\MySQL\\MySQL Server 8.0\\bin\\mysql.exe';
const psql=path.join(root,'tools','acceptance','pg','pgsql','bin','psql.exe');
const my=sql=>execFileSync(mysql,['-h','127.0.0.1','-P','3306','-u','root','-p123456','-N','-e',sql],{encoding:'utf8'});
const pg=sql=>execFileSync(psql,['-h','127.0.0.1','-p','5433','-U','postgres','-d','postgres','-v','ON_ERROR_STOP=1','-At','-c',sql],{encoding:'utf8',env:{...process.env,PGPASSWORD:''}});
let app;
let vite=null;
let createdMy=false,createdPg=false;
function terminateOwnProcess(){
  if(app?.pid && app.exitCode===null){try{execFileSync('taskkill',['/PID',String(app.pid),'/T','/F'],{stdio:'ignore'});}catch{}}
}
for(const signal of ['SIGINT','SIGTERM'])process.once(signal,()=>{terminateOwnProcess();process.exit(130)});
process.once('exit',terminateOwnProcess);
try{
  const withBuild=process.argv.includes('--with-build');
  if(withBuild){
    await command('npm-build','npm.cmd',['run','build']);
    const cargo=await command('cargo-build','cargo',['build','--manifest-path',path.join(root,'src-tauri','Cargo.toml')]);
    if(cargo!==0)throw Error('cargo build failed');
  }else{
    const exe=path.join(root,'src-tauri','target','debug','navkit.exe');
    if(!fs.existsSync(exe))throw Error('debug exe missing; run with --with-build');
  }
  my(`CREATE DATABASE \`${namespace}\`; CREATE TABLE \`${namespace}\`.items(id INT PRIMARY KEY,grp INT,name VARCHAR(100)); INSERT INTO \`${namespace}\`.items VALUES(1,2,'alpha'),(2,1,'beta'),(3,1,'gamma'),(4,2,'delta'); CREATE VIEW \`${namespace}\`.v_items AS SELECT * FROM \`${namespace}\`.items; CREATE FUNCTION \`${namespace}\`.fn_e2e(x INT) RETURNS INT DETERMINISTIC RETURN x+1;`);createdMy=true;
  pg(`CREATE SCHEMA "${namespace}"; CREATE TABLE "${namespace}".items(id INT PRIMARY KEY,grp INT,name TEXT); INSERT INTO "${namespace}".items VALUES(1,2,'alpha'),(2,1,'beta'),(3,1,'gamma'),(4,2,'delta');`);createdPg=true;
  // 先启动 Vite dev server（调试 exe 为 dev 模式，加载 devUrl localhost:5183，等价 tauri dev 语义）。
  // 若 5183 已有服务（本轮预先启动），复用之；否则由运行器启动并在结束时清理。
  let viteUp=false;
  try{const r=await fetch('http://localhost:5183/index.html');viteUp=r.ok;}catch{}
  if(!viteUp){
    const viteLog=fs.openSync(path.join(evidence,'vite.log'),'w');
    vite=spawn('npm.cmd',['run','dev'],{cwd:root,stdio:['ignore',viteLog,viteLog],shell:false,env:process.env});
    manifest.vitePid=vite.pid;persist();
    for(let i=0;i<60;i++){
      try{const r=await fetch('http://localhost:5183/index.html');if(r.ok){viteUp=true;break;}}catch{}
      await new Promise(r=>setTimeout(r,500));
    }
    if(!viteUp)throw Error('vite dev server unavailable on 5183 after 30s');
  }
  const exe=path.join(runtime,'navkit.exe');
  fs.copyFileSync(path.join(root,'src-tauri','target','debug','navkit.exe'),exe);
  fs.cpSync(path.join(root,'dist'),path.join(runtime,'frontend'),{recursive:true});
  const hash=file=>createHash('sha256').update(fs.readFileSync(file)).digest('hex');
  manifest.binarySha256=hash(exe);persist();
  fs.mkdirSync(path.join(runtime,'data'),{recursive:true});
  const stdout=fs.openSync(path.join(evidence,'desktop.log'),'w');
  app=spawn(exe,[],{cwd:runtime,env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=9237',WEBVIEW2_USER_DATA_FOLDER:path.join(runtime,'data','cache','webview')},stdio:['ignore',stdout,stdout]});
  manifest.appPid=app.pid;persist();
  let cdpUp=false;
  for(let i=0;i<120;i++){
    if(app.exitCode!==null)throw Error(`Desktop exited before CDP: ${app.exitCode}`);
    try{const response=await fetch('http://127.0.0.1:9237/json/version');if(response.ok){cdpUp=true;break;}}catch{}
    await new Promise(r=>setTimeout(r,500));
  }
  if(!cdpUp)throw Error('WebView2 CDP unavailable after 60s');
  const harness=fs.readFileSync(path.join(root,'tools','acceptance','e2e','harness.mjs'),'utf8').replace('export const EVID = path.join(ROOT, "docs", "验收证据");','export const EVID = process.env.NAVKIT_E2E_EVID;');
  fs.writeFileSync(path.join(evidence,'harness.mjs'),harness);
  const exit=await command('round1-tests','node',[path.join(root,'tools','acceptance','e2e','round1-extra-tests.mjs')],{...process.env,NAVKIT_R1_ONLY:process.env.NAVKIT_R1_ONLY??'',NAVKIT_E2E_EVID:evidence,NAVKIT_E2E_RUNTIME:runtime,NAVKIT_E2E_NAMESPACE:namespace,NAVKIT_E2E_PID:String(app.pid)});
  manifest.testExit=exit;
  const summary={passed:manifest.passed??null,failed:manifest.failed??null,total:manifest.total??null};
  if(!fs.existsSync(path.join(evidence,'report.json')))fs.writeFileSync(path.join(evidence,'report.json'),JSON.stringify({realDesktop:true,status:'FAIL',reason:'suite did not finalize report'},null,2));
  if(exit!==0)process.exitCode=1;
}catch(error){manifest.error=String(error);console.error(error);process.exitCode=1;}
finally{
  terminateOwnProcess();
  if(vite?.pid && vite.exitCode===null){try{execFileSync('taskkill',['/PID',String(vite.pid),'/T','/F'],{stdio:'ignore'});}catch{}}
  try{fs.rmSync(path.join(root,'node_modules','.vite-temp'),{recursive:true,force:true});}catch{}
  for(const [label,yes,fn,sql] of [['mysql',createdMy,my,`DROP DATABASE IF EXISTS \`${namespace}\``],['postgres',createdPg,pg,`DROP SCHEMA IF EXISTS "${namespace}" CASCADE`]]){
    if(yes){try{fn(sql);manifest.cleanup.push({label,status:'PASS'});}catch(error){manifest.cleanup.push({label,status:'FAIL',error:String(error)});process.exitCode=1;}}
  }
  try{fs.rmSync(runtime,{recursive:true,force:true,maxRetries:20,retryDelay:250});manifest.cleanup.push({label:'runtime',status:'PASS'});}catch(error){manifest.cleanup.push({label:'runtime',status:'FAIL',error:String(error)});}
  manifest.finished=new Date().toISOString();persist();
}
