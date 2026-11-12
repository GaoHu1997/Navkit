import assert from 'node:assert/strict';
import { test, before, after } from 'node:test';
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir, mkdtemp, rm, access } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import https from 'node:https';
import { createHash, randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { buildReference } from '../../bridge/http-tunnel/build.mjs';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const java=process.env.NAVKIT_JAVA||'D:/devlop/java/jdk-11.0.28/bin/java.exe';
const javac=path.join(path.dirname(java),process.platform==='win32'?'javac.exe':'javac');
const keytool=path.join(path.dirname(java),process.platform==='win32'?'keytool.exe':'keytool');
const jar=process.env.NAVKIT_H2_JAR||path.join(root,'tools/acceptance/datasources/h2/h2-2.3.232.jar');
const PROTOCOL='navkit-sql-tunnel/1',token='navkit_reference_test_token_A_0123456789abcdef',readToken='navkit_reference_test_read_token_0123456789abcd',bobToken='navkit_reference_test_token_B_0123456789abcdef',catalogScopeToken='navkit_reference_catalog_scope_token_0123456789abcdef';
const v=(kind,value)=>kind==='null'?{kind}:{kind,value};
const services=[],agents=[];let temp,cert,baseConfig,available=true;
function run(program,args,{env={},deadline=60000}={}) {return new Promise((resolve,reject)=>{const child=spawn(program,args,{windowsHide:true,stdio:['ignore','pipe','pipe'],env:{...process.env,...env}});let out='',err='';const timer=setTimeout(()=>{child.kill();reject(new Error(`process deadline: ${program}: ${err.slice(-4000)}`));},deadline);child.stdout.on('data',x=>{out+=x;});child.stderr.on('data',x=>{err+=x;});child.once('error',error=>{clearTimeout(timer);reject(error);});child.once('close',code=>{clearTimeout(timer);if(code!==0)reject(new Error(`${program} exit ${code}\n${err}\n${out}`));else resolve(out);});});}
class Service {
  constructor(child){this.child=child;this.lines=[];this.waiters=[];this.err='';let buffer='';child.stdout.setEncoding('utf8');child.stdout.on('data',part=>{buffer+=part;for(;;){const newline=buffer.indexOf('\n');if(newline<0)break;const line=buffer.slice(0,newline).trim();buffer=buffer.slice(newline+1);if(!line)continue;let value;try{value=JSON.parse(line);}catch{continue;}const i=this.waiters.findIndex(w=>w.predicate(value));if(i>=0){const waiter=this.waiters.splice(i,1)[0];clearTimeout(waiter.timer);waiter.resolve(value);}else this.lines.push(value);}});child.stderr.on('data',part=>{this.err=(this.err+part).slice(-16000);});this.exit=new Promise(resolve=>child.once('close',code=>{this.closed=true;this.code=code;for(const w of this.waiters.splice(0)){clearTimeout(w.timer);w.reject(new Error(`service exit ${code}: ${this.err}`));}resolve(code);}));}
  wait(predicate,ms=30000){const i=this.lines.findIndex(predicate);if(i>=0)return Promise.resolve(this.lines.splice(i,1)[0]);return new Promise((resolve,reject)=>{const w={predicate,resolve,reject,timer:setTimeout(()=>{this.waiters=this.waiters.filter(x=>x!==w);reject(new Error(`service event deadline: ${this.err}`));},ms)};this.waiters.push(w);});}
  async inspect(){const result=this.wait(x=>x.inspection);this.child.stdin.write('inspect\n');return result;}
  async close(){if(this.closed)return;this.child.stdin.end('stop\n');const timer=setTimeout(()=>this.child.kill(),5000);await this.exit;clearTimeout(timer);}
}
function digest(value){return createHash('sha256').update(value).digest('hex');}
function eid(binding,sequence){const bytes=createHash('sha256').update(`${PROTOCOL}\0${binding.session_id}\0${binding.session_generation}\0${sequence}`).digest().subarray(0,16);bytes[6]=(bytes[6]&15)|128;bytes[8]=(bytes[8]&63)|128;const hex=bytes.toString('hex');return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`;}
function agent(){const a=new https.Agent({keepAlive:true,maxSockets:2,ca:cert});agents.push(a);return a;}
function request(service,method,route,body,{bearer=token,owner,headers={},httpAgent=service.agent,raw,onFrame,deadline=20000}={}) {
  return new Promise((resolve,reject)=>{const target=new URL(route,service.endpoint),payload=raw??(body===undefined?undefined:JSON.stringify(body));const requestHeaders={'X-Navkit-Protocol':PROTOCOL,Authorization:`Bearer ${bearer}`,...(owner?{'X-Navkit-Owner':owner}:{}),...headers};if(payload!==undefined){requestHeaders['Content-Type']='application/json';requestHeaders['Content-Length']=Buffer.byteLength(payload);}
    const req=https.request(target,{method,headers:requestHeaders,agent:httpAgent,rejectUnauthorized:true},res=>{let data='',pending='';const frames=[];res.setEncoding('utf8');res.on('data',part=>{data+=part;if(Buffer.byteLength(data)>20*1024*1024){req.destroy(new Error('test response quota'));return;}if(String(res.headers['content-type']).startsWith('application/x-ndjson')){pending+=part;for(;;){const at=pending.indexOf('\n');if(at<0)break;const line=pending.slice(0,at);pending=pending.slice(at+1);if(line){const frame=JSON.parse(line);frames.push(frame);onFrame?.(frame);}}}});res.once('error',reject);res.once('end',()=>{try{const stream=String(res.headers['content-type']).startsWith('application/x-ndjson');resolve({status:res.statusCode,headers:res.headers,data,frames,json:stream?null:JSON.parse(data),pending});}catch(error){reject(error);}});});
    req.setTimeout(deadline,()=>req.destroy(new Error('independent HTTPS request deadline')));req.once('error',reject);if(payload!==undefined)req.write(payload);req.end();
  });
}
async function start(overrides={}) {const directory=await mkdtemp(path.join(temp,'server-'));const config={...baseConfig,...overrides,limits:{...baseConfig.limits,...overrides.limits}};const configPath=path.join(directory,'config.json');await writeFile(configPath,JSON.stringify(config));const child=spawn(java,['-Dfile.encoding=UTF-8','-cp',`${path.join(temp,'classes')}${path.delimiter}${jar}`,'HttpTunnelTestServer',configPath],{windowsHide:true,stdio:['pipe','pipe','pipe'],env:{...process.env,NAVKIT_TEST_STORE_PASSWORD:'temporary-keystore-only',NAVKIT_TEST_DB_PASSWORD:''}});const service=new Service(child);services.push(service);const ready=await service.wait(x=>x.ready);service.endpoint=ready.endpoint;service.agent=agent();const caps=await request(service,'GET','/v1/capabilities');assert.equal(caps.status,200);service.capabilities=caps.json;return service;}
async function open(service,{alias='main',namespace='PUBLIC',readonly=false,bearer=token,owner=randomUUID()}={}){const target=service.capabilities.targets.find(x=>x.alias===alias);const body={protocol:PROTOCOL,request_id:randomUUID(),owner_id:owner,target_alias:alias,expected_server_id:service.capabilities.server_id,expected_target_id:target.target_id,namespace,read_only:readonly};const response=await request(service,'POST','/v1/sessions',body,{bearer});assert.equal(response.status,201,JSON.stringify(response.json));assert.equal(response.json.binding.owner_id,owner);return {binding:response.json.binding,sequence:0,bearer};}
function execution(session,sql,binds=[],timeout=10000){const sequence=String(++session.sequence);return {protocol:PROTOCOL,request_id:randomUUID(),binding:session.binding,execution_id:eid(session.binding,sequence),sequence,sql,binds,timeout_ms:timeout};}
async function execute(service,session,sql,binds=[],options={}){const body=execution(session,sql,binds,options.timeout);const response=await request(service,'POST',`/v1/sessions/${session.binding.session_id}/executions`,body,{bearer:session.bearer,...options});return {body,...response};}
function final(response){assert.equal(response.status,200,JSON.stringify(response.json));assert.equal(response.pending,'');assert.equal(response.frames.at(-1)?.kind,'final',response.data);for(const frame of response.frames){assert.equal(frame.protocol,PROTOCOL);assert.equal(frame.request_id,response.body.request_id);assert.equal(frame.execution_id,response.body.execution_id);assert.equal(frame.sequence,response.body.sequence);assert.deepEqual(frame.binding,response.body.binding);}return response.frames.at(-1);}
function bound(session){return {protocol:PROTOCOL,request_id:randomUUID(),binding:session.binding};}
async function close(service,session){return request(service,'DELETE',`/v1/sessions/${session.binding.session_id}`,bound(session),{bearer:session.bearer});}

before(async()=>{
  try{await Promise.all([access(java),access(javac),access(keytool),access(jar)]);}catch{available=false;return;}
  temp=await mkdtemp(path.join(os.tmpdir(),'navkit-http-tunnel-'));const store=path.join(temp,'server.p12'),pem=path.join(temp,'server.pem');
  await run(keytool,['-genkeypair','-alias','server','-keyalg','RSA','-keysize','2048','-validity','2','-dname','CN=localhost','-ext','SAN=dns:localhost,ip:127.0.0.1','-storetype','PKCS12','-keystore',store,'-storepass:env','NAVKIT_TEST_STORE_PASSWORD','-noprompt'],{env:{NAVKIT_TEST_STORE_PASSWORD:'temporary-keystore-only'}});
  await run(keytool,['-exportcert','-rfc','-alias','server','-keystore',store,'-storepass:env','NAVKIT_TEST_STORE_PASSWORD','-file',pem],{env:{NAVKIT_TEST_STORE_PASSWORD:'temporary-keystore-only'}});cert=await readFile(pem);
  const extraSource=await readFile(path.join(root,'tools/tests/HttpTunnelTestServer.java'),'utf8');const built=await buildReference({out:path.join(temp,'source'),jar,java,extraSource});await mkdir(path.join(temp,'classes'));await run(javac,['-encoding','UTF-8','-cp',jar,'-d',path.join(temp,'classes'),built.source]);
  const seed=path.join(temp,'seed.sql'),url=`jdbc:h2:file:${path.join(temp,'data').replaceAll('\\','/')};DB_CLOSE_DELAY=-1;MODE=REGULAR`;
  const catalogSeed=[
    'CREATE TABLE PUBLIC."目录_%_中文"(ID INTEGER); CREATE TABLE PUBLIC."目录_AX_中文"(ID INTEGER); CREATE VIEW PUBLIC."目录_%_视图" AS SELECT ID FROM PUBLIC.EVENTS;',
    'CREATE TABLE OTHER."目录_%_秘密"(ID INTEGER); CREATE SCHEMA EMPTY_CATALOG;',
    'CREATE SCHEMA "AUTH_%"; CREATE TABLE "AUTH_%"."可见_%_中文"(ID INTEGER); CREATE SCHEMA AUTH_AX; CREATE TABLE AUTH_AX.HIDDEN_NAMESPACE(ID INTEGER);',
    'CREATE SCHEMA QUOTA_512; CREATE SCHEMA QUOTA_513;',
    ...Array.from({length:512},(_,i)=>`CREATE TABLE QUOTA_512."T${String(i).padStart(3,'0')}"(ID INTEGER);`),
    ...Array.from({length:513},(_,i)=>`CREATE TABLE QUOTA_513."T${String(i).padStart(3,'0')}"(ID INTEGER);`),
  ].join(' ');
  await writeFile(seed,`CREATE TABLE EVENTS(ID BIGINT PRIMARY KEY,TXT VARCHAR(200),BIG BIGINT,AMT DECIMAL(38,12),NIL VARCHAR(20),EMPTY VARCHAR(20),BIN VARBINARY(32),D DATE,TM TIME(9),TS TIMESTAMP(9),TZ TIMESTAMP(9) WITH TIME ZONE); CREATE TABLE WORK AS SELECT X AS N FROM SYSTEM_RANGE(1,2000); CREATE SCHEMA OTHER; CREATE TABLE OTHER.EVENTS(ID BIGINT PRIMARY KEY,TXT VARCHAR(200)); INSERT INTO OTHER.EVENTS VALUES(7,'other'); ${catalogSeed}`);
  await run(java,['-cp',jar,'org.h2.tools.RunScript','-url',url,'-user','sa','-password','','-script',seed]);
  const namespaces=['PUBLIC','OTHER','EMPTY_CATALOG','QUOTA_512','QUOTA_513','AUTH_%','AUTH_AX'];
  const target=alias=>({alias,target_id:randomUUID(),jdbc_url:url,user:'sa',password_env:'NAVKIT_TEST_DB_PASSWORD',namespaces});
  baseConfig={server_id:randomUUID(),bind_host:'127.0.0.1',port:0,pkcs12:store,store_password_env:'NAVKIT_TEST_STORE_PASSWORD',targets:['main','ack_lost','slow_commit','cleanup_read','catalog_denied'].map(target),tokens:[{sha256:digest(token),principal:'alice',aliases:['main','ack_lost','slow_commit','cleanup_read','catalog_denied'],namespaces:namespaces.filter(name=>name!=='AUTH_AX'),operations:['query','sql_write','metadata','cancel','status','close']},{sha256:digest(catalogScopeToken),principal:'alice',aliases:['main'],namespaces:['PUBLIC'],operations:['query','metadata','cancel','status','close']},{sha256:digest(readToken),principal:'reader',aliases:['main'],namespaces:['PUBLIC'],operations:['query','metadata','cancel','status','close']},{sha256:digest(bobToken),principal:'bob',aliases:['main'],namespaces:['PUBLIC'],operations:['query','metadata','cancel','status','close']}],limits:{status_ttl_ms:600000,session_idle_ms:60000,max_sessions:32,max_executions:256,max_timeout_ms:10000,drain_wait_ms:1}};
});
after(async()=>{for(const service of services)await service.close();for(const a of agents)a.destroy();if(temp&&path.basename(temp).startsWith('navkit-http-tunnel-'))await rm(temp,{recursive:true,force:true});});

// All assertions below cross actual HTTPS sockets and actual JDBC/H2 connections.
test('real HTTPS reference tunnel: identity, scopes, typed streaming, once-only execution and independent control',{timeout:180000},async t=>{
  if(!available){t.skip('Java11/keytool/verified H2 dependency missing; real HTTPS/H2 NOT VERIFIED');return;}
  const service=await start(),session=await open(service);
  await t.test('TLS trust and protocol/version/auth failures are not SQL success',async()=>{
    await assert.rejects(()=>request(service,'GET','/v1/capabilities',undefined,{httpAgent:new https.Agent({rejectUnauthorized:true})}),/certificate|self.signed|unable to verify/i);
    const wrong=await request(service,'GET','/v1/capabilities',undefined,{headers:{'X-Navkit-Protocol':'navkit-sql-tunnel/999'}});assert.equal(wrong.status,400);assert.equal(wrong.json.code,'PROTOCOL_MISMATCH');assert.equal(wrong.json.effects,'none');
    const unauth=await request(service,'GET','/v1/capabilities',undefined,{bearer:'x'.repeat(40)});assert.equal(unauth.status,401);assert.ok(!unauth.data.includes(token));
    assert.equal((await request(service,'GET','/redirect')).status,404);assert.equal(service.capabilities.capabilities.row_mutation,false);assert.equal(service.capabilities.capabilities.ddl,false);assert.equal(service.capabilities.capabilities.manual_transaction,false);
  });
  await t.test('actual typed PreparedStatement round trip and real schema metadata',async()=>{
    const values=[v('integer','1'),v('text',`中文; O'Reilly 🙂`),v('integer','9223372036854775807'),v('decimal','12345678901234567890.123456789012'),v('null'),v('text',''),v('binary','AP8BgA=='),v('date','2026-02-28'),v('time','12:34:56.123456789'),v('timestamp','2026-11-10T12:34:56.123456789'),v('timestamptz','2026-11-10T12:34:56.123456789+05:30')];
    const inserted=await execute(service,session,'INSERT INTO EVENTS(ID,TXT,BIG,AMT,NIL,EMPTY,BIN,D,TM,TS,TZ) VALUES(?,?,?,?,?,?,?,?,?,?,?)',values);assert.equal(final(inserted).state,'succeeded',inserted.data);assert.equal(final(inserted).affected_rows,'1');assert.equal(final(inserted).transaction,'committed');
    const selected=await execute(service,session,'SELECT ID,TXT,BIG,AMT,NIL,EMPTY,BIN,D,TM,TS,TZ FROM EVENTS WHERE ID = ?',[v('integer','1')]);assert.equal(final(selected).state,'succeeded',selected.data);assert.equal(final(selected).rows_read,'1');assert.deepEqual(selected.frames.find(x=>x.kind==='row').values,values);
    const meta=await request(service,'POST',`/v1/sessions/${session.binding.session_id}/metadata`,{...bound(session),object:{catalog:session.binding.target_id,schema:'PUBLIC',name:'EVENTS'}});assert.equal(meta.status,403,'wrong catalog is never guessed');
    const openedCaps=await request(service,'GET','/v1/capabilities');assert.equal(openedCaps.json.targets[0].product,'H2');
    const catalog=(await execute(service,session,'SELECT ID FROM EVENTS WHERE ID = ?',[v('integer','1')])).frames[0];assert.equal(catalog.columns[0].data_type,'BIGINT');
    // H2 file catalog is obtained through the actual open-session response in a new request.
    const opened=await request(service,'POST','/v1/sessions',{protocol:PROTOCOL,request_id:randomUUID(),owner_id:randomUUID(),target_alias:'main',expected_server_id:service.capabilities.server_id,expected_target_id:service.capabilities.targets[0].target_id,namespace:'PUBLIC',read_only:true});assert.equal(opened.status,201);
    const actual=await request(service,'POST',`/v1/sessions/${opened.json.binding.session_id}/metadata`,{protocol:PROTOCOL,request_id:randomUUID(),binding:opened.json.binding,object:{catalog:opened.json.catalog,schema:'PUBLIC',name:'EVENTS'}});assert.equal(actual.status,200,actual.data);assert.match(actual.json.snapshot.revision,/^[0-9a-f]{64}$/);assert.deepEqual(actual.json.snapshot.primary_key,['ID']);assert.equal(actual.json.snapshot.session_id,opened.json.binding.session_id);assert.ok(actual.json.snapshot.columns.find(x=>x.name==='AMT').precision>=38);assert.ok(actual.json.snapshot.definition.includes('CREATE'));
  });
  await t.test('same namespace pages do not share physical selection state; target/owner/scope cannot be switched',async()=>{
    const other=await open(service,{namespace:'OTHER'});const [a,b]=await Promise.all([execute(service,session,'SELECT TXT FROM EVENTS WHERE ID = ?',[v('integer','1')]),execute(service,other,'SELECT TXT FROM EVENTS WHERE ID = ?',[v('integer','7')])]);assert.equal(final(a).state,'succeeded');assert.equal(final(b).state,'succeeded');assert.equal(b.frames.find(x=>x.kind==='row').values[0].value,'other');
    const forged=execution(session,'SELECT 1');forged.binding={...forged.binding,owner_id:randomUUID()};const result=await request(service,'POST',`/v1/sessions/${session.binding.session_id}/executions`,forged);assert.equal(result.status,403);session.sequence--;
    assert.equal((await request(service,'POST',`/v1/sessions/${session.binding.session_id}/metadata`,{...bound(session),object:{catalog:'DATA',schema:'OTHER',name:'EVENTS'}})).status,403);
    const read=await open(service,{bearer:readToken});assert.equal(read.binding.read_only,true);const denied=await execute(service,read,'INSERT INTO EVENTS(ID,TXT) VALUES(?,?)',[v('integer','2'),v('text','forbidden')]);assert.equal(denied.status,403);
    const cross=await request(service,'POST',`/v1/sessions/${session.binding.session_id}/metadata`,{...bound(session),object:{catalog:'DATA',schema:'PUBLIC',name:'EVENTS'}},{bearer:bobToken});assert.equal(cross.status,403);
  });
  await t.test('complete grammar rejects functions, scripts, comments, unions, DDL and injected endpoint fields before SQL',async()=>{
    const before=await service.inspect();for(const sql of ['DROP TABLE EVENTS','SELECT 1; INSERT INTO EVENTS(ID) VALUES(80)','SELECT CSVREAD(\'C:/secret\')','SELECT NEXT VALUE FOR X','SELECT 1 UNION SELECT 2','SELECT 1 --x','SELECT 1 /*x*/','CALL 1','SELECT * FROM OTHER.EVENTS']){const body=execution(session,sql);const rejected=await request(service,'POST',`/v1/sessions/${session.binding.session_id}/executions`,body);assert.ok(rejected.status>=400,`${sql}: ${rejected.data}`);session.sequence--;}
    const body=execution(session,'SELECT 1');body.host='example.com';assert.equal((await request(service,'POST',`/v1/sessions/${session.binding.session_id}/executions`,body)).status,400);session.sequence--;
    const duplicate=`{"protocol":"${PROTOCOL}","request_id":"${randomUUID()}","binding":${JSON.stringify(session.binding)},"binding":{}}`;assert.equal((await request(service,'POST',`/v1/sessions/${session.binding.session_id}/metadata`,undefined,{raw:duplicate})).status,400);
    assert.equal((await service.inspect()).write_calls,before.write_calls);
  });
  await t.test('independent control TLS socket cancels a real running H2 statement without the session mutex',async()=>{
    const control=agent(),body=execution(session,'SELECT COUNT(*) FROM WORK AS A, WORK AS B, WORK AS C');const started=Date.now();const running=request(service,'POST',`/v1/sessions/${session.binding.session_id}/executions`,body,{httpAgent:agent()});
    let status;for(let i=0;i<40;i++){status=await request(service,'GET',`/v1/executions/${body.execution_id}`,undefined,{owner:session.binding.owner_id,httpAgent:control});if(status.status===200&&status.json.state==='running')break;await new Promise(resolve=>setTimeout(resolve,10));}assert.equal(status.status,200,status.data);assert.equal(status.json.state,'running');
    const foreign=await request(service,'GET',`/v1/executions/${body.execution_id}`,undefined,{owner:randomUUID(),httpAgent:control});assert.equal(foreign.status,403);
    const cancelled=await request(service,'POST',`/v1/executions/${body.execution_id}/cancel`,bound(session),{httpAgent:control});assert.equal(cancelled.status,202);assert.equal(cancelled.json.requested,true);assert.ok(Date.now()-started<5000,'control did not wait for the long database statement');const result={body,...await running};assert.equal(final(result).state,'cancelled',result.data);assert.equal(final(result).affected_rows,'0');assert.equal(final(result).transaction,'rolled_back');
    const healthy=await execute(service,session,'SELECT 1');assert.equal(final(healthy).state,'succeeded','cancelled query drained and restored the same connection');
  });
  await t.test('status quota eviction cannot reactivate an old execution ID',async()=>{
    const original=await execute(service,session,'INSERT INTO EVENTS(ID,TXT) VALUES(?,?)',[v('integer','90'),v('text','once')]);assert.equal(final(original).state,'succeeded');
    for(let i=0;i<260;i++){const value=await execute(service,session,'SELECT 1');assert.equal(final(value).state,'succeeded');}
    await new Promise(resolve=>setTimeout(resolve,150));const expired=await request(service,'GET',`/v1/executions/${original.body.execution_id}`,undefined,{owner:session.binding.owner_id});assert.equal(expired.status,404);
    const before=await service.inspect();const replay=await request(service,'POST',`/v1/sessions/${session.binding.session_id}/executions`,original.body);assert.equal(replay.status,409,replay.data);assert.equal(replay.json.code,'EXECUTION_REPLAY');assert.equal((await service.inspect()).write_calls,before.write_calls,'zero second JDBC executeLargeUpdate after actual 256-entry status eviction');
    const other=await open(service);const transplanted={...original.body,binding:other.binding,sequence:'1'};assert.equal((await request(service,'POST',`/v1/sessions/${other.binding.session_id}/executions`,transplanted)).status,400);assert.equal((await service.inspect()).write_calls,before.write_calls);
    const rows=await execute(service,session,'SELECT COUNT(*) FROM EVENTS WHERE ID = ?',[v('integer','90')]);assert.equal(rows.frames.find(x=>x.kind==='row').values[0].value,'1');
  });
  await t.test('real COMMIT followed by lost acknowledgment is UNKNOWN/null, quarantined and never replayed',async()=>{
    const fault=await open(service,{alias:'ack_lost'}),before=await service.inspect();const lost=await execute(service,fault,'INSERT INTO EVENTS(ID,TXT) VALUES(?,?)',[v('integer','91'),v('text','ack lost')]);const terminal=final(lost);assert.equal(terminal.state,'unknown',lost.data);assert.equal(terminal.transaction,'unknown');assert.equal(terminal.affected_rows,null);assert.equal(terminal.connection_poisoned,true);
    const status=await request(service,'GET',`/v1/executions/${lost.body.execution_id}`,undefined,{owner:fault.binding.owner_id});assert.equal(status.json.state,'unknown');assert.equal(status.json.affected_rows,null);
    const retry=await request(service,'POST',`/v1/sessions/${fault.binding.session_id}/executions`,lost.body);assert.ok(retry.status>=400);assert.equal((await service.inspect()).write_calls,before.write_calls+1);
    const observer=await execute(service,session,'SELECT COUNT(*) FROM EVENTS WHERE ID = ?',[v('integer','91')]);assert.equal(observer.frames.find(x=>x.kind==='row').values[0].value,'1','independent physical connection sees the one actual committed write');
  });
  await t.test('close returns pending while an actual commit is in progress, retains ownership and only then acknowledges closed',async()=>{
    const slow=await open(service,{alias:'slow_commit'}),waiting=service.wait(x=>x.commit_waiting),body=execution(slow,'INSERT INTO EVENTS(ID,TXT) VALUES(?,?)',[v('integer','92'),v('text','drain')]);const running=request(service,'POST',`/v1/sessions/${slow.binding.session_id}/executions`,body);await waiting;const drain=await close(service,slow);assert.equal(drain.status,202,drain.data);assert.equal(drain.json.state,'pending');assert.deepEqual(drain.json.pending_execution_ids,[body.execution_id]);
    const status=await request(service,'GET',`/v1/executions/${body.execution_id}`,undefined,{owner:slow.binding.owner_id,httpAgent:agent()});assert.equal(status.status,200);assert.equal(status.json.state,'running');const completed={body,...await running};assert.equal(final(completed).state,'succeeded',completed.data);const closed=await close(service,slow);assert.equal(closed.json.state,'closed');assert.deepEqual(closed.json.pending_execution_ids,[]);assert.ok((await request(service,'POST',`/v1/sessions/${slow.binding.session_id}/executions`,execution(slow,'SELECT 1'))).status>=400);
  });
  await t.test('acknowledged read rollback followed by a real JDBC restore exception closes and quarantines the physical session',async()=>{
    const cleanup=await open(service,{alias:'cleanup_read'}),before=await service.inspect();const result=await execute(service,cleanup,'SELECT 1');const terminal=final(result);assert.equal(terminal.state,'failed',result.data);assert.equal(terminal.affected_rows,'0');assert.equal(terminal.connection_poisoned,true);assert.equal((await service.inspect()).write_calls,before.write_calls);assert.equal((await service.inspect()).closed_physical,before.closed_physical+1,'real raw JDBC Connection.close and isClosed were observed');const rejected=await request(service,'POST',`/v1/sessions/${cleanup.binding.session_id}/executions`,execution(cleanup,'SELECT 1'));assert.equal(rejected.status,404);const drained=await close(service,cleanup);assert.equal(drained.json.state,'closed');assert.equal(drained.json.connection_poisoned,true);assert.deepEqual(drained.json.pending_execution_ids,[]);
  });
  assert.equal((await close(service,session)).json.state,'closed');
  await service.close();
});

test('real HTTPS idle session expiry cannot revive the old physical binding',{timeout:60000},async t=>{
  if(!available){t.skip('real Java/H2/HTTPS dependencies missing: NOT VERIFIED');return;}const service=await start({limits:{session_idle_ms:100}}),session=await open(service),body=execution(session,'SELECT 1');await new Promise(resolve=>setTimeout(resolve,250));const rejected=await request(service,'POST',`/v1/sessions/${session.binding.session_id}/executions`,body);assert.equal(rejected.status,404);const next=await open(service);assert.notEqual(next.binding.session_id,session.binding.session_id);assert.notEqual(next.binding.session_generation,session.binding.session_generation);const forged={...body,binding:next.binding};assert.equal((await request(service,'POST',`/v1/sessions/${next.binding.session_id}/executions`,forged)).status,400);await service.close();
});


test('real HTTPS status TTL expiry retains the physical sequence watermark and performs zero second writes',{timeout:60000},async t=>{
  if(!available){t.skip('real Java/H2/HTTPS dependencies missing: NOT VERIFIED');return;}
  const service=await start({limits:{status_ttl_ms:50}}),session=await open(service);const original=await execute(service,session,'INSERT INTO EVENTS(ID,TXT) VALUES(?,?)',[v('integer','93'),v('text','expired evidence')]);assert.equal(final(original).state,'succeeded');
  await new Promise(resolve=>setTimeout(resolve,150));assert.equal((await request(service,'GET',`/v1/executions/${original.body.execution_id}`,undefined,{owner:session.binding.owner_id})).status,404);const before=await service.inspect();const replay=await request(service,'POST',`/v1/sessions/${session.binding.session_id}/executions`,original.body);assert.equal(replay.status,409);assert.equal(replay.json.code,'EXECUTION_REPLAY');assert.equal((await service.inspect()).write_calls,before.write_calls);const observer=await execute(service,session,'SELECT COUNT(*) FROM EVENTS WHERE ID = ?',[v('integer','93')]);assert.equal(observer.frames.find(x=>x.kind==='row').values[0].value,'1');await service.close();
});

// The unmodified production entry point is source-launched from buildReference's
// returned argv. No fault subclass or test-only class is included in this source.
test('production Java 11 source launcher starts the actual HTTPS reference entry point',{timeout:60000},async t=>{
  if(!available){t.skip('real Java/H2/HTTPS dependencies missing: NOT VERIFIED');return;}
  const directory=await mkdtemp(path.join(temp,'production-'));const configPath=path.join(directory,'config.json');await writeFile(configPath,JSON.stringify(baseConfig));const built=await buildReference({out:path.join(directory,'source'),jar,java});const source=await readFile(built.source,'utf8');assert.ok(!source.includes('class HttpTunnelTestServer'));const child=spawn(built.command,[...built.args,configPath],{windowsHide:true,stdio:['pipe','pipe','pipe'],env:{...process.env,NAVKIT_TEST_STORE_PASSWORD:'temporary-keystore-only',NAVKIT_TEST_DB_PASSWORD:''}});const service=new Service(child);services.push(service);const ready=await service.wait(x=>x.ready);service.endpoint=ready.endpoint;service.agent=agent();const caps=await request(service,'GET','/v1/capabilities');assert.equal(caps.status,200,caps.data);service.capabilities=caps.json;const session=await open(service,{readonly:true});const result=await execute(service,session,'SELECT COUNT(*) FROM EVENTS WHERE ID = ?',[v('integer','93')]);assert.equal(final(result).state,'succeeded',result.data);assert.equal(result.frames.find(x=>x.kind==='row').values[0].value,'1');assert.equal((await close(service,session)).json.state,'closed');child.kill();const code=await service.exit;assert.ok(service.closed);assert.ok(code!==undefined,'production process actually terminated');
});

async function listCatalog(service,session,options={}) {
  const body=bound(session);
  return {body,...await request(service,'POST',`/v1/sessions/${session.binding.session_id}/catalog`,body,{bearer:session.bearer,...options})};
}
function knownCatalog(response,session,expected) {
  assert.equal(response.status,200,response.data);
  assert.equal(response.json.protocol,PROTOCOL);
  assert.equal(response.json.request_id,response.body.request_id);
  assert.deepEqual(response.json.binding,session.binding,'directory carries the complete physical session binding');
  assert.equal(response.json.completeness,'known');
  assert.deepEqual(response.json.tables.slice().sort((a,b)=>a.name<b.name?-1:a.name>b.name?1:0),expected.slice().sort((a,b)=>a.name<b.name?-1:a.name>b.name?1:0));
}
function noCatalog(response) {
  assert.equal(response.json.tables,undefined,'an unauthorized, incomplete or failed directory is not a known empty collection');
  assert.equal(response.json.completeness,undefined);
  assert.equal(response.json.snapshot,undefined);
  assert.ok(!response.data.includes('目录_%_秘密')&&!response.data.includes('HIDDEN_NAMESPACE'),'foreign directory names are not leaked');
}

test('real HTTPS catalog preserves exact Unicode/percent/underscore identities and only the authorized physical namespace',{timeout:60000},async t=>{
  if(!available){t.skip('real Java/H2/HTTPS dependencies missing: catalog NOT VERIFIED');return;}
  const service=await start();
  try {
    const session=await open(service,{readonly:true});
    knownCatalog(await listCatalog(service,session),session,[
      {schema:'PUBLIC',name:'EVENTS',kind:'table'},
      {schema:'PUBLIC',name:'WORK',kind:'table'},
      {schema:'PUBLIC',name:'目录_%_中文',kind:'table'},
      {schema:'PUBLIC',name:'目录_AX_中文',kind:'table'},
      {schema:'PUBLIC',name:'目录_%_视图',kind:'view'},
    ]);
    const literalNamespace=await open(service,{namespace:'AUTH_%',readonly:true});
    knownCatalog(await listCatalog(service,literalNamespace),literalNamespace,[{schema:'AUTH_%',name:'可见_%_中文',kind:'table'}]);
    const empty=await open(service,{namespace:'EMPTY_CATALOG',readonly:true});
    knownCatalog(await listCatalog(service,empty),empty,[],'a successfully queried real empty namespace is distinct from unknown');
    for(const owned of [session,literalNamespace,empty])assert.equal((await close(service,owned)).json.state,'closed');
  } finally {await service.close();}
});

test('real HTTPS catalog rejects foreign owner/full binding/namespace/principal/alias before JDBC without directory disclosure',{timeout:60000},async t=>{
  if(!available){t.skip('real Java/H2/HTTPS dependencies missing: catalog NOT VERIFIED');return;}
  const service=await start();
  try {
    const session=await open(service),other=await open(service,{namespace:'OTHER'}),otherAlias=await open(service,{alias:'ack_lost'});
    const before=await service.inspect(),route=`/v1/sessions/${session.binding.session_id}/catalog`;
    for(const [label,binding] of [
      ['foreign owner',{...session.binding,owner_id:randomUUID()}],
      ['altered namespace',{...session.binding,namespace:'OTHER'}],
      ['foreign physical session',{...session.binding,session_generation:randomUUID()}],
      ['other alias target',{...session.binding,target_id:otherAlias.binding.target_id,target_generation:otherAlias.binding.target_generation}],
    ]) {
      const result=await request(service,'POST',route,{...bound(session),binding});
      assert.equal(result.status,403,`${label}: ${result.data}`);assert.equal(result.json.code,'FORBIDDEN');noCatalog(result);
    }
    const foreignPrincipal=await listCatalog(service,session,{bearer:bobToken});
    assert.equal(foreignPrincipal.status,403);noCatalog(foreignPrincipal);
    const unauthorizedAlias=await listCatalog(service,otherAlias,{bearer:catalogScopeToken});
    assert.equal(unauthorizedAlias.status,403,'even the same principal must hold this physical target alias');noCatalog(unauthorizedAlias);
    const unauthorizedNamespace=await listCatalog(service,other,{bearer:catalogScopeToken});
    assert.equal(unauthorizedNamespace.status,403,'even a valid main alias does not grant OTHER namespace');noCatalog(unauthorizedNamespace);
    const injected=await request(service,'POST',route,{...bound(session),namespace:'OTHER',target_alias:'ack_lost'});
    assert.equal(injected.status,400,'catalog accepts the exact BoundRequest, not caller-supplied scope overrides');noCatalog(injected);
    assert.equal((await service.inspect()).catalog_calls,before.catalog_calls,'all scope rejections happened before real JDBC getTables');
    for(const owned of [session,other,otherAlias])assert.equal((await close(service,owned)).json.state,'closed');
  } finally {await service.close();}
});

test('real HTTPS catalog cannot borrow an active physical session and independent status/cancel remain reachable',{timeout:60000},async t=>{
  if(!available){t.skip('real Java/H2/HTTPS dependencies missing: catalog NOT VERIFIED');return;}
  const service=await start(),session=await open(service),control=agent();
  const body=execution(session,'SELECT COUNT(*) FROM WORK AS A, WORK AS B, WORK AS C');
  const running=request(service,'POST',`/v1/sessions/${session.binding.session_id}/executions`,body,{httpAgent:agent()});
  try {
    let status;
    for(let i=0;i<40;i++){
      status=await request(service,'GET',`/v1/executions/${body.execution_id}`,undefined,{owner:session.binding.owner_id,httpAgent:control});
      if(status.status===200&&status.json.state==='running')break;
      await new Promise(resolve=>setTimeout(resolve,10));
    }
    assert.equal(status.status,200,status.data);assert.equal(status.json.state,'running');
    const started=Date.now(),busy=await listCatalog(service,session,{httpAgent:control});
    assert.equal(busy.status,409,busy.data);assert.equal(busy.json.code,'SESSION_BUSY');noCatalog(busy);
    const cancel=await request(service,'POST',`/v1/executions/${body.execution_id}/cancel`,bound(session),{httpAgent:control});
    assert.equal(cancel.status,202,cancel.data);assert.equal(cancel.json.requested,true);
    assert.ok(Date.now()-started<5000,'catalog rejection and independent cancellation did not wait for the running SQL');
    const completed={body,...await running};
    assert.equal(final(completed).state,'cancelled',completed.data);assert.equal(final(completed).affected_rows,'0');assert.equal(final(completed).transaction,'rolled_back');
    assert.equal((await listCatalog(service,session)).status,200,'the acknowledged query drain restores owned catalog access');
    assert.equal((await close(service,session)).json.state,'closed');
  } finally {
    await request(service,'POST',`/v1/executions/${body.execution_id}/cancel`,bound(session),{httpAgent:control}).catch(()=>{});
    await running.catch(()=>{});
    await service.close();
  }
});

test('real HTTPS catalog returns all 512 real H2 objects and refuses 513 without an empty or truncated result',{timeout:60000},async t=>{
  if(!available){t.skip('real Java/H2/HTTPS dependencies missing: catalog NOT VERIFIED');return;}
  const service=await start();
  try {
    const atLimit=await open(service,{namespace:'QUOTA_512',readonly:true});
    const expected=Array.from({length:512},(_,i)=>({schema:'QUOTA_512',name:`T${String(i).padStart(3,'0')}`,kind:'table'}));
    knownCatalog(await listCatalog(service,atLimit),atLimit,expected);
    const excessive=await open(service,{namespace:'QUOTA_513',readonly:true}),before=await service.inspect();
    const rejected=await listCatalog(service,excessive);
    assert.equal(rejected.status,429,rejected.data);assert.equal(rejected.json.code,'QUOTA_EXCEEDED');noCatalog(rejected);
    assert.equal((await service.inspect()).catalog_calls,before.catalog_calls+1,'the quota was checked while consuming the real 513-object JDBC catalog');
    assert.equal(final(await execute(service,excessive,'SELECT 1')).state,'succeeded','a refused directory does not impersonate a broken SQL session');
    for(const owned of [atLimit,excessive])assert.equal((await close(service,owned)).json.state,'closed');
  } finally {await service.close();}
});

test('real HTTPS JDBC catalog rejection maps to metadata unknown rather than a fabricated known empty directory',{timeout:60000},async t=>{
  if(!available){t.skip('real Java/H2/HTTPS dependencies missing: catalog NOT VERIFIED');return;}
  const service=await start();
  try {
    const denied=await open(service,{alias:'catalog_denied',readonly:true}),before=await service.inspect();
    const response=await listCatalog(service,denied);
    assert.equal(response.status,400,response.data);assert.equal(response.json.code,'METADATA_UNKNOWN');assert.equal(response.json.effects,'none');noCatalog(response);
    assert.ok(!response.data.includes('TEST actual JDBC')&&!response.data.includes('catalog-denial-secret-marker'),'internal JDBC failure details are withheld');
    const inspected=await service.inspect();
    assert.equal(inspected.catalog_calls,before.catalog_calls+1,'the test reached real H2 DatabaseMetaData.getTables, not a fake HTTP response');
    assert.equal(inspected.catalog_rejections,before.catalog_rejections+1,'the dedicated JDBC fault was actually injected after the real result was closed');
    assert.equal(final(await execute(service,denied,'SELECT 1')).state,'succeeded');
    assert.equal((await close(service,denied)).json.state,'closed');
  } finally {await service.close();}
});

