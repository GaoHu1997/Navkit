// Focused IPC contract tests; no native hardware, database or RSS is simulated as real.
import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import path from 'node:path';
const root=process.cwd();
const bundle=await build({stdin:{contents:'export { syncApi } from "./src/core/api.ts";',resolveDir:root,loader:'ts'},bundle:true,write:false,platform:'browser',format:'iife',globalName:'DiffApiFixture',logLevel:'silent'});
function fixture(realBridge=false) {
 const calls=[];const report={source_rows:1,target_rows:1};const preview={available_bytes:123,suggested_limit_bytes:45,global_limit_bytes:67,scope:'mockUI contract only',limitations:[]};
 const host={__PREVIEW_MOCK__:{sync_diff:args=>{calls.push(['sync_diff',args]);return report;},preview_diff_memory:args=>{calls.push(['preview_diff_memory',args]);return preview;}}};
 if(realBridge) {
  host.__PREVIEW_MOCK__.sync_diff=host.__PREVIEW_MOCK__.preview_diff_memory=()=>{throw new Error('Native bridge must not consume browser mocks');};
  host.__TAURI_INTERNALS__={invoke:(command,args)=>{calls.push([command,args]);return Promise.resolve(command==='sync_diff'?report:preview);}};
 }
 const sandbox={window:host};vm.runInNewContext(bundle.outputFiles[0].text,sandbox);
 return {api:sandbox.DiffApiFixture.syncApi,calls,report,preview};
}
const req={source_connection:'source',source_schema:'main',source_table:'users',target_connection:'target',target_schema:'main',target_table:'users',where_clause:'id > 1'};
test('legacy omitted and explicit false memory arguments preserve only req and its identity',async()=>{
 const f=fixture();for(const result of [await f.api.diff(req),await f.api.diff(req,false)]) assert.equal(result,f.report);
 for(const [command,args] of f.calls){assert.equal(command,'sync_diff');assert.deepEqual(Object.keys(args),['req']);assert.equal(args.req,req);}
});
test('legacy bounded true shape is unchanged',async()=>{
 const f=fixture();await f.api.diff(req,true);const args=f.calls[0][1];assert.deepEqual(Object.keys(args).sort(),['bounded','req']);assert.equal(args.bounded,true);assert.equal(args.req,req);
});
test('speed auto manual memory forwards exact options and req, never bounded',async()=>{
 const f=fixture();for(const memory of [{mode:'speed'},{mode:'auto'},{mode:'manual',limit_bytes:33554432},{mode:'auto',limit_bytes:null}]){await f.api.diff(req,false,memory);const args=f.calls.at(-1)[1];assert.deepEqual(Object.keys(args).sort(),['memory','req']);assert.equal(args.req,req);assert.equal(args.memory,memory);}
});
test('bounded and memory are mutually exclusive before IPC',async()=>{
 const f=fixture();await assert.rejects(f.api.diff(req,true,{mode:'speed'}),/bounded.*memory/);assert.equal(f.calls.length,0);
});
test('preview options including omission keep the exact new command shape',async()=>{
 const f=fixture();for(const memory of [undefined,{mode:'auto'},{mode:'manual',limit_bytes:1048576}]){assert.equal(await f.api.previewMemory(memory),f.preview);const [command,args]=f.calls.at(-1);assert.equal(command,'preview_diff_memory');assert.deepEqual(Object.keys(args),['memory']);assert.equal(args.memory,memory);}
});
test('real native bridge cannot be shadowed by browser budget mocks',async()=>{
 const f=fixture(true);await f.api.previewMemory({mode:'speed'});await f.api.diff(req,false,{mode:'auto'});assert.deepEqual(f.calls.map(([command])=>command),['preview_diff_memory','sync_diff']);assert.equal(f.calls[1][1].req,req);
});
test('source keeps explicit legacy route and sync diff compatibility signature',()=>{
 const source=readFileSync(path.join(root,'src/core/api.ts'),'utf8');assert.match(source,/bounded = false, memory\?: DiffMemoryOptions/);assert.match(source,/memory !== undefined \? \{ req, memory \} : bounded \? \{ req, bounded: true \} : \{ req \}/);assert.match(source,/call<DiffMemoryPreview>\("preview_diff_memory", \{ memory \}\)/);
});
