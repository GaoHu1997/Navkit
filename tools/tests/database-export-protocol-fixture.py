"""仅测试：MySQL/TDS 协议模拟器，不是厂商数据库；只接受导出测试的 SQL 子集。"""
import ast
import datetime
import json
import os
from pathlib import Path
import re
import socket
import struct
import sys
import threading
import time

BASE = Path(sys.argv[1]).resolve()
PROTOCOL = sys.argv[2]
assert BASE.is_dir() and BASE.name.startswith('navkit-export-simulation-')
LOCK = threading.Lock()
MYSQL_SESSION = [1000]

def log(**value):
    with LOCK:
        with (BASE / 'wire.jsonl').open('a', encoding='utf-8') as file:
            file.write(json.dumps(value, ensure_ascii=True) + '\n')

def exact(stream, length):
    if not 0 <= length <= 16 * 1024 * 1024:
        raise ValueError('模拟帧超过上限')
    result = bytearray()
    while len(result) < length:
        part = stream.recv(length - len(result))
        if not part:
            raise EOFError()
        result.extend(part)
    return bytes(result)

def lenc(value):
    if value < 251:
        return bytes([value])
    if value <= 65535:
        return b'\xfc' + struct.pack('<H', value)
    return b'\xfd' + value.to_bytes(3, 'little')

def lstr(value):
    value = value if isinstance(value, bytes) else str(value).encode('utf-8')
    return lenc(len(value)) + value

def read_lenc(data, pos):
    value = data[pos]
    pos += 1
    if value < 251:
        return value, pos
    size = {252: 2, 253: 3, 254: 8}[value]
    return int.from_bytes(data[pos:pos+size], 'little'), pos + size

def mysql_packet(stream):
    header = exact(stream, 4)
    return header[3], exact(stream, int.from_bytes(header[:3], 'little'))

def mysql_send(stream, payload, sequence):
    stream.sendall(len(payload).to_bytes(3, 'little') + bytes([sequence % 256]) + payload)

def mysql_column(name, typ=253):
    binary = typ == 252
    flags = 128 if binary else 32 if typ == 13 else 0
    return b''.join(lstr(value) for value in ['def', 'fixture', '', '', name, name]) + b'\x0c' + struct.pack('<HIBHB', 63 if binary else 45, 16384, typ, flags, 0) + b'\0\0'

def mysql_preparable(sql):
    """真实 MySQL 8 在 COM_STMT_PREPARE 下拒绝事务/表锁语句并返回 1295。"""
    return not re.match(r'^(start\s+transaction|lock\s+tables|unlock\s+tables)\b', sql.lower().strip())

def mysql_model(sql, args):
    lower = sql.lower().strip()
    if lower.startswith(('set ', 'start transaction', 'rollback', 'commit')):
        return [], [], []
    if lower == 'select database()':
        return ['DATABASE()'], [253], [('fixture',)]
    if 'select table_type, engine from information_schema.tables' in lower:
        name = args[-1]
        return ['TABLE_TYPE', 'ENGINE'], [253,253], [('VIEW', None)] if name.endswith('_view') else [('BASE TABLE','MyISAM' if name == 'unsafe' else 'InnoDB')]
    if 'information_schema.view_table_usage' in lower:
        name = args[-1]
        if name == 'hidden_dependency_view': return ['TABLE_SCHEMA','TABLE_NAME'], [253,253], []
        return ['TABLE_SCHEMA','TABLE_NAME'], [253,253], [('fixture','base'),('fixture','unsafe')] if name == 'mixed_view' else [('fixture','base')]
    if lower.startswith('show create view '):
        name = re.findall(r'`((?:[^`]|``)*)`', sql)[-1].replace('``','`')
        if name == 'hidden_view': raise ValueError('SHOW VIEW permission denied for selected view')
        ddl = f'CREATE VIEW `fixture`.`{name}` AS select `base`.`id`, `base`.`payload`, `base`.`nullable`, `base`.`note` from `fixture`.`base`'
        if name == 'mixed_view':
            ddl += ' join `fixture`.`unsafe` on `unsafe`.`id`=`base`.`id`'
        return ['View','Create View'], [253,253], [(name,ddl)]
    if 'from information_schema.tables' in lower and 'table_name' in lower:
        return ['TABLE_NAME','TABLE_TYPE'], [253,253], [('base','BASE TABLE'),('selected_view','VIEW'),('empty_view','VIEW'),('unsafe','BASE TABLE'),('mixed_view','VIEW')]
    if re.match(r'select\s+\*\s+from\s+', lower):
        name = re.findall(r'`((?:[^`]|``)*)`', sql)[-1].replace('``','`')
        if name == 'year_values':
            return ['id','c_year'], [8,13], [(1,0),(2,1901),(3,2026),(4,2155),(5,None)]
        rows = [(9223372036854775807,b'\0\xff\x80\x27',None,'中文'),(2,b'', '', 'second')]
        if name == 'slow': rows = rows * 200
        limit = re.search(r'\blimit\s+(\d+)\s*$', lower)
        if name == 'empty_view':
            rows = []
        if limit:
            rows = rows[:int(limit.group(1))]
        return ['id','payload','nullable','note'], [8,252,253,253], rows
    raise ValueError('模拟器拒绝未实现 SQL：' + sql[:120])

def mysql_result(stream, columns, types, rows, binary, sequence=1, slow=False):
    if not columns:
        mysql_send(stream,b'\0\0\0\x02\0\0\0',sequence)
        return
    mysql_send(stream,lenc(len(columns)),sequence); sequence += 1
    for name, typ in zip(columns, types):
        mysql_send(stream,mysql_column(name,typ),sequence); sequence += 1
    mysql_send(stream,b'\xfe\0\0\x02\0',sequence); sequence += 1
    for row in rows:
        if slow: time.sleep(0.002)
        payload = bytearray()
        if binary:
            bitmap = bytearray((len(columns)+9)//8)
            for index, value in enumerate(row):
                if value is None: bitmap[(index+2)//8] |= 1 << ((index+2)%8)
            payload.extend(b'\0'+bitmap)
            for value, typ in zip(row, types):
                if value is None: continue
                payload.extend(struct.pack('<q',value) if typ == 8 else struct.pack('<H',value) if typ == 13 else lstr(value))
        else:
            for value in row: payload.extend(b'\xfb' if value is None else lstr(value))
        mysql_send(stream,payload,sequence); sequence += 1
    mysql_send(stream,b'\xfe\0\0\x02\0',sequence)

def mysql_client(stream):
    statements = {}; statement_id = 0
    with LOCK:
        MYSQL_SESSION[0] += 1
        session = MYSQL_SESSION[0]
    try:
        stream.settimeout(20)
        caps = 1 | 8 | 512 | 8192 | 32768 | 131072 | 524288
        handshake = b'\x0a8.0.36-export-simulation\0'+struct.pack('<I',session)+b'12345678\0'+struct.pack('<H',caps&65535)+b'\x2d\x02\0'+struct.pack('<H',caps>>16)+b'\x15'+b'\0'*10+b'abcdefgh1234\0mysql_native_password\0'
        mysql_send(stream,handshake,0)
        _, login = mysql_packet(stream)
        if b'fixture\0' not in login: raise ValueError('必须使用模拟身份')
        log(protocol='mysql',event='LOGIN',session=session,vendor=False)
        mysql_send(stream,b'\0\0\0\x02\0\0\0',2)
        while True:
            _, packet = mysql_packet(stream); command = packet[0]
            if command == 1:
                log(protocol='mysql',event='QUIT',session=session); return
            if command == 14:
                mysql_send(stream,b'\0\0\0\x02\0\0\0',1); continue
            if command == 25:
                statements.pop(struct.unpack_from('<I',packet,1)[0],None); continue
            if command == 22:
                sql = packet[1:].decode('utf-8')
                if not mysql_preparable(sql):
                    log(protocol='mysql',event='PREPARE_REJECTED',session=session,sql=sql)
                    mysql_send(stream,b'\xff'+struct.pack('<H',1295)+b'#HY000This command is not supported in the prepared statement protocol yet',1)
                    continue
                params = sql.count('?'); statement_id += 1
                # describe 得到真实 wire 列类型；尚未执行数据 SELECT。
                columns,types,_ = mysql_model(sql,['fixture','selected_view'][:params])
                statements[statement_id] = [sql,params,[]]
                log(protocol='mysql',event='PREPARE',session=session,sql=sql)
                mysql_send(stream,b'\0'+struct.pack('<IHHBH',statement_id,len(columns),params,0,0),1); sequence=2
                for index in range(params): mysql_send(stream,mysql_column(f'p{index}'),sequence); sequence+=1
                if params: mysql_send(stream,b'\xfe\0\0\x02\0',sequence); sequence+=1
                for name,typ in zip(columns,types): mysql_send(stream,mysql_column(name,typ),sequence); sequence+=1
                if columns: mysql_send(stream,b'\xfe\0\0\x02\0',sequence)
                continue
            if command == 23:
                sql,params,last_types=statements[struct.unpack_from('<I',packet,1)[0]]
                values=[]; pos=10
                if params:
                    bitmap=packet[pos:pos+(params+7)//8];pos+=len(bitmap)
                    new=packet[pos];pos+=1
                    if new:
                        last_types=[packet[pos+2*i] for i in range(params)]; pos+=2*params
                        statements[struct.unpack_from('<I',packet,1)[0]][2]=last_types
                    for index,typ in enumerate(last_types):
                        if bitmap[index//8] & (1<<(index%8)): values.append(None);continue
                        if typ==8: values.append(struct.unpack_from('<q',packet,pos)[0]);pos+=8
                        else:
                            length,pos=read_lenc(packet,pos); values.append(packet[pos:pos+length].decode('utf-8'));pos+=length
                log(protocol='mysql',event='EXECUTE',session=session,sql=sql,args=values)
                columns,types,rows=mysql_model(sql,values);mysql_result(stream,columns,types,rows,True,slow='`slow`' in sql);continue
            if command == 3:
                sql=packet[1:].decode('utf-8');log(protocol='mysql',event='QUERY',session=session,sql=sql)
                columns,types,rows=mysql_model(sql,[]);mysql_result(stream,columns,types,rows,False);continue
            raise ValueError(f'模拟器拒绝 MySQL 命令 {command}')
    except (EOFError,ConnectionError,socket.timeout): pass
    except Exception as error:
        log(protocol='mysql',event='ERROR',session=session,error=str(error))
        try: mysql_send(stream,b'\xff'+struct.pack('<H',1064)+b'#42000'+str(error).encode('utf-8'),1)
        except OSError: pass
    finally:
        stream.close()
        log(protocol='mysql',event='CLOSED',session=session)

def tds_server():
    source_path=Path(__file__).resolve().parents[1]/'acceptance/simulation/native/tds_server.py'
    module=ast.parse(source_path.read_text(encoding='utf-8'))
    boundary=next(i for i,node in enumerate(module.body) if isinstance(node,ast.Assign) and any(isinstance(target,ast.Name) and target.id=='listener' for target in node.targets))
    body=[node for node in module.body[:boundary] if not (isinstance(node,ast.Assign) and any(isinstance(target,ast.Name) and target.id=='base' for target in node.targets)) and not (isinstance(node,ast.Assert) and 'base.name' in ast.unparse(node.test))]
    wire={'__name__':'export_tds_simulation','__file__':str(source_path),'base':BASE}
    exec(compile(ast.Module(body=body,type_ignores=[]),str(source_path),'exec'),wire)
    states={}; counter=[100]
    data_columns=['id','payload','rv','nullable','date_value','time_value','moment','offset_value']
    data_types=['bigint','varbinary','rowversion','nvarchar','date','time','datetime2','datetimeoffset']
    def execute(conn,sql):
        key=id(conn)
        if key not in states: counter[0]+=1;states[key]={'spid':counter[0],'transaction':0,'isolation':2}
        state=states[key]; lower=sql.lower().strip(); outside=wire['outside_sql'](sql)
        if re.search(r'\blimit\b',outside,re.I): raise ValueError('TDS 模拟器严格拒绝 LIMIT')
        if 'sys.dm_exec_sessions' in lower: return ['session','transaction','isolation'],[[str(state['spid']),str(state['transaction']),str(state['isolation'])]],0
        if lower.startswith('if @@trancount'):
            state['transaction']=0;state['isolation']=2
            return ['session','transaction'],[[str(state['spid']),'0']],0
        if lower.startswith('set transaction isolation level serializable; begin transaction'):
            state['transaction']=1;state['isolation']=4;return [],[],0
        if '.sys.objects' in lower:
            if lower.startswith('select s.name, o.name, o.type'): return ['schema','name','type'],[['sales','selected_view','V'],['sales','order]detail','U']],0
            return ['type'],[['V' if "v.name" in lower or 'view' in lower else 'U']],0
        if '.sys.sql_modules' in lower:
            definition='CREATE VIEW [sales].[selected_view] AS SELECT * FROM [sales].[order]]detail]'
            return ['definition'],[[None if 'hidden_view' in lower else definition]],0
        if '.information_schema.columns' in lower:
            return ['COLUMN_NAME','DATA_TYPE','IS_NULLABLE','COLUMN_DEFAULT'],[[name,typ,'YES',None] for name,typ in zip(data_columns,data_types)],0
        if re.match(r'select\s+(?:top\s*\(\d+\)\s+)?\*\s+from\s+',lower):
            if '[catalog.with.dot].[sales].' not in lower: raise ValueError('必须逐段引用 catalog/schema/name')
            count=0 if 'empty_view' in lower else 2000 if '[slow]' in lower else 2
            top=re.match(r'select\s+top\s*\((\d+)\)',lower)
            if top: count=min(count,int(top.group(1)))
            return data_columns,[[9223372036854775807,b'\0\xff\x80\x27',b'\0\0\0\0\0\0\0\x01','中文' if '[encoding_failure]' in lower else None,0,0,0,0] for _ in range(count)],0
        raise ValueError('TDS 模拟器拒绝未实现 SQL：'+sql[:120])
    def text_result(stream,columns,rows,rpc=False,affected=0):
        if columns!=data_columns: return original_text_result(stream,columns,rows,rpc=rpc,affected=affected)
        payload=bytearray(b'\x81'+struct.pack('<H',len(columns)))
        type_bytes=[b'\x26\x08',b'\xa5\x10\0',b'\xad\x08\0',b'\xe7\x40\0'+bytes.fromhex('0904d00034'),b'\x28',b'\x29\x07',b'\x2a\x07',b'\x2b\x07']
        for name,typ in zip(columns,type_bytes): payload.extend(struct.pack('<IH',0,1)+typ+wire['bv'](name))
        days=(datetime.date(2026,11,11)-datetime.date(1,1,1)).days.to_bytes(3,'little')
        time_value=(12*3600*10_000_000+1_234_567).to_bytes(5,'little')
        offset_utc_time=(4*3600*10_000_000+1_234_567).to_bytes(5,'little')
        for row in rows:
            payload.extend(b'\xd1\x08'+struct.pack('<q',row[0]))
            for value in row[1:3]: payload.extend(struct.pack('<H',len(value))+value)
            nullable=b'\xff\xff' if row[3] is None else struct.pack('<H',len(row[3].encode('utf-16le')))+row[3].encode('utf-16le')
            payload.extend(nullable+b'\x03'+days+b'\x05'+time_value+b'\x08'+time_value+days+b'\x0a'+offset_utc_time+days+struct.pack('<h',480))
        payload.extend(wire['done'](len(rows),rpc=rpc));wire['send'](stream,bytes(payload))
    original_text_result=wire['text_result'];wire['text_result']=text_result;wire['execute']=execute
    return wire['client']

listener=socket.socket();listener.bind(('127.0.0.1',0));listener.listen()
(BASE/'ready.json').write_text(json.dumps({'port':listener.getsockname()[1],'protocol':PROTOCOL,'vendor_server_status':'NOT_RUN','simulation':True}),encoding='utf-8')
client=tds_server() if PROTOCOL=='tds' else mysql_client
while True:
    stream,_=listener.accept();threading.Thread(target=client,args=(stream,),daemon=True).start()
