import { readFile, writeFile, mkdir, access } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const here=path.dirname(fileURLToPath(import.meta.url));
export const H2_SHA256='8dae62d22db8982c3dcb3826edb9c727c5d302063a67eef7d63d82de401f07d3';
export async function buildReference({out,jar,java='java',extraSource=''}) {
  if(!out||!path.isAbsolute(out)||!jar)throw new Error('Explicit absolute output directory and verified H2 JAR are required');
  const bytes=await readFile(jar);if(bytes.length!==2651157||createHash('sha256').update(bytes).digest('hex')!==H2_SHA256)throw new Error('H2 artifact identity mismatch');
  await mkdir(out,{recursive:false});
  const main=await readFile(path.join(here,'NavkitHttpTunnel.java'),'utf8');
  const codec=await readFile(path.resolve(here,'../../src-tauri/src/JdbcMutationSupport.java'),'utf8');
  const sql=await readFile(path.join(here,'TunnelSql.java'),'utf8');
  const source=path.join(out,'NavkitHttpTunnel.java');
  await writeFile(source,`${main}\n${codec}\n${sql}\n${extraSource}`,'utf8');
  // Java 11 source launcher compiles the complete source once at startup. No npm,
  // Maven, shell script, downloaded code, or dependency resolution runs here.
  return {source,command:java,args:['-Dfile.encoding=UTF-8','--class-path',path.resolve(jar),source]};
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const [out,jar]=process.argv.slice(2);const built=await buildReference({out:out&&path.resolve(out),jar,java:process.env.NAVKIT_JAVA||'java'});
  console.log(JSON.stringify({source:built.source,run:[built.command,...built.args,'<server-config.json>']},null,2));
}
