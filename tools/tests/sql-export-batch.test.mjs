// Real export wizard modules with explicit isolated IPC. No native source/destination databases.
import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright-core';
const built=await build({stdin:{contents:'export { openExportWizard } from "./src/dialogs/ImportExportDialog.ts";',resolveDir:process.cwd(),loader:'ts'},bundle:true,write:false,platform:'browser',format:'iife',globalName:'SqlExportFixture',logLevel:'silent'});
const css=await readFile('src/styles/base.css','utf8');
test('SQL导出真实UI：默认兼容、显式批量需确认、编码预算进入task、CSV不继承批量',async t=>{
 const browser=await chromium.launch({executablePath:process.env.BROWSER_EXE||'C:/Program Files/Google/Chrome/Application/chrome.exe',headless:true});
 try{const page=await browser.newPage({viewport:{width:1280,height:850}});const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.route('https://sql-export-fixture.test/**',r=>r.fulfill({contentType:'text/html',body:'<!doctype html><html lang="zh-CN"><body></body></html>'}));await page.goto('https://sql-export-fixture.test/');await page.addStyleTag({content:css});await page.addScriptTag({content:built.outputFiles[0].text});
 await page.evaluate(()=>{window.exportsSeen=[];window.__TAURI_INTERNALS__={__previewShim:true,invoke:async(command,args)=>command.endsWith('|join')?args.paths.join('/'):'C:/fixture'};window.__PREVIEW_MOCK__={run_export_task:({opts})=>{window.exportsSeen.push(opts);return 513;},transfer_task_progress:({taskId})=>({task_id:taskId,status:'completed',processed:513,total:513,message:'fixture'})};});
 const open=async(kind='sqlite')=>{await page.evaluate(kind=>{window.dialog?.close();window.exportsSeen=[];window.dialog=SqlExportFixture.openExportWizard({id:'isolated',name:'测试连接',kind,params:'',read_only:true},'main','items',()=>{});},kind);await page.getByRole('button',{name:'下一步',exact:true}).click();};
 const next=()=>page.getByRole('button',{name:'下一步',exact:true}).click();
 const format=()=>page.locator('.wizard-panel:not([hidden]) .form-grid select').first();
 const run=async()=>{await next();await page.locator('.wizard-panel:not([hidden]) .form-control-line input').fill('C:/fixture/output.sql');await next();await next();await page.waitForFunction(()=>window.exportsSeen.length>0);};
 await t.test('默认单行与字节选择禁用',async()=>{await open();await format().selectOption('sql');assert.equal(await page.getByLabel('SQL INSERT 组包策略',{exact:true}).inputValue(),'1');assert.equal(await page.getByLabel('SQL 单语句字节上限',{exact:true}).isDisabled(),true);await run();const opts=await page.evaluate(()=>window.exportsSeen.at(-1));assert.equal(opts.sql_batch_rows,1);assert.equal(opts.sql_batch_bytes,0);});
 await t.test('256行需明确同意恢复语句语义变化',async()=>{await open();await format().selectOption('sql');await page.getByLabel('SQL INSERT 组包策略',{exact:true}).selectOption('256');await next();assert.match(await page.locator('.dialog').innerText(),/请确认多行 SQL/);assert.equal(await page.getByLabel('SQL 单语句字节上限',{exact:true}).isEnabled(),true);await page.getByRole('checkbox',{name:/我理解多行恢复/}).check();await page.getByLabel('SQL 单语句字节上限',{exact:true}).selectOption('262144');await run();const opts=await page.evaluate(()=>window.exportsSeen.at(-1));assert.equal(opts.sql_batch_rows,256);assert.equal(opts.sql_batch_bytes,262144);});
 await t.test('换CSV不继承SQL组包',async()=>{await open();await format().selectOption('sql');await page.getByLabel('SQL INSERT 组包策略',{exact:true}).selectOption('256');await format().selectOption('csv');assert.equal(await page.getByLabel('SQL INSERT 组包策略',{exact:true}).isVisible(),false);await run();const opts=await page.evaluate(()=>window.exportsSeen.at(-1));assert.equal(opts.format,'csv');assert.equal(opts.sql_batch_rows,1);assert.equal(opts.sql_batch_bytes,0);});
 await t.test('未验证协议只能默认兼容模式',async()=>{await open('mssql');await format().selectOption('sql');assert.equal(await page.getByLabel('SQL INSERT 组包策略',{exact:true}).isDisabled(),true);assert.match(await page.locator('.dialog').innerText(),/仅支持原生 SQLite\/MySQL\/PostgreSQL/);});
 assert.deepEqual(errors,[]);await page.close();}finally{await browser.close();}
});
