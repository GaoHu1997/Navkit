// Real backup wizard option/IPC gates, with isolated mock database service only.
import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';
const built=await build({stdin:{contents:'export { openBackupDialog } from "./src/dialogs/BackupDialog.ts";',resolveDir:process.cwd(),loader:'ts'},bundle:true,write:false,platform:'browser',format:'iife',globalName:'BackupConsistencyFixture',loader:{'.css':'empty'},logLevel:'silent'});
test('backup consistency UI: compatible default, opt-in native profiles and lock-risk disclosure',async t=>{
 const browser=await chromium.launch({executablePath:process.env.BROWSER_EXE||'C:/Program Files/Google/Chrome/Application/chrome.exe',headless:true});
 try{
  const page=await browser.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.setContent('<!doctype html><html lang="zh-CN"><body></body></html>');await page.addScriptTag({content:built.outputFiles[0].text});
  await page.evaluate(()=>{window.requests=[];window.__PREVIEW_MOCK__={list_tables:()=>[['t','table']],list_routines:()=>[],create_backup:args=>{window.requests.push(args.opts);return {rows:1,bytes:200,path:'C:/fixture/backup.sql',warnings:[]};}};});
  const open=async(kind)=>{
   await page.evaluate(kind=>{window.dialog?.close();window.requests=[];window.dialog=BackupConsistencyFixture.openBackupDialog({id:'isolated',name:'fixture',kind,database:'s'},'s',()=>{});},kind);
   await page.waitForFunction(()=>document.querySelector('.backup-tree-leaf'));
   await page.getByRole('button',{name:'下一步',exact:true}).click();
  };
  const submit=async()=>{await page.getByRole('button',{name:'下一步',exact:true}).click();await page.getByRole('button',{name:'开始备份',exact:true}).click();await page.waitForFunction(()=>window.requests.length===1);return page.evaluate(()=>window.requests[0]);};
  await t.test('default preserves compatible source policy',async()=>{await open('mysql');assert.equal(await page.getByRole('combobox',{name:'源一致性',exact:true}).inputValue(),'compat');const request=await submit();assert.equal(request.source_consistency,'compat');assert.equal(request.connection_id,'isolated');assert.equal(request.schema,'s');});
  await t.test('MySQL locked profile warns about related WRITE locks before submission',async()=>{await open('mysql');const control=page.getByRole('combobox',{name:'源一致性',exact:true});await control.selectOption('mysql_locked');const note=await control.getAttribute('aria-describedby');assert.ok(note);const text=await page.locator(`#${note}`).innerText();assert.match(text,/隐式锁未选关联表并升级 WRITE/);assert.match(text,/trigger\/FK/);const request=await submit();assert.equal(request.source_consistency,'mysql_locked');});
  await t.test('PG structure boundary and compatibility forks are not auto-enabled',async()=>{await open('postgresql');await page.getByRole('combobox',{name:'源一致性',exact:true}).selectOption('pg_snapshot');assert.match(await page.locator('.dialog').innerText(),/PG14\+.*STORAGE\/COMPRESSION/);const request=await submit();assert.equal(request.source_consistency,'pg_snapshot');for(const kind of ['sqlite','tidb','mariadb']){await open(kind);assert.deepEqual(await page.getByRole('combobox',{name:'源一致性',exact:true}).locator('option').evaluateAll(items=>items.map(item=>item.value)),['compat']);}});
  assert.deepEqual(errors,[]);await page.close();
 }finally{await browser.close();}
});
