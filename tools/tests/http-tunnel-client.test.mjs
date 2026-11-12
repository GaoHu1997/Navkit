import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir, mkdtemp, rm, access, stat } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import https from 'node:https';
import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { buildReference } from '../../bridge/http-tunnel/build.mjs';

// Fresh Rust compilation belongs to the parent. This runner accepts only an
// explicitly named existing libtest executable, and never invokes cargo/link.
export const RUST_EXACT_TEST = 'http_tunnel::integration_tests::real_https_java_h2_rust_client_contract';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const direct = !process.env.NODE_TEST_CONTEXT && path.resolve(process.argv[1] || '') === fileURLToPath(import.meta.url);
const selfCheck = direct && process.argv.includes('--self-check');
const executable = process.env.NAVKIT_HTTP_TUNNEL_TEST_EXE;
const protocol = 'navkit-sql-tunnel/1';
const faults = ['cap_redirect', 'cap_html', 'open_foreign_binding', 'open_target_generation',
  'stream_missing_final', 'stream_truncated_final', 'stream_final_then_bad', 'stream_foreign_binding',
  'stream_target_generation', 'stream_redirect', 'stream_html', 'control_foreign_status'];
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

async function fixtureLifecycle(work) {
  const temp = await mkdtemp(path.join(os.tmpdir(), 'navkit-http-tunnel-client-'));
  const children = new Set(), servers = [], sockets = new Set(), agents = [];
  const token = randomBytes(32).toString('base64url'), password = randomBytes(24).toString('hex');
  const java = process.env.NAVKIT_JAVA || (process.platform === 'win32' ? 'D:/devlop/java/jdk-11.0.28/bin/java.exe' : 'java');
  const keytool = process.env.NAVKIT_KEYTOOL || (path.isAbsolute(java)
    ? path.join(path.dirname(java), process.platform === 'win32' ? 'keytool.exe' : 'keytool') : 'keytool');
  const jar = path.resolve(process.env.NAVKIT_H2_JAR || path.join(root, 'tools/acceptance/datasources/h2/h2-2.3.232.jar'));
  const redact = value => String(value).split(token).join('<temporary bearer>').split(password).join('<temporary store password>');
  function childProcess(program, args, env = {}) {
    const child = spawn(program, args, { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, ...env } });
    children.add(child); let stdout = '', stderr = '';
    child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8');
    child.stdout.on('data', text => { stdout = (stdout + text).slice(-524288); });
    child.stderr.on('data', text => { stderr = (stderr + text).slice(-524288); });
    const exited = new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('close', (code, signal) => { children.delete(child); resolve({ code, signal, stdout: redact(stdout), stderr: redact(stderr) }); });
    });
    // The process may fail before a caller starts awaiting readiness.
    exited.catch(() => {});
    return { child, exited, output: () => redact(stdout + '\n' + stderr) };
  }
  async function run(program, args, { env = {}, timeout = 60000, requireSuccess = true } = {}) {
    const owned = childProcess(program, args, env); let expired = false;
    const timer = setTimeout(() => { expired = true; owned.child.kill(); }, timeout);
    try {
      const result = await owned.exited;
      assert.ok(!expired, `owned test process exceeded ${timeout}ms: ${redact(program)}\n${result.stderr}`);
      if (requireSuccess) assert.equal(result.code, 0, `${redact(program)} failed\n${result.stdout}\n${result.stderr}`);
      return result;
    } finally { clearTimeout(timer); }
  }
  async function listen(handler, credentials) {
    const server = https.createServer(credentials, (request, response) => {
      Promise.resolve(handler(request, response)).catch(error => {
        if (!response.headersSent) response.writeHead(502, { 'Content-Type': 'text/plain' });
        response.end('test fixture transport failure');
        server.fixtureErrors.push(redact(error?.stack || error));
      });
    });
    server.fixtureErrors = []; servers.push(server);
    server.on('connection', socket => { sockets.add(socket); socket.once('close', () => sockets.delete(socket)); });
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
    return { server, endpoint: `https://127.0.0.1:${server.address().port}/` };
  }
  try {
    await access(jar);
    const version = await run(java, ['-version']);
    assert.match(version.stderr + version.stdout, /(?:openjdk|java) version "11\./, 'acceptance explicitly requires Java 11');
    const caStore = path.join(temp, 'ca.p12'), caPath = path.join(temp, 'ca.pem');
    const p12 = path.join(temp, 'server.p12'), csr = path.join(temp, 'server.csr'), signed = path.join(temp, 'signed.pem');
    const secretEnv = { NAVKIT_HTTP_CLIENT_STORE_PASSWORD: password };
    const pass = ['-storepass:env', 'NAVKIT_HTTP_CLIENT_STORE_PASSWORD'];
    await run(keytool, ['-genkeypair', '-alias', 'ca', '-keyalg', 'RSA', '-keysize', '2048', '-validity', '2',
      '-dname', 'CN=Navkit Temporary Client Acceptance CA', '-ext', 'bc:c', '-ext', 'KU=keyCertSign,cRLSign',
      '-storetype', 'PKCS12', '-keystore', caStore, ...pass, '-noprompt'], { env: secretEnv });
    await run(keytool, ['-exportcert', '-rfc', '-alias', 'ca', '-keystore', caStore, ...pass, '-file', caPath], { env: secretEnv });
    await run(keytool, ['-genkeypair', '-alias', 'server', '-keyalg', 'RSA', '-keysize', '2048', '-validity', '2',
      '-dname', 'CN=localhost', '-ext', 'SAN=dns:localhost,ip:127.0.0.1', '-storetype', 'PKCS12', '-keystore', p12,
      ...pass, '-noprompt'], { env: secretEnv });
    await run(keytool, ['-certreq', '-alias', 'server', '-keystore', p12, ...pass, '-file', csr], { env: secretEnv });
    await run(keytool, ['-gencert', '-rfc', '-alias', 'ca', '-keystore', caStore, ...pass, '-infile', csr, '-outfile', signed,
      '-validity', '2', '-ext', 'SAN=dns:localhost,ip:127.0.0.1', '-ext', 'KU=digitalSignature,keyEncipherment', '-ext', 'EKU=serverAuth'], { env: secretEnv });
    await run(keytool, ['-importcert', '-alias', 'ca', '-keystore', p12, ...pass, '-file', caPath, '-noprompt'], { env: secretEnv });
    await run(keytool, ['-importcert', '-alias', 'server', '-keystore', p12, ...pass, '-file', signed, '-noprompt'], { env: secretEnv });
    const cert = await readFile(caPath), credentials = { pfx: await readFile(p12), passphrase: password };
    const upstreamAgent = new https.Agent({ ca: cert, rejectUnauthorized: true, keepAlive: true, maxSockets: 32 });
    agents.push(upstreamAgent);
    const database = `jdbc:h2:file:${path.join(temp, 'data').replaceAll('\\', '/')};DB_CLOSE_DELAY=-1;MODE=REGULAR`;
    const seed = path.join(temp, 'seed.sql');
    await writeFile(seed, 'CREATE TABLE EVENTS(ID BIGINT PRIMARY KEY,TXT VARCHAR(200),BIG BIGINT,AMT DECIMAL(38,12),NIL VARCHAR(20),EMPTY VARCHAR(20),BIN VARBINARY(32),D DATE,TM TIME(9),TS TIMESTAMP(9),TZ TIMESTAMP(9) WITH TIME ZONE); CREATE TABLE WORK AS SELECT X AS N FROM SYSTEM_RANGE(1,2000); CREATE SCHEMA OTHER; CREATE TABLE OTHER.EVENTS(ID BIGINT PRIMARY KEY,TXT VARCHAR(200)); INSERT INTO OTHER.EVENTS VALUES(7,\'other-namespace\');', { mode: 0o600 });
    await run(java, ['-cp', jar, 'org.h2.tools.RunScript', '-url', database, '-user', 'sa', '-password', '', '-script', seed]);
    const serverId = randomUUID(), targetId = randomUUID();
    const configPath = path.join(temp, 'server-config.json');
    await writeFile(configPath, JSON.stringify({ server_id: serverId, bind_host: '127.0.0.1', port: 0,
      pkcs12: p12, store_password_env: 'NAVKIT_HTTP_CLIENT_STORE_PASSWORD',
      targets: [{ alias: 'main', target_id: targetId, jdbc_url: database, user: 'sa', password_env: 'NAVKIT_HTTP_CLIENT_DB_PASSWORD', namespaces: ['PUBLIC', 'OTHER'] }],
      tokens: [{ sha256: digest(token), principal: 'rust-client-fixture', aliases: ['main'], namespaces: ['PUBLIC', 'OTHER'],
        operations: ['query', 'sql_write', 'metadata', 'cancel', 'status', 'close'] }],
      limits: { status_ttl_ms: 600000, session_idle_ms: 60000, max_sessions: 64, max_executions: 256, max_timeout_ms: 10000, drain_wait_ms: 1 },
    }), { mode: 0o600 });
    const built = await buildReference({ out: path.join(temp, 'source'), jar, java });
    // No test subclass is appended: the wire peer is the production reference entry point.
    const source = await readFile(built.source, 'utf8');
    assert.ok(!source.includes('class HttpTunnelTestServer'));
    const service = childProcess(built.command, [...built.args, configPath], { ...secretEnv, NAVKIT_HTTP_CLIENT_DB_PASSWORD: '' });
    const ready = await new Promise((resolve, reject) => {
      let pending = ''; const timer = setTimeout(() => reject(new Error(`Java HTTPS readiness deadline\n${service.output()}`)), 45000);
      service.child.stdout.on('data', text => {
        pending += text;
        while (pending.includes('\n')) {
          const at = pending.indexOf('\n'), line = pending.slice(0, at).trim(); pending = pending.slice(at + 1);
          let value; try { value = JSON.parse(line); } catch { continue; }
          if (value.ready) { clearTimeout(timer); resolve(value); }
        }
      });
      service.exited.then(result => { clearTimeout(timer); reject(new Error(`Java HTTPS exited before readiness: ${result.code}\n${result.stderr}`)); }, error => { clearTimeout(timer); reject(error); });
    });
    const endpoint = ready.endpoint.replace('://localhost:', '://127.0.0.1:');
    assert.equal(ready.server_id, serverId); assert.equal(ready.protocol, protocol);
    async function request(url, { method = 'GET', body, owner } = {}) {
      const bytes = body === undefined ? null : Buffer.from(JSON.stringify(body));
      return new Promise((resolve, reject) => {
        const headers = { Authorization: `Bearer ${token}`, 'X-Navkit-Protocol': protocol,
          ...(owner ? { 'X-Navkit-Owner': owner } : {}), ...(bytes ? { 'Content-Type': 'application/json', 'Content-Length': bytes.length } : {}) };
        const req = https.request(url, { method, headers, agent: upstreamAgent, family: 4 }, res => {
          const chunks = []; let length = 0;
          res.on('data', part => { length += part.length; if (length > 20 * 1024 * 1024) req.destroy(new Error('fixture response quota')); else chunks.push(part); });
          res.once('error', reject); res.once('end', () => resolve({ status: res.statusCode, headers: res.headers, data: Buffer.concat(chunks) }));
        });
        req.setTimeout(20000, () => req.destroy(new Error('fixture independent request deadline'))); req.once('error', reject);
        req.end(bytes);
      });
    }
    const capability = await request(new URL('/v1/capabilities', endpoint));
    assert.equal(capability.status, 200); assert.equal(JSON.parse(capability.data).targets[0].product, 'H2');
    const redirected = { requests: 0, credentials: 0 };
    const sink = await listen((req, res) => {
      redirected.requests++; if (req.headers.authorization) redirected.credentials++;
      res.writeHead(200, { 'Content-Type': 'text/html' }); res.end('redirect credential capture must remain unreachable');
    }, credentials);
    const gateways = {}, stats = {};
    async function bodyOf(req) {
      const parts = []; let length = 0;
      for await (const part of req) { length += part.length; assert.ok(length <= 2 * 1024 * 1024); parts.push(part); }
      return Buffer.concat(parts);
    }
    for (const fault of faults) {
      const record = stats[fault] = { executions: 0, injected: 0, closes: 0, status: 0, requests: 0, auth: 0 };
      const gateway = await listen(async (req, res) => {
        record.requests++; if (req.headers.authorization === `Bearer ${token}`) record.auth++;
        const isExecution = req.method === 'POST' && /^\/v1\/sessions\/[^/]+\/executions$/.test(req.url);
        if (req.method === 'DELETE') record.closes++;
        if (req.method === 'GET' && /^\/v1\/executions\//.test(req.url)) record.status++;
        if ((fault === 'cap_redirect' && req.url === '/v1/capabilities') || (fault === 'stream_redirect' && isExecution)) {
          record.injected++; res.writeHead(302, { Location: new URL('/capture', sink.endpoint).href, 'Content-Type': 'text/html' }); res.end('redirect is never followed'); return;
        }
        if ((fault === 'cap_html' && req.url === '/v1/capabilities') || (fault === 'stream_html' && isExecution)) {
          record.injected++; res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }); res.end('<html>login required</html>'); return;
        }
        const payload = await bodyOf(req); if (isExecution) record.executions++;
        const headers = { ...req.headers, host: new URL(endpoint).host, connection: 'keep-alive' };
        delete headers['transfer-encoding']; if (payload.length) headers['content-length'] = String(payload.length); else delete headers['content-length'];
        const upstream = await new Promise((resolve, reject) => {
          const sent = https.request(new URL(req.url, endpoint), { method: req.method, headers, agent: upstreamAgent, family: 4 }, actual => {
            const chunks = []; let length = 0;
            actual.on('data', chunk => { length += chunk.length; if (length > 20 * 1024 * 1024) sent.destroy(new Error('gateway response quota')); else chunks.push(chunk); });
            actual.once('error', reject); actual.once('end', () => resolve({ status: actual.statusCode, headers: actual.headers, body: Buffer.concat(chunks) }));
          });
          sent.setTimeout(20000, () => sent.destroy(new Error('gateway upstream deadline'))); sent.once('error', reject); sent.end(payload);
        });
        let output = upstream.body;
        if (req.method === 'POST' && req.url === '/v1/sessions' && fault.startsWith('open_')) {
          assert.equal(upstream.status, 201); const value = JSON.parse(output);
          if (fault === 'open_foreign_binding') value.binding.owner_id = randomUUID(); else value.binding.target_generation = randomUUID();
          output = Buffer.from(JSON.stringify(value)); record.injected++;
        } else if (fault === 'control_foreign_status' && req.method === 'GET' && /^\/v1\/executions\//.test(req.url) && upstream.status === 200) {
          const value = JSON.parse(output); value.binding.target_generation = randomUUID(); output = Buffer.from(JSON.stringify(value)); record.injected++;
        } else if (isExecution && fault.startsWith('stream_')) {
          assert.equal(upstream.status, 200); assert.match(upstream.headers['content-type'], /^application\/x-ndjson/);
          const lines = output.toString('utf8').trimEnd().split('\n'), frames = lines.map(line => JSON.parse(line));
          assert.equal(frames.at(-1).kind, 'final'); assert.equal(frames.at(-1).state, 'succeeded', 'fault injection follows a REAL acknowledged H2 operation');
          if (fault === 'stream_missing_final') output = Buffer.from(lines.slice(0, -1).join('\n') + '\n');
          else if (fault === 'stream_truncated_final') output = Buffer.from(lines.slice(0, -1).join('\n') + '\n' + lines.at(-1).slice(0, -7));
          else if (fault === 'stream_final_then_bad') output = Buffer.concat([output, Buffer.from('{}\n')]);
          else { frames[0].binding[fault === 'stream_foreign_binding' ? 'owner_id' : 'target_generation'] = randomUUID(); output = Buffer.from(frames.map(frame => JSON.stringify(frame)).join('\n') + '\n'); }
          record.injected++;
        }
        const responseHeaders = { ...upstream.headers, 'content-length': String(output.length) };
        delete responseHeaders['transfer-encoding']; delete responseHeaders.connection;
        res.writeHead(upstream.status, responseHeaders); res.end(output);
      }, credentials);
      gateways[fault] = gateway.endpoint;
    }
    const manifest = path.join(temp, 'client-fixture.json');
    await writeFile(manifest, JSON.stringify({ version: 1, endpoint, ca_path: caPath, server_id: serverId, target_id: targetId, bearer: token, gateways }), { mode: 0o600 });
    await work({ manifest, request, endpoint, gateways, stats, redirected, run, servers });
    for (const server of servers) assert.deepEqual(server.fixtureErrors, [], 'fixture transport itself must not fail');
    assert.equal(redirected.requests, 0, 'Rust/fixture client did not follow a 302 to a second trusted HTTPS origin');
    assert.equal(redirected.credentials, 0, 'bearer never reached a redirect target');
  } finally {
    // All resources are test-owned. Close child processes before destroying their
    // temporary H2 files, and destroy every keepalive socket so node cannot hang.
    for (const child of [...children]) child.kill();
    for (const agent of agents) agent.destroy();
    for (const socket of sockets) socket.destroy();
    await Promise.all(servers.map(server => new Promise(resolve => server.close(() => resolve()))));
    await Promise.all([...children].map(child => new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('owned Java/Rust process did not terminate during fixture cleanup')), 10000);
      child.once('close', () => { clearTimeout(timer); resolve(); });
    })));
    assert.ok(path.basename(temp).startsWith('navkit-http-tunnel-client-'));
    await rm(temp, { recursive: true, force: true });
  }
}

const skip = !direct && !executable ? 'Real Rust HTTPS/H2 client NOT VERIFIED: use NAVKIT_HTTP_TUNNEL_TEST_EXE with the dedicated runner' : false;
test(selfCheck ? 'temporary HTTPS Java/H2 client fixture self-check (Rust NOT executed)' : 'fresh Rust client crosses temporary trusted HTTPS Java 11/H2 and adversarial wire responses',
  { timeout: 300000, skip }, async () => {
    if (!selfCheck) {
      assert.ok(executable, 'NAVKIT_HTTP_TUNNEL_TEST_EXE must explicitly name a fresh Rust test executable; direct acceptance never silently skips');
      assert.ok(path.isAbsolute(executable)); assert.ok((await stat(executable)).isFile());
    }
    await fixtureLifecycle(async ({ manifest, request, endpoint, gateways, stats, redirected, run }) => {
      if (selfCheck) {
        const cap = await request(new URL('/v1/capabilities', endpoint));
        assert.equal(cap.status, 200); const capabilities = JSON.parse(cap.data);
        assert.equal(capabilities.protocol, protocol);
        assert.equal(capabilities.targets[0].product_version, '2.3.232 (2024-08-11)');
        const directOpen = async target => {
          const opened = await request(new URL('/v1/sessions', target), { method: 'POST', body: {
            protocol, request_id: randomUUID(), owner_id: randomUUID(), target_alias: 'main',
            expected_server_id: capabilities.server_id, expected_target_id: capabilities.targets[0].target_id,
            namespace: 'PUBLIC', read_only: false,
          } });
          assert.equal(opened.status, 201); return JSON.parse(opened.data);
        };
        for (const fault of ['cap_redirect', 'cap_html']) {
          const response = await request(new URL('/v1/capabilities', gateways[fault]));
          assert.equal(response.status, fault === 'cap_redirect' ? 302 : 200);
          assert.match(response.headers['content-type'], /^text\/html/);
        }
        for (const fault of ['open_foreign_binding', 'open_target_generation']) await directOpen(gateways[fault]);
        for (const fault of faults.filter(name => name.startsWith('stream_') || name === 'control_foreign_status')) {
          const opened = await directOpen(gateways[fault]), binding = opened.binding;
          const bytes = createHash('sha256').update(`${protocol}\0${binding.session_id}\0${binding.session_generation}\0${1}`).digest().subarray(0, 16);
          bytes[6] = (bytes[6] & 15) | 128; bytes[8] = (bytes[8] & 63) | 128;
          const hex = bytes.toString('hex'), id = `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
          const body = { protocol, request_id: randomUUID(), binding, execution_id: id, sequence: '1', sql: 'SELECT 1', binds: [], timeout_ms: 10000 };
          const response = await request(new URL(`/v1/sessions/${binding.session_id}/executions`, gateways[fault]), { method: 'POST', body });
          if (fault === 'stream_redirect') assert.equal(response.status, 302);
          else if (fault === 'stream_html') assert.match(response.headers['content-type'], /^text\/html/);
          else {
            assert.equal(response.status, 200); assert.match(response.headers['content-type'], /^application\/x-ndjson/);
            const lines = response.data.toString('utf8').trimEnd().split('\n');
            if (fault === 'stream_missing_final') assert.ok(lines.every(line => JSON.parse(line).kind !== 'final'));
            else if (fault === 'stream_truncated_final') assert.throws(() => JSON.parse(lines.at(-1)));
            else if (fault === 'stream_final_then_bad') { assert.equal(JSON.parse(lines.at(-2)).kind, 'final'); assert.deepEqual(JSON.parse(lines.at(-1)), {}); }
            else if (fault === 'stream_foreign_binding') assert.notEqual(JSON.parse(lines[0]).binding.owner_id, binding.owner_id);
            else if (fault === 'stream_target_generation') assert.notEqual(JSON.parse(lines[0]).binding.target_generation, binding.target_generation);
            else {
              const status = await request(new URL(`/v1/executions/${id}`, gateways[fault]), { owner: binding.owner_id });
              assert.equal(status.status, 200); assert.notEqual(JSON.parse(status.data).binding.target_generation, binding.target_generation);
            }
          }
          const closed = await request(new URL(`/v1/sessions/${binding.session_id}`, gateways[fault]), { method: 'DELETE', body: { protocol, request_id: randomUUID(), binding } });
          assert.equal(JSON.parse(closed.data).state, 'closed');
        }
        for (const [fault, record] of Object.entries(stats)) assert.equal(record.injected, 1, `fixture-only ${fault} really injected once`);
        // Fixture-only verification is labelled explicitly and cannot satisfy the
        // Rust acceptance path, whose result and evidence are checked below.
        console.log('HTTP_CLIENT_FIXTURE_ONLY trusted_ca java11 h2 production_source_launcher 12_fault_gateways; Rust client NOT executed');
        return;
      }
      const result = await run(executable, [RUST_EXACT_TEST, '--exact', '--ignored', '--nocapture', '--test-threads=1'],
        { env: { NAVKIT_HTTP_TUNNEL_CLIENT_FIXTURE: manifest, RUST_BACKTRACE: '1' }, timeout: 180000 });
      const output = result.stdout + '\n' + result.stderr;
      assert.match(output, /running 1 test\b/);
      assert.match(output, /test result: ok\. 1 passed; 0 failed; 0 ignored;/, 'the exact fresh test really ran: no zero-test or ignored-test false pass');
      for (const evidence of ['typed_prepared_dml_and_select', 'independent_status_cancel', 'c1_real_grant_owned_future_cancel_activity_and_lease',
        ...faults, 'complete']) assert.ok(output.includes(`HTTP_CLIENT_EVIDENCE ${evidence}`), `missing Rust runtime evidence: ${evidence}`);
      for (const [fault, record] of Object.entries(stats)) {
        assert.equal(record.injected, 1, `${fault}: intended response fault was actually exercised once`);
        assert.equal(record.auth, record.requests, `${fault}: same-origin request authentication was present`);
        assert.equal(record.executions, fault.startsWith('stream_') && !['stream_redirect', 'stream_html'].includes(fault) || fault === 'control_foreign_status' ? 1 : 0,
          `${fault}: no hidden execution retry`);
        if (fault.startsWith('stream_')) assert.ok(record.closes >= 1, `${fault}: a real independent drain/close was acknowledged`);
      }
      assert.equal(redirected.requests, 0); assert.equal(redirected.credentials, 0);
      const production = await run(executable, [
        'http_tunnel::production_tests::production_profile_to_c1_uses_remote_resource',
        '--exact', '--ignored', '--nocapture', '--test-threads=1',
      ], { env: { NAVKIT_HTTP_TUNNEL_CLIENT_FIXTURE: manifest, RUST_BACKTRACE: '1' }, timeout: 180000 });
      const productionOutput = production.stdout + '\n' + production.stderr;
      assert.match(productionOutput, /running 1 test\b/);
      assert.match(productionOutput, /test result: ok\. 1 passed; 0 failed; 0 ignored;/,
        'the production encrypted-profile/ensure/grant/namespace/retire path must actually run');
      console.log('HTTP_PRODUCTION_EVIDENCE encrypted_atomic_profile independent_store_root PUBLIC_derived_resource real_c1_once readonly_remote retire_reopen_durable');
      console.log(output.trim());
      console.log('HTTP_CLIENT_ACCEPTANCE Rust exact 1 passed 0 failed 0 ignored; redirect credentials 0; listeners and children owned by fixture');
    });
  });
