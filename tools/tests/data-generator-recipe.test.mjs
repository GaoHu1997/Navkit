// Product TS modules + explicit mock IPC. Native transaction semantics are tested by Rust recipe_*.
import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import path from 'node:path';
import { root, startBrowserFixture } from './batch-execution-fixture.mjs';
const compiled = await build({entryPoints:[path.join(root,'src/core/dataGenerator.ts')],bundle:true,write:false,platform:'node',format:'esm',logLevel:'silent'});
const generator = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString('base64')}`);
const fields=[{name:'id',mode:'sequence',value:'10'}];
const schema={columns:[{name:'id',type:'INTEGER',nullable:false,primary_key:true,auto_increment:false,default:null,length:null,precision:null,scale:null}],foreign_keys:[],indexes:[]};
test('旧500行契约保持，大recipe仅规则/总量/冻结时间，不生成百万rows',()=>{
 assert.equal(generator.generateDataPreview(schema,fields,500).rows.length,500);
 assert.throws(()=>generator.generateDataPreview(schema,fields,501),/500/);
 const r=generator.createGeneratedRecipe('c','main','items',fields,1000000,'seed','2026-11-13T08:00:00Z');
 assert.equal(r.total,1000000);assert.equal(r.transaction_mode,'atomic');assert.equal('rows' in r,false);assert.ok(JSON.stringify(r).length<1024);
 fields[0].value='11';assert.equal(r.fields[0].value,'10');fields[0].value='10';
 for(const total of [0,10000001,1.5,NaN])assert.throws(()=>generator.createGeneratedRecipe('c','main','items',fields,total,'seed'));
});
test('原生样本校验绑定总量/规则/冻结时间/结构，20行硬门禁',()=>{
 const r=generator.createGeneratedRecipe('c','main','items',fields,1000000,'seed','2026-11-13T08:00:00Z');
 const p={recipe:{...r,schema_fingerprint:'a'.repeat(64)},columns:['id'],rows:Array.from({length:20},(_,i)=>[String(i+10)]),database_decided:[],plan_hash:'b'.repeat(64)};
 assert.equal(generator.validateRecipePreview(r,p).rows.length,20);
 for(const patch of [{total:999999},{seed:'other'},{transaction_mode:'segmented'},{frozen_time:'2026-11-14T08:00:00Z'},{schema_fingerprint:''},{fields:[{...fields[0],value:'99'}]}])assert.throws(()=>generator.validateRecipePreview(r,{...p,recipe:{...p.recipe,...patch}}),/不匹配/);
 assert.throws(()=>generator.validateRecipePreview(r,{...p,rows:[...p.rows,['30']]}),/不匹配/);
});
test('显式segmented纯契约只改模式且预览严格绑定模式',()=>{
 const atomic=generator.createGeneratedRecipe('c','main','items',fields,2501,'seed','2026-11-13T08:00:00Z');
 const segmented=generator.createGeneratedRecipe('c','main','items',fields,2501,'seed','2026-11-13T08:00:00Z','segmented');
 assert.deepEqual({...segmented,transaction_mode:'atomic'},atomic);
 const p={recipe:{...segmented,schema_fingerprint:'a'.repeat(64)},columns:['id'],rows:Array.from({length:20},(_,i)=>[String(i+10)]),database_decided:[],plan_hash:'b'.repeat(64)};
 assert.equal(generator.validateRecipePreview(segmented,p).recipe.transaction_mode,'segmented');
 assert.throws(()=>generator.validateRecipePreview(atomic,p),/不匹配/);
 assert.throws(()=>generator.createGeneratedRecipe('c','main','items',fields,2501,'seed','2026-11-13T08:00:00Z','per_chunk'),/事务模式/);
});
async function withPage(run){const fixture=await startBrowserFixture();const page=await fixture.browser.newPage();page.on('dialog',d=>void d.accept());const errors=[];page.on('pageerror',e=>errors.push(e.message));try{await page.goto(fixture.url);await page.waitForFunction(()=>window.batchFixture);await page.evaluate(()=>{
 const f=window.batchFixture,register=f.source.registerBatchExecution.bind(f.source);
 f.source.registerBatchExecution=async request=>{const ticket=await register(request);if(request.task.kind==='generated_recipe'){ticket.total_rows=request.task.recipe.total;f.source.tasks.get(ticket.execution_id).snapshot.total_rows=ticket.total_rows;}return ticket;};
 window.__PREVIEW_MOCK__.preview_generated_recipe=({recipe,ownerId})=>{f.source.calls.push(['recipe-preview',structuredClone(recipe),ownerId]);const sample={recipe:{...structuredClone(recipe),schema_fingerprint:'a'.repeat(64)},columns:['id'],rows:Array.from({length:Math.min(20,recipe.total)},(_,i)=>[String(BigInt(recipe.fields[0].value)+BigInt(i))]),database_decided:[],plan_hash:'b'.repeat(64)};return f.delayRecipePreview?new Promise(resolve=>{f.resolveRecipePreview=()=>resolve(sample);}):sample;};
 });await run(page);assert.deepEqual(errors,[]);}finally{await page.close();await fixture.close();}}
async function openLarge(page,mode='atomic'){await page.evaluate(()=>window.batchFixture.openGenerator());const dialog=page.getByRole('dialog',{name:'数据生成',exact:true});await page.waitForFunction(()=>document.querySelector('[aria-label="目标表"]')?.options.length===1);await dialog.getByLabel('生成条数',{exact:true}).fill('1000000');await dialog.getByLabel('大总量事务模式',{exact:true}).selectOption(mode);await dialog.getByRole('button',{name:'下一步',exact:true}).click();await dialog.getByRole('button',{name:'下一步',exact:true}).click();return dialog;}
test('产品UI（显式mock IPC）：百万总量走Rust样本IPC、冻结recipe授权，不发送百万rows，关闭/取消不重放',async()=>withPage(async page=>{
 const dialog=await openLarge(page);await dialog.getByRole('button',{name:'生成预览',exact:true}).click();await page.waitForFunction(()=>document.querySelector('[role="status"]')?.textContent.includes('原生 recipe 已冻结'));
 assert.equal(await dialog.locator('tbody tr:visible').count(),20);assert.match(await dialog.innerText(),/完整单事务/);
 await dialog.getByRole('checkbox').check();await dialog.getByRole('button',{name:'执行写入',exact:true}).click();await page.waitForFunction(()=>window.batchFixture.service.list()[0]?.context);
 const calls=await page.evaluate(()=>window.batchFixture.source.calls),registration=calls.find(c=>c[0]==='register')[1],preview=calls.find(c=>c[0]==='recipe-preview');
 assert.equal(registration.task.kind,'generated_recipe');assert.equal(registration.task.recipe.transaction_mode,'atomic');assert.equal(registration.task.recipe.total,1000000);assert.equal('rows' in registration.task,false);assert.equal('rows' in registration.task.recipe,false);assert.equal(registration.task.recipe.frozen_time,preview[1].frozen_time);assert.equal(registration.owner_id,preview[2]);assert.ok(JSON.stringify(registration).length<2048);assert.equal(calls.filter(c=>c[0]==='run').length,1);
 await dialog.getByRole('button',{name:'停止',exact:true}).click();await page.waitForFunction(()=>window.batchFixture.source.calls.some(c=>c[0]==='cancel'));assert.equal(await page.evaluate(()=>window.batchFixture.service.list()[0].busy),true);
 await dialog.getByRole('button',{name:'转入后台',exact:true}).click();assert.equal(await page.getByRole('dialog',{name:'数据生成',exact:true}).count(),0);assert.equal(await page.evaluate(()=>window.batchFixture.source.calls.filter(c=>c[0]==='register').length),1);
}));
test('产品UI（显式mock IPC）：显式分段必须额外勾选前缀不可撤销确认，payload模式冻结',async()=>withPage(async page=>{
 const dialog=await openLarge(page,'segmented');await dialog.getByRole('button',{name:'生成预览',exact:true}).click();await page.waitForFunction(()=>document.querySelector('[role="status"]')?.textContent.includes('原生 recipe 已冻结'));
 await dialog.getByRole('checkbox',{name:/我确认按冻结 recipe/}).check();assert.equal(await dialog.getByRole('button',{name:'执行写入',exact:true}).isDisabled(),true);
 assert.equal(await page.evaluate(()=>window.batchFixture.source.calls.filter(c=>c[0]==='register').length),0);
 await dialog.getByRole('checkbox',{name:/已提交前缀失败\/取消不能撤销/}).check();await dialog.getByRole('button',{name:'执行写入',exact:true}).click();await page.waitForFunction(()=>window.batchFixture.service.list()[0]?.context);
 const calls=await page.evaluate(()=>window.batchFixture.source.calls),registration=calls.find(c=>c[0]==='register')[1],preview=calls.find(c=>c[0]==='recipe-preview');
 assert.equal(registration.task.recipe.transaction_mode,'segmented');assert.equal(registration.task.recipe.transaction_mode,preview[1].transaction_mode);assert.equal(registration.task.recipe.frozen_time,preview[1].frozen_time);assert.equal('rows' in registration.task.recipe,false);assert.equal(calls.filter(c=>c[0]==='run').length,1);
 await page.evaluate(()=>{const f=window.batchFixture,id=f.service.list()[0].context.execution_id;f.source.setSnapshot(id,{state:'failed',pending:false,transaction:'rolled_back',scanned:1501,executed:1501,errors:1,affected:1000,error:{code:'BATCH_SQL_FAILED',message:'UNIQUE failed; prefix retained',retryable:false}});Object.assign(f.source.tasks.get(id).snapshot.metrics,{generated:1501,sent:1501,acknowledged:1501,committed:1000});});
 await page.waitForFunction(()=>window.batchFixture.service.list()[0]?.snapshot?.metrics.committed===1000);
 assert.match(await dialog.innerText(),/当前段回滚已确认（已提交前缀保留）/);assert.match(await dialog.innerText(),/已有 1000 行确认提交前缀/);assert.equal(await page.evaluate(()=>window.batchFixture.done.length),0);
}));
test('产品UI（显式mock IPC）：改事务模式使已生成与在途样本失效',async()=>withPage(async page=>{
 const dialog=await openLarge(page);await dialog.getByRole('button',{name:'生成预览',exact:true}).click();await page.waitForFunction(()=>document.querySelector('[role="status"]')?.textContent.includes('原生 recipe 已冻结'));
 await page.evaluate(()=>{const mode=document.querySelector('[aria-label="大总量事务模式"]');mode.value='segmented';mode.dispatchEvent(new Event('change',{bubbles:true}));});
 assert.equal(await dialog.locator('tbody tr:visible').count(),0);assert.equal(await dialog.getByRole('button',{name:'执行写入',exact:true}).isDisabled(),true);
 await page.evaluate(()=>{window.batchFixture.delayRecipePreview=true;});await dialog.getByRole('button',{name:'生成预览',exact:true}).click();await page.waitForFunction(()=>window.batchFixture.resolveRecipePreview);
 await page.evaluate(()=>{const mode=document.querySelector('[aria-label="大总量事务模式"]');mode.value='atomic';mode.dispatchEvent(new Event('change',{bubbles:true}));window.batchFixture.resolveRecipePreview();});
 await page.waitForTimeout(30);assert.equal(await dialog.locator('tbody tr:visible').count(),0);assert.equal(await dialog.getByRole('button',{name:'执行写入',exact:true}).isDisabled(),true);assert.equal(await page.evaluate(()=>window.batchFixture.source.calls.filter(c=>c[0]==='register').length),0);
}));
test('产品UI（显式mock IPC）：旧小批仍generated_rows，PG/MySQL分段选项不开放',async()=>withPage(async page=>{
 await page.evaluate(()=>window.batchFixture.openGenerator());let dialog=page.getByRole('dialog',{name:'数据生成',exact:true});await page.waitForFunction(()=>document.querySelector('[aria-label="目标表"]')?.options.length===1);
 assert.equal(await dialog.getByLabel('大总量事务模式',{exact:true}).isVisible(),false);await dialog.getByRole('button',{name:'下一步',exact:true}).click();await dialog.getByRole('button',{name:'下一步',exact:true}).click();await dialog.getByRole('button',{name:'生成预览',exact:true}).click();await dialog.getByRole('checkbox').check();await dialog.getByRole('button',{name:'执行写入',exact:true}).click();await page.waitForFunction(()=>window.batchFixture.service.list()[0]?.context);
 const registration=await page.evaluate(()=>window.batchFixture.source.calls.find(c=>c[0]==='register')[1]);assert.equal(registration.task.kind,'generated_rows');assert.equal(registration.task.request.rows.length,10);assert.equal(await page.evaluate(()=>window.batchFixture.source.calls.some(c=>c[0]==='recipe-preview')),false);
 await dialog.getByRole('button',{name:'转入后台',exact:true}).click();
 for(const kind of ['postgresql','mysql']){await page.evaluate(kind=>{window.batchFixture.connection.kind=kind;window.batchFixture.openGenerator();},kind);dialog=page.getByRole('dialog',{name:'数据生成',exact:true});await page.waitForFunction(()=>document.querySelector('[aria-label="目标表"]')?.options.length===1);await dialog.getByLabel('生成条数',{exact:true}).fill('2501');assert.equal(await dialog.getByLabel('大总量事务模式',{exact:true}).inputValue(),'atomic');assert.equal(await dialog.locator('[aria-label="大总量事务模式"] option[value="segmented"]').isDisabled(),true);await dialog.getByRole('button',{name:'关闭',exact:true}).last().click();}
}));

test('产品UI（显式mock IPC）：在途样本后改规则/总量失效，迟到旧sample不能重新授权',async()=>withPage(async page=>{
 const dialog=await openLarge(page);await page.evaluate(()=>{window.batchFixture.delayRecipePreview=true;});await dialog.getByRole('button',{name:'生成预览',exact:true}).click();await page.waitForFunction(()=>window.batchFixture.resolveRecipePreview);
 await page.evaluate(()=>{const input=document.querySelector('[aria-label="生成条数"]');input.value='2000000';input.dispatchEvent(new Event('input',{bubbles:true}));window.batchFixture.resolveRecipePreview();});
 await page.waitForTimeout(30);assert.equal(await dialog.getByRole('button',{name:'执行写入',exact:true}).isDisabled(),true);assert.equal(await dialog.locator('tbody tr:visible').count(),0);assert.equal(await page.evaluate(()=>window.batchFixture.source.calls.filter(c=>c[0]==='register').length),0);
}));
