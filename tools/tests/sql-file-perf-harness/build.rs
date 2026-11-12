use std::{env, fs, path::{Path,PathBuf}};
fn section<'a>(s:&'a str,start:&str,end:&str)->&'a str { let p=s.find(start).expect(start); let q=s[p..].find(end).expect(end)+p; &s[p..q] }
fn main(){
 let root=Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
 let out=PathBuf::from(env::var_os("OUT_DIR").unwrap());
 let error=fs::read_to_string(root.join("src-tauri/src/error.rs")).unwrap();
 fs::write(out.join("error.rs"),error.lines().filter(|l|!l.starts_with("//!")).collect::<Vec<_>>().join("\n")).unwrap();
 let decoder=fs::read_to_string(root.join("src-tauri/src/transfer_stream.rs")).unwrap();
 fs::write(out.join("decoder.rs"),format!("use std::io::{{self,Read}}; use anyhow::{{bail,Result}}; const READ_CHUNK:usize=64*1024;\n{}",section(&decoder,"pub struct DecodeReader", "/// CSV 批"))).unwrap();
 let token=fs::read_to_string(root.join("src-tauri/src/manual_transaction.rs")).unwrap();
 fs::write(out.join("tokenizer.rs"),format!("use anyhow::{{bail,Result}};\n{}",section(&token,"pub(crate) fn tokenize", "fn checked_statements"))).unwrap();
 let policy=fs::read_to_string(root.join("src-tauri/src/execution_policy.rs")).unwrap();
 fs::write(out.join("policy.rs"),format!("use crate::error::{{AppError,CmdResult}};\n{}",section(&policy,"pub fn ordinary_native_batch", "pub fn readonly_batch"))).unwrap();
 // Prefer the persisted working-tree capture; never substitute git HEAD for a baseline.
 let baseline="tools/performance/baseline/sql-file-20261112";
 for (name,base) in [("before",baseline),("after","src-tauri/src")] {
  let path=root.join(base); let reader=path.join("batch_sql_reader.rs"); let batch=path.join("batch_execution.rs");
  println!("cargo:rerun-if-changed={}",reader.display()); println!("cargo:rerun-if-changed={}",batch.display());
  let s=fs::read_to_string(batch).unwrap().replace("\r\n","\n"); let r=fs::read_to_string(&reader).unwrap();
  let mut code=String::from("use super::*;\n");
  code.push_str(&format!("mod batch_sql_reader {{\n{}\n}}\nuse batch_sql_reader::{{Reader,SourceFile,Statement,Dialect}};\n",r.lines().filter(|l|!l.starts_with("//!")).collect::<Vec<_>>().join("\n")));
  code.push_str(section(&s,"fn clipped(","fn now_ms("));
  code.push_str(section(&s,"fn now_ms(","impl BatchRecord"));
  code.push_str("struct BatchRecord { entry:Arc<Entry>, progress:Mutex<Progress> }\nimpl BatchRecord {\n");
  code.push_str(section(&s,"    fn log(","    fn snapshot("));
  code.push_str("fn result(&self)->MultiResult {let p=self.progress.lock(); MultiResult {scanned:p.scanned,executed:p.executed,errors:p.errors,affected:p.affected,scan_complete:p.scan_complete,parse_error:p.parse_error.clone(),logs:p.logs.iter().cloned().collect()}}\n}\n");
  code.push_str(section(&s,"fn stop_error()","// 在组合"));
  code.push_str("fn safe_error(e:&anyhow::Error)->AppError {AppError::coded(\"BATCH_FAILED\",e.to_string())}\n");
  code.push_str(section(&s,"fn received_ack(","fn generated_row("));
  code.push_str(section(&s,"enum Scan {","#[cfg(test)]\n#[path=\"batch_execution_tests.rs\"]"));
  code.push_str("async fn no_deadline<C>(_c:&mut C,_e:&Arc<Entry>)->Result<()> {Ok(())}\nfile_worker!(file_sqlite,sqlx::SqliteConnection,Dialect::Sqlite,no_deadline);\nfile_worker!(file_mysql,sqlx::MySqlConnection,Dialect::Mysql,no_deadline);\nfile_worker!(file_pg,sqlx::PgConnection,Dialect::Postgres,no_deadline);\n");
  code.push_str("pub async fn sqlite(c:&mut sqlx::SqliteConnection,path:&str,stop:bool,entry:Arc<Entry>)->(Result<MultiResult>,MultiResult) {let r=Arc::new(BatchRecord{entry,progress:Mutex::new(Progress::default())});let result=file_sqlite(c,r.clone(),SourceFile::open(path).unwrap(),\"utf-8\".into(),stop).await; (result,r.result())}\n");
  code.push_str("pub async fn mysql(c:&mut sqlx::MySqlConnection,path:&str,stop:bool,entry:Arc<Entry>)->(Result<MultiResult>,MultiResult) {let r=Arc::new(BatchRecord{entry,progress:Mutex::new(Progress::default())});let result=file_mysql(c,r.clone(),SourceFile::open(path).unwrap(),\"utf-8\".into(),stop).await; (result,r.result())}\n");
  code.push_str("pub async fn pg(c:&mut sqlx::PgConnection,path:&str,stop:bool,entry:Arc<Entry>)->(Result<MultiResult>,MultiResult) {let r=Arc::new(BatchRecord{entry,progress:Mutex::new(Progress::default())});let result=file_pg(c,r.clone(),SourceFile::open(path).unwrap(),\"utf-8\".into(),stop).await; (result,r.result())}\n");
  code.push_str("pub fn parse(path:&str,dialect:&str)->Result<u64>{let d=match dialect{\"mysql\"=>Dialect::Mysql,\"pg\"=>Dialect::Postgres,_=>Dialect::Sqlite};let mut r=Reader::new(DecodeReader::new(SourceFile::open(path)?,\"utf-8\")?,d,None);let mut n=0;while r.next_statement()?.is_some(){n+=1;}Ok(n)}\n");
  code.push_str("pub fn audit(text:&str,dialect:&str)->String {let d=match dialect{\"mysql\"=>Dialect::Mysql,\"pg\"=>Dialect::Postgres,_=>Dialect::Sqlite};batch_sql_reader::redacted_template(text,d)}\n");
  // Rebase only the crate path for the two independently included reader versions.
  let code=code.replace("crate::batch_sql_reader::","batch_sql_reader::");
  fs::write(out.join(format!("{name}.rs")),code).unwrap();
 }
}
