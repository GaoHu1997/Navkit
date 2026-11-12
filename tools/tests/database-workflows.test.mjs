import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';
const read = file => fs.readFileSync(new URL(`../../${file}`,import.meta.url),'utf8');
const flush = () => new Promise(resolve=>setImmediate(resolve));
const deferred = () => {let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return {promise,resolve,reject};};
class Element {
  constructor(tag='div'){this.tagName=tag;this.children=[];this.attributes={};this.dataset={};this.style={};this.listeners=new Map();this.value='';this.disabled=false;this.textContent='';}
  append(...children){this.children.push(...children);} appendChild(child){this.children.push(child);return child;}
  prepend(...children){this.children.unshift(...children);}
  replaceChildren(...children){this.children=[...children];this.value='';}
  setAttribute(name,value){this.attributes[name]=String(value);} removeAttribute(name){delete this.attributes[name];}
  addEventListener(type,callback){if(!this.listeners.has(type))this.listeners.set(type,[]);this.listeners.get(type).push(callback);}
  fire(type){for(const callback of this.listeners.get(type)??[])callback({target:this});}
  querySelector(selector){return this.children.flatMap(child=>[child,...(child.children??[])]).find(child=>selector==='.field-label'&&child.className==='field-label')??null;}
}
const document={createElement:tag=>new Element(tag),createTextNode:text=>Object.assign(new Element('#text'),{textContent:text})};
function picker(api){
  const exports={}; const context=vm.createContext({exports,document,api,DB_KINDS:[{id:'mysql',protocol:'mysql'},{id:'postgresql',protocol:'postgres'},{id:'custom_pg',protocol:'postgres'},{id:'mssql',protocol:'mssql'},{id:'sqlite',protocol:'sqlite'}],field:(title,control)=>{const row=new Element(),label=new Element();label.className='field-label';label.textContent=title;row.append(label,control);return row;}});
  vm.runInContext(ts.transpile(read('src/core/namespace-label.ts').replace(/^import .*;\r?$/gm,''),{target:ts.ScriptTarget.ES2021,module:ts.ModuleKind.CommonJS}),context);
  Object.assign(context,exports);
  const namespaceLabel = context.namespaceLabel;
  vm.runInContext(ts.transpile(read('src/core/database-picker.ts').replace(/^import .*;\r?$/gm,''),{target:ts.ScriptTarget.ES2021,module:ts.ModuleKind.CommonJS}),context);
  return { ...exports, namespaceLabel };
}
async function pathModule(directory={downloadDir:async()=>'/users/真实下载',documentDir:async()=>'/users/Documents',homeDir:async()=>'/users/home',join:async(...parts)=>parts.join('/')}){
  const result=await build({entryPoints:[fileURLToPath(new URL('../../src/core/export-path.ts',import.meta.url))],bundle:true,write:false,platform:'node',format:'cjs',logLevel:'silent',plugins:[{name:'real-path-ipc-fixture',setup(build){build.onResolve({filter:/^@tauri-apps\/api\/path$/},()=>({path:'path',namespace:'fixture'}));build.onLoad({filter:/.*/,namespace:'fixture'},()=>({contents:'export const downloadDir=(...a)=>globalThis.pathFixture.downloadDir(...a);export const documentDir=(...a)=>globalThis.pathFixture.documentDir(...a);export const homeDir=(...a)=>globalThis.pathFixture.homeDir(...a);export const join=(...a)=>globalThis.pathFixture.join(...a);',loader:'js'}));}}]});
  const context=vm.createContext({module:{exports:{}},exports:{},pathFixture:directory});vm.runInContext(result.outputFiles[0].text,context);return context.module.exports;
}
test('真实用户目录默认路径、完整Windows/UNC/POSIX路径与保留basename的扩展名切换',async()=>{
  const paths=await pathModule();assert.equal(await paths.defaultExportPath('orders.csv'),'/users/真实下载/orders.csv');
  for(const value of ['/users/真实下载/export.csv','C:\\Users\\me\\Downloads\\export.csv','C:/Users/me/Downloads/export.csv','\\\\server\\share\\folder\\export.csv'])assert.equal(paths.exportPathProblem(value),null,value);
  for(const value of ['orders.csv','C:orders.csv','relative/f.csv','/a/evil\n.csv'])assert(paths.exportPathProblem(value),value);
  assert.equal(paths.exportExtension('C:\\Users\\me\\chosen.name.csv','json'),'C:\\Users\\me\\chosen.name.json');assert.equal(paths.exportExtension('/chosen/dotted.dir/basename','sql'),'/chosen/dotted.dir/basename.sql');
  for(const name of ['normal.csv','x_r_n_0.csv','report.csv','x00r0n.csv'])assert.equal(paths.exportFileName(name),name);assert.equal(paths.exportFileName('bad:/\\\u0000x'),'bad____x');
  for(const value of ['C:/Users/name/report.csv','D:\\Downloads\\a.csv','/home/user/a.csv','\\\\server\\share\\folder\\a.csv'])assert.equal(paths.exportPathProblem(value),null,value);
  const input=new Element('input'),format=new Element('select');format.value='csv';const binding=paths.bindExportPath(input,'orders.csv',format);await binding.ready;assert.equal(input.title,input.value);input.value='/my/export.custom.csv';input.fire('input');format.value='json';format.fire('change');assert.equal(input.value,'/my/export.custom.json');assert.equal(input.title,input.value);
});
test('目录IPC迟到不会覆盖用户输入，下载目录失败回退真实文档目录',async()=>{
  const pending=deferred();const paths=await pathModule({downloadDir:()=>pending.promise,documentDir:async()=>'/real/Documents',homeDir:async()=>'/real/home',join:async(...parts)=>parts.join('/')});
  const input=new Element('input');const binding=paths.bindExportPath(input,'default.csv');input.value='/chosen/my.csv';input.fire('input');pending.resolve('/download');await binding.ready;assert.equal(input.value,'/chosen/my.csv');assert.equal(input.title,input.value);
  const fallback=await pathModule({downloadDir:async()=>{throw Error('not available');},documentDir:async()=>'/real/Documents',homeDir:async()=>'/real/home',join:async(...parts)=>parts.join('/')});assert.equal(await fallback.defaultExportPath('result.json'),'/real/Documents/result.json');
});
test('输出目录必须是真实绝对目录，完整basename随默认对象变化且保留用户custom',async()=>{
  const paths=await pathModule({downloadDir:async()=>'',documentDir:async()=>'/real/Documents',homeDir:async()=>'/real/home',join:async(...parts)=>parts.join('/')});
  assert.equal(await paths.defaultExportPath('report.csv'),'/real/Documents/report.csv');
  for(const path of ['', '/', 'C:/Users/me/Downloads/', '/dir/.', '/dir/..'])assert(paths.exportPathProblem(path),path);
  const format=new Element('select');format.value='csv';const input=new Element('input');let name='database_orders.csv';
  const binding=paths.bindExportPath(input,()=>name,format);await binding.ready;assert.equal(input.value,'/real/Documents/database_orders.csv');
  name='database_recent.csv';binding.autoName();assert.equal(input.value,'/real/Documents/database_recent.csv');
  input.value='/chosen/custom.name.csv';input.fire('input');name='changed_object.csv';binding.autoName();assert.equal(input.value,'/chosen/custom.name.csv');
  format.value='json';format.fire('change');assert.equal(input.value,'/chosen/custom.name.json');assert.equal(input.title,input.value);
});

test('下载和文档目录不可用时使用真实home；所有目录缺失或相对时禁止生成输出路径',async()=>{
  const paths=await pathModule({downloadDir:async()=>{throw Error('missing');},documentDir:async()=>'',homeDir:async()=>'/real/home',join:async(...parts)=>parts.join('/')});
  assert.equal(await paths.defaultExportPath('orders.csv'),'/real/home/orders.csv');
  const missing=await pathModule({downloadDir:async()=>'',documentDir:async()=>'relative/Documents',homeDir:async()=>{throw Error('missing');},join:async(...parts)=>parts.join('/')});
  await assert.rejects(missing.defaultExportPath('orders.csv'),/无法读取用户导出目录/);
  const input=new Element('input');const binding=missing.bindExportPath(input,'orders.csv');await binding.ready;assert.equal(input.value,'');assert.match(binding.problem(),/无法读取用户导出目录/);
});
test('调度库选择先ensureConnection，真实metadata选项不接受未知旧值，PG物理库与schema分开',async()=>{
  const calls=[];const {createDatabasePicker,namespaceLabel}=picker({ensureConnection:async id=>{calls.push(['ensure',id]);return {ok:true};},listDatabases:async id=>{calls.push(['list',id]);return [['public','schema'],['analytics','schema']];}});
  const connection=new Element('select');connection.value='pg';const result=createDatabasePicker([{id:'pg',kind:'postgresql',database:'physical_db'}],connection,'old_namespace');await result.ready;
  assert.deepEqual(calls,[['ensure','pg'],['list','pg']]);assert.equal(result.select.value,'');assert(result.problem());assert(!result.select.children.some(option=>option.value==='old_namespace'));assert.match(result.el.children[1].children[1].textContent,/旧值/);
  result.select.value='analytics';result.select.fire('change');assert.equal(result.value(),'analytics');assert.equal(result.problem(),null);assert(!result.select.children.some(option=>option.value==='physical_db'));
  assert.equal(namespaceLabel({kind:'custom_pg'}),'模式 (schema)');assert.equal(namespaceLabel({kind:'mssql'}),'数据库');
});
test('切连接立即清除旧选择，旧metadata回包不能覆盖新连接，无隐式执行',async()=>{
  const old=deferred(),current=deferred(),calls=[];const {createDatabasePicker}=picker({ensureConnection:async id=>{calls.push(['ensure',id]);return {ok:true};},listDatabases:id=>{calls.push(['list',id]);return id==='a'?old.promise:current.promise;}});
  const connection=new Element('select');connection.value='a';const result=createDatabasePicker([{id:'a',kind:'mysql'},{id:'b',kind:'mssql'}],connection,'old');await flush();connection.value='b';connection.fire('change');assert.equal(result.select.value,'');assert.equal(result.select.disabled,true);assert(result.problem());await flush();current.resolve([['real_b','database']]);await flush();result.select.value='real_b';old.resolve([['old','database']]);await result.ready;await flush();assert.equal(result.value(),'real_b');assert(!result.select.children.some(option=>option.value==='old'));assert.equal(result.problem(),null);assert(calls.every(([name])=>name==='ensure'||name==='list'));
});
function batchHarness(rows,preselected=[]){
  let wizard;const requests=[];const exports={};
  class Dialog{constructor(options){this.body=new Element();this.footer=new Element();this.options=options;options.render(this.body,this);}onClose(){}showError(error){this.error=error;}}
  const context=vm.createContext({exports,document,Dialog,Wizard:class{constructor(options){wizard=options;this.stepsEl=new Element();this.bodyEl=new Element();options.steps[0].render(this.bodyEl);}buttons(){return [];}setBusy(){}},api:{ensureConnection:async()=>({ok:true}),listTables:async()=>rows},batchExportApi:{run:async request=>{requests.push(request);return {items:request.objects.map(object=>({object,status:'completed',rows:2,file:request.destination.file??request.destination.base_file})),combined_file:null,message:'done'};}},field:(label,input)=>input,textInput:value=>Object.assign(new Element('input'),{value}),select:(options,value)=>Object.assign(new Element('select'),{value,options}),namespaceLabel:()=> '模式 (schema)',accessibleDialog:()=>{},taskFeedback:()=>({begin:()=> 'task123',finish:()=>{}}),taskCenter:{register:()=>({update(){},finish(){},fail(){},cancel(){},setOpener(){}}),snapshot:()=>({tasks:[],running:0}),subscribe:()=>()=>{},clearFinished(){},open:()=>false},bindExportPath:(input,name,format)=>{input.value=`C:\\Users\\me\\Downloads\\${name}`;input.title=input.value;format?.addEventListener('change',()=>{input.value=input.value.replace(/\.[^.]+$/,`.${format.value}`);});return {ready:Promise.resolve(),refresh:()=>{input.title=input.value;},problem:()=>input.value?null:'path'};}});
  vm.runInContext(ts.transpile(read('src/dialogs/BatchExportDialog.ts').replace(/^import .*;\r?$/gm,''),{target:ts.ScriptTarget.ES2021,module:ts.ModuleKind.CommonJS}),context);
  const dialog=exports.openBatchExportWizard({id:'pg',name:'Postgres',kind:'postgresql',database:'physical'},'analytics',preselected);
  return {exports,dialog,requests,get wizard(){return wizard;}};
}
test('批量清单保留明确预选，过滤全选不扩大到不可见对象，只有显式执行发送typed payload',async()=>{
  const h=batchHarness([['orders','table'],['recent','view'],['fn','function']],[{name:'recent',object_type:'view'}]);await flush();assert.equal(h.requests.length,0);assert.equal(h.wizard.steps[0].validate(),null);
  const host=new Element();h.wizard.steps[0].render(host);const panel=host.children[0],filter=panel.children[1],actions=panel.children[2],list=panel.children[4];assert.equal(list.children.length,2);assert.equal(list.children[0].children[0].checked,false);assert.equal(list.children[1].children[0].checked,true);
  actions.children[1].fire('click');assert(h.wizard.steps[0].validate());filter.value='orders';filter.fire('input');actions.children[0].fire('click');assert.equal(list.children.length,1);assert.equal(list.children[0].children[0].checked,true);assert.equal(h.requests.length,0);
  h.wizard.onEnter(2);const summaryHost=new Element();h.wizard.steps[2].render(summaryHost);assert.match(summaryHost.children[0].children[0].children[0].textContent,/C:\\Users\\me\\Downloads\\analytics_export_001_orders.csv/);
  await h.wizard.onFinish();assert.equal(h.requests.length,1);assert.deepEqual(JSON.parse(JSON.stringify(h.requests[0].objects)),[{name:'orders',object_type:'table'}]);assert.equal(h.requests[0].namespace,'analytics');assert.equal(h.requests[0].connection_id,'pg');assert.equal(h.requests[0].destination.mode,'files');assert.equal(h.requests[0].task_id,'task123');
});
test('批量路径预览与后端规则一致，Windows目录、Unicode长度和特殊对象名保留真实完整路径',()=>{
  const h=batchHarness([]);const preview=h.exports.batchOutputPreview;
  assert.equal(preview('C:\\Users\\me\\Downloads\\chosen.name.csv',{name:'a/b',object_type:'table'},0,'json'),'C:\\Users\\me\\Downloads\\chosen.name_001_a_b.json');
  assert.equal(preview('/tmp/chosen.csv',{name:'.',object_type:'table'},1,'csv'),'/tmp/chosen_002_object.csv');
  assert.equal(preview('/tmp/chosen.csv',{name:'😀'.repeat(81)+'\u007f',object_type:'table'},2,'sql'),`/tmp/chosen_003_${'😀'.repeat(80)}.sql`);
  assert.equal(preview('/tmp/chosen.csv',{name:'a\u0080b',object_type:'view'},0,'csv'),'/tmp/chosen_001_a_b.csv');
});
test('metadata失败或空目录维持不可保存，不把旧值合成实际库',async()=>{
  for(const listDatabases of [async()=>[],async()=>{throw Error('offline');}]){const {createDatabasePicker}=picker({ensureConnection:async()=>({ok:true}),listDatabases});const connection=new Element('select');connection.value='a';const result=createDatabasePicker([{id:'a',kind:'mysql'}],connection,'stale');await result.ready;assert(result.problem());assert.equal(result.select.disabled,true);assert.equal(result.value(),'');assert(!result.select.children.some(option=>option.value==='stale'));}
});
async function appExportHarness() {
  // 直接编译当前 App 的真实 runCommand/导出成员，其他菜单命令不执行。
  const source=ts.createSourceFile('App.ts',read('src/core/App.ts'),ts.ScriptTarget.Latest,true,ts.ScriptKind.TS);
  const app=source.statements.find(node=>ts.isClassDeclaration(node)&&node.name.text==='App');assert(app);
  const names=new Set(['runCommand','cmdDump','cmdExport','importExportTarget']);
  const members=app.members.filter(member=>member.name&&names.has(member.name.getText(source))).map(member=>member.getText(source)).join('\n');
  assert.equal(app.members.filter(member=>member.name&&names.has(member.name.getText(source))).length,names.size);
  const bundle=await build({stdin:{contents:`export class ExportScopeApp { ${members} }`,loader:'ts'},bundle:true,write:false,format:'cjs',platform:'node',logLevel:'silent'});
  const calls=[];class ObjectsTabView {selectedObject(){return {name:'tab_order',type:'table'};}selectedObjects(){return [{name:'tab_order',type:'table'},{name:'tab_recent',type:'view'}];}}
  const context=vm.createContext({module:{exports:{}},exports:{},ObjectsTabView,QueryTabView:class{},openDumpDialog:(...args)=>calls.push(['object',...args.slice(0,3)]),openBatchExportWizard:(...args)=>calls.push(['batch',...args.slice(0,3)]),openExportWizard:(...args)=>calls.push(['single',...args.slice(0,3)]),api:{}});
  vm.runInContext(bundle.outputFiles[0].text,context);const instance=new context.module.exports.ExportScopeApp();
  const connections=[{id:'a',kind:'mysql',name:'A'},{id:'b',kind:'mssql',name:'B'}];
  const nodes=new Map([['conn:a/db/dbA',{id:'conn:a/db/dbA'}],['conn:a/db/dbA/table/tree_order',{id:'conn:a/db/dbA/table/tree_order'}]]);
  Object.assign(instance,{activeTab:{id:'tabB',connectionId:'b',kind:'table',schema:'dbB',object:'[dbo].[tab_order]',selectedRows:new Set()},views:new Map(),tree:{selectedNodeId:'conn:a/db/dbA',find:id=>nodes.get(id)},connectionById:id=>connections.find(connection=>connection.id===id),commandUnavailableReason:()=>null,notifyToolbar:()=>{},renderStatus:()=>{},toast:(...args)=>calls.push(['toast',...args]),cmdDumpDatabase:(...args)=>calls.push(['database',...args]),currentTableRef:()=>{throw Error('树范围不得调用会回落活动标签页的 currentTableRef');},currentDbTarget:()=>{throw Error('树范围不得回落活动标签页');}});
  Object.assign(instance,{menu:{renderBar:()=>{}},contextMenus:()=>[],viewMenu:()=>[]});
  return {instance,calls,ObjectsTabView};
}
test('真实 runCommand 树转储 freezes A/dbA 数据库级范围，不借活动 B/tableB',async()=>{
  const h=await appExportHarness();await h.instance.runCommand('tree-dump');assert.deepEqual(h.calls.map(([kind,cfg,namespace])=>[kind,cfg.id,namespace]),[['database','a','dbA']]);
  h.calls.length=0;h.instance.tree.selectedNodeId='conn:a/db/dbA/table/tree_order';await h.instance.runCommand('tree-dump');assert.deepEqual(h.calls.map(([kind,cfg,namespace,table])=>[kind,cfg.id,namespace,table]),[['object','a','dbA','tree_order']]);
  h.calls.length=0;await h.instance.runCommand('dump-sql');assert.deepEqual(h.calls.map(([kind,cfg,namespace,table])=>[kind,cfg.id,namespace,table]),[['object','b','dbB','[dbo].[tab_order]']]);
});
test('真实树导出空表打开批量选择，toolbar Objects 多选表/视图仅属于 tab owner',async()=>{
  const h=await appExportHarness();await h.instance.runCommand('tree-export');assert.equal(h.calls[0][0],'batch');assert.equal(h.calls[0][1].id,'a');assert.equal(h.calls[0][2],'dbA');assert.deepEqual(Array.from(h.calls[0][3]),[]);
  h.calls.length=0;h.instance.activeTab.kind='objects';h.instance.views.set('tabB',new h.ObjectsTabView());await h.instance.runCommand('export');assert.equal(h.calls[0][1].id,'b');assert.equal(h.calls[0][2],'dbB');assert.deepEqual(JSON.parse(JSON.stringify(h.calls[0][3])),[{name:'tab_order',object_type:'table'},{name:'tab_recent',object_type:'view'}]);
});
