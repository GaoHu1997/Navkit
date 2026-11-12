import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import { buildSync } from 'esbuild';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const bundle = buildSync({ stdin: { contents: `export { openConnectionDialog } from './src/dialogs/ConnectionDialog'; export { driverApi } from './src/core/api'; export { createRuntimeProfileEditor } from './src/ui/RuntimeProfileEditor'; export * from './src/core/runtime-profile-editor';`, resolveDir: root }, bundle: true, write: false, platform: 'node', format: 'cjs', target: 'es2022', external: ['@tauri-apps/*'], loader: { '.svg': 'dataurl', '.png': 'dataurl' }, logLevel: 'silent' }).outputFiles[0].text;
const plain = value => JSON.parse(JSON.stringify(value));
class Element {
  constructor(tag = 'div') {
    this.tagName = tag.toLowerCase(); this.children = []; this.parentNode = null; this.style = {}; this.dataset = {}; this.attributes = new Map(); this.listeners = new Map();
    this.className = ''; this.value = ''; this.checked = false; this.disabled = false; this.hidden = false; this._text = ''; this._html = ''; this.id = '';
    this.classList = { add: (...names) => { this.className = [...new Set([...this.className.split(/\s+/).filter(Boolean), ...names])].join(' '); }, remove: (...names) => { this.className = this.className.split(/\s+/).filter(n => !names.includes(n)).join(' '); }, contains: name => this.className.split(/\s+/).includes(name), toggle: (name, state) => { const yes = state ?? !this.classList.contains(name); yes ? this.classList.add(name) : this.classList.remove(name); return yes; } };
  }
  set textContent(value) { this._text = String(value); this.children = []; } get textContent() { return this._text + this.children.map(child => child.textContent).join(''); }
  set innerHTML(value) { this._html = String(value); this._text = ''; this.children = []; } get innerHTML() { return this._html; }
  appendChild(child) { child.remove(); this.children.push(child); child.parentNode = this; return child; } append(...children) { children.forEach(child => this.appendChild(child)); }
  insertBefore(child, before) { child.remove(); const i = this.children.indexOf(before); this.children.splice(i < 0 ? this.children.length : i, 0, child); child.parentNode = this; }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); this.parentNode = null; }
  get firstChild() { return this.children[0] ?? null; } get isConnected() { return Boolean(this.parentNode); }
  setAttribute(name, value) { this.attributes.set(name, String(value)); if (name === 'id') this.id = String(value); } getAttribute(name) { return this.attributes.get(name) ?? null; } removeAttribute(name) { this.attributes.delete(name); }
  addEventListener(name, handler) { const list = this.listeners.get(name) ?? []; list.push(handler); this.listeners.set(name, list); }
  async emit(name, extra = {}) { for (const handler of this.listeners.get(name) ?? []) await handler({ target: this, preventDefault() {}, ...extra }); }
  focus() { if (this.ownerDocument) this.ownerDocument.activeElement = this; }
  checkValidity() { return this.type !== 'number' || Number.isFinite(Number(this.value)) && Number(this.value) >= Number(this.min ?? 0) && Number(this.value) <= Number(this.max ?? 65535); }
  getClientRects() { for (let node = this; node; node = node.parentNode) if (node.hidden) return []; return [{}]; }
  matches(selector) {
    const excluded = [...selector.matchAll(/:not\(([^)]+)\)/g)].map(m => m[1]); if (excluded.some(s => this.matches(s))) return false;
    selector = selector.replace(/:not\([^)]+\)/g, '').trim(); const tag = selector.match(/^[\w-]+/); if (tag && this.tagName !== tag[0]) return false;
    if ([...selector.matchAll(/\.([\w-]+)/g)].some(m => !this.classList.contains(m[1]))) return false;
    for (const m of selector.matchAll(/\[([\w-]+)(?:=['"]?([^'"\]]+)['"]?)?\]/g)) { const value = this.getAttribute(m[1]) ?? (m[1] === 'disabled' ? (this.disabled ? '' : null) : this[m[1]] ?? null); if (value === null || m[2] !== undefined && String(value) !== m[2]) return false; }
    return true;
  }
  querySelectorAll(selector) { const alternatives = selector.split(',').map(s => s.trim()), out = []; const visit = node => { for (const child of node.children) { if (alternatives.some(s => child.matches(s))) out.push(child); visit(child); } }; visit(this); return out; }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}
function environment() {
  const document = { activeElement: null, createElement(tag) { const element = new Element(tag); element.ownerDocument = document; return element; } }; document.body = document.createElement('body');
  const ipc = [], exported = { exports: {} };
  const context = vm.createContext({ module: exported, exports: exported.exports, document, console, TextEncoder, TextDecoder, setTimeout, clearTimeout, URL, window: { __TAURI_INTERNALS__: {}, addEventListener() {}, removeEventListener() {}, innerWidth: 1600, innerHeight: 900 }, require(name) { if (name === '@tauri-apps/api/core') return { invoke: async (command, args) => { ipc.push({ command, args }); return []; } }; if (name === '@tauri-apps/plugin-dialog') return { open: async () => null }; throw new Error(`Unexpected external boundary: ${name}`); } });
  vm.runInContext(bundle, context); return { api: exported.exports, document, ipc };
}
const noTls = () => ({ mode: 'disable', ca: null, cert: null, key: null });
const ssh = () => ({ host: 'bastion.example', port: 2222, user: 'alice', authentication: 'password', key_file: null, known_hosts: 'C:/keys/known_hosts' });
const draft = () => ({ ssh: ssh(), tls: noTls(), ssh_password: { action: 'set', value: 'secret-中文;&=' }, ssh_key_passphrase: { action: 'keep' } });
const view = () => ({ connection_id: 'c_existing', endpoint_fingerprint: 'a'.repeat(64), revision: 'b'.repeat(64), ssh: ssh(), tls: noTls(), ssh_password_present: true, ssh_key_passphrase_present: false });
function editor({ previous = null, editing = false, bindings = true, raw = '' } = {}) {
  const env = environment(), container = env.document.createElement('div'), status = env.document.createElement('div'); status.id = 'runtime-status'; env.document.body.append(container, status);
  const implementation = typeof bindings === 'object' ? bindings : bindings ? { load: async () => previous, test: async () => ({ ok: true }) } : undefined;
  const value = env.api.createRuntimeProfileEditor({ container, status, connectionId: 'c_existing', editing, bindings: implementation, rawParameters: () => raw, pickFile: async () => null });
  const input = label => container.querySelectorAll('input,select').find(el => el.getAttribute('aria-label') === label);
  return { ...env, value, container, status, input };
}
async function change(form, label, value) { const control = form.input(label); assert(control, `missing actual control ${label}`); control.value = value; await control.emit('input'); }

for (const mode of ['DISABLE', 'invalid', 'verify-full ', 'true', '']) test(`TLS parsing does not silently downgrade ${JSON.stringify(mode)}`, () => { const { api } = environment(); if (!mode) assert.equal(api.tlsModeFromUi(mode), null); else assert.throws(() => api.tlsModeFromUi(mode), /TLS/); });
test('parameter inspection preserves original JSON number and encoded delimiter bytes', () => { const { api } = environment(), raw = '{"server_timeout":1844674407370955161512345,"sslmode":"verify-full","ssh_password":"x;&=中文"}'; assert.deepEqual(plain(api.runtimeParameterKeys(raw)), ['server_timeout', 'sslmode', 'ssh_password']); assert.equal(raw.includes('1844674407370955161512345'), true); assert.equal(api.hasPlaintextRuntimeSecret(api.runtimeParameterKeys(raw)), true); });
for (const raw of ['sslmode', 'abc=1;broken', '[]', 'null', '{"x":']) test(`malformed option token remains rejected ${raw}`, () => { const { api } = environment(); assert.throws(() => api.runtimeParameterKeys(raw)); });
for (const key of ['tls-mode', 'sslrootcert', 'ssh_known_hosts', 'insecure', 'client_key', 'trust-server-certificate']) test(`runtime key ${key} cannot coexist with a profile`, () => assert.equal(environment().api.isRuntimeParameter(key), true));
for (const modify of [d => { d.ssh.known_hosts = ''; }, d => { d.ssh.port = 0; }, d => { d.ssh.port = 1.5; }, d => { d.ssh.authentication = 'fallback'; }, d => { d.tls.mode = 'prefer'; }, d => { d.ssh.host = 'h\n'; }, d => { d.ssh_password.value = ''; }, d => { d.ssh_password.value = '中'.repeat(6000); }, d => { d.ssh = null; }]) test(`strict runtime draft rejects invalid input ${modify.toString()}`, () => { const { api } = environment(), candidate = draft(); modify(candidate); assert.throws(() => api.validateRuntimeDraft(candidate, null)); });
test('secret strings are not tokenized or trimmed and Keep requires presence', () => { const { api } = environment(), candidate = draft(); api.validateRuntimeDraft(candidate, null); assert.equal(candidate.ssh_password.value, 'secret-中文;&='); candidate.ssh_password = { action: 'keep' }; api.validateRuntimeDraft(candidate, view()); assert.throws(() => api.validateRuntimeDraft(candidate, { ...view(), ssh_password_present: false })); });
for (const modify of [v => { v.connection_id = 'other'; }, v => { v.revision = 'short'; }, v => { v.endpoint_fingerprint = ''; }, v => { v.ssh_password_present = 'true'; }, v => { v.tls.mode = 'bad'; }, v => { v.ssh.password = 'forbidden'; }, v => { v.secrets = { password: 'forbidden' }; }, v => { v.ssh = undefined; }]) test(`malformed profile view cannot become empty settings ${modify.toString()}`, () => { const { api } = environment(), candidate = view(); modify(candidate); assert.throws(() => api.assertProfileView(candidate, 'c_existing')); });
test('null is the only empty-profile receipt', () => { const { api } = environment(); api.assertProfileView(null, 'c_existing'); for (const v of [false, undefined, 0, []]) assert.throws(() => api.assertProfileView(v, 'c_existing')); });

test('untouched advanced parameters are preserved when no profile exists', async () => { const form = editor({ raw: 'sslmode=require;application_name=原文' }); await form.value.ready; assert.equal(form.value.submission(), undefined); form.value.dispose(); });
test('old plaintext secret is blocked without deleting or serializing it', async () => { const raw = 'ssh_password=retained;sslmode=disable', form = editor({ raw }); await form.value.ready; assert.throws(() => form.value.submission(), /明文/); assert.equal(raw, 'ssh_password=retained;sslmode=disable'); });
test('loaded profile only shows secret presence and keeps original revision', async () => { const form = editor({ previous: view(), editing: true }); assert.equal(form.input('SSH 主机').disabled, true); assert.throws(() => form.value.submission(), /读取/); await form.value.ready; assert.equal(form.input('SSH 密码').value, ''); assert.match(form.input('SSH 密码').placeholder, /已保存/); assert.equal(form.value.submission(), undefined); assert.deepEqual(plain(form.value.submission(true)), { draft: { ssh: ssh(), tls: noTls(), ssh_password: { action: 'keep' }, ssh_key_passphrase: { action: 'keep' } }, expected_revision: 'b'.repeat(64), rebind_endpoint: false }); });
test('load failure preserves inputs and forbids profile overwrite', async () => { const form = editor({ editing: true, bindings: { load: async () => { throw new Error('offline'); }, test: async () => ({ ok: true }) } }); await form.value.ready; assert.throws(() => form.value.submission(), /读取失败/); assert.match(form.status.textContent, /不会|不能覆盖/); });
test('forged profile identity is refused before entering controls', async () => { const form = editor({ previous: { ...view(), connection_id: 'other' }, editing: true }); await form.value.ready; assert.equal(form.input('SSH 主机').value, ''); assert.throws(() => form.value.submission(), /回执身份不符/); });
test('new secret never becomes a connection option and clear is explicit', async () => { const form = editor({ previous: view(), editing: true }); await form.value.ready; await change(form, 'SSH 密码', 'new;&=密码'); const result = plain(form.value.submission()); assert.equal(result.draft.ssh_password.action, 'set'); assert.equal(result.draft.ssh_password.value, 'new;&=密码'); form.input('清除本机私钥口令').checked = true; await form.input('清除本机私钥口令').emit('change'); assert.equal(form.value.submission().draft.ssh_key_passphrase.action, 'clear'); });
test('runtime field edits without IPC bindings cannot fall back to plaintext params', async () => { const form = editor({ bindings: false }); await change(form, 'SSL/TLS 模式', 'disable'); assert.throws(() => form.value.submission(), /尚未接线/); });
test('loaded runtime profile cannot silently overwrite old TLS aliases', async () => { const form = editor({ previous: view(), editing: true, raw: 'tls-mode=verify-full' }); await form.value.ready; assert.throws(() => form.value.submission(), /不能.*叠加/); });

test('an admitted runtime profile reference is retained without rewriting unchanged secrets', async () => {
  const form = editor({ previous: view(), editing: true, raw: `runtime_profile_id=${'b'.repeat(64)}` }); await form.value.ready;
  assert.equal(form.value.submission(), undefined); assert.equal(form.value.submission(true).expected_revision, 'b'.repeat(64)); assert.equal(form.value.submission(true).draft.ssh_password.action, 'keep'); form.value.dispose();
});
test('rebind remains opt-in and retains the loaded revision proof', async () => {
  const form = editor({ previous: view(), editing: true }); await form.value.ready; assert.equal(form.value.submission(true).rebind_endpoint, false);
  const rebind = form.input('将本机通道配置重新绑定到新端点'); assert(rebind); rebind.checked = true; await rebind.emit('change');
  const submission = form.value.submission(); assert.equal(submission.rebind_endpoint, true); assert.equal(submission.expected_revision, 'b'.repeat(64)); assert.equal(submission.draft.ssh_password.action, 'keep'); form.value.dispose();
});
test('saving untouched profile fields does not create a new runtime revision', async () => {
  let saved; const form = connection({ bindings: { load: async () => view(), test: async () => ({ ok: true }) }, onSaved: value => { saved = value; } });
  await new Promise(resolve => setImmediate(resolve)); await form.save(); assert(saved, form.status()); assert.equal(saved.runtimeProfile, undefined); assert.equal(saved.config.password, 'ciphertext'); assert.equal(saved.password, undefined);
});
test('busy freezes every runtime control and dispose erases DOM secrets', async () => { const form = editor({ previous: view(), editing: true }); await form.value.ready; await change(form, 'SSH 密码', 'secret'); form.value.setBusy(true); assert(form.container.querySelectorAll('input,select,button').every(el => el.disabled)); form.value.setBusy(false); assert(form.container.querySelectorAll('input,select,button').every(el => !el.disabled)); form.value.dispose(); assert.equal(form.input('SSH 密码').value, ''); assert.equal(form.input('私钥口令').value, ''); });
test('late asynchronous profile load never restores disposed secret state', async () => { let resolve; const form = editor({ editing: true, bindings: { load: () => new Promise(done => { resolve = done; }), test: async () => ({ ok: true }) } }); form.value.dispose(); resolve(view()); await form.value.ready; assert.equal(form.input('SSH 主机').value, ''); assert.equal(form.input('SSH 密码').value, ''); });

const config = () => ({ id: 'c_existing', name: 'Existing', kind: 'mysql', host: 'db.example', port: 3306, user: 'root', password: 'ciphertext', database: 'test', file: '', params: '', group: 'QA', read_only: true, keep_alive: true });
function connection({ existing = config(), bindings, onSaved = async () => {} } = {}) {
  const env = environment(), calls = [];
  env.api.driverApi.testConnection = async (...args) => { calls.push(args); return { ok: true }; };
  env.api.driverApi.options = async () => []; env.api.driverApi.statuses = async () => []; env.api.driverApi.list = async () => [];
  const dialog = env.api.openConnectionDialog(existing, [], onSaved, 'mysql', undefined, bindings);
  const input = label => dialog.body.querySelectorAll('input,select,textarea').find(el => el.getAttribute('aria-label') === label);
  const button = label => dialog.footer.querySelectorAll('button').find(el => el.textContent === label);
  return { ...env, dialog, calls, input, button, save: () => button('保存').emit('click'), test: () => button('测试连接').emit('click'), status: () => dialog.body.querySelector('[role="status"]').textContent };
}
test('actual connection form submits local profile separately and preserves ciphertext/flags', async () => { let saved; const form = connection({ bindings: { load: async () => view(), test: async () => ({ ok: true }) }, onSaved: value => { saved = value; } }); await new Promise(resolve => setImmediate(resolve)); await change(form, 'SSH 用户名', 'alice-updated'); await form.save(); assert(saved, form.status()); assert.equal(saved.config.params, ''); assert.equal(saved.config.password, 'ciphertext'); assert.equal(saved.password, undefined); assert.equal(saved.config.read_only, true); assert.equal(saved.config.keep_alive, true); assert.equal(saved.runtimeProfile.expected_revision, 'b'.repeat(64)); assert(!JSON.stringify(saved.config).includes('ssh_password')); });
test('actual transient profile test never calls ordinary saved-connection path', async () => { const seen = []; const form = connection({ bindings: { load: async () => view(), test: async (...args) => { seen.push(args); return { ok: true, version: 'test-server' }; } } }); await new Promise(resolve => setImmediate(resolve)); await form.test(); assert.equal(seen.length, 1, form.status()); assert.equal(form.calls.length, 0); assert.equal(seen[0][0].params, ''); assert.equal(seen[0][1], 'c_existing'); assert.equal(seen[0][3], true); assert.match(form.status(), /test-server/); });
test('ordinary copied connection retains stable ID and untouched raw params with no profile service', async () => { let saved; const form = connection({ existing: { ...config(), id: '', password: '', params: '{"x":1844674407370955161512345}' }, onSaved: value => { saved = value; } }); await form.save(); assert(saved.config.id); assert.notEqual(saved.config.id, 'c_existing'); assert.equal(saved.config.params, '{"x":1844674407370955161512345}'); assert.equal(saved.runtimeProfile, undefined); });
test('rejected save freezes captured profile, prevents duplicate save and retains input for retry', async () => { let reject, attempts = 0; const form = connection({ bindings: { load: async () => view(), test: async () => ({ ok: true }) }, onSaved: () => { attempts++; return new Promise((_, no) => { reject = no; }); } }); await new Promise(resolve => setImmediate(resolve)); const pending = form.save(); assert.equal(attempts, 1, form.status()); assert.equal(form.input('SSH 主机').disabled, true); await form.save(); assert.equal(attempts, 1); reject(new Error('backend rejected')); await pending; assert.equal(form.button('保存').disabled, false); assert.equal(form.input('SSH 主机').value, 'bastion.example'); assert.match(form.status(), /backend rejected/); });
