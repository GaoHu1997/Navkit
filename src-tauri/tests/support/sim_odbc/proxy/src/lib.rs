//! Test-only ODBC contract simulator. Handles/result sets are the real DuckDB driver;
//! only connection setup and a documented SQL subset are intercepted. Never a vendor driver.
#![allow(non_snake_case)]
use std::{collections::HashMap, ffi::{c_void,CString}, sync::{Mutex,OnceLock}};
type H = *mut c_void;
type Ret = i16;
#[link(name="kernel32")]
extern "system" { fn LoadLibraryExW(path:*const u16,file:H,flags:u32)->H; fn GetProcAddress(module:H,name:*const u8)->H; }
static BACKEND: OnceLock<usize> = OnceLock::new();
#[derive(Default)]
struct State { connections:HashMap<usize,String>, statements:HashMap<usize,usize>, errors:HashMap<usize,(String,String)> }
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
fn state()-> &'static Mutex<State> { STATE.get_or_init(||Mutex::new(State::default())) }
unsafe fn proc(name:&str)->H {
    let module=*BACKEND.get_or_init(|| {
        // Build harness sets an absolute path. Loading is explicit, never PATH/system fallback.
        let p=std::env::var("NAVKIT_SIM_ODBC_BACKEND").expect("simulation backend absolute path");
        assert!(std::path::Path::new(&p).is_absolute());
        let wide:Vec<u16>=p.encode_utf16().chain(Some(0)).collect();
        let m=LoadLibraryExW(wide.as_ptr(),std::ptr::null_mut(),0x00001100);
        assert!(!m.is_null(),"real DuckDB ODBC backend/dependencies must load"); m as usize
    });
    let n=CString::new(name).unwrap(); let p=GetProcAddress(module as H,n.as_ptr() as *const u8);
    assert!(!p.is_null(),"backend export {name} missing"); p
}
unsafe fn text8(p:*const u8,n:i32)->String { if n<0 { std::ffi::CStr::from_ptr(p as *const i8).to_string_lossy().into_owned() } else { String::from_utf8_lossy(std::slice::from_raw_parts(p,n as usize)).into_owned() } }
unsafe fn text16(p:*const u16,n:i32)->String { let count=if n<0 { let mut i=0; while *p.add(i)!=0 {i+=1;} i } else {n as usize}; String::from_utf16_lossy(std::slice::from_raw_parts(p,count)) }
fn error(h:H,code:&str,message:&str)->Ret {state().lock().unwrap().errors.insert(h as usize,(code.into(),message.into())); -1}
fn clear(h:H) {state().lock().unwrap().errors.remove(&(h as usize));}
fn profile(h:H)->String { let s=state().lock().unwrap(); let c=s.statements.get(&(h as usize)).copied().unwrap_or(h as usize); s.connections.get(&c).cloned().unwrap_or_default() }

/// Parse brace-escaped ODBC attributes without splitting embedded semicolons or `}}`.
fn attributes(s:&str)->Result<HashMap<String,String>,String> {
 let b=s.as_bytes(); let mut i=0; let mut out=HashMap::new();
 while i<b.len() {while i<b.len()&&(b[i]==b';'||b[i].is_ascii_whitespace()){i+=1;} if i==b.len(){break;}
 let start=i; while i<b.len()&&b[i]!=b'=' {i+=1;} if i==b.len(){return Err("attribute requires =".into());}
 let key=s[start..i].trim().to_ascii_lowercase(); i+=1;
 let value=if i<b.len()&&b[i]==b'{' {i+=1;let mut v=Vec::new();let mut closed=false;while i<b.len(){if b[i]==b'}'{if i+1<b.len()&&b[i+1]==b'}'{v.push(b'}');i+=2;}else{i+=1;closed=true;break;}}else{v.push(b[i]);i+=1;}}if !closed{return Err("unterminated attribute".into());}if i<b.len()&&b[i]!=b';'{return Err("invalid brace suffix".into());}String::from_utf8(v).map_err(|_|"attribute UTF8")?} else {let start=i;while i<b.len()&&b[i]!=b';'{i+=1;}s[start..i].trim().to_string()};
 if out.insert(key,value).is_some(){return Err("duplicate attribute".into());}}
 Ok(out)
}
fn literal(s:&str)->String {format!("'{}'",s.replace('\'',"''"))}
fn sql_words(s:&str)->Result<Vec<String>,String> {
 // Strings/quoted identifiers are removed before keyword rules. Sentinels may contain LIMIT/TOP.
 let b=s.as_bytes();let mut i=0;let mut out=Vec::new();while i<b.len(){
 if b[i]==b'\''||b[i]==b'"'||b[i]==b'`'{let q=b[i];i+=1;let mut closed=false;while i<b.len(){if b[i]==q{if i+1<b.len()&&b[i+1]==q{i+=2;}else{i+=1;closed=true;break;}}else{i+=1;}}if !closed{return Err("unterminated string or quoted identifier in dialect profile".into());}}
 else if b[i].is_ascii_alphanumeric()||b[i]==b'_'{let start=i;while i<b.len()&&(b[i].is_ascii_alphanumeric()||b[i]==b'_'){i+=1;}out.push(s[start..i].to_ascii_uppercase());}
 else{i+=1;}}
 Ok(out)
}
fn hive(k:&str)->bool {matches!(k,"hive"|"inceptor"|"argodb"|"spark"|"impala"|"kyuubi")}
fn oracle(k:&str)->bool {matches!(k,"oracle"|"dameng")}
fn top(k:&str)->bool {matches!(k,"access"|"iris"|"cache")}
fn pagination_translation(s:&str)->Result<String,String> {
 let upper=s.to_ascii_uppercase();
 // Standard SQL OFFSET n ROWS FETCH NEXT m ROWS ONLY (Oracle 12c+/DB2 profile).
 if let Some(i)=upper.rfind(" OFFSET ") {let words:Vec<&str>=s[i+1..].split_whitespace().collect();if words.len()==9&&words[0].eq_ignore_ascii_case("OFFSET")&&words[2].eq_ignore_ascii_case("ROWS")&&words[3].eq_ignore_ascii_case("FETCH")&&words[4].eq_ignore_ascii_case("NEXT")&&words[6].eq_ignore_ascii_case("ROWS")&&words[7].eq_ignore_ascii_case("ONLY"){return Err("malformed standard pagination".into());}
 if words.len()==8&&words[0].eq_ignore_ascii_case("OFFSET")&&words[2].eq_ignore_ascii_case("ROWS")&&words[3].eq_ignore_ascii_case("FETCH")&&words[4].eq_ignore_ascii_case("NEXT")&&words[6].eq_ignore_ascii_case("ROWS")&&words[7].eq_ignore_ascii_case("ONLY"){let offset=words[1].parse::<u64>().map_err(|_|"offset integer")?;let limit=words[5].parse::<u64>().map_err(|_|"limit integer")?;return Ok(format!("{} LIMIT {limit} OFFSET {offset}",&s[..i]));}}
 // SELECT TOP n ... (Access/IRIS common subset), FIRST n SKIP m (Firebird), SKIP m FIRST n (Informix).
 let parts:Vec<&str>=s.split_whitespace().collect();
 if parts.len()>3&&parts[0].eq_ignore_ascii_case("SELECT")&&parts[1].eq_ignore_ascii_case("TOP"){let n=parts[2].parse::<u64>().map_err(|_|"TOP integer")?;return Ok(format!("SELECT {} LIMIT {n}",parts[3..].join(" ")));}
 if parts.len()>5&&parts[0].eq_ignore_ascii_case("SELECT")&&parts[1].eq_ignore_ascii_case("FIRST")&&parts[3].eq_ignore_ascii_case("SKIP"){let n=parts[2].parse::<u64>().map_err(|_|"FIRST integer")?;let o=parts[4].parse::<u64>().map_err(|_|"SKIP integer")?;return Ok(format!("SELECT {} LIMIT {n} OFFSET {o}",parts[5..].join(" ")));}
 if parts.len()>5&&parts[0].eq_ignore_ascii_case("SELECT")&&parts[1].eq_ignore_ascii_case("SKIP")&&parts[3].eq_ignore_ascii_case("FIRST"){let o=parts[2].parse::<u64>().map_err(|_|"SKIP integer")?;let n=parts[4].parse::<u64>().map_err(|_|"FIRST integer")?;return Ok(format!("SELECT {} LIMIT {n} OFFSET {o}",parts[5..].join(" ")));}
 Ok(s.to_string())
}
fn translate(kind:&str,s:&str)->Result<String,String> {
 if kind.is_empty(){return Err("missing simulation product profile".into());}
 let words=sql_words(s)?;let has=|k:&str| words.iter().any(|w|w==k);
 if has("NAVKIT_SIM_FORBIDDEN_VENDOR_SYNTAX"){return Err(format!("{kind}: forbidden contract syntax"));}
 if (oracle(kind)||matches!(kind,"firebird"|"informix"|"db2")||top(kind))&&has("LIMIT"){return Err(format!("{kind}: LIMIT is outside this dialect profile"));}
 if hive(kind)&&has("OFFSET"){return Err(format!("{kind}: OFFSET forbidden by Hive compatible profile"));}
 if !top(kind)&&has("TOP"){return Err(format!("{kind}: TOP forbidden by this dialect profile"));}
 if kind!="firebird"&&kind!="informix"&&has("SKIP"){return Err(format!("{kind}: SKIP forbidden by this dialect profile"));}
 let compact=s.split_whitespace().collect::<Vec<_>>().join(" ");let u=compact.to_ascii_uppercase();
 let schemas="SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('information_schema','pg_catalog') ORDER BY schema_name";
 if matches!(u.as_str(),"SHOW DATABASES"|"SHOW SCHEMAS"|"SELECT USERNAME FROM ALL_USERS ORDER BY USERNAME"|"SELECT NAME FROM V$DATABASE"|"SELECT SCHEMANAME FROM SYSCAT.SCHEMATA ORDER BY SCHEMANAME"|"SELECT NAME FROM SYSMASTER:SYSDATABASES"|"SELECT DATABASENAME FROM DBC.DATABASES ORDER BY DATABASENAME"|"SELECT SCHEMA_NAME FROM SYS.SCHEMAS ORDER BY SCHEMA_NAME"){return Ok(schemas.into());}
 if u.contains("FROM USER_TABLES")&&u.contains("FROM USER_VIEWS"){return Ok("SELECT table_name, CASE WHEN table_type='VIEW' THEN 'VIEW' ELSE 'TABLE' END FROM information_schema.tables WHERE table_schema=current_schema() ORDER BY 1".into());}
 if u.contains("FROM SYSCAT.TABLES"){return Ok("SELECT table_name,CASE WHEN table_type='VIEW' THEN 'V' ELSE 'T' END FROM information_schema.tables WHERE table_schema=current_schema() ORDER BY table_name".into());}
 if u.contains("FROM DBC.TABLES") {return Ok("SELECT table_name,CASE WHEN table_type='VIEW' THEN 'V' ELSE 'T' END FROM information_schema.tables WHERE table_schema=current_schema() ORDER BY table_name".into());}
 if u.starts_with("SHOW TABLES") {let schema=compact.split_whitespace().last().unwrap_or("main").trim_matches('`').trim_matches('"');return Ok(format!("SELECT table_name FROM information_schema.tables WHERE table_schema={} ORDER BY table_name",literal(schema)));}
 if u.contains("FROM ALL_TAB_COLUMNS") {
 let name=s.split("TABLE_NAME = '").nth(1).and_then(|x|x.split('\'').next()).ok_or("missing catalog table predicate")?;
 return Ok(format!("SELECT column_name,data_type,CASE WHEN is_nullable='YES' THEN 'Y' ELSE 'N' END,column_default,character_maximum_length FROM information_schema.columns WHERE table_schema=current_schema() AND upper(table_name)={} ORDER BY ordinal_position",literal(name)));
 }
 if u.contains("FROM ALL_CONSTRAINTS") {
 let name=s.split("TABLE_NAME = '").nth(1).and_then(|x|x.split('\'').next()).ok_or("missing constraints table predicate")?;
 return Ok(format!("SELECT column_name FROM information_schema.key_column_usage WHERE table_schema=current_schema() AND upper(table_name)={} ORDER BY ordinal_position",literal(name)));
 }
 let result=pagination_translation(s)?;
 // Convert Hive quoted identifiers only; preserve SQL literals containing backticks exactly.
 let mut out=String::new();let mut chars=result.chars().peekable();let mut string=false;
 while let Some(c)=chars.next(){if c=='\'' {out.push(c);if string&&chars.peek()==Some(&'\''){out.push(chars.next().unwrap());}else{string=!string;}}else if c=='`'&&!string {if !hive(kind){return Err(format!("{kind}: backtick identifiers outside dialect profile"));}out.push('"');}else{out.push(c);}}
 Ok(out)
}
unsafe fn raw_exec(h:H,s:&str)->Ret {let f:unsafe extern "system" fn(H,*const u8,i32)->Ret=std::mem::transmute(proc("SQLExecDirect")); let ret=f(h,s.as_ptr(),s.len() as i32); trace("abi",&format!("SQLExecDirect handle={h:p} returned={ret}"),"");ret}
fn trace(k:&str,original:&str,translated:&str) {if let Ok(path)=std::env::var("NAVKIT_SIM_ODBC_TRACE") {use std::io::Write;if let Ok(mut f)=std::fs::OpenOptions::new().create(true).append(true).open(path){let _=writeln!(f,"kind={k}\toriginal={}\ttranslated={}",original.replace(['\r','\n']," "),translated.replace(['\r','\n']," "));}}}
#[no_mangle]
pub unsafe extern "system" fn SQLAllocHandle(ty:i16,input:H,output:*mut H)->Ret {let f:unsafe extern "system" fn(i16,H,*mut H)->Ret=std::mem::transmute(proc("SQLAllocHandle"));let ret=f(ty,input,output);if ret>=0&&ty==3&&!output.is_null(){state().lock().unwrap().statements.insert(*output as usize,input as usize);}ret}
#[no_mangle]
pub unsafe extern "system" fn SQLFreeHandle(ty:i16,h:H)->Ret {let f:unsafe extern "system" fn(i16,H)->Ret=std::mem::transmute(proc("SQLFreeHandle"));let ret=f(ty,h);if ret>=0{let mut s=state().lock().unwrap();s.errors.remove(&(h as usize));s.connections.remove(&(h as usize));s.statements.remove(&(h as usize));}ret}
unsafe fn connect(h:H,input:&str)->Ret {
 clear(h);let a=match attributes(input){Ok(a)=>a,Err(e)=>return error(h,"08001",&e)};
 let Some(kind)=a.get("navkitsimkind") else{return error(h,"08001","test-only driver requires NavkitSimKind");};
 if a.get("uid").map(String::as_str)!=Some("navkit_sim")||a.get("pwd").map(String::as_str)!=Some("contract;secret}中文"){return error(h,"28000","simulation authentication rejected");}
 let Some(path)=a.get("database") else{return error(h,"08001","simulation database file missing");};
 if !std::path::Path::new(path).is_absolute(){return error(h,"08001","simulation requires unique absolute database file");}
 let cs=format!("Driver=sim_duck_backend;Database={};",if path.contains([';','{','}']){format!("{{{}}}",path.replace('}',"}}"))}else{path.clone()});
 let f:unsafe extern "system" fn(H,H,*const u8,i16,*mut u8,i16,*mut i16,u16)->Ret=std::mem::transmute(proc("SQLDriverConnect"));
 let ret=f(h,std::ptr::null_mut(),cs.as_ptr(),cs.len() as i16,std::ptr::null_mut(),0,std::ptr::null_mut(),0);
 if ret>=0{state().lock().unwrap().connections.insert(h as usize,kind.clone());}ret
}
#[no_mangle]
pub unsafe extern "system" fn SQLDriverConnect(h:H,_window:H,input:*const u8,n:i16,_out:*mut u8,_outmax:i16,outlen:*mut i16,_completion:u16)->Ret {if !outlen.is_null(){*outlen=0;}connect(h,&text8(input,n as i32))}
#[no_mangle]
pub unsafe extern "system" fn SQLDriverConnectW(h:H,_window:H,input:*const u16,n:i16,_out:*mut u16,_outmax:i16,outlen:*mut i16,_completion:u16)->Ret {if !outlen.is_null(){*outlen=0;}connect(h,&text16(input,n as i32))}
unsafe fn exec(h:H,s:&str)->Ret {clear(h);let kind=profile(h);match translate(&kind,s){Ok(t)=>{trace(&kind,s,&t);raw_exec(h,&t)},Err(e)=>error(h,"42000",&e)}}
#[no_mangle]
pub unsafe extern "system" fn SQLExecDirect(h:H,s:*const u8,n:i32)->Ret {exec(h,&text8(s,n))}
#[no_mangle]
pub unsafe extern "system" fn SQLExecDirectW(h:H,s:*const u16,n:i32)->Ret {exec(h,&text16(s,n))}
unsafe fn prepare(h:H,s:&str)->Ret {clear(h);let kind=profile(h);match translate(&kind,s){Ok(t)=>{trace(&kind,s,&t);let f:unsafe extern "system" fn(H,*const u8,i32)->Ret=std::mem::transmute(proc("SQLPrepare"));f(h,t.as_ptr(),t.len() as i32)},Err(e)=>error(h,"42000",&e)}}
#[no_mangle]
pub unsafe extern "system" fn SQLPrepare(h:H,s:*const u8,n:i32)->Ret {prepare(h,&text8(s,n))}
#[no_mangle]
pub unsafe extern "system" fn SQLPrepareW(h:H,s:*const u16,n:i32)->Ret {prepare(h,&text16(s,n))}
#[no_mangle]
pub unsafe extern "system" fn SQLGetDiagRec(ty:i16,h:H,rec:i16,code:*mut u8,native:*mut i32,msg:*mut u8,max:i16,len:*mut i16)->Ret {
 if let Some((c,m))=state().lock().unwrap().errors.get(&(h as usize)).cloned(){if rec!=1{return 100;}if !code.is_null(){std::ptr::copy_nonoverlapping(c.as_ptr(),code,5);*code.add(5)=0;}if !native.is_null(){*native=0;}if !len.is_null(){*len=m.len() as i16;}let n=m.len().min((max.max(1)-1) as usize);if !msg.is_null()&&max>0{std::ptr::copy_nonoverlapping(m.as_ptr(),msg,n);*msg.add(n)=0;}return if n<m.len(){1}else{0};}
 let f:unsafe extern "system" fn(i16,H,i16,*mut u8,*mut i32,*mut u8,i16,*mut i16)->Ret=std::mem::transmute(proc("SQLGetDiagRec"));f(ty,h,rec,code,native,msg,max,len)
}
#[no_mangle]
pub unsafe extern "system" fn SQLGetDiagRecW(ty:i16,h:H,rec:i16,code:*mut u16,native:*mut i32,msg:*mut u16,max:i16,len:*mut i16)->Ret {
 if let Some((c,m))=state().lock().unwrap().errors.get(&(h as usize)).cloned(){if rec!=1{return 100;}let cw:Vec<u16>=c.encode_utf16().chain(Some(0)).collect();if !code.is_null(){std::ptr::copy_nonoverlapping(cw.as_ptr(),code,6);}if !native.is_null(){*native=0;}let mw:Vec<u16>=m.encode_utf16().collect();if !len.is_null(){*len=mw.len() as i16;}let n=mw.len().min((max.max(1)-1) as usize);if !msg.is_null()&&max>0{std::ptr::copy_nonoverlapping(mw.as_ptr(),msg,n);*msg.add(n)=0;}return if n<mw.len(){1}else{0};}
 let f:unsafe extern "system" fn(i16,H,i16,*mut u16,*mut i32,*mut u16,i16,*mut i16)->Ret=std::mem::transmute(proc("SQLGetDiagRecW"));f(ty,h,rec,code,native,msg,max,len)
}
#[cfg(test)] mod tests {use super::*;
#[test]fn brace_auth_roundtrip(){let a=attributes("PWD={contract;secret}}中文};UID=navkit_sim").unwrap();assert_eq!(a["pwd"],"contract;secret}中文");assert!(attributes("PWD={broken").is_err());}
#[test]fn oracle_never_accepts_duck_limit(){assert!(translate("oracle","SELECT * FROM t LIMIT 2 OFFSET 1").is_err());assert_eq!(translate("oracle","SELECT * FROM t OFFSET 1 ROWS FETCH NEXT 2 ROWS ONLY").unwrap(),"SELECT * FROM t LIMIT 2 OFFSET 1");}
#[test]fn string_keyword_not_dialect_error(){assert!(translate("oracle","INSERT INTO t VALUES ('LIMIT TOP 中文')").is_ok());}
#[test]fn quoted_tokens_are_well_formed(){for s in ["SELECT \"unterminated", "SELECT 'unterminated", "SELECT `unterminated"]{assert!(translate("oracle",s).is_err());}assert!(translate("oracle","SELECT \"escaped\"\"identifier\" FROM t WHERE payload='O''Reilly'").is_ok());}
#[test]fn pagination_profiles(){assert_eq!(translate("firebird","SELECT FIRST 2 SKIP 1 * FROM t").unwrap(),"SELECT * FROM t LIMIT 2 OFFSET 1");assert_eq!(translate("informix","SELECT SKIP 1 FIRST 2 * FROM t").unwrap(),"SELECT * FROM t LIMIT 2 OFFSET 1");assert_eq!(translate("access","SELECT TOP 2 * FROM t").unwrap(),"SELECT * FROM t LIMIT 2");assert!(translate("hive","SELECT * FROM t LIMIT 1 OFFSET 1").is_err());}
}
