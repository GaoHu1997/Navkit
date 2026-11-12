//! T8：以真实列类型编译敏感筛选，不把用户值是否像数字当作列类型。
use anyhow::{anyhow,ensure,Result};
use std::collections::BTreeMap;
use crate::{db::{self,Db},models::{DbKind,FilterCondition,FilterLine,PageRequest,Protocol}};
use sqlx::Row;
#[derive(Clone,Copy,Debug,PartialEq)] pub(super) enum FilterType{Text,Scalar,Unknown}
pub(super) fn column_type(ty:&str)->FilterType{
    let ty=ty.trim().to_ascii_lowercase();let base=ty.split(['(',' ']).next().unwrap_or("");
    if ["char","character","varchar","nvarchar","nchar","text","tinytext","mediumtext","longtext","clob","nclob","citext","enum","set","string"].contains(&base){FilterType::Text}
    else if ["int","integer","tinyint","smallint","mediumint","bigint","int2","int4","int8","float","float4","float8","double","real","numeric","decimal","number","boolean","bool","date","datetime","datetime2","timestamp","timestamptz","time","timetz","interval","uuid","bytea","blob","binary","varbinary","bit","varbit"].contains(&base){FilterType::Scalar}else{FilterType::Unknown}
}
fn glob_pattern(value:&str)->Result<String>{let mut out=String::new();let mut cs=value.chars();while let Some(mut c)=cs.next(){let escaped=c=='\\';if escaped{c=cs.next().ok_or_else(||anyhow!("FILTER_LIKE_ESCAPE_INVALID: LIKE 末尾转义符没有后续字符"))?;}if !escaped&&c=='%'{out.push('*');}else if !escaped&&c=='_'{out.push('?');}else{match c{'*'=>out.push_str("[*]"),'?'=>out.push_str("[?]"),'['=>out.push_str("[[]"),']'=>out.push_str("[]]"),_=>out.push(c)}}}Ok(out)}
pub(super) fn condition(kind:&DbKind,f:&FilterCondition,ty:FilterType)->Result<String>{
    let mut col=db::quote_ident(kind,&f.column);let op=f.operator.trim().to_ascii_lowercase();if op=="is null"||op=="is not null"{return Ok(format!("{col} {}",op.to_ascii_uppercase()));}
    let pattern=op=="like"||op=="not like";let equality=["=","!=","<>","in","not in"].contains(&op.as_str());
    let sql_op=match op.as_str(){"="=>"=","!="|"<>"=>"<>",">"=>">",">="=>">=","<"=>"<","<="=>"<=","like"=>"LIKE","not like"=>"NOT LIKE","in"=>"IN","not in"=>"NOT IN",_=>return Err(anyhow!("不支持的筛选运算符：{}",f.operator))};
    ensure!(ty!=FilterType::Unknown||!f.case_sensitive||!(pattern||equality),"FILTER_SENSITIVE_TYPE_UNCERTIFIED: 列 {} 的真实类型尚未认证敏感语义",f.column);
    ensure!(!pattern||ty==FilterType::Text,"FILTER_PATTERN_TEXT_REQUIRED: LIKE 需要真实文本列，不能隐式把数值列转换为文本");
    if pattern&&kind.is_sqlite()&&f.case_sensitive{return Ok(format!("{col} {} {}",if op=="not like"{"NOT GLOB"}else{"GLOB"},db::quote_literal(&glob_pattern(&f.value)?)));}
    let text=ty==FilterType::Text;
    if text&&(pattern||equality){if kind.is_pg_family(){col=format!("CAST({col} AS TEXT) COLLATE \"C\"");}
        else if kind.is_mysql_family()&&f.case_sensitive{col=format!("CONVERT({col} USING utf8mb4) COLLATE utf8mb4_bin");}
        else if kind.is_sqlite(){col=format!("{col} COLLATE {}",if f.case_sensitive{"BINARY"}else{"NOCASE"});}
        else if kind.protocol()==Protocol::Mssql&&f.case_sensitive{col=format!("{col} COLLATE Latin1_General_100_BIN2");}
        else if f.case_sensitive{return Err(anyhow!("FILTER_SENSITIVE_DIALECT_UNCERTIFIED: 此协议未认证文本敏感筛选"));}
        if !f.case_sensitive&&!kind.is_sqlite()&&!pattern{col=format!("LOWER({col})");}
    }
    let literal=|v:&str|{let quoted=if kind.is_mysql_family()&&v.contains('\\'){format!("CONVERT(0x{} USING utf8mb4)",v.as_bytes().iter().map(|b|format!("{b:02x}")).collect::<String>())}else{db::quote_literal(v)};if text&&!f.case_sensitive&&!kind.is_sqlite()&&!pattern{format!("LOWER({quoted})")}else{quoted}};
    if op=="in"||op=="not in"{let items=f.value.split(',').map(str::trim).filter(|s|!s.is_empty()).map(literal).collect::<Vec<_>>();ensure!(!items.is_empty(),"IN / NOT IN 条件需要至少一个值（逗号分隔）");return Ok(format!("{col} {sql_op} ({})",items.join(", ")));}
    let sql_op=if pattern&&kind.is_pg_family()&&!f.case_sensitive{if op=="not like"{"NOT ILIKE"}else{"ILIKE"}}else{sql_op};
    // 明确约定反斜线转义；SQLite 不改连接级 PRAGMA，PG C collation 覆盖 citext/非确定 collation。
    let escape=if pattern{if kind.is_mysql_family(){" ESCAPE 0x5c"}else{" ESCAPE '\\'"}}else{""};
    Ok(format!("{col} {sql_op} {}{escape}",literal(&f.value)))
}
pub(super) fn clause(kind:&DbKind,lines:&[FilterLine],types:&BTreeMap<String,FilterType>)->Result<String>{
    let mut items=Vec::new();for line in lines{if line.conds.is_empty(){continue;}let mut inner=String::new();for(i,c)in line.conds.iter().enumerate(){if i>0{inner.push_str(match c.logic.unwrap_or(crate::models::FilterLogic::And){crate::models::FilterLogic::And=>" AND ",crate::models::FilterLogic::Or=>" OR "});}let ty=*types.get(&c.column).ok_or_else(||anyhow!("FILTER_COLUMN_METADATA_MISSING: {}",c.column))?;inner.push_str(&condition(kind,c,ty)?);}items.push((line.logic,inner,line.conds.len()>1));}
    if items.is_empty(){return Ok(String::new());}let multi=items.len()>1;let mut out=String::new();for(i,(logic,inner,bracket))in items.iter().enumerate(){if i>0{out.push_str(match logic{crate::models::FilterLogic::And=>" AND ",crate::models::FilterLogic::Or=>" OR "});}if multi&&*bracket{out.push('(');out.push_str(inner);out.push(')');}else{out.push_str(inner);}}Ok(format!(" WHERE ({out})"))
}
pub(super) async fn typed_clause(db:&Db,kind:&DbKind,req:&PageRequest)->Result<String>{
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if req.filters.iter().all(|l|l.conds.is_empty()){return Ok(String::new());}
    let mut types=BTreeMap::new();
    if let Db::Postgres(pool)=db{
        // 递归解开 domain；citext 属文本，所有文本显式转 text/C，去除其重载比较和 nondeterministic collation。
        let rows=crate::sql_history::query("WITH RECURSIVE types AS (SELECT a.attname,t.oid,t.typbasetype,t.typcategory,t.typname,0 AS depth FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_type t ON t.oid=a.atttypid WHERE n.nspname=$1 AND c.relname=$2 AND a.attnum>0 AND NOT a.attisdropped UNION ALL SELECT x.attname,t.oid,t.typbasetype,t.typcategory,t.typname,x.depth+1 FROM types x JOIN pg_type t ON t.oid=x.typbasetype WHERE x.typbasetype<>0 AND x.depth<16) SELECT attname,typname,typcategory::text AS category FROM types WHERE typbasetype=0").bind(&req.schema).bind(&req.table).fetch_all(pool).await?;
        for r in rows{let name:String=r.try_get("attname")?;let ty:String=r.try_get("typname")?;let cat:String=r.try_get("category")?;types.insert(name,if cat=="S"{FilterType::Text}else{column_type(&ty)});}
    }else{let meta=super::table_schema(db,kind.clone(),&req.schema,&req.table).await?;for c in meta.columns{let ty=if kind.is_sqlite(){let text=c.data_type.to_ascii_uppercase();if text.contains("CHAR")||text.contains("CLOB")||text.contains("TEXT"){FilterType::Text}else if text.contains("INT")||text.contains("REAL")||text.contains("FLOA")||text.contains("DOUB")||text.contains("NUM")||text.contains("DEC"){FilterType::Scalar}else{column_type(&c.data_type)}}else{column_type(&c.data_type)};types.insert(c.name,ty);}}
    clause(kind,&req.filters,&types)

    }).await
}

#[cfg(test)]mod tests{
    use super::*;
    fn f(column:&str,operator:&str,value:&str)->FilterCondition{FilterCondition{column:column.into(),operator:operator.into(),value:value.into(),case_sensitive:true,logic:None}}
    #[tokio::test]async fn sqlite_real_nocase_ascii_chinese_in_not_in_escaped_like_and_numeric(){
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE t(x TEXT COLLATE NOCASE,n INTEGER)").execute(&pool).await.unwrap();
        for (x,n) in [("Ab中文",1),("ab中文",2),("a_b%[*]",3),("01",1)]{crate::sql_history::query("INSERT INTO t VALUES(?,?)").bind(x).bind(n).execute(&pool).await.unwrap();}
        let k=DbKind::of("sqlite");for (cond,expected) in [(f("x","=","Ab中文"),1),(f("x","!=","Ab中文"),3),(f("x","in","Ab中文,missing"),1),(f("x","not in","Ab中文,missing"),3),(f("x","like","Ab%"),1),(f("x","like",r"a\_b\%[*]"),1),(f("n","=","01"),2),(f("n","in","01"),2)]{
            let req:PageRequest=serde_json::from_value(serde_json::json!({"connection_id":"isolated","schema":"main","table":"t","offset":0,"limit":100,"filters":[{"conds":[cond]}]})).unwrap();
            let sql=format!("SELECT count(*) FROM t{}",typed_clause(&Db::Sqlite(pool.clone()),&k,&req).await.unwrap());let count:i64=crate::sql_history::query_scalar(&sql).fetch_one(&pool).await.unwrap();assert_eq!(count,expected,"{sql}");
        }
        let actual:i64=crate::sql_history::query_scalar("SELECT count(*) FROM t WHERE x LIKE 'ab%'").fetch_one(&pool).await.unwrap();assert_eq!(actual,2,"敏感筛选不能泄漏 case_sensitive_like PRAGMA");
        assert!(condition(&k,&f("n","like","1%"),FilterType::Scalar).is_err());assert!(condition(&k,&f("x","like","x\\"),FilterType::Text).is_err());pool.close().await;
    }
    #[test]fn mysql_and_pg_compile_real_metadata_types_including_citext(){
        let mysql=DbKind::of("mysql");let pg=DbKind::of("postgresql");
        for op in ["=","!=","in","not in","like"]{let sql=condition(&mysql,&f("x",op,"Ab%"),column_type("varchar(50)")).unwrap();assert!(sql.starts_with("CONVERT(`x` USING utf8mb4) COLLATE utf8mb4_bin"),"{sql}");let sql=condition(&pg,&f("x",op,"Ab%"),column_type("citext")).unwrap();assert!(sql.starts_with("CAST(\"x\" AS TEXT) COLLATE \"C\""),"{sql}");}
        for op in ["=","!=","in","not in"]{let sql=condition(&mysql,&f("n",op,"01"),column_type("int unsigned")).unwrap();assert!(!sql.contains("BINARY")&&!sql.contains("LOWER"),"{sql}");}
        assert!(condition(&pg,&f("x","=","A"),column_type("unknown_domain")).is_err());
    }
}
