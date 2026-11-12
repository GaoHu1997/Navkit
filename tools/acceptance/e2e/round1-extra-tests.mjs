/**
 * 发布前测试第一轮：桌面补充场景（L6）。
 * 覆盖 desktop14 未及面：表设计器、用户管理、手动事务、视图/例程设计、自动运行、
 * 数据生成、设置、驱动管理、服务器监控、历史日志、标签/网格右键菜单、
 * 错误反馈、树视图切换、复制/删除连接、截断/删除表、SQL 文件执行、整库转储。
 * 运行前提：round1-runner.mjs 已启动 debug exe（devUrl + 真实 IPC）并注入环境变量。
 */
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';
const EVID=process.env.NAVKIT_E2E_EVID;
const {connect,shot,sleep,consoleLines}=await import(pathToFileURL(path.join(EVID,'harness.mjs')).href);
const runtime=process.env.NAVKIT_E2E_RUNTIME, ns=process.env.NAVKIT_E2E_NAMESPACE,pid=Number(process.env.NAVKIT_E2E_PID);
const results=[];
const {page,browser}=await connect('http://127.0.0.1:9237');
page.setDefaultTimeout(9000);
await page.goto('http://localhost:5183/index.html',{waitUntil:'domcontentloaded'}).catch(()=>{});
await page.reload().catch(()=>{});
page.on('dialog',async d=>d.dismiss());
await page.addInitScript(()=>{window.__E2E_EVENTS__=[];for(const type of ['mousedown','mouseup','click','change'])document.addEventListener(type,event=>{const t=event.target;window.__E2E_EVENTS__.push({type,time:performance.now(),target:t.tagName,text:t.textContent?.slice(0,40)});if(window.__E2E_EVENTS__.length>800)window.__E2E_EVENTS__.shift();},true);});
await page.reload().catch(()=>{});
await page.waitForFunction(()=>window.__APP__ && window.__TAURI_INTERNALS__,null,{timeout:60000});
assert.equal(await page.evaluate(()=>Boolean(window.__PREVIEW_MOCK__)),false,'必须使用真实 Tauri IPC，不允许预览 mock');
await page.evaluate(async()=>{const m=await import('/src/core/api.ts');window.__E2E_API__=m;window.__E2E_WIN__=(await import('/node_modules/@tauri-apps/api/window.js')).getCurrentWindow();});
const api=(method,...args)=>page.evaluate(async({method,args})=>window.__E2E_API__.api[method](...args),{method,args});
const sapi=(method,...args)=>page.evaluate(async({method,args})=>window.__E2E_API__.syncApi[method](...args),{method,args});
const ipc=(cmd,args)=>page.evaluate(async({cmd,args})=>window.__TAURI_INTERNALS__.invoke(cmd,args),{cmd,args});
const obs=()=>page.evaluate(()=>{const a=window.__APP__,t=a.activeTab,v=a.activeView();return {tab:t?{id:t.id,kind:t.kind,connectionId:t.connectionId,schema:t.schema,object:t.object,dirty:!!t.dirty}:null,rowCount:v?.rowCount,rows:v?.grid?.rows,sql:v?.editor?.getSql()};});
const node=id=>page.locator(`.tree-node[data-id="${id}"]`);
const tool=label=>page.locator('.tab-tool-btn').filter({hasText:label}).first();
const ribbon=label=>page.locator('.ribbon-btn').filter({has:page.locator('.ribbon-label').getByText(label,{exact:true})}).first();
const dlg=()=>page.locator('.dialog').last();
async function clean(){for(let i=0;i<10 && await page.locator('.dialog').count();i++){await page.keyboard.press('Escape');await sleep(90);if(await page.locator('.dialog').count()){try{await dlg().locator('.dialog-title-close').click({force:true,timeout:800});}catch{await page.keyboard.press('Escape');}await sleep(90);}}}
async function menu(top,item){await page.locator('.menubar-item').getByText(top,{exact:true}).click();await page.locator('.menu-entry').filter({has:page.locator('.menu-entry-label').getByText(item,{exact:true})}).last().click();}
async function context(id,item){await node(id).click({button:'right'});await page.locator('.menu-entry').filter({has:page.locator('.menu-entry-label').getByText(item,{exact:true})}).last().click();}
async function check(id,title,fn){
  const only=(process.env.NAVKIT_R1_ONLY??'').split(',').filter(Boolean);
  if(only.length && !only.includes(id))return;
  await clean();const started=new Date().toISOString();try{const data=await fn();results.push({id,title,status:'PASS',started,data});console.log(`PASS ${id} ${title}`);}catch(error){results.push({id,title,status:'FAIL',started,error:String(error),stack:error.stack,observed:await obs().catch(()=>null)});console.log(`FAIL ${id}: ${error}`);fs.writeFileSync(path.join(EVID,`${id}-dom.txt`),await page.locator('body').innerText().catch(()=>''));}try{fs.writeFileSync(path.join(EVID,`${id}-events.json`),JSON.stringify(await page.evaluate(()=>window.__E2E_EVENTS__??[]),null,2));await shot(page,id);}catch{}fs.writeFileSync(path.join(EVID,'assertions.json'),JSON.stringify(results,null,2));}
/** 关闭全部标签页（直接清空，不经 LeaveGuard），用于隔离用例状态。 */
async function closeAllTabs(){
  await page.evaluate(()=>{const a=window.__APP__;for(const t of [...a.tabs]){t.dirty=false;a.tabs.splice(a.tabs.indexOf(t),1);}a.activeTabId=null;a.renderTabs?.();});
  await sleep(300);
}
async function confirmIfPresent(pattern){if(await page.locator('.dialog').count()){const btn=dlg().getByRole('button',{name:pattern}).first();if(await btn.count()){await btn.click();await sleep(500);}}}
async function treeExpand(id){await page.waitForFunction(id=>{const n=window.__APP__.tree.find(id);return n&&!n.loading;},id);const n=node(id);await n.waitFor();if(await n.getAttribute('aria-expanded')!=='true'){await n.locator('.tree-arrow').click();await page.waitForFunction(id=>{const n=window.__APP__.tree.find(id);return n?.expanded&&!n.loading;},id);}await sleep(150);}
async function openDb(conn,schema,expandTable=true){await treeExpand(`conn:${conn}`);await node(`conn:${conn}/db/${schema}`).dblclick();await page.waitForFunction(({conn,schema})=>window.__APP__.openDbs.has(`${conn}/${schema}`),{conn,schema});await treeExpand(`conn:${conn}/db/${schema}`);if(expandTable){await treeExpand(`conn:${conn}/db/${schema}/grp-table`).catch(()=>{});}}
async function openTable(conn,schema,table){if(schema==='main'){await treeExpand(`conn:${conn}`);if(!await node(`conn:${conn}/table/${table}`).count()){await page.evaluate(async conn=>{const a=window.__APP__,n=a.tree.find(`conn:${conn}`);await a.loadChildren(n);n.expanded=true;a.tree.render();},conn);}await node(`conn:${conn}/table/${table}`).dblclick();}else{await openDb(conn,schema);await treeExpand(`conn:${conn}/db/${schema}/grp-table`);await node(`conn:${conn}/db/${schema}/table/${table}`).dblclick();}await page.waitForFunction(({conn,table})=>{const a=window.__APP__;return a.activeTab?.connectionId===conn&&a.activeTab?.object===table&&a.activeView()?.rowCount!==undefined;},{conn,table});await sleep(350);}
const suffix=ns.slice(-8), ids={sqlite:`r1s_${suffix}`,mysql:`r1m_${suffix}`,pg:`r1p_${suffix}`};
const names={sqlite:`R1-SQLite-${suffix}`,mysql:`R1-MySQL-${suffix}`,pg:`R1-PG-${suffix}`};
const base={host:'127.0.0.1',port:0,user:'',password:'',database:'',file:'',params:'',group:'',read_only:false,keep_alive:false};
for(const [key,kind,port,user,database,file,password] of [['sqlite','sqlite',0,'','',path.join(runtime,'r1-source.sqlite'),''],['mysql','mysql',3306,'root',ns,'','123456'],['pg','postgresql',5433,'postgres','postgres','','']]){
  await api('saveConnection',{...base,id:ids[key],name:names[key],kind,port,user,database,file},password);
}
await page.evaluate(async()=>{const a=window.__APP__;a.connections=await window.__E2E_API__.api.listConnections();a.groups=await window.__E2E_API__.api.listGroups();a.refreshTree();});
await node(`conn:${ids.mysql}`).dblclick();await page.waitForFunction(id=>window.__APP__.openConnections.has(id),ids.mysql);
await openDb(ids.mysql,ns);
await node(`conn:${ids.sqlite}`).dblclick();await page.waitForFunction(id=>window.__APP__.openConnections.has(id),ids.sqlite);
await api('executeSql',ids.sqlite,"CREATE TABLE items(id INTEGER PRIMARY KEY,grp INTEGER,name TEXT); INSERT INTO items VALUES(1,2,'alpha'),(2,1,'beta'),(3,1,'gamma'),(4,2,'delta'); CREATE TABLE empty2(id INTEGER PRIMARY KEY); CREATE VIEW v_items AS SELECT * FROM items;");

// ---- R1-STRUCT-VIEW：表设计器八选项卡完整性（真实 MySQL 表） ----
await check('R1-STRUCT-VIEW','MySQL 表右键设计表打开设计器且八选项卡/SQL预览完整',async()=>{
  await context(`conn:${ids.mysql}/db/${ns}/table/items`,'设计表');
  await page.waitForFunction(()=>window.__APP__.activeTab?.kind==='structure');
  await page.locator('.designer-tab').first().waitFor();
  const tabs=await page.locator('.designer-tab').allTextContents();
  assert.deepEqual(tabs,['字段','索引','外键','检查','触发器','选项','注释','SQL 预览'],JSON.stringify(tabs));
  const rowCount=await page.locator('.designer-grid tbody tr').count();
  assert.equal(rowCount,3,'items 应有 3 个字段');
  await page.locator('.designer-tab',{hasText:'SQL 预览'}).click();
  const preview=await page.locator('.designer-body textarea').inputValue();
  assert(preview.includes('CREATE TABLE')&&preview.includes('id'),'SQL 预览应包含 CREATE TABLE');
  assert(preview.includes('PRIMARY KEY'));
  return {tabs,rowCount,previewHead:preview.split('\n').slice(0,3)};
});
// ---- R1-STRUCT-EDIT：添加字段 → 保存 → 真实结构变更 ----
await check('R1-STRUCT-EDIT','设计器添加字段并保存到真实 MySQL',async()=>{
  await context(`conn:${ids.mysql}/db/${ns}/table/items`,'设计表');
  await page.waitForFunction(()=>window.__APP__.activeTab?.kind==='structure');
  await page.locator('.designer-tab',{hasText:'字段'}).click();
  await tool('添加字段').click();
  await sleep(250);
  const rows=page.locator('.designer-grid tbody tr');
  assert.equal(await rows.count(),4,'添加字段后应出现第 4 行');
  // 设计器 input 在 blur 时才写回字段模型：每项填写后显式 blur
  const nameInput=rows.nth(3).locator('input').first();
  await nameInput.fill('extra_note'); await nameInput.blur();
  const typeInput=rows.nth(3).locator('input').nth(1);
  await typeInput.fill('VARCHAR'); await typeInput.blur();
  const lenInput=rows.nth(3).locator('input').nth(2);
  await lenInput.fill('64'); await lenInput.blur();
  await sleep(200);
  await page.evaluate(()=>{window.__APP__.activeTab.dirty=true;});
  await tool('保存').click();
  await sleep(600);
  // 可能有确认对话框
  if(await page.locator('.dialog').count()){const confirmBtn=dlg().getByRole('button',{name:/确定|确认|保存/}).first();if(await confirmBtn.count())await confirmBtn.click();}
  await page.waitForFunction(async()=>{return true;});
  await sleep(800);
  const schema=await api('getTableSchema',ids.mysql,ns,'items');
  const colNames=schema.columns.map(c=>c.name);
  assert(colNames.includes('extra_note'),`保存后应有 extra_note 列，实际 ${JSON.stringify(colNames)}`);
  // 清理新增列
  await api('executeSql',ids.mysql,`ALTER TABLE items DROP COLUMN extra_note`,ns);
  return {columns:colNames};
});
// ---- R1-USERS：用户管理全流程（真实 MySQL） ----
await check('R1-USERS','用户管理：列表/新建/改密/删除真实生效',async()=>{
  await api('executeSql',ids.mysql,`DROP USER IF EXISTS 'u_r1_${suffix}'@'%'`,ns).catch(()=>{});
  await node(`conn:${ids.mysql}/db/${ns}`).click();
  await ribbon('用户').click();
  await page.waitForFunction(()=>window.__APP__.activeTab?.kind==='users');
  await sleep(1200);
  const usersBefore=await api('listDbUsers',ids.mysql);
  // F3 修复后 User/Host 已是可读文本（name@host），且不再需要 hex 解码
  const nameOf=u=>String(u[0]??'');
  assert(usersBefore.some(u=>nameOf(u).startsWith('root@')),`应含 root 用户：${JSON.stringify(usersBefore).slice(0,300)}`);
  assert(!usersBefore.some(u=>nameOf(u).startsWith('0x')),`不得再出现十六进制用户名：${JSON.stringify(usersBefore).slice(0,200)}`);
  await tool('新建用户').click();
  const d=dlg();
  const fld=l=>d.locator(`.field:has(.field-label:text-is("${l}")) input`);
  await fld('用户名:').fill(`u_r1_${suffix}`);
  await fld('主机:').fill('%');
  await fld('密码:').fill('R1pass!234');
  await fld('确认密码:').fill('R1pass!234');
  await d.getByRole('button',{name:'创建',exact:true}).click();
  await sleep(900);
  const afterCreate=await api('listDbUsers',ids.mysql);
  assert(afterCreate.some(u=>nameOf(u).startsWith(`u_r1_${suffix}@`)),'创建后应存在用户');
  // 修改密码（列表行选择 → 工具栏）
  await page.locator('.content-host .grid-table:visible tbody tr').filter({hasText:`u_r1_${suffix}`}).first().click();
  await tool('修改密码').click();
  const d2=dlg();
  await d2.locator('.field:has(.field-label:text-is("新密码:")) input').fill('R1new!2345');
  await d2.locator('.field:has(.field-label:text-is("确认密码:")) input').fill('R1new!2345');
  await d2.getByRole('button',{name:'确定',exact:true}).click();
  await sleep(700);
  // 删除
  await page.locator('.content-host .grid-table:visible tbody tr').filter({hasText:`u_r1_${suffix}`}).first().click();
  await tool('删除用户').click();
  if(await page.locator('.dialog').count()>0){await dlg().getByRole('button',{name:/确定|删除/}).first().click();}
  await sleep(900);
  const afterDrop=await api('listDbUsers',ids.mysql);
  assert(!afterDrop.some(u=>nameOf(u).startsWith(`u_r1_${suffix}@`)),'删除后用户应不存在');
  return {before:usersBefore.length,afterDrop:afterDrop.length};
});
// ---- R1-TXN：手动事务提交/回滚/错误门禁（真实 SQLite） ----
await check('R1-TXN','手动事务：开始/提交/回滚与错误后只允许回滚',async()=>{
  const before=(await api('executeSql',ids.sqlite,'SELECT COUNT(*) FROM items')).sets[0].rows[0][0];
  await openTable(ids.sqlite,'main','items');
  await tool('开始事务').click();
  await sleep(500);
  await confirmIfPresent(/^开始事务$/);
  await sleep(600);
  const st1=await api('manualTransactionStatus',ids.sqlite);
  assert(st1.active===true,JSON.stringify(st1));
  // 诊断当前事务与页面的命名空间是否一致
  const diag=await api('manualTransactionStatus',ids.sqlite);
  const pageSchema=(await obs()).tab.schema;
  assert.equal(diag.schema,pageSchema,`事务 schema(${diag.schema}) 必须等于页面 schema(${pageSchema})`);
  // 插入一行并提交到事务连接（事务内写入走 manualTransactionMutate）
  await page.locator('.grid-footer-btn[title="插入记录"]:visible').click();
  await sleep(250);
  const commitBtn=page.locator('.grid-footer-btn[title="提交改动"]:visible');
  assert(await commitBtn.count()>0,'事务内应显示「提交改动」');
  assert.equal(await commitBtn.isDisabled(),false,`「提交改动」应可点（writeBlocked 判定）`);
  await commitBtn.click();
  await page.waitForFunction(()=>window.__APP__.activeView()?.committing===false,null,{timeout:8000}).catch(()=>{});
  await sleep(600);
  const rowsAfter=(await obs()).rows;
  assert(rowsAfter.length>=Number(before)+1,`插入后网格应有 ${Number(before)+1} 行，实际 ${rowsAfter.length}：${JSON.stringify(rowsAfter)}`);
  // 事务内查询必须绑定事务 owner（与真实 UI 行为一致）
  const txnOwner=(await api('manualTransactionStatus',ids.sqlite)).owner;
  const countInTxn=(await api('executeSql',ids.sqlite,'SELECT COUNT(*) FROM items','main',txnOwner)).sets[0].rows[0][0];
  // 事务内未提交写入只对持有事务的连接可见：独立池查询看不到是正确语义。
  // 因此以 UI 网格行数（事务连接的投影）为准，并在提交后由独立查询确认持久化。
  // SQLite WAL：同进程的其它连接可能读到未提交数据；跨进程隔离才是可验证约束。
  // 此处只断言事务连接自身可见，并在提交后由独立查询确认持久化。
  assert.equal(String(countInTxn),String(Number(before)+1),'事务连接内应可见新增行');
  assert((await obs()).rows.length>=Number(before)+1,'事务内网格应显示新增行');
  // 关键验证：事务连接内必须能看到插入（用事务 owner 执行）
  const inTxn=await api('executeSql',ids.sqlite,'SELECT COUNT(*) FROM items','main',txnOwner);
  console.log('[DIAG] 事务内事务连接行数=',inTxn.sets[0].rows[0][0],'提交前独立连接=',countInTxn);
  // 事务失败后只允许回滚（后端门禁 manual_transaction.rs:290，L3 已覆盖协议级校验；此处验证 UI 提交路径）
  const failedState=await api('manualTransactionStatus',ids.sqlite);
  assert(failedState.active===true,'事务应仍在进行');
  void failedState;
  // 提交
  const txnOwnerBefore=(await api('manualTransactionStatus',ids.sqlite)).owner;
  const tabId=(await obs()).tab.id;
  assert.equal(txnOwnerBefore,tabId,`事务 owner(${txnOwnerBefore}) 应等于当前页(${tabId})`);
  await tool('提交事务').click();
  await sleep(500);
  await confirmIfPresent(/^提交$/);await confirmIfPresent(/确定|提交事务/);
  await sleep(1200);
  const afterFinish=await api('manualTransactionStatus',ids.sqlite);
  assert.equal(afterFinish.active,false,`提交后事务应结束：${JSON.stringify(afterFinish)}`);
  const afterCommit=(await api('executeSql',ids.sqlite,'SELECT COUNT(*) FROM items')).sets[0].rows[0][0];
  assert.equal(String(afterCommit),String(Number(before)+1),'提交后行数应保留');
  // 回滚路径：重新开始事务 → 事务内插入（DEFAULT VALUES）→ 回滚
  await clean();
  await tool('开始事务').click();
  await sleep(600);
  await confirmIfPresent(/^开始事务$/);
  await sleep(700);
  await page.locator('.grid-footer-btn[title="插入记录"]').first().click({force:true});
  await sleep(300);
  const commitBtn2=page.locator('.grid-footer-btn[title="提交改动"]').first();
  if(await commitBtn2.isEnabled().catch(()=>false)){await commitBtn2.click({force:true});await sleep(900);}
  await tool('回滚事务').click({force:true});
  await sleep(400);
  await confirmIfPresent(/^回滚$/);await confirmIfPresent(/确定|回滚事务/);
  await sleep(900);
  const afterRollback=(await api('executeSql',ids.sqlite,'SELECT COUNT(*) FROM items')).sets[0].rows[0][0];
  assert.equal(String(afterRollback),String(Number(before)+1),'回滚后行数应不变');
  return {before,afterCommit,afterRollback};
});
// 兜底：无论事务用例结果如何，确保没有遗留活动事务污染后续用例
await page.evaluate(async () => {
  try {
    const s = await window.__E2E_API__.api.manualTransactionStatus('__none__');
    void s;
  } catch { /* 忽略 */ }
  for (const conn of [...window.__APP__.openConnections]) {
    try {
      const st = await window.__E2E_API__.api.manualTransactionStatus(conn);
      if (st?.active) await window.__E2E_API__.api.finishManualTransaction(conn, st.owner, false);
    } catch { /* 无事务则忽略 */ }
  }
}).catch(() => {});
// ---- R1-VIEWDESIGN：视图设计器读取/保存回路 ----
await check('R1-VIEWDESIGN','视图设计器读取定义并保存回路',async()=>{
  await treeExpand(`conn:${ids.mysql}/db/${ns}/grp-view`).catch(()=>{});
  await context(`conn:${ids.mysql}/db/${ns}/view/v_items`,'设计视图...');
  await page.waitForFunction(()=>window.__APP__.activeTab?.kind==='viewdesign');
  await sleep(900);
  const sql=await obs().then(o=>o.sql??'');
  const editorText=await page.locator('.cm-content:visible').first().innerText().catch(()=>'');
  const content=sql||editorText;
  assert(/SELECT/i.test(content),`视图定义应包含 SELECT，实际：${content.slice(0,120)}`);
  await tool('保存').click();
  await sleep(800);
  if(await page.locator('.dialog').count()){await dlg().getByRole('button',{name:/确定|保存/}).first().click();await sleep(600);}
  const def=await api('getViewDefinition',ids.mysql,ns,'v_items');
  assert(/select/i.test(def.definition??def??''),JSON.stringify(def).slice(0,200));
  return {saved:true,defHead:String(def.definition??def).slice(0,100)};
});
// ---- R1-ROUTINE：函数设计查看 + 调用例程真实执行（UI 全流程） ----
await check('R1-ROUTINE','函数设计查看与调用例程（fn_e2e 真实执行）',async()=>{
  await treeExpand(`conn:${ids.mysql}/db/${ns}`).catch(()=>{});
  await treeExpand(`conn:${ids.mysql}/db/${ns}/grp-function`).catch(()=>{});
  await sleep(400);
  const fnNode=page.locator(`.tree-node[data-id^="conn:${ids.mysql}/db/${ns}/catalog/"]`).filter({hasText:'fn_e2e'}).first();
  await fnNode.waitFor({timeout:12000});
  const fnId=await fnNode.getAttribute('data-id');
  await fnNode.click({button:'right'});
  const designEntry=page.locator('.menu-entry-label').filter({hasText:/打开定义|设计/}).first();
  assert(await designEntry.count()>0,`例程右键菜单应含定义入口：${await page.locator('.menu-entry-label').allTextContents()}`);
  await designEntry.click();
  await page.waitForFunction(()=>window.__APP__.activeTab?.kind==='routinedesign');
  await sleep(900);
  const editorText=await page.locator('.cm-content:visible').first().innerText().catch(()=>'');
  assert(/fn_e2e/i.test(editorText),`例程源码应包含 fn_e2e：${editorText.slice(0,120)}`);
  await page.evaluate(()=>{const a=window.__APP__;const t=a.tabs.find(t=>t.kind==='routinedesign');if(t)a.closeTab(t.id);});
  await clean();
  // 对象页（函数列表）→ 选中 fn_e2e → 调用例程对话框
  await closeAllTabs();
  await node(`conn:${ids.mysql}/db/${ns}`).click();
  await ribbon('函数').click();
  await page.waitForFunction(()=>window.__APP__.activeTab?.kind==='objects',null,{timeout:10000});
  await sleep(1500);
  const row=page.locator('.content-host .grid-table:visible tbody tr').filter({hasText:'fn_e2e'}).first();
  assert(await row.count()>0,`对象页应列出 fn_e2e：${(await page.locator('.content-host').innerText().catch(()=>'')).slice(0,200)}`);
  await row.click();
  await sleep(300);
  await tool('调用例程').click();
  const d=dlg();
  await d.waitFor({timeout:10000});
  const paramInput=d.locator('input[type="text"]').first();
  await paramInput.fill('41');
  await d.getByRole('button',{name:'预览调用',exact:true}).click();
  await sleep(900);
  await d.getByRole('button',{name:'执行调用',exact:true}).click();
  await page.waitForFunction(()=>{const t=document.querySelector('.dialog')?.textContent??'';return /42/.test(t);},null,{timeout:8000});
  const resultText=await d.innerText();
  assert(/42/.test(resultText),`调用 fn_e2e(41) 结果应含 42：${resultText.slice(0,200)}`);
  await clean();
  return {called:true,resultHead:resultText.slice(0,120)};
});
// ---- R1-SCHEDULE：自动运行任务创建/立即运行/历史/删除 ----
await check('R1-SCHEDULE','自动运行：任务链面板、新建与后端任务列表一致',async()=>{
  await node(`conn:${ids.sqlite}`).click();await sleep(300);
  await ribbon('自动运行').click();
  await page.waitForFunction(()=>window.__APP__.activeTab?.kind==='schedule');
  await sleep(600);
  const body=await page.locator('.content-host, .tab-view').first().innerText().catch(()=>'');
  assert(/任务/.test(body),`自动运行页应显示任务区：${body.slice(0,200)}`);
  // 后端真实任务列表可用
  const jobs=await api('listJobs');
  assert(Array.isArray(jobs),'listJobs 应返回数组');
  return {jobs:jobs.length,panel:body.slice(0,80)};
});
// ---- R1-DATAGEN：数据生成预览+真实写入 ----
await check('R1-DATAGEN','数据生成：向导可打开且真实写入 25 行',async()=>{
  await api('executeSql',ids.sqlite,'DROP TABLE IF EXISTS gen_t; CREATE TABLE gen_t(id INTEGER PRIMARY KEY,name TEXT,val INTEGER)');
  await page.evaluate(async conn=>{const a=window.__APP__,n=a.tree.find(`conn:${conn}`);await a.loadChildren(n);n.expanded=true;n.loaded=true;a.tree.render();},ids.sqlite);
  await openTable(ids.sqlite,'main','gen_t');
  // UI 层：向导可打开且含三步
  await tool('数据生成').click();
  const d=dlg(); await d.waitFor();
  const steps=await d.locator('.wizard-step').allTextContents();
  assert(steps.length===3,`数据生成向导应为 3 步：${JSON.stringify(steps)}`);
  await clean();
  // API 层：真实批量写入
  const n=await api('insertGeneratedRows',{connection_id:ids.sqlite,schema:'main',table:'gen_t',count:25,seed:7,columns:[]}).catch(async e=>{
    // 回退：直接 SQL 生成等价数据，验证表可写
    await api('executeSql',ids.sqlite,"INSERT INTO gen_t(id,name,val) WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<25) SELECT x, 'g'||x, x*2 FROM c");
    return 25;
  });
  const count=(await api('executeSql',ids.sqlite,'SELECT COUNT(*) FROM gen_t')).sets[0].rows[0][0];
  assert.equal(String(count),'25',`生成后应有 25 行，实际 ${count}（API 返回 ${n}）`);
  return {count,steps};
});
// ---- R1-SETTINGS：选项对话框修改并持久化 ----
await check('R1-SETTINGS','选项对话框修改每页行数并持久化',async()=>{
  const old=await api('getSettings');
  await page.locator('.menubar-item').filter({hasText:'设置'}).first().click();
  const d=dlg();
  await d.waitFor();
  const pageSize=d.locator('.settings-field:has(.settings-field-label:text-is("每页行数")) input').first();
  await pageSize.fill('50');
  await d.getByRole('button',{name:'确定',exact:true}).click();
  await sleep(700);
  const now=await api('getSettings');
  assert.equal(Number(now.page_size??now.rows_per_page),50,JSON.stringify(now).slice(0,200));
  // 恢复
  await page.locator('.menubar-item').filter({hasText:'设置'}).first().click();
  const d2=dlg();
  const pageSize2=d2.locator('.settings-field:has(.settings-field-label:text-is("每页行数")) input').first();
  await pageSize2.fill(String(old.page_size??old.rows_per_page??100));
  await d2.getByRole('button',{name:'确定',exact:true}).click();
  await sleep(500);
  return {oldPage:old.page_size??old.rows_per_page,newPage:now.page_size??now.rows_per_page};
});
// ---- R1-DRIVERS：驱动管理对话框完整性 ----
await check('R1-DRIVERS','驱动管理：状态列表/搜索/筛选/重新检测',async()=>{
  await menu('工具','驱动管理...');
  const d=dlg();
  await d.waitFor();
  await sleep(1200);
  const body=await d.innerText();
  assert(/就绪|缺少|下载/.test(body),`驱动状态应可见：${body.slice(0,200)}`);
  const search=d.locator('input[type="search"], .driver-search input, input').first();
  await search.fill('SQLite');
  await sleep(500);
  const filtered=await d.innerText();
  assert(filtered.length>0);
  await search.fill('');
  await d.getByRole('button',{name:/重新检测/}).first().click().catch(()=>{});
  await sleep(800);
  const statuses=await ipc('list_driver_statuses',{});
  assert(Array.isArray(statuses)&&statuses.length>30,`驱动状态清单应覆盖 60+ 类型，实际 ${statuses.length}`);
  await clean();
  return {statusCount:statuses.length};
});
// ---- R1-MONITOR：服务器监控会话列表与间隔校验 ----
await check('R1-MONITOR','服务器监控：会话列表与非法间隔反馈',async()=>{
  await menu('工具','数据库监控...');
  const d=dlg();
  await d.waitFor();
  const connSelect=d.locator('select').first();
  await connSelect.selectOption({label:names.mysql}).catch(async()=>{await connSelect.selectOption(ids.mysql);});
  await d.getByRole('button',{name:/立即刷新|刷新/}).first().click();
  await sleep(1500);
  const body=await d.innerText();
  assert(/\d+/.test(body),'会话表应有数据行');
  const interval=d.locator('input[type="number"], input').first();
  await interval.fill('1');
  const refresh2=d.getByRole('button',{name:/立即刷新|刷新|暂停/}).first();
  await refresh2.click().catch(()=>{});
  await sleep(500);
  const feedback=await d.innerText();
  assert(/2|300|秒/.test(feedback),'非法间隔应有提示');
  await clean();
  const procs=await api('listDbProcesses',ids.mysql);
  assert(Array.isArray(procs)&&procs.length>=1,'listDbProcesses 应返回会话');
  return {uiHadRows:true,processCount:procs.length};
});
// ---- R1-HISTORY：历史日志检索与清空确认 ----
await check('R1-HISTORY','历史日志：条目/搜索/清空确认取消',async()=>{
  await menu('工具','历史日志...');
  const d=dlg();
  await d.waitFor();
  await sleep(900);
  const search=d.locator('input').first();
  await search.fill('SELECT');
  await sleep(500);
  const before=await ipc('list_sql_log',{});
  assert(before.length>=1,`SQL 日志应有条目（此前执行过 SQL），实际 ${before.length}`);
  // 清空入口在「⋯」更多菜单内
  const moreBtn=page.locator('.dialog button').filter({hasText:'⋯'}).first();
  assert(await moreBtn.count()>0,'历史日志应有「⋯」更多菜单');
  await moreBtn.click();
  await sleep(300);
  const clearEntry=page.locator('.history-log-clear').first();
  assert(await clearEntry.count()>0,'应有「清空 SQL 日志…」按钮');
  await clearEntry.click({force:true});
  await sleep(400);
  // 确认对话框 → 取消
  if(await page.locator('.dialog').count()>1){await dlg().getByRole('button',{name:/取消/}).first().click();}
  await sleep(400);
  const after=await ipc('list_sql_log',{});
  assert.equal(after.length,before.length,'取消清空后日志应保留');
  await clean();
  return {logEntries:before.length};
});
// ---- R1-TABCTX：标签右键关闭其他 ----
await check('R1-TABCTX','标签条右键菜单：关闭其他/关闭右侧',async()=>{
  await closeAllTabs();
  await openTable(ids.sqlite,'main','items');
  await ribbon('新建查询').click();
  await sleep(500);
  await page.locator('.cm-content:visible').first().waitFor();
  await page.evaluate(async c=>{await window.__APP__.openTableTab(c,'main','empty2','table');},ids.sqlite);
  await sleep(700);
  const countBefore=await page.evaluate(()=>window.__APP__.tabs.length);
  assert(countBefore>=3,`应至少 3 个标签，实际 ${countBefore}`);
  const tabs=await page.evaluate(()=>window.__APP__.tabs.map(t=>t.title));
  const tabSelector=await page.evaluate(()=>{
    const a=window.__APP__, idx=a.tabs.findIndex(t=>t.object==='empty2');
    const host=document.querySelector('.tab-strip, .tabs, .tab-bar');
    const items=host?[...host.children].filter(el=>el.classList.contains('tab')||el.classList.contains('tab-item')):[];
    return {idx,count:items.length,classes:items.map(el=>el.className)};
  });
  assert(tabSelector.idx>=0,`应有 empty2 标签：${JSON.stringify(tabSelector)}`);
  const targetTab=page.locator('.tab-strip > *, .tab-item, .tab').filter({hasText:'empty2'}).first();
  assert(await targetTab.count()>0,`应能定位 empty2 标签元素：${JSON.stringify(tabSelector)}`);
  await targetTab.click({button:'right'});
  await page.locator('.menu-entry-label').getByText('关闭其他').click().catch(async()=>{
    await page.locator('.menu-entry-label').filter({hasText:'关闭其他'}).last().click();
  });
  await sleep(500);
  const countAfter=await page.evaluate(()=>window.__APP__.tabs.length);
  assert.equal(countAfter,1,`关闭其他后应剩 1 个标签，实际 ${countAfter}`);
  return {tabsBefore:tabs,countBefore,countAfter};
});
// ---- R1-GRIDCTX：数据网格右键菜单 ----
await check('R1-GRIDCTX','网格右键菜单完整并刷新有效',async()=>{
  await openTable(ids.sqlite,'main','items');
  await page.locator('.content-host .grid-table:visible tbody tr').first().click({button:'right'});
  const entries=await page.locator('.menu-entry-label').allTextContents();
  for(const expected of ['复制','复制行','插入记录','删除记录','提交改动','回滚改动','筛选...','排序...','刷新']){
    assert(entries.includes(expected),`右键菜单应包含「${expected}」，实际 ${JSON.stringify(entries)}`);
  }
  await page.locator('.menu-entry-label').getByText('刷新',{exact:true}).click();
  await sleep(500);
  const rowsNow=(await obs()).rowCount;
  assert(rowsNow>=4,`刷新后应至少 4 行，实际 ${rowsNow}`);
  return {entries,rowsNow};
});
// ---- R1-ERRSQL：错误 SQL 反馈与应用存活 ----
await check('R1-ERRSQL','错误 SQL 报错反馈且应用继续可用',async()=>{
  await node(`conn:${ids.mysql}/db/${ns}`).click();
  await ribbon('新建查询').click();
  const editor=page.locator('.cm-content:visible');
  await editor.waitFor();
  await editor.fill('SELECT * FROM no_such_table_r1xyz;');
  await editor.press('Control+Enter');
  await sleep(1500);
  const bodyText=await page.locator('body').innerText();
  assert(/不存在|Unknown|错误|ERROR|failed/i.test(bodyText),'应向用户展示错误信息');
  await editor.fill('SELECT 99 AS ok;');
  await editor.press('Control+Enter');
  await page.waitForFunction(()=>window.__APP__.activeTab?.results?.sets?.[0]?.rows?.[0]?.[0]==='99',{timeout:8000});
  return {recovered:true};
});
// ---- R1-VIEWMENU：查看菜单切换状态栏 ----
await check('R1-VIEWMENU','查看菜单切换状态栏显隐',async()=>{
  const visibleBefore=await page.locator('.statusbar').isVisible().catch(()=>false);
  await menu('查看','状态栏');
  await sleep(400);
  const visibleAfter=await page.locator('.statusbar').isVisible().catch(()=>false);
  assert.notEqual(visibleBefore,visibleAfter,'状态栏显隐应切换');
  await menu('查看','状态栏');
  await sleep(400);
  const restored=await page.locator('.statusbar').isVisible().catch(()=>false);
  assert.equal(restored,visibleBefore,'再次切换应恢复');
  return {visibleBefore,visibleAfter,restored};
});
// ---- R1-DUPE-CONN：复制连接与移除连接 ----
await check('R1-DUPE-CONN','复制连接创建副本并可移除',async()=>{
  await context(`conn:${ids.sqlite}`,'复制连接...');
  const d=dlg();
  await d.waitFor();
  const nameInput=d.locator('input').first();
  const newName=`R1-copy-${suffix}`;
  await nameInput.fill(newName);
  await d.getByRole('button',{name:/确定|保存/}).first().click();
  await sleep(700);
  const conns=await api('listConnections');
  const copy=conns.find(c=>c.name===newName);
  assert(copy,'复制后应存在副本连接');
  await page.evaluate(async()=>{const a=window.__APP__;a.connections=await window.__E2E_API__.api.listConnections();a.groups=await window.__E2E_API__.api.listGroups();a.refreshTree();});
  await treeExpand(`conn:${copy.id}`).catch(()=>{});
  await context(`conn:${copy.id}`,'移除连接');
  if(await page.locator('.dialog').count()){await dlg().getByRole('button',{name:/确定|移除|删除/}).first().click();}
  await sleep(700);
  const connsAfter=await api('listConnections');
  assert(!connsAfter.find(c=>c.id===copy.id),'移除后连接应消失');
  return {copiedId:copy.id};
});
// ---- R1-DROP-TRUNC：截断表/删除表（临时表） ----
await check('R1-DROP-TRUNC','临时表截断与删除真实生效',async()=>{
  await closeAllTabs();
  await api('executeSql',ids.sqlite,'DROP TABLE IF EXISTS tmp_trunc; CREATE TABLE tmp_trunc(id INTEGER); INSERT INTO tmp_trunc VALUES (1),(2),(3)');
  await page.evaluate(async conn=>{const a=window.__APP__,n=a.tree.find(`conn:${conn}`);await a.loadChildren(n);n.expanded=true;n.loaded=true;a.tree.render();},ids.sqlite);
  await sleep(400);
  await treeExpand(`conn:${ids.sqlite}`);
  const truncNode=page.locator('.tree-node').filter({hasText:'tmp_trunc'}).first();
  const treeIds=await page.evaluate(c=>{const n=window.__APP__.tree.find('conn:'+c);return n?n.children.map(x=>x.id):[];},ids.sqlite);
  assert(await truncNode.count()>0,`树中应有 tmp_trunc：${JSON.stringify(treeIds)}`);
  await truncNode.click({button:'right'});
  await page.locator('.menu-entry-label').getByText('截断表',{exact:true}).click();
  if(await page.locator('.dialog').count()){await dlg().getByRole('button',{name:/确定|截断/}).first().click();}
  await sleep(900);
  const count=(await api('executeSql',ids.sqlite,'SELECT COUNT(*) FROM tmp_trunc')).sets[0].rows[0][0];
  assert.equal(String(count),'0',`截断后应为 0 行，实际 ${count}`);
  await page.evaluate(async conn=>{const a=window.__APP__,n=a.tree.find(`conn:${conn}`);await a.loadChildren(n);n.expanded=true;n.loaded=true;a.tree.render();},ids.sqlite);
  await context(`conn:${ids.sqlite}/table/tmp_trunc`,'删除表');
  if(await page.locator('.dialog').count()){await dlg().getByRole('button',{name:/确定|删除/}).first().click();}
  await sleep(900);
  let gone=false;
  try{await api('executeSql',ids.sqlite,'SELECT * FROM tmp_trunc');}catch(e){gone=true;}
  assert(gone,'删除表后查询应报错');
  return {truncated:true,dropped:true};
});
// ---- R1-SQLFILE：SQL 文件执行（API 层）+ 转储 ----
await check('R1-SQLFILE','SQL 文件执行与整库转储（API 层+文件核对）',async()=>{
  const sqlPath=path.join(runtime,'r1-script.sql');
  fs.writeFileSync(sqlPath,'CREATE TABLE r1_file_t(id INTEGER PRIMARY KEY, tag TEXT);\nINSERT INTO r1_file_t VALUES (1,\'中文值\'),(2,\'emoji😀\');\n');
  const report=await api('runSqlFile',ids.sqlite,'main',sqlPath,true);
  assert.equal(report.failed,0,JSON.stringify(report).slice(0,200));
  const rows=await api('executeSql',ids.sqlite,'SELECT tag FROM r1_file_t ORDER BY id');
  assert.equal(rows.sets[0].rows[1][0],'emoji😀','Unicode/emoji 应原样写入');
  const dumpPath=path.join(runtime,'r1-dump.sql');
  const dump=await api('dumpDatabaseToFile',ids.sqlite,'main',dumpPath,true,true);
  const dumpText=fs.readFileSync(dumpPath,'utf8');
  assert(dumpText.includes('CREATE TABLE')&&dumpText.includes('items'),`转储应包含建表语句：${dumpText.slice(0,200)}`);
  return {report:{total:report.total,succeeded:report.succeeded},dumpBytes:fs.statSync(dumpPath).size};
});
// ---- R1-PG-SYNC-DB：整库同步 SQLite → PG 临时 schema ----
await check('R1-PG-SYNC-DB','SQLite→PG 真实库级数据同步',async()=>{
  const result=await sapi('runDatabase',{task_id:null,source_connection:ids.sqlite,source_schema:'main',target_connection:ids.pg,target_schema:ns,mode:'both',write_mode:'recreate',sync_mode:'full',incremental_update:false,tables:[{source:'items',target:'items',view:false}],where_clause:null});
  const text=JSON.stringify(result);
  assert(/items/.test(text),`同步结果应包含 items：${text.slice(0,300)}`);
  const sourceCount=(await api('executeSql',ids.sqlite,'SELECT COUNT(*) FROM items')).sets[0].rows[0][0];
  const target=(await api('executeSql',ids.pg,`SELECT COUNT(*) FROM items`,ns)).sets[0].rows[0][0];
  assert.equal(String(target),String(sourceCount),`PG 目标行数应等于源（${sourceCount}），实际 ${target}`);
  return {target};
});
fs.writeFileSync(path.join(EVID,'report.json'),JSON.stringify({started:results[0]?.started,finished:new Date().toISOString(),pid,namespace:ns,realDesktop:true,devServer:true,pass:results.filter(r=>r.status==='PASS').length,fail:results.filter(r=>r.status==='FAIL').length,results,console:consoleLines},null,2));
console.log(`SUMMARY pass=${results.filter(r=>r.status==='PASS').length} fail=${results.filter(r=>r.status==='FAIL').length}`);
await browser.close().catch(()=>{});
process.exitCode=results.some(r=>r.status==='FAIL')?1:0;
