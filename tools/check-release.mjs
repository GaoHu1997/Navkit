import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';

const defaultRoot = fileURLToPath(new URL('../', import.meta.url));

// Fail before expensive builds or uploads if the release identity is inconsistent.
export function checkRelease({ root = defaultRoot, tag } = {}) {
  const text = filename => readFileSync(path.join(root, filename), 'utf8');
  const json = filename => JSON.parse(text(filename));
  const pkg = json('package.json');
  const lock = json('package-lock.json');
  const tauri = json('src-tauri/tauri.conf.json');
  const cargoPackage = text('src-tauri/Cargo.toml').match(/\[package\]([\s\S]*?)(?=\n\[|$)/)?.[1];
  const cargoVersion = cargoPackage?.match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1];
  const cargoLocked = text('src-tauri/Cargo.lock').split('[[package]]')
    .find(block => /^name\s*=\s*"navkit"\s*$/m.test(block));
  const cargoLockVersion = cargoLocked?.match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1];
  const errors = [];
  const versionPattern = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
  if (typeof pkg.version !== 'string' || !versionPattern.test(pkg.version)) errors.push('package.json version must be valid SemVer');
  for (const [filename, version] of [
    ['package-lock.json', lock.version], ['package-lock.json packages[""]', lock.packages?.['']?.version],
    ['src-tauri/tauri.conf.json', tauri.version], ['src-tauri/Cargo.toml', cargoVersion],
    ['src-tauri/Cargo.lock', cargoLockVersion],
  ]) {
    if (version !== pkg.version) errors.push(`${filename}: version ${JSON.stringify(version)} does not match ${pkg.version}`);
  }
  for (const key of ['dependencies', 'devDependencies']) {
    if (!isDeepStrictEqual(pkg[key] ?? {}, lock.packages?.['']?.[key] ?? {})) errors.push(`package-lock.json ${key} are out of sync; update the lock file before release`);
  }
  // Compare the simple root importer emitted by pnpm (no YAML execution).
  const importer = text('pnpm-lock.yaml').split(/^packages:\s*$/m)[0];
  const pnpmVersions = new Map(); let dependency;
  for (const line of importer.split(/\r?\n/)) {
    const name = /^      (?:'([^']+)'|([^\s:]+)):\s*$/.exec(line);
    if (name) dependency = name[1] ?? name[2];
    const version = /^        version:\s*'?([^\s'(]+)/.exec(line);
    if (version && dependency) pnpmVersions.set(dependency, version[1]);
  }
  for (const name of Object.keys({ ...pkg.dependencies, ...pkg.devDependencies })) {
    const expected = lock.packages?.[`node_modules/${name}`]?.version;
    if (pnpmVersions.get(name) !== expected) errors.push(`pnpm-lock.yaml ${name}: ${pnpmVersions.get(name)} does not match npm lock ${expected}`);
  }
  const override = json('src-tauri/tauri.release.conf.json');
  if (override.version !== undefined && override.version !== pkg.version) errors.push('tauri.release.conf.json overrides the release version');
  if (tag !== undefined && (tag !== `v${pkg.version}` || !/^v[0-9A-Za-z.+-]+$/.test(tag))) {
    errors.push(`release tag ${JSON.stringify(tag)} must be exactly v${pkg.version}`);
  }
  if (errors.length) throw new Error(errors.join('\n'));
  return { version: pkg.version, tag: tag ?? null };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const args = process.argv.slice(2);
    if (args.length && (args.length !== 2 || args[0] !== '--tag' || !args[1])) throw new Error('Usage: node tools/check-release.mjs [--tag v<version>]');
    const result = checkRelease({ tag: args[1] });
    console.log(`Release identity verified: ${result.version}${result.tag ? ` (${result.tag})` : ''}`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
