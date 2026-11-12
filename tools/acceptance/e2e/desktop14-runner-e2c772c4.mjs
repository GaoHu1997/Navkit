import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import {randomUUID,createHash} from 'node:crypto';
import {spawn, execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../..');
const id=randomUUID();
const evidence=path.join(root,'tools','acceptance',`desktop14-${id}`);
const runtime=fs.mkdtempSync(path.join(os.tmpdir(),`navkit-desktop14-${id}-`));
fs.mkdirSync(evidence,{recursive:true});
const namespace=`navkit_e2e_${id.replaceAll('-','')}`;
const manifest={id,root,evidence,runtime,namespace,port:9235,started:new Date().toISOString(),builds:[],cleanup:[]};
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
let createdMy=false,createdPg=false;
let stopping=false;
function terminateOwnProcess(){
  if(app?.pid && app.exitCode===null){try{execFileSync('taskkill',['/PID',String(app.pid),'/T','/F'],{stdio:'ignore'});}catch{}}
}
for(const signal of ['SIGINT','SIGTERM'])process.once(signal,()=>{terminateOwnProcess();process.exit(130)});
process.once('exit',terminateOwnProcess);
try{
  const noBuild=process.argv.includes('--no-build');
  if(!noBuild)await command('npm-build','npm.cmd',['run','build']);
  const cargo=noBuild?0:await command('cargo-build','cargo',['build','--manifest-path',path.join(root,'src-tauri','Cargo.toml')]);
  if(!noBuild){await command('frontend-integration','node',['--test',path.join(root,'tools','acceptance','object-tools.test.mjs'),path.join(root,'tools','tests','data-grid-filter-sort.test.mjs')]);
  await command('cargo-integration','cargo',['test','--manifest-path',path.join(root,'src-tauri','Cargo.toml'),'--test','e2e_pipeline','--test','backup_roundtrip','--test','clickhouse_parse','--test','transfer_sync_acceptance','--','--nocapture']);}
  if(cargo!==0)throw Error('cargo build failed; see cargo-build.log');
  my(`CREATE DATABASE \`${namespace}\`; CREATE TABLE \`${namespace}\`.items(id INT PRIMARY KEY,grp INT,name VARCHAR(100)); INSERT INTO \`${namespace}\`.items VALUES(1,2,'alpha'),(2,1,'beta'),(3,1,'gamma'),(4,2,'delta'); CREATE VIEW \`${namespace}\`.v_items AS SELECT * FROM \`${namespace}\`.items; CREATE FUNCTION \`${namespace}\`.fn_e2e(x INT) RETURNS INT DETERMINISTIC RETURN x+1;`);createdMy=true;
  pg(`CREATE SCHEMA "${namespace}"; CREATE TABLE "${namespace}".items(id INT PRIMARY KEY,grp INT,name TEXT); INSERT INTO "${namespace}".items VALUES(1,2,'alpha'),(2,1,'beta'),(3,1,'gamma'),(4,2,'delta'); CREATE VIEW "${namespace}".v_items AS SELECT * FROM "${namespace}".items; CREATE FUNCTION "${namespace}".fn_e2e(x INT) RETURNS INT LANGUAGE SQL IMMUTABLE AS 'SELECT x+1';`);createdPg=true;
  const exe=path.join(runtime,'navkit.exe');
  fs.copyFileSync(path.join(root,'src-tauri','target','debug','navkit.exe'),exe);
  fs.cpSync(path.join(root,'dist'),path.join(runtime,'frontend'),{recursive:true});
  const hash=file=>createHash('sha256').update(fs.readFileSync(file)).digest('hex');
  manifest.binarySha256=hash(exe);manifest.frontendArtifacts=fs.readdirSync(path.join(runtime,'frontend','assets')).map(name=>({name,sha256:hash(path.join(runtime,'frontend','assets',name))}));persist();
  fs.mkdirSync(path.join(runtime,'data'),{recursive:true});
  const stdout=fs.openSync(path.join(evidence,'desktop.log'),'w');
  app=spawn(exe,[],{cwd:runtime,env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=9235',WEBVIEW2_USER_DATA_FOLDER:path.join(runtime,'data','cache','webview')},stdio:['ignore',stdout,stdout]});
  manifest.appPid=app.pid;persist();
  for(let i=0;i<120;i++){
    if(app.exitCode!==null)throw Error(`Desktop exited before CDP: ${app.exitCode}`);
    try{const response=await fetch('http://127.0.0.1:9235/json/version');if(response.ok)break;}catch{}
    if(i===119)throw Error('WebView2 CDP unavailable after 60s');
    await new Promise(r=>setTimeout(r,500));
  }
  const harness=fs.readFileSync(path.join(root,'tools','acceptance','e2e','harness.mjs'),'utf8').replace('export const EVID = path.join(ROOT, "docs", "验收证据");','export const EVID = process.env.NAVKIT_E2E_EVID;');
  fs.writeFileSync(path.join(evidence,'harness.mjs'),harness);
  const exit=await command('desktop14','node',[path.join(root,'tools','acceptance','e2e','desktop14-tests-e2c772c4.mjs')],{...process.env,NAVKIT_E2E_EVID:evidence,NAVKIT_E2E_RUNTIME:runtime,NAVKIT_E2E_NAMESPACE:namespace,NAVKIT_E2E_PID:String(app.pid)});
  manifest.testExit=exit;
  if(!fs.existsSync(path.join(evidence,'report.json')))fs.writeFileSync(path.join(evidence,'report.json'),JSON.stringify({realDesktop:true,status:'FAIL',reason:'Desktop suite did not reach report finalization; see desktop14.log',requirements:Array.from({length:14},(_,i)=>({requirement:i+1,status:'FAIL',reason:'Not fully verified due suite initialization/interruption failure'}))},null,2));
  if(exit!==0 || manifest.builds.some(b=>b.code!==0))process.exitCode=1;
}catch(error){manifest.error=String(error);console.error(error);process.exitCode=1;}
finally{
  terminateOwnProcess();
  for(const [label,yes,fn,sql] of [['mysql',createdMy,my,`DROP DATABASE IF EXISTS \`${namespace}\``],['postgres',createdPg,pg,`DROP SCHEMA IF EXISTS "${namespace}" CASCADE`]]){
    if(yes){try{fn(sql);manifest.cleanup.push({label,status:'PASS'});}catch(error){manifest.cleanup.push({label,status:'FAIL',error:String(error)});process.exitCode=1;}}
  }
  try{fs.rmSync(runtime,{recursive:true,force:true,maxRetries:20,retryDelay:250});manifest.cleanup.push({label:'runtime',status:'PASS'});}catch(error){manifest.cleanup.push({label:'runtime',status:'FAIL',error:String(error)});process.exitCode=1;}
  manifest.finished=new Date().toISOString();persist();
}
