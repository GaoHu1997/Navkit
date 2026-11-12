import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
const read=file=>fs.readFileSync(new URL('../../'+file,import.meta.url),'utf8');
const transpile=text=>ts.transpile(text,{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.None});
const env=vm.createContext({exports:{}});
vm.runInContext(transpile(read('src/core/sqlFormat.ts')),env);
const formatSql=env.exports.formatSql;
const source=ts.createSourceFile('App.ts',read('src/core/App.ts'),ts.ScriptTarget.Latest,true);
const cls=source.statements.find(n=>ts.isClassDeclaration(n)&&n.name?.text==='App');
const members=names=>cls.members.filter(n=>names.includes(n.name?.getText(source))).map(n=>n.getText(source)).join('\n');
const definition='CREATE VIEW v AS SELECT a.id,a.name,COUNT(*) AS total FROM items a LEFT OUTER JOIN other b ON a.id=b.id WHERE a.id>0 GROUP BY a.id,a.name ORDER BY total;';

test('一行视图按字段和子句排版，别名与函数不改写，重复美化稳定',()=>{
 const result=formatSql(definition);
 assert.match(result,/AS\n  SELECT/); assert.match(result,/a\.id,\n    a\.name,/);
 assert.match(result,/COUNT\(\*\) AS total/); assert.match(result,/\n  LEFT OUTER JOIN other b\n    ON/);
 assert.match(result,/\n  FROM/); assert.match(result,/\n  WHERE/);
 assert.equal(result.replace(/\s/g,''),definition.replace(/\s/g,'')); assert.equal(formatSql(result),result);
});
test('字面量、标识符、注释、转义前缀和运算符不改写',()=>{
 const tick=String.fromCharCode(96);
 const cases=[
  ["SELECT N'from  where',_utf8'中 文',X'AB',B'01',1e-3 FROM t WHERE id=-1;",'mysql'],
  ["SELECT E'it\\'s from',U&'d\\0061t',a::int,$1 FROM t WHERE id=1;",'postgres'],
  ["SELECT 'a\\',id FROM t WHERE id=1;",'postgres'],
  ["SELECT 'from  where','it''s',\"select\",[name]]end] FROM t WHERE id=1;",'sqlite'],
  ['SELECT '+tick+'a'+tick+tick+'from'+tick+',id FROM t WHERE id=1;','mysql'],
  ['SELECT id -- 保留注释  内容\nFROM t WHERE id=1; # mysql 注释\nSELECT 2;','mysql'],
  ['SELECT /* 外层 /* 内层 */ 仍在注释 */ id FROM t WHERE id=1;','postgres'],
  ["SELECT doc#>>'{where}',id FROM t WHERE id=1;",'postgres'],
  ["SELECT 'a'\n'b',id FROM t WHERE id=1;",'postgres'],
 ];
 for(const [sql,dialect] of cases){const result=formatSql(sql,{dialect});assert.equal(result.replace(/\s/g,''),sql.replace(/\s/g,''),sql);assert.equal(formatSql(result,{dialect}),result,sql);}
 assert.match(formatSql(cases[0][0]),/N'from  where'/);
 assert.match(formatSql(cases[1][0],{dialect:'postgres'}),/E'it\\'s from'/);
 assert.match(formatSql(cases[5][0]),/-- 保留注释  内容\n/);
 assert.match(formatSql(cases[8][0],{dialect:'postgres'}),/'a'\n'b'/);
});
test('美元引用函数体逐字保留，多语句分隔稳定',()=>{
 const body="$body$ BEGIN\n  RETURN 'from  where'; -- 保留\nEND; $body$";
 const sql='CREATE FUNCTION f() RETURNS text AS '+body+' LANGUAGE plpgsql; SELECT id,name FROM t;';
 const result=formatSql(sql,{dialect:'postgres'});assert(result.includes(body));assert.match(result,/;\n\nSELECT/);assert.equal(formatSql(result,{dialect:'postgres'}),result);
});
test('未闭合引用和注释不猜测、不破坏草稿',()=>{
 for(const sql of ["SELECT 'unfinished",'SELECT /* unfinished','CREATE VIEW v AS SELECT $$unfinished']) assert.equal(formatSql(sql,{dialect:'postgres'}),sql);
 assert.equal(formatSql(''),'');
});
function fixture(kind='viewdesign',sql=definition){
 const tab={id:'test',kind,connectionId:'c1',schema:'demo',object:'v',objectType:'view',sql,dirty:false};
 class Editor{constructor(){this.value=sql;this.history=[];this.focused=false;}getSql(){return this.value;}setSql(value){this.history.push(this.value);this.value=value;tab.sql=value;tab.dirty=true;}focus(){this.focused=true;}undo(){this.value=this.history.pop();tab.sql=this.value;tab.dirty=false;}}
 class QueryTabView{constructor(){this.editor=new Editor();}resultBusy(){return false;}}
 class SqlDesignTabView{constructor(){this.editor=new Editor();}copyEditor(){return this.editor;}}
 class RoutineTabView extends SqlDesignTabView{draftBusy(){return !!this.busy;}}
 class OtherView{}
 const listeners={},document={addEventListener:(type,callback)=>listeners[type]=callback};
 const context=vm.createContext({exports:{},document,QueryTabView,SqlDesignTabView,RoutineTabView,ObjectsTabView:OtherView,DataTabView:OtherView,StructureTabView:OtherView,SqlEditor:Editor,formatSql,objectProtocol:cfg=>cfg?.kind,MANUAL_TRANSACTION_WRITE_COMMANDS:new Set(),objectCapability:()=>null});
 const names=['activeSqlEditor','cmdBeautifySql','tabToolbarItems','guardedTabToolbarItems','commandUnavailableReason','setupKeyboard'];
 vm.runInContext(transpile('globalThis.TestApp=class {'+members(names)+'};'),context);
 const app=new context.TestApp(),view=kind==='query'?new QueryTabView():kind==='routinedesign'?new RoutineTabView():new SqlDesignTabView();
 Object.assign(app,{activeTab:tab,views:new Map([[tab.id,view]]),connections:[{id:'c1',kind:'mysql',read_only:true}],connectionById:()=>app.connections[0],manualTransactionFor:()=>null,manualTransactionIsBusy:()=>false,toast:(level,message)=>app.messages.push({level,message}),messages:[]});
 return {app,view,tab,listeners};
}
for(const kind of ['viewdesign','routinedesign','query'])test(kind+' 美化只更新当前草稿、可撤销、不自动保存',async()=>{
 const {app,view,tab}=fixture(kind);assert.equal(app.commandUnavailableReason('beautify-sql'),null);await app.cmdBeautifySql();
 assert.match(view.editor.getSql(),/\n/);assert.equal(tab.sql,view.editor.getSql());assert(tab.dirty);assert(view.editor.focused);assert.equal(app.messages.at(-1).level,'success');
 await app.cmdBeautifySql();assert.equal(view.editor.history.length,1);view.editor.undo();assert.equal(view.editor.getSql(),definition);
});
test('设计页有美化按钮，例程忙碌时禁用并说明原因',()=>{
 for(const kind of ['viewdesign','routinedesign']){const {app,view,tab}=fixture(kind),items=app.guardedTabToolbarItems(tab);assert.deepEqual(Array.from(items,x=>x.id),['save','beautify-sql','refresh']);assert.equal(items[1].label,'美化 SQL');assert(!items[1].disabled);if(kind==='routinedesign'){view.busy=true;const button=app.guardedTabToolbarItems(tab)[1];assert(button.disabled);assert.match(button.disabledReason,/稍后/);}}
});
test('F6 分发设计页美化，空 SQL 不创建撤销记录',async()=>{
 const {app,view,listeners}=fixture();app.setupKeyboard();const pending=[];app.runCommand=id=>{assert.equal(id,'beautify-sql');pending.push(app.cmdBeautifySql());};let prevented=false;
 listeners.keydown({key:'F6',target:null,preventDefault:()=>prevented=true});await Promise.all(pending);assert(prevented);assert.match(view.editor.getSql(),/\n/);
 const empty=fixture('viewdesign','  ');await empty.app.cmdBeautifySql();assert.equal(empty.view.editor.history.length,0);
});
test('失效连接允许本地美化但不放开保存和执行',()=>{
 const {app,tab}=fixture();tab.datasourceStale='连接已变更';assert.equal(app.commandUnavailableReason('beautify-sql'),null);assert.equal(app.commandUnavailableReason('save'),'连接已变更');assert.equal(app.commandUnavailableReason('run'),'连接已变更');
});
