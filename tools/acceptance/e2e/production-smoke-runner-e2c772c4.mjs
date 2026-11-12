import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import {randomUUID,createHash} from 'node:crypto';
import {spawn,execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../..');
const id=randomUUID(), runtime=fs.mkdtempSync(path.join(os.tmpdir(),`navkit-production-${id}-`)),evidence=path.join(root,'tools','acceptance',`production-smoke-${id}`);
fs.mkdirSync(evidence,{recursive:true});
const source=process.env.NAVKIT_E2E_EXE||path.join(root,'src-tauri','target','release','navkit.exe');
const exe=path.join(runtime,'navkit.exe');fs.copyFileSync(source,exe);
const hash=file=>createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const report={id,source,sha256:hash(exe),runtime,evidence,port:9235,started:new Date().toISOString(),cleanup:[]};
const persist=()=>fs.writeFileSync(path.join(evidence,'runner.json'),JSON.stringify(report,null,2));persist();console.log(`EVIDENCE=${evidence}`);
let app;
const terminate=()=>{if(app?.pid&&app.exitCode===null)try{execFileSync('taskkill',['/PID',String(app.pid),'/T','/F'],{stdio:'ignore'});}catch{}};
process.once('exit',terminate);for(const signal of ['SIGINT','SIGTERM'])process.once(signal,()=>{terminate();process.exit(130)});
try{
 const log=fs.openSync(path.join(evidence,'desktop.log'),'w');
 app=spawn(exe,[],{cwd:runtime,env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=9235',WEBVIEW2_USER_DATA_FOLDER:path.join(runtime,'data','cache','webview')},stdio:['ignore',log,log]});report.pid=app.pid;persist();
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:9235/json/version')).ok)break;}catch{}if(i===119)throw Error('Production CDP not available');await new Promise(r=>setTimeout(r,500));}
 const harness=fs.readFileSync(path.join(root,'tools','acceptance','e2e','harness.mjs'),'utf8').replace('export const EVID = path.join(ROOT, "docs", "验收证据");','export const EVID = process.env.NAVKIT_E2E_EVID;');fs.writeFileSync(path.join(evidence,'harness.mjs'),harness);
 const logTest=fs.openSync(path.join(evidence,'tests.log'),'w');const test=spawn(process.execPath,[path.join(root,'tools','acceptance','e2e','production-smoke-tests-e2c772c4.mjs')],{cwd:root,env:{...process.env,NAVKIT_E2E_EVID:evidence,NAVKIT_E2E_RUNTIME:runtime,NAVKIT_E2E_PID:String(app.pid)},stdio:['ignore',logTest,logTest]});report.exit=await new Promise((resolve,reject)=>{test.once('exit',resolve);test.once('error',reject)});if(report.exit!==0)process.exitCode=1;
}catch(e){report.error=String(e);process.exitCode=1;}
finally{terminate();try{for(let attempt=0;;attempt++){try{fs.rmSync(runtime,{recursive:true,force:true,maxRetries:30,retryDelay:200});break;}catch(e){if(attempt===9)throw e;await new Promise(r=>setTimeout(r,1000));}}report.cleanup.push({runtime:'PASS'});}catch(e){report.cleanup.push({runtime:'FAIL',error:String(e)});process.exitCode=1;}report.finished=new Date().toISOString();persist();}
