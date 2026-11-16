// 用户编辑（通用数据库客户端 式多标签）前端契约与纯逻辑测试。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const read = (rel) => fs.readFileSync(path.join(root, rel), 'utf8');

function loadModule(rel) {
  const source = read(rel);
  const js = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  const module = { exports: {} };
  const context = vm.createContext({ module, exports: module.exports, console, Set, Map, Number, String, Object, JSON, Array });
  vm.runInContext(js, context);
  return module.exports;
}

const model = loadModule('src/core/db-user.ts');

test('privilegeKey 统一空白和下划线', () => {
  assert.equal(model.privilegeKey('Alter Routine'), 'ALTER_ROUTINE');
  assert.equal(model.privilegeKey('ALTER_ROUTINE'), 'ALTER_ROUTINE');
  assert.equal(model.privilegeKey('Show  Databases'), 'SHOW_DATABASES');
});

test('expiryFromAccount 映射默认/永不过期/按天', () => {
  const base = { user: 'u', host: '%', plugin: '', password_expired: false, account_locked: false,
    max_questions: 0, max_updates: 0, max_connections: 0, max_user_connections: 0,
    ssl_type: '', ssl_cipher: '', x509_issuer: '', x509_subject: '' };
  assert.equal(model.expiryFromAccount({ ...base, password_lifetime_days: null }), 'default');
  assert.equal(model.expiryFromAccount({ ...base, password_lifetime_days: 0 }), 'never');
  assert.equal(model.expiryFromAccount({ ...base, password_lifetime_days: 30 }), 'interval');
});

test('draftFromDetail 只保留已授予权限并复制对象授权', () => {
  const detail = {
    account: { user: 'app', host: '%', plugin: 'caching_sha2_password', password_expired: false,
      password_lifetime_days: 30, account_locked: true, max_questions: 10, max_updates: 0,
      max_connections: 0, max_user_connections: 2, ssl_type: 'ANY', ssl_cipher: 'c', x509_issuer: '', x509_subject: '' },
    server_privileges: [
      { name: 'Select', granted: true, grantable: false },
      { name: 'Insert', granted: false, grantable: false },
      { name: 'Grant Option', granted: false, grantable: false },
    ],
    object_privileges: [{ database: 'shop', name: '*', privileges: ['Select'], grantable: false }],
    roles: [{ role: 'readonly', host: '%' }],
    members: [],
    supports_roles: true,
    warnings: [],
  };
  const draft = model.draftFromDetail(detail);
  assert.equal(JSON.stringify(draft.server_privileges), JSON.stringify(['Select']));
  assert.equal(draft.password_expiry, 'interval');
  assert.equal(draft.password_lifetime_days, 30);
  assert.equal(draft.account_locked, true);
  assert.equal(draft.object_grants.length, 1);
  assert.equal(JSON.stringify(draft.object_grants[0].privileges), JSON.stringify(['Select']));
  assert.equal(JSON.stringify(draft.roles), JSON.stringify([{ role: 'readonly', host: '%' }]));
  assert.equal(JSON.stringify(model.OBJECT_PRIVILEGE_COLUMNS.slice(0, 4)), JSON.stringify(['Select', 'Insert', 'Update', 'Delete']));
});

test('UserDialog 提供七个编辑标签与权限管理', () => {
  const source = read('src/dialogs/UserDialog.ts');
  for (const label of ['常规', '高级', '成员属于', '成员', '服务器权限', '权限', 'SQL 预览']) {
    assert.ok(source.includes('"' + label + '"'), 'missing tab label ' + label);
  }
  assert.ok(source.includes('api.readDbUser'), 'must read full user detail');
  assert.ok(source.includes('api.previewDbUserChanges'), 'must preview SQL');
  assert.ok(source.includes('api.applyDbUserChanges'), 'must apply changes');
  assert.ok(source.includes('openEditUserDialog'), 'must export editor');
  assert.ok(source.includes('OBJECT_PRIVILEGE_COLUMNS'), 'must render privilege matrix');
});

test('api 暴露用户编辑命令且 App 接入编辑用户', () => {
  const api = read('src/core/api.ts');
  assert.ok(api.includes('"read_db_user"'));
  assert.ok(api.includes('"preview_db_user_changes"'));
  assert.ok(api.includes('"apply_db_user_changes"'));
  const app = read('src/core/App.ts');
  assert.ok(app.includes('openEditUserDialog'));
  assert.ok(app.includes('label: "编辑用户"'));
  assert.ok(app.includes('cmdEditUser()'));
});

test('Rust 后端生成改名/授权/SSL/过期语句', () => {
  const rust = read('src-tauri/src/db_users.rs');
  for (const needle of ['RENAME USER', 'GRANT ', 'REVOKE ', 'PASSWORD EXPIRE NEVER', 'REQUIRE SSL', 'ACCOUNT LOCK', 'information_schema.USER_PRIVILEGES', 'information_schema.SCHEMA_PRIVILEGES']) {
    assert.ok(rust.includes(needle), 'missing ' + needle);
  }
});

test('权限面板用真实数据库/表下拉，不再手填库表名', () => {
  const source = read('src/dialogs/UserDialog.ts');
  assert.ok(source.includes('api.listDatabases'), '必须从连接读取真实数据库');
  assert.ok(source.includes('api.listTables'), '必须按数据库读取真实表');
  assert.ok(source.includes('grantDatabaseSelect'), '数据库应使用下拉');
  assert.ok(source.includes('grantTableSelect'), '表应使用下拉');
  assert.ok(!source.includes('grantDatabaseInput'), '数据库不应保留手填输入框');
  assert.ok(!source.includes('grantTableInput'), '表不应保留手填输入框');
  assert.ok(source.includes('loadGrantTables'), '切换数据库应刷新表清单');
});
