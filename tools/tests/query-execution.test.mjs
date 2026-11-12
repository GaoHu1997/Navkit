import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';

// 与其余 core 测试一致：esbuild 现场打包，再以 data URL 导入，不新增依赖。
const bundle = await build({
  entryPoints: ['src/core/queryExecution.ts'],
  bundle: true,
  platform: 'node',
  format: 'esm',
  write: false,
  logLevel: 'silent',
});
const { explainQuerySql } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`
);

const SINGLE = /单条|一条/;

test('a single statement gets exactly one EXPLAIN prefix and keeps its text', () => {
  assert.equal(explainQuerySql('SELECT 1'), 'EXPLAIN SELECT 1');
  assert.equal(explainQuerySql('  select  1 ;  '), 'EXPLAIN select  1');
  assert.equal(explainQuerySql('SELECT * FROM t_big'), 'EXPLAIN SELECT * FROM t_big');
  assert.equal(explainQuerySql('SELECT\n  1'), 'EXPLAIN SELECT\n  1');
});

test('multi-statement SQL is refused instead of prefixing only the first', () => {
  for (const sql of [
    'SELECT 1; DELETE FROM t_big',
    'SELECT 1;\nUPDATE t SET a = 1',
    'DELETE FROM t; SELECT 1',
    'SELECT 1; EXPLAIN DELETE FROM t',
  ]) {
    assert.throws(() => explainQuerySql(sql), SINGLE, sql);
  }
});

test('trailing empty statements and comments are ignored', () => {
  assert.equal(explainQuerySql('SELECT 1;;'), 'EXPLAIN SELECT 1');
  assert.equal(explainQuerySql('SELECT 1; -- 注释'), 'EXPLAIN SELECT 1');
  assert.equal(explainQuerySql('/* c */ SELECT 1'), 'EXPLAIN /* c */ SELECT 1');
  assert.equal(explainQuerySql('SELECT 1 # note'), 'EXPLAIN SELECT 1 # note');
});

test('empty or comment-only input is refused', () => {
  for (const sql of ['', '   ', ';', ';;', '-- nothing here', '/* nothing */', '/* a */ -- b\n', '# only']) {
    assert.throws(() => explainQuerySql(sql), /没有可解释|一条/, JSON.stringify(sql));
  }
});

test('semicolons inside the three quote styles do not split statements', () => {
  assert.equal(explainQuerySql("SELECT ';' AS x"), "EXPLAIN SELECT ';' AS x");
  assert.equal(explainQuerySql("SELECT 'a''b; c'"), "EXPLAIN SELECT 'a''b; c'");
  assert.equal(explainQuerySql('SELECT "a;b" FROM t'), 'EXPLAIN SELECT "a;b" FROM t');
  assert.equal(explainQuerySql('SELECT `a;b` FROM t'), 'EXPLAIN SELECT `a;b` FROM t');
});

test('semicolons inside comments do not split statements', () => {
  assert.equal(explainQuerySql('SELECT 1 -- ; DROP TABLE t\n'), 'EXPLAIN SELECT 1 -- ; DROP TABLE t');
  assert.equal(explainQuerySql('SELECT /* ; */ 1'), 'EXPLAIN SELECT /* ; */ 1');
});

test('backslash is not a string escape, matching the backend splitter', () => {
  // 后端 split_statements 不处理反斜杠，\' 会提前闭合字符串；前端必须同样保守拒绝。
  assert.throws(() => explainQuerySql("SELECT 'it\\'s; ok'"), /未闭合|单条|一条/);
});

test('Postgres dollar quotes are accepted only when the backend splitter agrees', () => {
  assert.equal(explainQuerySql('SELECT $$abc$$'), 'EXPLAIN SELECT $$abc$$');
  assert.equal(explainQuerySql('SELECT $tag$a$tag$'), 'EXPLAIN SELECT $tag$a$tag$');
  assert.throws(() => explainQuerySql('SELECT $$a;b$$'), /美元引号/);
  assert.throws(() => explainQuerySql('SELECT $tag$a;b$tag$'), /美元引号/);
});

test('SQL Server brackets are accepted only when the backend splitter agrees', () => {
  assert.equal(explainQuerySql('SELECT [a] FROM t'), 'EXPLAIN SELECT [a] FROM t');
  assert.throws(() => explainQuerySql('SELECT [a;b] FROM t'), /方括号/);
});

test('MySQL # comments are refused when they hide a semicolon', () => {
  assert.throws(() => explainQuerySql('SELECT 1 # a; b'), /# 注释/);
  assert.equal(explainQuerySql('SELECT 1 # a b'), 'EXPLAIN SELECT 1 # a b');
});

test('MySQL executable comments are always refused', () => {
  assert.throws(() => explainQuerySql('SELECT 1 /*! DELETE FROM t_big */'), /可执行注释/);
  assert.throws(() => explainQuerySql('/*!40101 SET @a = 1 */; SELECT 1'), /可执行注释/);
});

test('unterminated quoting is refused instead of guessing', () => {
  for (const sql of ["SELECT 'abc", 'SELECT "abc', 'SELECT `abc', 'SELECT [abc', 'SELECT /* abc', 'SELECT $$abc']) {
    assert.throws(() => explainQuerySql(sql), /未闭合/, sql);
  }
});

test('input that already carries EXPLAIN is refused, including EXPLAIN ANALYZE writes', () => {
  assert.throws(() => explainQuerySql('EXPLAIN SELECT 1'), /EXPLAIN/);
  assert.throws(() => explainQuerySql('explain analyze delete from t_big'), /EXPLAIN/);
  assert.throws(() => explainQuerySql('/* c */ EXPLAIN ANALYZE UPDATE t SET a = 1'), /EXPLAIN/);
});

test('a write statement can never survive the explain rewrite', () => {
  const attempted = ['SELECT 1; DELETE FROM t_big', 'SELECT 1;\nDROP TABLE t_big', 'SELECT 1; INSERT INTO t VALUES (1)'];
  for (const sql of attempted) {
    assert.throws(() => explainQuerySql(sql), error => {
      assert.ok(!String(error.message).includes('EXPLAIN SELECT 1;'), 'must not return a prefix-only rewrite');
      return SINGLE.test(error.message) || /未闭合/.test(error.message);
    }, sql);
  }
});
