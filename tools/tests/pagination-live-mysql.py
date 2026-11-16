"""真实 MySQL 端到端：建 UUID 库 → 跑 ignored 集成测试 → 清理。凭据仅从已授权本机 fixture 读取，不回显。"""
import os, re, subprocess, sys, uuid
from pathlib import Path

ROOT = Path('D:/github/navkit-query-pagination')
fixture = Path('D:/github/tauri-navicat/tools/acceptance/e2e/desktop14-runner-e2c772c4.mjs')
match = re.search(r"'-p([^']+)'", fixture.read_text(encoding='utf-8'))
assert match, 'authorized fixture password not found'
pw = match[1]
mysql = 'C:/Program Files/MySQL/MySQL Server 8.0/bin/mysql.exe'
ns = 'navkit_pgtest_' + uuid.uuid4().hex
base_env = {**os.environ, 'MYSQL_PWD': pw}

def sql(statement, timeout=120):
    r = subprocess.run([mysql, '-h', '127.0.0.1', '-P', '3306', '-u', 'root',
                        '--default-character-set=utf8mb4', '-e', statement],
                       env=base_env, capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.replace(pw, '[redacted]'))
    return r.stdout

sql('CREATE DATABASE ' + ns + ' CHARACTER SET utf8mb4')
code = 1
try:
    env = {**os.environ,
           'NAVKIT_MYSQL_HOST': '127.0.0.1', 'NAVKIT_MYSQL_PORT': '3306',
           'NAVKIT_MYSQL_USER': 'root', 'NAVKIT_MYSQL_PASSWORD': pw,
           'NAVKIT_MYSQL_DATABASE': ns,
           'CARGO_TARGET_DIR': 'D:/github/navkit-stream-preview-eval/src-tauri/target'}
    r = subprocess.run(['cargo', 'test', '--manifest-path', 'src-tauri/Cargo.toml', '--lib',
                        'mysql_live_pagination', '--', '--ignored', '--nocapture', '--test-threads=1'],
                       cwd=str(ROOT), env=env, capture_output=True, text=True, timeout=1200)
    out = ((r.stdout or '') + (r.stderr or '')).replace(pw, '[redacted]')
    print(out[-4000:])
    code = 0 if r.returncode == 0 else 1
finally:
    try:
        sql('DROP DATABASE IF EXISTS ' + ns)
        print('cleanup: database', ns, 'dropped')
    except Exception as e:
        print('cleanup warning:', str(e).replace(pw, '[redacted]'))
sys.exit(code)
