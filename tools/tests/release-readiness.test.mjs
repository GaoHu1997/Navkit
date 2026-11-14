import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { checkRelease } from '../check-release.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const identityFiles = ['package.json', 'package-lock.json', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock', 'src-tauri/tauri.conf.json', 'src-tauri/tauri.release.conf.json', 'pnpm-lock.yaml'];
function fixture(run) {
  const dir = mkdtempSync(path.join(tmpdir(), 'navkit-release-test-'));
  try {
    mkdirSync(path.join(dir, 'src-tauri'));
    for (const filename of identityFiles) writeFileSync(path.join(dir, filename), readFileSync(path.join(root, filename)));
    const update = (filename, change) => {
      const full = path.join(dir, filename);
      const value = JSON.parse(readFileSync(full, 'utf8'));
      change(value); writeFileSync(full, JSON.stringify(value));
    };
    run(dir, update);
  } finally { rmSync(dir, { recursive: true, force: true }); }
}

test('release identity accepts the current consistent versions and exact tag', () => {
  const version = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')).version;
  assert.deepEqual(checkRelease({ root, tag: `v${version}` }), { version, tag: `v${version}` });
});

test('release rejects branch names, wrong versions and shell-shaped tags', () => {
  for (const tag of ['main', 'v999.0.0', 'v1.0.0-pre\nattack', 'v$(echo attack)', '']) {
    assert.throws(() => checkRelease({ root, tag }), /release tag/);
  }
});

for (const filename of ['package-lock.json', 'src-tauri/tauri.conf.json', 'src-tauri/tauri.release.conf.json']) {
  test(`release rejects a mismatched version in ${filename}`, () => fixture((dir, update) => {
    update(filename, value => { value.version = '999.0.0'; });
    assert.throws(() => checkRelease({ root: dir }), /version/);
  }));
}

test('release rejects dependency drift that npm ci would reject', () => fixture((dir, update) => {
  update('package.json', pkg => { pkg.devDependencies.typescript = '0.0.0'; });
  assert.throws(() => checkRelease({ root: dir }), /devDependencies are out of sync/);
}));

test('release rejects a stale Cargo package lock version', () => fixture(dir => {
  const filename = path.join(dir, 'src-tauri/Cargo.lock');
  writeFileSync(filename, readFileSync(filename, 'utf8').replace(/(name = "navkit"\r?\nversion = ")[^"]+/, '$1999.0.0'));
  assert.throws(() => checkRelease({ root: dir }), /Cargo.lock.*version/);
}));

test('release rejects a different pnpm root dependency version', () => fixture(dir => {
  const filename = path.join(dir, 'pnpm-lock.yaml');
  writeFileSync(filename, readFileSync(filename, 'utf8').replace(/(      playwright-core:\r?\n        specifier:[^\n]+\r?\n        version: )[\d.]+/, '$1999.0.0'));
  assert.throws(() => checkRelease({ root: dir }), /pnpm-lock.yaml playwright-core/);
}));

test('release build and publish check out the requested tag and depend on acceptance', () => {
  const source = readFileSync(path.join(root, '.github/workflows/release.yml'), 'utf8');
  assert.equal((source.match(/ref: \$\{\{ inputs\.tag \|\| github\.ref \}\}/g) ?? []).length, 3);
  assert.match(source, /build:[\s\S]*?needs: acceptance/);
  assert.match(source, /publish:[\s\S]*?needs: build/);
  assert.match(source, /node tools\/check-release\.mjs --tag "\$RELEASE_TAG"/);
  assert.match(source, /npm run test:ui/);
  assert.match(source, /cargo test --locked --manifest-path src-tauri\/Cargo\.toml --lib/);
  assert.doesNotMatch(source, /run: echo "tag=\$\{\{/);
});

test('default UI gate includes new data pipeline safety regressions', () => {
  const pkg = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8'));
  for (const filename of ['backup-consistency', 'data-generator-recipe', 'sql-export-batch', 'sync-diff-memory-api', 'sync-diff-option', 'release-readiness']) {
    assert.ok(pkg.scripts['test:ui'].split(' ').includes(`tools/tests/${filename}.test.mjs`), `${filename} is missing from test:ui`);
  }
});
