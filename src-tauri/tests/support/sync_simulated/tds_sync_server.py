"""TEST ONLY: bounded TDS 7.4 wire + mutable SQLite, NOT Microsoft SQL Server.

The wire codec is the existing catalogue fixture. It is loaded without its
listener, then only execute/text_result are replaced for the documented sync
subset. This keeps real LOGIN7/RPC/row tokens and the actual tiberius client.
MAX types/0x binary literals have explicit translations; X'hex' is REJECTED
rather than accidentally accepted by SQLite. No vendor/TLS/transaction claim.
"""
import ast
import json
import os
import re
import socket
import sqlite3
import struct
import sys
import threading
import time
from pathlib import Path

BASE = Path(sys.argv[1]).resolve()
if not BASE.is_dir() or not BASE.name.startswith("navkit-sync-sim-"):
    raise RuntimeError("owned sync simulator directory required")
WIRE_SOURCE = Path(sys.argv[2]).resolve()
source = WIRE_SOURCE.read_text(encoding="utf-8")
module = ast.parse(source, filename=str(WIRE_SOURCE))
# An explicit AST boundary, not a textual split inside strings/comments.
# Reject an incompatible fixture instead of silently running its listener.
listener_index = next(
    (i for i, node in enumerate(module.body)
     if isinstance(node, ast.Assign)
     and any(isinstance(t, ast.Name) and t.id == "listener" for t in node.targets)),
    None,
)
if listener_index is None:
    raise RuntimeError("catalogue TDS fixture listener boundary not found")
body = ast.Module(body=module.body[:listener_index], type_ignores=[])
# Its only ownership assertion uses the catalogue prefix. Supply the owned
# directory explicitly and remove precisely that assignment/assert pair.
body.body = [n for n in body.body if not (
    isinstance(n, ast.Assign)
    and any(isinstance(t, ast.Name) and t.id == "base" for t in n.targets)
) and not (isinstance(n, ast.Assert) and "base.name" in ast.unparse(n.test))]
wire = {"__name__": "sync_tds_wire", "__file__": str(WIRE_SOURCE), "base": BASE}
exec(compile(body, str(WIRE_SOURCE), "exec"), wire)

COUNTS = {}
COUNT_LOCK = threading.Lock()


def literals(sql):
    """Transform only outside quoted strings/identifiers; no global replacement."""
    out = []
    i = 0
    while i < len(sql):
        c = sql[i]
        if c in "'\"[":
            quote = "]" if c == "[" else c
            start = i
            i += 1
            closed = False
            while i < len(sql):
                if sql[i] == quote:
                    if i + 1 < len(sql) and sql[i + 1] == quote:
                        i += 2
                    else:
                        i += 1
                        closed = True
                        break
                else:
                    i += 1
            if not closed:
                raise ValueError("unterminated quoted SQL")
            out.append(sql[start:i])
            continue
        boundary = i == 0 or not (sql[i - 1].isalnum() or sql[i - 1] == "_")
        if boundary and c in "Xx" and i + 1 < len(sql) and sql[i + 1] == "'":
            raise ValueError("SQL Server binary syntax requires 0xHEX; X'hex' is invalid")
        if boundary and c in "Nn" and i + 1 < len(sql) and sql[i + 1] == "'":
            i += 1
            continue
        if boundary and sql[i:i + 2].lower() == "0x":
            match = re.match(r"0[xX]([0-9a-fA-F]+)", sql[i:])
            if not match or len(match.group(1)) % 2:
                raise ValueError("invalid even-length binary literal")
            out.append("X'" + match.group(1) + "'")
            i += len(match.group(0))
            continue
        out.append(c)
        i += 1
    return "".join(out)


def failpoint(sql):
    match = re.match(r"\s*INSERT\s+INTO\s+(?:\[dbo\]\.)?\[([A-Za-z0-9_]+)\]", sql, re.I)
    if not match:
        return
    table = match.group(1)
    config_path = BASE / ("hook-" + table + ".json")
    if not config_path.exists():
        return
    cfg = json.loads(config_path.read_text(encoding="utf-8"))
    with COUNT_LOCK:
        count = COUNTS.get(table, 0) + 1
        COUNTS[table] = count
    if count != cfg["ordinal"]:
        return
    (BASE / ("entered-" + table)).write_text("entered", encoding="ascii")
    if cfg["action"] == "fail":
        raise ValueError("test-only deterministic INSERT fault after earlier auto-commit")
    if cfg["action"] != "pause":
        raise ValueError("unknown test-only fault action")
    deadline = time.monotonic() + 10
    while not (BASE / ("release-" + table)).exists():
        if time.monotonic() > deadline:
            raise ValueError("test-only INSERT pause deadline")
        time.sleep(0.01)


def execute(conn, sql):
    outside = wire["outside_sql"](sql)
    lower = sql.lower()
    if re.search(r"\blimit\b", outside, re.I):
        raise ValueError("SQL Server dialect rejects LIMIT")
    if "information_schema.columns" in lower:
        table = wire["named_table"](sql)
        rows = conn.execute(
            'SELECT name,type,"notnull",dflt_value FROM pragma_table_info(?,\'dbo\') ORDER BY cid',
            (table,),
        ).fetchall()
        types = {"TEXT": "nvarchar", "BLOB": "varbinary"}
        return ["COLUMN_NAME", "DATA_TYPE", "IS_NULLABLE", "COLUMN_DEFAULT"], [
            [n, types.get(t.upper(), t.lower()), "NO" if nn else "YES", d]
            for n, t, nn, d in rows
        ], 0
    if any(q in lower for q in ("from sys.", "@@version", "information_schema.table_constraints")):
        return wire_execute(conn, sql)
    failpoint(sql)
    translated = literals(sql)
    # MAX is a documented SQL Server construct, not SQLite SQL. Restrict type
    # substitutions to CREATE TABLE, never alter a sentinel string in INSERT.
    if re.match(r"\s*CREATE\s+TABLE\b", outside, re.I):
        translated = re.sub(r"\bN?VARCHAR\s*\(\s*MAX\s*\)", "TEXT", translated, flags=re.I)
        translated = re.sub(r"\bVARBINARY\s*\(\s*MAX\s*\)", "BLOB", translated, flags=re.I)
    cur = conn.execute(translated)
    rows = cur.fetchall() if cur.description else []
    return [c[0] for c in cur.description] if cur.description else [], rows, max(cur.rowcount, 0)


def text_result(s, columns, rows, rpc=False, affected=0):
    types = []
    for i in range(len(columns)):
        values = [r[i] for r in rows if r[i] is not None]
        typ = "binary" if values and all(isinstance(v, bytes) for v in values) else (
            "int" if values and all(isinstance(v, int) for v in values) else (
                "float" if values and all(isinstance(v, float) for v in values) else "text"))
        types.append(typ)
    payload = bytearray(b"\x81" + struct.pack("<H", len(columns)))
    for name, typ in zip(columns, types):
        payload.extend(struct.pack("<IH", 0, 1))
        if typ == "int":
            payload.extend(b"\x26\x08")
        elif typ == "float":
            payload.extend(b"\x6d\x08")
        elif typ == "binary":
            payload.extend(b"\xa5" + struct.pack("<H", 8000))
        else:
            payload.extend(b"\xe7" + struct.pack("<H", 8000) + bytes.fromhex("0904d00034"))
        payload.extend(wire["bv"](name))
    for row in rows:
        payload.append(0xd1)
        for value, typ in zip(row, types):
            if typ in ("int", "float"):
                if value is None:
                    payload.append(0)
                else:
                    payload.extend(b"\x08" + (struct.pack("<q", value) if typ == "int" else struct.pack("<d", value)))
            elif value is None:
                payload.extend(b"\xff\xff")
            else:
                b = value if typ == "binary" else wire["u16"](str(value))
                if len(b) > 8000:
                    raise ValueError("test-only bounded binary/text cell")
                payload.extend(struct.pack("<H", len(b)) + b)
    payload.extend(wire["done"](len(rows) or affected, rpc=rpc))
    wire["send"](s, bytes(payload))


wire_execute = wire["execute"]
wire["execute"] = execute
wire["text_result"] = text_result


def self_check():
    conn = sqlite3.connect(":memory:", isolation_level=None)
    conn.execute("ATTACH DATABASE ':memory:' AS dbo")
    execute(conn, "CREATE TABLE [dbo].[X] ([ID] BIGINT NOT NULL PRIMARY KEY,[PAYLOAD] NVARCHAR(MAX),[BYTES] VARBINARY(MAX))")
    execute(conn, "INSERT INTO [dbo].[X] VALUES (1,N'中文;O''Reilly\\tail',0x00FF4142)")
    columns, rows, _ = execute(conn, "SELECT * FROM [dbo].[X]")
    assert columns == ["ID", "PAYLOAD", "BYTES"]
    assert rows == [(1, "中文;O'Reilly\\tail", bytes.fromhex("00FF4142"))]
    metadata = execute(conn, "SELECT * FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'X'")[1]
    assert metadata[1][1] == "nvarchar" and metadata[2][1] == "varbinary"
    for sql in ("INSERT INTO [dbo].[X] VALUES (2,N'x',X'00FF')", "SELECT * FROM [dbo].[X] LIMIT 1", "THIS IS INVALID SQL"):
        try:
            execute(conn, sql)
        except (ValueError, sqlite3.Error):
            pass
        else:
            raise AssertionError("strict simulator accepted invalid TDS dialect")
    conn.close()


if __name__ == "__main__":
    if "--self-check" in sys.argv:
        self_check()
        print("TDS sync fixture self-check: mutable SQL/MAX/binary/strict errors OK; NOT product synchronization")
    else:
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        (BASE / "ready.json").write_text(json.dumps({
            "port": listener.getsockname()[1], "protocol": "TDS7.4", "backend": "SQLite state", "vendor_server_status": "NOT_TESTED"
        }), encoding="ascii")
        while True:
            client, _ = listener.accept()
            threading.Thread(target=wire["client"], args=(client,), daemon=True).start()
