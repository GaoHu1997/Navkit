"""Stateful TDS 7.4 loopback server for the REAL tiberius product client.
NOT Microsoft SQL Server. SQLite owns the rows; only the documented test SQL subset
and metadata queries are translated. Invalid login/SQL/dialect must return TDS errors.
No TLS is advertised: encryption tests must reject it, never silently downgrade.
"""
import json, os, re, socket, sqlite3, struct, sys, threading
from pathlib import Path
base=Path(sys.argv[1]).resolve()
assert base.name.startswith('navkit-sim-tds-') and base.is_dir()
USER=os.environ['NAVKIT_TDS_USER']; PASSWORD=os.environ['NAVKIT_TDS_PASSWORD']; DATABASE=os.environ['NAVKIT_TDS_DATABASE']
lock=threading.Lock()
def log(**entry):
    with lock:
        with (base/'wire.jsonl').open('a',encoding='utf-8') as f: f.write(json.dumps(entry,ensure_ascii=True)+'\n')
def read_exact(s,n):
    if n<0 or n>1048576: raise ValueError('bounded TDS frame length')
    out=bytearray()
    while len(out)<n:
        b=s.recv(n-len(out))
        if not b: raise EOFError()
        out.extend(b)
    return bytes(out)
def packet(s):
    payload=bytearray(); typ=None
    while True:
        h=read_exact(s,8); t,status,size,spid,pid,window=struct.unpack('!BBHHBB',h)
        if size<8 or size>65535 or (typ is not None and t!=typ): raise ValueError('invalid TDS packet header')
        typ=t; payload.extend(read_exact(s,size-8))
        if len(payload)>1048576: raise ValueError('bounded TDS message')
        if status&1: return typ,bytes(payload)
def send(s,payload):
    chunks=[payload[i:i+4088] for i in range(0,len(payload),4088)] or [b'']
    for i,b in enumerate(chunks): s.sendall(struct.pack('!BBHHBB',4,1 if i==len(chunks)-1 else 0,len(b)+8,0,(i+1)%256,0)+b)
def u16(s): return s.encode('utf-16le')
def bv(s):
    b=u16(s); assert len(b)//2<256
    return bytes([len(b)//2])+b
def done(count=0,error=False,rpc=False): return bytes([0xfe if rpc else 0xfd])+struct.pack('<HHq',(2 if error else 0)|(16 if count else 0),0,count)
def error(s,message,number=102,rpc=False):
    b=u16(str(message)[:700]); body=struct.pack('<IBBH',number,1,14,len(b)//2)+b+bv('navkit_sim')+bv('')+struct.pack('<I',1)
    send(s,b'\xaa'+struct.pack('<H',len(body))+body+done(error=True,rpc=rpc))
def login_ack(s):
    body=b'\x01'+struct.pack('!I',0x74000004)+bv('NavKit Stateful TDS')+bytes([15,0])+struct.pack('!H',2000)
    env=b'\x01'+bv(DATABASE)+bv('')
    send(s,b'\xad'+struct.pack('<H',len(body))+body+b'\xe3'+struct.pack('<H',len(env))+env+done())
def prelogin(s):
    # VERSION + ENCRYPTION. ENCRYPT_NOT_SUP is intentional and declared in evidence.
    send(s,struct.pack('!BHHBHH',0,11,6,1,17,1)+b'\xff'+bytes([15,0,7,208,0,0])+b'\x02')
def field(data,offset,password=False):
    start,length=struct.unpack_from('<HH',data,offset); end=start+length*2
    if start<94 or end>len(data): raise ValueError('invalid LOGIN7 string bounds')
    value=data[start:end]
    if password: value=bytes((((b^0xa5)<<4)&0xf0)|((b^0xa5)>>4) for b in value)
    return value.decode('utf-16le')
def text_result(s,columns,rows,rpc=False,affected=0):
    types=[]
    for i in range(len(columns)):
        values=[r[i] for r in rows if r[i] is not None]
        types.append('int' if values and all(isinstance(v,int) for v in values) else 'float' if values and all(isinstance(v,float) for v in values) else 'text')
    payload=bytearray(b'\x81'+struct.pack('<H',len(columns)))
    for name,typ in zip(columns,types):
        payload.extend(struct.pack('<IH',0,1))
        payload.extend(b'\x26\x08' if typ=='int' else b'\x6d\x08' if typ=='float' else b'\xe7'+struct.pack('<H',8000)+bytes.fromhex('0904d00034'))
        payload.extend(bv(name))
    for row in rows:
        payload.append(0xd1)
        for value,typ in zip(row,types):
            if typ in ('int','float'):
                if value is None: payload.append(0)
                else: payload.extend(b'\x08'+(struct.pack('<q',value) if typ=='int' else struct.pack('<d',value)))
            else:
                if value is None: payload.extend(b'\xff\xff')
                else:
                    b=u16(str(value)); assert len(b)<=8000
                    payload.extend(struct.pack('<H',len(b))+b)
    payload.extend(done(len(rows) or affected,rpc=rpc)); send(s,bytes(payload))
def batch(data,rpc):
    pos=0
    if len(data)>=4:
        headers=struct.unpack_from('<I',data)[0]
        if 4<=headers<=len(data) and headers<=64: pos=headers
    if not rpc: return data[pos:].decode('utf-16le')
    n=struct.unpack_from('<H',data,pos)[0];pos+=2
    if n==65535:
        proc=struct.unpack_from('<H',data,pos)[0];pos+=2
        if proc!=10: raise ValueError('only sp_executesql RPC is supported')
    else:
        proc=data[pos:pos+2*n].decode('utf-16le');pos+=2*n
        if proc.lower()!='sp_executesql': raise ValueError('unknown RPC procedure')
    pos+=2 # option flags
    n=data[pos];pos+=1+n*2
    pos+=1 # parameter status
    typ=data[pos];pos+=1
    if typ not in (0xe7,0x63): raise ValueError(f'unsupported statement parameter type {typ:x}')
    maximum=struct.unpack_from('<H',data,pos)[0];pos+=2
    pos+=5 # collation
    if maximum==65535:
        total=struct.unpack_from('<Q',data,pos)[0];pos+=8
        if total==0xffffffffffffffff: raise ValueError('NULL statement')
        chunks=bytearray()
        while True:
            n=struct.unpack_from('<I',data,pos)[0];pos+=4
            if n==0: break
            if n>1048576 or pos+n>len(data): raise ValueError('invalid PLP bounds')
            chunks.extend(data[pos:pos+n]);pos+=n
        return bytes(chunks).decode('utf-16le')
    n=struct.unpack_from('<H',data,pos)[0];pos+=2
    return data[pos:pos+n].decode('utf-16le')
def outside_sql(sql):
    out=[];quote=None;i=0
    while i<len(sql):
        c=sql[i]
        if quote:
            out.append(' ')
            if c==quote:
                if i+1<len(sql) and sql[i+1]==quote: out.append(' ');i+=1
                else: quote=None
        elif c in "'\"[": quote=']' if c=='[' else c;out.append(' ')
        else:out.append(c)
        i+=1
    return ''.join(out)
def strip_n_prefix(sql):
    out=[];i=0;inside=False
    while i<len(sql):
        c=sql[i]
        if not inside and c in 'Nn' and i+1<len(sql) and sql[i+1]=="'" and (i==0 or not(sql[i-1].isalnum() or sql[i-1]=='_')): i+=1;continue
        out.append(c)
        if c=="'":
            if inside and i+1<len(sql) and sql[i+1]=="'":out.append("'");i+=1
            else:inside=not inside
        i+=1
    return ''.join(out)
def named_table(sql):
    match=re.search(r"TABLE_NAME\s*=\s*'((?:[^']|'')*)'",sql,re.I)
    if not match:raise ValueError('metadata requires concrete table name')
    return match.group(1).replace("''", "'")
def execute(conn,sql):
    lower=sql.lower();outside=outside_sql(sql)
    if re.search(r'\blimit\b',outside,re.I):raise ValueError('SQL Server dialect rejects LIMIT')
    if '@@version' in lower:return ['version'],[['NavKit stateful TDS 7.4 simulator; not SQL Server']],0
    if 'from sys.databases' in lower:return ['name'],[[DATABASE]],0
    if 'from sys.tables' in lower or 'from sys.views' in lower:
        typ='view' if 'from sys.views' in lower else 'table'
        rows=conn.execute('SELECT name FROM dbo.sqlite_master WHERE type=? ORDER BY name',(typ,)).fetchall()
        return ['name'],rows,0
    if 'information_schema.columns' in lower:
        table=named_table(sql);rows=conn.execute('SELECT name,type,"notnull",dflt_value FROM pragma_table_info(?,\'dbo\') ORDER BY cid',(table,)).fetchall()
        return ['COLUMN_NAME','DATA_TYPE','IS_NULLABLE','COLUMN_DEFAULT'],[[n,t,'NO' if nn else 'YES',d] for n,t,nn,d in rows],0
    if 'information_schema.table_constraints' in lower:
        table=named_table(sql);rows=conn.execute('SELECT name FROM pragma_table_info(?,\'dbo\') WHERE pk>0 ORDER BY pk',(table,)).fetchall()
        return ['COLUMN_NAME'],rows,0
    translated=strip_n_prefix(sql)
    paging=re.search(r'\s+OFFSET\s+(\d+)\s+ROWS\s+FETCH\s+NEXT\s+(\d+)\s+ROWS\s+ONLY\s*;?\s*$',translated,re.I)
    if paging:
        offset,limit=map(int,paging.groups())
        if limit==0:raise ValueError('SQL Server FETCH count must be positive')
        translated=translated[:paging.start()]+f' LIMIT {limit} OFFSET {offset}'
    top=re.match(r'\s*SELECT\s+TOP\s*\(?\s*(\d+)\s*\)?\s+',translated,re.I)
    if top:translated='SELECT '+translated[top.end():]+f' LIMIT {int(top.group(1))}'
    cur=conn.execute(translated)
    rows=cur.fetchall() if cur.description else []
    return [c[0] for c in cur.description] if cur.description else [],rows,max(cur.rowcount,0)
def client(s):
    conn=None
    try:
        s.settimeout(15)
        typ,data=packet(s)
        if typ!=0x12:raise ValueError('expected PRELOGIN')
        log(event='PRELOGIN',tls='not-supported');prelogin(s)
        typ,data=packet(s)
        if typ!=0x10:raise ValueError('expected LOGIN7')
        user,pwd,database=field(data,40),field(data,44,True),field(data,68)
        good=user==USER and pwd==PASSWORD and database in ('',DATABASE)
        log(event='LOGIN7',authenticated=good,database_valid=database in ('',DATABASE))
        if not good:error(s,'Login failed for simulation identity or database',18456);return
        login_ack(s)
        conn=sqlite3.connect(':memory:',isolation_level=None)
        conn.execute('ATTACH DATABASE ? AS dbo',(str(base/'state.sqlite3'),))
        while True:
            typ,data=packet(s)
            if typ not in (1,3):error(s,f'Unsupported TDS packet {typ}');continue
            rpc=typ==3
            try:
                sql=batch(data,rpc);log(event='SQL',packet=typ,sql=sql)
                columns,rows,affected=execute(conn,sql)
                if columns:text_result(s,columns,rows,rpc=rpc,affected=affected)
                else:send(s,done(affected,rpc=rpc))
                log(event='RESULT',row_count=len(rows),affected=affected)
            except (ValueError,sqlite3.Error,UnicodeError,struct.error,IndexError) as e:
                log(event='ERROR',error=str(e));error(s,str(e),rpc=rpc)
    except (EOFError,ConnectionError,socket.timeout):pass
    except Exception as e:log(event='SERVER_EXCEPTION',error=repr(e))
    finally:
        if conn is not None:conn.close()
        s.close()
listener=socket.socket();listener.bind(('127.0.0.1',0));listener.listen()
(base/'ready.json').write_text(json.dumps({'port':listener.getsockname()[1],'protocol':'TDS7.4','backend':'stateful SQLite','tls':False}),encoding='ascii')
while True:
    s,addr=listener.accept()
    threading.Thread(target=client,args=(s,),daemon=True).start()
