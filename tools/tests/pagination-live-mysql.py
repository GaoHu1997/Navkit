"""显式授权的真实 MySQL 端到端：建 UUID 库 → 跑 ignored 集成测试 → 核验清理。"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--check-env', action='store_true', help='仅检查显式配置和 Git 基线，不连接数据库或编译')
args = parser.parse_args()

def required_env(name):
    value = os.environ.get(name)
    if not value:
        parser.error(f'必须显式设置 {name}；不会读取其它 fixture 或使用默认数据库凭据')
    return value

pw = required_env('NAVKIT_MYSQL_PASSWORD')
host = required_env('NAVKIT_MYSQL_HOST')
user = required_env('NAVKIT_MYSQL_USER')
port_text = required_env('NAVKIT_MYSQL_PORT')
if host not in ['127.0.0.1', 'localhost', '::1']:
    parser.error('NAVKIT_MYSQL_HOST 仅允许 loopback：127.0.0.1 / localhost / ::1')
if not port_text.isdigit() or not 1 <= int(port_text) <= 65535:
    parser.error('NAVKIT_MYSQL_PORT 必须为 1..65535 的整数')
port = int(port_text)
mysql = shutil.which(os.environ.get('NAVKIT_MYSQL_CLIENT') or 'mysql')
if not mysql:
    parser.error('找不到 MySQL 客户端；请设置 NAVKIT_MYSQL_CLIENT 或将 mysql 加入 PATH')
baseline = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
if args.check_env:
    print(json.dumps({'mode': 'configuration_only', 'baseline': baseline, 'host': host, 'port': port,
                      'mysqlClient': mysql, 'credentialSource': 'explicit_environment'}))
    sys.exit(0)
if not shutil.which('cargo'):
    parser.error('找不到 cargo；请先安装 Rust 工具链')

ns = 'navkit_pgtest_' + uuid.uuid4().hex
assert re.fullmatch(r'navkit_pgtest_[0-9a-f]{32}', ns)
base_env = {**os.environ, 'MYSQL_PWD': pw}
redact = lambda value: str(value).replace(pw, '[redacted]')

def sql(statement, timeout=120):
    result = subprocess.run([mysql, '-h', host, '-P', str(port), '-u', user,
                             '--default-character-set=utf8mb4', '-N', '-B', '-e', statement],
                            cwd=ROOT, env=base_env, capture_output=True, text=True, timeout=timeout)
    if result.returncode != 0:
        raise RuntimeError(redact(result.stderr))
    return result.stdout

created = False
code = 1
try:
    sql('CREATE DATABASE `' + ns + '` CHARACTER SET utf8mb4')
    created = True
    env = {**os.environ, 'NAVKIT_MYSQL_HOST': host, 'NAVKIT_MYSQL_PORT': str(port),
           'NAVKIT_MYSQL_USER': user, 'NAVKIT_MYSQL_PASSWORD': pw, 'NAVKIT_MYSQL_DATABASE': ns}
    # 保留调用者自己的 CARGO_TARGET_DIR；不把 main 的编译重定向到其它工作树。
    result = subprocess.run(['cargo', 'test', '--manifest-path', 'src-tauri/Cargo.toml', '--lib',
                             'mysql_live_pagination', '--', '--ignored', '--nocapture', '--test-threads=1'],
                            cwd=ROOT, env=env, capture_output=True, text=True, timeout=1200)
    output = redact((result.stdout or '') + (result.stderr or ''))
    print(output[-4000:])
    passed = re.search(r'test result: ok\. ([1-9][0-9]*) passed;', output)
    code = 0 if result.returncode == 0 and passed else 1
except Exception as error:
    print(redact(error), file=sys.stderr)
finally:
    if created:
        try:
            assert re.fullmatch(r'navkit_pgtest_[0-9a-f]{32}', ns)
            sql('DROP DATABASE `' + ns + '`')
            remaining = sql("SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name='" + ns + "'").strip()
            if remaining != '0':
                raise RuntimeError('本轮 UUID 数据库删除核验未通过')
            print('cleanup: owned database', ns, 'dropped and verified absent')
        except Exception as error:
            code = 1
            print('cleanup failed:', redact(error), file=sys.stderr)
sys.exit(code)
