"""验收测试夹具生成器。

产出:
  testdata/accept.db                 SQLite 验收库（无主键/复合主键/单主键/空表/10万行/外键/触发器）
  testdata/import_utf8.csv           UTF-8 CSV（引号/逗号/换行/NULL/中文/BOM）
  testdata/import_gbk.csv            GBK CSV（同结构）
  testdata/import_rows.json          JSON 导入夹具
  testdata/import_stmts.sql          SQL 导入夹具
  testdata/big_export.csv            10 万行 CSV（性能/流式）
  testdata/mysql_fixture.sql         MySQL navkit_accept 全量夹具脚本（utf8mb4）

用法:  python tools/acceptance/make_fixtures.py
幂等:  重复运行会先删除重建 accept.db 与 CSV；MySQL 脚本自带 DROP DATABASE。
"""
import json
import os
import random
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TD = os.path.join(ROOT, "testdata")
os.makedirs(TD, exist_ok=True)

# ---------------------------------------------------------------- SQLite 基线
DB = os.path.join(TD, "accept.db")
if os.path.exists(DB):
    os.remove(DB)
con = sqlite3.connect(DB)
cur = con.cursor()
cur.executescript(
    """
CREATE TABLE t_single_pk (id INTEGER PRIMARY KEY, name TEXT, amount REAL, note TEXT);
CREATE TABLE t_composite_pk (a INTEGER, b TEXT, val TEXT, PRIMARY KEY (a, b));
CREATE TABLE t_no_pk (name TEXT, qty INTEGER, created_at TEXT);
CREATE TABLE t_empty (id INTEGER PRIMARY KEY, payload TEXT);
CREATE TABLE t_fk_parent (pid INTEGER PRIMARY KEY, pname TEXT);
CREATE TABLE t_fk_child (cid INTEGER PRIMARY KEY, pid INTEGER REFERENCES t_fk_parent(pid) ON DELETE CASCADE, memo TEXT);
CREATE TABLE t_big (
  id INTEGER PRIMARY KEY,
  name TEXT,
  created_at TEXT,
  amount REAL
);
CREATE TABLE t_parent_for_trig (id INTEGER PRIMARY KEY, n INTEGER, log TEXT);
CREATE TABLE t_trig_log (seq INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT);
CREATE TRIGGER trg_after_insert AFTER INSERT ON t_parent_for_trig
BEGIN
  INSERT INTO t_trig_log (at) VALUES (datetime('now'));
END;
CREATE INDEX idx_big_name ON t_big(name);
CREATE INDEX idx_child_pid ON t_fk_child(pid);
CREATE VIEW v_single AS SELECT id, name, amount FROM t_single_pk WHERE amount > 0;
"""
)
cur.executemany("INSERT INTO t_single_pk VALUES (?,?,?,?)",
                [(i, f"row-{i}", round(random.uniform(-100, 1000), 4), None if i % 7 == 0 else f"备注{i}")
                 for i in range(1, 201)])
cur.executemany("INSERT INTO t_composite_pk VALUES (?,?,?)",
                [(i % 10, f"k{i}", f"v{i}") for i in range(1, 61)])
cur.executemany("INSERT INTO t_no_pk VALUES (?,?,datetime('now'))",
                [(f"np{i}", i * 3) for i in range(1, 51)])
cur.executemany("INSERT INTO t_fk_parent VALUES (?,?)", [(i, f"p{i}") for i in range(1, 21)])
cur.executemany("INSERT INTO t_fk_child VALUES (?,?,?)",
                [(i, (i % 20) + 1, f"c{i}") for i in range(1, 81)])
cur.executemany("INSERT INTO t_big VALUES (?,?,?,?)",
                [(i, f"name_{i % 997:04d}", f"2026-01-{(i % 28) + 1:02d} 12:00:00",
                  round(random.uniform(0, 9999), 2)) for i in range(1, 100_001)])
con.commit()
con.close()
print(f"OK  {DB}  ({os.path.getsize(DB)//1024//1024} MB, t_big=100000 行)")

# ---------------------------------------------------------------- 导入导出夹具
UTF8_ROWS = [
    ["id", "名称", "备注", "数量"],
    ["1", "中文值,含逗号", "普通行", "10"],
    ["2", '引号"内嵌"值', "带引号", "20"],
    ["3", "换行\n在第2行", "含换行", "30"],
    ["4", "NULL", "字面 NULL", ""],
    ["5", "空串", "", "50"],
    ["6", "emoji😀😀", "表情", "60"],
    ["7", "Tab\t分隔", "含制表符", "70"],
]
p_utf8 = os.path.join(TD, "import_utf8.csv")
with open(p_utf8, "w", encoding="utf-8-sig", newline="") as f:  # 带 BOM
    import csv as _csv
    _csv.writer(f).writerows(UTF8_ROWS)
print(f"OK  {p_utf8}  (UTF-8 BOM)")

p_gbk = os.path.join(TD, "import_gbk.csv")
GBK_ROWS = [[c.replace("emoji😀😀", "emoji★★") for c in row] for row in UTF8_ROWS]
with open(p_gbk, "w", encoding="gbk", newline="") as f:
    import csv as _csv
    _csv.writer(f).writerows(GBK_ROWS)
print(f"OK  {p_gbk}  (GBK)")

p_json = os.path.join(TD, "import_rows.json")
with open(p_json, "w", encoding="utf-8") as f:
    json.dump([{"id": i, "名称": f"j{i}", "备注": None if i % 3 == 0 else f"备注{i}", "数量": i * 11}
               for i in range(1, 31)], f, ensure_ascii=False, indent=1)
print(f"OK  {p_json}")

p_sql = os.path.join(TD, "import_stmts.sql")
with open(p_sql, "w", encoding="utf-8") as f:
    f.write("CREATE TABLE IF NOT EXISTS imported_sql (id INTEGER PRIMARY KEY, tag TEXT, val TEXT);\n")
    for i in range(1, 26):
        f.write(f"INSERT INTO imported_sql VALUES ({i}, 'tag{i}', '值{i}');\n")
print(f"OK  {p_sql}")

p_big = os.path.join(TD, "big_export.csv")
with open(p_big, "w", encoding="utf-8", newline="") as f:
    f.write("id,name,created_at,amount\n")
    for i in range(1, 100_001):
        f.write(f"{i},name_{i % 997:04d},2026-01-{(i % 28) + 1:02d} 12:00:00,{i * 0.5:.2f}\n")
print(f"OK  {p_big}  ({os.path.getsize(p_big)//1024//1024} MB, 100000 行)")

# ---------------------------------------------------------------- MySQL 夹具脚本
MYSQL = r"""DROP DATABASE IF EXISTS navkit_accept;
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
"""
p_mysql = os.path.join(TD, "mysql_fixture.sql")
with open(p_mysql, "w", encoding="utf-8") as f:
    f.write(MYSQL)
print(f"OK  {p_mysql}")
print("下一步: mysql -u root -p123456 < testdata/mysql_fixture.sql")
