//! T1/T11 原生 MySQL/PostgreSQL 表工作流。
//!
//! 本模块只消费调用者提供的 Entry 和 ordinary lease，不签发授权、不终结 Entry。
//! 元数据和业务 SQL 均在同一 owned 物理会话执行；取消控制使用独立连接并等待排空。
//! Stored.rowid 仅在原生路由中保存带版本标记的目录证据 JSON，绝不解释为 SQLite rowid。
//! 未配置 live fixture：本模块的编译/守卫测试不构成服务器兼容性验证。

use anyhow::{anyhow, bail, ensure, Result};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Column, Connection, Executor, Row};
use std::{collections::{BTreeMap, BTreeSet}, sync::Arc, time::Instant};

use crate::{
    db::Db,
    design::{self, CheckDesign, ColumnDesign, DesignReceipt, DesignStep, DesignStepResult,
        ForeignKeyDesign, IndexDesign, TableDesign, TableOptionsDesign, TriggerDesign},
    error::{AppError, CmdResult},
    execution::{self, Binding, Entry},
    manual_transaction::OrdinaryLease,
    models::{ColumnInfo, DbKind, ForeignKeyInfo, IndexInfo, MultiResult, TableSchema},
    table_objects::{ConstraintInfo, ConstraintSet, TriggerInfo},
    table_workflows::{self, BoundResult, FeatureCapability, Stored, TableCapabilities,
        TableDesignReceipt, TableDesignSnapshot, TableOperation, TableOperationReceipt, TableWork},
};

const GENERATED: i64 = 1;
const IDENTITY_ALWAYS: i64 = 2;
const IDENTITY_DEFAULT: i64 = 4;
const SERIAL: i64 = 8;
const MAX_DDL: usize = 8 * 1024 * 1024;

fn unknown(message: impl Into<String>) -> AppError {
    AppError::coded("EXECUTION_EFFECTS_UNKNOWN", message)
}
fn cancelled() -> AppError {
    AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "发送业务 SQL 前已停止，没有发送表修改")
}
fn stop(entry: &Entry) -> Result<()> {
    ensure!(!entry.stopping(), "TABLE_NATIVE_STOP_REQUESTED");
    Ok(())
}
fn same<T: Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}
fn revision<T: Serialize>(value: &T) -> String {
    let mut hash = Sha256::new();
    // 此处所有输入均为可序列化目录结构，不接受任意 Serialize 实现。
    hash.update(serde_json::to_vec(value).expect("目录证据必须可以序列化"));
    hex_digest(hash.finalize().as_slice())
}
fn hex_digest(bytes:&[u8])->String {bytes.iter().map(|b|format!("{b:02x}")).collect()}
fn pg(kind: &DbKind) -> bool { kind.is_pg_family() }
fn ident(kind: &DbKind, value: &str) -> Result<String> {
    ensure!(!value.is_empty() && !value.contains('\0'), "TABLE_IDENTIFIER_INVALID");
    ensure!(if pg(kind) { value.len() <= 63 } else { value.chars().count() <= 64 },
        "TABLE_IDENTIFIER_TOO_LONG");
    Ok(if pg(kind) { format!("\"{}\"", value.replace('"', "\"\"")) }
        else { format!("`{}`", value.replace('`', "``")) })
}
fn full(kind: &DbKind, schema: &str, table: &str) -> Result<String> {
    Ok(format!("{}.{}", ident(kind, schema)?, ident(kind, table)?))
}
fn literal(value: &str) -> Result<String> {
    ensure!(!value.contains('\0') && value.len() <= MAX_DDL, "TABLE_LITERAL_INVALID");
    // MySQL owned 会话强制 NO_BACKSLASH_ESCAPES；PG 强制 standard_conforming_strings。
    Ok(format!("'{}'", value.replace('\'', "''")))
}
fn scope(kind: &DbKind, binding: &Binding, table: &str) -> Result<()> {
    ensure!(pg(kind) || kind.is_mysql_family(), "TABLE_NATIVE_ADAPTER_UNSUPPORTED");
    full(kind, &binding.namespace, table)?;
    ensure!(!binding.owner.is_empty() && !binding.principal.is_empty()
        && !binding.connection.is_empty() && !binding.fingerprint.is_empty(), "TABLE_BINDING_REQUIRED");
    let ns = binding.namespace.to_ascii_lowercase();
    ensure!(!["pg_catalog", "information_schema", "mysql", "sys", "performance_schema"].contains(&ns.as_str())
        && !(pg(kind) && ns.starts_with("pg_")), "TABLE_SYSTEM_NAMESPACE_FORBIDDEN");
    Ok(())
}
fn step(label: impl Into<String>, sql: impl Into<String>) -> DesignStep {
    DesignStep { label: label.into(), sql: sql.into() }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NativeColumn {
    name: String,
    raw: String,
    flags: i64,
    generated: Option<String>,
    collation: Option<String>,
    storage: Option<String>,
    compression: Option<String>,
    statistics: Option<i32>,
    options: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NativeConstraint {
    name: String,
    kind: String,
    definition: String,
    expression: Option<String>,
    validated: bool,
    local: bool,
    inherited: i32,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NativeIndex {
    name: String,
    definition: String,
    keys: Vec<String>,
    unique: bool,
    primary: bool,
    constraint: Option<String>,
    constraint_kind: Option<String>,
    complex: bool,
    valid: bool,
    properties: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NativeTrigger {
    name: String,
    definition: String,
    timing: String,
    event: String,
    body: String,
    order: i64,
    mode: Option<String>,
    charset: Option<String>,
    collation: Option<String>,
    enabled: Option<String>,
    complex: bool,
    properties: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NativeSequence {
    column: String,
    schema: String,
    name: String,
    identity: bool,
    owned: bool,
    data_type: String,
    start: i64,
    increment: i64,
    min: i64,
    max: i64,
    cache: i64,
    cycle: bool,
    last: Option<i64>,
    called: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Evidence {
    format: u32,
    physical: String,
    object: String,
    columns: Vec<NativeColumn>,
    constraints: Vec<NativeConstraint>,
    indexes: Vec<NativeIndex>,
    triggers: Vec<NativeTrigger>,
    sequences: Vec<NativeSequence>,
    table_properties: serde_json::Value,
    unsupported: Vec<String>,
    copy_unsupported: Vec<String>,
    next_auto: Option<String>,
    safe_mode: Option<String>,
}
fn evidence(stored: &Stored) -> Result<Evidence> {
    let result: Evidence = serde_json::from_str(stored.rowid.as_deref()
        .ok_or_else(|| anyhow!("TABLE_NATIVE_EVIDENCE_REQUIRED"))?)?;
    ensure!(result.format == 1, "TABLE_NATIVE_EVIDENCE_VERSION");
    Ok(result)
}
fn structural_evidence(meta: &Evidence) -> Evidence {
    let mut out = meta.clone();
    out.next_auto = None;
    out.safe_mode = None;
    for sequence in &mut out.sequences { sequence.last = None; sequence.called = None; }
    out
}
fn cap(ok: bool, reason: Option<&str>) -> FeatureCapability {
    FeatureCapability { supported: ok, reason: reason.map(str::to_string) }
}
fn capabilities(kind: &DbKind, read_only: bool, complete: bool, meta: &Evidence) -> TableCapabilities {
    let general = if read_only { Some("保存的连接为只读") }
        else if !complete { Some("完整约束/触发器目录的可见权限尚未证明") }
        else if !meta.unsupported.is_empty() { Some("此表包含尚未认证的特殊依赖，详见能力限制") }
        else { None };
    let regular = general.is_none();
    let sequence_unknown = meta.sequences.iter().any(|s| !s.owned || s.last.is_none() || s.called.is_none());
    let copy_reason = general.or(sequence_unknown.then_some("无法证明原序列的 ownership 或当前 state；仅对此表拒绝完整复制"))
        .or((!meta.copy_unsupported.is_empty()).then_some("此表的特殊复制属性未认证，详见限制"));
    let mut limits = meta.unsupported.clone();
    limits.extend(meta.copy_unsupported.clone());
    if pg(kind) {
        limits.push("PostgreSQL 分区/继承/RLS/规则及特殊依赖仅展示原始目录，普通基表使用事务 DDL；fillfactor 为 10..100".into());
        if sequence_unknown { limits.push("序列当前 state 不可读或默认值引用非 owned 序列，不能声称完整复制".into()); }
    } else {
        limits.push("MySQL DDL 隐式提交，失败保留逐条已应用事实，不承诺整批回滚；复制先写数据再补外键与触发器".into());
        limits.push("修改 MySQL 表选项只接受 engine/charset/collation/row_format 白名单；不转换未修改列的 collation".into());
    }
    TableCapabilities { checks: cap(regular, general), triggers: cap(regular, general),
        options: cap(regular, general), rebuild: cap(regular, general), clear: cap(regular, general),
        copy_structure: cap(copy_reason.is_none(), copy_reason), copy_data: cap(copy_reason.is_none(), copy_reason),
        supported_options: if pg(kind) { vec!["fillfactor".into()] }
            else { ["engine", "charset", "collation", "row_format"].map(str::to_string).to_vec() },
        limitations: limits }
}

// 原生 lexer 单独处理点号、运算符、MySQL backslash 和 PG dollar quote。
// 输入校验时拒绝注释；受信任的服务器原文中保留注释的字节范围。
#[derive(Debug, Clone, Copy, PartialEq)]
enum LexKind { Word, Ident, String, Symbol, Comment }
#[derive(Debug, Clone)]
struct Lex<'a> { text: &'a str, start: usize, end: usize, kind: LexKind }
impl Lex<'_> {
    fn word(&self, value: &str) -> bool { self.kind == LexKind::Word && self.text.eq_ignore_ascii_case(value) }
    fn symbol(&self, value: &str) -> bool { self.kind == LexKind::Symbol && self.text == value }
    fn name(&self) -> Result<String> {
        match self.kind {
            LexKind::Word => Ok(self.text.into()),
            LexKind::Ident => {
                let ch = self.text.chars().next().unwrap();
                Ok(self.text[1..self.text.len()-1].replace(&format!("{ch}{ch}"), &ch.to_string()))
            }
            _ => bail!("TABLE_DDL_IDENTIFIER_EXPECTED"),
        }
    }
}
fn lex(sql: &str, backslash: bool) -> Result<Vec<Lex<'_>>> {
    ensure!(sql.len() <= MAX_DDL && !sql.contains('\0'), "TABLE_DDL_QUOTA_OR_NUL");
    let bytes = sql.as_bytes(); let mut at = 0; let mut out = Vec::new();
    while at < bytes.len() {
        if bytes[at].is_ascii_whitespace() { at += 1; continue; }
        let start = at;
        let kind;
        if bytes[at..].starts_with(b"--") || bytes[at] == b'#' {
            while at < bytes.len() && bytes[at] != b'\n' { at += 1; }
            kind = LexKind::Comment;
        } else if bytes[at..].starts_with(b"/*") {
            at += 2;
            let mut nested = 1;
            while at < bytes.len() && nested > 0 {
                if bytes[at..].starts_with(b"/*") { nested += 1; at += 2; }
                else if bytes[at..].starts_with(b"*/") { nested -= 1; at += 2; }
                else { at += 1; }
            }
            ensure!(nested == 0, "TABLE_DDL_COMMENT_UNCLOSED"); kind = LexKind::Comment;
        } else if matches!(bytes[at], b'\'' | b'"' | b'`') {
            let quote = bytes[at]; at += 1;
            loop {
                ensure!(at < bytes.len(), "TABLE_DDL_QUOTE_UNCLOSED");
                if bytes[at] == b'\\' && backslash && quote != b'`' {
                    at += 2; ensure!(at <= bytes.len(), "TABLE_DDL_ESCAPE_UNCLOSED");
                } else if bytes[at] == quote {
                    at += 1;
                    if at < bytes.len() && bytes[at] == quote { at += 1; } else { break; }
                } else { at += 1; }
            }
            kind = if quote == b'\'' { LexKind::String } else { LexKind::Ident };
        } else if bytes[at] == b'$' && {
            let mut end = at + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') { end += 1; }
            end < bytes.len() && bytes[end] == b'$'
        } {
            let mut end = at + 1;
            while bytes[end] != b'$' { end += 1; } end += 1;
            let delimiter = &sql[at..end];
            let tail = sql[end..].find(delimiter).ok_or_else(|| anyhow!("TABLE_DDL_DOLLAR_UNCLOSED"))?;
            at = end + tail + delimiter.len(); kind = LexKind::String;
        } else if bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_' || bytes[at] >= 128 || bytes[at] == b'$' {
            at += 1;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || matches!(bytes[at], b'_' | b'$') || bytes[at] >= 128) { at += 1; }
            kind = LexKind::Word;
        } else { at += 1; kind = LexKind::Symbol; }
        out.push(Lex { text: &sql[start..at], start, end: at, kind });
        ensure!(out.len() <= 262144, "TABLE_DDL_TOKEN_QUOTA");
    }
    Ok(out)
}
fn syntax(sql: &str) -> Result<Vec<Lex<'_>>> {
    let out = lex(sql, false)?;
    ensure!(!out.iter().any(|t| t.kind == LexKind::Comment), "TABLE_INPUT_SQL_COMMENT_FORBIDDEN");
    Ok(out)
}
fn meaningful(sql: &str, backslash: bool) -> Result<Vec<Lex<'_>>> {
    Ok(lex(sql, backslash)?.into_iter().filter(|t| t.kind != LexKind::Comment).collect())
}
fn balanced(tokens: &[Lex<'_>], separators: bool) -> Result<()> {
    let mut depth = 0_i64;
    for token in tokens {
        if token.symbol("(") { depth += 1; }
        if token.symbol(")") { depth -= 1; ensure!(depth >= 0, "TABLE_INPUT_PAREN_ESCAPE"); }
        ensure!(separators || !token.symbol(";"), "TABLE_INPUT_STATEMENT_ESCAPE");
    }
    ensure!(depth == 0, "TABLE_INPUT_PAREN_UNCLOSED"); Ok(())
}
fn expression(sql: &str, default: bool) -> Result<()> {
    ensure!(!sql.trim().is_empty() && sql.len() <= 65536, "TABLE_EXPRESSION_INVALID");
    let tokens = syntax(sql)?; balanced(&tokens, false)?;
    let mut depth=0;
    for (at,t) in tokens.iter().enumerate() {
        if t.symbol("("){depth+=1;}if t.symbol(")"){depth-=1;}
        ensure!(depth!=0||!t.symbol(","),"TABLE_EXPRESSION_TOP_LEVEL_COMMA");
        ensure!(!["CREATE", "ALTER", "DROP", "TRUNCATE", "GRANT", "REVOKE", "CONSTRAINT", "REFERENCES",
            "DELIMITER", "COMMIT", "ROLLBACK", "LOCK", "UNLOCK"].iter().any(|w| t.word(w)), "TABLE_EXPRESSION_DDL_ESCAPE");
        if default { ensure!(!["DEFAULT", "CHECK", "GENERATED", "PRIMARY", "UNIQUE", "COMMENT", "ON", "UPDATE", "INSERT", "DELETE", "SELECT"].iter().any(|w| t.word(w)), "TABLE_DEFAULT_CLAUSE_ESCAPE");
            ensure!(!(t.word("NOT")&&tokens.get(at+1).is_some_and(|t|t.word("NULL"))&&!at.checked_sub(1).and_then(|i|tokens.get(i)).is_some_and(|t|t.word("IS"))),"TABLE_DEFAULT_NULL_CLAUSE_ESCAPE"); }
    }
    Ok(())
}
fn canonical(sql: &str) -> Result<Vec<(u8, String)>> {
    let mut out = Vec::new();
    for t in meaningful(sql, false)? {
        out.push(match t.kind {
            LexKind::Word => (0, t.text.to_ascii_lowercase()),
            LexKind::Ident => {let name=t.name()?;if name.bytes().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||b==b'_'){(0,name)}else{(1,name)}},
            LexKind::String => (2, t.text.into()),
            _ => (3, t.text.into()),
        });
    }
    Ok(out)
}
fn strip_outer(sql: &str) -> Result<&str> {
    let mut out = sql.trim();
    loop {
        let tokens = meaningful(out, false)?;
        if !tokens.first().is_some_and(|t| t.symbol("(")) || !tokens.last().is_some_and(|t| t.symbol(")")) { break; }
        let mut depth = 0; let mut whole = true;
        for (i, t) in tokens.iter().enumerate() {
            if t.symbol("(") { depth += 1; } if t.symbol(")") { depth -= 1; }
            if depth == 0 && i + 1 < tokens.len() { whole = false; break; }
        }
        if !whole || depth != 0 { break; }
        out = out[tokens[0].end..tokens.last().unwrap().start].trim();
    }
    Ok(out)
}
fn expression_equal(a: &str, b: &str) -> bool {
    if a==b{return true;}
    let strip_casts=!a.contains("::")||!b.contains("::");
    expression_signature(a,strip_casts).ok().zip(expression_signature(b,strip_casts).ok()).is_some_and(|(a,b)|a==b)
}

// pg_get_expr/SHOW CREATE 会补足优先级括号。小型 Pratt 归一化仅覆盖可证明的常规表达式；
// CASE/特殊方言等不识别形式退回完整 token 比对，绝不删除所有括号或改变 AND/OR 语义。
#[derive(Debug,Clone,PartialEq,Serialize)]
enum Expr { Atom((u8,String)), Unary(String,Box<Expr>), Binary(String,Box<Expr>,Box<Expr>), Call(Box<Expr>,Vec<Expr>), Tuple(Vec<Expr>) }
struct ExprParser {tokens:Vec<(u8,String)>,at:usize,strip_text_casts:bool}
impl ExprParser {
    fn take(&mut self)->Result<(u8,String)> {let out=self.tokens.get(self.at).cloned().ok_or_else(||anyhow!("TABLE_EXPR_END"))?;self.at+=1;Ok(out)}
    fn is(&self,value:&str)->bool {self.tokens.get(self.at).is_some_and(|(_,v)|v==value)}
    fn primary(&mut self)->Result<Expr> {
        let first=self.take()?;
        let mut out=if first.1=="(" {
            let mut args=vec![self.parse(0)?];while self.is(","){self.at+=1;args.push(self.parse(0)?);}
            ensure!(self.is(")"),"TABLE_EXPR_PAREN");self.at+=1;
            if args.len()==1{args.remove(0)}else{Expr::Tuple(args)}
        }else if first.0==3&&["+","-","~"].contains(&first.1.as_str())||first==(0,"not".into()) {
            let precedence=if first.1=="not"{3}else{8};Expr::Unary(first.1,Box::new(self.parse(precedence)?))
        }else{ensure!(first.0!=3,"TABLE_EXPR_ATOM");Expr::Atom(first)};
        while self.is(".") {
            self.at+=1;let next=self.take()?;ensure!(next.0<=1,"TABLE_EXPR_QUALIFIER");
            out=Expr::Binary(".".into(),Box::new(out),Box::new(Expr::Atom(next)));
        }
        if self.is("(") {
            self.at+=1;let mut args=Vec::new();if !self.is(")"){args.push(self.parse(0)?);while self.is(","){self.at+=1;args.push(self.parse(0)?);}}
            ensure!(self.is(")"),"TABLE_EXPR_CALL");self.at+=1;out=Expr::Call(Box::new(out),args);
        }
        while self.is(":")&&self.tokens.get(self.at+1).is_some_and(|(_,v)|v==":") {
            self.at+=2;let ty=self.take()?;ensure!(ty.0<=1,"TABLE_EXPR_CAST_TYPE");let mut name=ty.1;
            if name=="character"&&self.is("varying"){self.at+=1;name.push_str(" varying");}
            let literal_text=matches!(&out,Expr::Atom((2,_)))&&["text","character varying","varchar"].contains(&name.as_str())&&!self.is("(")&&!self.is("[");
            if !(self.strip_text_casts&&literal_text){out=Expr::Binary("::".into(),Box::new(out),Box::new(Expr::Atom((ty.0,name))));}
        }
        Ok(out)
    }
    fn operator(&self)->Option<(String,u8,usize)> {
        let (_,first)=self.tokens.get(self.at)?;
        let next=self.tokens.get(self.at+1).map(|(_,s)|s.as_str());
        if ["<",">","!"].contains(&first.as_str())&&matches!(next,Some("="|">")){return Some((format!("{first}{}",next.unwrap()),4,2));}
        if first=="|"&&next==Some("|"){return Some(("||".into(),5,2));}
        if first=="is"&&next==Some("not"){return Some(("is not".into(),4,2));}
        if first=="not"&&matches!(next,Some("in"|"like"|"ilike")){return Some((format!("not {}",next.unwrap()),4,2));}
        let precedence=match first.as_str(){"or"=>1,"and"=>2,"="|"<"|">"|"is"|"in"|"like"|"ilike"=>4,"+"|"-"=>5,"*"|"/"|"%"=>6,"^"=>7,_=>return None};
        Some((first.clone(),precedence,1))
    }
    fn parse(&mut self,minimum:u8)->Result<Expr> {
        let mut out=self.primary()?;
        while let Some((operator,precedence,count))=self.operator(){if precedence<minimum{break;}self.at+=count;let rhs=self.parse(precedence+1)?;out=Expr::Binary(operator,Box::new(out),Box::new(rhs));}
        Ok(out)
    }
}
fn expression_signature(sql:&str,strip_text_casts:bool)->Result<String> {
    let tokens=canonical(strip_outer(sql)?)?;let mut parser=ExprParser {tokens:tokens.clone(),at:0,strip_text_casts};
    match parser.parse(0){Ok(ast) if parser.at==parser.tokens.len()=>Ok(serde_json::to_string(&ast)?),_=>Ok(serde_json::to_string(&tokens)?)}
}
fn replace_ranges(sql: &str, mut edits: Vec<(usize, usize, String)>) -> Result<String> {
    edits.sort_by_key(|e| e.0); let mut out = String::new(); let mut at = 0;
    for (start, end, replacement) in edits {
        ensure!(at <= start && start <= end && end <= sql.len(), "TABLE_DDL_REWRITE_AMBIGUOUS");
        out.push_str(&sql[at..start]); out.push_str(&replacement); at = end;
    }
    out.push_str(&sql[at..]); Ok(out)
}
fn qualified_range(tokens: &[Lex<'_>], at: usize) -> Result<(usize, usize, Vec<String>)> {
    let first = tokens.get(at).ok_or_else(|| anyhow!("TABLE_DDL_NAME_MISSING"))?;
    let mut names = vec![first.name()?]; let mut end = first.end; let mut i = at + 1;
    while tokens.get(i).is_some_and(|t| t.symbol(".")) {
        let next = tokens.get(i+1).ok_or_else(|| anyhow!("TABLE_DDL_NAME_MISSING"))?;
        names.push(next.name()?); end = next.end; i += 2;
    }
    ensure!(names.len() <= 2, "TABLE_DDL_NAME_AMBIGUOUS"); Ok((first.start, end, names))
}
fn body_parts(ddl: &str) -> Result<(String, Vec<String>, String)> {
    let tokens = meaningful(ddl, false)?;
    ensure!(tokens.first().is_some_and(|t| t.word("CREATE")) && tokens.iter().any(|t| t.word("TABLE")), "TABLE_CREATE_DEFINITION_REQUIRED");
    let open = tokens.iter().position(|t| t.symbol("(")).ok_or_else(|| anyhow!("TABLE_CREATE_BODY_UNKNOWN"))?;
    let mut depth = 1; let mut start = tokens[open].end; let mut parts = Vec::new();
    for token in &tokens[open+1..] {
        if token.symbol("(") { depth += 1; }
        if token.symbol(")") {
            depth -= 1;
            if depth == 0 { parts.push(ddl[start..token.start].trim().into()); return Ok((ddl[..tokens[open].start].trim().into(), parts, ddl[token.end..].trim().into())); }
        }
        if depth == 1 && token.symbol(",") { parts.push(ddl[start..token.start].trim().into()); start = token.end; }
    }
    bail!("TABLE_CREATE_BODY_UNCLOSED")
}
fn mysql_structural_ddl(ddl: &str) -> Result<String> {
    // 只移除 CREATE TABLE body 之后的 AUTO_INCREMENT=next，不触及列属性/注释/字符串。
    let (_, _, suffix) = body_parts(ddl)?;
    let suffix_start = ddl.len() - suffix.len();
    let tokens = meaningful(&suffix, false)?; let mut edits = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if t.word("AUTO_INCREMENT") {
            ensure!(tokens.get(i+1).is_some_and(|t| t.symbol("=")) && tokens.get(i+2).is_some_and(|t| t.text.bytes().all(|b| b.is_ascii_digit())), "TABLE_AUTO_INCREMENT_STATE_UNKNOWN");
            edits.push((suffix_start+t.start, suffix_start+tokens[i+2].end, String::new()));
        }
    }
    Ok(replace_ranges(ddl, edits)?.trim().to_string())
}
fn raw_check_expression(definition: &str) -> Result<String> {
    let tokens = meaningful(definition, false)?;
    let Some(check) = tokens.iter().position(|t| t.word("CHECK")) else { return Ok(definition.trim().into()); };
    let open = check+1; ensure!(tokens.get(open).is_some_and(|t| t.symbol("(")), "TABLE_CHECK_DEFINITION_UNKNOWN");
    let mut depth = 0;
    for t in &tokens[open..] {
        if t.symbol("(") { depth += 1; } if t.symbol(")") { depth -= 1; }
        if depth == 0 { return Ok(definition[tokens[open].end..t.start].trim().into()); }
    }
    bail!("TABLE_CHECK_DEFINITION_UNKNOWN")
}

fn trigger_header(definition: &str, is_pg: bool, backslash: bool) -> Result<(String, String, String, bool)> {
    let tokens = meaningful(definition, backslash)?;
    let trigger = tokens.iter().position(|t| t.word("TRIGGER")).ok_or_else(|| anyhow!("TABLE_TRIGGER_HEADER_UNKNOWN"))?;
    let on = tokens.iter().enumerate().skip(trigger+2).find(|(_,t)| t.word("ON")).map(|(i,_)| i).ok_or_else(|| anyhow!("TABLE_TRIGGER_PARENT_UNKNOWN"))?;
    let timing_at = tokens.iter().enumerate().take(on).skip(trigger+2)
        .find(|(_,t)| t.word("BEFORE") || t.word("AFTER") || t.word("INSTEAD")).map(|(i,_)| i)
        .ok_or_else(|| anyhow!("TABLE_TRIGGER_TIMING_UNKNOWN"))?;
    let event_at = if tokens[timing_at].word("INSTEAD") { timing_at+2 } else { timing_at+1 };
    let timing = definition[tokens[timing_at].start..tokens[event_at-1].end].to_ascii_uppercase();
    let event = definition[tokens[event_at].start..tokens[on-1].end].trim().to_ascii_uppercase();
    let body = if is_pg {
        let at = tokens.iter().position(|t| t.word("EXECUTE")).ok_or_else(|| anyhow!("TABLE_TRIGGER_FUNCTION_UNKNOWN"))?;
        definition[tokens[at].start..].trim().trim_end_matches(';').trim().to_string()
    } else {
        let at = tokens.windows(3).position(|t| t[0].word("FOR") && t[1].word("EACH") && t[2].word("ROW"))
            .ok_or_else(|| anyhow!("TABLE_TRIGGER_ROW_BODY_UNKNOWN"))? + 3;
        let at = if tokens.get(at).is_some_and(|t| t.word("FOLLOWS") || t.word("PRECEDES")) { at+2 } else { at };
        let begin = tokens.get(at).ok_or_else(|| anyhow!("TABLE_TRIGGER_BODY_UNKNOWN"))?;
        let last = tokens.iter().rposition(|t| !t.symbol(";")).unwrap();
        if begin.word("BEGIN") && tokens[last].word("END") {
            definition[begin.end..tokens[last].start].trim().trim_end_matches(';').trim().into()
        } else { definition[begin.start..tokens[last].end].trim().into() }
    };
    let complex = tokens.iter().take_while(|t| !t.word("EXECUTE")).any(|t| t.word("CONSTRAINT") || t.word("REFERENCING") || t.word("WHEN") || t.word("STATEMENT"))
        || event.contains(" OF ") || timing.starts_with("INSTEAD");
    Ok((timing, event, body, complex))
}
fn native_design(schema: &TableSchema, objects: &ConstraintSet, options: &TableOptionsDesign, meta: &Evidence, kind: &DbKind) -> Result<TableDesign> {
    let mut columns = Vec::new();
    for actual in &schema.columns {
        let mut data_type = actual.data_type.clone(); let mut unsigned = false; let mut zerofill = false;
        if !pg(kind) {
            let tokens = meaningful(&data_type, false)?;
            unsigned = tokens.iter().any(|t| t.word("UNSIGNED"));
            zerofill = tokens.iter().any(|t| t.word("ZEROFILL"));
            data_type = replace_ranges(&data_type, tokens.iter().filter(|t| t.word("UNSIGNED") || t.word("ZEROFILL"))
                .map(|t| (t.start,t.end,String::new())).collect())?.trim().to_string();
        }
        columns.push(ColumnDesign { name: actual.name.clone(), orig_name: Some(actual.name.clone()), data_type,
            length: None, scale: None, nullable: actual.nullable, default_value: actual.default.clone(),
            auto_increment: actual.auto_increment, comment: actual.comment.clone(), unsigned, zerofill });
    }
    Ok(TableDesign { schema: schema.database.clone(), table: schema.table.clone(), columns,
        primary_key: design::ordered_primary_key(schema)?, comment: schema.comment.clone(),
        indexes: Some(schema.indexes.iter().filter(|i| !i.primary).map(|i| IndexDesign { name: i.name.clone(), columns: i.columns.clone(), unique: i.unique, method: i.method.clone(), descending: Vec::new() }).collect()),
        foreign_keys: Some(schema.foreign_keys.iter().map(|f| ForeignKeyDesign { name: f.name.clone(), columns: f.columns.clone(),
            ref_table: f.ref_table.clone(), ref_schema: f.ref_schema.clone(), ref_columns: f.ref_columns.clone(),
            on_update: f.on_update.clone(), on_delete: f.on_delete.clone() }).collect()),
        checks: Some(objects.checks.iter().map(|c| Ok(CheckDesign { name: c.name.clone(), orig_name: Some(c.name.clone()),
            expression: raw_check_expression(&c.definition)?, generated_name: c.generated_name })).collect::<Result<_>>()?),
        triggers: Some(meta.triggers.iter().map(|t| TriggerDesign { name: t.name.clone(), orig_name: Some(t.name.clone()),
            timing: t.timing.clone(), event: t.event.clone(), statement: t.body.clone(), definition: Some(t.definition.clone()) }).collect()),
        options: Some(options.clone()), base_revision: None, source_fingerprint: None })
}
fn pack(schema: TableSchema, mut objects: ConstraintSet, options: TableOptionsDesign, meta: Evidence,
    kind: &DbKind, binding: &Binding, read_only: bool, ddl: String) -> Result<Stored> {
    let complete = objects.completeness == "complete";
    let mut design = native_design(&schema, &objects, &options, &meta, kind)?;
    let structural_ddl = if pg(kind) { ddl.clone() } else { mysql_structural_ddl(&ddl)? };
    let base = revision(&(canonical(&structural_ddl)?, &schema, &options, structural_evidence(&meta)));
    let rev = revision(&(&base, &meta.physical, &meta.object, &binding.fingerprint));
    design.base_revision = Some(rev.clone()); design.source_fingerprint = Some(binding.fingerprint.clone());
    // SHOW CREATE 完整原文用于读回/显示；revision 单独排除 AUTO_INCREMENT next。
    objects.table_definition = Some(ddl.clone());
    let public = TableDesignSnapshot { schema, objects, options, design, revision: rev,
        source_fingerprint: binding.fingerprint.clone(), owner_id: binding.owner.clone(),
        completeness: if complete { "complete" } else { "unknown" }.into(),
        capabilities: capabilities(kind, read_only, complete, &meta) };
    Ok(Stored { public, ddl, indexes: meta.indexes.iter().map(|i| (i.name.clone(),i.definition.clone())).collect(),
        columns: meta.columns.iter().map(|c| (c.name.clone(),c.flags)).collect(),
        rowid: Some(serde_json::to_string(&meta)?), sequence: meta.next_auto.as_deref().and_then(|v| v.parse().ok()),
        base, kind: kind.clone() })
}

async fn mysql_mode(conn: &mut sqlx::MySqlConnection) -> Result<String> {
    let current: String = crate::sql_history::query_scalar("SELECT @@SESSION.sql_mode").fetch_one(&mut *conn).await?;
    let mut modes: BTreeSet<String> = current.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
    modes.insert("NO_BACKSLASH_ESCAPES".into()); modes.insert("STRICT_ALL_TABLES".into());
    modes.insert("NO_AUTO_VALUE_ON_ZERO".into());
    let safe = modes.into_iter().collect::<Vec<_>>().join(",");
    // 绑定值在修改 sql_mode 之前也不会被反斜杠或引号解释。
    crate::sql_history::query("SET SESSION sql_mode = ?").bind(&safe).execute(&mut *conn).await?;
    Ok(safe)
}
fn grant_has_trigger(grant: &str, schema: &str, table: &str) -> Result<bool> {
    let tokens = meaningful(grant, false)?;
    if !tokens.first().is_some_and(|t| t.word("GRANT")) { return Ok(false); }
    let Some(on) = tokens.iter().position(|t| t.word("ON")) else { return Ok(false); };
    if !tokens[1..on].iter().any(|t| t.word("TRIGGER") || t.word("ALL")) { return Ok(false); }
    let mut at = on+1;
    if tokens.get(at).is_some_and(|t| t.word("TABLE")) { at += 1; }
    let Some(ns) = tokens.get(at) else { return Ok(false); };
    let Some(object) = tokens.get(at+2) else { return Ok(false); };
    if !tokens.get(at+1).is_some_and(|t| t.symbol(".")) { return Ok(false); }
    Ok((ns.symbol("*") || ns.name()? == schema) && (object.symbol("*") || object.name()? == table))
}
async fn mysql_trigger_visible(conn: &mut sqlx::MySqlConnection, schema: &str, table: &str) -> Result<bool> {
    let grants = crate::sql_history::query("SHOW GRANTS FOR CURRENT_USER()").fetch_all(&mut *conn).await?;
    for row in grants { if grant_has_trigger(&row.try_get::<String,_>(0)?, schema, table)? { return Ok(true); } }
    // MySQL 8 active roles：只把服务器返回且严格解析为 role tuple 的身份用于 USING。
    // MariaDB/旧版本不支持 CURRENT_ROLE() 时保留直接 GRANT 证据，不能当空目录 complete。
    let roles = crate::sql_history::query_scalar::<_,Option<String>>("SELECT CURRENT_ROLE()").fetch_one(&mut *conn).await;
    if let Ok(Some(roles)) = roles {
        if roles != "NONE" && !roles.is_empty() {
            let tokens = meaningful(&roles, false)?;
            let valid = !tokens.is_empty() && tokens.iter().all(|t| matches!(t.kind,LexKind::Ident|LexKind::String|LexKind::Word) || t.symbol("@") || t.symbol(","))
                && !tokens.iter().any(|t| ["SELECT","ON","TO","GRANT","USING"].iter().any(|w| t.word(w)));
            if valid {
                let rows = crate::sql_history::query(&format!("SHOW GRANTS FOR CURRENT_USER() USING {roles}")).fetch_all(&mut *conn).await;
                if let Ok(rows) = rows { for row in rows { if grant_has_trigger(&row.try_get::<String,_>(0)?, schema, table)? { return Ok(true); } } }
            }
        }
    }
    Ok(false)
}
fn mysql_column_clauses(raw: &str) -> Result<(String, BTreeMap<String,String>)> {
    let tokens = meaningful(raw, false)?; ensure!(tokens.len() >= 2, "TABLE_MYSQL_COLUMN_RAW_UNKNOWN");
    let mut depth = 0; let mut markers = Vec::<(String,usize)>::new();
    for (i,t) in tokens.iter().enumerate().skip(1) {
        if t.symbol("(") { depth += 1; } if t.symbol(")") { depth -= 1; }
        if depth != 0 { continue; }
        let marker = if t.word("NOT") && tokens.get(i+1).is_some_and(|t| t.word("NULL")) { Some("nullable") }
            else if t.word("NULL") && !tokens.get(i.wrapping_sub(1)).is_some_and(|t| t.word("NOT") || t.word("DEFAULT")) { Some("nullable") }
            else if t.word("DEFAULT") { Some("default") } else if t.word("COMMENT") { Some("comment") }
            else if t.word("AUTO_INCREMENT") { Some("auto") } else if t.word("COLLATE") { Some("collation") }
            else if t.word("CHARACTER") && tokens.get(i+1).is_some_and(|t| t.word("SET")) { Some("charset") }
            else if t.word("GENERATED") || (t.word("AS") && !markers.iter().any(|m| m.0=="generated")) { Some("generated") }
            else if t.word("ON") && tokens.get(i+1).is_some_and(|t| t.word("UPDATE")) { Some("on_update") }
            else if t.word("INVISIBLE") || t.word("VISIBLE") { Some("visibility") }
            else if t.word("COLUMN_FORMAT") { Some("column_format") } else if t.word("STORAGE") { Some("storage") }
            else if t.word("REFERENCES") { Some("references") } else if t.word("CHECK") { Some("check") }
            else { None };
        if let Some(marker) = marker { markers.push((marker.into(),t.start)); }
    }
    let type_end = markers.first().map_or(raw.len(),|m|m.1);
    let data_type = raw[tokens[1].start..type_end].trim().into(); let mut clauses = BTreeMap::new();
    for (i,(name,start)) in markers.iter().enumerate() {
        ensure!(!clauses.contains_key(name), "TABLE_MYSQL_COLUMN_ATTRIBUTE_AMBIGUOUS");
        clauses.insert(name.clone(), raw[*start..markers.get(i+1).map_or(raw.len(),|m|m.1)].trim().into());
    }
    Ok((data_type,clauses))
}
fn mysql_index(part: &str, columns: &[ColumnInfo]) -> Result<Option<NativeIndex>> {
    let tokens = meaningful(part, false)?;
    let Some(first) = tokens.first() else { return Ok(None); };
    if !["PRIMARY","UNIQUE","KEY","INDEX","FULLTEXT","SPATIAL"].iter().any(|w| first.word(w)) { return Ok(None); }
    let primary = first.word("PRIMARY"); let unique = primary || first.word("UNIQUE");
    let key_at = tokens.iter().position(|t| t.word("KEY") || t.word("INDEX")).ok_or_else(|| anyhow!("TABLE_MYSQL_INDEX_UNKNOWN"))?;
    let name = if primary { "PRIMARY".into() } else { tokens.get(key_at+1).ok_or_else(||anyhow!("TABLE_MYSQL_INDEX_NAME_UNKNOWN"))?.name()? };
    let open = tokens.iter().position(|t| t.symbol("(")).ok_or_else(||anyhow!("TABLE_MYSQL_INDEX_KEYS_UNKNOWN"))?;
    let mut depth = 1; let mut start = tokens[open].end; let mut keys = Vec::new(); let mut complex = first.word("FULLTEXT") || first.word("SPATIAL");
    let mut close = None;
    for t in &tokens[open+1..] {
        if t.symbol("(") { depth += 1; complex = true; }
        if t.symbol(")") { depth -= 1; if depth == 0 { keys.push(part[start..t.start].trim().to_string()); close=Some(t.end); break; } }
        if depth==1 && t.symbol(",") { keys.push(part[start..t.start].trim().to_string()); start=t.end; }
        if t.word("DESC") { complex=true; }
    }
    let close = close.ok_or_else(||anyhow!("TABLE_MYSQL_INDEX_KEYS_UNKNOWN"))?;
    if !part[close..].trim().is_empty() { complex=true; }
    let keys = keys.into_iter().map(|key| {
        let ts=meaningful(&key,false)?;
        if let Some(first)=ts.first() { if let Ok(name)=first.name() { if columns.iter().any(|c|c.name==name) { return Ok(name); } } }
        complex=true; Ok(key)
    }).collect::<Result<Vec<_>>>()?;
    Ok(Some(NativeIndex { name,definition:part.into(),keys,unique,primary,complex,valid:true,..Default::default() }))
}

async fn mysql_snapshot(conn: &mut sqlx::MySqlConnection, kind: &DbKind, binding: &Binding,
    table: &str, read_only: bool) -> Result<Stored> {
    scope(kind,binding,table)?; let namespace=&binding.namespace;
    let row=crate::sql_history::query(&format!("SHOW CREATE TABLE {}",full(kind,namespace,table)?)).fetch_one(&mut *conn).await?;
    let position=row.columns().iter().position(|c|c.name().eq_ignore_ascii_case("Create Table"))
        .ok_or_else(||anyhow!("TABLE_BASE_TABLE_REQUIRED: SHOW CREATE 返回视图，不能清空或复制为基表"))?;
    let ddl:String=row.try_get(position)?; let (_,parts,suffix)=body_parts(&ddl)?;
    let flags=crate::sql_history::query("SELECT TABLE_TYPE, ENGINE, TABLE_COLLATION, TABLE_COMMENT, ROW_FORMAT, CAST(CREATE_TIME AS CHAR) AS created, CAST(AUTO_INCREMENT AS CHAR) AS next_auto FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=?")
        .bind(namespace).bind(table).fetch_optional(&mut *conn).await?.ok_or_else(||anyhow!("TABLE_METADATA_NOT_FOUND"))?;
    let object_type:String=flags.try_get("TABLE_TYPE")?; ensure!(object_type=="BASE TABLE","TABLE_BASE_TABLE_REQUIRED");
    let engine:Option<String>=flags.try_get("ENGINE")?; ensure!(engine.is_some(),"TABLE_MYSQL_ENGINE_UNKNOWN");
    let collation:Option<String>=flags.try_get("TABLE_COLLATION")?;
    let charset=if let Some(collation)=&collation { Some(crate::sql_history::query_scalar::<_,String>("SELECT CHARACTER_SET_NAME FROM information_schema.COLLATIONS WHERE COLLATION_NAME=?").bind(collation).fetch_one(&mut *conn).await?) } else { None };
    let raw_columns: BTreeMap<String,String>=parts.iter().filter_map(|part| {
        let tokens=meaningful(part,false).ok()?; let first=tokens.first()?;
        if first.kind==LexKind::Ident { Some(first.name().ok().map(|n|(n,part.clone()))) } else { None }
    }).flatten().collect();
    let rows=crate::sql_history::query("SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, EXTRA, COLUMN_COMMENT, GENERATION_EXPRESSION FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION")
        .bind(namespace).bind(table).fetch_all(&mut *conn).await?;
    ensure!(!rows.is_empty() && rows.len()<=4096,"TABLE_COLUMNS_UNKNOWN_OR_QUOTA");
    let mut meta=Evidence { format:1,next_auto:flags.try_get("next_auto")?,safe_mode:Some(crate::sql_history::query_scalar("SELECT @@SESSION.sql_mode").fetch_one(&mut *conn).await?),..Default::default() };
    let physical:(String,String,String,String)=crate::sql_history::query_as("SELECT CAST(@@server_id AS CHAR), @@hostname, CAST(@@port AS CHAR), @@version").fetch_one(&mut *conn).await?;
    meta.physical=revision(&physical); meta.object=revision(&(namespace,table,flags.try_get::<Option<String>,_>("created")?));
    if meaningful(&suffix,false)?.iter().any(|t|t.word("PARTITION")) { meta.unsupported.push("MySQL 分区表的特殊依赖尚未认证，未发送修改".into()); }
    let mut columns=Vec::new();
    for row in rows {
        let name:String=row.try_get("COLUMN_NAME")?;
        let raw=raw_columns.get(&name).ok_or_else(||anyhow!("TABLE_MYSQL_COLUMN_RAW_UNKNOWN: {name}"))?.clone();
        let (_,clauses)=mysql_column_clauses(&raw)?;
        let default=clauses.get("default").map(|d|d["DEFAULT".len()..].trim().to_string());
        let extra:String=row.try_get("EXTRA")?;
        let generated:Option<String>=row.try_get("GENERATION_EXPRESSION")?;
        let generated=generated.filter(|g|!g.is_empty()); let bits=if generated.is_some(){GENERATED}else{0};
        meta.columns.push(NativeColumn {name:name.clone(),raw,flags:bits,generated,
            collation:clauses.get("collation").cloned(),..Default::default()});
        columns.push(ColumnInfo {name,data_type:row.try_get("COLUMN_TYPE")?,nullable:row.try_get::<String,_>("IS_NULLABLE")?.eq_ignore_ascii_case("YES"),
            default,primary_key:false,auto_increment:extra.to_ascii_lowercase().contains("auto_increment"),comment:row.try_get("COLUMN_COMMENT")?,length:None,precision:None,scale:None});
    }
    for part in &parts { if let Some(index)=mysql_index(part,&columns)? {meta.indexes.push(index);} }
    meta.indexes.sort_by(|a,b|a.name.cmp(&b.name));
    let indexes:Vec<IndexInfo>=meta.indexes.iter().map(|i|IndexInfo {name:i.name.clone(),columns:i.keys.clone(),unique:i.unique,primary:i.primary,method:None}).collect();
    for c in &mut columns {c.primary_key=indexes.iter().any(|i|i.primary&&i.columns.contains(&c.name));}
    let fkrows=crate::sql_history::query("SELECT k.CONSTRAINT_NAME,k.COLUMN_NAME,k.REFERENCED_TABLE_SCHEMA,k.REFERENCED_TABLE_NAME,k.REFERENCED_COLUMN_NAME,r.UPDATE_RULE,r.DELETE_RULE FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r ON r.CONSTRAINT_SCHEMA=k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME=k.CONSTRAINT_NAME AND r.TABLE_NAME=k.TABLE_NAME WHERE k.TABLE_SCHEMA=? AND k.TABLE_NAME=? AND k.REFERENCED_TABLE_NAME IS NOT NULL ORDER BY k.CONSTRAINT_NAME,k.ORDINAL_POSITION")
        .bind(namespace).bind(table).fetch_all(&mut *conn).await?;
    let mut fks=BTreeMap::<String,ForeignKeyInfo>::new();
    for row in fkrows {
        let name:String=row.try_get("CONSTRAINT_NAME")?;
        let fk=fks.entry(name.clone()).or_insert(ForeignKeyInfo {name,columns:Vec::new(),ref_table:row.try_get("REFERENCED_TABLE_NAME")?,ref_schema:row.try_get("REFERENCED_TABLE_SCHEMA")?,ref_columns:Vec::new(),on_update:row.try_get("UPDATE_RULE")?,on_delete:row.try_get("DELETE_RULE")?});
        fk.columns.push(row.try_get("COLUMN_NAME")?); fk.ref_columns.push(row.try_get("REFERENCED_COLUMN_NAME")?);
    }
    let mut checks=Vec::new(); let mut raw_check_count=0;
    for part in &parts {
        let tokens=meaningful(part,false)?;
        if tokens.first().is_some_and(|t|t.word("CONSTRAINT")) {
            let name=tokens.get(1).ok_or_else(||anyhow!("TABLE_CONSTRAINT_NAME_UNKNOWN"))?.name()?;
            if tokens.iter().any(|t|t.word("CHECK")) {
                raw_check_count+=1; meta.constraints.push(NativeConstraint {name,kind:"c".into(),definition:part.clone(),expression:Some(raw_check_expression(part)?),validated:true,local:true,..Default::default()});
            } else if tokens.iter().any(|t|t.word("FOREIGN")) {
                meta.constraints.push(NativeConstraint {name,kind:"f".into(),definition:part.clone(),validated:true,local:true,..Default::default()});
            }
        } else if tokens.first().is_some_and(|t|t.word("CHECK")) { bail!("TABLE_MYSQL_CHECK_NAME_UNKNOWN"); }
    }
    let check_rows:Vec<(String,Option<String>)>=crate::sql_history::query_as("SELECT tc.CONSTRAINT_NAME,cc.CHECK_CLAUSE FROM information_schema.TABLE_CONSTRAINTS tc JOIN information_schema.CHECK_CONSTRAINTS cc ON cc.CONSTRAINT_SCHEMA=tc.CONSTRAINT_SCHEMA AND cc.CONSTRAINT_NAME=tc.CONSTRAINT_NAME WHERE tc.TABLE_SCHEMA=? AND tc.TABLE_NAME=? AND tc.CONSTRAINT_TYPE='CHECK' ORDER BY tc.CONSTRAINT_NAME")
        .bind(namespace).bind(table).fetch_all(&mut *conn).await?;
    ensure!(raw_check_count==check_rows.len(),"TABLE_MYSQL_CHECK_CATALOG_INCOMPLETE");
    for (name,clause) in check_rows {
        let clause=clause.ok_or_else(||anyhow!("TABLE_MYSQL_CHECK_DEFINITION_UNKNOWN"))?;
        let raw=meta.constraints.iter().find(|c|c.name==name&&c.kind=="c").ok_or_else(||anyhow!("TABLE_MYSQL_CHECK_CATALOG_MISMATCH"))?;
        ensure!(expression_equal(raw.expression.as_deref().unwrap(),&clause),"TABLE_MYSQL_CHECK_CATALOG_MISMATCH");
        checks.push(ConstraintInfo {name,definition:raw.definition.clone(),generated_name:false});
    }
    ensure!(fks.len()==meta.constraints.iter().filter(|c|c.kind=="f").count(),"TABLE_MYSQL_FK_CATALOG_INCOMPLETE");
    let visible=mysql_trigger_visible(conn,namespace,table).await?;
    let trigger_rows=crate::sql_history::query("SELECT TRIGGER_NAME,ACTION_TIMING,EVENT_MANIPULATION,ACTION_ORDER,SQL_MODE,CHARACTER_SET_CLIENT,COLLATION_CONNECTION FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA=? AND EVENT_OBJECT_TABLE=? ORDER BY ACTION_TIMING,EVENT_MANIPULATION,ACTION_ORDER,TRIGGER_NAME")
        .bind(namespace).bind(table).fetch_all(&mut *conn).await?;
    ensure!(trigger_rows.len()<=4096,"TABLE_TRIGGER_QUOTA");
    for row in trigger_rows {
        let name:String=row.try_get("TRIGGER_NAME")?;
        let mode:String=row.try_get("SQL_MODE")?;
        let raw=crate::sql_history::query(&format!("SHOW CREATE TRIGGER {}",full(kind,namespace,&name)?)).fetch_one(&mut *conn).await?;
        let column=raw.columns().iter().position(|c|c.name().eq_ignore_ascii_case("SQL Original Statement"))
            .ok_or_else(||anyhow!("TABLE_MYSQL_TRIGGER_DEFINITION_UNKNOWN"))?;
        let definition:String=raw.try_get(column)?;
        let (timing,event,body,complex)=trigger_header(&definition,false,!mode.split(',').any(|m|m=="NO_BACKSLASH_ESCAPES"))?;
        ensure!(timing==row.try_get::<String,_>("ACTION_TIMING")?&&event==row.try_get::<String,_>("EVENT_MANIPULATION")?,"TABLE_MYSQL_TRIGGER_CATALOG_MISMATCH");
        meta.triggers.push(NativeTrigger {name,definition,timing,event,body,complex,
            order:row.try_get::<u64,_>("ACTION_ORDER")? as i64,mode:Some(mode),charset:row.try_get("CHARACTER_SET_CLIENT")?,collation:row.try_get("COLLATION_CONNECTION")?,..Default::default()});
    }
    let options=TableOptionsDesign {engine:engine.clone(),charset,collation:collation.clone(),row_format:flags.try_get("ROW_FORMAT")?,..Default::default()};
    meta.table_properties=serde_json::json!({"engine":engine,"collation":collation});
    let schema=TableSchema {table:table.into(),database:namespace.clone(),columns,indexes,foreign_keys:fks.into_values().collect(),comment:flags.try_get("TABLE_COMMENT")?,engine:options.engine.clone(),collation:options.collation.clone(),row_count:None};
    let objects=ConstraintSet {checks,triggers:meta.triggers.iter().map(|t|TriggerInfo {name:t.name.clone(),timing:Some(t.timing.clone()),event:Some(t.event.clone()),statement:t.definition.clone()}).collect(),
        completeness:if visible{"complete"}else{"unknown"}.into(),limitations:if visible{Vec::new()}else{vec!["MySQL 当前账户及 active roles 未证明目标表 TRIGGER 权限；可见空集合不能当作完整目录".into()]},table_definition:None};
    pack(schema,objects,options,meta,kind,binding,read_only,ddl)
}

fn pg_action(code: &str) -> Result<String> {
    Ok(match code { "a"=>"NO ACTION", "r"=>"RESTRICT", "c"=>"CASCADE", "n"=>"SET NULL", "d"=>"SET DEFAULT",
        _=>bail!("TABLE_PG_FOREIGN_KEY_ACTION_UNKNOWN") }.into())
}
async fn pg_sequence(conn: &mut sqlx::PgConnection, table_oid: i64, attnum: i16, column: &str,
    serial_name: &str, identity: bool) -> Result<NativeSequence> {
    let row=crate::sql_history::query("SELECT s.seqrelid::bigint AS oid,n.nspname,c.relname,pg_catalog.format_type(s.seqtypid,NULL) AS data_type,s.seqstart,s.seqincrement,s.seqmin,s.seqmax,s.seqcache,s.seqcycle,EXISTS(SELECT 1 FROM pg_catalog.pg_depend d WHERE d.classid='pg_catalog.pg_class'::regclass AND d.objid=s.seqrelid AND d.refclassid='pg_catalog.pg_class'::regclass AND d.refobjid=$2::bigint::oid AND d.refobjsubid=$3 AND d.deptype IN ('a','i')) AS owned,pg_catalog.has_sequence_privilege(s.seqrelid,'SELECT') AS readable FROM pg_catalog.pg_sequence s JOIN pg_catalog.pg_class c ON c.oid=s.seqrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE s.seqrelid=$1::text::regclass")
        .bind(serial_name).bind(table_oid).bind(attnum as i32).fetch_one(&mut *conn).await?;
    let mut out=NativeSequence {column:column.into(),schema:row.try_get("nspname")?,name:row.try_get("relname")?,identity,
        owned:row.try_get("owned")?,data_type:row.try_get("data_type")?,start:row.try_get("seqstart")?,increment:row.try_get("seqincrement")?,
        min:row.try_get("seqmin")?,max:row.try_get("seqmax")?,cache:row.try_get("seqcache")?,cycle:row.try_get("seqcycle")?,..Default::default()};
    if row.try_get::<bool,_>("readable")? {
        let kind=DbKind::of("postgresql");
        let state:(i64,bool)=crate::sql_history::query_as(&format!("SELECT last_value,is_called FROM {}",full(&kind,&out.schema,&out.name)?)).fetch_one(&mut *conn).await?;
        out.last=Some(state.0); out.called=Some(state.1);
    }
    Ok(out)
}
async fn pg_snapshot(conn: &mut sqlx::PgConnection, kind: &DbKind, binding: &Binding,
    table: &str, read_only: bool) -> Result<Stored> {
    scope(kind,binding,table)?; let namespace=&binding.namespace;
    let version:i32=crate::sql_history::query_scalar("SELECT current_setting('server_version_num')::integer").fetch_one(&mut *conn).await?;
    ensure!(version>=120000,"TABLE_PG_VERSION_UNSUPPORTED: 原生完整设计需要 PostgreSQL 12 或更新版本");
    let relation=crate::sql_history::query("SELECT c.oid::bigint AS oid,c.relkind::text,c.relpersistence::text,c.relreplident::text,c.relispartition,c.relrowsecurity,c.relforcerowsecurity,COALESCE(c.reloptions,ARRAY[]::text[]) AS options,pg_catalog.obj_description(c.oid,'pg_class') AS comment,am.amname,ts.spcname,c.relowner::bigint AS owner,EXISTS(SELECT 1 FROM pg_catalog.pg_inherits i WHERE i.inhrelid=c.oid OR i.inhparent=c.oid) AS inheritance,EXISTS(SELECT 1 FROM pg_catalog.pg_rewrite r WHERE r.ev_class=c.oid AND r.rulename<>'_RETURN') AS rules,EXISTS(SELECT 1 FROM pg_catalog.pg_policy p WHERE p.polrelid=c.oid) AS policies FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace LEFT JOIN pg_catalog.pg_am am ON am.oid=c.relam LEFT JOIN pg_catalog.pg_tablespace ts ON ts.oid=c.reltablespace WHERE n.nspname=$1 AND c.relname=$2")
        .bind(namespace).bind(table).fetch_optional(&mut *conn).await?.ok_or_else(||anyhow!("TABLE_NOT_FOUND"))?;
    let relkind:String=relation.try_get("relkind")?;
    ensure!(relkind=="r"||relkind=="p","TABLE_BASE_TABLE_REQUIRED: PostgreSQL 视图/物化视图不允许清空或复制为完整基表");
    let oid:i64=relation.try_get("oid")?;
    let persistence:String=relation.try_get("relpersistence")?;
    let physical:(String,String,String,String)=crate::sql_history::query_as("SELECT current_database(),COALESCE(inet_server_addr()::text,''),COALESCE(inet_server_port(),0)::text,(SELECT oid::text FROM pg_catalog.pg_database WHERE datname=current_database())").fetch_one(&mut *conn).await?;
    let mut meta=Evidence {format:1,physical:revision(&physical),object:oid.to_string(),..Default::default()};
    for (condition,message) in [
        (relkind=="p"||relation.try_get::<bool,_>("relispartition")?,"PostgreSQL 分区/分区成员未认证完整复制与设计保存"),
        (relation.try_get::<bool,_>("inheritance")?,"PostgreSQL 继承及 inherited constraint 依赖未认证"),
        (relation.try_get::<bool,_>("relrowsecurity")?||relation.try_get::<bool,_>("relforcerowsecurity")?||relation.try_get::<bool,_>("policies")?,"PostgreSQL RLS/policy 可能改变可见行，不能声称完整清空或复制"),
        (relation.try_get::<bool,_>("rules")?,"PostgreSQL rule 的执行副作用尚未认证"),
        (persistence=="t","PostgreSQL 临时表不属于持久 owned 表工作流"),
    ] {if condition{meta.unsupported.push(message.into());}}
    let reloptions:Vec<String>=relation.try_get("options")?;
    let mut options=TableOptionsDesign::default();
    for option in &reloptions {
        if let Some(value)=option.strip_prefix("fillfactor=") {
            options.fillfactor=Some(value.parse().map_err(|_|anyhow!("TABLE_PG_FILLFACTOR_UNKNOWN"))?);
        }
    }
    let am:Option<String>=relation.try_get("amname")?;
    let tablespace:Option<String>=relation.try_get("spcname")?;
    meta.table_properties=serde_json::json!({"persistence":persistence,"access_method":am,"tablespace":tablespace,
        "options":reloptions,"owner":relation.try_get::<i64,_>("owner")?,"relkind":relkind,
        "partition":relation.try_get::<bool,_>("relispartition")?,"rls":relation.try_get::<bool,_>("relrowsecurity")?,
        "force_rls":relation.try_get::<bool,_>("relforcerowsecurity")?,"replica_identity":relation.try_get::<String,_>("relreplident")?});
    if am.as_deref().is_some_and(|am|am!="heap") {meta.unsupported.push("非 heap table access method 的特殊依赖尚未认证".into());}
    let compression=if version>=140000{"a.attcompression::text"}else{"''::text"};
    let column_sql=format!("SELECT a.attnum,a.attname,pg_catalog.format_type(a.atttypid,a.atttypmod) AS data_type,a.attnotnull,a.attidentity::text,a.attgenerated::text,pg_catalog.pg_get_expr(ad.adbin,ad.adrelid,false) AS default_expr,pg_catalog.col_description(a.attrelid,a.attnum) AS comment,a.attstorage::text,{compression} AS compression,a.attstattarget,COALESCE(a.attoptions,ARRAY[]::text[]) AS options,CASE WHEN a.attcollation<>ty.typcollation THEN pg_catalog.quote_ident(cn.nspname)||'.'||pg_catalog.quote_ident(co.collname) ELSE NULL END AS collation,pg_catalog.pg_get_serial_sequence($2,a.attname) AS serial_sequence FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type ty ON ty.oid=a.atttypid LEFT JOIN pg_catalog.pg_attrdef ad ON ad.adrelid=a.attrelid AND ad.adnum=a.attnum LEFT JOIN pg_catalog.pg_collation co ON co.oid=a.attcollation LEFT JOIN pg_catalog.pg_namespace cn ON cn.oid=co.collnamespace WHERE a.attrelid=$1::bigint::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum");
    let rows=crate::sql_history::query(&column_sql).bind(oid).bind(full(kind,namespace,table)?).fetch_all(&mut *conn).await?;
    ensure!(!rows.is_empty()&&rows.len()<=4096,"TABLE_COLUMNS_UNKNOWN_OR_QUOTA");
    let mut columns=Vec::new();
    for row in rows {
        let name:String=row.try_get("attname")?; let data_type:String=row.try_get("data_type")?;
        let identity:String=row.try_get("attidentity")?; let generated:String=row.try_get("attgenerated")?;
        ensure!(["","a","d"].contains(&identity.as_str())&&["","s","v"].contains(&generated.as_str()),"TABLE_PG_COLUMN_GENERATION_UNKNOWN");
        let default:Option<String>=row.try_get("default_expr")?;
        let serial_name:Option<String>=row.try_get("serial_sequence")?;
        let has_nextval=default.as_deref().map(|d|meaningful(d,false).map(|ts|ts.iter().any(|t|t.word("nextval")))).transpose()?.unwrap_or(false);
        let mut bits=if generated.is_empty(){0}else{GENERATED};
        if identity=="a" {bits|=IDENTITY_ALWAYS;} if identity=="d" {bits|=IDENTITY_DEFAULT;}
        if identity.is_empty()&&has_nextval {bits|=SERIAL;}
        if let Some(serial_name)=serial_name {
            meta.sequences.push(pg_sequence(conn,oid,row.try_get("attnum")?,&name,&serial_name,!identity.is_empty()).await?);
        } else if has_nextval {
            meta.sequences.push(NativeSequence {column:name.clone(),owned:false,..Default::default()});
        }
        let mut raw=format!("{} {}",ident(kind,&name)?,data_type);
        let collation:Option<String>=row.try_get("collation")?;
        if let Some(value)=&collation {raw.push_str(&format!(" COLLATE {value}"));}
        if !generated.is_empty() {
            let expr=default.as_deref().ok_or_else(||anyhow!("TABLE_PG_GENERATED_EXPRESSION_UNKNOWN"))?;
            raw.push_str(&format!(" GENERATED ALWAYS AS ({expr}) {}",if generated=="s"{"STORED"}else{"VIRTUAL"}));
        } else if !identity.is_empty() {
            raw.push_str(if identity=="a"{" GENERATED ALWAYS AS IDENTITY"}else{" GENERATED BY DEFAULT AS IDENTITY"});
        } else if let Some(value)=&default {raw.push_str(&format!(" DEFAULT {value}"));}
        let nullable=!row.try_get::<bool,_>("attnotnull")?;
        if !nullable {raw.push_str(" NOT NULL");}
        meta.columns.push(NativeColumn {name:name.clone(),raw,flags:bits,generated:if generated.is_empty(){None}else{default.clone()},collation,
            storage:row.try_get("attstorage")?,compression:row.try_get("compression")?,statistics:Some(row.try_get("attstattarget")?),options:row.try_get("options")?});
        columns.push(ColumnInfo {name,data_type,nullable,default:if generated.is_empty(){default}else{None},primary_key:false,
            auto_increment:!identity.is_empty()||has_nextval,comment:row.try_get("comment")?,length:None,precision:None,scale:None});
    }
    let constraints=crate::sql_history::query("SELECT con.conname,con.contype::text,pg_catalog.pg_get_constraintdef(con.oid,false) AS definition,CASE WHEN con.contype='c' THEN pg_catalog.pg_get_expr(con.conbin,con.conrelid,false) ELSE NULL END AS expression,con.convalidated,con.conislocal,con.coninhcount,ARRAY(SELECT a.attname::text FROM unnest(con.conkey) WITH ORDINALITY k(attnum,position) JOIN pg_catalog.pg_attribute a ON a.attrelid=con.conrelid AND a.attnum=k.attnum ORDER BY k.position) AS columns,r.relname AS ref_table,n.nspname AS ref_schema,ARRAY(SELECT a.attname::text FROM unnest(con.confkey) WITH ORDINALITY k(attnum,position) JOIN pg_catalog.pg_attribute a ON a.attrelid=con.confrelid AND a.attnum=k.attnum ORDER BY k.position) AS ref_columns,con.confupdtype::text,con.confdeltype::text FROM pg_catalog.pg_constraint con LEFT JOIN pg_catalog.pg_class r ON r.oid=con.confrelid LEFT JOIN pg_catalog.pg_namespace n ON n.oid=r.relnamespace WHERE con.conrelid=$1::bigint::oid ORDER BY con.conname")
        .bind(oid).fetch_all(&mut *conn).await?;
    ensure!(constraints.len()<=4096,"TABLE_CONSTRAINT_QUOTA");
    let mut checks=Vec::new(); let mut foreign_keys=Vec::new();
    for row in constraints {
        let name:String=row.try_get("conname")?; let constraint_kind:String=row.try_get("contype")?;
        let definition:String=row.try_get("definition")?;
        let inherited:i32=row.try_get("coninhcount")?; let local:bool=row.try_get("conislocal")?;
        if inherited!=0||!local {meta.unsupported.push(format!("约束 {name} 是 inherited/nonlocal，未认证修改"));}
        if constraint_kind=="c" {checks.push(ConstraintInfo {name:name.clone(),definition:definition.clone(),generated_name:false});}
        if constraint_kind=="f" {foreign_keys.push(ForeignKeyInfo {name:name.clone(),columns:row.try_get("columns")?,ref_table:row.try_get("ref_table")?,ref_schema:row.try_get("ref_schema")?,ref_columns:row.try_get("ref_columns")?,on_update:Some(pg_action(&row.try_get::<String,_>("confupdtype")?)?),on_delete:Some(pg_action(&row.try_get::<String,_>("confdeltype")?)?)});}
        meta.constraints.push(NativeConstraint {name,kind:constraint_kind,definition,expression:row.try_get("expression")?,validated:row.try_get("convalidated")?,local,inherited});
    }
    let index_rows=crate::sql_history::query("SELECT c.relname,pg_catalog.pg_get_indexdef(i.indexrelid,0,false) AS definition,i.indisunique,i.indisprimary,i.indisvalid,i.indisready,i.indislive,con.conname,con.contype::text AS constraint_kind,ARRAY(SELECT CASE WHEN k.attnum>0 THEN a.attname::text ELSE pg_catalog.pg_get_indexdef(i.indexrelid,k.position::integer,false) END FROM unnest(i.indkey) WITH ORDINALITY k(attnum,position) LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=k.attnum WHERE k.position<=i.indnkeyatts ORDER BY k.position) AS keys,pg_catalog.jsonb_build_object('method',am.amname,'include',ARRAY(SELECT a.attname::text FROM unnest(i.indkey) WITH ORDINALITY k(attnum,position) JOIN pg_catalog.pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=k.attnum WHERE k.position>i.indnkeyatts ORDER BY k.position),'options',COALESCE(c.reloptions,ARRAY[]::text[]),'tablespace',ts.spcname,'predicate',pg_catalog.pg_get_expr(i.indpred,i.indrelid,false),'expressions',pg_catalog.pg_get_expr(i.indexprs,i.indrelid,false),'collations',i.indcollation::text,'opclasses',i.indclass::text,'order',i.indoption::text,'nulls_not_distinct',to_jsonb(i)->'indnullsnotdistinct','clustered',i.indisclustered,'replica_identity',i.indisreplident)::text AS properties FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid LEFT JOIN pg_catalog.pg_am am ON am.oid=c.relam LEFT JOIN pg_catalog.pg_tablespace ts ON ts.oid=c.reltablespace LEFT JOIN pg_catalog.pg_constraint con ON con.conindid=i.indexrelid AND con.contype IN ('p','u','x') WHERE i.indrelid=$1::bigint::oid ORDER BY c.relname")
        .bind(oid).fetch_all(&mut *conn).await?;
    ensure!(index_rows.len()<=1024,"TABLE_INDEX_QUOTA");
    let mut indexes=Vec::new();
    for row in index_rows {
        let name:String=row.try_get("relname")?; let keys:Vec<String>=row.try_get("keys")?;
        let definition:String=row.try_get("definition")?; let properties:serde_json::Value=serde_json::from_str(&row.try_get::<String,_>("properties")?)?;
        if properties["clustered"].as_bool()==Some(true)||properties["replica_identity"].as_bool()==Some(true) {
            meta.copy_unsupported.push(format!("索引 {name} 带有 CLUSTER/REPLICA IDENTITY 身份，LIKE 自动命名后的身份映射尚未认证；设计保存仍保留原始定义"));
        }
        let valid=row.try_get::<bool,_>("indisvalid")?&&row.try_get::<bool,_>("indisready")?&&row.try_get::<bool,_>("indislive")?;
        if !valid {meta.unsupported.push(format!("索引 {name} 是 invalid/not-ready，完整复制状态未认证"));}
        let primary:bool=row.try_get("indisprimary")?; let unique:bool=row.try_get("indisunique")?;
        let complex=properties["predicate"]!=serde_json::Value::Null||properties["expressions"]!=serde_json::Value::Null
            || properties["include"].as_array().is_some_and(|a|!a.is_empty())||properties["options"].as_array().is_some_and(|a|!a.is_empty())
            || properties["tablespace"]!=serde_json::Value::Null||properties["method"].as_str()!=Some("btree")
            || properties["order"].as_str().is_some_and(|v|v.split_whitespace().any(|n|n!="0"))
            || properties["nulls_not_distinct"].as_bool()==Some(true);
        indexes.push(IndexInfo {name:name.clone(),columns:keys.clone(),unique,primary,method:properties["method"].as_str().map(str::to_string)});
        meta.indexes.push(NativeIndex {name,definition,keys,unique,primary,complex,valid,constraint:row.try_get("conname")?,constraint_kind:row.try_get("constraint_kind")?,properties});
    }
    for c in &mut columns {c.primary_key=indexes.iter().any(|i|i.primary&&i.columns.contains(&c.name));}
    let trigger_rows=crate::sql_history::query("SELECT t.tgname,pg_catalog.pg_get_triggerdef(t.oid,false) AS definition,t.tgenabled::text,pg_catalog.jsonb_build_object('enabled',t.tgenabled::text,'type',t.tgtype,'args',pg_catalog.encode(t.tgargs,'hex'),'columns',t.tgattr::text,'old',t.tgoldtable,'new',t.tgnewtable,'constraint',t.tgconstraint<>0,'function',t.tgfoid::regprocedure::text)::text AS properties FROM pg_catalog.pg_trigger t WHERE t.tgrelid=$1::bigint::oid AND NOT t.tgisinternal ORDER BY t.tgname")
        .bind(oid).fetch_all(&mut *conn).await?;
    ensure!(trigger_rows.len()<=4096,"TABLE_TRIGGER_QUOTA");
    for row in trigger_rows {
        let definition:String=row.try_get("definition")?;
        let (timing,event,body,complex)=trigger_header(&definition,true,false)?;
        meta.triggers.push(NativeTrigger {name:row.try_get("tgname")?,definition,timing,event,body,complex,enabled:row.try_get("tgenabled")?,
            properties:serde_json::from_str(&row.try_get::<String,_>("properties")?)?,..Default::default()});
    }
    let schema=TableSchema {table:table.into(),database:namespace.clone(),columns,indexes,foreign_keys,comment:relation.try_get("comment")?,engine:None,collation:None,row_count:None};
    let mut definitions:Vec<String>=meta.columns.iter().map(|c|c.raw.clone()).collect();
    definitions.extend(meta.constraints.iter().filter(|c|c.kind!="n").map(|c|format!("CONSTRAINT {} {}",ident(kind,&c.name).unwrap(),c.definition)));
    let mut ddl=format!("CREATE {}TABLE {} (\n  {}\n)",if persistence=="u"{"UNLOGGED "}else{""},full(kind,namespace,table)?,definitions.join(",\n  "));
    if let Some(am)=am {ddl.push_str(&format!(" USING {}",ident(kind,&am)?));}
    if !reloptions.is_empty() {ddl.push_str(&format!(" WITH ({})",reloptions.join(", ")));}
    if let Some(ts)=tablespace {ddl.push_str(&format!(" TABLESPACE {}",ident(kind,&ts)?));}
    let objects=ConstraintSet {checks,triggers:meta.triggers.iter().map(|t|TriggerInfo {name:t.name.clone(),timing:Some(t.timing.clone()),event:Some(t.event.clone()),statement:t.definition.clone()}).collect(),completeness:"complete".into(),limitations:meta.unsupported.clone(),table_definition:None};
    pack(schema,objects,options,meta,kind,binding,read_only,ddl)
}

async fn read_mysql_owned(pool: sqlx::MySqlPool, kind: DbKind, binding: Binding, table: String, read_only: bool) -> CmdResult<Stored> {
    let mut conn=pool.acquire().await.map_err(AppError::from)?;
    conn.close_on_drop();
    let result:Result<Stored>=async {
        mysql_mode(&mut conn).await?;
        // TiDB 的 READ ONLY 只是 noop 占位（默认 1235），按服务端版本降级为普通事务。
        let (version,comment):(String,String)=crate::sql_history::query_as("SELECT @@version, @@version_comment").fetch_one(&mut *conn).await?;
        // START TRANSACTION 不在 MySQL 预处理语句白名单内，服务端返回 1295；必须走文本协议。
        (&mut *conn).execute(if crate::db::mysql_read_only_unsupported(&version,&comment){"START TRANSACTION"}else{"START TRANSACTION READ ONLY"}).await?;
        // 打开源表持有事务期 MDL，随后所有 SHOW/目录查询使用同一连接。
        crate::sql_history::query(&format!("SELECT 1 FROM {} LIMIT 0",full(&kind,&binding.namespace,&table)?)).fetch_all(&mut *conn).await?;
        mysql_snapshot(&mut conn,&kind,&binding,&table,read_only).await
    }.await;
    let cleanup=crate::sql_history::query("ROLLBACK").execute(&mut *conn).await;
    let closed=conn.close().await;
    if cleanup.is_err()||closed.is_err(){return Err(unknown("TABLE_MYSQL_METADATA_DRAIN_UNKNOWN: 读事务或物理连接销毁未确认"));}
    result.map_err(AppError::from)
}
async fn read_pg_owned(pool: sqlx::PgPool, kind: DbKind, binding: Binding, table: String, read_only: bool) -> CmdResult<Stored> {
    let mut conn=pool.acquire().await.map_err(AppError::from)?;
    conn.close_on_drop();
    let result:Result<Stored>=async {
        crate::sql_history::query("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY").execute(&mut *conn).await?;
        crate::sql_history::query("SET LOCAL search_path = pg_catalog").execute(&mut *conn).await?;
        crate::sql_history::query("SET LOCAL standard_conforming_strings = on").execute(&mut *conn).await?;
        crate::sql_history::query(&format!("LOCK TABLE {} IN ACCESS SHARE MODE",full(&kind,&binding.namespace,&table)?)).execute(&mut *conn).await?;
        pg_snapshot(&mut conn,&kind,&binding,&table,read_only).await
    }.await;
    let cleanup=crate::sql_history::query("ROLLBACK").execute(&mut *conn).await;
    let closed=conn.close().await;
    if cleanup.is_err()||closed.is_err(){return Err(unknown("TABLE_PG_METADATA_DRAIN_UNKNOWN: 读事务或物理连接销毁未确认"));}
    result.map_err(AppError::from)
}

/// IPC future 被丢弃时 JoinHandle 仅 detached；owned 任务仍持 lease 到读回/ROLLBACK/close 全部排空。
pub(crate) async fn read(db: Arc<Db>, kind: DbKind, binding: Binding, table: String,
    read_only: bool, ordinary: OrdinaryLease) -> CmdResult<Stored> {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    scope(&kind,&binding,&table).map_err(AppError::from)?;
    // 用具体连接类型和显式 Send 检查点避免 PoolConnection/OrdinaryLease 的 HRTB 推断问题。
    let lease=Arc::new(parking_lot::Mutex::new(Some(ordinary)));
    let task=crate::sql_history::spawn(async move {
        let result=match db.as_ref() {
            Db::Mysql(pool) if kind.is_mysql_family()=>read_mysql_owned(pool.clone(),kind,binding,table,read_only).await,
            Db::Postgres(pool) if kind.is_pg_family()=>read_pg_owned(pool.clone(),kind,binding,table,read_only).await,
            _=>Err(AppError::coded("TABLE_NATIVE_ADAPTER_UNSUPPORTED","完整原生表目录需要匹配的 MySQL/PostgreSQL adapter")),
        };
        lease.lock().take(); result
    });
    task.await.map_err(|e|unknown(format!("TABLE_NATIVE_METADATA_TASK_UNKNOWN: {e}")))?

    }).await
}

fn type_sql(kind: &DbKind, column: &ColumnDesign) -> Result<String> {
    ident(kind,&column.name)?;
    ensure!(!column.data_type.trim().is_empty()&&column.data_type.len()<=1024,"TABLE_COLUMN_TYPE_INVALID");
    let tokens=syntax(&column.data_type)?; balanced(&tokens,false)?;
    let mut depth=0;
    for t in &tokens {
        if t.symbol("("){depth+=1;}if t.symbol(")"){depth-=1;}
        ensure!(depth!=0||!t.symbol(","),"TABLE_COLUMN_TYPE_TOP_LEVEL_COMMA");
        ensure!(matches!(t.kind,LexKind::Word|LexKind::Ident)
            || t.symbol("(")||t.symbol(")")||t.symbol(",")||t.symbol(".")||t.symbol("[")||t.symbol("]")
            || (t.kind==LexKind::String&&!pg(kind)&&tokens.first().is_some_and(|t|t.word("ENUM")||t.word("SET"))),"TABLE_COLUMN_TYPE_ESCAPE");
        ensure!(!["CONSTRAINT","CHECK","REFERENCES","DEFAULT","PRIMARY","UNIQUE","NULL","NOT","GENERATED","AS","COLLATE","COMMENT","AUTO_INCREMENT",
            "CREATE","ALTER","DROP","TRUNCATE","GRANT","REVOKE","SELECT","INSERT","UPDATE","DELETE"].iter().any(|w|t.word(w)),"TABLE_COLUMN_TYPE_CLAUSE_ESCAPE");
    }
    let mut out=column.data_type.trim().to_string();
    if let Some(length)=column.length.as_deref().filter(|v|!v.is_empty()) {
        ensure!(length.len()<=7&&length.bytes().all(|b|b.is_ascii_digit()),"TABLE_COLUMN_LENGTH_INVALID");
        ensure!(!tokens.iter().any(|t|t.symbol("(")),"TABLE_COLUMN_LENGTH_DUPLICATED");
        out.push('('); out.push_str(length);
        if let Some(scale)=column.scale.as_deref().filter(|v|!v.is_empty()) {
            ensure!(scale.len()<=7&&scale.bytes().all(|b|b.is_ascii_digit()),"TABLE_COLUMN_SCALE_INVALID"); out.push(',');out.push_str(scale);
        }
        out.push(')');
    } else {ensure!(column.scale.as_deref().map_or(true,|v|v.is_empty()),"TABLE_COLUMN_SCALE_WITHOUT_LENGTH");}
    ensure!(!pg(kind)||(!column.unsigned&&!column.zerofill),"TABLE_COLUMN_UNSIGNED_UNSUPPORTED");
    if !pg(kind) {
        if column.unsigned&&!tokens.iter().any(|t|t.word("UNSIGNED")){out.push_str(" UNSIGNED");}
        if column.zerofill&&!tokens.iter().any(|t|t.word("ZEROFILL")){out.push_str(" ZEROFILL");}
    }
    Ok(out)
}
fn normalized_type(value: &str) -> Result<Vec<(u8,String)>> {
    let mut tokens=canonical(value)?;
    if let Some((kind,first))=tokens.first_mut() {
        if *kind==0 {
            *first=match first.as_str() {"int"|"int4"=>"integer","int8"=>"bigint","int2"=>"smallint","decimal"=>"numeric",other=>other}.to_string();
        }
    }
    if tokens.first()==Some(&(0,"varchar".into())) {tokens.splice(0..1,[(0,"character".into()),(0,"varying".into())]);}
    if tokens.first()==Some(&(0,"timestamptz".into())) {tokens[0].1="timestamp".into();tokens.extend([(0,"with".into()),(0,"time".into()),(0,"zone".into())]);}
    if tokens.first()==Some(&(0,"timestamp".into()))&&!tokens.iter().any(|(_,s)|s=="with"||s=="without") {tokens.extend([(0,"without".into()),(0,"time".into()),(0,"zone".into())]);}
    if tokens.first()==Some(&(0,"time".into()))&&!tokens.iter().any(|(_,s)|s=="with"||s=="without") {tokens.extend([(0,"without".into()),(0,"time".into()),(0,"zone".into())]);}
    Ok(tokens)
}
fn type_equal(kind: &DbKind, actual: &str, wanted: &ColumnDesign) -> Result<bool> {
    let mut desired=type_sql(kind,wanted)?;
    if pg(kind) { desired=match desired.to_ascii_lowercase().as_str(){"serial"|"serial4"=>"integer","bigserial"|"serial8"=>"bigint","smallserial"|"serial2"=>"smallint",_=>&desired}.to_string(); }
    Ok(normalized_type(actual)?==normalized_type(&desired)?)
}
fn default_equal(a: Option<&str>, b: Option<&str>) -> bool {
    let null=|v:Option<&str>|v.is_none()||v.is_some_and(|s|strip_outer(s).ok().is_some_and(|s|s.eq_ignore_ascii_case("NULL")));
    if null(a)&&null(b){return true;}
    match (a,b) { (None,None)=>true,(Some(a),Some(b))=>expression_equal(a,b),_=>false }
}
fn typed_default_equal(kind:&DbKind,data_type:&str,a:Option<&str>,b:Option<&str>)->bool {
    if default_equal(a,b){return true;}
    let Some((a,b))=a.zip(b) else{return false;};
    let numeric=["tinyint","smallint","integer","int","bigint","mediumint","numeric","decimal","float","double","real"].iter()
        .any(|name|data_type.to_ascii_lowercase().starts_with(name));
    if !pg(kind)&&numeric {
        let number=|s:&str|->Option<sqlx::types::BigDecimal>{let s=strip_outer(s).ok()?;let s=if s.starts_with('\'')&&s.ends_with('\''){&s[1..s.len()-1]}else{s};s.parse().ok()};
        return number(a).zip(number(b)).is_some_and(|(a,b)|a==b);
    }
    false
}
fn action(value: Option<&str>, kind: &DbKind) -> Result<String> {
    let out=value.unwrap_or("NO ACTION").split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
    ensure!(["NO ACTION","RESTRICT","CASCADE","SET NULL","SET DEFAULT"].contains(&out.as_str()),"TABLE_FK_ACTION_INVALID");
    ensure!(pg(kind)||out!="SET DEFAULT","TABLE_MYSQL_FK_SET_DEFAULT_UNSUPPORTED"); Ok(out)
}
fn identifier_list(kind: &DbKind, names: &[String]) -> Result<String> {
    ensure!(!names.is_empty()&&names.len()<=128,"TABLE_KEY_COLUMNS_INVALID");
    ensure!(names.iter().collect::<BTreeSet<_>>().len()==names.len(),"TABLE_KEY_COLUMN_DUPLICATED");
    Ok(names.iter().map(|n|ident(kind,n)).collect::<Result<Vec<_>>>()?.join(", "))
}
fn fk_sql(kind: &DbKind, schema: &str, table: &str, fk: &ForeignKeyDesign) -> Result<String> {
    ensure!(fk.columns.len()==fk.ref_columns.len(),"TABLE_FK_COLUMN_ARITY");
    Ok(format!("ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({}) ON UPDATE {} ON DELETE {}",
        full(kind,schema,table)?,ident(kind,&fk.name)?,identifier_list(kind,&fk.columns)?,
        full(kind,fk.ref_schema.as_deref().unwrap_or(schema),&fk.ref_table)?,identifier_list(kind,&fk.ref_columns)?,
        action(fk.on_update.as_deref(),kind)?,action(fk.on_delete.as_deref(),kind)?))
}
fn old_fk_design(f: &ForeignKeyInfo) -> ForeignKeyDesign {
    ForeignKeyDesign {name:f.name.clone(),columns:f.columns.clone(),ref_table:f.ref_table.clone(),ref_schema:f.ref_schema.clone(),ref_columns:f.ref_columns.clone(),on_update:f.on_update.clone(),on_delete:f.on_delete.clone()}
}
fn fk_equal(a: &ForeignKeyInfo, b: &ForeignKeyDesign, schema: &str, kind: &DbKind) -> Result<bool> {
    Ok(a.columns==b.columns&&a.ref_columns==b.ref_columns&&a.ref_table==b.ref_table
        &&a.ref_schema.as_deref().unwrap_or(schema)==b.ref_schema.as_deref().unwrap_or(schema)
        &&action(a.on_update.as_deref(),kind)?==action(b.on_update.as_deref(),kind)?
        &&action(a.on_delete.as_deref(),kind)?==action(b.on_delete.as_deref(),kind)?)
}
fn mysql_edit_column(meta: &NativeColumn, actual: &ColumnDesign, wanted: &ColumnDesign, kind: &DbKind) -> Result<String> {
    let (old_type,clauses)=mysql_column_clauses(&meta.raw)?;
    let type_changed=!type_equal(kind,&old_type,wanted)?;
    let default_changed=!default_equal(actual.default_value.as_deref(),wanted.default_value.as_deref());
    let auto_changed=actual.auto_increment!=wanted.auto_increment;
    if meta.flags&GENERATED!=0 {
        ensure!(!type_changed&&!default_changed&&!auto_changed&&actual.nullable==wanted.nullable,
            "TABLE_MYSQL_GENERATED_COLUMN_EDIT_UNSUPPORTED: 未修改原始生成表达式保持原文，修改生成列属性未认证");
    }
    let tokens=meaningful(&meta.raw,false)?;
    let mut edits=vec![(tokens[0].start,tokens[0].end,ident(kind,&wanted.name)?)];
    if type_changed {
        let end=clauses.values().filter_map(|c|tokens.iter().find(|t|meta.raw[t.start..].starts_with(c)).map(|t|t.start)).min().unwrap_or(meta.raw.len());
        edits.push((tokens[1].start,end,format!("{} ",type_sql(kind,wanted)?)));
    }
    let mut changes=BTreeMap::<&str,String>::new();
    if actual.nullable!=wanted.nullable {changes.insert("nullable",if wanted.nullable{""}else{"NOT NULL"}.into());}
    if default_changed {changes.insert("default",match wanted.default_value.as_deref(){Some(v)=>{expression(v,true)?;format!("DEFAULT ({v})")},None=>String::new()});}
    if auto_changed {changes.insert("auto",if wanted.auto_increment{"AUTO_INCREMENT"}else{""}.into());}
    if actual.comment.as_deref().unwrap_or("")!=wanted.comment.as_deref().unwrap_or("") {changes.insert("comment",format!("COMMENT {}",literal(wanted.comment.as_deref().unwrap_or(""))?));}
    let mut additions=Vec::new();
    for (key,value) in changes {
        if let Some(old)=clauses.get(key) {
            let start=tokens.iter().find(|t|t.kind==LexKind::Word&&meta.raw[t.start..].starts_with(old))
                .ok_or_else(||anyhow!("TABLE_MYSQL_COLUMN_REWRITE_AMBIGUOUS"))?.start;
            edits.push((start,start+old.len(),value));
        } else if !value.is_empty() {additions.push(value);}
    }
    let mut result=replace_ranges(&meta.raw,edits)?;
    for value in additions {result.push(' ');result.push_str(&value);}
    Ok(result)
}
fn new_column(kind: &DbKind, column: &ColumnDesign) -> Result<String> {
    let ty=type_sql(kind,column)?;let mut out=format!("{} {ty}",ident(kind,&column.name)?);
    let serial=pg(kind)&&["serial","serial4","serial8","bigserial","smallserial","serial2"].contains(&ty.to_ascii_lowercase().as_str());
    if column.auto_increment&&!serial {
        if pg(kind) {
            ensure!(["integer","int","int4","smallint","int2","bigint","int8"].contains(&ty.to_ascii_lowercase().as_str()),"TABLE_PG_IDENTITY_TYPE_INVALID");
            ensure!(column.default_value.is_none(),"TABLE_PG_IDENTITY_DEFAULT_CONFLICT");out.push_str(" GENERATED BY DEFAULT AS IDENTITY");
        } else {out.push_str(" AUTO_INCREMENT");}
    }
    if !column.nullable {out.push_str(" NOT NULL");}
    if let Some(default)=&column.default_value {expression(default,true)?;out.push_str(&format!(" DEFAULT ({default})"));}
    if !pg(kind) {if let Some(comment)=&column.comment {out.push_str(&format!(" COMMENT {}",literal(comment)?));}}
    Ok(out)
}
fn option_word(value: &str) -> Result<&str> {
    ensure!(!value.is_empty()&&value.len()<=64&&value.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_'),"TABLE_OPTION_IDENTIFIER_INVALID");Ok(value)
}
fn desired_options(before: &TableOptionsDesign, requested: Option<&TableOptionsDesign>, kind: &DbKind) -> Result<TableOptionsDesign> {
    let mut out=before.clone();
    if let Some(w)=requested {
        ensure!(w.strict.is_none()&&w.without_rowid.is_none(),"TABLE_NATIVE_SQLITE_OPTION_UNSUPPORTED");
        if pg(kind) {
            ensure!(w.engine.is_none()&&w.charset.is_none()&&w.collation.is_none()&&w.row_format.is_none(),"TABLE_PG_OPTION_UNSUPPORTED");
            if let Some(f)=w.fillfactor {ensure!((10..=100).contains(&f),"TABLE_PG_FILLFACTOR_INVALID");out.fillfactor=Some(f);}
        } else {
            ensure!(w.fillfactor.is_none(),"TABLE_MYSQL_FILLFACTOR_UNSUPPORTED");
            if let Some(v)=&w.engine {option_word(v)?;ensure!(["INNODB","MYISAM","MEMORY"].contains(&v.to_ascii_uppercase().as_str()),"TABLE_MYSQL_ENGINE_UNSUPPORTED");out.engine=Some(v.clone());}
            if let Some(v)=&w.charset {option_word(v)?;out.charset=Some(v.clone());}
            if let Some(v)=&w.collation {option_word(v)?;out.collation=Some(v.clone());}
            if let Some(v)=&w.row_format {option_word(v)?;ensure!(["DEFAULT","DYNAMIC","COMPRESSED","COMPACT","REDUNDANT","FIXED"].contains(&v.to_ascii_uppercase().as_str()),"TABLE_MYSQL_ROW_FORMAT_UNSUPPORTED");out.row_format=Some(v.to_ascii_uppercase());}
        }
    }
    Ok(out)
}
fn option_steps(before: &Stored, wanted: Option<&TableOptionsDesign>) -> Result<Vec<DesignStep>> {
    let kind=&before.kind;let out=desired_options(&before.public.options,wanted,kind)?;
    let mut steps=Vec::new();let target=full(kind,&before.public.schema.database,&before.public.schema.table)?;
    if pg(kind) {
        if out.fillfactor!=before.public.options.fillfactor {steps.push(step("修改 fillfactor",format!("ALTER TABLE {target} SET (fillfactor = {})",out.fillfactor.unwrap())));}
    } else {
        let mut pieces=Vec::new();
        for (key,value,old) in [("ENGINE",&out.engine,&before.public.options.engine),("DEFAULT CHARACTER SET",&out.charset,&before.public.options.charset),
            ("COLLATE",&out.collation,&before.public.options.collation),("ROW_FORMAT",&out.row_format,&before.public.options.row_format)] {
            if value.as_ref().map(|s|s.to_ascii_lowercase())!=old.as_ref().map(|s|s.to_ascii_lowercase()) {
                let value=value.as_deref().ok_or_else(||anyhow!("TABLE_MYSQL_OPTION_CLEAR_UNSUPPORTED"))?;
                option_word(value)?;pieces.push(format!("{key} = {}",ident(kind,value)?));
            }
        }
        if !pieces.is_empty(){steps.push(step("修改表选项",format!("ALTER TABLE {target} {}",pieces.join(", "))));}
    }
    Ok(steps)
}

fn trigger_same(old: &NativeTrigger, wanted: &TriggerDesign) -> Result<bool> {
    if let Some(definition)=&wanted.definition {ensure!(definition==&old.definition,"TABLE_TRIGGER_DEFINITION_READ_ONLY: definition 只能保留服务器原文");}
    Ok(wanted.name==old.name&&wanted.orig_name.as_deref()==Some(old.name.as_str())
        &&wanted.timing.trim().eq_ignore_ascii_case(&old.timing)&&wanted.event.split_whitespace().collect::<Vec<_>>().join(" ").eq_ignore_ascii_case(&old.event)
        &&(wanted.statement==old.body||canonical(&wanted.statement)?==canonical(&old.body)?))
}
fn mysql_body(body: &str) -> Result<String> {
    ensure!(!body.trim().is_empty()&&body.len()<=1024*1024,"TABLE_TRIGGER_BODY_INVALID");
    let tokens=syntax(body)?; balanced(&tokens,true)?;
    // 单 trigger 的包裹边界：所有 END 必须关闭 body 内的块，不能关闭后端生成的外层 BEGIN。
    let mut stack=Vec::<&str>::new(); let mut at=0;
    while at<tokens.len() {
        let t=&tokens[at];
        ensure!(!["CREATE","ALTER","DROP","TRUNCATE","TRIGGER","PROCEDURE","FUNCTION","DELIMITER","PREPARE","EXECUTE","COMMIT","ROLLBACK","LOCK","UNLOCK","GRANT","REVOKE"].iter().any(|w|t.word(w)),"TABLE_MYSQL_TRIGGER_BODY_ESCAPE");
        if t.word("BEGIN") {stack.push("BEGIN");}
        else if t.word("CASE") {stack.push("CASE");}
        else if t.word("LOOP")||t.word("REPEAT")||t.word("WHILE") {stack.push(if t.word("LOOP"){"LOOP"}else if t.word("REPEAT"){"REPEAT"}else{"WHILE"});}
        else if t.word("IF") {
            let previous=at.checked_sub(1).and_then(|i|tokens.get(i));
            let starts_statement=at==0||previous.is_some_and(|p|p.symbol(";")||p.word("BEGIN")||p.word("THEN")||p.word("ELSE"));
            if starts_statement {stack.push("IF");}
        } else if t.word("END") {
            let opened=stack.pop().ok_or_else(||anyhow!("TABLE_MYSQL_TRIGGER_OUTER_END_ESCAPE"))?;
            if opened!="BEGIN" {
                ensure!(tokens.get(at+1).is_some_and(|t|t.word(opened))||opened=="CASE","TABLE_MYSQL_TRIGGER_BLOCK_END_MISMATCH");
                if tokens.get(at+1).is_some_and(|t|t.word(opened)){at+=1;}
            }
        }
        at+=1;
    }
    ensure!(stack.is_empty(),"TABLE_MYSQL_TRIGGER_BLOCK_UNCLOSED");
    Ok(body.trim().trim_end_matches(';').trim().to_string())
}
fn pg_function_call(statement: &str, kind: &DbKind, namespace: &str) -> Result<String> {
    let tokens=syntax(statement)?; balanced(&tokens,false)?;
    ensure!(tokens.len()>=5&&tokens[0].word("EXECUTE")&&(tokens[1].word("FUNCTION")||tokens[1].word("PROCEDURE")),"TABLE_PG_TRIGGER_EXECUTE_FUNCTION_REQUIRED");
    let (_,end,names)=qualified_range(&tokens,2)?;
    let open=tokens.iter().position(|t|t.start>=end&&t.symbol("(")).ok_or_else(||anyhow!("TABLE_PG_TRIGGER_FUNCTION_ARGS_REQUIRED"))?;
    ensure!(tokens[open].start==tokens.iter().find(|t|t.start>=end).unwrap().start&&tokens.last().is_some_and(|t|t.symbol(")")),"TABLE_PG_TRIGGER_FUNCTION_ESCAPE");
    let args=&tokens[open+1..tokens.len()-1]; let mut values=Vec::new();
    for (i,arg) in args.iter().enumerate() {
        if i%2==1 {ensure!(arg.symbol(","),"TABLE_PG_TRIGGER_ARGUMENT_ESCAPE");continue;}
        ensure!(arg.kind==LexKind::String&&arg.text.starts_with('\''),"TABLE_PG_TRIGGER_ARGUMENT_LITERAL_REQUIRED");
        values.push(literal(&arg.text[1..arg.text.len()-1].replace("''","'"))?);
    }
    ensure!(args.is_empty()||args.len()%2==1,"TABLE_PG_TRIGGER_ARGUMENT_EMPTY");
    let (schema,function)=if names.len()==2{(names[0].as_str(),names[1].as_str())}else{(namespace,names[0].as_str())};
    Ok(format!("EXECUTE FUNCTION {}({})",full(kind,schema,function)?,values.join(", ")))
}
fn trigger_create(kind: &DbKind, schema: &str, table: &str, trigger: &TriggerDesign) -> Result<String> {
    let timing=trigger.timing.trim().to_ascii_uppercase();
    ensure!(["BEFORE","AFTER"].contains(&timing.as_str()),"TABLE_TRIGGER_TIMING_INVALID");
    let event=trigger.event.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
    let event_tokens=syntax(&event)?;let mut events=BTreeSet::new();
    for (i,t) in event_tokens.iter().enumerate() {
        if i%2==1 {ensure!(pg(kind)&&t.word("OR"),"TABLE_TRIGGER_EVENT_INVALID");}
        else {ensure!(["INSERT","UPDATE","DELETE"].iter().any(|w|t.word(w))&&events.insert(t.text),"TABLE_TRIGGER_EVENT_INVALID");}
    }
    ensure!(!event_tokens.is_empty()&&event_tokens.len()%2==1,"TABLE_TRIGGER_EVENT_INVALID");
    let body=if pg(kind){pg_function_call(&trigger.statement,kind,schema)?}
        else{format!("BEGIN {}; END",mysql_body(&trigger.statement)?)};
    Ok(format!("CREATE TRIGGER {} {timing} {event} ON {} FOR EACH ROW {body}",
        if pg(kind){ident(kind,&trigger.name)?}else{full(kind,schema,&trigger.name)?},full(kind,schema,table)?))
}
fn drop_trigger(kind: &DbKind, schema: &str, table: &str, name: &str) -> Result<String> {
    Ok(if pg(kind){format!("DROP TRIGGER {} ON {}",ident(kind,name)?,full(kind,schema,table)?)}
        else{format!("DROP TRIGGER {}",full(kind,schema,name)?)})
}
fn enable_trigger(kind: &DbKind, schema: &str, table: &str, old: &NativeTrigger, name: &str) -> Result<Option<DesignStep>> {
    if !pg(kind){return Ok(None);}
    let command=match old.enabled.as_deref(){Some("O")|None=>return Ok(None),Some("D")=>"DISABLE",Some("R")=>"ENABLE REPLICA",Some("A")=>"ENABLE ALWAYS",_=>bail!("TABLE_PG_TRIGGER_ENABLED_UNKNOWN")};
    Ok(Some(step(format!("保留触发器 {name} enabled 属性"),format!("ALTER TABLE {} {command} TRIGGER {}",full(kind,schema,table)?,ident(kind,name)?))))
}
fn check_tail(definition: &str) -> Result<&str> {
    let ts=meaningful(definition,false)?;
    let check=ts.iter().position(|t|t.word("CHECK")).ok_or_else(||anyhow!("TABLE_CHECK_DEFINITION_UNKNOWN"))?;
    let mut depth=0;
    for t in &ts[check+1..] {
        if t.symbol("("){depth+=1;}if t.symbol(")"){depth-=1;}
        if depth==0 {return Ok(definition[t.end..].trim());}
    }
    bail!("TABLE_CHECK_DEFINITION_UNKNOWN")
}
fn names_unique<'a>(kind: &DbKind, names: impl Iterator<Item=&'a str>) -> Result<()> {
    let mut used=BTreeSet::new();
    for name in names {ident(kind,name)?;ensure!(used.insert(if pg(kind){name.to_string()}else{name.to_ascii_lowercase()}),"TABLE_OBJECT_NAME_DUPLICATED");}
    Ok(())
}
fn change_sets(before: &Stored, design: &TableDesign, meta: &Evidence) -> Result<(Vec<DesignStep>,Vec<DesignStep>)> {
    let kind=&before.kind;let schema=&design.schema;let table=&design.table;let target=full(kind,schema,table)?;
    let mut drops=Vec::new();let mut adds=Vec::new();
    if let Some(checks)=&design.checks {
        names_unique(kind,checks.iter().map(|c|c.name.as_str()))?;let mut used=BTreeSet::new();
        for c in checks {
            let old=c.orig_name.as_deref().map(|name|meta.constraints.iter().find(|o|o.kind=="c"&&o.name==name)
                .ok_or_else(||anyhow!("TABLE_CHECK_ORIGIN_UNKNOWN"))).transpose()?;
            if let Some(old)=old {ensure!(used.insert(old.name.clone()),"TABLE_CHECK_ORIGIN_DUPLICATED");}
            if old.is_some_and(|old|old.name==c.name&&expression_equal(old.expression.as_deref().unwrap_or(""),&c.expression)) {continue;}
            expression(&c.expression,false)?;
            if let Some(old)=old {drops.push(step(format!("撤销检查 {}",old.name),format!("ALTER TABLE {target} DROP {} {}",if pg(kind){"CONSTRAINT"}else{"CHECK"},ident(kind,&old.name)?)));}
            let tail=old.map(|old|check_tail(&old.definition)).transpose()?.unwrap_or("");
            adds.push(step(format!("保存检查 {}",c.name),format!("ALTER TABLE {target} ADD CONSTRAINT {} CHECK ({}){}",ident(kind,&c.name)?,c.expression,if tail.is_empty(){String::new()}else{format!(" {tail}")})));
        }
        for old in meta.constraints.iter().filter(|c|c.kind=="c") {
            if !used.contains(&old.name) {drops.push(step(format!("删除检查 {}",old.name),format!("ALTER TABLE {target} DROP {} {}",if pg(kind){"CONSTRAINT"}else{"CHECK"},ident(kind,&old.name)?)));}
        }
    }
    if let Some(triggers)=&design.triggers {
        names_unique(kind,triggers.iter().map(|t|t.name.as_str()))?;let mut used=BTreeSet::new();
        for t in triggers {
            let old=t.orig_name.as_deref().map(|name|meta.triggers.iter().find(|o|o.name==name).ok_or_else(||anyhow!("TABLE_TRIGGER_ORIGIN_UNKNOWN"))).transpose()?;
            if let Some(old)=old {ensure!(used.insert(old.name.clone()),"TABLE_TRIGGER_ORIGIN_DUPLICATED");if trigger_same(old,t)?{continue;}}
            else{ensure!(t.definition.is_none(),"TABLE_TRIGGER_DEFINITION_READ_ONLY: 新 trigger 不接受任意 definition");}
            if let Some(old)=old {ensure!(!old.complex,"TABLE_TRIGGER_COMPLEX_EDIT_UNSUPPORTED: constraint/WHEN/transition/statement trigger 仅保留或显式删除");
                drops.push(step(format!("撤销触发器 {}",old.name),drop_trigger(kind,schema,table,&old.name)?));}
            adds.push(step(format!("保存触发器 {}",t.name),trigger_create(kind,schema,table,t)?));
            if let Some(old)=old {if let Some(s)=enable_trigger(kind,schema,table,old,&t.name)?{adds.push(s);}}
        }
        for old in &meta.triggers {if !used.contains(&old.name){drops.push(step(format!("删除触发器 {}",old.name),drop_trigger(kind,schema,table,&old.name)?));}}
    }
    Ok((drops,adds))
}

/// 完整目标设计 -> 原生 ALTER/COMMENT/约束/trigger 计划；未修改目录定义不重建。
pub(crate) fn design_steps(before: &Stored, d: &TableDesign) -> Result<(Vec<DesignStep>,bool)> {
    ensure!(before.public.capabilities.rebuild.supported,"TABLE_DESIGN_CAPABILITY_UNSUPPORTED: {}",before.public.capabilities.limitations.join("；"));
    let kind=&before.kind;let schema=&before.public.schema;
    ensure!(d.schema==schema.database&&d.table==schema.table,"TABLE_DESIGN_IDENTITY_CHANGED");
    ensure!(!d.columns.is_empty()&&d.columns.len()<=4096,"TABLE_DESIGN_COLUMNS_INVALID");
    names_unique(kind,d.columns.iter().map(|c|c.name.as_str()))?;
    let meta=evidence(before)?;let baseline=&before.public.design;
    let mut originals=BTreeSet::new();
    for c in &d.columns {
        type_sql(kind,c)?;
        if let Some(original)=c.orig_name.as_deref() {
            ensure!(originals.insert(original.to_string())&&schema.columns.iter().any(|a|a.name==original),"TABLE_COLUMN_ORIGIN_UNKNOWN_OR_DUPLICATED");
        }
        if let Some(comment)=&c.comment {literal(comment)?;}
        let old=c.orig_name.as_deref().and_then(|n|baseline.columns.iter().find(|b|b.name==n));
        if !old.is_some_and(|b|default_equal(b.default_value.as_deref(),c.default_value.as_deref())) {if let Some(v)=&c.default_value{expression(v,true)?;}}
    }
    if pg(kind) {
        let actual_order:Vec<&str>=schema.columns.iter().filter(|c|originals.contains(&c.name)).map(|c|c.name.as_str()).collect();
        let wanted_order:Vec<&str>=d.columns.iter().filter_map(|c|c.orig_name.as_deref()).collect();
        ensure!(actual_order==wanted_order,"TABLE_PG_COLUMN_REORDER_UNSUPPORTED: PostgreSQL 无原生列顺序 ALTER，不能静默忽略移动");
        let first_new=d.columns.iter().position(|c|c.orig_name.is_none());
        ensure!(first_new.map_or(true,|at|d.columns[at..].iter().all(|c|c.orig_name.is_none())),"TABLE_PG_COLUMN_INSERT_POSITION_UNSUPPORTED: 新列仅能原生追加，请将新增列移到末尾");
    }
    let target=full(kind,&d.schema,&d.table)?;
    identifier_list(kind,&d.primary_key).or_else(|e|if d.primary_key.is_empty(){Ok(String::new())}else{Err(e)})?;
    ensure!(d.primary_key.iter().all(|n|d.columns.iter().any(|c|&c.name==n)),"TABLE_PRIMARY_COLUMN_UNKNOWN");
    let old_pk=design::ordered_primary_key(schema)?;let pk_changed=old_pk!=d.primary_key;
    let (mut steps,object_adds)=change_sets(before,d,&meta)?;
    let mut key_adds=Vec::new();
    if let Some(indexes)=&d.indexes {
        names_unique(kind,indexes.iter().map(|i|i.name.as_str()))?;
        for old in meta.indexes.iter().filter(|i|!i.primary) {
            let wanted=indexes.iter().find(|i|i.name==old.name);
            let unchanged=wanted.is_some_and(|w|old.keys==w.columns&&old.unique==w.unique);
            if unchanged {continue;}
            if wanted.is_some(){ensure!(!old.complex&&old.constraint_kind.as_deref()!=Some("x"),"TABLE_COMPLEX_INDEX_EDIT_UNSUPPORTED: 保留原始索引或显式删除后新建");}
            let sql=if pg(kind) {if let Some(constraint)=&old.constraint {format!("ALTER TABLE {target} DROP CONSTRAINT {}",ident(kind,constraint)?)}else{format!("DROP INDEX {}",full(kind,&d.schema,&old.name)?)} }
                else{format!("ALTER TABLE {target} DROP INDEX {}",ident(kind,&old.name)?)};
            steps.push(step(format!("撤销索引 {}",old.name),sql));
        }
        for index in indexes {
            let unchanged=meta.indexes.iter().any(|i|!i.primary&&i.name==index.name&&i.keys==index.columns&&i.unique==index.unique);
            if unchanged {continue;}
            ensure!(index.columns.iter().all(|name|d.columns.iter().any(|c|&c.name==name)),"TABLE_INDEX_COLUMN_UNKNOWN");
            let keys=identifier_list(kind,&index.columns)?;let unique=if index.unique{"UNIQUE "}else{""};
            key_adds.push(step(format!("保存索引 {}",index.name),if pg(kind){format!("CREATE {unique}INDEX {} ON {target} ({keys})",ident(kind,&index.name)?)}else{format!("ALTER TABLE {target} ADD {unique}INDEX {} ({keys})",ident(kind,&index.name)?)}));
        }
    }
    if let Some(fks)=&d.foreign_keys {
        names_unique(kind,fks.iter().map(|f|f.name.as_str()))?;
        for old in &schema.foreign_keys {
            if fks.iter().find(|f|f.name==old.name).map(|f|fk_equal(old,f,&d.schema,kind)).transpose()?.unwrap_or(false){continue;}
            steps.push(step(format!("撤销外键 {}",old.name),format!("ALTER TABLE {target} DROP {} {}",if pg(kind){"CONSTRAINT"}else{"FOREIGN KEY"},ident(kind,&old.name)?)));
        }
        for fk in fks {
            if schema.foreign_keys.iter().find(|old|old.name==fk.name).map(|old|fk_equal(old,fk,&d.schema,kind)).transpose()?.unwrap_or(false){continue;}
            ensure!(fk.columns.iter().all(|name|d.columns.iter().any(|c|&c.name==name)),"TABLE_FK_LOCAL_COLUMN_UNKNOWN");
            key_adds.push(step(format!("保存外键 {}",fk.name),fk_sql(kind,&d.schema,&d.table,fk)?));
        }
    }
    if pk_changed&&pg(kind)&&!old_pk.is_empty() {
        let constraint=meta.constraints.iter().find(|c|c.kind=="p").ok_or_else(||anyhow!("TABLE_PRIMARY_CONSTRAINT_UNKNOWN"))?;
        steps.push(step("撤销原主键",format!("ALTER TABLE {target} DROP CONSTRAINT {}",ident(kind,&constraint.name)?)));
    }
    // 未修改索引/外键不能因为删列而被服务器顺带移除。
    let removed:Vec<&str>=schema.columns.iter().filter(|c|!originals.contains(&c.name)).map(|c|c.name.as_str()).collect();
    for index in &meta.indexes {
        let kept=index.primary&&!pk_changed || !index.primary&&d.indexes.as_ref().map_or(true,|is|is.iter().any(|i|i.name==index.name&&i.columns==index.keys));
        ensure!(!kept||!index.keys.iter().any(|key|removed.contains(&key.as_str())),"TABLE_DROP_COLUMN_REQUIRES_INDEX_CHANGE");
    }
    for fk in &schema.foreign_keys {
        let kept=d.foreign_keys.as_ref().map_or(true,|fs|fs.iter().any(|f|same(&old_fk_design(fk),f)));
        ensure!(!kept||!fk.columns.iter().any(|key|removed.contains(&key.as_str())),"TABLE_DROP_COLUMN_REQUIRES_FK_CHANGE");
    }
    let mut auto_new=None;
    for c in d.columns.iter().filter(|c|c.orig_name.is_none()) {
        if !pg(kind)&&c.auto_increment {ensure!(auto_new.is_none(),"TABLE_MYSQL_MULTIPLE_AUTO_INCREMENT");auto_new=Some(c);continue;}
        steps.push(step(format!("新增列 {}",c.name),format!("ALTER TABLE {target} ADD COLUMN {}",new_column(kind,c)?)));
        if pg(kind) {if let Some(comment)=&c.comment {steps.push(step(format!("保存列 {} 注释",c.name),format!("COMMENT ON COLUMN {target}.{} IS {}",ident(kind,&c.name)?,literal(comment)?)));}}
    }
    let mut pk_done=false;
    if let Some(column)=auto_new {
        ensure!(d.primary_key.first()==Some(&column.name),"TABLE_MYSQL_NEW_AUTO_REQUIRES_LEADING_PRIMARY_KEY");
        let mut clauses=vec![format!("ADD COLUMN {}",new_column(kind,column)?)];
        if !old_pk.is_empty(){clauses.push("DROP PRIMARY KEY".into());}
        clauses.push(format!("ADD PRIMARY KEY ({})",identifier_list(kind,&d.primary_key)?));
        steps.push(step(format!("新增自增列 {} 与目标主键",column.name),format!("ALTER TABLE {target} {}",clauses.join(", "))));pk_done=true;
    }
    for c in d.columns.iter().filter(|c|c.orig_name.is_some()) {
        let original=c.orig_name.as_deref().unwrap();let old=baseline.columns.iter().find(|o|o.name==original).unwrap();
        let column=meta.columns.iter().find(|o|o.name==original).ok_or_else(||anyhow!("TABLE_COLUMN_EVIDENCE_UNKNOWN"))?;
        if !pg(kind) {
            if c.name!=original||!same(c,old) {
                let raw=mysql_edit_column(column,old,c,kind)?;
                steps.push(step(format!("修改列 {}",c.name),format!("ALTER TABLE {target} CHANGE COLUMN {} {raw}",ident(kind,original)?)));
            }
        } else {
            let type_changed=!type_equal(kind,&schema.columns.iter().find(|a|a.name==original).unwrap().data_type,c)?;
            let default_changed=!default_equal(old.default_value.as_deref(),c.default_value.as_deref());
            if column.flags&GENERATED!=0 {ensure!(!type_changed&&!default_changed&&c.auto_increment==old.auto_increment,"TABLE_PG_GENERATED_COLUMN_EDIT_UNSUPPORTED");}
            if c.name!=original {steps.push(step(format!("重命名列 {original}"),format!("ALTER TABLE {target} RENAME COLUMN {} TO {}",ident(kind,original)?,ident(kind,&c.name)?)));}
            let name=ident(kind,&c.name)?;
            if type_changed {steps.push(step(format!("修改列 {} 类型",c.name),format!("ALTER TABLE {target} ALTER COLUMN {name} TYPE {}",type_sql(kind,c)?)));}
            if old.nullable!=c.nullable {steps.push(step(format!("修改列 {} 可空性",c.name),format!("ALTER TABLE {target} ALTER COLUMN {name} {} NOT NULL",if c.nullable{"DROP"}else{"SET"})));}
            if old.auto_increment!=c.auto_increment {
                if column.flags&SERIAL!=0 {bail!("TABLE_PG_SERIAL_EDIT_UNSUPPORTED: 保留既有 owned serial 序列；移除/替换需显式序列设计");}
                let sql=if c.auto_increment {ensure!(c.default_value.is_none(),"TABLE_PG_IDENTITY_DEFAULT_CONFLICT");format!("ALTER TABLE {target} ALTER COLUMN {name} ADD GENERATED BY DEFAULT AS IDENTITY")}
                    else{format!("ALTER TABLE {target} ALTER COLUMN {name} DROP IDENTITY")};
                steps.push(step(format!("修改列 {} identity",c.name),sql));
            }
            if default_changed {
                ensure!(column.flags&(IDENTITY_ALWAYS|IDENTITY_DEFAULT)==0||!c.auto_increment,"TABLE_PG_IDENTITY_DEFAULT_CONFLICT");
                steps.push(step(format!("修改列 {} 默认值",c.name),match c.default_value.as_deref(){Some(v)=>format!("ALTER TABLE {target} ALTER COLUMN {name} SET DEFAULT ({v})"),None=>format!("ALTER TABLE {target} ALTER COLUMN {name} DROP DEFAULT")}));
            }
            if old.comment.as_deref().unwrap_or("")!=c.comment.as_deref().unwrap_or("") {steps.push(step(format!("保存列 {} 注释",c.name),format!("COMMENT ON COLUMN {target}.{name} IS {}",match c.comment.as_deref().filter(|s|!s.is_empty()){Some(v)=>literal(v)?,None=>"NULL".into()})));}
        }
    }
    if pk_changed&&!pk_done {
        if pg(kind) {
            if !d.primary_key.is_empty(){steps.push(step("建立目标主键",format!("ALTER TABLE {target} ADD PRIMARY KEY ({})",identifier_list(kind,&d.primary_key)?)));}
        } else {
            let mut clauses=Vec::new();if !old_pk.is_empty(){clauses.push("DROP PRIMARY KEY".into());}
            if !d.primary_key.is_empty(){clauses.push(format!("ADD PRIMARY KEY ({})",identifier_list(kind,&d.primary_key)?));}
            if !clauses.is_empty(){steps.push(step("保存目标主键",format!("ALTER TABLE {target} {}",clauses.join(", "))));}
        }
    }
    for original in removed {steps.push(step(format!("删除列 {original}"),format!("ALTER TABLE {target} DROP COLUMN {}",ident(kind,original)?)));}
    if !pg(kind) {
        let current_order:Vec<&str>=schema.columns.iter().filter(|c|originals.contains(&c.name)).map(|c|c.name.as_str()).collect();
        let wanted_order:Vec<&str>=d.columns.iter().filter_map(|c|c.orig_name.as_deref()).collect();
        let inserted_middle=d.columns.iter().position(|c|c.orig_name.is_none()).is_some_and(|at|d.columns[at..].iter().any(|c|c.orig_name.is_some()));
        if current_order!=wanted_order||inserted_middle {
            for (at,c) in d.columns.iter().enumerate() {
                let raw=if let Some(original)=c.orig_name.as_deref(){mysql_edit_column(meta.columns.iter().find(|m|m.name==original).unwrap(),baseline.columns.iter().find(|m|m.name==original).unwrap(),c,kind)?}else{new_column(kind,c)?};
                let position=if at==0{"FIRST".to_string()}else{format!("AFTER {}",ident(kind,&d.columns[at-1].name)?)};
                steps.push(step(format!("排列列 {}",c.name),format!("ALTER TABLE {target} MODIFY COLUMN {raw} {position}")));
            }
        }
    }
    steps.extend(key_adds);
    if let Some(comment)=d.comment.as_deref() {
        if comment!=schema.comment.as_deref().unwrap_or("") {
            steps.push(step("保存表注释",if pg(kind){format!("COMMENT ON TABLE {target} IS {}",if comment.is_empty(){"NULL".into()}else{literal(comment)?})}else{format!("ALTER TABLE {target} COMMENT = {}",literal(comment)?)}));
        }
    }
    steps.extend(option_steps(before,d.options.as_ref())?);steps.extend(object_adds);
    let destructive=steps.iter().any(|s|meaningful(&s.sql,false).map(|ts|ts.iter().any(|t|t.word("DROP"))).unwrap_or(true));
    Ok((steps,destructive))
}

fn copy_name(kind: &DbKind, target: &str, original: &str) -> String {
    let suffix=revision(&(target,original)); let suffix=&suffix[..12];
    let maximum=if pg(kind){63}else{64};
    let mut prefix=format!("{target}_{original}");
    while prefix.len()+suffix.len()+1>maximum {prefix.pop();}
    format!("{prefix}_{suffix}")
}
fn rewrite_constraint(raw: &str, kind: &DbKind, schema: &str, source: &str, target: &str,
    original: &str, replacement: &str) -> Result<String> {
    let tokens=meaningful(raw,false)?;let mut edits=Vec::new();
    if tokens.first().is_some_and(|t|t.word("CONSTRAINT")) {
        ensure!(tokens.get(1).is_some_and(|t|t.name().ok().as_deref()==Some(original)),"TABLE_CONSTRAINT_REWRITE_NAME_MISMATCH");
        edits.push((tokens[1].start,tokens[1].end,ident(kind,replacement)?));
    }
    for (at,t) in tokens.iter().enumerate() {
        if t.word("REFERENCES") {
            let (start,end,names)=qualified_range(&tokens,at+1)?;
            if names.last().is_some_and(|n|n==source)&&(names.len()==1||names[0]==schema) {
                edits.push((start,end,full(kind,schema,target)?));
            }
        }
    }
    replace_ranges(raw,edits)
}
fn copy_trigger(old: &NativeTrigger, kind: &DbKind, schema: &str, source: &str, target: &str,
    replacement: &str) -> Result<String> {
    let backslash=old.mode.as_deref().is_some_and(|m|!m.split(',').any(|m|m=="NO_BACKSLASH_ESCAPES"));
    let tokens=meaningful(&old.definition,backslash)?;
    let trigger=tokens.iter().position(|t|t.word("TRIGGER")).ok_or_else(||anyhow!("TABLE_TRIGGER_HEADER_UNKNOWN"))?;
    let (name_start,name_end,names)=qualified_range(&tokens,trigger+1)?;
    ensure!(names.last()==Some(&old.name)&&(names.len()==1||names[0]==schema),"TABLE_TRIGGER_NAME_REWRITE_MISMATCH");
    let on=tokens.iter().enumerate().skip(trigger+2).find(|(_,t)|t.word("ON")).map(|(i,_)|i).ok_or_else(||anyhow!("TABLE_TRIGGER_PARENT_UNKNOWN"))?;
    let (start,end,names)=qualified_range(&tokens,on+1)?;
    ensure!(names.last().is_some_and(|n|n==source)&&(names.len()==1||names[0]==schema),"TABLE_TRIGGER_PARENT_REWRITE_MISMATCH");
    if !pg(kind) {
        let body_at=tokens.windows(3).position(|w|w[0].word("FOR")&&w[1].word("EACH")&&w[2].word("ROW")).ok_or_else(||anyhow!("TABLE_TRIGGER_BODY_UNKNOWN"))?+3;
        for (at,t) in tokens.iter().enumerate().skip(body_at) {
            if matches!(t.kind,LexKind::Word|LexKind::Ident)&&t.name().ok().as_deref()==Some(source) {
                // NEW/OLD.<列名> 可以和表同名；其他显式 source 身份需专门语义编译。
                let column=at>=2&&tokens[at-1].symbol(".")&&(tokens[at-2].word("NEW")||tokens[at-2].word("OLD"));
                ensure!(column,"TABLE_COPY_TRIGGER_SELF_REFERENCE_UNSUPPORTED: trigger body 显式引用源表，不能猜测重写");
            }
        }
    }
    replace_ranges(&old.definition,vec![(name_start,name_end,if pg(kind){ident(kind,replacement)?}else{full(kind,schema,replacement)?}),
        (start,end,full(kind,schema,target)?)])
}
fn trigger_environment(old: &NativeTrigger, safe_mode: Option<&str>) -> Result<(Vec<DesignStep>,Vec<DesignStep>)> {
    if old.mode.is_none(){return Ok((Vec::new(),Vec::new()));}
    let mut before=vec![step(format!("保留 trigger {} sql_mode",old.name),format!("SET SESSION sql_mode = {}",literal(old.mode.as_deref().unwrap())?))];
    if let (Some(charset),Some(collation))=(&old.charset,&old.collation) {
        option_word(charset)?;option_word(collation)?;
        before.push(step(format!("保留 trigger {} 字符环境",old.name),format!("SET NAMES {charset} COLLATE {collation}")));
    }
    let mut after=Vec::new();
    if let Some(mode)=safe_mode {
        // SET sql_mode 的值没有反斜杠；无论前一个 trigger 的模式如何，字面量均安全。
        ensure!(!mode.contains('\\'),"TABLE_SQL_MODE_INVALID");
        after.push(step("恢复 owned SQL 模式",format!("SET SESSION sql_mode = {}",literal(mode)?)));
    }
    // 业务元数据读回统一 UTF-8，复制 trigger 后不沿用它可能为 latin1 的客户端环境。
    after.push(step("恢复 owned 字符环境","SET NAMES utf8mb4"));
    Ok((before,after))
}
fn sequence_create(kind: &DbKind, sequence: &NativeSequence, namespace: &str, target: &str) -> Result<Vec<DesignStep>> {
    ensure!(sequence.owned&&sequence.last.is_some()&&sequence.called.is_some(),"TABLE_COPY_SEQUENCE_UNKNOWN");
    if sequence.identity{return Ok(Vec::new());}
    ensure!(["smallint","integer","bigint"].contains(&sequence.data_type.as_str()),"TABLE_COPY_SEQUENCE_TYPE_UNSUPPORTED");
    let name=copy_name(kind,target,&sequence.name);let seq=full(kind,namespace,&name)?;
    let table=full(kind,namespace,target)?;let column=ident(kind,&sequence.column)?;
    Ok(vec![step(format!("复制独立序列 {}",sequence.column),format!("CREATE SEQUENCE {seq} AS {} INCREMENT BY {} MINVALUE {} MAXVALUE {} START WITH {} CACHE {} {}CYCLE",
        sequence.data_type,sequence.increment,sequence.min,sequence.max,sequence.start,sequence.cache,if sequence.cycle{""}else{"NO "})),
        step(format!("绑定序列 {} ownership",sequence.column),format!("ALTER SEQUENCE {seq} OWNED BY {table}.{column}")),
        step(format!("替换列 {} serial 默认值",sequence.column),format!("ALTER TABLE {table} ALTER COLUMN {column} SET DEFAULT pg_catalog.nextval({}::regclass)",literal(&seq)?))])
}
fn sequence_state(kind: &DbKind, sequence: &NativeSequence, namespace: &str, target: &str) -> Result<DesignStep> {
    let last=sequence.last.ok_or_else(||anyhow!("TABLE_COPY_SEQUENCE_STATE_UNKNOWN"))?;
    let called=sequence.called.ok_or_else(||anyhow!("TABLE_COPY_SEQUENCE_STATE_UNKNOWN"))?;
    let object=if sequence.identity {
        format!("pg_catalog.pg_get_serial_sequence({}, {})::regclass",literal(&full(kind,namespace,target)?)?,literal(&sequence.column)?)
    } else {format!("{}::regclass",literal(&full(kind,namespace,&copy_name(kind,target,&sequence.name))?)?)};
    Ok(step(format!("保留列 {} 序列 state",sequence.column),format!("SELECT pg_catalog.setval({object},{last},{called})")))
}
fn copy_mysql_create(before: &Stored, target: &str, meta: &Evidence) -> Result<String> {
    let kind=&before.kind;let namespace=&before.public.schema.database;let source=&before.public.schema.table;
    let (_,parts,suffix)=body_parts(&before.ddl)?;let mut copied=Vec::new();
    for part in parts {
        let tokens=meaningful(&part,false)?;
        if tokens.first().is_some_and(|t|t.word("CONSTRAINT")) {
            let name=tokens[1].name()?;
            let old=meta.constraints.iter().find(|c|c.name==name).ok_or_else(||anyhow!("TABLE_COPY_CONSTRAINT_UNKNOWN"))?;
            if old.kind=="f" {continue;}
            copied.push(rewrite_constraint(&part,kind,namespace,source,target,&name,&copy_name(kind,target,&name))?);
        } else {copied.push(part);}
    }
    Ok(format!("CREATE TABLE {} (\n  {}\n){}",full(kind,namespace,target)?,copied.join(",\n  "),if suffix.is_empty(){String::new()}else{format!(" {suffix}")}))
}
fn copy_fk_steps(before: &Stored, meta: &Evidence, target: &str) -> Result<Vec<DesignStep>> {
    let kind=&before.kind;let namespace=&before.public.schema.database;let source=&before.public.schema.table;
    let mut out=Vec::new();
    for constraint in meta.constraints.iter().filter(|c|c.kind=="f") {
        let name=if pg(kind){constraint.name.clone()}else{copy_name(kind,target,&constraint.name)};
        let raw=rewrite_constraint(&constraint.definition,kind,namespace,source,target,&constraint.name,&name)?;
        let definition=if pg(kind){format!("CONSTRAINT {} {raw}",ident(kind,&name)?)}else{raw};
        out.push(step(format!("复制外键 {}",constraint.name),format!("ALTER TABLE {} ADD {definition}",full(kind,namespace,target)?)));
    }
    Ok(out)
}
fn copy_trigger_steps(before: &Stored, meta: &Evidence, target: &str) -> Result<Vec<DesignStep>> {
    let kind=&before.kind;let namespace=&before.public.schema.database;let source=&before.public.schema.table;
    let mut out=Vec::new();
    for trigger in &meta.triggers {
        let name=if pg(kind){trigger.name.clone()}else{copy_name(kind,target,&trigger.name)};
        let raw=copy_trigger(trigger,kind,namespace,source,target,&name)?;
        let (environment,restore)=trigger_environment(trigger,meta.safe_mode.as_deref())?;
        out.extend(environment);out.push(step(format!("复制触发器 {}",trigger.name),raw));out.extend(restore);
        if let Some(s)=enable_trigger(kind,namespace,target,trigger,&name)?{out.push(s);}
    }
    Ok(out)
}

/// T1 完整复制：原始 MySQL CREATE 或 PG LIKE INCLUDING ALL，补独立 serial/FK/trigger。
/// 复制数据先于 FK/trigger 安装；generated 列由目标定义计算，identity 显式保留原值。
pub(crate) fn operation_steps(before: &Stored, o: &TableOperation) -> Result<Vec<DesignStep>> {
    let kind=&before.kind;let schema=&before.public.schema;
    ensure!(o.schema==schema.database&&o.table==schema.table,"TABLE_OPERATION_IDENTITY_CHANGED");
    let meta=evidence(before)?;
    if o.kind=="clear" {
        ensure!(before.public.capabilities.clear.supported,"TABLE_CLEAR_CAPABILITY_UNSUPPORTED");
        ensure!(o.target_table.is_none(),"TABLE_CLEAR_TARGET_FORBIDDEN");
        return Ok(vec![step("清空表数据",format!("DELETE FROM {}",full(kind,&o.schema,&o.table)?))]);
    }
    ensure!(["copy_structure","copy_data"].contains(&o.kind.as_str()),"TABLE_OPERATION_KIND_INVALID");
    let allowed=if o.kind=="copy_data"{&before.public.capabilities.copy_data}else{&before.public.capabilities.copy_structure};
    ensure!(allowed.supported,"TABLE_COPY_CAPABILITY_UNSUPPORTED: {}",allowed.reason.as_deref().unwrap_or("特殊表目录无法证明完整复制"));
    let target=o.target_table.as_deref().ok_or_else(||anyhow!("TABLE_COPY_TARGET_REQUIRED"))?;
    ident(kind,target)?;ensure!(if pg(kind){target!=o.table}else{!target.eq_ignore_ascii_case(&o.table)},"TABLE_COPY_TARGET_IS_SOURCE");
    let target_full=full(kind,&o.schema,target)?;
    let mut steps=vec![step("复制完整表定义",if pg(kind){format!("CREATE {}TABLE {target_full} (LIKE {} INCLUDING ALL){}",
        if meta.table_properties["persistence"].as_str()==Some("u"){"UNLOGGED "}else{""},full(kind,&o.schema,&o.table)?,
        meta.table_properties["access_method"].as_str().map(|m|ident(kind,m).map(|m|format!(" USING {m}"))).transpose()?.unwrap_or_default())}
        else{copy_mysql_create(before,target,&meta)?})];
    if pg(kind) {
        if let Some(space)=meta.table_properties["tablespace"].as_str(){steps.push(step("复制 tablespace",format!("ALTER TABLE {target_full} SET TABLESPACE {}",ident(kind,space)?)));}
        if let Some(replica)=meta.table_properties["replica_identity"].as_str() {
            let value=match replica{"d"=>None,"f"=>Some("FULL"),"n"=>Some("NOTHING"),_=>bail!("TABLE_COPY_REPLICA_IDENTITY_INDEX_UNSUPPORTED")};
            if let Some(value)=value{steps.push(step("复制 replica identity",format!("ALTER TABLE {target_full} REPLICA IDENTITY {value}")));}
        }
        if let Some(options)=meta.table_properties["options"].as_array() {
            let mut pairs=Vec::new();
            for option in options {
                let option=option.as_str().ok_or_else(||anyhow!("TABLE_COPY_RELOPTION_UNKNOWN"))?;
                let (name,value)=option.split_once('=').ok_or_else(||anyhow!("TABLE_COPY_RELOPTION_UNKNOWN"))?;
                ensure!(name.split('.').all(|part|option_word(part).is_ok()),"TABLE_COPY_RELOPTION_NAME_INVALID");
                // 值来自服务器目录，仍按值引用；不能把目录值提升为任意 SQL 片段。
                pairs.push(format!("{name}={}",literal(value)?));
            }
            if !pairs.is_empty(){steps.push(step("复制全部 reloptions",format!("ALTER TABLE {target_full} SET ({})",pairs.join(", "))));}
        }
        for seq in &meta.sequences {steps.extend(sequence_create(kind,seq,&o.schema,target)?);}
        for column in &meta.columns {
            if column.statistics.is_some_and(|n|n>=0){steps.push(step(format!("复制列 {} statistics",column.name),format!("ALTER TABLE {target_full} ALTER COLUMN {} SET STATISTICS {}",ident(kind,&column.name)?,column.statistics.unwrap())));}
            for option in &column.options {
                let (key,value)=option.split_once('=').ok_or_else(||anyhow!("TABLE_COPY_COLUMN_OPTION_UNKNOWN"))?;
                ensure!(["n_distinct","n_distinct_inherited"].contains(&key)&&value.parse::<f64>().is_ok()&&value.bytes().all(|b|b.is_ascii_digit()||matches!(b,b'.'|b'-'|b'+'|b'e'|b'E')),"TABLE_COPY_COLUMN_OPTION_UNSUPPORTED");
                steps.push(step(format!("复制列 {} option",column.name),format!("ALTER TABLE {target_full} ALTER COLUMN {} SET ({key}={value})",ident(kind,&column.name)?)));
            }
        }
    }
    if o.kind=="copy_data" {
        let columns:Vec<String>=before.columns.iter().filter(|(_,flags)|flags&GENERATED==0).map(|(name,_)|name.clone()).collect();
        ensure!(!columns.is_empty(),"TABLE_COPY_ALL_GENERATED_COLUMNS_UNSUPPORTED");
        let cols=identifier_list(kind,&columns)?;
        steps.push(step("复制原始行值",format!("INSERT INTO {target_full} ({cols}){} SELECT {cols} FROM {}",
            if pg(kind){" OVERRIDING SYSTEM VALUE"}else{""},full(kind,&o.schema,&o.table)?)));
    }
    if pg(kind) {for seq in &meta.sequences{steps.push(sequence_state(kind,seq,&o.schema,target)?);}}
    else if let Some(next)=&meta.next_auto {ensure!(next.bytes().all(|b|b.is_ascii_digit()),"TABLE_AUTO_INCREMENT_STATE_INVALID");steps.push(step("保留 AUTO_INCREMENT next",format!("ALTER TABLE {target_full} AUTO_INCREMENT = {next}")));}
    steps.extend(copy_fk_steps(before,&meta,target)?);
    // 所有源 trigger 已在源表 metadata 中读取；此时数据已复制，不触发目标 trigger 的副作用。
    steps.extend(copy_trigger_steps(before,&meta,target)?);
    if pg(kind) {if let Some(comment)=&schema.comment{steps.push(step("复制表注释",format!("COMMENT ON TABLE {target_full} IS {}",literal(comment)?)));}}
    Ok(steps)
}

fn option_matches(expected: &TableOptionsDesign, actual: &TableOptionsDesign, kind: &DbKind, ddl: &str) -> Result<bool> {
    if pg(kind){return Ok(expected.fillfactor==actual.fillfactor);}
    let eq=|a:&Option<String>,b:&Option<String>|a.as_deref().unwrap_or("").eq_ignore_ascii_case(b.as_deref().unwrap_or(""));
    let row=if expected.row_format.as_deref()==Some("DEFAULT") {
        let (_,_,suffix)=body_parts(ddl)?;let ts=meaningful(&suffix,false)?;
        !ts.iter().enumerate().any(|(at,t)|t.word("ROW_FORMAT")&&!ts.get(at+2).is_some_and(|t|t.word("DEFAULT")))
    }else{eq(&expected.row_format,&actual.row_format)};
    Ok(eq(&expected.engine,&actual.engine)&&eq(&expected.charset,&actual.charset)&&eq(&expected.collation,&actual.collation)&&row)
}
fn verify_design(before: &Stored, after: &Stored, wanted: &TableDesign) -> Result<()> {
    let kind=&before.kind;let actual=&after.public.schema;let old=evidence(before)?;let new=evidence(after)?;
    ensure!(after.public.completeness=="complete","TABLE_READBACK_INCOMPLETE");
    ensure!(actual.columns.iter().map(|c|&c.name).collect::<Vec<_>>()==wanted.columns.iter().map(|c|&c.name).collect::<Vec<_>>(),"TABLE_COLUMNS_ORDER_READBACK_MISMATCH");
    for c in &wanted.columns {
        let actual=actual.columns.iter().find(|a|a.name==c.name).ok_or_else(||anyhow!("TABLE_COLUMN_READBACK_MISSING"))?;
        let serial_type=pg(kind)&&["serial","bigserial","smallserial","serial2","serial4","serial8"].contains(&c.data_type.to_ascii_lowercase().as_str());
        ensure!(type_equal(kind,&actual.data_type,c)?&&actual.nullable==c.nullable&&actual.auto_increment==(c.auto_increment||serial_type),
            "TABLE_COLUMN_READBACK_MISMATCH: {}",c.name);
        ensure!(actual.comment.as_deref().unwrap_or("")==c.comment.as_deref().unwrap_or(""),"TABLE_COLUMN_COMMENT_READBACK_MISMATCH: {}",c.name);
        let serial_new=pg(kind)&&c.orig_name.is_none()&&["serial","bigserial","smallserial","serial2","serial4","serial8"].contains(&c.data_type.to_ascii_lowercase().as_str());
        if !serial_new {ensure!(typed_default_equal(kind,&actual.data_type,actual.default.as_deref(),c.default_value.as_deref()),"TABLE_COLUMN_DEFAULT_READBACK_MISMATCH: {}",c.name);}
        if let Some(original)=c.orig_name.as_deref() {
            let baseline=before.public.design.columns.iter().find(|b|b.name==original).unwrap();
            if same(baseline,c) {
                let b=old.columns.iter().find(|m|m.name==original).unwrap();let a=new.columns.iter().find(|m|m.name==c.name).unwrap();
                ensure!(same(a,b),"TABLE_UNCHANGED_COLUMN_DEFINITION_LOST: {}",c.name);
            }
        }
    }
    ensure!(design::ordered_primary_key(actual)?==wanted.primary_key,"TABLE_PRIMARY_KEY_READBACK_MISMATCH");
    if let Some(comment)=&wanted.comment {ensure!(actual.comment.as_deref().unwrap_or("")==comment,"TABLE_COMMENT_READBACK_MISMATCH");}
    else{ensure!(actual.comment==before.public.schema.comment,"TABLE_UNCHANGED_COMMENT_LOST");}
    let indexes=wanted.indexes.as_ref().or(before.public.design.indexes.as_ref()).unwrap();
    let actual_indexes:Vec<&IndexInfo>=actual.indexes.iter().filter(|i|!i.primary).collect();
    ensure!(indexes.len()==actual_indexes.len()&&indexes.iter().all(|i|actual_indexes.iter().any(|a|a.name==i.name&&a.columns==i.columns&&a.unique==i.unique)),"TABLE_INDEX_READBACK_MISMATCH");
    for b in old.indexes.iter().filter(|i|!i.primary) {
        if indexes.iter().any(|i|i.name==b.name&&i.columns==b.keys&&i.unique==b.unique) {
            let a=new.indexes.iter().find(|i|i.name==b.name).ok_or_else(||anyhow!("TABLE_INDEX_DEFINITION_LOST"))?;
            ensure!(canonical(&a.definition)?==canonical(&b.definition)?&&a.properties==b.properties,"TABLE_UNCHANGED_INDEX_DEFINITION_LOST: {}",b.name);
        }
    }
    let fks=wanted.foreign_keys.as_ref().or(before.public.design.foreign_keys.as_ref()).unwrap();
    ensure!(fks.len()==actual.foreign_keys.len(),"TABLE_FOREIGN_KEY_READBACK_MISMATCH");
    for fk in fks {
        let a=actual.foreign_keys.iter().find(|a|a.name==fk.name).ok_or_else(||anyhow!("TABLE_FOREIGN_KEY_READBACK_MISSING"))?;
        ensure!(fk_equal(a,fk,&wanted.schema,kind)?,"TABLE_FOREIGN_KEY_READBACK_MISMATCH: {}",fk.name);
        if let Some(b)=before.public.schema.foreign_keys.iter().find(|b|b.name==fk.name) {
            if fk_equal(b,fk,&wanted.schema,kind)? {
                let b=old.constraints.iter().find(|c|c.kind=="f"&&c.name==fk.name).unwrap();
                let a=new.constraints.iter().find(|c|c.kind=="f"&&c.name==fk.name).unwrap();
                ensure!(same(a,b),"TABLE_UNCHANGED_FOREIGN_KEY_DEFINITION_LOST");
            }
        }
    }
    let checks=wanted.checks.as_ref().or(before.public.design.checks.as_ref()).unwrap();
    ensure!(checks.len()==after.public.objects.checks.len(),"TABLE_CHECK_READBACK_MISMATCH");
    for c in checks {
        let a=new.constraints.iter().find(|a|a.kind=="c"&&a.name==c.name).ok_or_else(||anyhow!("TABLE_CHECK_READBACK_MISSING"))?;
        ensure!(expression_equal(a.expression.as_deref().unwrap_or(""),&c.expression),"TABLE_CHECK_READBACK_MISMATCH");
        if let Some(b)=c.orig_name.as_deref().and_then(|n|old.constraints.iter().find(|a|a.kind=="c"&&a.name==n)) {
            if b.name==c.name&&expression_equal(b.expression.as_deref().unwrap_or(""),&c.expression){ensure!(same(a,b),"TABLE_UNCHANGED_CHECK_DEFINITION_LOST");}
            else{ensure!(a.validated==b.validated,"TABLE_CHECK_VALIDATION_PROPERTY_LOST");}
        }
    }
    let triggers=wanted.triggers.as_ref().or(before.public.design.triggers.as_ref()).unwrap();
    ensure!(triggers.len()==new.triggers.len(),"TABLE_TRIGGER_READBACK_MISMATCH");
    for t in triggers {
        let a=new.triggers.iter().find(|a|a.name==t.name).ok_or_else(||anyhow!("TABLE_TRIGGER_READBACK_MISSING"))?;
        ensure!(a.timing.eq_ignore_ascii_case(&t.timing)&&a.event.eq_ignore_ascii_case(&t.event.split_whitespace().collect::<Vec<_>>().join(" "))
            &&(a.body==t.statement||canonical(&a.body)?==canonical(&t.statement)?),"TABLE_TRIGGER_READBACK_MISMATCH: {}",t.name);
        if let Some(b)=t.orig_name.as_deref().and_then(|n|old.triggers.iter().find(|b|b.name==n)) {
            if trigger_same(b,t)? {ensure!(same(a,b),"TABLE_UNCHANGED_TRIGGER_DEFINITION_LOST: {}",t.name);}
            else if pg(kind){ensure!(a.enabled==b.enabled,"TABLE_TRIGGER_ENABLED_PROPERTY_LOST");}
        }
    }
    ensure!(option_matches(&desired_options(&before.public.options,wanted.options.as_ref(),kind)?,&after.public.options,kind,&after.ddl)?,"TABLE_OPTIONS_READBACK_MISMATCH");
    // 保留未建模 reloptions/storage/access method；仅 fillfactor 可通过此设计器修改。
    if pg(kind) {
        for key in ["persistence","access_method","tablespace","owner","relkind","partition","rls","force_rls","replica_identity"] {
            ensure!(old.table_properties[key]==new.table_properties[key],"TABLE_UNCHANGED_TABLE_PROPERTY_LOST: {key}");
        }
        let filtered=|m:&Evidence|m.table_properties["options"].as_array().map(|a|a.iter().filter(|v|!v.as_str().is_some_and(|s|s.starts_with("fillfactor="))).cloned().collect::<Vec<_>>());
        ensure!(filtered(&old)==filtered(&new),"TABLE_UNCHANGED_RELOPTIONS_LOST");
    }
    Ok(())
}
fn copy_index_signature(index: &NativeIndex, kind: &DbKind) -> Result<serde_json::Value> {
    let mut properties=index.properties.clone();
    // constraint/index 名由 LIKE 自动生成；定义 suffix 保留表达式、collation、opclass 等全部语法。
    let definition=if pg(kind) {
        let tokens=meaningful(&index.definition,false)?;
        let on=tokens.iter().position(|t|t.word("ON")).ok_or_else(||anyhow!("TABLE_PG_INDEX_DEFINITION_UNKNOWN"))?;
        let (_,end,_)=qualified_range(&tokens,if tokens.get(on+1).is_some_and(|t|t.word("ONLY")){on+2}else{on+1})?;
        canonical(&index.definition[end..])?
    }else{canonical(&index.definition)?};
    // JSON 为 null 的 properties 也保留，不把缺失值猜成已支持。
    if let Some(object)=properties.as_object_mut(){object.remove("name");}
    Ok(serde_json::json!({"definition":definition,"properties":properties,"keys":index.keys,"unique":index.unique,"primary":index.primary,"constraint_kind":index.constraint_kind,"valid":index.valid}))
}
fn verify_copy(before: &Stored, after: &Stored, target: &str) -> Result<()> {
    let kind=&before.kind;let old=evidence(before)?;let new=evidence(after)?;
    ensure!(after.public.completeness=="complete","TABLE_COPY_READBACK_INCOMPLETE");
    let a=&after.public.schema;let b=&before.public.schema;
    ensure!(a.columns.len()==b.columns.len()&&a.columns.iter().map(|c|&c.name).collect::<Vec<_>>()==b.columns.iter().map(|c|&c.name).collect::<Vec<_>>(),"TABLE_COPY_COLUMN_ORDER_MISMATCH");
    for (a,b) in a.columns.iter().zip(&b.columns) {
        ensure!(a.data_type==b.data_type&&a.nullable==b.nullable&&a.primary_key==b.primary_key&&a.auto_increment==b.auto_increment&&a.comment==b.comment,"TABLE_COPY_COLUMN_MISMATCH: {}",b.name);
        let serial=old.sequences.iter().any(|s|s.column==b.name&&!s.identity);
        if !serial {ensure!(default_equal(a.default.as_deref(),b.default.as_deref()),"TABLE_COPY_COLUMN_DEFAULT_MISMATCH");}
    }
    for (a,b) in new.columns.iter().zip(&old.columns) {
        ensure!(a.flags==b.flags&&a.generated==b.generated&&a.collation==b.collation&&a.storage==b.storage&&a.compression==b.compression&&a.statistics==b.statistics&&a.options==b.options,"TABLE_COPY_COLUMN_ATTRIBUTE_MISMATCH");
        if !pg(kind){ensure!(canonical(&a.raw)?==canonical(&b.raw)?,"TABLE_COPY_RAW_COLUMN_DEFINITION_LOST");}
    }
    ensure!(a.comment==b.comment&&option_matches(&before.public.options,&after.public.options,kind,&after.ddl)?,"TABLE_COPY_OPTIONS_COMMENT_MISMATCH");
    let signatures=|indexes:&[NativeIndex]|->Result<Vec<String>> {
        let mut out=indexes.iter().map(|i|copy_index_signature(i,kind).map(|v|v.to_string())).collect::<Result<Vec<_>>>()?;out.sort();Ok(out)
    };
    ensure!(signatures(&old.indexes)?==signatures(&new.indexes)?,"TABLE_COPY_INDEX_PROPERTIES_MISMATCH");
    if pg(kind) {
        let constraints=|meta:&Evidence|->Result<Vec<String>> {
            let mut values=meta.constraints.iter().filter(|c|["p","u","x"].contains(&c.kind.as_str()))
                .map(|c|canonical(&c.definition).map(|d|revision(&(&c.kind,d,c.validated,c.local,c.inherited)))).collect::<Result<Vec<_>>>()?;
            values.sort();Ok(values)
        };
        ensure!(constraints(&old)?==constraints(&new)?,"TABLE_COPY_KEY_EXCLUSION_CONSTRAINT_MISMATCH");
    }
    ensure!(old.constraints.iter().filter(|c|c.kind=="c"||c.kind=="f").count()==new.constraints.iter().filter(|c|c.kind=="c"||c.kind=="f").count(),"TABLE_COPY_CONSTRAINT_COUNT_MISMATCH");
    for c in old.constraints.iter().filter(|c|c.kind=="c"||c.kind=="f") {
        let name=if pg(kind){c.name.clone()}else{copy_name(kind,target,&c.name)};
        let a=new.constraints.iter().find(|a|a.kind==c.kind&&a.name==name).ok_or_else(||anyhow!("TABLE_COPY_CONSTRAINT_MISSING"))?;
        let expected=rewrite_constraint(&c.definition,kind,&b.database,&b.table,"__navkit_table",&c.name,"__navkit_constraint")?;
        let observed=rewrite_constraint(&a.definition,kind,&b.database,target,"__navkit_table",&name,"__navkit_constraint")?;
        ensure!(canonical(&observed)?==canonical(&expected)?&&a.validated==c.validated&&a.local==c.local&&a.inherited==c.inherited,"TABLE_COPY_CONSTRAINT_DEFINITION_MISMATCH: {}",c.name);
    }
    ensure!(old.triggers.len()==new.triggers.len(),"TABLE_COPY_TRIGGER_COUNT_MISMATCH");
    for t in &old.triggers {
        let name=if pg(kind){t.name.clone()}else{copy_name(kind,target,&t.name)};
        let a=new.triggers.iter().find(|a|a.name==name).ok_or_else(||anyhow!("TABLE_COPY_TRIGGER_MISSING"))?;
        let expected=copy_trigger(t,kind,&b.database,&b.table,"__navkit_table","__navkit_trigger")?;
        let observed=copy_trigger(a,kind,&b.database,target,"__navkit_table","__navkit_trigger")?;
        let bs=t.mode.as_deref().is_some_and(|m|!m.split(',').any(|m|m=="NO_BACKSLASH_ESCAPES"));
        let tokenized=|s:&str|meaningful(s,bs).map(|ts|ts.iter().map(|t|(t.kind as u8,t.text.to_string())).collect::<Vec<_>>());
        ensure!(tokenized(&observed)?==tokenized(&expected)?&&a.mode==t.mode&&a.charset==t.charset&&a.collation==t.collation&&a.enabled==t.enabled&&a.order==t.order&&a.properties==t.properties,"TABLE_COPY_TRIGGER_DEFINITION_MISMATCH: {}",t.name);
    }
    ensure!(old.sequences.len()==new.sequences.len(),"TABLE_COPY_SEQUENCE_COUNT_MISMATCH");
    for s in &old.sequences {
        let a=new.sequences.iter().find(|a|a.column==s.column).ok_or_else(||anyhow!("TABLE_COPY_SEQUENCE_MISSING"))?;
        ensure!(a.owned&&(a.schema!=s.schema||a.name!=s.name)&&a.identity==s.identity&&a.data_type==s.data_type&&a.start==s.start&&a.increment==s.increment&&a.min==s.min&&a.max==s.max&&a.cache==s.cache&&a.cycle==s.cycle&&a.last==s.last&&a.called==s.called,"TABLE_COPY_SEQUENCE_PROPERTIES_STATE_MISMATCH");
    }
    if !pg(kind){ensure!(old.next_auto==new.next_auto,"TABLE_COPY_AUTO_INCREMENT_STATE_MISMATCH");}
    else {
        for key in ["persistence","access_method","tablespace","options","relkind","partition","rls","force_rls","replica_identity"] {ensure!(old.table_properties[key]==new.table_properties[key],"TABLE_COPY_TABLE_PROPERTY_MISMATCH: {key}");}
    }
    Ok(())
}

async fn pg_count(conn: &mut sqlx::PgConnection, kind: &DbKind, namespace: &str, table: &str) -> Result<i64> {
    Ok(crate::sql_history::query_scalar(&format!("SELECT count(*)::bigint FROM {}",full(kind,namespace,table)?)).fetch_one(&mut *conn).await?)
}
async fn mysql_count(conn: &mut sqlx::MySqlConnection, kind: &DbKind, namespace: &str, table: &str) -> Result<i64> {
    Ok(crate::sql_history::query_scalar(&format!("SELECT CAST(count(*) AS SIGNED) FROM {}",full(kind,namespace,table)?)).fetch_one(&mut *conn).await?)
}
async fn pg_digest(conn: &mut sqlx::PgConnection, kind: &DbKind, namespace: &str, table: &str) -> Result<String> {
    let sql=format!("SELECT pg_catalog.md5(pg_catalog.row_to_json(navkit_row)::text) COLLATE \"C\" AS digest FROM {} AS navkit_row ORDER BY 1",full(kind,namespace,table)?);
    let mut rows=crate::sql_history::query_scalar::<_,String>(&sql).fetch(&mut *conn);let mut hash=Sha256::new();
    // 停止时仍排空当前 stream；drive 的独立 cancel 同时运行，不丢弃已发送 query future。
    while let Some(row)=rows.try_next().await? {hash.update(row.as_bytes());hash.update(b"\n");}
    Ok(hex_digest(hash.finalize().as_slice()))
}
async fn mysql_digest(conn: &mut sqlx::MySqlConnection, before: &Stored, table: &str) -> Result<String> {
    let values=before.public.schema.columns.iter().map(|c| {
        let col=ident(&before.kind,&c.name)?;
        Ok(format!("IF({col} IS NULL,'N;',CONCAT('V',OCTET_LENGTH(CAST({col} AS BINARY)),':',HEX(CAST({col} AS BINARY)),';'))"))
    }).collect::<Result<Vec<_>>>()?;
    let sql=format!("SELECT SHA2(CONCAT({}),256) AS digest FROM {} ORDER BY digest",values.join(","),full(&before.kind,&before.public.schema.database,table)?);
    let mut rows=crate::sql_history::query_scalar::<_,String>(&sql).fetch(&mut *conn);let mut hash=Sha256::new();
    while let Some(row)=rows.try_next().await? {hash.update(row.as_bytes());hash.update(b"\n");}
    Ok(hex_digest(hash.finalize().as_slice()))
}
async fn pg_target_absent(conn: &mut sqlx::PgConnection, namespace: &str, target: &str) -> Result<()> {
    let exists:bool=crate::sql_history::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2)").bind(namespace).bind(target).fetch_one(&mut *conn).await?;
    ensure!(!exists,"TABLE_COPY_TARGET_EXISTS: 目标对象已存在，禁止覆盖");Ok(())
}
async fn mysql_target_absent(conn: &mut sqlx::MySqlConnection, namespace: &str, target: &str) -> Result<()> {
    let count:i64=crate::sql_history::query_scalar("SELECT CAST(count(*) AS SIGNED) FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=?").bind(namespace).bind(target).fetch_one(&mut *conn).await?;
    ensure!(count==0,"TABLE_COPY_TARGET_EXISTS: 目标对象已存在，禁止覆盖");Ok(())
}

#[derive(Default)]
struct Progress {
    steps:Vec<DesignStepResult>,
    design:Option<TableDesignReceipt>,
    operation:Option<TableOperationReceipt>,
    transaction:String,
    begun:bool,
    commit_sent:bool,
    committed:bool,
    ddl_applied:usize,
    affected:u64,
    copied:u64,
    nontransactional:bool,
    dml_sent:bool,
    dml_active:bool,
    table_locked:bool,
    named_locks:Vec<String>,
}
type Output=Arc<parking_lot::Mutex<Progress>>;
fn target_work(work:&TableWork)->(&str,&str) {
    match work{TableWork::Design(d)=>(&d.schema,&d.table),TableWork::Operation(o)=>(&o.schema,&o.table)}
}
fn target_after(work:&TableWork)->&str {
    match work{TableWork::Design(d)=>&d.table,TableWork::Operation(o)=>o.target_table.as_deref().unwrap_or(&o.table)}
}
fn baseline_work(before:&Stored,work:&TableWork,binding:&Binding)->Result<()> {
    match work{TableWork::Design(d)=>table_workflows::require_baseline(before,d.base_revision.as_deref(),d.source_fingerprint.as_deref(),binding),
        TableWork::Operation(o)=>table_workflows::require_baseline(before,Some(&o.base_revision),Some(&o.source_fingerprint),binding)}
}
fn work_steps(before:&Stored,work:&TableWork)->Result<Vec<DesignStep>> {
    match work{TableWork::Design(d)=>design_steps(before,d).map(|(steps,_)|steps),TableWork::Operation(o)=>operation_steps(before,o)}
}
fn is_ddl(sql:&str)->bool {
    meaningful(sql,false).ok().and_then(|t|t.first().map(|t|t.word("CREATE")||t.word("ALTER")||t.word("DROP"))).unwrap_or(false)
}
fn dml(sql:&str)->bool {
    meaningful(sql,false).ok().and_then(|t|t.first().map(|t|t.word("INSERT")||t.word("DELETE")||t.word("UPDATE"))).unwrap_or(false)
}
fn record(output:&Output,step:&DesignStep,result:&std::result::Result<u64,sqlx::Error>) {
    let mut p=output.lock();
    p.steps.push(DesignStepResult {label:step.label.clone(),sql:step.sql.clone(),status:if result.is_ok(){"applied"}else{"failed"}.into(),error:result.as_ref().err().map(str_error)});
    if let Ok(count)=result {
        if is_ddl(&step.sql){p.ddl_applied+=1;}
        if step.sql.trim_start().starts_with("INSERT "){p.copied=*count;}
        if step.sql.trim_start().starts_with("DELETE "){p.affected=*count;}
    }
}
fn str_error(error:&sqlx::Error)->String {error.to_string()}
fn sql_result(result:std::result::Result<u64,sqlx::Error>,context:&str)->Result<u64> {
    match result {
        Ok(count)=>Ok(count),
        Err(error) if error.as_database_error().is_some()=>Err(anyhow::Error::from(error).context(context.to_string())),
        Err(error)=>Err(unknown(format!("TABLE_NATIVE_SENT_SQL_UNKNOWN: {context}: {error}; 不自动重放")).into()),
    }
}
fn success_receipt(output:&Output,work:&TableWork,after:Stored,transaction:&str,count:i64) {
    let mut p=output.lock();p.transaction=transaction.into();let public=after.public;
    match work {
        TableWork::Design(_)=>p.design=Some(TableDesignReceipt {receipt:DesignReceipt {steps:p.steps.clone(),verified:true,unverified:false,mismatches:Vec::new(),transaction:transaction.into(),schema:Some(public.schema.clone())},snapshot:Some(public)}),
        TableWork::Operation(o)=>p.operation=Some(TableOperationReceipt {kind:o.kind.clone(),source_table:o.table.clone(),target_table:o.target_table.clone(),affected_rows:p.affected,copied_rows:if o.kind=="copy_data"{count as u64}else{0},verified:true,mismatches:Vec::new(),transaction:transaction.into(),steps:p.steps.clone(),snapshot:Some(public)}),
    }
}
fn messages(started:Instant,transaction:&str)->MultiResult {
    MultiResult {sets:Vec::new(),messages:vec![format!("表操作已在同一 owned 连接完整读回，事务回执：{transaction}")],elapsed_ms:started.elapsed().as_millis() as u64,spool_id:None}
}
fn state_equal(before:&Stored,after:&Stored)->Result<bool> {
    let a=evidence(before)?;let b=evidence(after)?;
    Ok(a.next_auto==b.next_auto&&a.sequences.len()==b.sequences.len()&&a.sequences.iter().all(|s|b.sequences.iter().any(|a|a.column==s.column&&a.schema==s.schema&&a.name==s.name&&a.last==s.last&&a.called==s.called)))
}
fn preserve_data(work:&TableWork,before:&Stored)->bool {
    match work {
        TableWork::Operation(o)=>o.kind=="copy_data",
        TableWork::Design(d)=>d.columns.len()==before.public.design.columns.len()&&d.columns.iter().zip(&before.public.design.columns).all(|(a,b)|{
            let mut a=a.clone();a.comment=b.comment.clone();same(&a,b)
        }),
    }
}
fn advisory_key(namespace:&str,table:&str)->i64 {
    let digest=Sha256::digest(format!("navkit/table/{namespace}/{table}").as_bytes());
    i64::from_be_bytes(digest[..8].try_into().unwrap())
}

async fn pg_apply(conn:&mut sqlx::PgConnection,kind:&DbKind,entry:&Arc<Entry>,work:&TableWork,output:&Output)->Result<MultiResult> {
    let started=Instant::now();let (namespace,table)=target_work(work);stop(entry)?;
    crate::sql_history::query("BEGIN").execute(&mut *conn).await?;
    {let mut p=output.lock();p.begun=true;p.transaction="active".into();}
    crate::sql_history::query("SET LOCAL search_path = pg_catalog").execute(&mut *conn).await?;
    crate::sql_history::query("SET LOCAL standard_conforming_strings = on").execute(&mut *conn).await?;
    stop(entry)?;
    crate::sql_history::query(&format!("LOCK TABLE {} IN ACCESS EXCLUSIVE MODE",full(kind,namespace,table)?)).execute(&mut *conn).await?;
    // CAS 必须在实际持有表锁后重新读，不能把预览端 pool 快照当执行基线。
    let before=pg_snapshot(conn,kind,&entry.binding,table,false).await?;
    baseline_work(&before,work,&entry.binding)?;
    let steps=work_steps(&before,work)?;
    if let TableWork::Operation(o)=work {
        if o.kind!="clear" {
            let target=o.target_table.as_deref().ok_or_else(||anyhow!("TABLE_COPY_TARGET_REQUIRED"))?;
            crate::sql_history::query("SELECT pg_catalog.pg_advisory_xact_lock($1)").bind(advisory_key(namespace,target)).execute(&mut *conn).await?;
            pg_target_absent(conn,namespace,target).await?;
        }
    }
    let source_count=pg_count(conn,kind,namespace,table).await?;
    let digest=if preserve_data(work,&before){Some(pg_digest(conn,kind,namespace,table).await?)}else{None};
    for planned in &steps {
        stop(entry)?;
        let result=crate::sql_history::query(&planned.sql).execute(&mut *conn).await.map(|r|r.rows_affected());
        record(output,planned,&result);sql_result(result,&planned.label)?;
    }
    stop(entry)?;
    let after=pg_snapshot(conn,kind,&entry.binding,target_after(work),false).await?;
    let count=pg_count(conn,kind,namespace,target_after(work)).await?;
    match work {
        TableWork::Design(d)=>{verify_design(&before,&after,d)?;ensure!(count==source_count,"TABLE_DESIGN_ROW_COUNT_MISMATCH");
            if let Some(digest)=&digest{ensure!(pg_digest(conn,kind,namespace,table).await?==*digest,"TABLE_DESIGN_DATA_PRESERVATION_MISMATCH");}},
        TableWork::Operation(o) if o.kind=="clear"=>{ensure!(count==0&&before.base==after.base,"TABLE_CLEAR_READBACK_MISMATCH");},
        TableWork::Operation(o)=>{
            verify_copy(&before,&after,target_after(work))?;
            ensure!(count==if o.kind=="copy_data"{source_count}else{0},"TABLE_COPY_ROW_COUNT_MISMATCH");
            if let Some(digest)=&digest {ensure!(pg_digest(conn,kind,namespace,target_after(work)).await?==*digest,"TABLE_COPY_DATA_READBACK_MISMATCH");ensure!(pg_digest(conn,kind,namespace,table).await?==*digest,"TABLE_COPY_SOURCE_DATA_CHANGED");}
            let source=pg_snapshot(conn,kind,&entry.binding,table,false).await?;
            ensure!(before.base==source.base&&state_equal(&before,&source)?&&pg_count(conn,kind,namespace,table).await?==source_count,"TABLE_COPY_SOURCE_CHANGED");
        },
    }
    stop(entry)?;
    output.lock().commit_sent=true;
    crate::sql_history::query("COMMIT").execute(&mut *conn).await.map_err(|e|unknown(format!("TABLE_PG_COMMIT_UNKNOWN: {e}; 不自动重放")))?;
    {let mut p=output.lock();p.begun=false;p.committed=true;p.transaction="committed".into();}
    entry.record_remote_transaction("committed");
    if entry.stopping(){return Err(unknown("TABLE_PG_COMMIT_STOP_RACE: COMMIT 已发送，不能声明已回滚或发布成功 receipt").into());}
    success_receipt(output,work,after,if steps.is_empty(){"none"}else{"committed"},count);
    Ok(messages(started,"committed"))
}

async fn mysql_options_available(conn:&mut sqlx::MySqlConnection,before:&Stored,work:&TableWork)->Result<()> {
    let TableWork::Design(d)=work else{return Ok(());};
    let out=desired_options(&before.public.options,d.options.as_ref(),&before.kind)?;
    if let Some(engine)=&out.engine {
        let support:String=crate::sql_history::query_scalar("SELECT SUPPORT FROM information_schema.ENGINES WHERE ENGINE=?").bind(engine).fetch_one(&mut *conn).await?;
        ensure!(["YES","DEFAULT"].contains(&support.as_str()),"TABLE_MYSQL_ENGINE_UNAVAILABLE");
    }
    if let Some(charset)=&out.charset {
        let exists:i64=crate::sql_history::query_scalar("SELECT CAST(count(*) AS SIGNED) FROM information_schema.CHARACTER_SETS WHERE CHARACTER_SET_NAME=?").bind(charset).fetch_one(&mut *conn).await?;
        ensure!(exists==1,"TABLE_MYSQL_CHARSET_UNAVAILABLE");
    }
    if let Some(collation)=&out.collation {
        let charset:Option<String>=crate::sql_history::query_scalar("SELECT CHARACTER_SET_NAME FROM information_schema.COLLATIONS WHERE COLLATION_NAME=?").bind(collation).fetch_optional(&mut *conn).await?;
        ensure!(charset.is_some()&&charset.as_ref().map(|s|s.to_ascii_lowercase())==out.charset.as_ref().map(|s|s.to_ascii_lowercase()),"TABLE_MYSQL_COLLATION_CHARSET_MISMATCH");
    }
    Ok(())
}
async fn mysql_table_lock(conn:&mut sqlx::MySqlConnection,kind:&DbKind,namespace:&str,source:&str,target:Option<&str>,output:&Output)->Result<()> {
    let mut names=vec![format!("{} WRITE",full(kind,namespace,source)?)];
    if let Some(target)=target {names.push(format!("{} WRITE",full(kind,namespace,target)?));}
    // LOCK TABLES 不在 MySQL 预处理语句白名单内，服务端返回 1295；必须走文本协议。
    let statement=format!("LOCK TABLES {}",names.join(", "));
    (&mut *conn).execute(statement.as_str()).await?;
    output.lock().table_locked=true;Ok(())
}
async fn mysql_unlock(conn:&mut sqlx::MySqlConnection,output:&Output)->Result<()> {
    if output.lock().table_locked {
        // UNLOCK TABLES 不在 MySQL 预处理语句白名单内，服务端返回 1295；必须走文本协议。
        (&mut *conn).execute("UNLOCK TABLES").await?;output.lock().table_locked=false;
    }
    Ok(())
}
async fn mysql_commit(conn:&mut sqlx::MySqlConnection,entry:&Entry,output:&Output)->Result<()> {
    stop(entry)?;output.lock().commit_sent=true;
    crate::sql_history::query("COMMIT").execute(&mut *conn).await.map_err(|e|unknown(format!("TABLE_MYSQL_COMMIT_UNKNOWN: {e}; 不自动重放")))?;
    {let mut p=output.lock();p.dml_active=false;p.committed=true;}
    entry.record_remote_transaction("committed");
    ensure!(!entry.stopping(),"TABLE_MYSQL_COMMIT_STOP_RACE");Ok(())
}
async fn mysql_apply(conn:&mut sqlx::MySqlConnection,kind:&DbKind,entry:&Arc<Entry>,work:&TableWork,output:&Output)->Result<MultiResult> {
    let started=Instant::now();let (namespace,table)=target_work(work);stop(entry)?;
    mysql_mode(conn).await?;crate::sql_history::query("SET NAMES utf8mb4").execute(&mut *conn).await?;
    let autocommit:i64=crate::sql_history::query_scalar("SELECT @@SESSION.autocommit").fetch_one(&mut *conn).await?;
    ensure!(autocommit==1,"TABLE_MYSQL_NONORDINARY_SESSION: 不接管未知 session 事务");
    // InnoDB LOCK TABLES + autocommit=0 保有物理表锁；START TRANSACTION 会释放该锁，禁止使用。
    crate::sql_history::query("SET SESSION autocommit=0").execute(&mut *conn).await?;
    let mut locks=vec![format!("navkit_table_{}",revision(&(namespace,table)))];
    if let TableWork::Operation(o)=work {if let Some(target)=&o.target_table{locks.push(format!("navkit_table_{}",revision(&(namespace,target))));}}
    locks.sort();locks.dedup();
    for mut lock in locks {
        lock.truncate(64);stop(entry)?;
        let taken:Option<i64>=crate::sql_history::query_scalar("SELECT GET_LOCK(?,0)").bind(&lock).fetch_one(&mut *conn).await?;
        ensure!(taken==Some(1),"TABLE_MYSQL_NAMED_LOCK_BUSY_OR_UNKNOWN");output.lock().named_locks.push(lock);
    }
    stop(entry)?;mysql_table_lock(conn,kind,namespace,table,None,output).await?;
    let before=mysql_snapshot(conn,kind,&entry.binding,table,false).await?;baseline_work(&before,work,&entry.binding)?;
    mysql_options_available(conn,&before,work).await?;
    let steps=work_steps(&before,work)?;
    {let mut p=output.lock();p.nontransactional=before.public.options.engine.as_deref().map_or(true,|e|!e.eq_ignore_ascii_case("InnoDB"));p.transaction="active".into();}
    let source_count=mysql_count(conn,kind,namespace,table).await?;
    let digest=if preserve_data(work,&before){Some(mysql_digest(conn,&before,table).await?)}else{None};
    let copy=matches!(work,TableWork::Operation(o) if o.kind!="clear");
    if copy {mysql_target_absent(conn,namespace,target_after(work)).await?;}
    let mut target_created=false;
    for (at,planned) in steps.iter().enumerate() {
        stop(entry)?;
        // CREATE 目标或 CREATE/DROP TRIGGER 在部分 MySQL/MariaDB 版本不能处于 LOCK TABLES 模式。
        // 只在这些明确边界释放物理表锁；GET_LOCK 仍保有，目标 CREATE 不使用 IF NOT EXISTS。
        // 重新持锁后再次验证源结构、行数与 sequence，检测非协作连接的并发变化。
        let tokens=meaningful(&planned.sql,false)?;
        let creates_target=copy&&at==0;
        let trigger_ddl=tokens.first().is_some_and(|t|t.word("CREATE")||t.word("DROP"))&&tokens.iter().any(|t|t.word("TRIGGER"));
        if creates_target||trigger_ddl {mysql_unlock(conn,output).await?;}
        if dml(&planned.sql) {let mut p=output.lock();p.dml_sent=true;p.dml_active=true;}
        let result=crate::sql_history::query(&planned.sql).execute(&mut *conn).await.map(|r|r.rows_affected());
        record(output,planned,&result);sql_result(result,&planned.label)?;
        if is_ddl(&planned.sql){output.lock().committed=true;entry.record_remote_transaction("committed");}
        if creates_target{target_created=true;}
        if creates_target||trigger_ddl {mysql_table_lock(conn,kind,namespace,table,if target_created{Some(target_after(work))}else{None},output).await?;}
        if copy&&(is_ddl(&planned.sql)||creates_target) {
            let source=mysql_snapshot(conn,kind,&entry.binding,table,false).await?;
            ensure!(source.base==before.base&&state_equal(&before,&source)?&&mysql_count(conn,kind,namespace,table).await?==source_count,"TABLE_COPY_SOURCE_CONCURRENT_CHANGE");
        }
        // 在后续隐式 DDL 前确认数据 COMMIT；失败不能把已经提交的数据称为已回滚。
        if planned.sql.trim_start().starts_with("INSERT "){mysql_commit(conn,entry,output).await?;}
    }
    stop(entry)?;
    let after=mysql_snapshot(conn,kind,&entry.binding,target_after(work),false).await?;
    let count=mysql_count(conn,kind,namespace,target_after(work)).await?;
    match work {
        TableWork::Design(d)=>{verify_design(&before,&after,d)?;ensure!(count==source_count,"TABLE_DESIGN_ROW_COUNT_MISMATCH");if let Some(digest)=&digest{ensure!(mysql_digest(conn,&before,table).await?==*digest,"TABLE_DESIGN_DATA_PRESERVATION_MISMATCH");}},
        TableWork::Operation(o) if o.kind=="clear"=>{ensure!(count==0&&before.base==after.base,"TABLE_CLEAR_READBACK_MISMATCH");},
        TableWork::Operation(o)=>{
            verify_copy(&before,&after,target_after(work))?;
            ensure!(count==if o.kind=="copy_data"{source_count}else{0},"TABLE_COPY_ROW_COUNT_MISMATCH");
            if let Some(digest)=&digest {ensure!(mysql_digest(conn,&before,target_after(work)).await?==*digest,"TABLE_COPY_DATA_READBACK_MISMATCH");ensure!(mysql_digest(conn,&before,table).await?==*digest,"TABLE_COPY_SOURCE_DATA_CHANGED");}
            let source=mysql_snapshot(conn,kind,&entry.binding,table,false).await?;
            ensure!(source.base==before.base&&state_equal(&before,&source)?&&mysql_count(conn,kind,namespace,table).await?==source_count,"TABLE_COPY_SOURCE_CHANGED");
        },
    }
    mysql_commit(conn,entry,output).await?;
    if entry.stopping(){return Err(unknown("TABLE_MYSQL_COMMIT_STOP_RACE: 提交与停止竞态，不发布成功 receipt").into());}
    let implicit={let p=output.lock();p.ddl_applied>0||p.nontransactional};
    let transaction=if implicit{"implicit"}else if steps.is_empty(){"none"}else{"committed"};
    success_receipt(output,work,after,transaction,count);Ok(messages(started,transaction))
}

fn error_details(output:&Output,work:&TableWork,error:&AppError,transaction:&str) {
    let mut p=output.lock();p.transaction=transaction.into();p.design=None;p.operation=None;
    let mismatches=vec![error.message.clone()];
    match work {
        TableWork::Design(_)=>p.design=Some(TableDesignReceipt {receipt:DesignReceipt {steps:p.steps.clone(),verified:false,unverified:p.ddl_applied>0||p.committed,
            mismatches,transaction:transaction.into(),schema:None},snapshot:None}),
        TableWork::Operation(o)=>p.operation=Some(TableOperationReceipt {kind:o.kind.clone(),source_table:o.table.clone(),target_table:o.target_table.clone(),
            affected_rows:p.affected,copied_rows:p.copied,verified:false,mismatches,transaction:transaction.into(),steps:p.steps.clone(),snapshot:None}),
    }
}
fn failure_error(result:anyhow::Error,output:&Output,transaction:&str,code:&str)->AppError {
    let p=output.lock();
    let facts=serde_json::json!({"transaction":transaction,"steps":p.steps,"affected_rows":p.affected,"copied_rows":p.copied,
        "verified":false,"automatic_replay":false});
    let uncertain=result.downcast_ref::<AppError>().is_some_and(|e|e.code=="EXECUTION_EFFECTS_UNKNOWN");
    AppError::coded(if uncertain{"EXECUTION_EFFECTS_UNKNOWN"}else{code},format!("{result:#}\n表操作实际回执：{facts}"))
}
async fn pg_cleanup(conn:&mut sqlx::PgConnection,entry:&Entry,work:&TableWork,output:&Output,result:Result<MultiResult>)->CmdResult<MultiResult> {
    let (begun,commit_sent,committed)={let p=output.lock();(p.begun,p.commit_sent,p.committed)};
    let uncertain=result.as_ref().err().is_some_and(|e|e.downcast_ref::<AppError>().is_some_and(|e|e.code=="EXECUTION_EFFECTS_UNKNOWN"));
    if commit_sent&&!committed||uncertain {
        // COMMIT ACK 丢失或发出的 SQL 连接中断：仅关闭会话，不自动重发 COMMIT/DDL。
        entry.record_remote_transaction("unknown");
        let error=failure_error(result.err().unwrap_or_else(||anyhow!("TABLE_PG_COMMIT_STATE_UNKNOWN")),output,"unknown","EXECUTION_EFFECTS_UNKNOWN");
        error_details(output,work,&error,"unknown");return Err(unknown(error.message));
    }
    if begun {
        if let Err(e)=crate::sql_history::query("ROLLBACK").execute(&mut *conn).await {
            entry.record_remote_transaction("unknown");let error=unknown(format!("TABLE_PG_ROLLBACK_UNKNOWN: {e}"));error_details(output,work,&error,"unknown");return Err(error);
        }
        {let mut p=output.lock();p.begun=false;p.transaction="rolled_back".into();}
        entry.record_remote_transaction("rolled_back");
    }
    match result {
        Ok(value)=>Ok(value),
        Err(e)=>{
            let transaction=if committed{"committed"}else if begun{"rolled_back"}else{"none"};
            let code=if committed{"EXECUTION_EFFECTS_UNKNOWN"}else if entry.stopping(){"TABLE_NATIVE_STOPPED_ROLLED_BACK"}else{"TABLE_NATIVE_FAILED_ROLLED_BACK"};
            let error=failure_error(e,output,transaction,code);error_details(output,work,&error,transaction);Err(error)
        },
    }
}
async fn mysql_cleanup(conn:&mut sqlx::MySqlConnection,entry:&Entry,work:&TableWork,output:&Output,result:Result<MultiResult>)->CmdResult<MultiResult> {
    let (commit_sent,committed,ddl_applied,nontransactional,dml_sent,dml_active)={let p=output.lock();(p.commit_sent,p.committed,p.ddl_applied,p.nontransactional,p.dml_sent,p.dml_active)};
    let uncertain=result.as_ref().err().is_some_and(|e|e.downcast_ref::<AppError>().is_some_and(|e|e.code=="EXECUTION_EFFECTS_UNKNOWN"));
    let mut cleanup_unknown=None;
    // 即使停止，清理仍需要被轮询至 ACK；不会把未来丢弃当作取消成功。
    if !uncertain&&!(commit_sent&&!committed) {
        if let Err(e)=crate::sql_history::query("ROLLBACK").execute(&mut *conn).await {cleanup_unknown=Some(format!("TABLE_MYSQL_ROLLBACK_UNKNOWN: {e}"));}
    }
    if let Err(e)=mysql_unlock(conn,output).await {cleanup_unknown=Some(format!("TABLE_MYSQL_UNLOCK_UNKNOWN: {e:#}"));}
    let locks=output.lock().named_locks.clone();
    for lock in locks {
        match crate::sql_history::query_scalar::<_,Option<i64>>("SELECT RELEASE_LOCK(?)").bind(&lock).fetch_one(&mut *conn).await {
            Ok(Some(1))=>{},Ok(other)=>cleanup_unknown=Some(format!("TABLE_MYSQL_NAMED_UNLOCK_UNKNOWN: {other:?}")),Err(e)=>cleanup_unknown=Some(format!("TABLE_MYSQL_NAMED_UNLOCK_UNKNOWN: {e}")),
        }
    }
    if uncertain||commit_sent&&!committed||cleanup_unknown.is_some() {
        entry.record_remote_transaction("unknown");
        let error=failure_error(result.err().unwrap_or_else(||anyhow!("{}",cleanup_unknown.unwrap_or_else(||"TABLE_MYSQL_COMMIT_STATE_UNKNOWN".into()))),output,"unknown","EXECUTION_EFFECTS_UNKNOWN");
        error_details(output,work,&error,"unknown");return Err(unknown(error.message));
    }
    match result {
        Ok(value)=>Ok(value),
        Err(e)=>{
            let partial=ddl_applied>0||committed||nontransactional&&dml_sent;
            let transaction=if partial{"implicit"}else if dml_active{"rolled_back"}else{"none"};
            // remote_transaction 只接受已有枚举；隐式部分效果记 committed，receipt 明确 implicit。
            entry.record_remote_transaction(if partial{"committed"}else if dml_active{"rolled_back"}else{"none"});
            let error=failure_error(e,output,transaction,if partial{"TABLE_NATIVE_PARTIAL_APPLY"}else if entry.stopping(){"TABLE_NATIVE_STOPPED_ROLLED_BACK"}else{"TABLE_NATIVE_FAILED_ROLLED_BACK"});
            error_details(output,work,&error,transaction);Err(error)
        },
    }
}
async fn pg_control(opts:Arc<sqlx::postgres::PgConnectOptions>,target:i32)->Result<()> {
    let mut control=sqlx::PgConnection::connect_with(&opts).await?;
    let stopped=crate::sql_history::query_scalar::<_,bool>("SELECT pg_catalog.pg_cancel_backend($1)").bind(target).fetch_one(&mut control).await;
    let closed=control.close().await;let stopped=stopped?;closed?;
    ensure!(stopped,"TABLE_PG_CANCEL_NOT_CONFIRMED");Ok(())
}
async fn mysql_control(opts:Arc<sqlx::mysql::MySqlConnectOptions>,target:u64)->Result<()> {
    let mut control=sqlx::MySqlConnection::connect_with(&opts).await?;
    let stopped=crate::sql_history::query(&format!("KILL QUERY {target}")).execute(&mut control).await;
    let closed=control.close().await;stopped?;closed?;Ok(())
}
fn send<F:std::future::Future+Send>(future:F)->F {future}
async fn pg_run_pinned(pool:sqlx::PgPool,kind:DbKind,entry:Arc<Entry>,work:TableWork,output:Output)->(CmdResult<MultiResult>,bool) {
    let acquired=tokio::select! {biased;_=entry.wait_stop()=>return (Err(cancelled()),true),result=pool.acquire()=>result};
    let mut conn=match acquired {Ok(c)=>c,Err(e)=>return(Err(AppError::from(e)),true)};
    conn.close_on_drop();
    if entry.stopping()||!entry.start(){let closed=conn.close().await;return(if closed.is_ok(){Err(cancelled())}else{Err(unknown("TABLE_PG_UNSENT_CONNECTION_DRAIN_UNKNOWN"))},true);}
    let target=crate::sql_history::query_scalar::<_,i32>("SELECT pg_catalog.pg_backend_pid()").fetch_one(&mut *conn).await;
    let result=match target {
        Ok(target)=>{
            let options=pool.connect_options();
            let (result,_)=execution::drive(send(Box::pin(pg_apply(&mut conn,&kind,&entry,&work,&output))),&entry,||pg_control(options,target)).await;
            // drive 等 work 和 control 两者排空之后才清理 ROLLBACK，避免迟到 cancel 打断清理。
            pg_cleanup(&mut conn,&entry,&work,&output,result).await
        },
        Err(e)=>Err(AppError::from(e)),
    };
    if let Err(e)=conn.close().await {let error=unknown(format!("TABLE_PG_PHYSICAL_DRAIN_UNKNOWN: {e}"));error_details(&output,&work,&error,"unknown");return(Err(error),false);}
    let confirmed=result.as_ref().err().is_some_and(|e|e.code!="EXECUTION_EFFECTS_UNKNOWN");
    (result,confirmed)
}
async fn mysql_run_pinned(pool:sqlx::MySqlPool,kind:DbKind,entry:Arc<Entry>,work:TableWork,output:Output)->(CmdResult<MultiResult>,bool) {
    let acquired=tokio::select! {biased;_=entry.wait_stop()=>return (Err(cancelled()),true),result=pool.acquire()=>result};
    let mut conn=match acquired {Ok(c)=>c,Err(e)=>return(Err(AppError::from(e)),true)};
    conn.close_on_drop();
    if entry.stopping()||!entry.start(){let closed=conn.close().await;return(if closed.is_ok(){Err(cancelled())}else{Err(unknown("TABLE_MYSQL_UNSENT_CONNECTION_DRAIN_UNKNOWN"))},true);}
    let target=crate::sql_history::query_scalar::<_,u64>("SELECT CONNECTION_ID()").fetch_one(&mut *conn).await;
    let result=match target {
        Ok(target)=>{
            let options=pool.connect_options();
            let (result,_)=execution::drive(send(Box::pin(mysql_apply(&mut conn,&kind,&entry,&work,&output))),&entry,||mysql_control(options,target)).await;
            mysql_cleanup(&mut conn,&entry,&work,&output,result).await
        },
        Err(e)=>Err(AppError::from(e)),
    };
    if let Err(e)=conn.close().await {let error=unknown(format!("TABLE_MYSQL_PHYSICAL_DRAIN_UNKNOWN: {e}"));error_details(&output,&work,&error,"unknown");return(Err(error),false);}
    let confirmed=result.as_ref().err().is_some_and(|e|e.code!="EXECUTION_EFFECTS_UNKNOWN");
    (result,confirmed)
}

/// 上层已消费真实 window/owner/runtime grant；此处不 grant/register/finish，不创建平行执行引擎。
/// lease 由 supervise 所拥有的工作任务持有，Unknown 提前返回或 IPC 断开均不释放真实资源门禁。
pub(crate) async fn run(db:Arc<Db>,kind:DbKind,entry:Arc<Entry>,ordinary:OrdinaryLease,work:TableWork,read_only:bool)->BoundResult {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    let reject=|error|BoundResult {result:Err(error),confirmed:true,design:None,operation:None};
    if read_only{return reject(AppError::coded("READ_ONLY_TABLE","只读连接禁止修改表"));}
    if let Err(e)=table_workflows::validate_work(&work,&entry.binding){return reject(e);}
    if let Err(e)=scope(&kind,&entry.binding,target_work(&work).1){return reject(AppError::from(e));}
    if !matches!((db.as_ref(),pg(&kind),kind.is_mysql_family()),(Db::Postgres(_),true,_)|(Db::Mysql(_),_,true)) {
        return reject(AppError::coded("TABLE_NATIVE_ADAPTER_UNSUPPORTED","受控表工作需要匹配的原生 MySQL/PostgreSQL adapter"));
    }
    if entry.stopping(){return reject(cancelled());}
    let output=Arc::new(parking_lot::Mutex::new(Progress::default()));let report=output.clone();let running=entry.clone();
    // Arc 包装只解决 sqlx HRTB Send 推断；ordinary 仍在 task 内，绝不能放在 supervise 外层栈。
    let lease=Arc::new(parking_lot::Mutex::new(Some(ordinary)));
    let (result,confirmed)=execution::supervise(entry.clone(),async move {
        let result=match db.as_ref() {
            Db::Postgres(pool)=>send(Box::pin(pg_run_pinned(pool.clone(),kind,running,work,output))).await,
            Db::Mysql(pool)=>send(Box::pin(mysql_run_pinned(pool.clone(),kind,running,work,output))).await,
            _=>(Err(AppError::coded("TABLE_NATIVE_ADAPTER_UNSUPPORTED","原生 adapter 不匹配")),true),
        };
        lease.lock().take();result
    }).await;
    let mut p=report.lock();
    // 失败 receipt 保有 MySQL 已应用事实；现有上层可能仅在成功时展示，错误消息另含完整逐条事实。
    // Unknown 无成功 receipt，监督器后台迟到的成功不得升级既有 Unknown。
    let known=result.as_ref().err().map_or(true,|e|e.code!="EXECUTION_EFFECTS_UNKNOWN");
    let publish=known&&(result.is_err()||!entry.stopping());
    BoundResult {result,confirmed,design:if publish{p.design.take()}else{None},operation:if publish{p.operation.take()}else{None}}

    }).await
}

#[cfg(test)]
#[path="table_native_tests.rs"]
mod tests;
