DROP DATABASE IF EXISTS navkit_accept;
CREATE DATABASE navkit_accept CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;
USE navkit_accept;

CREATE TABLE t_pk_auto (
  id INT AUTO_INCREMENT PRIMARY KEY,
  name VARCHAR(255),
  amount DECIMAL(18,4),
  created_at DATETIME
) COMMENT='自增主键表';

CREATE TABLE t_composite (
  a INT, b VARCHAR(50), val VARCHAR(255),
  PRIMARY KEY (a, b)
) COMMENT='复合主键表';

CREATE TABLE t_no_pk (name VARCHAR(100), qty INT, created_at DATETIME) COMMENT='无主键表';
CREATE TABLE t_empty (id INT PRIMARY KEY, payload TEXT) COMMENT='空表';

CREATE TABLE t_types (
  id INT AUTO_INCREMENT PRIMARY KEY,
  big_col BIGINT,
  dec_col DECIMAL(18,4),
  vc_col VARCHAR(255),
  text_col TEXT,
  date_col DATE,
  dt_col DATETIME(3),
  ts_col TIMESTAMP NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
  time_col TIME,
  bool_col TINYINT(1),
  json_col JSON,
  blob_col BLOB,
  nullable_col VARCHAR(50) NULL,
  e_col VARCHAR(10) NULL,
  plain_varchar VARCHAR(255)
) COMMENT='全类型表';

CREATE TABLE t_fk_parent (pid INT PRIMARY KEY, pname VARCHAR(50)) COMMENT='父表';
CREATE TABLE t_fk_child (
  cid INT PRIMARY KEY,
  pid INT,
  memo VARCHAR(100),
  CONSTRAINT chk_qty CHECK (cid >= 0),
  CONSTRAINT fk_child_parent FOREIGN KEY (pid) REFERENCES t_fk_parent(pid) ON DELETE CASCADE
) COMMENT='外键+检查约束表';

CREATE TABLE t_comment (
  id INT PRIMARY KEY COMMENT '主键列注释',
  note VARCHAR(100) COMMENT '备注列注释'
) COMMENT='带注释的表';

CREATE TABLE t_big (id INT PRIMARY KEY, name VARCHAR(64), created_at DATETIME, amount DECIMAL(18,4));

CREATE INDEX idx_types_vc ON t_types(vc_col);
CREATE UNIQUE INDEX uniq_types_e ON t_types(e_col);
CREATE INDEX idx_comp ON t_composite(a, b);
CREATE INDEX idx_big_name ON t_big(name);

CREATE VIEW v_types AS SELECT id, vc_col, dec_col FROM t_types WHERE id <= 100;
CREATE VIEW v_empty AS SELECT * FROM t_empty;

DELIMITER $$
CREATE PROCEDURE sp_count_rows(IN tbl VARCHAR(64), OUT cnt INT)
BEGIN
  SET @s = CONCAT('SELECT COUNT(*) INTO cnt FROM ', tbl);
  PREPARE st FROM @s; EXECUTE st; DEALLOCATE PREPARE st;
END$$
CREATE FUNCTION fn_add10(x INT) RETURNS INT DETERMINISTIC
BEGIN
  RETURN x + 10;
END$$
CREATE TRIGGER trg_types_ai BEFORE INSERT ON t_types
FOR EACH ROW SET NEW.plain_varchar = CONCAT('auto-', IFNULL(NEW.vc_col, 'null'))$$
DELIMITER ;

-- 特殊字符行（中文/emoji/单双引号/反斜杠/换行/制表符/超长 5000 字符）
INSERT INTO t_types (big_col, dec_col, vc_col, text_col, date_col, dt_col, time_col, bool_col, json_col, blob_col, nullable_col, e_col, plain_varchar) VALUES
 (9223372036854775807, 12345678901234.5678, '中文 English 😀 ''单引号'' "双引号" \反斜杠\t制表', '超长文本前缀—123456789012345678901234567890123456789012345678901234567890', '2026-02-28', '2026-02-28 23:59:59.123', '13:45:06', 1, '{"k": [1, 2, {"嵌套": "值"}], "unicode": "😀"}', X'00FF10', NULL, 'A1', 'plain1'),
 (-9223372036854775808, -0.0001, CONCAT(REPEAT('长', 127), 'E'), CONCAT(REPEAT('超长TEXT:', 720), 'END'), '1900-01-01', '2000-01-01 00:00:00.000', '00:00:00', 0, 'null', NULL, '', 'B2', 'plain2'),
 (0, 0.0000, '', '空串行', NULL, NULL, NULL, NULL, JSON_ARRAY(), _binary'_binary', NULL, NULL, 'plain3');

INSERT INTO t_pk_auto (name, amount, created_at) VALUES
 ('行1', 10.5, '2026-01-01 10:00:00'),
 ('行2,含逗号', NULL, NULL),
 ('行3 "引号"', 0.0001, '2026-12-31 23:59:59');
INSERT INTO t_pk_auto (name) VALUES ('中文😀'), (NULL);

INSERT INTO t_composite VALUES (1,'a','v1'),(1,'b','v2'),(2,'a','v3');
INSERT INTO t_no_pk VALUES ('np1', 1, NOW()), ('np2', 2, NOW());
INSERT INTO t_fk_parent VALUES (1,'p1'),(2,'p2'),(3,'p3');
INSERT INTO t_fk_child VALUES (10,1,'c10'),(20,2,'c20'),(30,3,'c30'),(40,1,'c40');
INSERT INTO t_comment VALUES (1,'备注1'),(2,NULL);

SET SESSION cte_max_recursion_depth = 200000;
-- big_table：100000 行
INSERT INTO t_big (id, name, created_at, amount)
WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM seq WHERE i < 100000)
SELECT i,
       CONCAT('name_', LPAD(i % 997, 4, '0')),
       TIMESTAMP(DATE '2026-01-01' + INTERVAL (i % 365) DAY, TIME '12:00:00'),
       ROUND(i * 1.37, 2)
FROM seq;
