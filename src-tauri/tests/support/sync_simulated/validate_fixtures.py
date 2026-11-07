"""Fixture-only offline smoke checks. Does NOT run navkit synchronization."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[3]
checks = []
for name in ("tds", "clickhouse"):
    root = Path(tempfile.mkdtemp(prefix="navkit-sync-sim-offline-"))
    env = dict(os.environ, NAVKIT_TDS_USER="offline_fixture", NAVKIT_TDS_PASSWORD="offline_fixture", NAVKIT_TDS_DATABASE="OFFLINE", NAVKIT_CH_USER="offline_fixture", NAVKIT_CH_PASSWORD="offline_fixture", NAVKIT_CH_DATABASE="OFFLINE")
    args = [sys.executable, str(HERE / ("tds_sync_server.py" if name == "tds" else "clickhouse_sync_server.py")), str(root)]
    if name == "tds":
        args.append(str(REPO / "tools/acceptance/simulation/native/tds_server.py"))
    args.append("--self-check")
    entry = {"name": name, "evidence_type": "OFFLINE_FIXTURE_ONLY_NOT_PRODUCT_SYNC", "vendor_server_status": "NOT_TESTED"}
    try:
        result = subprocess.run(args, env=env, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=30)
        entry.update(exit_code=result.returncode, stdout=result.stdout, stderr=result.stderr)
    except Exception as error:
        entry.update(exit_code=-1, error=str(error))
    finally:
        shutil.rmtree(root)
        entry["cleanup"] = not root.exists()
    checks.append(entry)
print(json.dumps(checks, ensure_ascii=False, indent=2))
sys.exit(0 if all(c["exit_code"] == 0 and c["cleanup"] for c in checks) else 1)
