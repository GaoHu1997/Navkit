import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const root = resolve(import.meta.dirname, '../..');

test('portable packaging updates executable without deleting connections, encryption key or backups', () => {
  const source = readFileSync(join(root, 'build.ps1'), 'utf8');
  const start = source.indexOf('$outDir = Join-Path $PSScriptRoot "dist-portable"');
  const end = source.indexOf('# WebView2', start);
  assert.ok(start >= 0 && end > start);
  const block = source.slice(start, end).replace('$outDir = Join-Path $PSScriptRoot "dist-portable"', '$outDir = $PortableDir');
  assert.doesNotMatch(block, /Remove-Item/i);
  const dir = mkdtempSync(join(tmpdir(), 'navkit-packaging-test-'));
  const portable = join(dir, 'portable');
  const srcExe = join(dir, 'fresh.exe');
  const data = join(portable, 'data');
  const script = join(dir, 'update.ps1');
  try {
    mkdirSync(join(data, 'backups', 'fixture'), { recursive: true });
    const fixtures = new Map([
      [join(data, 'connections.json'), Buffer.from('{"connections":[{"id":"fixture"}]}')],
      [join(data, 'key.bin'), Buffer.from(Array.from({ length: 32 }, (_, i) => i))],
      [join(data, 'backups', 'fixture', 'backup.sql'), Buffer.from('SELECT 42;')],
    ]);
    for (const [path, bytes] of fixtures) writeFileSync(path, bytes);
    writeFileSync(script, `param([string]$srcExe,[string]$PortableDir)\n$ErrorActionPreference='Stop'\n${block}`, 'utf8');
    for (const version of ['version-one', 'version-two']) {
      writeFileSync(srcExe, version);
      const run = spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', script, '-srcExe', srcExe, '-PortableDir', portable], { encoding: 'utf8' });
      assert.equal(run.status, 0, `${run.stdout}\n${run.stderr}`);
      assert.equal(readFileSync(join(portable, 'navkit.exe'), 'utf8'), version);
      for (const [path, bytes] of fixtures) assert.deepEqual(readFileSync(path), bytes);
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
