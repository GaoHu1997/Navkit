// Current production desktop acceptance: original downloads through real WebView2/Tauri IPC, with dbx provenance explicit.
// No preview mocks, fixture HTTP servers, staged drivers or substituted command handlers.
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import net from 'node:net';
import {spawn,spawnSync} from 'node:child_process';
import {createHash,randomUUID} from 'node:crypto';
import assert from 'node:assert/strict';
import {chromium} from 'playwright-core';
const digest=file=>createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const exe=path.resolve(process.env.NAVKIT_E2E_EXE || 'src-tauri/target/debug/navkit.exe');
assert.ok(fs.statSync(exe).isFile(),'Build the custom-protocol desktop executable first');
const evidence=path.resolve(process.env.NAVKIT_E2E_EVIDENCE || 'tools/acceptance/simulation/desktop-final-verified-20261107');
const registryFile=path.resolve('tools/acceptance/simulation/driver-registry-final-20261107.json');
const registry=JSON.parse(fs.readFileSync(registryFile,'utf8'));
assert.equal(registry.products.length,29);
const labels={trino:'Trino',prestosql:'Presto',databricks:'Databricks',db2:'DB2',informix:'Informix',saphana:'SAP HANA',teradata:'Teradata',vertica:'Vertica',exasol:'Exasol',dameng:'达梦',firebird:'Firebird',oracle:'Oracle',hive:'Hive',kylin:'Kylin',snowflake:'Snowflake',dremio:'Dremio',ignite:'Ignite',access:'Access',yashandb:'崖山',xugu:'虚谷',iris:'IRIS',bigquery:'BigQuery',gbase:'GBase',inceptor:'Inceptor',spanner:'Cloud Spanner',cache:'Caché',sundb:'SUNDB',oscar:'OSCAR',argodb:'ArgoDB'};
const defaultStatuses=items=>{const result=new Map();for(const item of items)if(!result.has(item.kind))result.set(item.kind,item);return result;};
assert.equal(fs.existsSync(evidence),false,'Each final acceptance run uses a new evidence directory');
fs.mkdirSync(evidence,{recursive:true});
const runtime=fs.mkdtempSync(path.join(os.tmpdir(),'navkit-datasource-desktop-'));
const application=path.join(runtime,`navkit-datasource-e2e-${randomUUID()}.exe`);
fs.copyFileSync(exe,application);
const report={started_at:new Date().toISOString(),executable:exe,sha256:digest(exe),checks:[],page_errors:[],console_errors:[],ipc_requests:[],runtime};
let child,browser,page;
const downloadCounts=Object.create(null);
const log=fs.openSync(path.join(evidence,'application.log'),'w');
const check=(name,details={})=>{report.checks.push({name,status:'PASS',...details});console.log(`PASS ${name}`);};
const pause=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function port(){return new Promise((resolve,reject)=>{const server=net.createServer();server.on('error',reject);server.listen(0,'127.0.0.1',()=>{const p=server.address().port;server.close(()=>resolve(p));});});}
async function screenshot(name){await page.screenshot({path:path.join(evidence,`${name}.png`),fullPage:true});}
const request=(kind,values={})=>({kind,host:'127.0.0.1',port:0,user:'',password:'',database:'',file:'',params:'',timeout_secs:15,...values});
const invoke=(command,args={})=>page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
async function openPicker(){
  if(await page.locator('.conn-picker-search').count())return;
  await page.locator('.ribbon-btn').filter({has:page.locator('.ribbon-label',{hasText:/^连接$/})}).first().click();
  await page.locator('.conn-picker-search').waitFor();
}
async function directDownload(kind,label){
  await openPicker();
  await page.locator('.conn-picker-search').fill(label);
  const select=page.locator(`button[data-select-kind="${kind}"]`);
  const action=page.locator(`button[data-driver-kind="${kind}"]`);
  await select.waitFor();
  await page.waitForFunction(kind=>{
    const button=document.querySelector(`button[data-driver-kind="${kind}"]`);
    return button && !button.disabled && button.getAttribute('aria-disabled')!=='true' && button.dataset.mode==='download' && /下载驱动/.test(button.getAttribute('aria-label') || '');
  },kind,{timeout:30000});
  const thirdParty=['cache','sundb','oscar'].includes(kind);
  assert.match(await action.getAttribute('aria-label'),thirdParty?/dbx.*第三方/:/官方源/);
  const selectedBefore=await select.getAttribute('aria-pressed');
  assert.equal(await action.locator('button').count(),0);
  assert.equal(await action.evaluate(node=>!!node.parentElement?.closest('button')),false,'Download and selection buttons must be siblings');
  const totalBefore=downloadCounts[kind] || 0;
  await screenshot(`${kind}-before-download`);
  let status,feedback,completed=false,attemptsUsed=0;
  for(let attempt=1;attempt<=2;attempt++){
    attemptsUsed=attempt;
    const before=downloadCounts[kind] || 0;
    const completion=page.waitForResponse(response=>{
      const url=new URL(response.url());
      return url.hostname==='ipc.localhost' && decodeURIComponent(url.pathname)==='/download_database_driver';
    },{timeout:210000});
    await action.click();
    const response=await completion;
    await response.finished();
    await page.waitForFunction(kind=>{
      const button=document.querySelector(`button[data-driver-kind="${kind}"]`);
      return !button || (button.dataset.mode!=='busy' && button.getAttribute('aria-disabled')!=='true');
    },kind,{timeout:30000});
    assert.equal((downloadCounts[kind] || 0)-before,1,'Each click must cause exactly one real IPC');
    assert.equal(await select.getAttribute('aria-pressed'),selectedBefore);
    status=defaultStatuses(await invoke('list_driver_statuses')).get(kind);
    feedback=await page.locator('.conn-driver-feedback').textContent();
    completed=status?.status==='ready';
    (report.download_attempts ??= []).push({kind,attempt,completed,status:status?.status,feedback});
    if(completed)break;
    report.download_failure={kind,attempt,status,feedback};
    await screenshot(`${kind}-failed-download-${attempt}`);
    const transportFailure=/timed out|error sending request|client error \(Connect\)|connection reset|connection.*closed|连接失败|超时|超过\s*180\s*秒|deadline has elapsed/i.test(feedback || '');
    if(attempt===2 || !transportFailure)break;
    assert.equal(await action.getAttribute('aria-disabled'),'false');
    assert.equal(await action.evaluate(node=>document.activeElement===node),true);
  }
  assert.equal(completed,true,JSON.stringify({status,feedback}));
  assert.ok(status.library && fs.statSync(status.library).isFile());
  assert.equal((downloadCounts[kind] || 0)-totalBefore,attemptsUsed);
  assert.equal(await select.getAttribute('aria-pressed'),selectedBefore);
  assert.equal(await action.count(),0,'Verified ready driver removes the missing-driver action');
  assert.equal(await select.evaluate(node=>document.activeElement===node && !node.disabled),true,'Focus returns to the exact available source');
  await screenshot(`${kind}-after-download`);
  check(`${label}: original card download, complete driver, selection and focus`,{kind,version:status.version || null,driver_status:status.status,architecture:status.architecture,attempts:attemptsUsed,source_type:thirdParty?'DBX_THIRD_PARTY_NOT_VENDOR':'OFFICIAL_VENDOR'});
  return status;
}
async function directJdbc(product){
  const {kind,version}=product;
  const label=labels[kind];
  const status=await directDownload(kind,label);
  assert.equal(status.version,version,'Card installs the recommended exact release');
  assert.ok(status.library.endsWith('.jar'));
  assert.equal(digest(status.library),product.jar.sha256);
  const directory=path.dirname(status.library);
  for(const asset of product.assets){
    const name=asset.name ?? (asset.role==='jar'?'driver.jar':asset.role==='pom'?'official.pom':'LICENSE');
    const file=path.join(directory,name);
    assert.equal(fs.statSync(file).size,asset.bytes);
    assert.equal(digest(file),asset.sha256);
  }
  for(const asset of product.dependencies){
    const file=path.join(directory,asset.name);
    assert.equal(fs.statSync(file).size,asset.bytes);
    assert.equal(digest(file),asset.sha256,'Mandatory runtime dependencies retain exact official bytes');
  }
  if(kind==='access'){
    const file=path.join(runtime,"真实 Access O'Reilly.accdb");
    await saveAndQuery('access',request('access',{file,params:'newDatabaseVersion=V2010'}),'PUBLIC','NavkitAccessAcceptance');
    assert.ok(fs.statSync(file).size>0,'The real Access file must exist after ordinary connection close');
    await page.locator('button[data-select-kind="access"]').dblclick();
    const dialog=page.locator('.dialog').filter({has:page.locator('.dialog-title-text',{hasText:/^新建连接 \(Microsoft Access\)$/})});
    await dialog.waitFor();await dialog.locator('input[aria-label="数据库文件"]').fill(file);
    await dialog.locator('.dialog-footer button').filter({hasText:/^测试连接$/}).click();
    await dialog.getByRole('status').filter({hasText:'连接成功'}).waitFor({timeout:30000});
    await screenshot('access-real-file-connection-form');
    check('Access: original complete JAR, real local file SQL and new-connection form',{kind,version,jar_sha256:product.jar.sha256,file_status:'REAL_LOCAL_FILE_PASS',dependencies:product.dependencies.length});
    await dialog.locator('.dialog-footer button').filter({hasText:/^取消$/}).click();
    return;
  }
  const values={host:'127.0.0.1',port:1,user:'navkit_acceptance',password:'isolated_test_password',database:'navkit',timeout_secs:3};
  if(kind==='informix')values.params='informixserver=navkit_probe';
  if(kind==='exasol' || kind==='dremio')values.database='';
  if(kind==='ignite')values.database='PUBLIC';
  if(kind==='databricks'){values.user='token';values.params='httpPath=/sql/1.0/warehouses/navkit-probe;ssl=true';}
  if(kind==='bigquery'){
    values.user='token';values.password='LOCAL_DESKTOP_BEARER_NOT_A_GOOGLE_CREDENTIAL.123-abc';
    values.database='navkit-loopback-probe/probe_dataset';values.params='ssl=false';
  }
  if(kind==='spanner'){
    values.user='';values.password='';values.database='navkit-project/probe-instance/probe-db';values.params='ssl=false';
  }
  if(kind==='argodb'){values.user='';values.password='';values.database='default';}
  if(kind==='snowflake'){
    // Never send dummy credentials to a real account DNS. This rejection precedes networking.
    values.params='ssl=false';
    const result=await invoke('test_connection',{req:request(kind,values)});
    assert.equal(result.ok,false);assert.match(result.message,/TLS|SSL/i);
    assert.doesNotMatch(result.message,/isolated_test_password|NoClassDefFoundError|ClassNotFoundException/);
  }else{
    const result=await invoke('test_connection',{req:request(kind,values)});
    assert.equal(result.ok,false,'A closed loopback endpoint cannot become a connected server');
    assert.match(result.message,/JDBC/i,'Must reach the actual default JDBC route');
    assert.doesNotMatch(result.message,/ODBC|NoClassDefFoundError|ClassNotFoundException|isolated_test_password|LOCAL_DESKTOP_BEARER_NOT_A_GOOGLE_CREDENTIAL|驱动.*不存在|找不到.*驱动/i);
  }
  check(`${label}: exact JAR/dependencies and default-route negative check`,{kind,version,jar_sha256:product.jar.sha256,server_status:'UNVERIFIED',dependencies:product.dependencies.length});
  if(kind==='bigquery'){
    await page.locator('button[data-select-kind="bigquery"]').dblclick();
    const dialog=page.locator('.dialog').filter({has:page.locator('.dialog-title-text',{hasText:/^新建连接 \(BigQuery\)$/})});
    await dialog.waitFor();
    assert.equal(await dialog.locator('input[aria-label="主机 / 地址"]').inputValue(),'bigquery.googleapis.com');
    assert.equal(await dialog.locator('input[aria-label="用户名"]').inputValue(),'token');
    assert.equal(await dialog.locator('input[aria-label="OAuth 访问令牌"]').getAttribute('type'),'password');
    assert.equal(await dialog.locator('input[aria-label="项目 / 数据集"]').getAttribute('placeholder'),'project 或 project/dataset');
    await screenshot('bigquery-default-secure-connection-form');
    check('BigQuery: official package, secure OAuth form and real lazy-query refusal',{kind,version,server_status:'UNVERIFIED',tls_status:'UNVERIFIED',cloud_credentials_used:false});
    await dialog.locator('.dialog-footer button').filter({hasText:/^取消$/}).click();
  }
  if(kind==='spanner'){
    await page.locator('button[data-select-kind="spanner"]').dblclick();
    const dialog=page.locator('.dialog').filter({has:page.locator('.dialog-title-text',{hasText:/^新建连接 \(Cloud Spanner\)$/})});
    await dialog.waitFor();
    assert.equal(await dialog.locator('input[aria-label="主机 / 地址"]').inputValue(),'spanner.googleapis.com');
    assert.equal(await dialog.locator('input[aria-label="用户名"]').inputValue(),'token');
    const token=dialog.locator('input[aria-label="OAuth 访问令牌"]');
    assert.equal(await token.getAttribute('type'),'password');
    assert.ok((await token.getAttribute('aria-describedby')).trim().split(/\s+/).length>=2);
    assert.equal(await dialog.locator('input[aria-label="项目 / 实例 / 库"]').getAttribute('placeholder'),'project/instance/database');
    await screenshot('spanner-default-secure-connection-form');
    check('Spanner: complete original package, secure OAuth form and explicit credential-free loopback route',{kind,version,server_status:'UNVERIFIED',tls_status:'UNVERIFIED',cloud_credentials_used:false});
    await dialog.locator('.dialog-footer button').filter({hasText:/^取消$/}).click();
  }
  if(kind==='argodb'){
    await page.locator('button[data-select-kind="argodb"]').dblclick();
    const dialog=page.locator('.dialog').filter({has:page.locator('.dialog-title-text',{hasText:/^新建连接 \(ArgoDB\)$/})});
    await dialog.waitFor();
    assert.equal(await dialog.locator('input[aria-label="用户名"]').inputValue(),'');
    assert.equal(await dialog.locator('input[aria-label="密码"]').inputValue(),'');
    assert.equal(await dialog.locator('input[aria-label="端口"]').inputValue(),'10000');
    const params=dialog.locator('textarea[aria-label="附加参数"]');
    assert.match(await params.getAttribute('placeholder'),/noSasl/);
    assert.ok((await params.getAttribute('aria-describedby')).trim().split(/\s+/).length>=2);
    await screenshot('argodb-exact-quark-connection-form');
    check('ArgoDB: exact Quark original, explicit noSasl credentials and accessible form',{kind,version,server_status:'UNVERIFIED',tls_status:'UNVERIFIED'});
    await dialog.locator('.dialog-footer button').filter({hasText:/^取消$/}).click();
  }
  if(kind==='hive'){
    const states=defaultStatuses(await invoke('list_driver_statuses'));
    for(const alias of ['kyuubi','impala','spark']){
      const state=states.get(alias);
      assert.equal(state?.status,'ready');assert.equal(state.library,status.library);assert.equal(state.version,version);
    }
    check('Kyuubi/Impala/Spark HiveServer2-compatible endpoints share the original complete package',{compatible_kinds:['kyuubi','impala','spark'],server_status:'UNVERIFIED'});
  }
}
async function builtinRestForm(kind,label){
  await openPicker();await page.locator('.conn-picker-search').fill(label);
  const select=page.locator(`button[data-select-kind="${kind}"]`);await select.waitFor();
  assert.equal(await page.locator(`button[data-driver-kind="${kind}"]`).count(),0,'Built-in REST requires no external driver download');
  const state=defaultStatuses(await invoke('list_driver_statuses')).get(kind);assert.equal(state?.status,'builtin');
  await select.dblclick();
  const dialog=page.locator('.dialog').filter({has:page.locator('.dialog-title-text',{hasText:new RegExp(`^新建连接 \\(${label}\\)$`)})});await dialog.waitFor();
  const host=dialog.locator('input[aria-label="主机 / 地址"]'),user=dialog.locator('input[aria-label="用户名"]'),params=dialog.locator('textarea[aria-label="附加参数"]');
  assert.ok((await host.getAttribute('aria-describedby')).trim().split(/\s+/).length>=2);
  assert.ok((await params.getAttribute('aria-describedby')).trim().split(/\s+/).length>=2);
  if(kind==='salesforce'){
    assert.equal(await host.getAttribute('placeholder'),'your-org.my.salesforce.com');assert.equal(await user.inputValue(),'token');
    assert.equal(await dialog.locator('input[aria-label="OAuth 访问令牌"]').getAttribute('type'),'password');
    assert.equal(await dialog.locator('input[aria-label="REST API 版本"]').getAttribute('placeholder'),'API 版本（默认 v59.0）');
    assert.equal(await dialog.locator('input[aria-label="端口"]').inputValue(),'443');
  }else{
    assert.equal(await dialog.locator('input[aria-label="端口"]').inputValue(),'8080');assert.equal(await user.inputValue(),'');
    assert.equal(await dialog.locator('input[aria-label="Namespace"]').getAttribute('placeholder'),'namespace（默认 default）');
  }
  const result=await invoke('test_connection',{req:request(kind,{host:'127.0.0.1',port:1,user:kind==='salesforce'?'token':'',password:kind==='salesforce'?'DESKTOP_LOOPBACK_TOKEN.123':'',params:'ssl=false',timeout_secs:3})});
  assert.equal(result.ok,false);assert.match(result.message,/REST/);assert.doesNotMatch(result.message,/ODBC|JDBC|DESKTOP_LOOPBACK_TOKEN/);
  await screenshot(`${kind}-builtin-rest-connection-form`);
  check(`${label}: built-in REST card, accessible form and actual native transport refusal`,{kind,driver_status:'builtin',adapter:'NATIVE_REST',server_status:'UNVERIFIED',download_required:false});
  await dialog.locator('.dialog-footer button').filter({hasText:/^取消$/}).click();
}
async function saveAndQuery(kind,req,container,table){
  const id=randomUUID();
  const config={id,name:`desktop-acceptance-${kind}`,kind,group:'',host:req.host,port:req.port,user:req.user,password:'',database:req.database,file:req.file,params:req.params,timeout_secs:req.timeout_secs,color:'',read_only:false};
  await invoke('save_connection',{config,password:req.password});
  const opened=await invoke('open_connection',{id});assert.equal(opened.ok,true,JSON.stringify(opened));
  const value=`desktop-${randomUUID()}-中文;O'Reilly\\尾`;
  const literal=`'${value.replaceAll("'","''")}'`;
  const sql=text=>invoke('execute_sql',{id,sql:text,database:container});
  try{
    await sql(`CREATE TABLE ${table} (id INTEGER, payload VARCHAR(200), nullable_value VARCHAR(20))`);
    await sql(`INSERT INTO ${table} VALUES (1,${literal},NULL)`);
    const containers=await invoke('list_databases',{id});assert.ok(containers.some(item=>item[0]===container),JSON.stringify(containers));
    const tables=await invoke('list_tables',{id,schemaName:container});assert.ok(tables.some(item=>item[0]===table),JSON.stringify(tables));
    const result=await sql(`SELECT * FROM ${table}`);
    assert.ok(result.sets.some(set=>set.rows.some(row=>row[1]===value && row[2]===null)),JSON.stringify(result));
    assert.equal(await sql('SELECT * FROM NAVKIT_TABLE_THAT_DOES_NOT_EXIST').then(()=>false,()=>true),true);
    check(`${kind}: production IPC open, metadata, Unicode/quotes/semicolon/NULL and invalid SQL`);
  }finally{
    await sql(kind==='access'?`DROP TABLE ${table}`:`DROP TABLE IF EXISTS ${table}`);
    await invoke('close_connection',{id});await invoke('delete_connection',{id});
  }
}
try{
  const cdpPort=await port();
  const environment={...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${cdpPort}`,WEBVIEW2_USER_DATA_FOLDER:path.join(runtime,'data','cache','webview')};
  delete environment.NAVKIT_H2_JAR;delete environment.NAVKIT_ODBC_DRIVER_FILE;
  child=spawn(application,[],{cwd:runtime,env:environment,stdio:['ignore',log,log]});report.application_pid=child.pid;
  child.once('exit',(code,signal)=>{report.application_exit={code,signal,at:new Date().toISOString(),phase:report.cleanup_phase?'cleanup':'test'};});
  let ready=false;
  for(let attempt=0;attempt<120;attempt++){
    if(child.exitCode!==null)throw new Error(`Desktop exited before CDP: ${child.exitCode}`);
    try{const response=await fetch(`http://127.0.0.1:${cdpPort}/json/version`,{signal:AbortSignal.timeout(2000)});if(response.ok){ready=true;break;}}catch{}
    await pause(250);
  }
  assert.ok(ready,'Production WebView2 CDP did not start');
  browser=await chromium.connectOverCDP(`http://127.0.0.1:${cdpPort}`);page=browser.contexts()[0].pages()[0];
  page.on('close',()=>{report.page_closed={at:new Date().toISOString(),phase:report.cleanup_phase?'cleanup':'test'};});
  browser.on('disconnected',()=>{report.browser_disconnected={at:new Date().toISOString(),phase:report.cleanup_phase?'cleanup':'test'};});
  page.on('request',request=>{
    let url;try{url=new URL(request.url());}catch{return;}
    if(url.hostname!=='ipc.localhost')return;
    const command=decodeURIComponent(url.pathname.replace(/^\/+/,''));
    let args={};try{args=JSON.parse(request.postData() || '{}');}catch{}
    const kind=args.kind ?? args.args?.kind ?? args.payload?.kind;
    report.ipc_requests.push({command,...(typeof kind==='string'?{kind}:{})});
    if(command==='download_database_driver' && typeof kind==='string')downloadCounts[kind]=(downloadCounts[kind] || 0)+1;
  });
  page.on('pageerror',error=>report.page_errors.push(String(error)));
  page.on('console',message=>{if(message.type()==='error')report.console_errors.push(message.text());});
  await page.waitForFunction(()=>window.__TAURI_INTERNALS__?.invoke && document.querySelector('.ribbon-btn'),{},{timeout:30000});
  assert.equal(await page.evaluate(()=>!!window.__PREVIEW_MOCK__),false);
  assert.ok(!page.url().includes(':5183') && !page.url().includes(':5173'),page.url());report.url=page.url();
  check('Fresh isolated executable uses embedded frontend and real Tauri IPC');
  await builtinRestForm('salesforce','Salesforce');await builtinRestForm('hbase','Apache HBase');
  const h2=await directDownload('h2','H2');
  assert.equal(digest(h2.library),registry.standalone.find(p=>p.kind==='h2').sha256);
  await page.locator('button[data-select-kind="h2"]').dblclick();
  const dialog=page.locator('.dialog').filter({has:page.locator('.dialog-title-text',{hasText:/^新建连接 \(H2\)$/})});
  await dialog.waitFor();
  const buttons=dialog.locator('.dialog-footer button');await buttons.filter({hasText:/^测试连接$/}).click();
  await dialog.getByRole('status').filter({hasText:'连接成功'}).waitFor({timeout:30000});
  await screenshot('h2-real-jdbc-connection-form');
  check('H2: new-connection form defaults to real memory JDBC and connects');
  await buttons.filter({hasText:/^取消$/}).click();
  await saveAndQuery('h2',request('h2',{user:'sa'}),'PUBLIC','NAVKIT_DESKTOP_PROBE');
  await directDownload('duckdb','DuckDB');
  await saveAndQuery('duckdb',request('duckdb',{file:path.join(runtime,'desktop-probe.duckdb')}),'main','navkit_desktop_probe');
  // Order only: preserve every package and every final assertion in the complete run.
  const requestedOrder=(process.env.NAVKIT_E2E_JDBC_ORDER || '').split(',').filter(Boolean);
  assert.equal(new Set(requestedOrder).size,requestedOrder.length);
  for(const kind of requestedOrder)assert.ok(registry.products.some(product=>product.kind===kind),kind);
  const orderedProducts=[...requestedOrder.map(kind=>registry.products.find(product=>product.kind===kind)),...registry.products.filter(product=>!requestedOrder.includes(product.kind))];
  assert.equal(orderedProducts.length,registry.products.length);
  for(const product of orderedProducts)await directJdbc(product);
  const finalStates=defaultStatuses(await invoke('list_driver_statuses'));
  report.driver_kinds=['h2','duckdb',...registry.products.flatMap(p=>p.compatible_kinds)];
  for(const kind of report.driver_kinds)assert.equal(finalStates.get(kind)?.status,'ready',kind);
  assert.equal(new Set(report.driver_kinds).size,34);report.download_packages=31;
  report.registry={path:registryFile,sha256:digest(registryFile)};
  assert.equal(report.page_errors.length,0,JSON.stringify(report.page_errors));assert.equal(report.console_errors.length,0,JSON.stringify(report.console_errors));report.status='PASS';
}catch(error){
  report.status='FAIL';report.error=error?.stack || String(error);console.error(report.error);
  if(page)await screenshot('failure').catch(()=>{});process.exitCode=1;
}finally{
  report.cleanup_phase=true;
  if(browser)await browser.close().catch(()=>{});
  if(child?.pid && child.exitCode===null){
    const killed=spawnSync(path.join(process.env.SystemRoot || 'C:\\Windows','System32','taskkill.exe'),['/PID',String(child.pid),'/T','/F'],{stdio:'ignore'});
    report.taskkill={pid:child.pid,status:killed.status,error:killed.error?.message};
    for(let attempt=0;attempt<50 && child.exitCode===null && child.signalCode===null;attempt++)await pause(100);
  }
  fs.closeSync(log);
  assert.equal(path.dirname(runtime),path.resolve(os.tmpdir()));assert.ok(path.basename(runtime).startsWith('navkit-datasource-desktop-'));
  await pause(1000);
  try{fs.rmSync(runtime,{recursive:true,force:true,maxRetries:50,retryDelay:100});report.runtime_removed=true;}
  catch(error){report.runtime_removed=false;report.cleanup_error=error?.stack || String(error);report.status='FAIL';process.exitCode=1;}
  report.finished_at=new Date().toISOString();fs.writeFileSync(path.join(evidence,'report.json'),JSON.stringify(report,null,2));
}
