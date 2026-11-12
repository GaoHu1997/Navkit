-- NavKit 验收 PostgreSQL 夹具（navkit_accept@5433）
DROP DATABASE IF EXISTS navkit_accept;
CREATE DATABASE navkit_accept;
\c navkit_accept

CREATE TABLE t_pk_auto (
  id SERIAL PRIMARY KEY,
  name VARCHAR(255),
  amount NUMERIC(18,4),
  created_at TIMESTAMP
);
COMMENT ON TABLE t_pk_auto IS '自增主键表';

CREATE TABLE t_composite (a INT, b VARCHAR(50), val VARCHAR(255), PRIMARY KEY (a, b));
CREATE TABLE t_no_pk (name VARCHAR(100), qty INT, created_at TIMESTAMP);
CREATE TABLE t_empty (id INT PRIMARY KEY, payload TEXT);

CREATE TABLE t_types (
  id SERIAL PRIMARY KEY,
  big_col BIGINT,
  dec_col NUMERIC(18,4),
  vc_col VARCHAR(255),
  text_col TEXT,
  date_col DATE,
  ts_col TIMESTAMP(3),
  time_col TIME,
  bool_col BOOLEAN,
  json_col JSONB,
  bytea_col BYTEA,
  nullable_col VARCHAR(50) NULL,
  e_col VARCHAR(10) NULL,
  plain_varchar VARCHAR(255)
);

CREATE TABLE t_fk_parent (pid INT PRIMARY KEY, pname VARCHAR(50));
CREATE TABLE t_fk_child (
  cid INT PRIMARY KEY,
  pid INT REFERENCES t_fk_parent(pid) ON DELETE CASCADE,
  memo VARCHAR(100),
  CONSTRAINT chk_cid CHECK (cid >= 0)
);
COMMENT ON COLUMN t_fk_child.memo IS '备注列注释';

CREATE TABLE t_comment (id INT PRIMARY KEY, note VARCHAR(100));
COMMENT ON COLUMN t_comment.id IS '主键列注释';
COMMENT ON COLUMN t_comment.note IS '备注列注释';

CREATE TABLE t_big (id INT PRIMARY KEY, name VARCHAR(64), created_at TIMESTAMP, amount NUMERIC(18,4));

CREATE INDEX idx_types_vc ON t_types(vc_col);
CREATE UNIQUE INDEX uniq_types_e ON t_types(e_col);
CREATE INDEX idx_comp ON t_composite(a, b);
CREATE INDEX idx_big_name ON t_big(name);

CREATE VIEW v_types AS SELECT id, vc_col, dec_col FROM t_types WHERE id <= 100;
CREATE VIEW v_empty AS SELECT * FROM t_empty;

CREATE OR REPLACE FUNCTION fn_add10(x INT) RETURNS INT LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN RETURN x + 10; END $$;

CREATE OR REPLACE PROCEDURE sp_reset() LANGUAGE plpgsql AS $$
BEGIN TRUNCATE t_empty; END $$;

-- 特殊字符行
INSERT INTO t_types (big_col, dec_col, vc_col, text_col, date_col, ts_col, time_col, bool_col, json_col, bytea_col, nullable_col, e_col, plain_varchar) VALUES
 (9223372036854775807, 12345678901234.5678, '中文 English 😀 ''单引号'' "双引号" \反斜杠 制表', repeat('超长TEXT:', 720) || 'END', '2026-02-28', '2026-02-28 23:59:59.123', '13:45:06', true, '{"k": [1, 2, {"嵌套": "值"}], "unicode": "😀"}', '\x00FF10', NULL, 'A1', 'plain1'),
 (-9223372036854775808, -0.0001, repeat('长', 127) || 'E', repeat('超长TEXT:', 720) || 'END', '1900-01-01', '2000-01-01 00:00:00.000', '00:00:00', false, 'null', NULL, '', 'B2', 'plain2'),
 (0, 0.0000, '', '空串行', NULL, NULL, NULL, NULL, '[]', NULL, NULL, NULL, 'plain3');

INSERT INTO t_pk_auto (name, amount, created_at) VALUES
 ('行1', 10.5, '2026-01-01 10:00:00'),
 ('行2,含逗号', NULL, NULL),
 ('行3 "引号"', 0.0001, '2026-12-31 23:59:59');
INSERT INTO t_pk_auto (name) VALUES ('中文😀'), (NULL);

INSERT INTO t_composite VALUES (1,'a','v1'),(1,'b','v2'),(2,'a','v3');
INSERT INTO t_no_pk VALUES ('np1', 1, now()), ('np2', 2, now());
INSERT INTO t_fk_parent VALUES (1,'p1'),(2,'p2'),(3,'p3');
INSERT INTO t_fk_child VALUES (10,1,'c10'),(20,2,'c20'),(30,3,'c30'),(40,1,'c40');
INSERT INTO t_comment VALUES (1,'备注1'),(2,NULL);

INSERT INTO t_big (id, name, created_at, amount)
SELECT i,
       'name_' || lpad((i % 997)::text, 4, '0'),
       TIMESTAMP '2026-01-01 12:00:00' + (i % 365) * INTERVAL '1 day',
       round((i * 1.37)::numeric, 2)
FROM generate_series(1, 100000) AS i;
