"""Cargo runner：仅执行当前 Cargo 测试产物的隔离副本，补入 Windows loader manifest。"""
import ctypes
import os
from pathlib import Path
import subprocess
import sys
import json
import shutil
import tempfile

source = Path(os.environ['NAVKIT_TEST_SOURCE']).resolve(strict=True)
assert Path(sys.argv[1]).resolve(strict=True) == source, '拒绝运行非本次 Cargo 产物'
stat = source.stat()
ctypes.windll.kernel32.SetErrorMode(3)
prepared = os.environ['NAVKIT_TEST_PREPARED']
if str(stat.st_size) != os.environ['NAVKIT_TEST_SOURCE_SIZE'] or str(stat.st_mtime_ns) != os.environ['NAVKIT_TEST_SOURCE_MTIME']:
    # Cargo 若因其他 owner 的最新源码重新链接，必须执行这次传入的当前产物。
    directory = Path(tempfile.mkdtemp(prefix='relinked-', dir=Path(prepared).parent.parent))
    prepared = str(directory / 'navkit-workflows-tests.exe')
    shutil.copyfile(source, prepared)
    current = source.stat()
    assert (current.st_size,current.st_mtime_ns) == (stat.st_size,stat.st_mtime_ns), '复制期间 Cargo 产物发生变化'
    subprocess.run([os.environ['NAVKIT_TEST_MT'], '-nologo', '-manifest', os.environ['NAVKIT_TEST_MANIFEST'], f'-outputresource:{prepared};#1'], check=True)
    print(json.dumps({'loader':'重新链接后当前 Cargo 产物的隔离 manifest 副本','source':str(source),'prepared':prepared},ensure_ascii=False), flush=True)
sys.exit(subprocess.call([prepared, *sys.argv[2:]]))
