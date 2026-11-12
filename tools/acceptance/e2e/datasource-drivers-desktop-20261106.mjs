// Production desktop/CDP acceptance. No preview mocks, fixture HTTP servers or driver stubs.
// Build first: cargo build --manifest-path src-tauri/Cargo.toml --features custom-protocol
// NAVKIT_E2E_EXE must identify that newly built executable; NAVKIT_JAVA can identify Java 11+.
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import assert from 'node:assert/strict';
import { chromium } from 'playwright-core';

const exe = path.resolve(process.env.NAVKIT_E2E_EXE || 'src-tauri/target/debug/navkit.exe');
assert.ok(fs.statSync(exe).isFile(), 'Build the custom-protocol desktop executable first');
const evidence = path.resolve(process.env.NAVKIT_E2E_EVIDENCE || 'tools/acceptance/datasources/desktop-evidence');
fs.mkdirSync(evidence, { recursive:true });
const runtime = fs.mkdtempSync(path.join(os.tmpdir(), 'navkit-datasource-desktop-'));
// Keep identical production bytes but a unique process name for parallel test isolation.
const application = path.join(runtime, `navkit-datasource-e2e-${randomUUID()}.exe`);
fs.copyFileSync(exe, application);
const report = { started_at:new Date().toISOString(), executable:exe, sha256:createHash('sha256').update(fs.readFileSync(exe)).digest('hex'), checks:[], page_errors:[], console_errors:[] };
let child, browser, page;
const downloadCounts = Object.create(null);
report.ipc_requests = [];
report.runtime = runtime;
const log = fs.openSync(path.join(evidence,'application.log'),'w');
const check = (name, details={}) => { report.checks.push({name,status:'PASS',...details}); console.log(`PASS ${name}`); };
const pause = ms => new Promise(resolve=>setTimeout(resolve,ms));
async function port() {
  return new Promise((resolve,reject)=>{ const server=net.createServer();server.on('error',reject);server.listen(0,'127.0.0.1',()=>{ const p=server.address().port;server.close(()=>resolve(p)); }); });
}
async function screenshot(name) { await page.screenshot({path:path.join(evidence,`${name}.png`),fullPage:true}); }
const request = (kind, values={}) => ({kind,host:'127.0.0.1',port:0,user:'',password:'',database:'',file:'',params:'',timeout_secs:15,...values});
const invoke = (command,args={}) => page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
async function openPicker() {
  if (await page.locator('.conn-picker-search').count()) return;
  await page.locator('.ribbon-btn').filter({has:page.locator('.ribbon-label',{hasText:/^连接$/})}).first().click();
  await page.locator('.conn-picker-search').waitFor();
}
async function directDownload(kind, label, expectedStatus='ready') {
  await openPicker();
  await page.locator('.conn-picker-search').fill(label);
  const select = page.locator(`button[data-select-kind="${kind}"]`);
  const action = page.locator(`button[data-driver-kind="${kind}"]`);
  await select.waitFor();
  await page.waitForFunction(kind=>{
    const button=document.querySelector(`button[data-driver-kind="${kind}"]`);
    return button && !button.disabled && button.getAttribute('aria-disabled')!=='true' && button.dataset.mode==='download' && /下载驱动/.test(button.getAttribute('aria-label') || '');
  },kind,{timeout:30000});
  const selectedBefore = await select.getAttribute('aria-pressed');
  assert.equal(await action.locator('button').count(),0,'Download must not contain nested interactive buttons');
  assert.equal(await action.evaluate(node=>!!node.closest('button')?.parentElement?.closest('button')),false,'Download must not be nested in selection button');
  const totalBeforeCount=downloadCounts[kind] || 0;
  await screenshot(`${kind}-before-download`);
  let status, feedback, completed=false, attemptsUsed=0;
  // Retry only a real transport failure, through the same enabled UI action.
  // Each attempt remains exactly one observed IPC; checksum/runtime failures are never hidden.
  for (let attempt=1; attempt<=2; attempt++) {
    attemptsUsed=attempt;
    const beforeCount=downloadCounts[kind] || 0;
    const completion=page.waitForResponse(response=>{
      const url=new URL(response.url());
      return url.hostname==='ipc.localhost' && decodeURIComponent(url.pathname)==='/download_database_driver';
    },{timeout:210000});
    await action.click();
    const downloadResponse=await completion;
    await downloadResponse.finished();
    await page.waitForFunction(kind=>{
      const button=document.querySelector(`button[data-driver-kind="${kind}"]`);
      return !button || (button.dataset.mode!=='busy' && button.getAttribute('aria-disabled')!=='true');
    },kind,{timeout:30000});
    assert.equal((downloadCounts[kind] || 0)-beforeCount,1,'Each card click must perform exactly one real download IPC');
    assert.equal(await select.getAttribute('aria-pressed'),selectedBefore,'A failed or successful download must not change selection');
    status=(await invoke('list_driver_statuses')).find(item=>item.kind===kind);
    feedback=await page.locator('.conn-driver-feedback').textContent();
    completed=status?.status===expectedStatus && (expectedStatus!=='manual' || !!status.download_path);
    (report.download_attempts ??= []).push({kind,attempt,completed,status:status?.status,feedback});
    if(completed) break;
    report.download_failure={kind,attempt,status,feedback};
    await screenshot(`${kind}-failed-download-${attempt}`);
    const transportFailure=/timed out|error sending request|client error \(Connect\)|connection reset|连接失败|超时/i.test(feedback || '');
    if(attempt===2 || !transportFailure) break;
    assert.equal(await action.getAttribute('aria-disabled'),'false','A failed download must leave a usable retry action');
    assert.equal(await action.evaluate(node=>document.activeElement===node),true,'A failed download preserves keyboard focus before retry');
  }
  assert.equal(completed,true,JSON.stringify({status,feedback}));
  assert.equal(status?.status,expectedStatus,JSON.stringify({status,feedback}));
   if (expectedStatus==='manual') {
     assert.equal(status.library,null,'Downloaded MSI must not masquerade as a loaded ODBC DLL');
     assert.ok(status.download_path && fs.statSync(status.download_path).isFile());
     const manualHashes={snowflake:'30d0c5c095c52f5d00ddc0c3618c6fddd1fa04186268f1c09e4f6c27d213a150',firebird:'935d9f211b1f58148afd7bfcecf53d78210d12bcf21853d691ec1ecd576eaac2'};
      assert.ok(manualHashes[kind],'Every installer assertion requires its own pinned digest');
      assert.equal(createHash('sha256').update(fs.readFileSync(status.download_path)).digest('hex'),manualHashes[kind]);
   }
  const afterCount = downloadCounts[kind] || 0;
  assert.equal(afterCount-totalBeforeCount,attemptsUsed,'No download IPC may occur outside the explicitly counted card clicks');
  assert.equal(await select.getAttribute('aria-pressed'),selectedBefore,'Download must not select a different source');
  if (expectedStatus==='ready') assert.equal(await action.count(),0,'Ready driver must no longer show missing-driver action');
  // A ready driver removes its action; a manual installer keeps its trigger.
  const focusTarget = expectedStatus==='ready' ? select : action;
  const focused = await focusTarget.evaluate(node=>document.activeElement===node && !node.disabled);
  assert.equal(focused,true,'Download completion restores focus to the exact available source/trigger');
  await screenshot(`${kind}-after-download`);
  check(`${label}: card direct download, ${expectedStatus==='ready'?'complete local driver':'complete official installer, still not installed'}, selection and focus`,{kind,driver_status:status.status,architecture:status.architecture,attempts:attemptsUsed});
}
async function directJdbc(kind, label, sha256) {
  await directDownload(kind,label);
  const status=(await invoke('list_driver_statuses')).find(item=>item.kind===kind);
  assert.ok(status.library?.endsWith('.jar'),'Ready managed JDBC requires the actual validated JAR');
  assert.equal(createHash('sha256').update(fs.readFileSync(status.library)).digest('hex'),sha256);
  const result = await invoke('test_connection',{req:request(kind,{host:'127.0.0.1',port:1,user:'navkit_acceptance',database:'memory/default',timeout_secs:3})});
  assert.equal(result.ok,false,'Closed loopback port must never be accepted as a connected server');
  assert.match(result.message,/JDBC/i,'The downloaded source must reach the default JDBC route, not a missing ODBC fallback');
  assert.doesNotMatch(result.message,/ODBC|驱动.*不存在|找不到.*驱动/i);
  check(`${label}: default JDBC route rejects a real closed endpoint; server remains UNVERIFIED`,{server_status:'UNVERIFIED',jar_sha256:sha256});
}
async function saveAndQuery(kind, req, container, table) {
  const id=randomUUID();
  const config={id,name:`desktop-acceptance-${kind}`,kind,group:'',host:req.host,port:req.port,user:req.user,password:'',database:req.database,file:req.file,params:req.params,timeout_secs:req.timeout_secs,color:'',read_only:false};
  await invoke('save_connection',{config,password:req.password});
  const opened=await invoke('open_connection',{id});
  assert.equal(opened.ok,true,JSON.stringify(opened));
  const value=`desktop-${randomUUID()}-中文;O'Reilly\\尾`;
  const literal=`'${value.replaceAll("'","''")}'`;
  const sql = text=>invoke('execute_sql',{id,sql:text,database:container});
  try {
    await sql(`CREATE TABLE ${table} (id INTEGER, payload VARCHAR(200), nullable_value VARCHAR(20))`);
    await sql(`INSERT INTO ${table} VALUES (1, ${literal}, NULL)`);
    const containers=await invoke('list_databases',{id});
    assert.ok(containers.some(item=>item[0]===container),JSON.stringify(containers));
    const tables=await invoke('list_tables',{id,schemaName:container});
    assert.ok(tables.some(item=>item[0]===table),JSON.stringify(tables));
    const result=await sql(`SELECT * FROM ${table}`);
    assert.ok(result.sets.some(set=>set.rows.some(row=>row.includes(value))),JSON.stringify(result));
    const invalid=await sql(`SELECT * FROM NAVKIT_TABLE_THAT_DOES_NOT_EXIST`).then(()=>false,()=>true);
    assert.equal(invalid,true,'Invalid SQL must return an error');
    check(`${kind}: production IPC open, schema, write, Unicode/quotes/semicolon/null read and negative SQL`);
  } finally {
    await sql(`DROP TABLE IF EXISTS ${table}`);
    await invoke('close_connection',{id});
    await invoke('delete_connection',{id});
  }
}
try {
  const cdpPort=await port();
  const environment={...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${cdpPort}`,WEBVIEW2_USER_DATA_FOLDER:path.join(runtime,'data','cache','webview')};
  // Isolation must exercise card installation, not a globally supplied acceptance driver.
  delete environment.NAVKIT_H2_JAR;
  delete environment.NAVKIT_ODBC_DRIVER_FILE;
  child=spawn(application,[],{cwd:runtime,env:environment,stdio:['ignore',log,log]});
  report.application_pid=child.pid;
  child.once('exit',(code,signal)=>{report.application_exit={code,signal,at:new Date().toISOString(),phase:report.cleanup_phase?'cleanup':'test'};});
  let ready=false;
  for(let attempt=0;attempt<120;attempt++) {
    if(child.exitCode!==null) throw new Error(`Desktop exited before CDP: ${child.exitCode}`);
    try { const response=await fetch(`http://127.0.0.1:${cdpPort}/json/version`,{signal:AbortSignal.timeout(2000)});if(response.ok){ready=true;break;} } catch {}
    await pause(250);
  }
  assert.ok(ready,'Production WebView2 CDP did not start');
  browser=await chromium.connectOverCDP(`http://127.0.0.1:${cdpPort}`);
  page=browser.contexts()[0].pages()[0];
  page.on('close',()=>{report.page_closed={at:new Date().toISOString(),phase:report.cleanup_phase?'cleanup':'test'};});
  browser.on('disconnected',()=>{report.browser_disconnected={at:new Date().toISOString(),phase:report.cleanup_phase?'cleanup':'test'};});
  page.on('request', request=>{
    let url;
    try { url=new URL(request.url()); } catch { return; }
    if(url.hostname!=='ipc.localhost') return;
    const command=decodeURIComponent(url.pathname.replace(/^\/+/,''));
    let args={};
    try { args=JSON.parse(request.postData() || '{}'); } catch {}
    const kind=args.kind ?? args.args?.kind ?? args.payload?.kind;
    report.ipc_requests.push({command,...(typeof kind==='string'?{kind}:{})});
    if(command==='download_database_driver' && typeof kind==='string') downloadCounts[kind]=(downloadCounts[kind] || 0)+1;
  });
  page.on('pageerror',error=>report.page_errors.push(String(error)));
  page.on('console',message=>{if(message.type()==='error') report.console_errors.push(message.text());});
  await page.waitForFunction(()=>window.__TAURI_INTERNALS__?.invoke && document.querySelector('.ribbon-btn'),{},{timeout:30000});
  assert.equal(await page.evaluate(()=>!!window.__PREVIEW_MOCK__),false,'Preview mock must not be active');
  assert.ok(!page.url().includes(':5183') && !page.url().includes(':5173'),`Embedded production URL expected, got ${page.url()}`);
  report.url=page.url();
  // Tauri protects its invoke property. Observe real ipc.localhost requests with
  // CDP instead of mutating framework internals or replacing any command handler.
  check('Fresh isolated executable uses embedded frontend and real Tauri IPC');
  await directDownload('h2','H2');
  await page.locator('button[data-select-kind="h2"]').dblclick();
  const dialog=page.locator('.dialog').filter({has:page.locator('.dialog-title-text',{hasText:/^新建连接 \(H2\)$/})});
  await dialog.waitFor();
  assert.ok(await dialog.locator('input').count()>0);
  const buttons=dialog.locator('.dialog-footer button');
  await buttons.filter({hasText:/^测试连接$/}).click();
  await dialog.getByRole('status').filter({hasText:'连接成功'}).waitFor({timeout:30000});
  await screenshot('h2-real-jdbc-connection-form');
  check('H2: new-connection form defaults to real memory JDBC and successfully connects');
  await buttons.filter({hasText:/^取消$/}).click();
  await saveAndQuery('h2',request('h2',{user:'sa'}),'PUBLIC','NAVKIT_DESKTOP_PROBE');
  await directDownload('duckdb','DuckDB');
  await saveAndQuery('duckdb',request('duckdb',{file:path.join(runtime,'desktop-probe.duckdb')}),'main','navkit_desktop_probe');
   await directDownload('snowflake','Snowflake','manual');
  if(process.env.NAVKIT_E2E_JDBC==='1' || process.env.NAVKIT_E2E_ALL_DOWNLOADS==='1') {
    await directJdbc('trino','Trino','75fd7373cd5d7d7d1fe272d936d436e4dca024ab3d901382277fcdfc46cad476');
    await directJdbc('prestosql','Presto','664a23e517c120c1d5a9bb88b215612b117415b9f1649bc22c48298eaf3f979b');
  }
  if(process.env.NAVKIT_E2E_ALL_DOWNLOADS==='1') {
    await directDownload('firebird','Firebird','manual');
    await directDownload('oracle','Oracle');
    const oracle=(await invoke('list_driver_statuses')).find(item=>item.kind==='oracle');
    assert.ok(oracle.library && fs.statSync(oracle.library).isFile(),'Oracle ready requires the real loaded ODBC library');
    check('Oracle: complete Basic/ODBC runtime is loadable; server remains UNVERIFIED',{server_status:'UNVERIFIED'});
    await directDownload('databricks','Databricks');
    const databricks=(await invoke('list_driver_statuses')).find(item=>item.kind==='databricks');
    assert.ok(databricks.library?.endsWith('.jar'),'Databricks ready requires the real full JAR');
    assert.equal(createHash('sha256').update(fs.readFileSync(databricks.library)).digest('hex'),'1f2e2c5474f2bfbf914f9647319d0d69d8eeb2b1ebb891b05044d79539a359e7');
    const rejected=await invoke('test_connection',{req:request('databricks',{host:'127.0.0.1',port:1,user:'token',password:'isolated-test-token',database:'memory/default',params:'httpPath=/sql/1.0/warehouses/navkit_test;SSL=0',timeout_secs:3})});
    assert.equal(rejected.ok,false,'Databricks TLS downgrade must be rejected');
    assert.match(rejected.message,/TLS|SSL|加密/i);
    assert.doesNotMatch(rejected.message,/isolated-test-token/,'Credential must not leak in errors');
    check('Databricks: full JAR SHA/readiness and TLS downgrade rejection; server remains UNVERIFIED',{server_status:'UNVERIFIED'});
  }
  assert.equal(report.page_errors.length,0,JSON.stringify(report.page_errors));
  report.status='PASS';
} catch(error) {
  report.status='FAIL';report.error=error?.stack || String(error);console.error(report.error);
  if(page) await screenshot('failure').catch(()=>{});
  process.exitCode=1;
} finally {
  report.cleanup_phase=true;
  if(browser) await browser.close().catch(()=>{});
  if(child?.pid && child.exitCode===null) {
    const taskkill=path.join(process.env.SystemRoot || 'C:\\Windows','System32','taskkill.exe');
    const killed=spawnSync(taskkill,['/PID',String(child.pid),'/T','/F'],{stdio:'ignore'});
    report.taskkill={pid:child.pid,status:killed.status,error:killed.error?.message};
    for(let attempt=0;attempt<50 && child.exitCode===null && child.signalCode===null;attempt++) await pause(100);
  }
  fs.closeSync(log);
  // Windows WebView child handles may outlive the parent exit briefly. Do not
  // mask the primary test result with an immediate cleanup exception.
  assert.equal(path.dirname(runtime),path.resolve(os.tmpdir()));
  assert.ok(path.basename(runtime).startsWith('navkit-datasource-desktop-'));
  await pause(1000);
  try {
    fs.rmSync(runtime,{recursive:true,force:true,maxRetries:10,retryDelay:100});
    report.runtime_removed=true;
  } catch(error) {
    report.runtime_removed=false;report.cleanup_error=error?.stack || String(error);
    report.status='FAIL';process.exitCode=1;
  }
  report.finished_at=new Date().toISOString();
  fs.writeFileSync(path.join(evidence,'report.json'),JSON.stringify(report,null,2));
}
