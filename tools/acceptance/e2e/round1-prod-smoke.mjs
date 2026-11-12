/**
 * L7 生产构建桌面冒烟：release exe + dist 前端（真实交付形态）。
 * 校验：启动、前端资源来自生产构建（无 devUrl）、SQLite 真实连接、配置保存、
 * 重启后连接保留、关闭退出干净。运行时目录完全隔离。
 */
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import {randomUUID, createHash} from 'node:crypto';
import {spawn, execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../..');
const id=randomUUID();
const evidence=path.join(root,'docs','验收交付','发布前测试-20261110','证据',`round1-prod-${id}`);
const runtime=fs.mkdtempSync(path.join(os.tmpdir(),`navkit-prod-${id}-`));
fs.mkdirSync(evidence,{recursive:true});
const manifest={id,root,evidence,runtime,started:new Date().toISOString(),checks:[]};
const persist=()=>fs.writeFileSync(path.join(evidence,'manifest.json'),JSON.stringify(manifest,null,2));
persist();
const check=(name,ok,detail='')=>{manifest.checks.push({name,ok,detail});persist();console.log(`${ok?'PASS':'FAIL'} ${name}${detail?' :: '+detail:''}`);if(!ok)process.exitCode=1;};
let app=null;
let exePath=null;
function terminate(){if(app?.pid&&app.exitCode===null){try{execFileSync('taskkill',['/PID',String(app.pid),'/T','/F'],{stdio:'ignore'});}catch{}}}
process.once('exit',terminate);
const launch=async(extraEnv={})=>{
  const stdout=fs.openSync(path.join(evidence,`desktop-${manifest.checks.length}.log`),'a');
  app=spawn(exePath,[],{cwd:runtime,env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=9241',WEBVIEW2_USER_DATA_FOLDER:path.join(runtime,'data','cache','webview'),...extraEnv},stdio:['ignore',stdout,stdout]});
  for(let i=0;i<120;i++){
    if(app.exitCode!==null)throw Error(`app exited: ${app.exitCode}`);
    try{const r=await fetch('http://127.0.0.1:9241/json/version');if(r.ok)return;}catch{}
    await new Promise(r=>setTimeout(r,500));
  }
  throw Error('CDP unavailable');
};
try{
  // 前置：release 产物与 dist 必须存在（由外层构建步骤生成）
  const exe=path.join(root,'src-tauri','target','release','navkit.exe');
  if(!fs.existsSync(exe))throw Error('release exe missing; run cargo build --release first');
  const distIndex=path.join(root,'dist','index.html');
  if(!fs.existsSync(distIndex))throw Error('dist/index.html missing');
  // 便携布局：exe 与 frontend 同目录（runner 布局与 desktop14 一致）
  exePath=path.join(runtime,'navkit.exe');
  fs.copyFileSync(exe,exePath);
  fs.cpSync(path.join(root,'dist'),path.join(runtime,'frontend'),{recursive:true});
  const hash=f=>createHash('sha256').update(fs.readFileSync(f)).digest('hex');
  manifest.binarySha256=hash(exePath);
  manifest.frontendSha256=fs.readdirSync(path.join(runtime,'frontend','assets')).map(n=>({name:n,sha256:hash(path.join(runtime,'frontend','assets',n))}));
  persist();
  check('release_artifacts_present',true,`sha256=${manifest.binarySha256.slice(0,16)}… assets=${manifest.frontendSha256.length}`);

  await launch();
  const {chromium}=await import('playwright-core');
  const browser=await chromium.connectOverCDP('http://127.0.0.1:9241');
  const ctx=browser.contexts()[0];
  let page=ctx.pages().find(p=>/navkit|tauri|localhost/.test(p.url()))??ctx.pages()[0];
  page.setDefaultTimeout(15000);
  await page.waitForFunction(()=>window.__APP__&&window.__TAURI_INTERNALS__,null,{timeout:30000});
  check('app_boot_with_real_ipc',true,`url=${page.url()}`);
  const isCustomProtocol=/tauri:|^http:\/\/tauri\.localhost/i.test(page.url());
  check('production_frontend_protocol',isCustomProtocol,`url=${page.url()}（tauri build 产物不应使用 devUrl 5183）`);
  check('no_preview_mock',!(await page.evaluate(()=>Boolean(window.__PREVIEW_MOCK__))));

  // 前端资源确实来自本地 frontend 目录（非 5183）
  const assetUrls=await page.evaluate(()=>performance.getEntriesByType('resource').map(e=>e.name).filter(n=>/assets\//.test(n)).slice(0,5));
  const noDev=assetUrls.every(u=>!u.includes('5183'));
  check('assets_from_production_dist',noDev&&isCustomProtocol,JSON.stringify(assetUrls));

  // 真实 SQLite 连接 + 保存 + 数据操作
  const sqliteFile=path.join(runtime,'prod-smoke.sqlite');
  const api=await page.evaluate(async()=>{const m=await import('/src/core/api.ts');return m.api;}).then(a=>null).catch(()=>null);
  // release 前端没有 /src/core/api.ts；改用 invoke 直连
  const ipc=(cmd,args)=>page.evaluate(async({cmd,args})=>window.__TAURI_INTERNALS__.invoke(cmd,args),{cmd,args});
  const connId=`prod_${id.slice(0,8)}`;
  const saved=await ipc('save_connection',{config:{id:connId,name:`Prod冒烟-${id.slice(0,6)}`,kind:'sqlite',host:'',port:0,user:'',password:'',database:'',file:sqliteFile,params:'',group:'',read_only:false,keep_alive:false},password:null});
  check('save_connection_persisted',Array.isArray(saved)&&saved.some(c=>c.id===connId));
  const opened=await ipc('open_connection',{id:connId});
  check('open_sqlite_connection',opened&&opened.ok===true,JSON.stringify(opened).slice(0,120));
  const r=await ipc('execute_sql',{id:connId,sql:"CREATE TABLE smoke(id INTEGER PRIMARY KEY, tag TEXT); INSERT INTO smoke VALUES(1,'生产冒烟OK'); SELECT tag FROM smoke;",database:null,owner_id:'prod-smoke',namespace:''}).catch(e=>({__err:String(e)}));
  if(r.__err){
    // v1 execute_sql 契约可能不同；回退用 v2 最小路径校验连接可用性
    const pageR=await ipc('page_table',{id:connId,schema:'main',table:'smoke',page:1,page_size:10}).catch(e=>({__err:String(e)}));
    check('sqlite_data_readable',!pageR.__err&&pageR.rows?.total>=1,pageR.__err??`${pageR.rows?.total} 行`);
  }else{
    check('sqlite_data_readable',JSON.stringify(r).includes('生产冒烟OK'),JSON.stringify(r).slice(0,120));
  }
  const settings=await ipc('get_settings',{});
  const settingsBefore=JSON.stringify(settings);
  await ipc('set_settings',{settings:{...settings,rows_per_page:42}}).catch(e=>{throw Error('set_settings: '+e);});
  check('settings_saved',true);

  // 关闭应用（干净退出）
  // （生产构建无 node_modules；窗口关闭走下方 IPC 调用）
  // 生产构建不含 node_modules 路径：通过 IPC 关闭窗口（等价用户点击退出并确认）
  let closed;
  try{closed=await page.evaluate(async()=>{
    try{
      // 先关闭退出确认（若有），再请求窗口关闭
      const app=window.__APP__;
      if(app?.settings) app.settings={...app.settings,confirm_on_close:false};
      await window.__TAURI_INTERNALS__.invoke('plugin:window|close',{label:'main'});
      return 'close-requested';
    }catch(e){return String(e);}
  });}
  catch{closed='cdp-closed';}
  await new Promise(r=>setTimeout(r,1800));
  let exited=app.exitCode!==null;
  if(!exited){try{process.kill(app.pid,0);}catch{exited=true;}}
  check('clean_exit',exited,`destroy=${closed}`);

  // 重启：连接与设置保留
  await launch();
  const browser2=await chromium.connectOverCDP('http://127.0.0.1:9241');
  const page2=browser2.contexts()[0].pages()[0]??browser2.contexts()[0].pages().pop();
  await page2.waitForFunction(()=>window.__APP__&&window.__TAURI_INTERNALS__,null,{timeout:30000});
  const ipc2=(cmd,args)=>page2.evaluate(async({cmd,args})=>window.__TAURI_INTERNALS__.invoke(cmd,args),{cmd,args});
  const conns=await ipc2('list_connections',{});
  check('connection_survives_restart',conns.some(c=>c.id===connId),`共 ${conns.length} 个连接`);
  const settings2=await ipc2('get_settings',{});
  check('settings_survive_restart',settings2.rows_per_page===42,`rows_per_page=${settings2.rows_per_page}`);
  // 还原设置，避免影响其他测试
  await ipc2('set_settings',{settings:JSON.parse(settingsBefore)});
  const live=await ipc2('open_connection',{id:connId});
  check('reconnect_after_restart',live&&live.ok===true);
  await browser2.close().catch(()=>{});
}catch(error){
  check('smoke_aborted',false,String(error).slice(0,300));
}finally{
  terminate();
  try{fs.rmSync(runtime,{recursive:true,force:true,maxRetries:15,retryDelay:250});}catch(e){manifest.cleanupError=String(e);persist();}
  manifest.finished=new Date().toISOString();persist();
}
console.log(`SUMMARY ${manifest.checks.filter(c=>c.ok).length}/${manifest.checks.length} checks passed; evidence=${evidence}`);
