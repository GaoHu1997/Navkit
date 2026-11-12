//! SQLite 完整 CREATE 文本的受限变换；从不使用 CTAS 或按简化元数据伪造完整结构。
use anyhow::{anyhow, ensure, Result};
use crate::{design::{CheckDesign, TriggerDesign}, table_objects::{tokens, identifier, TokenKind}};

pub(crate) fn q(name: &str) -> Result<String> {
    ensure!(!name.is_empty() && name.len() <= 1024 && !name.contains('\0'), "TABLE_NAME_INVALID");
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}
fn patches(sql: &str, mut edits: Vec<(usize, usize, String)>) -> Result<String> {
    edits.sort_by_key(|e| e.0);
    ensure!(edits.windows(2).all(|e| e[0].1 <= e[1].0), "TABLE_DDL_OVERLAPPING_TRANSFORM");
    let mut out=String::new(); let mut previous=0;
    for (start,end,replacement) in edits { out.push_str(&sql[previous..start]); out.push_str(&replacement); previous=end; }
    out.push_str(&sql[previous..]); Ok(out)
}
/// 仅改真实 CREATE 的对象身份和明确 table-reference 位置。字符串、注释、列名不做全局替换。
pub(crate) fn rename_object(sql: &str, object_kind: &str, new_object: &str, source: &str, target: &str, copy: bool) -> Result<String> {
    let ts=tokens(sql)?;
    ensure!(ts.first().is_some_and(|t|t.word("CREATE")), "TABLE_DDL_CREATE_REQUIRED");
    let ki=ts.iter().take(4).position(|t|t.word(object_kind)).ok_or_else(||anyhow!("TABLE_DDL_KIND_UNKNOWN"))?;
    ensure!(ki==1 || object_kind=="INDEX" && ki==2 && ts[1].word("UNIQUE"), "TABLE_DDL_UNSUPPORTED_HEADER");
    let ni=ki+1; let old=ts.get(ni).ok_or_else(||anyhow!("TABLE_DDL_NAME_UNKNOWN"))?;
    ensure!(!old.word("IF"), "TABLE_DDL_CONDITIONAL_UNSUPPORTED");
    let _=identifier(old)?;
    let mut edits=vec![(old.start,old.end,q(new_object)?)];
    let mut recognized=std::collections::BTreeSet::new(); recognized.insert(ni);
    if object_kind!="TABLE" {
        let on=ts.iter().enumerate().skip(ni+1).find(|(_,t)|t.word("ON")).map(|(i,_)|i).ok_or_else(||anyhow!("TABLE_DDL_PARENT_UNKNOWN"))?;
        let parent=ts.get(on+1).ok_or_else(||anyhow!("TABLE_DDL_PARENT_UNKNOWN"))?;
        ensure!(identifier(parent)?==source, "TABLE_DDL_PARENT_MISMATCH");
        edits.push((parent.start,parent.end,q(target)?)); recognized.insert(on+1);
    }
    for i in ni+1..ts.len() {
        let t=&ts[i];
        if t.kind==TokenKind::Literal || t.kind==TokenKind::Symbol || recognized.contains(&i) { continue; }
        if identifier(t).ok().as_deref()!=Some(source) { continue; }
        let context=ts.get(i.wrapping_sub(1));
        if context.is_some_and(|p|p.word("REFERENCES") || object_kind=="TRIGGER" && ["INTO","UPDATE","FROM","JOIN"].iter().any(|w|p.word(w))) {
            edits.push((t.start,t.end,q(target)?)); recognized.insert(i);
        } else if copy && object_kind=="TRIGGER" {
            ensure!(source==target, "TABLE_TRIGGER_COPY_REFERENCE_UNKNOWN: 触发器内存在无法证明是表引用的同名 token；拒绝完整复制");
        }
    }
    patches(sql,edits)
}

pub(crate) fn expression(expr: &str) -> Result<String> {
    ensure!(!expr.trim().is_empty() && expr.len()<=65536 && !expr.contains('\0'), "TABLE_CHECK_EXPRESSION_INVALID");
    let ts=tokens(expr)?; let mut depth=0;
    for t in &ts { ensure!(!t.symbol(";"), "TABLE_CHECK_STATEMENT_FORBIDDEN"); if t.symbol("("){depth+=1;} if t.symbol(")"){depth-=1;ensure!(depth>=0,"TABLE_CHECK_EXPRESSION_UNBALANCED");} }
    ensure!(depth==0,"TABLE_CHECK_EXPRESSION_UNBALANCED"); Ok(expr.trim().to_string())
}
pub(crate) fn check_expression(definition: &str) -> Result<String> {
    let ts=tokens(definition)?;
    if ts.first().is_some_and(|t|t.word("CHECK")) {
        ensure!(ts.get(1).is_some_and(|t|t.symbol("(")) && ts.last().is_some_and(|t|t.symbol(")")),"TABLE_CHECK_METADATA_UNKNOWN");
        return expression(&definition[ts[1].end..ts.last().unwrap().start]);
    }
    expression(definition)
}
/// 分开顶层列/约束，允许精确保留未建模的 UNIQUE、生成列、COLLATE、FK、STRICT 等。
pub(crate) fn table_parts(sql:&str)->Result<(String,Vec<String>,String)> {
    let ts=tokens(sql)?; ensure!(ts.first().is_some_and(|t|t.word("CREATE")) && ts.get(1).is_some_and(|t|t.word("TABLE")),"TABLE_REBUILD_NOT_ORDINARY_TABLE");
    let open=ts.iter().position(|t|t.symbol("(")).ok_or_else(||anyhow!("TABLE_REBUILD_DEFINITION_UNKNOWN"))?;
    ensure!(open==3,"TABLE_REBUILD_HEADER_UNSUPPORTED");
    let mut depth=1;let mut start=ts[open].end;let mut parts=Vec::new();let mut close=None;
    for t in ts.iter().skip(open+1) { if t.symbol("("){depth+=1;} if t.symbol(")"){depth-=1;if depth==0{parts.push(sql[start..t.start].trim().to_string());close=Some(t);break;}} if depth==1&&t.symbol(","){parts.push(sql[start..t.start].trim().to_string());start=t.end;} }
    let close=close.ok_or_else(||anyhow!("TABLE_REBUILD_UNBALANCED"))?;
    ensure!(!parts.is_empty()&&parts.iter().all(|p|!p.is_empty()),"TABLE_REBUILD_PARTS_UNKNOWN");
    Ok((sql[..ts[open].end].to_string(),parts,sql[close.start..].to_string()))
}
pub(crate) fn with_checks(sql:&str, checks:&[CheckDesign])->Result<String> {
    let (prefix,parts,suffix)=table_parts(sql)?; let mut output=Vec::new();
    for part in parts {
        let ts=tokens(&part)?; let mut edits=Vec::new(); let mut i=0;
        while i<ts.len() {
            if !ts[i].word("CHECK"){i+=1;continue;}
            let begin=if i>=2 && ts[i-2].word("CONSTRAINT"){i-2}else{i};
            ensure!(ts.get(i+1).is_some_and(|t|t.symbol("(")),"TABLE_CHECK_METADATA_UNKNOWN");
            let mut depth=0;let mut end=None;
            for j in i+1..ts.len(){if ts[j].symbol("("){depth+=1;}if ts[j].symbol(")"){depth-=1;if depth==0{end=Some(j);break;}}}
            let end=end.ok_or_else(||anyhow!("TABLE_CHECK_METADATA_UNKNOWN"))?;
            edits.push((ts[begin].start,ts[end].end,String::new()));i=end+1;
        }
        let preserved=patches(&part,edits)?;
        if !preserved.trim().is_empty(){output.push(preserved.trim().to_string());}
    }
    let mut names=std::collections::BTreeSet::new();
    for check in checks {ensure!(names.insert(check.name.clone()),"TABLE_CHECK_DUPLICATE_NAME");let expr=expression(&check.expression)?;output.push(if check.generated_name{format!("CHECK ({expr})")}else{format!("CONSTRAINT {} CHECK ({expr})",q(&check.name)?)});}
    Ok(format!("{}{}{}",prefix,output.join(", "),suffix))
}
pub(crate) fn trigger_sql(table:&str,trigger:&TriggerDesign)->Result<String> {
    let timing=trigger.timing.to_ascii_uppercase();let event=trigger.event.to_ascii_uppercase();
    ensure!(["BEFORE","AFTER"].contains(&timing.as_str()),"TABLE_TRIGGER_TIMING_UNSUPPORTED");
    ensure!(["INSERT","UPDATE","DELETE"].contains(&event.as_str()),"TABLE_TRIGGER_EVENT_UNSUPPORTED");
    let ts=tokens(&trigger.statement)?;ensure!(!ts.is_empty()&&trigger.statement.len()<=1024*1024,"TABLE_TRIGGER_BODY_INVALID");
    ensure!(["INSERT","UPDATE","DELETE","SELECT"].iter().any(|w|ts[0].word(w)),"TABLE_TRIGGER_BODY_DML_REQUIRED");
    let forbidden=["CREATE","DROP","ALTER","ATTACH","DETACH","PRAGMA","VACUUM","BEGIN","COMMIT","ROLLBACK","SAVEPOINT","RELEASE","END"];
    ensure!(!ts.iter().any(|t|forbidden.iter().any(|w|t.word(w))),"TABLE_TRIGGER_BODY_UNSUPPORTED_TOKEN: 只允许由后端包装的简单 DML body");
    Ok(format!("CREATE TRIGGER {} {timing} {event} ON {} FOR EACH ROW BEGIN {}; END",q(&trigger.name)?,q(table)?,trigger.statement.trim().trim_end_matches(';')))
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn raw_structure_keeps_generated_unique_options_and_literals(){let ddl="CREATE TABLE src(id INTEGER PRIMARY KEY, x TEXT COLLATE NOCASE UNIQUE CHECK(length(x)>0), g TEXT GENERATED ALWAYS AS (x||'src') STORED) STRICT";let new=rename_object(ddl,"TABLE","copy","src","copy",true).unwrap();assert!(new.contains("GENERATED ALWAYS"));assert!(new.contains("'src'"));assert!(new.ends_with("STRICT"));let changed=with_checks(&new,&[CheckDesign{name:"positive".into(),orig_name:None,expression:"id>0".into(),generated_name:false}]).unwrap();assert!(changed.contains("COLLATE NOCASE UNIQUE"));assert!(changed.contains("CHECK (id>0)"));assert!(!changed.contains("length(x)"));}
    #[test] fn rejects_statement_escape_and_preserves_quoted_names(){assert!(expression("x); DROP TABLE victim; --").is_err());let ddl="CREATE TABLE \"a\"\"b\"(x TEXT DEFAULT 'a\"b', p INTEGER REFERENCES \"a\"\"b\"(x))";let copy=rename_object(ddl,"TABLE","target","a\"b","target",true).unwrap();assert!(copy.contains("REFERENCES \"target\""));assert!(copy.contains("DEFAULT 'a\"b'"));}
}
