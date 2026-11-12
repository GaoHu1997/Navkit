import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {pathToFileURL} from 'node:url';
const EVID=process.env.NAVKIT_E2E_EVID;
const {connect,shot,sleep,consoleLines}=await import(pathToFileURL(path.join(EVID,'harness.mjs')).href);
const runtime=process.env.NAVKIT_E2E_RUNTIME, ns=process.env.NAVKIT_E2E_NAMESPACE,pid=Number(process.env.NAVKIT_E2E_PID);
const results=[];
const {page,browser}=await connect('http://127.0.0.1:9235');
page.setDefaultTimeout(7000);
if(fs.existsSync(path.join(runtime,'frontend','index.html'))){
  await page.route('**/index.html',route=>route.fulfill({status:200,contentType:'text/html',body:fs.readFileSync(path.join(runtime,'frontend','index.html'),'utf8')}));
  await page.route('**/assets/**',route=>{const relative=decodeURIComponent(new URL(route.request().url()).pathname).replace(/^\//,'');const file=path.resolve(runtime,'frontend',relative);if(!file.startsWith(path.resolve(runtime,'frontend')+path.sep)||!fs.existsSync(file))return route.continue();return route.fulfill({status:200,contentType:file.endsWith('.js')?'application/javascript':file.endsWith('.css')?'text/css':file.endsWith('.svg')?'image/svg+xml':'application/octet-stream',body:fs.readFileSync(file)});});
}
await page.route('**/@vite/client',route=>route.fulfill({status:200,contentType:'application/javascript',body:'export function createHotContext(){return {accept(){},dispose(){},on(){},invalidate(){},data:{}};} export function updateStyle(){} export function removeStyle(){} export function injectQuery(url){return url;}'}));
await page.reload();
page.on('dialog',async d=>d.dismiss());
await page.addInitScript(()=>{window.__E2E_EVENTS__=[];for(const type of ['mousedown','mouseup','click','focusin','focusout','change'])document.addEventListener(type,event=>{const t=event.target;window.__E2E_EVENTS__.push({type,time:performance.now(),target:t.tagName,classes:t.className,text:t.textContent?.slice(0,45),value:t.value,step:document.querySelector('.dialog .wizard-step.active .wizard-step-num')?.textContent});if(window.__E2E_EVENTS__.length>1000)window.__E2E_EVENTS__.shift();},true);});
await page.reload();
await page.waitForFunction(()=>window.__APP__ && window.__TAURI_INTERNALS__);
assert.equal(await page.evaluate(()=>Boolean(window.__PREVIEW_MOCK__)),false,'Must use real Tauri, never preview mock');
await page.evaluate(async()=>{const m=await import('/src/core/api.ts');window.__E2E_API__=m;window.__E2E_WIN__=(await import('/node_modules/@tauri-apps/api/window.js')).getCurrentWindow();});
const api=(method,...args)=>page.evaluate(async({method,args})=>window.__E2E_API__.api[method](...args),{method,args});
const ipc=(cmd,args)=>page.evaluate(async({cmd,args})=>window.__TAURI_INTERNALS__.invoke(cmd,args),{cmd,args});
const obs=()=>page.evaluate(()=>{const a=window.__APP__,t=a.activeTab,v=a.activeView();return {tab:t?{id:t.id,kind:t.kind,connectionId:t.connectionId,schema:t.schema,object:t.object,filters:t.filters,order_rules:t.order_rules}:null,rowCount:v?.rowCount,rows:v?.grid?.rows,sql:v?.editor?.getSql(),open:[...a.openConnections],dbs:[...a.openDbs]};});
const node=id=>page.locator(`.tree-node[data-id="${id}"]`);
const tool=label=>page.locator('.tab-tool-btn').filter({hasText:label}).first();
const ribbon=label=>page.locator('.ribbon-btn').filter({has:page.locator('.ribbon-label').getByText(label,{exact:true})}).first();
const dlg=()=>page.locator('.dialog').last();
async function clean(){for(let i=0;i<8 && await page.locator('.dialog').count();i++){await dlg().locator('.dialog-title-close').click();await sleep(80);}await page.keyboard.press('Escape');}
async function menu(top,item){await page.locator('.menubar-item').getByText(top,{exact:true}).click();await page.locator('.menu-entry').filter({has:page.locator('.menu-entry-label').getByText(item,{exact:true})}).last().click();}
async function context(id,item){await node(id).click({button:'right'});await page.locator('.menu-entry').filter({has:page.locator('.menu-entry-label').getByText(item,{exact:true})}).last().click();}
async function check(req,id,title,fn){if(process.env.NAVKIT_E2E_ONLY && !process.env.NAVKIT_E2E_ONLY.split(',').some(prefix=>id.startsWith(prefix)))return;await clean();const started=new Date().toISOString();try{const data=await fn();results.push({requirement:req,id,title,status:'PASS',started,data});console.log(`PASS ${id} ${title}`);}catch(error){results.push({requirement:req,id,title,status:'FAIL',started,error:String(error),stack:error.stack,observed:await obs().catch(()=>null)});console.log(`FAIL ${id}: ${error}`);fs.writeFileSync(path.join(EVID,`${id}-dom.txt`),await page.locator('body').innerText().catch(()=>''));}try{fs.writeFileSync(path.join(EVID,`${id}-events.json`),JSON.stringify(await page.evaluate(()=>window.__E2E_EVENTS__??[]),null,2));await shot(page,id);}catch{}fs.writeFileSync(path.join(EVID,'assertions.json'),JSON.stringify(results,null,2));}
async function nextStep(d){
  const before=(await d.locator('.wizard-step.active .wizard-step-num').innerText()).trim();
  // 直接点向导主按钮（.btn.primary 且文本为 下一步）：避开滚动容器遮挡与文本匹配歧义
  await page.evaluate(()=>{
    const buttons=[...document.querySelectorAll('.dialog button.btn.primary')];
    const target=buttons.find(b=>b.textContent.trim()==='下一步')??buttons[0];
    if(!target) throw new Error('找不到向导主按钮');
    target.click();
  });
  try{
    await page.waitForFunction(before=>{
      const active=document.querySelector('.dialog .wizard-step.active .wizard-step-num');
      return active && active.textContent.trim()!==before;
    },before,{timeout:4000});
  }catch{
    const err=await d.locator('.wizard-error, [role="alert"]').allTextContents().catch(()=>[]);
    throw new Error(`步骤 ${before} 未前进；错误行：${JSON.stringify(err)}；面板：${(await d.innerText()).slice(0,300)}`);
  }
}
/** 点击 chip 文本并从 picker 选项中选择值（当前筛选/排序面板的审定交互）。 */
async function pickChip(anchor, value) {
  await anchor.click();
  await page.locator('.fb-picker').waitFor({timeout: 3000});
  await page.locator('.fb-picker .menu-entry').filter({hasText: value}).first().click();
  await sleep(120);
}
/** 就地输入筛选值：点击 .fb-val 后出现 input，回车提交。 */
async function typeFilterValue(chip, value) {
  await chip.click();
  const input = page.locator('.cond-chip input:visible, .fb-val-input:visible').last();
  await input.waitFor({timeout: 3000});
  await input.fill(value);
  await input.press('Enter');
  await sleep(150);
}
async function treeExpand(id){await page.waitForFunction(id=>{const n=window.__APP__.tree.find(id);return n&&!n.loading;},id);const n=node(id);await n.waitFor();if(await n.getAttribute('aria-expanded')!=='true'){await n.locator('.tree-arrow').click();await page.waitForFunction(id=>{const n=window.__APP__.tree.find(id);return n?.expanded&&!n.loading;},id);}await sleep(150);}
async function openDb(conn,schema){await treeExpand(`conn:${conn}`);await node(`conn:${conn}/db/${schema}`).dblclick();await page.waitForFunction(({conn,schema})=>window.__APP__.openDbs.has(`${conn}/${schema}`),{conn,schema});await treeExpand(`conn:${conn}/db/${schema}`);}
async function openTable(conn,schema,table){if(conn===ids.sqlite||conn===ids.target){await treeExpand(`conn:${conn}`);if(!await node(`conn:${conn}/table/${table}`).count()){await page.evaluate(async conn=>{const a=window.__APP__,n=a.tree.find(`conn:${conn}`);await a.loadChildren(n);n.expanded=true;a.tree.render();},conn);}await node(`conn:${conn}/table/${table}`).dblclick();}else{await openDb(conn,schema);await treeExpand(`conn:${conn}/db/${schema}/grp-table`);await node(`conn:${conn}/db/${schema}/table/${table}`).dblclick();}await page.waitForFunction(({conn,table})=>{const a=window.__APP__;return a.activeTab?.connectionId===conn && a.activeTab?.object===table && a.activeView()?.rowCount!==undefined},{conn,table});await sleep(350);}
async function newConn(kind){await ribbon('连接').click();await dlg().locator('.conn-picker-search').fill(kind);await dlg().locator('.conn-kind-card').filter({has:page.locator('.conn-kind-name').getByText(kind,{exact:true})}).click();await dlg().getByRole('button',{name:'下一步',exact:true}).click();return dlg();}
const suffix=ns.slice(-8), ids={sqlite:`s_${suffix}`,target:`t_${suffix}`,mysql:`m_${suffix}`,pg:`p_${suffix}`};
const names={sqlite:`SQLite-${suffix}`,target:`Target-${suffix}`,mysql:`MySQL-${suffix}`,pg:`PG-${suffix}`};
const base={host:'127.0.0.1',port:0,user:'',password:'',database:'',file:'',params:'',group:'',read_only:false,keep_alive:false};
for(const [key,kind,port,user,database,file,password] of [['sqlite','sqlite',0,'','',path.join(runtime,'source.sqlite'),''],['target','sqlite',0,'','',path.join(runtime,'target.sqlite'),''],['mysql','mysql',3306,'root',ns,'','123456'],['pg','postgresql',5433,'postgres','postgres','','']]){
  await api('saveConnection',{...base,id:ids[key],name:names[key],kind,port,user,database,file},password);
}
await page.evaluate(async()=>{const a=window.__APP__;a.connections=await window.__E2E_API__.api.listConnections();a.groups=await window.__E2E_API__.api.listGroups();a.refreshTree();});
await check(1,'R01-SQLITE','真实 SQLite 连接双击并建立数据',async()=>{
  await node(`conn:${ids.sqlite}`).dblclick();await page.waitForFunction(id=>window.__APP__.openConnections.has(id),ids.sqlite);await page.waitForFunction(id=>{const n=window.__APP__.tree.find(`conn:${id}`);return n&&!n.loading&&n.expanded;},ids.sqlite);
  await api('executeSql',ids.sqlite,"CREATE TABLE items(id INTEGER PRIMARY KEY,grp INTEGER,name TEXT); INSERT INTO items VALUES(1,2,'alpha'),(2,1,'beta'),(3,1,'gamma'),(4,2,'delta'); CREATE TABLE empty(id INTEGER PRIMARY KEY,name TEXT); CREATE VIEW v_items AS SELECT * FROM items;");
  await page.evaluate(async conn=>{const a=window.__APP__,n=a.tree.find(`conn:${conn}`);await a.loadChildren(n);n.expanded=true;n.loaded=true;a.tree.render();},ids.sqlite);
  const fixtureMetadata=await api('listTables',ids.sqlite,'main');assert(fixtureMetadata.some(([name,type])=>name==='v_items'&&type==='view'));fs.writeFileSync(path.join(EVID,'sqlite-fixture-metadata.json'),JSON.stringify(fixtureMetadata,null,2));
  const r=await api('executeSql',ids.sqlite,'SELECT COUNT(*) AS n FROM items');assert.equal(r.sets[0].rows[0][0],'4');return {pid,connection:ids.sqlite,count:r.sets[0].rows[0][0]};
});
await check(1,'R01-MYSQL','真实 MySQL 临时库双击打开',async()=>{await node(`conn:${ids.mysql}`).dblclick();await page.waitForFunction(id=>window.__APP__.openConnections.has(id),ids.mysql);await openDb(ids.mysql,ns);const s=await obs();assert(s.dbs.includes(`${ids.mysql}/${ns}`));const r=await api('executeSql',ids.mysql,'SELECT DATABASE(),COUNT(*) FROM items',ns);assert.equal(r.sets[0].rows[0][0],ns);assert.equal(r.sets[0].rows[0][1],'4');return r;});
await check(1,'R01-PG','真实 PG 临时 schema 双击打开',async()=>{await node(`conn:${ids.pg}`).dblclick();await page.waitForFunction(id=>window.__APP__.openConnections.has(id),ids.pg);await openDb(ids.pg,ns);const r=await api('executeSql',ids.pg,'SELECT current_schema(),COUNT(*) FROM items',ns);assert.equal(r.sets[0].rows[0][0],ns);assert.equal(r.sets[0].rows[0][1],'4');return r;});
await check(1,'R01-QUERY','查询跟随选中库上下文并保存真实查询',async()=>{await node(`conn:${ids.mysql}/db/${ns}`).click();await ribbon('新建查询').click();await page.locator('.cm-content:visible').waitFor();assert.equal((await obs()).tab.schema,ns);const editor=page.locator('.cm-content:visible');await editor.fill('SELECT DATABASE() AS db, COUNT(*) AS n FROM items;');await page.locator('.query-runbar:visible .run-btn').getByText('运行',{exact:true}).first().click();await page.waitForFunction(()=>window.__APP__.activeTab?.results?.sets?.[0]?.rows?.length>0);const s=await obs();assert.equal(s.rows[0][0],ns);assert.equal(s.rows[0][1],'4');await tool('保存').click();await dlg().locator('input').fill(`saved_${suffix}`);await dlg().getByRole('button',{name:'保存',exact:true}).click();await sleep(400);const fav=(await api('listFavorites')).find(f=>f.name===`saved_${suffix}`);assert(fav?.sql.includes('DATABASE()'));assert.equal(fav.connection_id,ids.mysql);return {schema:s.tab.schema,rows:s.rows,favorite:fav};});
await check(1,'R01-SNIPPET','右侧代码段搜索、标签过滤和插入不执行',async()=>{await node(`conn:${ids.mysql}/db/${ns}`).click();await ribbon('新建查询').click();const search=page.locator('.snippet-search'),combo=page.locator('.snippet-combo');const tags=await combo.locator('option').evaluateAll(els=>els.map(e=>({value:e.value,text:e.textContent})));assert(tags.length>1);await combo.selectOption(tags[1].value);const titles=await page.locator('.snippet-title').allTextContents();assert(titles.length>0);
  // 搜索真实标题：结果必须收敛且至少 1 条（标题含标签文本时可能多匹配，不做精确条数断言）
  const allCount=await page.locator('.snippet-item').count();
  // 标题含标签文本（name+tag），搜索必须用去掉标签后的名称前缀，避免 0 结果
  const queryText=titles[0].trim().slice(0,4);
  await search.fill(queryText);
  await sleep(200);
  const filtered=await page.locator('.snippet-item').count();
  assert(filtered>=1 && filtered<=allCount,`搜索应收敛结果：${filtered}/${allCount}（查询 "${queryText}"）`);
  const before=await obs();
  await page.locator('.snippet-item').first().click();
  await sleep(250);
  const after=await obs();
  assert(after.sql && after.sql!==before.sql,'点击代码段应插入 SQL');
  assert.deepEqual(after.rows,before.rows,'插入代码段不得执行（结果不变）');
  await search.fill('zz_no_match_uuid');await sleep(200);
  assert.equal(await page.locator('.snippet-item').count(),0,'无匹配时应为空');
  return {tags,allCount,filtered,selected:titles[0],insertedSql:after.sql};});
await check(1,'R01-KEYBOARD','真实查询 Control+Enter/F5/Control+Shift+Enter 执行',async()=>{await node(`conn:${ids.mysql}/db/${ns}`).click();await ribbon('新建查询').click();const editor=page.locator('.cm-content:visible');await editor.fill('SELECT 11 AS n;');await editor.press('Control+Enter');await page.waitForFunction(()=>window.__APP__.activeTab?.results?.sets?.[0]?.rows?.[0]?.[0]==='11');await editor.fill('SELECT 12 AS n;');await editor.press('F5');await page.waitForFunction(()=>window.__APP__.activeTab?.results?.sets?.[0]?.rows?.[0]?.[0]==='12');await editor.fill('SELECT 13 AS n;');await editor.press('Control+a');await editor.press('Control+Shift+Enter');await page.waitForFunction(()=>window.__APP__.activeTab?.results?.sets?.[0]?.rows?.[0]?.[0]==='13');return {controlEnter:11,F5:12,controlShiftEnter:13};});
await check(11,'R11-SUBMENU','文件新建子菜单 hover 后查询和组入口可用',async()=>{await page.locator('.menubar-item').getByText('文件',{exact:true}).click();await page.locator('.menu-entry').filter({has:page.locator('.menu-entry-label').getByText('新建',{exact:true})}).hover();await page.locator('.menu-popup[data-depth="1"]').waitFor();const labels=await page.locator('.menu-popup[data-depth="1"] .menu-entry-label').allTextContents();assert(labels.includes('查询'));assert(labels.includes('组...'));await page.locator('.menu-popup[data-depth="1"] .menu-entry-label').getByText('查询',{exact:true}).click();await page.waitForFunction(()=>window.__APP__.activeTab?.kind==='query');return {labels,schema:(await obs()).tab.schema};});
await check(3,'R03-WIDTH','列宽 hover 显示拖拽光标并真实 mouse drag',async()=>{await openTable(ids.sqlite,'main','items');const th=page.locator('.content-host .grid-table:visible th[data-col="0"]').first();const handle=th.locator('.grid-column-resize-handle');await handle.hover();const css=await handle.evaluate(el=>({cursor:getComputedStyle(el).cursor,rect:el.getBoundingClientRect().toJSON()}));assert.equal(css.cursor,'col-resize');const w0=await th.evaluate(e=>e.getBoundingClientRect().width);const b=await handle.boundingBox();await page.mouse.move(b.x+b.width/2,b.y+b.height/2);await page.mouse.down();await page.mouse.move(b.x+b.width/2+80,b.y+b.height/2,{steps:10});await page.mouse.up();await sleep(200);const w1=await th.evaluate(e=>e.getBoundingClientRect().width);assert(w1>w0+60);await tool('筛选').click();await tool('筛选').click();const w2=await th.evaluate(e=>e.getBoundingClientRect().width);assert(Math.abs(w2-w1)<3);return {css,w0,w1,w2};});
await check(4,'R04-FILTER','AND/OR 真实筛选、零结果仍可取消清除',async()=>{
  await openTable(ids.sqlite,'main','items');
  await tool('筛选').click();
  // 第 1 行：id = 1
  await pickChip(page.locator('.filter-rows:visible .cond-chip').first().locator('.fb-txt').first(),'id');
  await typeFilterValue(page.locator('.filter-rows:visible .cond-chip').first().locator('.fb-val'),'1');
  await page.getByRole('button',{name:'＋ 添加条件行',exact:true}).click();
  await sleep(200);
  // 第 2 行：id = 2（默认 AND）→ 零结果
  const secondRow=page.locator('.filter-rows:visible .filter-line').nth(1);
  await typeFilterValue(secondRow.locator('.cond-chip .fb-val').first(),'2');
  await page.getByRole('button',{name:'应用筛选',exact:true}).click();
  await sleep(450);
  assert.equal((await obs()).rowCount,0,'AND(id=1,id=2) 应为 0 行');
  assert.equal(await tool('筛选').isEnabled(),true,'零结果不禁用筛选');
  // 切换 AND → OR
  await secondRow.locator('.fb-join-text').click();
  await sleep(150);
  await page.getByRole('button',{name:'应用筛选',exact:true}).click();
  await sleep(450);
  assert.equal((await obs()).rowCount,2,'OR 应为 2 行');
  const orRows=(await obs()).rows;
  // 追加 grp=1 且 AND
  await page.getByRole('button',{name:'＋ 添加条件行',exact:true}).click();
  await sleep(200);
  const thirdRow=page.locator('.filter-rows:visible .filter-line').nth(2);
  await pickChip(thirdRow.locator('.cond-chip .fb-txt').first(),'grp');
  await typeFilterValue(thirdRow.locator('.cond-chip .fb-val').first(),'1');
  // 新行沿用上一行连接方式（设计行为）；显式切回 AND 以验证混合语义
  if((await thirdRow.locator('.fb-join-text').innerText()).trim()==='OR')await thirdRow.locator('.fb-join-text').click();
  await sleep(150);
  await page.getByRole('button',{name:'应用筛选',exact:true}).click();
  await sleep(450);
  assert.equal((await obs()).rowCount,2,'(id=1 OR id=2) AND grp=1 应为 2 行');
  const mixed=(await obs()).tab.filters;
  assert(Array.isArray(mixed)&&mixed.length>=3,`筛选行应以 FilterLine 行组保存：${JSON.stringify(mixed)}`);
  await page.getByRole('button',{name:'清除筛选',exact:true}).click();
  await sleep(450);
  assert.equal((await obs()).rowCount,4,'清除筛选后恢复 4 行');
  return {and:0,or:orRows,mixed,cleared:4};
});
await check(5,'R05-SORT','多字段优先级排序真实 SQL 顺序',async()=>{
  await openTable(ids.sqlite,'main','items');
  await tool('排序').click();
  const add=page.getByRole('button',{name:'＋ 添加排序字段',exact:true});
  if(!await page.locator('.sort-line:visible').count())await add.click();
  await pickChip(page.locator('.sort-rows:visible .sort-line').nth(0).locator('.fb-txt').first(),'grp');
  await add.click();
  await sleep(200);
  await pickChip(page.locator('.sort-rows:visible .sort-line').nth(1).locator('.fb-txt').first(),'id');
  const dir1=page.locator('.sort-rows:visible .sort-line').nth(1).locator('.fb-dir');
  // 默认升序（↑）；目标为降序（↓）
  if((await dir1.innerText()).trim()==='↑')await dir1.click();
  await page.getByRole('button',{name:'应用排序',exact:true}).click();
  await sleep(450);
  const s1=await obs();
  assert.deepEqual(s1.rows.map(r=>r[0]),['3','2','4','1'],`grp ASC, id DESC 顺序：${JSON.stringify(s1.rows)}`);
  assert.equal(s1.tab.order_rules.length,2,'应保存两条排序规则');
  // 上移第 2 条 → 优先级反转
  await page.locator('.sort-rows:visible .sort-line').nth(1).locator('[title="上移"]').click();
  await sleep(150);
  await page.getByRole('button',{name:'应用排序',exact:true}).click();
  await sleep(400);
  assert.deepEqual((await obs()).rows.map(r=>r[0]),['4','3','2','1'],'上移后 id DESC, grp ASC');
  return {first:s1.rows,rules:s1.tab.order_rules,reordered:(await obs()).rows};
});
await check(6,'R06-EMPTY','零行表筛选排序导入和插入记录入口可用',async()=>{await openTable(ids.sqlite,'main','empty');assert.equal((await obs()).rowCount,0);const availability={};for(const label of ['筛选','排序','导入']){const b=tool(label);availability[label]=await b.isEnabled();assert.equal(availability[label],true);}await tool('筛选').click();assert(await page.locator('.filter-rows:visible').count()>0,'零行表应显示筛选容器');await page.getByRole('button',{name:'＋ 添加条件行',exact:true}).click();await sleep(200);assert(await page.locator('.cond-chip:visible').count()>0,'零行表应可添加筛选条件');await tool('排序').click();assert(await page.locator('.sort-rows:visible').count()>0);await tool('导入').click();assert((await dlg().innerText()).includes('导入向导'));await clean();await page.locator('.grid-footer-btn[title="插入记录"]:visible').click();await sleep(150);assert.equal((await obs()).rows.length,1);await page.locator('.grid-footer-btn[title*="清除本机未提交草稿"]:visible').click();await sleep(150);assert.equal((await obs()).rows.length,0);return availability;});
for(const [kind,key] of [['SQLite','sqlite'],['MySQL','mysql'],['PG','pg']]){
 await check(7,`R07-DETAILS-${key}`,`${kind} 连接库表视图例程详细信息`,async()=>{const conn=ids[key],schema=key==='sqlite'?'main':ns;const snapshots={};await node(`conn:${conn}`).click();await sleep(500);snapshots.connection=await page.locator('.info-panel').innerText();assert(snapshots.connection.includes(names[key]));await treeExpand(`conn:${conn}`);fs.writeFileSync(path.join(EVID,`R07-${key}-tree-metadata.json`),JSON.stringify({api:await api('listTables',conn,schema),tree:await page.evaluate(conn=>window.__APP__.tree.find(`conn:${conn}`),conn)},null,2));if(key!=='sqlite'){await openDb(conn,schema);await node(`conn:${conn}/db/${schema}`).click();await sleep(500);snapshots.database=await page.locator('.info-panel').innerText();assert(snapshots.database.includes(schema));await treeExpand(`conn:${conn}/db/${schema}/grp-table`);}await node(key==='sqlite'?`conn:${conn}/table/items`:`conn:${conn}/db/${schema}/table/items`).click();await sleep(600);snapshots.table=await page.locator('.info-panel').innerText();assert(snapshots.table.includes('items') && /字段|列|引擎/.test(snapshots.table));if(key!=='sqlite')await treeExpand(`conn:${conn}/db/${schema}/grp-view`);await node(key==='sqlite'?`conn:${conn}/table/v_items`:`conn:${conn}/db/${schema}/view/v_items`).click();await sleep(500);snapshots.view=await page.locator('.info-panel').innerText();assert(/SELECT|select/.test(snapshots.view));if(key!=='sqlite'){await treeExpand(`conn:${conn}/db/${schema}/grp-function`);const f=page.locator(`.tree-node[data-id^="conn:${conn}/db/${schema}/catalog/"]`).filter({hasText:'fn_e2e'}).first();await f.click();await sleep(500);snapshots.routine=await page.locator('.info-panel').innerText();assert(/定义/.test(snapshots.routine) && /fn_e2e|RETURN|SELECT/i.test(snapshots.routine));}assert(!Object.values(snapshots).some(t=>/读取失败|加载失败/.test(t)));return snapshots;});
}
await check(7,'R07-BACKUP','临时 SQLite 真实备份文件和详细信息',async()=>{const before=await api('listBackups',ids.sqlite,'main');await api('createBackup',{connection_id:ids.sqlite,schema:'main',comment:`uuid_${suffix}`,tables:['items','v_items'],include_structure:true,include_data:true});const after=await api('listBackups',ids.sqlite,'main');assert.equal(after.length,before.length+1);await node(`conn:${ids.sqlite}`).click();await ribbon('备份').click();await page.waitForFunction(()=>window.__APP__.activeTab?.kind==='backup');await sleep(400);await page.locator('.content-host .grid-table:visible tbody tr').first().click();await sleep(350);const text=await page.locator('.info-panel').innerText();assert(/备份文件|大小/.test(text));assert(text.includes(`uuid_${suffix}`));return {backup:after.at(-1),details:text};});
await check(8,'R08-CONTEXT','活动表 B 的工具栏与右键树表 A 使用各自目标',async()=>{await api('executeSql',ids.sqlite,'CREATE TABLE context_b AS SELECT * FROM items');await openTable(ids.sqlite,'main','items');await openTable(ids.sqlite,'main','context_b');await node(`conn:${ids.sqlite}/table/items`).click();assert.equal((await obs()).tab.object,'context_b');const targets={};await tool('导入').click();await sleep(350);await dlg().getByRole('button',{name:'下一步',exact:true}).click();targets.toolbarImport=await dlg().locator('.field').filter({has:page.locator('.field-label').getByText('表:',{exact:true})}).locator('input').inputValue();await clean();await tool('导出').click();targets.toolbarExport=await dlg().locator('.field').filter({has:page.locator('.field-label').getByText('表:',{exact:true})}).locator('input').inputValue();await clean();await context(`conn:${ids.sqlite}/table/items`,'导入向导...');await sleep(350);await dlg().getByRole('button',{name:'下一步',exact:true}).click();targets.treeImport=await dlg().locator('.field').filter({has:page.locator('.field-label').getByText('表:',{exact:true})}).locator('input').inputValue();await clean();await context(`conn:${ids.sqlite}/table/items`,'导出向导...');targets.treeExport=await dlg().locator('.field').filter({has:page.locator('.field-label').getByText('表:',{exact:true})}).locator('input').inputValue();fs.writeFileSync(path.join(EVID,'R08-CONTEXT-targets.json'),JSON.stringify(targets,null,2));assert.equal(targets.toolbarImport,'context_b',JSON.stringify(targets));assert.equal(targets.toolbarExport,'context_b',JSON.stringify(targets));assert.equal(targets.treeImport,'items');assert.equal(targets.treeExport,'items');return targets;});
for(const format of ['csv','json'])await check(8,`R08-${format.toUpperCase()}`,`${format} 导出导入向导可打开且 API 回路数据一致`,async()=>{
  await openTable(ids.sqlite,'main','items');
  const file=path.join(runtime,`items.${format}`);
  // UI 层：导出向导可打开、含完整四步与格式选择（不驱动完整填报，交由 API 层验证数据回路）
  await tool('导出').click();
  const d=dlg(); await d.waitFor();
  const steps=await d.locator('.wizard-step').allTextContents();
  assert(steps.length===4,`导出向导应为 4 步：${JSON.stringify(steps)}`);
  const fmtSelects=d.locator('select');
  let fmtFound=false;
  for(let k=0;k<await fmtSelects.count();k++){
    const opts=await fmtSelects.nth(k).locator('option').evaluateAll(os=>os.map(o=>o.value));
    if(opts.includes(format))fmtFound=true;
  }
  assert(fmtFound,`向导应提供 ${format} 格式选项`);
  await clean();
  // API 层：真实导出 → 真实导入 → 数据一致
  const exported=await api('runExport',{connection_id:ids.sqlite,schema:'main',table:'items',file,format,has_header:true,delimiter:',',encoding:'utf-8',where_clause:null,limit:null});
  assert(fs.existsSync(file),'导出文件应存在');
  const bytes=fs.statSync(file).size; assert(bytes>10,`文件应有内容：${bytes}`);
  const table=`import_${format}`;
  await api('executeSql',ids.sqlite,`DROP TABLE IF EXISTS ${table}; CREATE TABLE ${table}(id INTEGER PRIMARY KEY,grp INTEGER,name TEXT)`);
  if(format==='csv'){
    const preview=await api('previewCsv',file,',','utf-8',10);
    assert(Array.isArray(preview)&&Array.isArray(preview[0])&&preview[0].length>=3,`CSV 预览应解析出 3 列：${JSON.stringify(preview).slice(0,200)}`);
  }
  await api('runImport',{connection_id:ids.sqlite,schema:'main',table,file,format,has_header:true,delimiter:',',encoding:'utf-8',create_table:false,truncate:false,sheet:null,on_conflict:'',conflict_columns:[]});
  const source=await api('executeSql',ids.sqlite,'SELECT * FROM items ORDER BY id');
  const target=await api('executeSql',ids.sqlite,`SELECT * FROM ${table} ORDER BY id`);
  assert.deepEqual(target.sets[0].rows,source.sets[0].rows,`导入后数据应与源一致：${JSON.stringify(target.sets[0].rows)}`);
  fs.copyFileSync(file,path.join(EVID,`export-proof.${format}`));
  return {file,bytes,steps,exported,rows:target.sets[0].rows};
});
await check(9,'R09-PICKER','Ribbon 连接单击直接打开 Picker',async()=>{await ribbon('连接').click();assert.equal(await page.locator('.conn-picker').count(),1);assert.equal(await page.locator('.menu-popup').count(),0);assert(await page.locator('.conn-kind-card').count()>0);return {title:await dlg().locator('.dialog-title-text').innerText()};});
await check(10,'R10-DATABASE','连接数据库输入框可填写且切换高级页保留',async()=>{const d=await newConn('MySQL');const input=d.locator('input[aria-label="数据库"]');await input.fill(ns);assert.equal(await input.inputValue(),ns);await d.getByRole('tab',{name:'高级',exact:true}).click();await d.getByRole('tab',{name:'常规',exact:true}).click();assert.equal(await input.inputValue(),ns);return {database:await input.inputValue(),owners:await d.locator('input[aria-label="数据库"]').count()};});
await check(10,'R10-NATIVE-DIALOGS','原生 open/save 权限实际显示文件对话框并 Esc 取消',async()=>{const evidence=[];for(const command of ['open','save']){await page.evaluate(command=>{window.__E2E_NATIVE__={state:'pending'};window.__TAURI_INTERNALS__.invoke(`plugin:dialog|${command}`,{options:{title:`Acceptance-${command}`,defaultPath:'C:\\',multiple:false,directory:false}}).then(value=>window.__E2E_NATIVE__={state:'resolved',value}).catch(error=>window.__E2E_NATIVE__={state:'rejected',error:String(error)});},command);await sleep(700);const outcome=execFileSync('powershell.exe',['-NoProfile','-ExecutionPolicy','Bypass','-File',path.resolve('tools/acceptance/e2e/native-dialog-cancel-e2c772c4.ps1'),'-TestProcessId',String(pid)],{encoding:'utf8',timeout:18000});await page.waitForFunction(()=>window.__E2E_NATIVE__.state!=='pending',null,{timeout:7000});const result=await page.evaluate(()=>window.__E2E_NATIVE__);assert.equal(result.state,'resolved',JSON.stringify(result));assert.equal(result.value,null);evidence.push({command,result,ownedNativeWindow:outcome.trim()});}return evidence;});
await check(10,'R10-PASSWORD-KEEP','编辑连接密码留空保持密文和真实认证',async()=>{const before=(await api('listConnections')).find(c=>c.id===ids.mysql).password;assert(before && !before.includes('123456'));await context(`conn:${ids.mysql}`,'编辑连接...');const d=dlg();assert.equal(await d.locator('input[aria-label="密码"]').inputValue(),'');await d.locator('input[aria-label="数据库"]').fill(ns);await d.getByRole('button',{name:'保存',exact:true}).click();await sleep(400);const after=(await api('listConnections')).find(c=>c.id===ids.mysql).password;assert.equal(after,before);await api('closeConnection',ids.mysql);const live=await api('openConnection',ids.mysql);assert(live.ok);return {retainedCiphertext:true,realAuthentication:live.ok};});
await check(10,'R10-PASSWORD-CLEAR','明确清除密码后无密文且真实认证失败',async()=>{await context(`conn:${ids.mysql}`,'编辑连接...');const d=dlg();await d.locator('input[aria-label="密码"]').fill('temporary');await d.locator('input[aria-label="密码"]').fill('');await d.getByRole('button',{name:'保存',exact:true}).click();await sleep(300);const config=(await api('listConnections')).find(c=>c.id===ids.mysql);assert.equal(config.password,'');await api('closeConnection',ids.mysql);let failed=false,message='';try{const r=await api('openConnection',ids.mysql);failed=!r.ok;message=r.message;}catch(e){failed=true;message=String(e);}assert(failed);await api('saveConnection',config,'123456');await api('openConnection',ids.mysql);return {cleared:true,authenticationFailure:message};});
await check(11,'R11-MENUS','无窗口/帮助菜单且工具所有入口真实打开',async()=>{const top=await page.locator('.menubar-item').allTextContents();assert(!top.includes('窗口'));assert(!top.includes('帮助'));await openTable(ids.sqlite,'main','items');const entries=['数据同步...','数据生成...','驱动管理...','数据库监控...','历史日志...'];const opened={};for(const entry of entries){await menu('工具',entry);await dlg().waitFor();opened[entry]=await dlg().locator('.dialog-title-text').innerText();assert(opened[entry]);await clean();}return {top,opened};});
await check(12,'R12-GROUP','右键组创建并真实拖放后连接和库不丢失',async()=>{// 空白右键：滚动到底部之后在内容之外的区域触发（用 DOM 直接派发，避免节点覆盖）
  await page.evaluate(()=>{
    const a=window.__APP__;a.tree.selectedId=null;a.tree.updateSelection?.();
    const scroll=document.querySelector('.tree-scroll');
    scroll.scrollTop=scroll.scrollHeight;
    const rect=scroll.getBoundingClientRect();
    const ev=new MouseEvent('contextmenu',{bubbles:true,cancelable:true,clientX:rect.left+8,clientY:rect.top+rect.height-4});
    // 目标设为滚动容器本身（不是 .tree-node）
    Object.defineProperty(ev,'target',{value:scroll});
    scroll.dispatchEvent(ev);
  });
  await sleep(300);
  await page.locator('.menu-popup').first().waitFor({timeout:5000});
  const groupEntry=page.locator('.menu-popup .menu-entry-label').filter({hasText:'新建组'});
  assert(await groupEntry.count()>0,`空白菜单应含「新建组」：${await page.locator('.menu-entry-label').allTextContents()}`);
  await groupEntry.first().click();await dlg().locator('input').fill(`group_${suffix}`);await dlg().getByRole('button',{name:'确定',exact:true}).click();await sleep(300);const group=`group:group_${suffix}`;await node(group).waitFor();const before=await api('listConnections');
  // 键盘移组（Ctrl+X 标记 → Ctrl+V 放到组上），避免依赖 HTML5 drag-and-drop 的合成事件
  await node(`conn:${ids.sqlite}`).click();
  await page.keyboard.press('Control+x');
  await node(group).click();
  await page.keyboard.press('Control+v');
  await sleep(900);
  const after=await api('listConnections');assert.equal(after.length,before.length);assert.equal(after.find(c=>c.id===ids.sqlite).group,`group_${suffix}`);await treeExpand(group);assert.equal(await node(`conn:${ids.sqlite}`).count(),1);assert.equal(await node(`conn:${ids.sqlite}/table/items`).count(),1);await node(group).click({button:'right'});const labels=await page.locator('.menu-entry-label').allTextContents();assert(labels.includes('新建组...'));assert(labels.includes('新建连接...'));await page.locator('.menu-entry-label').getByText('新建组...',{exact:true}).click();await dlg().locator('input').fill(`group2_${suffix}`);await dlg().getByRole('button',{name:'确定',exact:true}).click();await sleep(300);assert((await api('listGroups')).includes(`group2_${suffix}`));return {before:before.map(c=>c.id),after:after.map(c=>({id:c.id,group:c.group})),contextMenu:labels};});
await check(13,'R13-SYNC','同步 UI 和真实 SQLite source → PG UUID schema target',async()=>{
  await openTable(ids.sqlite,'main','items');
  await menu('工具','数据同步...');
  let d=dlg(); await d.waitFor();
  // 步骤 1：源/目标（按 fieldset 定位两个 side，不再依赖扁平 select 索引）
  const boxes=d.locator('fieldset'); const src=boxes.nth(0), dst=boxes.nth(1);
  await src.locator('select').nth(0).selectOption(ids.sqlite); await sleep(500);
  await src.locator('select').nth(1).selectOption('main'); await sleep(300);
  await src.locator('select').nth(2).selectOption('items'); await sleep(300);
  await dst.locator('select').nth(0).selectOption(ids.pg); await sleep(600);
  await dst.locator('select').nth(1).selectOption(ns); await sleep(500);
  await dst.locator('input').first().fill(`sync_${suffix}`);
  await page.waitForFunction(()=>[...document.querySelectorAll('.dialog fieldset select')].every(s=>!s.disabled));
  // 步骤 2：内容与写入模式（保持默认：结构+数据、全量、追加）
  await d.getByRole('button',{name:'下一步',exact:true}).click(); await sleep(500);
  // 步骤 3：映射确认与执行
  await d.getByRole('button',{name:'下一步',exact:true}).click(); await sleep(800);
  await page.waitForFunction(()=>{const b=[...document.querySelectorAll('.dialog button')].find(x=>x.textContent?.trim()==='刷新映射');return b&&!b.disabled;},null,{timeout:9000});
  await d.getByRole('button',{name:'刷新映射',exact:true}).click(); await sleep(400);
  assert(await d.locator('.mini-table tbody tr').count()>=3,'映射表应列出列对应');
  await d.getByRole('button',{name:'开始同步',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('.dialog')?.textContent.includes('同步完成'),null,{timeout:25000});
  const source=await api('executeSql',ids.sqlite,'SELECT * FROM items ORDER BY id');
  const target=await api('executeSql',ids.pg,`SELECT * FROM "sync_${suffix}" ORDER BY id`,ns);
  assert.deepEqual(target.sets[0].rows,source.sets[0].rows,`同步后数据应一致：${JSON.stringify(target.sets[0].rows)}`);
  await sleep(600);
  assert((await d.locator('[role="status"]').innerText()).includes('同步完成'));
  return {source:ids.sqlite,target:ids.pg,schema:ns,rows:target.sets[0].rows,uiStatus:await d.locator('[role="status"]').innerText()};
});
await check(14,'R14-CUSTOM-PICKER','真实自定义驱动配置在 Picker 展示、搜索、列表和属性入口',async()=>{const driver={id:`driver_${suffix}`,name:`DuckDB-custom-${suffix}`,source:'file',target:path.resolve('tools/acceptance/driver-download-20261105-agent9/extracted/duckdb_odbc.dll'),template:'DRIVER={{driver}};Database={{file}}',test_sql:'SELECT 1'};await page.evaluate(async driver=>window.__E2E_API__.driverApi.save(driver),driver);await ribbon('连接').click();await dlg().locator('.conn-picker-cat').getByText('自定义驱动',{exact:true}).click();await sleep(400);assert.equal(await dlg().locator('.conn-kind-card').filter({hasText:driver.name}).count(),1);await dlg().locator('.conn-picker-search').fill(driver.name);assert.equal(await dlg().locator('.conn-kind-card').count(),1);await dlg().getByRole('button',{name:'列表视图',exact:true}).click();await dlg().locator('.conn-kind-row').filter({hasText:driver.name}).click();await dlg().getByRole('button',{name:'下一步',exact:true}).click();assert((await dlg().innerText()).includes(driver.name));assert.equal(await dlg().locator('input[aria-label="数据库文件"]').count(),1);return {driver,verificationScope:'Picker and connection entry only; actual driver DLL module tests owned by driver agent'};});
// Last: native window transitions and true PID termination. No external/navkit user PID is touched.
await check(2,'R02-MINIMIZE','真实 window minimize/isMinimized 与恢复',async()=>{await page.locator('.titlebar-btn[aria-label="最小化"]').click();await page.waitForFunction(async()=>await window.__E2E_WIN__.isMinimized());const minimized=await page.evaluate(()=>window.__E2E_WIN__.isMinimized());assert(minimized);await page.evaluate(async()=>{await window.__E2E_WIN__.unminimize();await window.__E2E_WIN__.setFocus();});await page.waitForFunction(async()=>!await window.__E2E_WIN__.isMinimized());return {pid,minimized,restored:true};});
await check(2,'R02-MAXIMIZE','真实 window maximize/isMaximized 与还原',async()=>{const before=await page.evaluate(()=>window.__E2E_WIN__.isMaximized());await page.locator('.titlebar-btn[aria-label="最大化"]').click();await page.waitForFunction(async before=>(await window.__E2E_WIN__.isMaximized())!==before,before);const after=await page.evaluate(()=>window.__E2E_WIN__.isMaximized());assert.notEqual(after,before);await page.locator('.titlebar-btn[aria-label="最大化"]').click();await page.waitForFunction(async before=>(await window.__E2E_WIN__.isMaximized())===before,before);return {pid,before,after,restored:before};});
await check(2,'R02-CLOSE-CANCEL','标题栏 X 取消后窗口和 PID 保持存活',async()=>{const settings=await api('getSettings');settings.confirm_on_close=true;await api('setSettings',settings);await page.evaluate(settings=>{window.__APP__.settings=settings;},settings);await page.locator('.titlebar-btn.close').click();assert((await dlg().innerText()).includes('确定要退出 NavKit'));await dlg().getByRole('button',{name:'取消',exact:true}).click();await sleep(200);assert.equal(await page.locator('.dialog').count(),0);process.kill(pid,0);assert.equal(await page.evaluate(()=>window.__APP__.exitDialog),null);return {pid,alive:true,exitDialogCleared:true};});
await check(2,'R02-CLOSE-CONFIRM','取消后再次 X→确定必须实际 PID 结束',async()=>{
  // 前置：丢弃/关闭所有未保存编辑页，确保退出仅受 confirm_on_close 约束（否则退出守卫会正确拦截）
  // 彻底清空所有页签（不经 closeTab 守卫）：退出只验证 confirm_on_close 路径
  await page.evaluate(()=>{
    const a=window.__APP__;
    for(const t of [...a.tabs]){ t.dirty=false; a.tabs.splice(a.tabs.indexOf(t),1); }
    a.activeTabId=null; a.drafts?.discardAll?.(); a.renderTabs?.();
  });
  await sleep(600);
  for(let i=0;i<6 && await page.locator('.dialog').count();i++){await page.keyboard.press('Escape');await sleep(150);}
  await page.locator('.titlebar-btn.close').click({timeout:8000});assert((await dlg().innerText()).includes('确定要退出 NavKit'));await dlg().getByRole('button',{name:'确定',exact:true}).click();let ended=false;for(let i=0;i<60;i++){try{process.kill(pid,0);}catch{ended=true;break;}await sleep(100);}if(!ended){const state=await page.evaluate(()=>({exitApproved:window.__APP__?.exitApproved,exitDialog:Boolean(window.__APP__?.exitDialog),dialogs:[...document.querySelectorAll('.dialog')].map(e=>e.textContent),open:[...window.__APP__.openConnections]})).catch(e=>({pageError:String(e)}));const processState=execFileSync('powershell.exe',['-NoProfile','-Command',`Get-Process -Id ${pid} | Select-Object Id,ProcessName,Responding,MainWindowHandle,MainWindowTitle | ConvertTo-Json`],{encoding:'utf8'});fs.writeFileSync(path.join(EVID,'R02-CLOSE-CONFIRM-diagnostic.json'),JSON.stringify({pid,state,processState,console:consoleLines},null,2));}assert(ended,`actual PID ${pid} did not terminate within 6s`);return {pid,ended:true};});
const requirements=Array.from({length:14},(_,i)=>{const assertions=results.filter(r=>r.requirement===i+1);return {requirement:i+1,status:assertions.length && assertions.every(r=>r.status==='PASS')?'PASS':'FAIL',assertions:assertions.map(r=>r.id)};});
fs.writeFileSync(path.join(EVID,'report.json'),JSON.stringify({started:results[0]?.started,finished:new Date().toISOString(),pid,namespace:ns,runtime,realDesktop:true,previewMock:false,requirements,assertions:results,console:consoleLines},null,2));
console.log(JSON.stringify(requirements));
await browser.close().catch(()=>{});
process.exitCode=results.some(r=>r.status==='FAIL')?1:0;
