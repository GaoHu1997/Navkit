// Real MySQL + fresh embedded Tauri; no existing user windows or data are modified.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import { spawn, execFileSync } from 'node:child_process';
import { randomUUID, createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
const root=fileURLToPath(new URL('../../',import.meta.url));
const source=path.resolve(process.argv[2]||path.join(root,'src-tauri/target/debug/navkit.exe'));
const tag=randomUUID().replaceAll('-',''), namespace='navkit_ipc_'+tag;
const q=n=>String.fromCharCode(96)+n+String.fromCharCode(96), db=q(namespace);
const limitedUser='ni_'+tag.slice(0,20), limitedPassword=randomUUID();
const runtime=fs.mkdtempSync(path.join(os.tmpdir(),'navkit-ipc-'+tag+'-'));
const evidence=path.join(root,'artifacts','routine-ipc-'+tag); fs.mkdirSync(evidence,{recursive:true});
const mysql=process.env.NAVKIT_MYSQL_CLIENT||'C:/Program Files/MySQL/MySQL Server 8.0/bin/mysql.exe';
// Only explicit local repository test credentials, never user connection storage.
const fixture=fs.readFileSync(path.join(root,'tools/acceptance/e2e/desktop14-runner-e2c772c4.mjs'),'utf8');
const password=process.env.NAVKIT_MYSQL_PASSWORD??fixture.match(/'-p([^']+)'/)[1];
const host=process.env.NAVKIT_MYSQL_HOST||'127.0.0.1',port=Number(process.env.NAVKIT_MYSQL_PORT||3306),user=process.env.NAVKIT_MYSQL_USER||'root';
assert(['127.0.0.1','localhost','::1'].includes(host),'Local test databases only');
const report={realDesktop:true,realMysql:true,source,runtime,namespace,started:new Date().toISOString(),checks:[],cleanup:[],pageErrors:[]};
const persist=()=>fs.writeFileSync(path.join(evidence,'report.json'),JSON.stringify(report,null,2));
function sql(statement){return execFileSync(mysql,['-h',host,'-P',String(port),'-u',user,'--default-character-set=utf8mb4','-N','-e',statement],{encoding:'utf8',env:{...process.env,MYSQL_PWD:password},stdio:['ignore','pipe','pipe']});}
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const freePort=()=>new Promise((resolve,reject)=>{const s=net.createServer();s.once('error',reject);s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>resolve(p));});});
let app,browser,page,databaseCreated=false,userCreated=false;
const processEnded=child=>new Promise(resolve=>{if(child.exitCode!==null)resolve();else child.once('exit',resolve);});
function terminateOwnApp(){if(app?.pid&&app.exitCode===null){try{execFileSync('taskkill',['/PID',String(app.pid),'/T','/F'],{stdio:'ignore'});}catch{}}}
process.once('exit',terminateOwnApp);
for(const signal of ['SIGINT','SIGTERM'])process.once(signal,()=>{terminateOwnApp();process.exit(130);});
async function invoke(command,args={}){
 const reply=await page.evaluate(async({command,args})=>{try{return {ok:true,value:await window.__TAURI_INTERNALS__.invoke(command,args)};}catch(error){return {ok:false,error:typeof error==='object'&&error?{message:error.message,code:error.code,operation_id:error.operation_id,retryable:error.retryable}:{message:String(error)}};}},{command,args});
 if(!reply.ok)throw Object.assign(new Error(reply.error.message),reply.error,{command});return reply.value;
}
async function check(name,work){try{const details=await work();report.checks.push({name,state:'passed',details});console.log('PASS '+name);}catch(error){report.checks.push({name,state:'failed',error:error.message,code:error.code,command:error.command});throw error;}finally{persist();}}
const connection=(id,username=user)=>({id,name:id,kind:'mysql',host,port,user:username,password:'',database:namespace,file:'',params:'',group:'',read_only:false,keep_alive:false});
const connId='routine_'+tag,limitedId='limited_'+tag;
const object=(name,kind)=>({connection_id:connId,catalog:namespace,schema:namespace,name,kind,identity_arguments:'',parent:null});
try{
 assert(fs.existsSync(source),'Build fresh --features custom-protocol executable first');
 report.binarySha256=createHash('sha256').update(fs.readFileSync(source)).digest('hex');report.mysqlVersion=sql('SELECT VERSION()').trim();
 sql('CREATE DATABASE '+db+' CHARACTER SET utf8mb4');databaseCreated=true;
 sql("CREATE TABLE "+db+".items(id INT PRIMARY KEY,note VARCHAR(50)); INSERT INTO "+db+".items VALUES(1,'中文'),(2,'second'); CREATE FUNCTION "+db+".fn_add10(x INT) RETURNS INT DETERMINISTIC RETURN x+10; CREATE FUNCTION "+db+".shared_name() RETURNS INT DETERMINISTIC RETURN 7;");
 sql('DELIMITER $$\nCREATE PROCEDURE '+db+".sp_values() BEGIN SELECT 'a;b' AS v_first; SELECT 2 AS v_second; END$$\nCREATE PROCEDURE "+db+".shared_name() BEGIN SELECT 8; END$$\nDELIMITER ;\n");
 sql("CREATE USER '"+limitedUser+"'@'127.0.0.1' IDENTIFIED BY '"+limitedPassword+"'");userCreated=true;
 sql('GRANT SELECT ON '+db+".* TO '"+limitedUser+"'@'127.0.0.1'");
 const exe=path.join(runtime,'navkit.exe');fs.copyFileSync(source,exe);fs.mkdirSync(path.join(runtime,'data'));
 const cdp=await freePort();report.cdpPort=cdp;const log=fs.openSync(path.join(evidence,'desktop.log'),'w');
 app=spawn(exe,[],{cwd:runtime,env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port='+cdp,WEBVIEW2_USER_DATA_FOLDER:path.join(runtime,'data/cache/webview')},stdio:['ignore',log,log]});fs.closeSync(log);report.appPid=app.pid;persist();
 const end=Date.now()+60000;
 for(;;){assert.equal(app.exitCode,null,'Isolated desktop must stay running');try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break;}catch{}if(Date.now()>end)throw new Error('Isolated WebView2 CDP startup timed out');await delay(250);}
 browser=await chromium.connectOverCDP('http://127.0.0.1:'+cdp);page=browser.contexts()[0].pages()[0];page.on('pageerror',e=>report.pageErrors.push(e.message));
 await page.waitForFunction(()=>window.__APP__&&window.__TAURI_INTERNALS__,null,{timeout:60000});report.url=page.url();
 assert(!/^https?:\/\/(localhost|127\.0\.0\.1):\d+/.test(report.url),'Embedded frontend required');assert.equal(await page.evaluate(()=>Boolean(window.__PREVIEW_MOCK__)),false);
 await check('isolated-empty-storage-and-real-ipc',async()=>{assert.deepEqual(await invoke('list_connections'),[]);await invoke('save_connection',{config:connection(connId),password,runtimeProfile:null});const opened=await invoke('open_connection',{id:connId});assert.equal(opened.ok,true);report.mysqlVersion=opened.version;});
 await check('fn_add10-definition-and-typed-call-5-equals-15',async()=>{
  const target=object('fn_add10','function');const legacy=await invoke('get_routine_definition',{id:connId,schemaName:namespace,name:'fn_add10',routineType:'function'});assert.match(legacy,/FUNCTION.*fn_add10/s);
  const snapshot=await invoke('read_exact_routine_design',{object:target});assert.match(snapshot.definition,/RETURN.*x.*10/s);
  const metadata=await invoke('get_routine_call_metadata',{object:target});assert.equal(metadata.completeness,'complete');assert.equal(metadata.parameters.length,1);
  const draft={object:target,metadata_revision:metadata.revision,source_fingerprint:metadata.source_fingerprint,arguments:[{ordinal:1,argument:{mode:'value',value:{kind:'integer',value:'5'}}}]},ownerId='routine-ipc-native';
  const plan=await invoke('preview_routine_call',{draft,ownerId});assert.match(plan.sql_preview,/^SELECT /);assert(!plan.sql_preview.startsWith('CALL'));
  const request=await invoke('issue_execution_grant',{request:{execution_id:'',connection_id:connId,namespace,owner_id:ownerId,sql:'ROUTINE_CALL_PLAN '+plan.plan_id,transaction_owner:null,timeout_ms:10000,origin:'routine',execution_grant:null,cancel_before_send:false}});
  const response=await invoke('execute_routine_call',{planId:plan.plan_id,request});assert.equal(response.outcome.state,'succeeded',response.outcome.error?.message);assert.equal(response.outcome.result.sets[0].rows[0][0],'15');return {execution_id:response.outcome.execution_id,result:'15'};
 });
 await check('same-name-function-and-procedure-stay-distinct',async()=>{for(const routineType of ['function','procedure']){const text=await invoke('get_routine_definition',{id:connId,schemaName:namespace,name:'shared_name',routineType});assert(text.toUpperCase().includes(routineType.toUpperCase()));}await assert.rejects(invoke('get_routine_definition',{id:connId,schemaName:namespace,name:'fn_add10',routineType:'trigger'}),e=>e.code==='ROUTINE_KIND_INVALID');});
 const backup={connection_id:connId,schema:namespace,tables:[],routines:['fn_add10','sp_values','shared_name','fn_add10'],include_structure:true,include_data:true,file_name:'routines',comment:'IPC regression'};
 await check('backup-functions-procedures-same-names-without-1305-or-omissions',async()=>{
  const result=await invoke('create_backup',{opts:backup});assert.equal(result[0],2);const file=path.join(runtime,'data/backups',connId,namespace,'routines.sql'),text=fs.readFileSync(file,'utf8');assert.equal(result[1],Buffer.byteLength(text));
  for(const name of ['fn_add10','sp_values','shared_name'])assert(text.includes(name));assert.equal((text.match(/DROP FUNCTION IF EXISTS/g)||[]).length,2);assert.equal((text.match(/DROP PROCEDURE IF EXISTS/g)||[]).length,2);assert(text.includes('DELIMITER '));fs.copyFileSync(file,path.join(evidence,'routines.sql'));return {rows:result[0],bytes:result[1]};
 });
 await check('restore-complete-compound-body-and-real-results',async()=>{
  sql('DROP FUNCTION '+db+'.fn_add10; DROP PROCEDURE '+db+'.sp_values; DROP FUNCTION '+db+'.shared_name; DROP PROCEDURE '+db+'.shared_name;');
  const result=await invoke('restore_backup_detailed',{connectionId:connId,schemaName:namespace,file:'routines.sql'});report.restore=result;persist();assert.equal(result.failed,0,JSON.stringify(result));
  const execute = async statement => {
    if (!statement.startsWith('CALL ')) return invoke('execute_sql', { id: connId, sql: statement, database: namespace, transactionOwner: null, origin: 'query' });
    // 使用产品正式typed例程调用通道；普通SQL查询入口仅把CALL当影响行数，不承诺多结果集。
    const name = statement.includes('.sp_values') ? 'sp_values' : 'shared_name';
    const target = object(name, 'procedure'), ownerId = 'routine-restore-verify';
    const metadata = await invoke('get_routine_call_metadata', { object: target });
    const draft = { object: target, metadata_revision: metadata.revision, source_fingerprint: metadata.source_fingerprint, arguments: [] };
    const plan = await invoke('preview_routine_call', { draft, ownerId }); assert.match(plan.sql_preview, /^CALL /);
    const request = await invoke('issue_execution_grant', { request: { execution_id: '', connection_id: connId, namespace, owner_id: ownerId, sql: 'ROUTINE_CALL_PLAN ' + plan.plan_id, transaction_owner: null, timeout_ms: 10000, origin: 'routine', execution_grant: null, cancel_before_send: false } });
    const response = await invoke('execute_routine_call', { planId: plan.plan_id, request }); assert.equal(response.outcome.state, 'succeeded', response.outcome.error?.message);
    return response.outcome.result;
  };
  report.verification = {};
  for (const [key, statement] of [['function','SELECT '+db+'.fn_add10(5)'],['compound','CALL '+db+'.sp_values()'],['sharedFunction','SELECT '+db+'.shared_name()'],['sharedProcedure','CALL '+db+'.shared_name()']]) { report.verification[key] = await execute(statement); persist(); }
  const cells = value => value.sets.flatMap(set => set.rows.flat()).filter(cell => cell !== null);
  assert.deepEqual(cells(report.verification.function),['15']); assert.deepEqual(cells(report.verification.compound),['a;b','2']);
  assert.deepEqual(cells(report.verification.sharedFunction),['7']); assert.deepEqual(cells(report.verification.sharedProcedure),['8']); return {statements:result.total};
 });
 await check('missing-routine-error-preserves-existing-backup',async()=>{const file=path.join(runtime,'data/backups',connId,namespace,'routines.sql'),before=fs.readFileSync(file);await assert.rejects(invoke('create_backup',{opts:{...backup,file_name:'missing-routine-check',routines:['missing_routine']}}),e=>e.code==='ROUTINE_NOT_FOUND');assert.deepEqual(fs.readFileSync(file),before);await assert.rejects(invoke('get_routine_definition',{id:connId,schemaName:namespace,name:'missing_routine',routineType:'function'}),e=>/1305|不存在|does not exist/i.test(e.message));});
 await check('permission-denial-never-becomes-empty-success',async()=>{await invoke('save_connection',{config:connection(limitedId,limitedUser),password:limitedPassword,runtimeProfile:null});assert.equal((await invoke('open_connection',{id:limitedId})).ok,true);await assert.rejects(invoke('get_routine_definition',{id:limitedId,schemaName:namespace,name:'fn_add10',routineType:'function'}));await assert.rejects(invoke('create_backup',{opts:{...backup,connection_id:limitedId,include_data:false}}));assert(!fs.existsSync(path.join(runtime,'data/backups',limitedId,namespace,'routines.sql')));});
 await check('exact-function-editor-has-no-unhandled-errors',async()=>{const observed=await page.evaluate(async({connId,namespace,target})=>{const a=window.__APP__;a.connections=await window.__TAURI_INTERNALS__.invoke('list_connections',{});await a.openRoutineDesignTab(connId,namespace,'fn_add10','function',target);return {kind:a.activeTab?.kind,type:a.activeTab?.objectType,name:a.activeTab?.object,sql:a.activeTab?.sql};},{connId,namespace,target:object('fn_add10','function')});assert.equal(observed.kind,'routinedesign');assert.equal(observed.type,'function');assert.equal(observed.name,'fn_add10');assert.match(observed.sql,/FUNCTION.*fn_add10/s);await page.screenshot({path:path.join(evidence,'function-editor.png')});assert.deepEqual(report.pageErrors,[]);return {kind:observed.kind,type:observed.type,name:observed.name};});
 report.state='passed';
}catch(error){report.state='failed';report.error=error.message;report.code=error.code;console.error('FAIL '+error.message);process.exitCode=1;}
finally{
 if(page){try{await invoke('close_all_connections');}catch(error){report.cleanup.push({resource:'connections',error:error.message});process.exitCode=1;}}
 if(browser){try{await browser.close();}catch{}}terminateOwnApp();if(app)await Promise.race([processEnded(app),delay(10000)]);
 if(userCreated){try{sql("DROP USER '"+limitedUser+"'@'127.0.0.1'");report.cleanup.push({resource:'test-user',state:'passed'});}catch{report.cleanup.push({resource:'test-user',state:'failed'});process.exitCode=1;}}
 if(databaseCreated){try{sql('DROP DATABASE '+db);report.cleanup.push({resource:'test-database',state:'passed'});}catch{report.cleanup.push({resource:'test-database',state:'failed'});process.exitCode=1;}}
 try{fs.rmSync(runtime,{recursive:true,force:true,maxRetries:20,retryDelay:250});report.cleanup.push({resource:'isolated-runtime',state:'passed'});}catch(error){report.cleanup.push({resource:'isolated-runtime',state:'retained',reason:error.message});}
 report.finished=new Date().toISOString();persist();console.log(JSON.stringify({state:report.state,passed:report.checks.filter(c=>c.state==='passed').length,evidence,cleanup:report.cleanup},null,2));
}
