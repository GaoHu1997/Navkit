// 标准 cargo 定向测试；先探测真实加载器退出码，必要时仅给隔离测试副本嵌入 Common Controls v6 manifest。
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../../',import.meta.url));
const output=path.join(root,'.tmp','database-workflows');await mkdir(output,{recursive:true});
const commandEnv={...process.env,CARGO_TERM_COLOR:'never'};
if(process.platform==='win32')commandEnv.RUSTC_WORKSPACE_WRAPPER=path.join(root,'tools/tests/database-workflows-rustc-wrapper.cmd');
const vendorBoundary = key => process.env[key] ? `NOT_RUN: ${key} 已提供；本 runner 仅运行本机协议模拟，未连接厂商服务器` : `NOT_RUN: ${key} 未提供`;
const report={suite:'database-workflows-native',vendor:{mysql:vendorBoundary('NAVKIT_EXPORT_LIVE_MYSQL_URL'),mssql:vendorBoundary('NAVKIT_EXPORT_LIVE_MSSQL_URL')},simulation:'真实 SQLx/Tiberius + 本机协议模拟；不是厂商测试',commands:[],loader:[]};
async function command(executable,args,{json=false}={}) {
  return new Promise((resolve,reject)=>{
    const child=spawn(executable,args,{cwd:root,windowsHide:true,env:commandEnv,stdio:['ignore','pipe','pipe']});let stdout='',stderr='';
    child.on('error',reject);child.stdout.on('data',part=>{stdout+=part.toString();if(!json)process.stdout.write(part);});child.stderr.on('data',part=>{stderr+=part.toString();});
    child.on('close',code=>{const diagnostics=json?stdout.split(/\r?\n/).flatMap(line=>{try{const item=JSON.parse(line);return item.message?.level==='error'?[item.message]:[];}catch{return [];}}):[];report.commands.push({executable,args,code,diagnostics,stdout:stdout.slice(-24000),stderr:stderr.slice(-24000)});if(code!==0){process.stderr.write(stderr.slice(-24000));for(const diagnostic of diagnostics)console.error(diagnostic.rendered||diagnostic.message);}resolve({code,stdout,stderr});});
  });
}
try {
  const compiled=await command('cargo',['test','--offline','--manifest-path','src-tauri/Cargo.toml','--lib','--no-run','--message-format=json'],{json:true});assert.equal(compiled.code,0,'标准 cargo 编译必须成功');
  const artifact=compiled.stdout.split(/\r?\n/).map(line=>{try{return JSON.parse(line);}catch{return null;}}).find(item=>item?.reason==='compiler-artifact'&&item.profile?.test&&item.executable&&item.target?.name==='navkit_lib');assert(artifact,'必须取得当前库测试产物');
  if(process.platform==='win32') {
    const probe='import ctypes,json,subprocess,sys; ctypes.windll.kernel32.SetErrorMode(3); r=subprocess.run([sys.argv[1],"--list"],stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=20); print(json.dumps({"returncode":r.returncode,"hex":hex(r.returncode & 0xffffffff),"stdout_bytes":len(r.stdout),"stderr":r.stderr.decode("utf-8",errors="replace")}))';
    const before=await command('python',['-c',probe,artifact.executable]);assert.equal(before.code,0,'运行期探测必须取得真实退出码');const state=JSON.parse(before.stdout.trim());report.loader.push({phase:'before',...state});
    if(state.hex==='0xc0000139') {
      const manifest=artifact.executable+'.manifest';
      const xml='<?xml version="1.0" encoding="UTF-8" standalone="yes"?><assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><assemblyIdentity type="win32" name="NavKit.DatabaseWorkflowTests" version="1.0.0.0" processorArchitecture="amd64"/><dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="amd64" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency></assembly>';
      try {await writeFile(manifest,xml,{flag:'wx'});}catch(error){if(error.code!=='EEXIST')throw error;assert.equal(await readFile(manifest,'utf8'),xml,'保留既有非本测试 manifest，不覆盖');}
      report.loader.push({phase:'test-artifact-manifest',file:manifest,scope:'仅测试产物；未修改 build.rs 或厂商 DLL'});
      // 旁置 manifest 在当前 Windows 未生效时，只修改隔离测试副本；不抢占其他 owner 的 Cargo 产物。
      const isolated=path.join(output,`native-${Date.now()}`);await mkdir(isolated);const prepared=path.join(isolated,'navkit-workflows-tests.exe');await copyFile(artifact.executable,prepared);
      const mt=process.env.NAVKIT_MT_EXE||'C:/Program Files (x86)/Windows Kits/10/bin/10.0.26100.0/x64/mt.exe';
      const embedded=await command(mt,['-nologo','-manifest',manifest,`-outputresource:${prepared};#1`]);assert.equal(embedded.code,0,'测试副本必须成功嵌入 Common Controls v6 资源');
      const after=await command('python',['-c',probe,prepared]);assert.equal(after.code,0);const result=JSON.parse(after.stdout.trim());report.loader.push({phase:'isolated-embedded-manifest',file:prepared,...result});assert.equal(result.returncode,0,'manifest 修复后必须真实进入测试程序');
      const sourceStat=await command('python',['-c','import json,os,sys; s=os.stat(sys.argv[1]); print(json.dumps([str(s.st_size),str(s.st_mtime_ns)]))',artifact.executable]);assert.equal(sourceStat.code,0);const [size,mtime]=JSON.parse(sourceStat.stdout);
      Object.assign(commandEnv,{NAVKIT_TEST_SOURCE:artifact.executable,NAVKIT_TEST_PREPARED:prepared,NAVKIT_TEST_SOURCE_SIZE:size,NAVKIT_TEST_SOURCE_MTIME:mtime,NAVKIT_TEST_MANIFEST:manifest,NAVKIT_TEST_MT:mt,CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER:path.join(root,'tools/tests/database-workflows-test-runner.cmd')});
    } else assert.equal(state.returncode,0,'其他运行期失败不吞掉或跳过');
  }
  const filters=['batch_export::','object_identity::','mssql::export_adapter::','transfer::transfer_export_cursor::','transfer::transfer_output::','transfer::transfer_spool_tests::','transfer::transfer_streaming_tests::'];
  const result=await command('cargo',['test','--offline','--manifest-path','src-tauri/Cargo.toml','--lib','--',...filters,'--nocapture']);assert.equal(result.code,0,'全部定向导出测试必须成功');assert.match(result.stdout,/test result: ok\. [1-9]\d* passed/,'不得用 0 项测试冒充通过');for(const filter of filters)assert(result.stdout.includes('test '+filter),'每个指定模块必须实际执行：'+filter);
  report.state='passed';
} catch(error) {report.state='failed';report.error=String(error);throw error;}
finally {await writeFile(path.join(output,'native-results.json'),JSON.stringify(report,null,2));console.log(JSON.stringify({state:report.state,loader:report.loader,vendor:report.vendor,report:path.join(output,'native-results.json')},null,2));}
