import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import { buildSync } from 'esbuild';

// Exercise shipped validators and UI. Only the Tauri/network boundary is replaced.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const bundle = buildSync({
  stdin: { contents: `export { openConnectionDialog } from './src/dialogs/ConnectionDialog'; export { driverApi } from './src/core/api'; export { createRuntimeProfileEditor } from './src/ui/RuntimeProfileEditor'; export * from './src/core/runtime-profile-editor';`, resolveDir: root },
  bundle: true, write: false, platform: 'node', format: 'cjs', target: 'es2022',
  external: ['@tauri-apps/*'], loader: { '.svg': 'dataurl', '.png': 'dataurl' }, logLevel: 'silent',
}).outputFiles[0].text;
const plain = value => JSON.parse(JSON.stringify(value));

class Element {
  constructor(tag = 'div') {
    this.tagName = tag.toLowerCase(); this.children = []; this.parentNode = null; this.style = {}; this.dataset = {}; this.attributes = new Map(); this.listeners = new Map();
    this.className = ''; this.value = ''; this.checked = false; this.disabled = false; this.hidden = false; this._text = ''; this._html = ''; this.id = '';
    this.classList = {
      add: (...names) => { this.className = [...new Set([...this.className.split(/\s+/).filter(Boolean), ...names])].join(' '); },
      remove: (...names) => { this.className = this.className.split(/\s+/).filter(name => !names.includes(name)).join(' '); },
      contains: name => this.className.split(/\s+/).includes(name),
      toggle: (name, state) => { const yes = state ?? !this.classList.contains(name); yes ? this.classList.add(name) : this.classList.remove(name); return yes; },
    };
  }
  set textContent(value) { this._text = String(value); this.children = []; }
  get textContent() { return this._text + this.children.map(child => child.textContent).join(''); }
  set innerHTML(value) { this._html = String(value); this._text = ''; this.children = []; }
  get innerHTML() { return this._html; }
  appendChild(child) { child.remove(); this.children.push(child); child.parentNode = this; return child; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  insertBefore(child, before) { child.remove(); const i = this.children.indexOf(before); this.children.splice(i < 0 ? this.children.length : i, 0, child); child.parentNode = this; }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); this.parentNode = null; }
  get firstChild() { return this.children[0] ?? null; }
  get isConnected() { return Boolean(this.parentNode); }
  setAttribute(name, value) { this.attributes.set(name, String(value)); if (name === 'id') this.id = String(value); }
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  removeAttribute(name) { this.attributes.delete(name); }
  addEventListener(name, handler) { const list = this.listeners.get(name) ?? []; list.push(handler); this.listeners.set(name, list); }
  async emit(name, extra = {}) { for (const handler of this.listeners.get(name) ?? []) await handler({ target: this, preventDefault() {}, ...extra }); }
  focus() { if (this.ownerDocument) this.ownerDocument.activeElement = this; }
  checkValidity() { return this.type !== 'number' || Number.isFinite(Number(this.value)) && Number(this.value) >= Number(this.min ?? 0) && Number(this.value) <= Number(this.max ?? 65535); }
  getClientRects() { for (let node = this; node; node = node.parentNode) if (node.hidden) return []; return [{}]; }
  matches(selector) {
    const excluded = [...selector.matchAll(/:not\(([^)]+)\)/g)].map(match => match[1]); if (excluded.some(value => this.matches(value))) return false;
    selector = selector.replace(/:not\([^)]+\)/g, '').trim(); const tag = selector.match(/^[\w-]+/); if (tag && this.tagName !== tag[0]) return false;
    if ([...selector.matchAll(/\.([\w-]+)/g)].some(match => !this.classList.contains(match[1]))) return false;
    for (const match of selector.matchAll(/\[([\w-]+)(?:=['"]?([^'"\]]+)['"]?)?\]/g)) {
      const value = this.getAttribute(match[1]) ?? (match[1] === 'disabled' ? (this.disabled ? '' : null) : this[match[1]] ?? null);
      if (value === null || match[2] !== undefined && String(value) !== match[2]) return false;
    }
    return true;
  }
  querySelectorAll(selector) {
    const alternatives = selector.split(',').map(value => value.trim()), out = [];
    const visit = node => { for (const child of node.children) { if (alternatives.some(value => child.matches(value))) out.push(child); visit(child); } };
    visit(this); return out;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}
function environment() {
  const document = { activeElement: null, createElement(tag) { const element = new Element(tag); element.ownerDocument = document; return element; } };
  document.body = document.createElement('body');
  const ipc = [], exported = { exports: {} };
  const context = vm.createContext({
    module: exported, exports: exported.exports, document, console, TextEncoder, TextDecoder, setTimeout, clearTimeout, URL,
    window: { __TAURI_INTERNALS__: {}, addEventListener() {}, removeEventListener() {}, innerWidth: 1600, innerHeight: 900 },
    require(name) {
      if (name === '@tauri-apps/api/core') return { invoke: async (command, args) => { ipc.push({ command, args }); return []; } };
      if (name === '@tauri-apps/plugin-dialog') return { open: async () => null };
      throw new Error(`Unexpected external boundary: ${name}`);
    },
  });
  vm.runInContext(bundle, context);
  // driverStatus.ts intentionally prewarms the local driver inventory on import. The
  // imported module must make exactly that benign call; every operation below starts
  // with an empty ledger, so HTTP discovery/session/SQL calls remain a hard failure.
  assert.deepEqual(ipc.map(entry => entry.command), ['list_driver_statuses']);
  assert.equal(ipc[0].args, undefined);
  ipc.length = 0;
  return { api: exported.exports, document, ipc };
}
const profile = () => ({
  endpoint: 'https://bridge.example:8443/', protocol: 'navkit-sql-tunnel/1',
  expected_server_id: '671d872f-9f15-47d1-968d-87126f9ca944', expected_target_id: 'de177ee0-af44-4f7d-9e1d-b1254f6a3fe7',
  target_alias: 'h2-qa', tls: { mode: 'verify_full', ca: null, cert: null, key: null }, connect_timeout_ms: 10_000,
});
const candidate = () => ({ ssh: null, tls: null, ssh_password: { action: 'keep' }, ssh_key_passphrase: { action: 'keep' }, http: profile(), http_bearer: { action: 'set', value: 'Bearer-test_only.+=;/@!' } });
const receipt = () => ({ connection_id: 'c_existing', endpoint_fingerprint: 'a'.repeat(64), revision: 'b'.repeat(64), ssh: null, tls: null, ssh_password_present: false, ssh_key_passphrase_present: false, http: profile(), http_bearer_present: true });
const savedConfig = () => ({ id: 'c_existing', name: 'H2 QA', kind: 'h2', host: 'localhost', port: 9092, user: 'sa', password: 'existing-local-ciphertext', database: 'qa', file: '', params: '', group: 'QA', read_only: true, keep_alive: true });
const inputIn = (container, label) => container.querySelectorAll('input,select,textarea').find(control => control.getAttribute('aria-label') === label);
function editor({ previous = null, editing = false, bindings = true, raw = '', httpAvailable = true, databaseTransportAvailable = false } = {}) {
  const env = environment(), container = env.document.createElement('div'), status = env.document.createElement('div'); status.id = 'http-runtime-status'; env.document.body.append(container, status);
  const implementation = typeof bindings === 'object' ? bindings : bindings ? { load: async () => previous, test: async () => ({ ok: true }) } : undefined;
  const value = env.api.createRuntimeProfileEditor({ container, status, connectionId: 'c_existing', editing, bindings: implementation, rawParameters: () => raw, pickFile: async () => null, httpAvailable, databaseTransportAvailable });
  return { ...env, value, container, status, input: label => inputIn(container, label) };
}
function connection({ existing = savedConfig(), bindings, onSaved = async () => {}, defaultKind = 'h2', selectedDriver } = {}) {
  const env = environment(), calls = [];
  env.api.driverApi.testConnection = async (...args) => { calls.push(args); return { ok: true }; };
  env.api.driverApi.options = async () => []; env.api.driverApi.statuses = async () => []; env.api.driverApi.list = async () => [];
  const dialog = env.api.openConnectionDialog(existing, [], onSaved, defaultKind, selectedDriver, bindings);
  const button = label => dialog.footer.querySelectorAll('button').find(control => control.textContent === label);
  return { ...env, dialog, calls, input: label => inputIn(dialog.body, label), button, save: () => button('保存').emit('click'), test: () => button('测试连接').emit('click'), status: () => dialog.body.querySelector('[role="status"]').textContent };
}
async function change(form, label, value) { const control = form.input(label); assert(control, `missing actual control ${label}`); control.value = value; await control.emit('input'); }
async function toggle(form, label, enabled) { const control = form.input(label); assert(control, `missing actual checkbox ${label}`); control.checked = enabled; await control.emit('change'); }
async function fillHttp(form, bearer = 'ui-test_only-token.+=') {
  await toggle(form, '启用 HTTP SQL 通道', true);
  const value = profile();
  for (const [label, entry] of [['HTTP HTTPS origin', value.endpoint], ['HTTP 服务 ID', value.expected_server_id], ['HTTP 目标 ID', value.expected_target_id], ['HTTP 目标 alias', value.target_alias], ['HTTP 连接期限（毫秒）', String(value.connect_timeout_ms)]]) await change(form, label, entry);
  if (bearer !== null) await change(form, 'HTTP Bearer', bearer);
}
const settle = () => new Promise(resolve => setImmediate(resolve));

test('valid explicit HTTPS origin and H2 identity produce no network or implicit target discovery', () => {
  const env = environment(); env.api.validateHttpProfile(profile()); env.api.validateRuntimeDraft(candidate(), null);
  assert.equal(env.ipc.length, 0, JSON.stringify(env.ipc)); assert.equal(env.api.validateHttpEndpoint(profile().endpoint).origin, 'https://bridge.example:8443');
});
for (const endpoint of ['https://bridge.example', 'HTTPS://BRIDGE.EXAMPLE:443/', 'https://127.0.0.1:8443/', 'https://[::1]:8443/']) test(`explicit valid HTTPS origin accepted: ${endpoint}`, () => {
  const api = environment().api; assert.equal(api.validateHttpEndpoint(endpoint).protocol, 'https:');
});
for (const endpoint of ['http://bridge.example/', 'https://user:pass@bridge.example/', 'https://bridge.example/sql', 'https://bridge.example/a/../', 'https://bridge.example/?target=qa', 'https://bridge.example/#qa', 'https://bridge.example//', 'https://bridge.example\\sql', ' https://bridge.example/', 'https://bridge.example/\n', 'https://bridge.example:99999/', 'https:///']) test(`raw HTTPS origin rejects downgrade or URL normalization: ${JSON.stringify(endpoint)}`, () => {
  assert.throws(() => environment().api.validateHttpEndpoint(endpoint), /HTTPS|HTTP/);
});
for (const [label, modify] of [
  ['unknown protocol', value => { value.protocol = 'rest/1'; }],
  ['missing service identity', value => { value.expected_server_id = ''; }],
  ['non-canonical uppercase service identity', value => { value.expected_server_id = value.expected_server_id.toUpperCase(); }],
  ['non-canonical target identity', value => { value.expected_target_id = value.expected_target_id.replaceAll('-', ''); }],
  ['empty alias', value => { value.target_alias = ''; }],
  ['alias trim would change identity', value => { value.target_alias = ' qa '; }],
  ['alias exceeds UTF8 byte budget', value => { value.target_alias = '中'.repeat(67); }],
  ['alias has a control character', value => { value.target_alias = 'qa\u0000other'; }],
  ['short timeout', value => { value.connect_timeout_ms = 999; }],
  ['long timeout', value => { value.connect_timeout_ms = 60001; }],
  ['fractional timeout', value => { value.connect_timeout_ms = 1000.5; }],
  ['NaN timeout', value => { value.connect_timeout_ms = NaN; }],
  ['infinite timeout', value => { value.connect_timeout_ms = Infinity; }],
  ['caller certificate identity override', value => { value.server_name = 'other.example'; }],
  ['extra plaintext token field', value => { value.bearer = 'forbidden'; }],
]) test(`HTTP profile rejects ${label}`, () => {
  const value = profile(); modify(value); assert.throws(() => environment().api.validateHttpProfile(value));
});
for (const mode of ['disable', 'prefer', 'require', 'verify_ca', 'insecure']) test(`outer HTTPS rejects non-VerifyFull mode ${mode}`, () => {
  const value = profile(); value.tls.mode = mode; assert.throws(() => environment().api.validateHttpProfile(value), /VerifyFull/);
});
for (const [cert, key] of [['client.pem', null], [null, 'client.key']]) test(`client authentication requires paired material ${cert}/${key}`, () => {
  const value = profile(); value.tls.cert = cert; value.tls.key = key; assert.throws(() => environment().api.validateHttpProfile(value), /成对/);
});
test('outer CA path and paired client files do not become database TLS settings', () => {
  const value = candidate(); value.http.tls.ca = 'C:/keys/root.pem'; value.http.tls.cert = 'C:/keys/client.pem'; value.http.tls.key = 'C:/keys/client.key';
  environment().api.validateRuntimeDraft(value, null); assert.equal(value.tls, null);
});
for (const value of ['', ' secret', 'secret ', 'line\r\nHeader:value', 'secret\tvalue', '中文', '\u0000', 'a'.repeat(16 * 1024 + 1)]) test(`HTTP Bearer rejects header-invalid bytes ${JSON.stringify(value.slice(0, 40))}`, () => {
  const draft = candidate(); draft.http_bearer.value = value; assert.throws(() => environment().api.validateRuntimeDraft(draft, null));
});
test('header-safe ASCII Bearer remains byte-exact including punctuation', () => {
  const draft = candidate(); draft.http_bearer.value = '!#$%&\'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~';
  const before = draft.http_bearer.value; environment().api.validateRuntimeDraft(draft, null); assert.equal(draft.http_bearer.value, before);
});
test('maximum-size ASCII token is valid without trimming', () => {
  const draft = candidate(); draft.http_bearer.value = '~'.repeat(16 * 1024); environment().api.validateRuntimeDraft(draft, null);
});
for (const action of ['keep', 'clear']) test(`active HTTP profile cannot ${action} a missing Bearer or fall back to direct connection`, () => {
  const draft = candidate(); draft.http_bearer = { action }; assert.throws(() => environment().api.validateRuntimeDraft(draft, null), /Bearer/);
});
test('clear Bearer requires disabling the HTTP profile explicitly', () => {
  const api = environment().api, draft = candidate(); draft.http_bearer = { action: 'clear' };
  assert.throws(() => api.validateRuntimeDraft(draft, receipt()), /Bearer/); draft.http = null; api.validateRuntimeDraft(draft, receipt());
});
for (const changeIdentity of [value => { value.endpoint = 'https://other.example/'; }, value => { value.expected_server_id = 'cd7b9d40-9a66-4f6f-a12a-0bfa2b4e45c5'; }, value => { value.expected_target_id = 'aaf16d29-dba6-41b5-9d66-adf0b8dd7c77'; }, value => { value.target_alias = 'other'; }]) test(`Keep token requires explicit rebind for changed target ${changeIdentity.toString()}`, () => {
  const api = environment().api, draft = candidate(); draft.http_bearer = { action: 'keep' }; changeIdentity(draft.http);
  assert.throws(() => api.validateRuntimeDraft(draft, receipt()), /重新绑定/); api.validateRuntimeDraft(draft, receipt(), true);
});
test('canonical same origin and timeout changes do not falsely require rebind', () => {
  const api = environment().api, draft = candidate(); draft.http_bearer = { action: 'keep' }; draft.http.endpoint = 'https://BRIDGE.EXAMPLE:8443'; draft.http.connect_timeout_ms = 12000;
  api.validateRuntimeDraft(draft, receipt()); assert.equal(api.httpProfileSameTarget(draft.http, receipt().http), true);
});
for (const changeMode of [value => { value.tls = { mode: 'disable', ca: null, cert: null, key: null }; }, value => { value.ssh = { host: 'bastion', port: 22, user: 'alice', authentication: 'password', key_file: null, known_hosts: 'pins' }; }, value => { value.ssh_password = { action: 'set', value: 'secret' }; }]) test(`HTTP is exclusive with database TLS/SSH ${changeMode.toString()}`, () => {
  const draft = candidate(); changeMode(draft); assert.throws(() => environment().api.validateRuntimeDraft(draft, null), /互斥|SSH/);
});
for (const changeReceipt of [value => { delete value.http_bearer_present; }, value => { delete value.http; }, value => { value.http_bearer_present = false; }, value => { value.http_bearer_present = 'true'; }, value => { value.http = undefined; }, value => { value.http.bearer = 'forbidden'; }, value => { value.token = 'forbidden'; }, value => { value.http.tls.mode = 'require'; }]) test(`malformed HTTP presence or leaked receipt rejected ${changeReceipt.toString()}`, () => {
  const value = receipt(); changeReceipt(value); assert.throws(() => environment().api.assertProfileView(value, 'c_existing'));
});
test('older SSH/TLS receipt and current HTTP-free receipt remain compatible', () => {
  const api = environment().api, older = receipt(); older.http = null; delete older.http; delete older.http_bearer_present; api.assertProfileView(older, 'c_existing');
  api.assertProfileView({ ...older, http: null, http_bearer_present: false }, 'c_existing');
});
for (const key of ['http_bearer', 'http_token', 'authorization', 'bearer', 'access_token']) test(`legacy plaintext ${key} is never silently migrated into params`, async () => {
  const raw = `${key}=keep-original`, form = editor({ raw }); await form.value.ready;
  assert.equal(form.api.hasPlaintextRuntimeSecret(form.api.runtimeParameterKeys(raw)), true); assert.throws(() => form.value.submission(), /明文/); assert.equal(raw, `${key}=keep-original`);
});

test('H2 HTTP controls require explicit enable and expose separate HTTPS evidence instructions', async () => {
  const form = editor(); await form.value.ready; assert.equal(form.value.usesHttp(), false); assert.equal(form.input('HTTP HTTPS origin').disabled, true); assert.equal(form.input('启用 HTTP SQL 通道').getAttribute('aria-expanded'), 'false');
  await fillHttp(form); assert.equal(form.value.usesHttp(), true); assert.equal(form.input('HTTP HTTPS origin').disabled, false); assert.equal(form.input('启用 HTTP SQL 通道').getAttribute('aria-expanded'), 'true');
  assert.match(form.container.textContent, /H2 2\.3\.232 REGULAR/); assert.match(form.container.textContent, /下游 TLS/); assert.match(form.container.textContent, /不会自动|不会.*密码/);
  const submitted = plain(form.value.submission()); assert.deepEqual(submitted.draft.http, profile()); assert.equal(submitted.draft.http_bearer.action, 'set'); assert.equal(submitted.draft.ssh, null); assert.equal(submitted.draft.tls, null);
});
test('loading HTTP profile shows only token presence and preserves original CAS revision', async () => {
  const form = editor({ previous: receipt(), editing: true }); assert.equal(form.input('启用 HTTP SQL 通道').disabled, true); assert.throws(() => form.value.submission(), /读取/);
  await form.value.ready; assert.equal(form.value.usesHttp(), true); assert.equal(form.input('HTTP Bearer').value, ''); assert.match(form.input('HTTP Bearer').placeholder, /已保存/); assert.equal(form.value.submission(), undefined);
  const submitted = plain(form.value.submission(true)); assert.equal(submitted.expected_revision, 'b'.repeat(64)); assert.equal(submitted.draft.http_bearer.action, 'keep'); assert.deepEqual(submitted.draft.http, profile());
});
test('HTTP disable and token clear are independent explicit actions', async () => {
  const form = editor({ previous: receipt(), editing: true }); await form.value.ready; await toggle(form, '清除本机 HTTP Bearer', true); assert.throws(() => form.value.submission(), /Bearer/);
  await toggle(form, '启用 HTTP SQL 通道', false); const submitted = form.value.submission(); assert.equal(submitted.draft.http, null); assert.equal(submitted.draft.http_bearer.action, 'clear');
});
test('HTTP rebind checkbox preserves the loaded revision and never discovers new identity', async () => {
  const form = editor({ previous: receipt(), editing: true }); await form.value.ready; await change(form, 'HTTP 目标 alias', 'other'); assert.throws(() => form.value.submission(), /重新绑定/);
  await toggle(form, '将本机通道配置重新绑定到新端点', true); const submitted = form.value.submission(); assert.equal(submitted.rebind_endpoint, true); assert.equal(submitted.expected_revision, 'b'.repeat(64)); assert.equal(submitted.draft.http_bearer.action, 'keep'); assert.equal(form.ipc.length, 0, JSON.stringify(form.ipc));
});
test('HTTP token, CA and imported identities never appear in raw connection parameters', async () => {
  const raw = '{"fetch_size":1844674407370955161512345}', form = editor({ raw }); await fillHttp(form, 'unmistakable-http-test-token'); await change(form, 'HTTP 外层 CA 证书', 'C:/keys/http-root.pem');
  const submitted = form.value.submission(); assert.equal(submitted.draft.http_bearer.value, 'unmistakable-http-test-token'); assert.equal(submitted.draft.http.tls.ca, 'C:/keys/http-root.pem'); assert.equal(raw, '{"fetch_size":1844674407370955161512345}');
});
test('HTTP profile cannot overwrite unresolved old SSL settings', async () => {
  const form = editor({ raw: 'sslmode=require' }); await fillHttp(form); assert.throws(() => form.value.submission(), /叠加/);
});
test('HTTP draft without local profile bindings fails closed instead of storing a plaintext token', async () => {
  const form = editor({ bindings: false }); await fillHttp(form); assert.throws(() => form.value.submission(), /尚未接线/);
});
test('busy state freezes every active HTTP control and dispose erases Bearer', async () => {
  const form = editor(); await fillHttp(form); form.value.setBusy(true); assert(form.container.querySelectorAll('input,select,button').every(control => control.disabled)); form.value.setBusy(false);
  assert.equal(form.input('HTTP Bearer').disabled, false); form.value.dispose(); assert.equal(form.input('HTTP Bearer').value, ''); assert(form.container.querySelectorAll('input,select,button').every(control => control.disabled));
});
test('HTTP controls have native names and linked status, and expand control points at fields', async () => {
  const form = editor(); await fillHttp(form); const enable = form.input('启用 HTTP SQL 通道'); assert(enable.getAttribute('aria-controls'));
  for (const label of ['HTTP HTTPS origin', 'HTTP 服务 ID', 'HTTP 目标 ID', 'HTTP 目标 alias', 'HTTP 连接期限（毫秒）', 'HTTP 外层 CA 证书', 'HTTP 外层客户端证书', 'HTTP 外层客户端私钥', 'HTTP Bearer', '清除本机 HTTP Bearer']) {
    const control = form.input(label); assert(control); assert.equal(control.getAttribute('aria-describedby'), 'http-runtime-status');
  }
});
test('late HTTP receipt cannot restore fields after dialog disposal', async () => {
  let resolve; const form = editor({ editing: true, bindings: { load: () => new Promise(done => { resolve = done; }), test: async () => ({ ok: true }) } });
  form.value.dispose(); resolve(receipt()); await form.value.ready; assert.equal(form.input('HTTP HTTPS origin').value, ''); assert.equal(form.input('HTTP Bearer').value, ''); assert.equal(form.value.usesHttp(), false);
});
test('non-H2 editor never offers hidden HTTP enable controls and refuses existing HTTP settings', async () => {
  const form = editor({ httpAvailable: false, databaseTransportAvailable: true, previous: receipt(), editing: true }); await form.value.ready;
  assert.equal(form.input('启用 HTTP SQL 通道'), undefined); assert.throws(() => form.value.submission(), /读取失败|不支持/);
});
for (const kind of ['mysql', 'postgresql', 'kafka', 'mqtt', 'odbc']) test(`HTTP SQL controls are not offered to unverified ${kind} entry`, () => {
  const form = connection({ existing: { ...savedConfig(), kind }, bindings: { load: async () => null, test: async () => ({ ok: true }) } }); assert.equal(form.input('启用 HTTP SQL 通道'), undefined); form.dialog.close();
});
test('custom JDBC driver remains authoritative and cannot acquire H2 HTTP profile implicitly', () => {
  const form = connection({ existing: null, selectedDriver: { id: 'jdbc-custom', name: 'Custom JDBC', protocol: 'jdbc', template: 'jdbc:h2:mem:custom' }, bindings: { load: async () => receipt(), test: async () => ({ ok: true }) } });
  assert.equal(form.input('启用 HTTP SQL 通道'), undefined); form.dialog.close();
});
test('real H2 form tests HTTP separately and uploads neither typed nor stored database password', async () => {
  const seen = [], form = connection({ bindings: { load: async () => receipt(), test: async (...args) => { seen.push(plain(args)); return { ok: true, version: 'HTTP server identity verified' }; } } });
  await settle(); await change(form, '密码', 'must-never-leave-local-dialog'); await form.test();
  assert.equal(form.calls.length, 0); assert.equal(seen.length, 1, form.status()); assert.equal(seen[0][0].kind, 'h2'); assert.equal(seen[0][0].password, ''); assert.equal(seen[0][3], false); assert.equal(seen[0][1], 'c_existing'); assert.equal(seen[0][2].draft.http_bearer.action, 'keep'); assert.match(form.status(), /identity verified/);
  assert(!JSON.stringify(seen[0]).includes('must-never-leave-local-dialog')); assert(!JSON.stringify(seen[0]).includes('existing-local-ciphertext'));
});
test('saving HTTP form submits independent encrypted-profile draft and does not save typed database password', async () => {
  let saved; const form = connection({ bindings: { load: async () => receipt(), test: async () => ({ ok: true }) }, onSaved: value => { saved = plain(value); } });
  await settle(); await change(form, '密码', 'do-not-save-this-for-http'); await change(form, 'HTTP Bearer', 'new-test-http-bearer'); await form.save();
  assert(saved, form.status()); assert.equal(saved.password, undefined); assert.equal(saved.config.password, 'existing-local-ciphertext'); assert.equal(saved.config.params, ''); assert.equal(saved.config.read_only, true); assert.equal(saved.config.keep_alive, true); assert.equal(saved.runtimeProfile.draft.http_bearer.value, 'new-test-http-bearer'); assert.equal(saved.runtimeProfile.expected_revision, 'b'.repeat(64));
  assert(!JSON.stringify(saved.config).includes('new-test-http-bearer')); assert(!JSON.stringify(saved).includes('do-not-save-this-for-http'));
});
test('unchanged HTTP profile survives ordinary connection save without a new local revision', async () => {
  let saved; const form = connection({ bindings: { load: async () => receipt(), test: async () => ({ ok: true }) }, onSaved: value => { saved = plain(value); } });
  await settle(); await form.save(); assert(saved, form.status()); assert.equal(saved.runtimeProfile, undefined); assert.equal(saved.password, undefined); assert.equal(saved.config.password, 'existing-local-ciphertext');
});
test('HTTP connection test rejects missing token before either transport can send anything', async () => {
  const seen = [], form = connection({ existing: { ...savedConfig(), id: '', password: '' }, bindings: { load: async () => null, test: async (...args) => { seen.push(args); return { ok: true }; } } });
  await fillHttp(form, null); await form.test(); assert.equal(seen.length, 0); assert.equal(form.calls.length, 0); assert.match(form.status(), /Bearer/);
});
test('failed HTTP test preserves input and never retries using the ordinary H2 connection', async () => {
  const seen = [], form = connection({ bindings: { load: async () => receipt(), test: async (...args) => { seen.push(args); throw new Error('HTTP_TARGET_ID_MISMATCH'); } } });
  await settle(); await change(form, 'HTTP Bearer', 'retained-http-test-token'); await form.test(); assert.equal(seen.length, 1); assert.equal(form.calls.length, 0); assert.equal(form.input('HTTP Bearer').value, 'retained-http-test-token'); assert.match(form.status(), /HTTP_TARGET_ID_MISMATCH/); assert.equal(form.button('测试连接').disabled, false);
});
test('failed HTTP profile load cannot silently choose an ordinary H2 connection', async () => {
  const seen = [], form = connection({ bindings: { load: async () => { throw new Error('decryption failed'); }, test: async (...args) => { seen.push(args); return { ok: true }; } } });
  await settle(); await form.test(); await form.save(); assert.equal(seen.length, 0); assert.equal(form.calls.length, 0); assert.match(form.status(), /读取失败/);
});
for (const [label, bindings] of [
  ['unavailable local profile binding', undefined],
  ['missing referenced local profile', { load: async () => null, test: async () => { throw new Error('must not be called'); } }],
]) test(`referenced H2 HTTP connection never falls back after ${label}`, async () => {
  let saves = 0;
  const form = connection({ existing: { ...savedConfig(), params: `runtime_profile_id=${'b'.repeat(64)}` }, bindings, onSaved: () => { saves++; } });
  await settle(); await form.test(); await form.save();
  assert.equal(form.calls.length, 0); assert.equal(saves, 0); assert.match(form.status(), /不能.*回退/);
});
test('copying a referenced HTTP connection cannot reuse another connection local token implicitly', async () => {
  let saves = 0;
  const form = connection({ existing: { ...savedConfig(), id: '', params: `runtime_profile_id=${'b'.repeat(64)}` }, bindings: { load: async () => receipt(), test: async () => { throw new Error('must not be called'); } }, onSaved: () => { saves++; } });
  await form.test(); await form.save(); assert.equal(saves, 0); assert.equal(form.calls.length, 0); assert.match(form.status(), /不能.*回退/);
});
test('ordinary H2 with no channel reference remains an explicit direct connection', async () => {
  const form = connection({ existing: savedConfig(), bindings: undefined }); await form.test();
  assert.equal(form.calls.length, 1); assert.equal(form.calls[0][0].kind, 'h2'); assert.equal(form.input('启用 HTTP SQL 通道').checked, false);
});
test('busy H2 form captures HTTP fields and prevents a second simultaneous test', async () => {
  let resolve, count = 0; const form = connection({ bindings: { load: async () => receipt(), test: () => { count++; return new Promise(done => { resolve = done; }); } } }); await settle();
  const pending = form.test(); assert.equal(count, 1); assert.equal(form.input('启用 HTTP SQL 通道').disabled, true); assert.equal(form.input('HTTP 目标 alias').disabled, true); await form.test(); assert.equal(count, 1);
  resolve({ ok: false, message: 'controlled test completion' }); await pending; assert.equal(form.input('HTTP 目标 alias').disabled, false); assert.match(form.status(), /controlled test completion/);
});
