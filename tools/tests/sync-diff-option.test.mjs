// Current SyncDialog with isolated read-only metadata/diff IPC, not a live DB acceptance.
import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
const root=process.cwd();
const bundle=await build({stdin:{contents:'export { openSyncDialog } from "./src/dialogs/SyncDialog.ts";',resolveDir:root,loader:'ts'},bundle:true,write:false,platform:'browser',format:'iife',globalName:'DiffOptionFixture',logLevel:'silent'});
test('diff option belongs to data comparison, preserves default/opt-in IPC and fits narrow windows',async t=>{
 const browser=await chromium.launch({executablePath:process.env.BROWSER_EXE||'C:/Program Files/Google/Chrome/Application/chrome.exe',headless:true});
 try {
  const page=await browser.newPage({viewport:{width:1080,height:850},deviceScaleFactor:2});const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.route('https://diff-options-fixture.test/**',r=>r.fulfill({contentType:'text/html',body:'<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"></head><body></body></html>'}));await page.goto('https://diff-options-fixture.test/');
  await page.addStyleTag({path:path.join(root,'src/styles/base.css')});await page.addScriptTag({content:bundle.outputFiles[0].text});
  await page.evaluate(()=>{
   window.calls=[];
    // Isolated mock UI data only; no OS sampling or live SQLite acceptance.
    window.report={source_rows:100000,target_rows:99991,keyed:true,missing_in_target:10,extra_in_target:1,changed:3,changed_samples:['1','50000','99990'],target_missing_columns:[],target_extra_columns:[],message:null};
    window.preview={available_bytes:1536*1048576,suggested_limit_bytes:384*1048576,global_limit_bytes:640*1048576,scope:'仅 mockUI 比较缓冲，不是实库/OS',limitations:['仅隔离 UI 测试']};const record=(name,handler)=>args=>{calls.push([name,args]);return handler(args);};
   window.__PREVIEW_MOCK__={ensure_connection:record('ensure_connection',()=>({ok:true,message:''})),list_databases:record('list_databases',()=>[['main','sqlite']]),list_tables:record('list_tables',()=>[['users','table']]),sync_type_mapping:record('sync_type_mapping',()=>[{source:'id',source_type:'INTEGER',target_type:'INTEGER'},{source:'name',source_type:'TEXT',target_type:'TEXT'}]),get_table_schema:record('get_table_schema',()=>({columns:[{name:'id',data_type:'INTEGER',primary_key:true},{name:'name',data_type:'TEXT'}]})),sync_diff:record('sync_diff',args=>window.diffHandler?window.diffHandler(args):window.report),preview_diff_memory:record('preview_diff_memory',args=>window.previewHandler?window.previewHandler(args):window.preview)};
   const c=(id,name)=>({id,name,kind:'sqlite',host:'',port:0,user:'',password:'',database:'',file:'',params:'',group:'',read_only:false,keep_alive:false});const source=c('source','示例源 SQLite');window.fixtureConnections=[source,c('target','示例目标 SQLite')];window.dialog=DiffOptionFixture.openSyncDialog(window.fixtureConnections,source,{schema:'main',table:'users'});
  });
  const dialog=page.getByRole('dialog',{name:'数据库同步',exact:true});const next=dialog.locator('.wizard-actions button').nth(1);const back=dialog.locator('.wizard-actions button').nth(0);
  await page.waitForFunction(()=>[...document.querySelectorAll('select[aria-label="源数据库"],select[aria-label="目标数据库"]')].length===2&&[...document.querySelectorAll('select[aria-label="源数据库"],select[aria-label="目标数据库"]')].every(el=>el.value==='main'&&!el.disabled));await next.click();await next.click();
  const group=dialog.getByRole('group',{name:'数据差异比较',exact:true});const checkbox=group.getByRole('checkbox',{name:'内存预算控制',exact:true});const compare=group.getByRole('button',{name:'比较差异',exact:true});await checkbox.waitFor({state:'visible'});
  await page.waitForFunction(()=>!!document.querySelector('[aria-label="id 对应目标列（留空跳过）"]'));
   const budgetMode=group.getByRole('combobox',{name:'比较内存预算模式',exact:true});const manual=group.getByRole('spinbutton',{name:'手动比较缓冲预算（MiB）',exact:true,includeHidden:true});
   const done=()=>page.waitForFunction(()=>document.querySelector('.sync-map > [role="status"]')?.textContent.startsWith('差异比较完成'));
   const args=()=>page.evaluate(()=>calls.filter(([name])=>name==='sync_diff').at(-1)[1]);
  await t.test('one comparison group, no default enable, nearby label and exact default request',async()=>{
   assert.equal(await group.getByRole('button').count(),1);assert.equal(await checkbox.isChecked(),false);assert.match(await checkbox.locator('..').getAttribute('title'),/仅影响.*比较差异.*不改变同步写入/);
   const b=await compare.boundingBox();const label=await checkbox.locator('..').boundingBox();assert.ok(label.x>=b.x+b.width&&label.x-(b.x+b.width)<=20,'memory policy must be adjacent to its button, not auto-pushed toward structure actions');assert.ok(Math.abs((label.y+label.height/2)-(b.y+b.height/2))<=2);
   const layout=await page.evaluate(()=>{const r=s=>{const el=document.querySelector(s);if(!el)return null;const b=el.getBoundingClientRect();return{top:Math.round(b.top),bottom:Math.round(b.bottom)}};return{head:r('.sync-map-head'),map:r('.sync-map'),diff:r('.sync-map-diff-row'),structure:r('.sync-structure-row'),incremental:r('.sync-incremental-row'),headButtons:[...document.querySelectorAll('.sync-map-head button')].map(el=>el.textContent),headLabel:document.querySelector('.sync-map-title')?.textContent,structureChildren:[...document.querySelectorAll('.sync-structure-row > *')].map(el=>el.className||el.tagName),incrementalChildren:[...document.querySelectorAll('.sync-incremental-row > *')].map(el=>el.className||el.tagName)}});
   assert.deepEqual(layout.headButtons,['刷新映射'],'refresh must sit above the mapping table, not with the write actions');
   assert.equal(layout.headLabel,'列映射 · 2 列');assert.ok(layout.head.bottom<=layout.map.top,'refresh header must be above the mapping table');
   assert.deepEqual(layout.structureChildren,['btn','btn','btn','checkbox sync-danger-option'],'drop option belongs with the structure actions');
   assert.deepEqual(layout.incrementalChildren,['sync-incremental-key','checkbox sync-danger-option','INPUT'],'delete-missing-rows follows the business-key title; input comes last');
   const keyLabel=await dialog.locator('.sync-incremental-key').boundingBox();const keyInput=await dialog.locator('.sync-incremental-row > input').boundingBox();const missing=await dialog.locator('.sync-incremental-row > .checkbox').boundingBox();const incremental=await dialog.locator('.sync-incremental-row').boundingBox();
   assert.ok(missing.x>=keyLabel.x+keyLabel.width&&missing.x-(keyLabel.x+keyLabel.width)<=20,'delete-missing-rows must immediately follow the title');
   assert.ok(Math.abs(keyLabel.y+keyLabel.height/2-(missing.y+missing.height/2))<=2,'title and delete-missing-rows must share one horizontal axis');
   assert.ok(keyInput.y>=Math.max(keyLabel.y+keyLabel.height,missing.y+missing.height),'business-key input must sit below the complete title row');
   assert.ok(Math.abs(keyInput.x-incremental.x)<=1&&Math.abs(keyInput.width-incremental.width)<=1,'business-key input must span the full next row');
   assert.ok(layout.map.bottom<=layout.diff.top,'diff row must sit below the mapping table');assert.ok(layout.diff.bottom<=layout.structure.top,'structure row must sit below the diff row');assert.ok(layout.structure.bottom<=layout.incremental.top,'incremental row must sit below the structure row');
   await compare.click();await page.waitForFunction(()=>calls.filter(([name])=>name==='sync_diff').length===1);const args=await page.evaluate(()=>calls.find(([name])=>name==='sync_diff')[1]);assert.deepEqual(Object.keys(args).sort(),['memory','req']);assert.deepEqual(args.memory,{mode:'speed'});assert.deepEqual(args.req,{source_connection:'source',source_schema:'main',source_table:'users',target_connection:'target',target_schema:'main',target_table:'users',where_clause:null});await done();assert.equal(await budgetMode.isDisabled(),true);assert.equal(await manual.isDisabled(),true);assert.equal(await manual.isVisible(),false);
    assert.equal(await group.getByRole('status').textContent(),'','no budget preview while the policy is off');assert.equal(await page.evaluate(()=>calls.some(([name])=>name==='preview_diff_memory')),false);
  });
  await t.test('checked auto budget uses memory only and remains read-only',async()=>{
    await page.evaluate(()=>{window.budgetConfigurationEvents=0;for(const name of ['input','change'])window.dialog.body.addEventListener(name,()=>window.budgetConfigurationEvents++);});await checkbox.check();assert.equal(await page.evaluate(()=>window.budgetConfigurationEvents),0);assert.equal(await budgetMode.isDisabled(),false);await page.waitForFunction(()=>document.querySelector('.sync-diff-controls [role="status"]')?.textContent.includes('预估：'));await compare.click();await done();assert.deepEqual((await args()).memory,{mode:'auto'});assert.equal(Object.hasOwn(await args(),'bounded'),false);
    assert.equal(await page.evaluate(()=>calls.some(([name])=>name.startsWith('run_sync')||name.includes('apply_structure')||name==='execute_structure_sync')),false);
   });
   await t.test('manual MiB freezes exact request, policy and diff-only controls until completion',async()=>{
    await budgetMode.selectOption('manual');await manual.fill('1024');await page.evaluate(()=>{window.diffHandler=()=>new Promise(resolve=>window.finishDiff=resolve);});
    await compare.click();await page.waitForFunction(()=>!!window.finishDiff);assert.equal(await compare.isDisabled(),true);assert.equal(await checkbox.isDisabled(),true);assert.equal(await budgetMode.isDisabled(),true);assert.equal(await manual.isDisabled(),true);assert.equal(await back.isDisabled(),true);assert.equal(await next.isDisabled(),true);
    assert.equal(await dialog.getByRole('button',{name:'预览结构差异',exact:true}).isDisabled(),true);
    const frozen=await args();assert.deepEqual(frozen.memory,{mode:'manual',limit_bytes:1073741824});assert.equal(frozen.req.source_table,'users');assert.equal(frozen.req.target_table,'users');assert.equal(frozen.req.where_clause,null);
    assert.equal(await page.evaluate(()=>(() => {const controls=[...document.querySelectorAll('.sync-side select,.sync-side input:not([type="checkbox"]),[aria-label="源筛选 WHERE"]')];return controls.length===7&&controls.every(el=>el.disabled);})()),true);
    await page.evaluate(()=>{document.querySelector('[data-diff]').click();window.finishDiff({...window.report,memory:{mode:'manual',available_bytes:1536*1048576,limit_bytes:640*1048576,used_bytes:0,peak_bytes:20*1048576,spilled_bytes:8*1048576,backend:'disk',scope:'仅 mockUI 比较缓冲',pressure_events:2}});window.diffHandler=null;});await done();assert.equal(await checkbox.isDisabled(),false);assert.equal(await manual.isDisabled(),false);
    const metrics=await group.getByRole('status').textContent();assert.match(metrics,/请求上限 1,024 MiB/);assert.match(metrics,/实际额度上限 640 MiB/);assert.match(metrics,/核算峰值（含暂存预留）20 MiB/);assert.match(metrics,/磁盘（disk）/);assert.match(metrics,/溢写数据量 8 MiB/);assert.match(metrics,/压力事件 2/);assert.match(metrics,/不是进程RSS/);
    assert.match(await dialog.locator('.sync-map > [role="status"]').textContent(),/目标缺失 10 行.*目标多出 1 行.*内容差异 3 行/);
   });
   await t.test('manual validation rejects non-positive, fractional, non-finite and unsafe byte requests',async()=>{
    const before=await page.evaluate(()=>calls.filter(([name])=>name==='sync_diff').length);
    for(const value of ['', '0','-1','1.5','1e309','9007199254740991','8589934592']){await manual.fill(value);assert.equal(await manual.getAttribute('aria-invalid'),'true');assert.equal(await compare.isDisabled(),true);assert.match(await group.getByRole('status').textContent(),/正整数 MiB/);await page.evaluate(()=>document.querySelector('[data-diff]').click());}
    assert.equal(await page.evaluate(()=>calls.filter(([name])=>name==='sync_diff').length),before);
    await manual.fill('32');assert.equal(await manual.getAttribute('aria-invalid'),'false');await compare.click();await done();assert.deepEqual((await args()).memory,{mode:'manual',limit_bytes:33554432});
   });
   await t.test('budget preview is debounced, failure remains visible and does not disable normal comparison',async()=>{
    await page.evaluate(()=>{window.previewHandler=()=>{throw new Error('DIFF_MEMORY_PREVIEW_NATIVE_REQUIRED mockUI');};});
    const before=await page.evaluate(()=>calls.filter(([name])=>name==='preview_diff_memory').length);
    await page.evaluate(()=>{const input=document.querySelector('[aria-label="手动比较缓冲预算（MiB）"]');for(const value of ['100','101','102','103']){input.value=value;input.dispatchEvent(new Event('input',{bubbles:true}));}});
    await page.waitForFunction(()=>document.querySelector('.sync-diff-controls [role="status"]').textContent.includes('DIFF_MEMORY_PREVIEW_NATIVE_REQUIRED'));assert.equal(await page.evaluate(()=>calls.filter(([name])=>name==='preview_diff_memory').length),before+1);assert.equal(await compare.isDisabled(),false);await compare.click();await done();assert.equal((await args()).memory.limit_bytes,103*1048576);
    await page.evaluate(()=>window.previewHandler=null);
   });
   await t.test('count-only reports remain precise and expose their separate budget path',async()=>{
    await page.evaluate(()=>{window.report.keyed=false;window.report.memory={mode:'manual',available_bytes:64*1048576,limit_bytes:16*1048576,used_bytes:0,peak_bytes:0,spilled_bytes:0,backend:'count_only',scope:'仅 mockUI 比较缓冲',pressure_events:0};});await compare.click();await done();assert.match(await group.getByRole('status').textContent(),/仅行数（count_only）/);assert.equal((await dialog.locator('.sync-map > [role="status"]').textContent()).includes('目标缺失'),false);await page.evaluate(()=>{window.report.keyed=true;delete window.report.memory;});
   });
   await t.test('a late preview cannot overwrite actual comparison statistics',async()=>{
    await page.evaluate(()=>window.previewHandler=()=>new Promise(resolve=>window.finishPreview=resolve));await manual.fill('104');await page.waitForFunction(()=>!!window.finishPreview);
    await page.evaluate(()=>window.report.memory={mode:'manual',available_bytes:1536*1048576,limit_bytes:104*1048576,used_bytes:0,peak_bytes:2*1048576,spilled_bytes:0,backend:'memory',scope:'仅 mockUI 比较缓冲',pressure_events:0});await compare.click();await done();const text=await group.getByRole('status').textContent();
    await page.evaluate(()=>{window.finishPreview({...window.preview,available_bytes:7});window.previewHandler=null;});await page.evaluate(()=>new Promise(requestAnimationFrame));assert.equal(await group.getByRole('status').textContent(),text);assert.match(text,/内存（memory）/);
    await checkbox.uncheck();assert.equal(await manual.isDisabled(),true);
   });
  const dir=path.join(root,'artifacts/performance-review');await mkdir(dir,{recursive:true});
  await t.test('backend diff errors are visible, unlock controls and never silently fall back to legacy',async()=>{
    const before=await page.evaluate(()=>calls.filter(([name])=>name==='sync_diff').length);await page.evaluate(()=>window.diffHandler=()=>{throw new Error('DIFF_MEMORY_DENIED mockUI');});await compare.click();await page.waitForFunction(()=>document.querySelector('.sync-map > [role="alert"]')?.textContent.includes('DIFF_MEMORY_DENIED'));assert.equal(await page.evaluate(()=>calls.filter(([name])=>name==='sync_diff').length),before+1);assert.deepEqual((await args()).memory,{mode:'speed'});assert.equal(await compare.isDisabled(),false);assert.equal(await checkbox.isDisabled(),false);await dialog.locator('.wizard-error-close').click();await page.evaluate(()=>window.diffHandler=null);await compare.click();await done();
   });
   await t.test('whole groups wrap safely at 1080, 560 and 390px',async()=>{
   await checkbox.check();await manual.fill('128');await page.evaluate(()=>window.report.message=Array.from({length:50},(_,i)=>`长状态 ${i}：只验证 mockUI 布局。`).join('\n'));await compare.click();await done();
    for(const width of [1080,560,390]){await page.setViewportSize({width,height:850});await group.scrollIntoViewIfNeeded();const bounds=await dialog.boundingBox();const status=await dialog.locator('.sync-map > [role="status"]').boundingBox();const controls=await dialog.locator('.sync-map-diff-row').boundingBox();assert.ok(status.y+status.height<=controls.y+1,`result text must not overlap option controls at ${width}px: status=${JSON.stringify(status)} controls=${JSON.stringify(controls)}`);for(const locator of [group,compare,checkbox.locator('..'),budgetMode,manual,group.getByRole('status'),dialog.locator('.sync-structure-row'),dialog.locator('.sync-incremental-row > input'),dialog.locator('.sync-incremental-row > .checkbox')]){const b=await locator.boundingBox();assert.ok(b.x>=bounds.x-1&&b.x+b.width<=bounds.x+bounds.width+1,`control overflow at ${width}px`);}await dialog.screenshot({path:path.join(dir,`adaptive-diff-mock-ui-${width}.png`)});}await checkbox.uncheck();
  });
  await t.test('non-SQLite budget is disabled while exact legacy comparison remains available',async()=>{
    await page.setViewportSize({width:1080,height:850});await page.evaluate(()=>{window.fixtureConnections[0].kind='mysql';delete window.report.memory;window.report.message=null;document.querySelector('.sync-side select').dispatchEvent(new Event('change',{bubbles:true}));});await page.waitForFunction(()=>!document.querySelector('.sync-side select').disabled);assert.equal(await checkbox.isDisabled(),true);assert.equal(await budgetMode.isDisabled(),true);assert.equal(await manual.isDisabled(),true);assert.match(await checkbox.getAttribute('title'),/预算控制暂未认证，使用兼容比较/);assert.equal((await group.textContent()).includes('预算控制暂未认证'),false);assert.equal(await compare.isDisabled(),false);await compare.click();await done();assert.deepEqual(Object.keys(await args()),['req']);assert.deepEqual((await args()).req,{source_connection:'source',source_schema:'main',source_table:'users',target_connection:'target',target_schema:'main',target_table:'users',where_clause:null});
    assert.equal(await page.evaluate(()=>calls.some(([name])=>name.startsWith('run_sync')||name.includes('apply_structure')||name==='execute_structure_sync')),false);
   });
   await t.test('database mode hides the complete comparison group',async()=>{await page.setViewportSize({width:1080,height:850});await back.click();await back.click();await dialog.locator('.wizard-panel:not([hidden])').getByRole('combobox',{name:'同步方式',exact:true}).selectOption('database');await next.click();await next.click();assert.equal(await group.isVisible(),false);});
  await t.test('missing budget preview API remains diagnostic, legacy diff is not blocked',async()=>{
    await back.click();await back.click();await dialog.locator('.wizard-panel:not([hidden])').getByRole('combobox',{name:'同步方式',exact:true}).selectOption('table');await page.evaluate(()=>{delete window.__PREVIEW_MOCK__.preview_diff_memory;});await next.click();await next.click();assert.equal(await checkbox.isDisabled(),true);await compare.click();await done();assert.deepEqual(Object.keys(await args()),['req']);
    await page.evaluate(()=>{window.fixtureConnections[0].kind='sqlite';document.querySelector('.sync-side select').dispatchEvent(new Event('change',{bubbles:true}));});await page.waitForFunction(()=>!document.querySelector('.sync-side select').disabled);await checkbox.check();await page.waitForFunction(()=>document.querySelector('.sync-diff-controls [role="status"]').textContent.includes('预算预估不可用'));assert.equal(await compare.isDisabled(),false);await checkbox.uncheck();await compare.click();await done();assert.deepEqual((await args()).memory,{mode:'speed'});
   });
   await t.test('close during read-only diff has no write confirmation and late completion cannot touch detached DOM',async()=>{
    await page.evaluate(()=>{window.diffHandler=()=>new Promise(resolve=>window.finishClosedDiff=resolve);window.confirmCalls=0;window.confirm=()=>{window.confirmCalls++;return true;};});await compare.click();await page.waitForFunction(()=>!!window.finishClosedDiff);await dialog.locator('.dialog-footer').getByRole('button',{name:'关闭',exact:true}).click();assert.equal(await dialog.count(),0);
    await page.evaluate(()=>{window.closedText=window.dialog.el.textContent;window.finishClosedDiff(window.report);});await page.evaluate(()=>new Promise(requestAnimationFrame));assert.equal(await page.evaluate(()=>window.confirmCalls),0);assert.equal(await page.evaluate(()=>window.dialog.el.textContent===window.closedText),true);
   });
   assert.deepEqual(errors,[]);await page.close();
 }finally{await browser.close();}
});
