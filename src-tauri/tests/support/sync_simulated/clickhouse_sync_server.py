"""TEST ONLY: ClickHouse HTTP SQL subset with actual mutable SQLite storage.

NOT a ClickHouse server. No vendor engine/partition/async-mutation/transaction
certification. JSONCompactEachRowWithNamesAndTypes is encoded from actual query
rows. DDL metadata is derived from accepted CREATE definitions, not fixed rows.
String is non-nullable unless Nullable(String) is declared. Standard UPDATE is
rejected (ClickHouse requires ALTER TABLE ... UPDATE or an engine-specific path).
"""
import base64
import json
import os
import re
import sqlite3
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

BASE = Path(sys.argv[1]).resolve()
if not BASE.is_dir() or not BASE.name.startswith("navkit-sync-sim-"):
    raise RuntimeError("owned sync simulator directory required")
DATABASE = os.environ["NAVKIT_CH_DATABASE"]
AUTH = "Basic " + base64.b64encode((os.environ["NAVKIT_CH_USER"] + ":" + os.environ["NAVKIT_CH_PASSWORD"]).encode()).decode()
LOCK = threading.RLock()
CATALOGUE = {}
CONN = sqlite3.connect(BASE / "state.sqlite3", check_same_thread=False, isolation_level=None)
CONN.execute("ATTACH DATABASE ? AS " + '"' + DATABASE.replace('"', '""') + '"', (str(BASE / "tables.sqlite3"),))


def split_fields(text):
    fields = []
    start = 0
    depth = 0
    quote = None
    i = 0
    while i < len(text):
        c = text[i]
        if quote:
            if c == quote:
                if i + 1 < len(text) and text[i + 1] == quote:
                    i += 1
                else:
                    quote = None
        elif c in "'\"`":
            quote = c
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
        elif c == "," and depth == 0:
            fields.append(text[start:i].strip())
            start = i + 1
        i += 1
    if quote or depth:
        raise ValueError("unbalanced SQL field list")
    fields.append(text[start:].strip())
    return fields


def sql_identifiers(sql):
    out = []
    i = 0
    while i < len(sql):
        if sql[i] == "'":
            start = i
            i += 1
            while i < len(sql):
                if sql[i] == "'":
                    i += 1
                    if i < len(sql) and sql[i] == "'":
                        i += 1
                    else:
                        break
                else:
                    i += 1
            else:
                raise ValueError("unterminated SQL literal")
            out.append(sql[start:i])
        elif sql[i] == "`":
            i += 1
            identifier = []
            while i < len(sql):
                if sql[i] == "`":
                    i += 1
                    if i < len(sql) and sql[i] == "`":
                        identifier.append("`")
                        i += 1
                    else:
                        break
                else:
                    identifier.append(sql[i])
                    i += 1
            else:
                raise ValueError("unterminated SQL identifier")
            out.append('"' + "".join(identifier).replace('"', '""') + '"')
        else:
            out.append(sql[i])
            i += 1
    return "".join(out)


def concrete_table(sql):
    match = re.search(r"(?:FROM|INTO|TABLE)\s+`([A-Za-z0-9_]+)`\.`([A-Za-z0-9_]+)`", sql, re.I)
    if not match or match.group(1) != DATABASE:
        raise ValueError("concrete owned database/table required")
    return match.group(2)


def execute(sql):
    with LOCK:
        return execute_locked(sql)


def execute_locked(sql):
    sql = sql.strip().rstrip(";").strip()
    if re.match(r"^UPDATE\b", sql, re.I):
        raise ValueError("ClickHouse profile rejects standard UPDATE; use ALTER TABLE UPDATE")
    if re.match(r"^SHOW\s+DATABASES$", sql, re.I):
        return ["name"], ["String"], [[DATABASE]], 0
    if re.match(r"^SHOW\s+TABLES(?:\s+FROM\s+`[A-Za-z0-9_]+`)?$", sql, re.I):
        return ["name"], ["String"], [[n] for n in sorted(CATALOGUE)], 0
    if re.match(r"^DESCRIBE\s+TABLE\b", sql, re.I):
        table = concrete_table(sql)
        if table not in CATALOGUE:
            raise ValueError("unknown table")
        return ["name", "type", "default_type", "default_expression", "comment"], ["String"] * 5, [
            [c["name"], c["type"], "", "", ""] for c in CATALOGUE[table]
        ], 0
    if re.match(r"^CREATE\s+TABLE\b", sql, re.I):
        table = concrete_table(sql)
        match = re.match(r"^CREATE\s+TABLE\s+`[A-Za-z0-9_]+`\.`[A-Za-z0-9_]+`\s*\((.*)\)\s+ENGINE\s*=\s*MergeTree\s+ORDER\s+BY\s+.+$", sql, re.I | re.S)
        if not match:
            raise ValueError("only explicit MergeTree CREATE/ORDER BY subset supported")
        definitions = []
        cols = []
        for field in split_fields(match.group(1)):
            column = re.match(r"^`([A-Za-z0-9_]+)`\s+([A-Za-z0-9_]+(?:\([^)]*\))?(?:\))?)\s*(NOT\s+NULL)?$", field, re.I)
            if not column:
                raise ValueError("column definition outside documented sync subset: " + field)
            name, typ, not_null = column.groups()
            nullable = typ.startswith("Nullable(") and not not_null
            underlying = typ[9:-1] if typ.startswith("Nullable(") else typ
            if not (underlying == "String" or re.fullmatch(r"U?Int(?:8|16|32|64)", underlying) or underlying.startswith(("Decimal(", "Float", "Date", "DateTime"))):
                raise ValueError("unsupported ClickHouse type: " + typ)
            sqlite_type = "INTEGER" if "Int" in underlying else ("NUMERIC" if underlying.startswith(("Decimal", "Float")) else "TEXT")
            definitions.append('"' + name + '" ' + sqlite_type + ("" if nullable else " NOT NULL"))
            cols.append({"name": name, "type": typ})
        CONN.execute('CREATE TABLE "' + DATABASE + '"."' + table + '" (' + ",".join(definitions) + ")")
        CATALOGUE[table] = cols
        return [], [], [], 0
    if re.match(r"^DROP\s+TABLE\b", sql, re.I):
        table = concrete_table(sql)
        CONN.execute(sql_identifiers(sql))
        CATALOGUE.pop(table, None)
        return [], [], [], 0
    if re.match(r"^DELETE\s+FROM\b", sql, re.I) and not re.search(r"\bWHERE\b", sql, re.I):
        raise ValueError("ClickHouse lightweight DELETE profile requires WHERE predicate")
    translated = sql_identifiers(sql)
    translated = re.sub(r"\bcount\(\)", "COUNT(*)", translated, flags=re.I)
    if not re.match(r"^(SELECT|INSERT|DELETE)\b", sql, re.I):
        raise ValueError("SQL outside documented mutable ClickHouse subset")
    cur = CONN.execute(translated)
    rows = [list(r) for r in cur.fetchall()] if cur.description else []
    names = [c[0] for c in cur.description] if cur.description else []
    types = []
    for i in range(len(names)):
        values = [r[i] for r in rows if r[i] is not None]
        types.append("Int64" if values and all(isinstance(x, int) for x in values) else ("Float64" if values and all(isinstance(x, float) for x in values) else "Nullable(String)"))
    return names, types, rows, max(cur.rowcount, 0)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        if not 0 < length <= 1048576:
            self.send_error(413)
            return
        body = self.rfile.read(length).decode("utf-8")
        formatted = body.rstrip().endswith("FORMAT JSONCompactEachRowWithNamesAndTypes")
        sql = body.rsplit("FORMAT JSONCompactEachRowWithNamesAndTypes", 1)[0].strip() if formatted else body
        status = 200
        written = 0
        try:
            if self.headers.get("Authorization") != AUTH or self.headers.get("X-ClickHouse-Database") != DATABASE:
                raise PermissionError("test-only ClickHouse authentication/database mismatch")
            names, types, rows, written = execute(sql)
            if formatted:
                payload = "\n".join(json.dumps(x, ensure_ascii=False) for x in [names, types, *rows]).encode("utf-8")
            else:
                payload = b""
        except (ValueError, sqlite3.Error, PermissionError) as error:
            status = 401 if isinstance(error, PermissionError) else 400
            payload = str(error).encode("utf-8")
        with LOCK:
            with (BASE / "wire.jsonl").open("a", encoding="utf-8") as log:
                log.write(json.dumps({"sql": sql, "status": status, "written_rows": written, "error": payload.decode("utf-8") if status != 200 else None}, ensure_ascii=False) + "\n")
        self.send_response(status)
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Content-Type", "application/json" if formatted and status == 200 else "text/plain")
        self.send_header("X-ClickHouse-Summary", json.dumps({"written_rows": str(written)}))
        self.end_headers()
        self.wfile.write(payload)


def self_check():
    execute("CREATE TABLE `" + DATABASE + "`.`ROWS` (`ID` Int64 NOT NULL,`PAYLOAD` Nullable(String)) ENGINE = MergeTree ORDER BY (ID)")
    execute("INSERT INTO `" + DATABASE + "`.`ROWS` VALUES (1,'中文;O''Reilly\\tail'),(2,NULL)")
    names, _, rows, _ = execute("SELECT * FROM `" + DATABASE + "`.`ROWS` ORDER BY `ID`")
    assert names == ["ID", "PAYLOAD"] and rows == [[1, "中文;O'Reilly\\tail"], [2, None]]
    assert execute("DESCRIBE TABLE `" + DATABASE + "`.`ROWS`")[2][1][1] == "Nullable(String)"
    execute("CREATE TABLE `" + DATABASE + "`.`STRICT` (`ID` Int64 NOT NULL,`PAYLOAD` String) ENGINE = MergeTree ORDER BY (ID)")
    invalid = (
        "INSERT INTO `" + DATABASE + "`.`STRICT` VALUES (1,NULL)",
        "UPDATE `" + DATABASE + "`.`ROWS` SET PAYLOAD='changed' WHERE ID=1",
        "DELETE FROM `" + DATABASE + "`.`ROWS`",
        "THIS IS NOT VALID SQL",
    )
    for sql in invalid:
        try:
            execute(sql)
        except (ValueError, sqlite3.Error):
            pass
        else:
            raise AssertionError("strict simulator accepted invalid ClickHouse profile")
    execute("DROP TABLE `" + DATABASE + "`.`ROWS`")
    execute("DROP TABLE `" + DATABASE + "`.`STRICT`")
    print("ClickHouse sync fixture self-check: mutable SQL/metadata/Nullable/strict errors OK; NOT product synchronization")


if __name__ == "__main__":
    if "--self-check" in sys.argv:
        import faulthandler
        faulthandler.dump_traceback_later(5)
        try:
            self_check()
        finally:
            faulthandler.cancel_dump_traceback_later()
            CONN.close()
    else:
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        (BASE / "ready.json").write_text(json.dumps({"port": server.server_port, "protocol": "ClickHouse HTTP SQL subset", "backend": "SQLite state", "vendor_server_status": "NOT_TESTED"}), encoding="ascii")
        server.serve_forever(poll_interval=0.1)
