"""隔离进程/造表管理；不使用Python数据库驱动，不在Python中计查询性能。"""
import csv
import ctypes
import hashlib
import json
import os
import platform
import queue
import re
import shutil
import subprocess
import sys
import time
import threading
import uuid
from datetime import datetime, timezone
from pathlib import Path

sys.stdout.reconfigure(encoding='utf-8')
sys.stderr.reconfigure(encoding='utf-8')

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / 'artifacts/stream-preview-eval-20261115'
EVIDENCE.mkdir(parents=True, exist_ok=True)
BASELINE = '3b67b6120b1a0dc539bef5655ea06a031a9f9508'
HIDDEN = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
now = lambda: datetime.now(timezone.utc).isoformat()
assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip() == BASELINE
binary = Path(sys.argv[1]).resolve()
assert binary.is_relative_to(ROOT) and binary.is_file(), '需要当前worktree中新鲜release测试exe'
stages = list(map(int,sys.argv[2].split(','))) if len(sys.argv)>2 else [10_000,100_000,1_000_000,10_000_000,100_000_000]
assert all(n in [10_000,100_000,1_000_000,10_000_000,100_000_000] for n in stages)
assert all(a < b for a,b in zip(stages,stages[1:])), '规模必须严格递增'
fixture_path = Path('D:/github/tauri-navicat/tools/acceptance/e2e/desktop14-runner-e2c772c4.mjs')
password = os.environ.get('NAVKIT_MYSQL_PASSWORD')
if password is None:
    match = re.search(r"'-p([^']+)'",fixture_path.read_text(encoding='utf-8'))
    assert match, '授权fixture凭据未找到'
    password = match[1]
assert password, '授权fixture密码为空'
host = os.environ.get('NAVKIT_MYSQL_HOST','127.0.0.1')
assert host in ['127.0.0.1','localhost','::1'], '仅允许loopback'
port = int(os.environ.get('NAVKIT_MYSQL_PORT','3306'))
user = os.environ.get('NAVKIT_MYSQL_USER','root')
mysql = os.environ.get('NAVKIT_MYSQL_CLIENT','C:/Program Files/MySQL/MySQL Server 8.0/bin/mysql.exe')
namespace = 'navkit_stream_bench_' + uuid.uuid4().hex
assert re.fullmatch(r'navkit_stream_bench_[0-9a-f]{32}',namespace)
q = '`' + namespace + '`'
tag = re.sub(r'[^0-9]','',now())
records_file = EVIDENCE / f'runs-{tag}.jsonl'
manifest_path = EVIDENCE / f'manifest-{tag}.json'
env = {**os.environ, 'MYSQL_PWD':password,'NAVKIT_MYSQL_PASSWORD':password,'NAVKIT_MYSQL_HOST':host,
       'NAVKIT_MYSQL_PORT':str(port),'NAVKIT_MYSQL_USER':user,'NAVKIT_STREAM_BENCH':'1',
       'NAVKIT_STREAM_BENCH_DATABASE':namespace,'NAVKIT_STREAM_BENCH_OUTPUT':str(records_file),
       'NAVKIT_SPOOL_ISOLATION':'0'}
redact = lambda value: str(value).replace(password,'[redacted]')

class GlobalMemory(ctypes.Structure):
    _fields_ = [('length',ctypes.c_uint32),('load',ctypes.c_uint32),
                *[(name,ctypes.c_uint64) for name in ['total','available','page_total','page_available',
                    'virtual_total','virtual_available','extended']]]

def memory():
    if os.name != 'nt':
        return {'total':None,'free':None}
    status = GlobalMemory()
    status.length = ctypes.sizeof(status)
    assert ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)), '读取系统内存失败'
    return {'total':status.total,'free':status.available}

cpu = subprocess.run(['pwsh','-NoProfile','-Command','(Get-CimInstance Win32_Processor | Select-Object -First 1).Name'],
                     capture_output=True,text=True,creationflags=HIDDEN,timeout=30).stdout.strip()
report = {'baseline':BASELINE,'binary':str(binary),'binarySha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
          'started':now(),'namespace':namespace,'host':host,'port':port,'fixtureSource':str(fixture_path),'stages':stages,
          'machine':{'platform':platform.platform(),'arch':platform.machine(),'cpu':cpu or platform.processor(),
                     'logicalCpus':os.cpu_count(),'totalRamBytes':memory()['total'],'freeRamBytesAtStart':memory()['free'],
                     'freeDiskBytesAtStart':shutil.disk_usage(ROOT).free},
          'schema':'bench_rows(id BIGINT NOT NULL PRIMARY KEY,payload CHAR(16) CHARACTER SET ascii NOT NULL) ENGINE=InnoDB',
          'payload':'abcdefghijklmnop','seedBatchRows':100_000,'seed':[],'runs':[],'cleanup':[],
          'limits':{'perQuerySeconds':1800,'seedSeconds':1800,'minimumFreeRamBytes':2*1024**3,
                    'minimumFreeDiskBytes':40*1024**3,'clientPrivateBytes':4*1024**3},
          'cachePolicy':{'singleSetBytes':512*1024**2,'globalBytes':2*1024**3},
          'scope':'Rust命令服务，不含IPC/WebView/DOM；同一实体表、SQL、grant、产品驱动、缓存；共享MySQL自然缓存',
          'seedPolicy':'最终轮先建固定实体表，按id<=N输出各档；单个本机厂商客户端连接，每批10万行并确认提交；播种与验证单列计时',
          'warmup':'每档old/stream各一轮LIMIT200，round=0不入中位数',
          'cpuScope':'GetProcessTimes仅统计Rust测试进程；MySQL服务CPU未测',
          'memoryScope':'K32GetProcessMemoryInfo每100ms采样Rust测试进程RSS/private；MySQL内存未测',
          'processManager':'Python subprocess仅进程与夹具管理；计时在Rust服务；Node输出捕获失败的首轮manifest保留'}

def persist():
    manifest_path.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')

def sql(statement,timeout=180):
    result = subprocess.run([mysql,'-h',host,'-P',str(port),'-u',user,'--default-character-set=utf8mb4','-N','-B','-e',statement],
                            cwd=ROOT,env=env,capture_output=True,encoding='utf-8',errors='replace',
                            creationflags=HIDDEN,timeout=timeout)
    if result.returncode != 0:
        raise RuntimeError('隔离MySQL操作失败：'+redact(result.stderr))
    return result.stdout

def check_resources():
    free_disk = shutil.disk_usage(ROOT).free
    free_ram = memory()['free']
    assert free_disk >= report['limits']['minimumFreeDiskBytes'], '磁盘低于40GiB止损'
    assert free_ram is None or free_ram >= report['limits']['minimumFreeRamBytes'], '系统内存低于2GiB止损'
    return {'freeDisk':free_disk,'freeRam':free_ram}

def run_stage(n):
    args = [str(binary),'execution_tests::stream_preview_bench::live_mysql_old_vs_stream_preview_benchmark',
            '--exact','--ignored','--nocapture','--test-threads=1']
    log_path = EVIDENCE / f'stage-{n}-{tag}.log'
    stage = {'rows':n,'args':args,'started':now(),'log':str(log_path)}
    report['runs'].append(stage)
    passed_one = False
    with log_path.open('w',encoding='utf-8') as log:
        process = subprocess.Popen(args,cwd=ROOT,env={**env,'NAVKIT_STREAM_BENCH_ROWS':str(n)},
                    stdout=subprocess.PIPE,stderr=subprocess.STDOUT,encoding='utf-8',errors='replace',
                    creationflags=HIDDEN,bufsize=1)
        stage['pid'] = process.pid
        persist()
        # 不用外部进程强杀查询；Rust自身止损必须等实际排空，进程退出后才清理独立库。
        for raw_line in process.stdout:
            line = redact(raw_line)
            log.write(line); log.flush()
            if 'test result: ok. 1 passed;' in line:
                passed_one = True
            marker = 'STREAM_BENCH '
            if marker in line:
                r = json.loads(line.split(marker,1)[1])
                first = r.get('first_200_available_ms')
                first_text = f'{first:.2f}' if first is not None else '未实测'
                print(f"实测 输出范围={n} 实体表={r['source_rows']} {'LIMIT200' if r['limit_200_control'] else '全量'} {r['mode']} round={r['round']} "
                      f"200行={first_text}ms final={r['final_ms']:.2f}ms cached={r.get('cached_rows')} truncated={r.get('cache_truncated')}",flush=True)
        stage['code'] = process.wait()
    stage['finished'] = now()
    persist()
    assert stage['code']==0 and passed_one, f'Rust阶段{n}失败或非零测试通过证据缺失，见脱敏日志'
    actual = [json.loads(line) for line in records_file.read_text(encoding='utf-8').splitlines() if line]
    stage_records = [r for r in actual if r.get('range_upper_bound',r['source_rows'])==n]
    assert len(stage_records)==(14 if n<=1_000_000 else 6), '实测逐轮记录数量不匹配'

def seed_fixed_table(n):
    """只发送一个有界批次并等待服务器ACK后再发送下一批；不堆积SQL/行对象。"""
    started = time.monotonic()
    args=[mysql,'-h',host,'-P',str(port),'-u',user,'--default-character-set=utf8mb4','-N','-B','--raw','--unbuffered']
    process=subprocess.Popen(args,cwd=ROOT,env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,
        encoding='utf-8',errors='replace',creationflags=HIDDEN,bufsize=1)
    output=queue.Queue(maxsize=8)
    def read_lines():
        for line in process.stdout:
            output.put(line.rstrip('\r\n'))
        output.put(None)
    reader=threading.Thread(target=read_lines,daemon=True)
    reader.start()
    current=0
    try:
        while current<n:
            check_resources()
            assert time.monotonic()-started<1800, '固定表播种超过30分钟止损'
            take=min(100_000,n-current)
            next_rows=current+take
            statement=(f"INSERT INTO {q}.bench_rows SELECT {current}+n,'abcdefghijklmnop' FROM {q}.bench_numbers WHERE n<={take} ORDER BY n; "
                       f"SELECT '{namespace}',{next_rows};\n")
            process.stdin.write(statement); process.stdin.flush()
            expected=f'{namespace}\t{next_rows}'
            while True:
                line=output.get(timeout=180)
                if line is None:
                    raise RuntimeError('本机播种客户端在ACK前结束')
                if line.startswith('ERROR'):
                    raise RuntimeError('本机播种失败：'+redact(line))
                if line==expected:
                    break
            current=next_rows
            if current%10_000_000==0:
                print(f'固定实体表播种进度{current}行；单批上限10万',flush=True)
    finally:
        process.stdin.close()
        # 正常ACK后无在途批次；异常亦先关闭输入并等待本轮客户端退出。
        try:
            process.wait(timeout=180)
        except subprocess.TimeoutExpired:
            process.terminate(); process.wait(timeout=30)
        reader.join(timeout=5)
    assert process.returncode==0, '本轮播种客户端失败'
    return current,(time.monotonic()-started)*1000

created = False
rows = 0
try:
    check_resources()
    report['mysqlVersion'] = sql('SELECT VERSION()').strip()
    assert report['mysqlVersion'], '真实服务器版本不可为空'
    sql('CREATE DATABASE '+q+' CHARACTER SET ascii')
    created = True
    persist()
    sql('CREATE TABLE '+q+'.bench_rows(id BIGINT NOT NULL PRIMARY KEY,payload CHAR(16) CHARACTER SET ascii NOT NULL) ENGINE=InnoDB; '+
        'CREATE TABLE '+q+'.bench_digits(d INT NOT NULL PRIMARY KEY); INSERT INTO '+q+'.bench_digits VALUES(0),(1),(2),(3),(4),(5),(6),(7),(8),(9); '+
        'CREATE TABLE '+q+'.bench_numbers(n INT NOT NULL PRIMARY KEY); INSERT INTO '+q+'.bench_numbers '+
        'SELECT 1+a.d+10*b.d+100*c.d+1000*d.d+10000*e.d FROM '+q+'.bench_digits a CROSS JOIN '+q+'.bench_digits b CROSS JOIN '+q+'.bench_digits c CROSS JOIN '+q+'.bench_digits d CROSS JOIN '+q+'.bench_digits e;')
    resources=check_resources()
    rows,seed_ms=seed_fixed_table(max(stages))
    verified_started=time.monotonic()
    aggregate=list(map(int,sql(f'SELECT COUNT(*),MIN(id),MAX(id),MIN(OCTET_LENGTH(payload)),MAX(OCTET_LENGTH(payload)) FROM {q}.bench_rows').strip().split('\t')))
    assert aggregate==[rows,1,rows,16,16], '实体表计数/边界/行宽校验失败'
    report['fixedTableRows']=rows
    report['seed'].append({'target':max(stages),'actualRows':rows,'seedMs':seed_ms,'verificationMs':(time.monotonic()-verified_started)*1000,
                          'aggregate':aggregate,'resources':resources})
    env['NAVKIT_STREAM_BENCH_TABLE_ROWS']=str(rows)
    persist()
    print(f'固定实体表{rows}行就绪；播种{seed_ms:.2f}ms，不计入查询',flush=True)
    ready_path=os.environ.get('NAVKIT_STREAM_BENCH_WAIT_FILE')
    if ready_path:
        ready_path=Path(ready_path).resolve()
        assert ready_path.is_relative_to(EVIDENCE), '构建标记必须位于本轮隔离产物目录'
        waiting_since=time.monotonic()
        while not ready_path.exists():
            check_resources()
            assert time.monotonic()-waiting_since<1800, '等待新鲜release构建超过30分钟'
            time.sleep(1)
        ready=json.loads(ready_path.read_text(encoding='utf-8-sig'))
        assert ready['baseline']==BASELINE
        assert Path(ready['binary']).resolve()==binary
        assert hashlib.sha256(binary.read_bytes()).hexdigest()==ready['sha256']
        report['binarySha256']=ready['sha256']
        report['buildCompleted']=ready['buildCompleted']
    else:
        report['binarySha256']=hashlib.sha256(binary.read_bytes()).hexdigest()
    report['queriesStartedAfterSeedAndBuild']=now()
    persist()
    for n in stages:
        run_stage(n)
    report['state'] = 'completed'
except Exception as error:
    report['state'] = 'failed_or_stopped'
    report['error'] = redact(error)
    print(report['error'],file=sys.stderr,flush=True)
finally:
    if created:
        try:
            assert re.fullmatch(r'navkit_stream_bench_[0-9a-f]{32}',namespace)
            sql('DROP DATABASE '+q,timeout=300)
            absent = sql(f"SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name='{namespace}'").strip()
            assert absent=='0', '本轮库清理核验未通过'
            report['cleanup'].append({'database':namespace,'state':'removed','verifiedAbsent':True})
            print('本轮独立库已删除并核验 '+namespace,flush=True)
        except Exception as error:
            report['cleanup'].append({'database':namespace,'state':'failed','error':redact(error)})
            report['state'] = 'failed_or_stopped'
    report['finished'] = now()
    persist()
    if records_file.exists():
        records = [json.loads(line) for line in records_file.read_text(encoding='utf-8').splitlines() if line]
        (EVIDENCE/f'runs-{tag}.json').write_text(json.dumps(records,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
        columns = ['source_rows','range_upper_bound','output_rows','limit_200_control','round','mode','preview_200_ms','first_200_available_ms',
                   'first_200_source','final_ms','drained_ms','cached_rows','cached_bytes','cache_truncated','complete_export_allowed',
                   'total_drained_rows','rss_peak_bytes','private_peak_bytes','process_cpu_ms','sql_log_count','state','stop_reason']
        with (EVIDENCE/f'runs-{tag}.csv').open('w',newline='',encoding='utf-8') as csv_file:
            writer = csv.DictWriter(csv_file,fieldnames=columns,extrasaction='ignore')
            writer.writeheader(); writer.writerows(records)
    print('证据 '+str(manifest_path),flush=True)
sys.exit(0 if report.get('state')=='completed' else 1)
