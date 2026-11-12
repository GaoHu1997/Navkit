use super::*;
use crate::{models::{ColumnInfo,IndexInfo,ForeignKeyInfo,TableSchema}, design::TableOptionsDesign, table_objects::{ConstraintInfo,ConstraintSet,TriggerInfo}, table_workflows::{TableDesignSnapshot,TableCapabilities,FeatureCapability}};
fn ident(k:&DbKind,s:&str)->String{db::quote_ident(k,s)}
fn columns(rows:&[Record])->Result<Vec<ColumnInfo>>{rows.iter().map(|r|Ok(ColumnInfo{name:get(r,"name")?.into(),data_type:get(r,"type")?.into(),nullable:get(r,"nullable")?=="1",default:opt(r,"default")?,primary_key:false,auto_increment:get(r,"auto")?=="1",comment:opt(r,"comment")?,length:None,precision:None,scale:None})).collect()}
fn json_names(r:&Record,k:&str)->Result<Vec<String>>{Ok(serde_json::from_str(get(r,k)?)?)}
pub(super) async fn snapshot(s:&mut Session,b:&Binding,table:&str,readonly:bool)->Result<Stored>{
    let k=s.kind();let ns=db::quote_literal(&b.namespace);let tn=db::quote_literal(table);let full=db::qualified(&k,&b.namespace,table);
    let (meta,ddl,mut cols,mut indexes,foreign_keys,objects,options)=if k.is_mysql_family(){
        let identity=s.rows("SELECT @@server_uuid AS server, @@hostname AS host, @@port AS port, DATABASE() AS db, @@sql_mode AS mode, @@lower_case_table_names AS names, @@session.character_set_client AS charset_client, @@session.collation_connection AS collation_connection").await?;
        ensure!(identity.len()==1&&get(&identity[0],"db")?==b.namespace,"TABLE_NAMESPACE_CHANGED");
        // SHOW GRANTS 本身不会把无 TRIGGER 权限误报为空触发器目录。
        let grants=s.rows("SHOW GRANTS FOR CURRENT_USER").await?;
        let schema_scope=format!("{}.*",ident(&k,&b.namespace));let table_scope=full.clone();
        let visible=grants.iter().flat_map(|r|r.values().flatten()).any(|g|{let upper=g.to_ascii_uppercase();let privilege=upper.split(" ON ").next().unwrap_or("");(privilege.contains("ALL PRIVILEGES")||privilege.split([',',' ']).any(|p|p=="TRIGGER"))&&(g.contains(" ON *.* TO ")||g.contains(&format!(" ON {schema_scope} TO "))||g.contains(&format!(" ON {table_scope} TO ")))});
        ensure!(visible,"TABLE_TRIGGER_VISIBILITY_UNPROVEN: 需要可证明的直接 TRIGGER/ALL 权限；角色授权目录未认证，不能发布空触发器");
        let table_rows=s.rows(&format!("SELECT TABLE_TYPE AS kind, ENGINE AS engine, TABLE_COLLATION AS collation, ROW_FORMAT AS row_format, CREATE_OPTIONS AS extra, TABLE_COMMENT AS comment, CAST(CREATE_TIME AS CHAR) AS created FROM information_schema.TABLES WHERE TABLE_SCHEMA={ns} AND TABLE_NAME={tn}")).await?;
        ensure!(table_rows.len()==1&&get(&table_rows[0],"kind")?=="BASE TABLE","TABLE_BASE_TABLE_REQUIRED");let mut tr=table_rows[0].clone();tr.insert("session_sql_mode".into(),Some(get(&identity[0],"mode")?.into()));
        mysql_session_environment(&mut tr,&identity[0])?;
        let create=s.rows(&format!("SHOW CREATE TABLE {full}")).await?;ensure!(create.len()==1,"TABLE_DEFINITION_UNKNOWN");let ddl=get(&create[0],"Create Table")?.to_string();
        let cr=s.rows(&format!("SELECT COLUMN_NAME AS name, COLUMN_TYPE AS type, IF(IS_NULLABLE='YES',1,0) AS nullable, COLUMN_DEFAULT AS `default`, IF(EXTRA LIKE '%auto_increment%',1,0) AS auto, COLUMN_COMMENT AS comment, EXTRA AS extra, GENERATION_EXPRESSION AS `generated`, COLLATION_NAME AS collation, ORDINAL_POSITION AS ordinal FROM information_schema.COLUMNS WHERE TABLE_SCHEMA={ns} AND TABLE_NAME={tn} ORDER BY ORDINAL_POSITION")).await?;
        ensure!(!cr.is_empty(),"TABLE_COLUMNS_UNKNOWN");let mut cols=columns(&cr)?;
        // COLUMN_DEFAULT 是值而非 SQL；从 SHOW CREATE 的 DEFAULT token 提取真实表达式。
        let (_,parts,_)=crate::table_copy::table_parts(&ddl)?;
        for c in &mut cols{let part=parts.iter().find(|p|crate::table_objects::tokens(p).ok().and_then(|ts|ts.first().and_then(|t|crate::table_objects::identifier(t).ok())).as_deref()==Some(c.name.as_str())).ok_or_else(||anyhow!("TABLE_COLUMN_DEFINITION_UNKNOWN"))?;c.default=default_expression(part)?;}
        let mut ix=ordered_mysql_indexes(s.rows(&format!("SHOW INDEX FROM {full}")).await?)?;for row in &mut ix{row.remove("Cardinality");}let mut indexes=BTreeMap::<String,IndexInfo>::new();
        for r in &ix{let name=get(r,"Key_name")?.to_string();let i=indexes.entry(name.clone()).or_insert(IndexInfo{name:name.clone(),columns:vec![],unique:get(r,"Non_unique")?=="0",primary:name=="PRIMARY",method:Some(get(r,"Index_type")?.into())});if let Some(col)=opt(r,"Column_name")?{i.columns.push(col);}else{ensure!(r.get("Expression").is_some_and(|v|v.is_some()),"TABLE_INDEX_EXPRESSION_UNKNOWN");}}
        let fkrows=s.rows(&format!("SELECT k.CONSTRAINT_NAME AS name,k.COLUMN_NAME AS col,k.REFERENCED_TABLE_NAME AS ref_table,k.REFERENCED_TABLE_SCHEMA AS ref_schema,k.REFERENCED_COLUMN_NAME AS ref_col,r.UPDATE_RULE AS on_update,r.DELETE_RULE AS on_delete FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r ON r.CONSTRAINT_SCHEMA=k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME=k.CONSTRAINT_NAME AND r.TABLE_NAME=k.TABLE_NAME WHERE k.TABLE_SCHEMA={ns} AND k.TABLE_NAME={tn} AND k.REFERENCED_TABLE_NAME IS NOT NULL ORDER BY k.CONSTRAINT_NAME,k.ORDINAL_POSITION")).await?;
        let mut fks=BTreeMap::<String,ForeignKeyInfo>::new();for r in &fkrows{let name=get(r,"name")?.into();let f=fks.entry(name).or_insert(ForeignKeyInfo{name:get(r,"name")?.into(),columns:vec![],ref_table:get(r,"ref_table")?.into(),ref_schema:Some(get(r,"ref_schema")?.into()),ref_columns:vec![],on_update:Some(get(r,"on_update")?.into()),on_delete:Some(get(r,"on_delete")?.into())});f.columns.push(get(r,"col")?.into());f.ref_columns.push(get(r,"ref_col")?.into());}
        let checks=crate::table_objects::extract_checks(&ddl)?;
        let triggerrows=s.rows(&format!("SELECT TRIGGER_NAME AS name,ACTION_TIMING AS timing,EVENT_MANIPULATION AS event,ACTION_STATEMENT AS body,ACTION_ORIENTATION AS orientation,ACTION_ORDER AS ordering,SQL_MODE AS mode,DEFINER AS definer,CHARACTER_SET_CLIENT AS charset,COLLATION_CONNECTION AS collation FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA={ns} AND EVENT_OBJECT_TABLE={tn} ORDER BY ACTION_TIMING,EVENT_MANIPULATION,ACTION_ORDER,TRIGGER_NAME")).await?;
        let mut raw_triggers=Vec::new();let mut triggers=Vec::new();for r in triggerrows{let mut r=r;let name=get(&r,"name")?.to_string();let raw=s.rows(&format!("SHOW CREATE TRIGGER {}",db::qualified(&k,&b.namespace,&name))).await?;ensure!(raw.len()==1,"TABLE_TRIGGER_DEFINITION_UNKNOWN");let statement=get(&raw[0],"SQL Original Statement")?.to_string();r.insert("definition".into(),Some(statement.clone()));triggers.push(TriggerInfo{name,timing:Some(get(&r,"timing")?.into()),event:Some(get(&r,"event")?.into()),statement});raw_triggers.push(r);}
        let options=TableOptionsDesign{engine:Some(get(&tr,"engine")?.into()),charset:Some(get(&tr,"collation")?.split('_').next().ok_or_else(||anyhow!("TABLE_CHARSET_UNKNOWN"))?.into()),collation:Some(get(&tr,"collation")?.into()),row_format:Some(get(&tr,"row_format")?.into()),..Default::default()};
        let objects=ConstraintSet{checks,triggers,completeness:"complete".into(),limitations:vec![],table_definition:Some(ddl.clone())};
        let innodb=s.rows(&format!("SELECT CAST(TABLE_ID AS CHAR) AS id FROM information_schema.INNODB_TABLES WHERE NAME={}",db::quote_literal(&format!("{}/{}",b.namespace,table)))).await?;ensure!(innodb.len()==1,"TABLE_MYSQL_PHYSICAL_IDENTITY_UNPROVEN: 需要真实 InnoDB TABLE_ID；分区/特殊文件名/非 InnoDB 未认证");
        let physical=crate::object_catalog::revision(&identity);let id=crate::object_catalog::revision(&(&physical,get(&innodb[0],"id")?));
        (NativeMetadata{identity:id,physical,columns:cr,indexes:ix,constraints:fkrows,triggers:raw_triggers,table:tr,sequences:vec![]},ddl,cols,indexes.into_values().collect(),fks.into_values().collect(),objects,options)
    }else{
        let identity=s.rows("SELECT current_database() AS db,(SELECT oid::text FROM pg_database WHERE datname=current_database()) AS dboid,inet_server_addr()::text AS addr,inet_server_port()::text AS port,pg_postmaster_start_time()::text AS started").await?;ensure!(identity.len()==1,"TABLE_PHYSICAL_IDENTITY_UNKNOWN");
        let tbl=s.rows(&format!("SELECT c.oid::text AS oid,c.relfilenode::text AS filenode,c.relkind::text AS kind,c.relpersistence::text AS persistence,c.relrowsecurity::text AS rls,c.relforcerowsecurity::text AS force_rls,c.relispartition::text AS partition,c.relreplident::text AS replica,(SELECT amname FROM pg_am WHERE oid=c.relam) AS access_method,c.reloptions::text AS options,c.reltablespace::text AS tablespace,(SELECT count(*)::text FROM pg_inherits WHERE inhrelid=c.oid OR inhparent=c.oid) AS inherits,(SELECT count(*)::text FROM pg_rewrite WHERE ev_class=c.oid AND rulename<>'_RETURN') AS rules,obj_description(c.oid,'pg_class') AS comment,pg_get_userbyid(c.relowner) AS owner FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname={ns} AND c.relname={tn}")).await?;
        ensure!(tbl.len()==1&&["r","p"].contains(&get(&tbl[0],"kind")?),"TABLE_BASE_TABLE_REQUIRED");let tr=tbl[0].clone();let oid=get(&tr,"oid")?;
        let cr=s.rows(&format!("SELECT a.attname AS name,format_type(a.atttypid,a.atttypmod) AS type,(NOT a.attnotnull)::int::text AS nullable,CASE WHEN a.attgenerated='' THEN pg_get_expr(ad.adbin,ad.adrelid) END AS \"default\",(a.attidentity<>'' OR COALESCE(pg_get_expr(ad.adbin,ad.adrelid) LIKE 'nextval(%',false))::int::text AS auto,col_description(a.attrelid,a.attnum) AS comment,a.attidentity::text AS identity,a.attgenerated::text AS generated,CASE WHEN a.attgenerated<>'' THEN pg_get_expr(ad.adbin,ad.adrelid) END AS expression,a.attcollation::text AS collation,a.attstorage::text AS storage,to_jsonb(a)->>'attcompression' AS compression,a.attnum::text AS ordinal,t.typtype::text AS typekind,t.typname AS typename,(SELECT json_agg(e.enumlabel ORDER BY e.enumsortorder)::text FROM pg_enum e WHERE e.enumtypid=t.oid) AS enum_labels FROM pg_attribute a JOIN pg_type t ON t.oid=a.atttypid LEFT JOIN pg_attrdef ad ON ad.adrelid=a.attrelid AND ad.adnum=a.attnum WHERE a.attrelid={oid}::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum")).await?;ensure!(!cr.is_empty(),"TABLE_COLUMNS_UNKNOWN");let cols=columns(&cr)?;
        let constraints=s.rows(&format!("SELECT conname AS name,contype::text AS kind,pg_get_constraintdef(oid,false) AS definition,condeferrable::text AS deferred,condeferred::text AS initially,convalidated::text AS validated,connoinherit::text AS noinherit,(SELECT json_agg(a.attname ORDER BY u.n)::text FROM unnest(conkey) WITH ORDINALITY u(id,n) JOIN pg_attribute a ON a.attrelid=conrelid AND a.attnum=u.id) AS columns,(SELECT relname FROM pg_class WHERE oid=confrelid) AS ref_table,(SELECT n.nspname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.oid=confrelid) AS ref_schema,(SELECT json_agg(a.attname ORDER BY u.n)::text FROM unnest(confkey) WITH ORDINALITY u(id,n) JOIN pg_attribute a ON a.attrelid=confrelid AND a.attnum=u.id) AS ref_columns,confupdtype::text AS on_update,confdeltype::text AS on_delete FROM pg_constraint WHERE conrelid={oid}::oid ORDER BY conname")).await?;
        let ix=s.rows(&format!("SELECT ci.relname AS name,i.indisprimary::int::text AS primary,i.indisunique::int::text AS unique,i.indisvalid::int::text AS valid,i.indisready::int::text AS ready,am.amname AS method,pg_get_indexdef(i.indexrelid) AS definition,obj_description(i.indexrelid,'pg_class') AS comment,(SELECT json_agg(a.attname ORDER BY u.n)::text FROM unnest(i.indkey) WITH ORDINALITY u(id,n) JOIN pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=u.id WHERE u.n<=i.indnkeyatts) AS columns,(i.indexprs IS NOT NULL OR i.indpred IS NOT NULL OR i.indnatts<>i.indnkeyatts)::int::text AS complex FROM pg_index i JOIN pg_class ci ON ci.oid=i.indexrelid JOIN pg_am am ON am.oid=ci.relam WHERE i.indrelid={oid}::oid ORDER BY ci.relname")).await?;
        let mut indexes=Vec::new();for r in &ix{indexes.push(IndexInfo{name:get(r,"name")?.into(),columns:opt(r,"columns")?.map(|v|serde_json::from_str(&v)).transpose()?.unwrap_or_default(),unique:get(r,"unique")?=="1",primary:get(r,"primary")?=="1",method:Some(get(r,"method")?.into())});}
        let mut checks=Vec::new();let mut fks=Vec::new();for r in &constraints{match get(r,"kind")?{"c"=>checks.push(ConstraintInfo{name:get(r,"name")?.into(),definition:get(r,"definition")?.into(),generated_name:false}),"f"=>fks.push(ForeignKeyInfo{name:get(r,"name")?.into(),columns:json_names(r,"columns")?,ref_table:get(r,"ref_table")?.into(),ref_schema:Some(get(r,"ref_schema")?.into()),ref_columns:json_names(r,"ref_columns")?,on_update:Some(action(get(r,"on_update")?)?.into()),on_delete:Some(action(get(r,"on_delete")?)?.into())}),_=>{}}}
        let triggerrows=s.rows(&format!("SELECT t.tgname AS name,pg_get_triggerdef(t.oid,false) AS definition,t.tgenabled::text AS enabled,t.tgfoid::text AS function,t.tgconstraint::text AS constraint,pg_get_functiondef(t.tgfoid) AS function_definition,obj_description(t.oid,'pg_trigger') AS comment FROM pg_trigger t WHERE t.tgrelid={oid}::oid AND NOT t.tgisinternal ORDER BY t.tgname")).await?;
        let triggers=triggerrows.iter().map(|r|Ok(TriggerInfo{name:get(r,"name")?.into(),timing:None,event:None,statement:get(r,"definition")?.into()})).collect::<Result<Vec<_>>>()?;
        let seq=s.rows(&format!("SELECT a.attname AS column,n.nspname AS schema,c.relname AS name,format_type(q.seqtypid,NULL) AS type,q.seqstart::text AS start,q.seqincrement::text AS increment,q.seqmax::text AS max,q.seqmin::text AS min,q.seqcache::text AS cache,q.seqcycle::text AS cycle,a.attidentity::text AS identity FROM pg_depend d JOIN pg_class c ON c.oid=d.objid AND c.relkind='S' JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_sequence q ON q.seqrelid=c.oid JOIN pg_attribute a ON a.attrelid=d.refobjid AND a.attnum=d.refobjsubid WHERE d.refobjid={oid}::oid AND d.deptype IN ('a','i') ORDER BY a.attnum")).await?;
        let options_text=opt(&tr,"options")?.unwrap_or_default();let fillfactor=options_text.trim_matches(['{','}']).split(',').find_map(|v|v.strip_prefix("fillfactor=")).map(str::parse).transpose()?;
        let options=TableOptionsDesign{fillfactor,..Default::default()};let ddl=crate::object_catalog::revision(&(&cr,&constraints,&ix,&tr,&triggerrows,&seq));
        let objects=ConstraintSet{checks,triggers,completeness:"complete".into(),limitations:vec![],table_definition:Some(serde_json::to_string(&(&cr,&constraints,&ix,&tr,&triggerrows,&seq))?)};
        let physical=crate::object_catalog::revision(&identity);let id=crate::object_catalog::revision(&(&physical,oid,get(&tr,"filenode")?));
        (NativeMetadata{identity:id,physical,columns:cr,indexes:ix,constraints,triggers:triggerrows,table:tr,sequences:seq},ddl,cols,indexes,fks,objects,options)
    };
    indexes.sort_by(|a,b|a.name.cmp(&b.name));for c in &mut cols{c.primary_key=indexes.iter().filter(|i|i.primary).any(|i|i.columns.contains(&c.name));}
    let schema=TableSchema{table:table.into(),database:b.namespace.clone(),columns:cols,indexes,foreign_keys,comment:opt(&meta.table,"comment")?,engine:options.engine.clone(),collation:options.collation.clone(),row_count:None};
    let mut modeled=objects.clone();for c in &mut modeled.checks{let ts=crate::table_objects::tokens(&c.definition)?;if ts.first().is_some_and(|t|t.word("CHECK")){let mut depth=0;let mut end=None;for t in ts.iter().skip(1){if t.symbol("("){depth+=1;}if t.symbol(")"){depth-=1;if depth==0{end=Some(t.end);break;}}}c.definition=c.definition[..end.ok_or_else(||anyhow!("TABLE_CHECK_DEFINITION_UNKNOWN"))?].into();}}
    let mut design=workflow::snapshot_design(&schema,&modeled,&options)?;
    // PG 触发器为调用函数的完整 DDL；MySQL body 使用真实 ACTION_STATEMENT。
    for t in design.triggers.as_mut().unwrap(){if let Some(r)=meta.triggers.iter().find(|r|get(r,"name").ok()==Some(t.name.as_str())){if k.is_mysql_family(){t.statement=get(r,"body")?.into();}else{t.statement=get(r,"definition")?.into();}}}
    let base=crate::object_catalog::revision(&(&ddl,&meta.columns,&meta.indexes,&meta.constraints,&meta.triggers,&meta.table,&meta.sequences));
    let revision=crate::object_catalog::revision(&(&base,&meta.identity,&meta.physical,b));design.base_revision=Some(revision.clone());design.source_fingerprint=Some(b.fingerprint.clone());
    let cap=FeatureCapability{supported:!readonly,reason:readonly.then(||"保存连接为只读".into())};
    let capabilities=TableCapabilities{checks:cap.clone(),triggers:cap.clone(),options:cap.clone(),rebuild:cap.clone(),clear:clear_capability(&k,&meta.table,readonly),copy_structure:cap.clone(),copy_data:cap,supported_options:if k.is_mysql_family(){vec!["charset","collation","row_format"]}else{vec!["fillfactor"]}.into_iter().map(str::to_string).collect(),limitations:if k.is_mysql_family(){vec!["DDL 隐式提交；失败可能部分保存，保留草稿并报告未验证".into(),"直接 TRIGGER 权限必须可证明；角色授权需单独认证".into(),"仅认证 InnoDB 真实 TABLE_ID；非 InnoDB 及更换引擎在发送 DDL 前拒绝".into()]}else{vec!["清空拒绝 RLS/FORCE RLS 或缺失策略元数据，不能用同角色可见行数证明物理清空".into(),"复制对 RLS/分区/继承/规则/排除约束及自定义类型逐项拒绝；普通表完整支持".into()]}};
    let hidden=meta.columns.iter().map(|c|Ok((get(c,"name")?.into(),if opt(c,"generated")?.is_some_and(|v|!v.is_empty()){3}else{0}))).collect::<Result<_>>()?;
    Ok(Stored{public:TableDesignSnapshot{schema,objects,options,design,revision,source_fingerprint:b.fingerprint.clone(),owner_id:b.owner.clone(),completeness:"complete".into(),capabilities},ddl,indexes:vec![],columns:hidden,rowid:None,sequence:None,base,kind:k,native:Some(meta)})
}
fn mysql_session_environment(table:&mut Record,session:&Record)->Result<()>{
    for (source,target) in [("charset_client","session_charset_client"),("collation_connection","session_collation_connection")]{
        let value=get(session,source)?;ensure!(!value.is_empty(),"TABLE_MYSQL_SESSION_ENVIRONMENT_EMPTY: {source}");
        table.insert(target.into(),Some(value.into()));
    }
    Ok(())
}
pub(super) fn require_clear_visibility(kind:&DbKind,table:&Record)->Result<()>{
    if kind.is_pg_family(){
        // 同角色可见行数不能证明物理清空；不发送任何关闭/绕过 RLS 的命令。
        ensure!(get(table,"rls")?=="false"&&get(table,"force_rls")?=="false","TABLE_CLEAR_PG_RLS_UNCERTIFIED: RLS/FORCE RLS 不能通过当前角色的行数证明物理清空");
    }
    Ok(())
}
fn clear_capability(kind:&DbKind,table:&Record,readonly:bool)->FeatureCapability{
    let reason=if readonly{Some("保存连接为只读".into())}else{require_clear_visibility(kind,table).err().map(|e|format!("清空表不可用：{e}"))};
    FeatureCapability{supported:reason.is_none(),reason}
}
fn action(s:&str)->Result<&'static str>{Ok(match s{"a"=>"NO ACTION","r"=>"RESTRICT","c"=>"CASCADE","n"=>"SET NULL","d"=>"SET DEFAULT",_=>return Err(anyhow!("TABLE_FOREIGN_KEY_ACTION_UNKNOWN"))})}
fn ordered_mysql_indexes(rows:Vec<Record>)->Result<Vec<Record>>{let mut indexed=rows.into_iter().map(|r|Ok((get(&r,"Key_name")?.to_string(),get(&r,"Seq_in_index")?.parse::<usize>()?,r))).collect::<Result<Vec<_>>>()?;indexed.sort_by(|a,b|(&a.0,a.1).cmp(&(&b.0,b.1)));let mut previous=None;let mut expected=0;for(name,sequence,_)in &indexed{if previous!=Some(name){previous=Some(name);expected=1;}ensure!(*sequence==expected,"TABLE_INDEX_ORDINAL_PARTIAL: 索引列顺序目录缺失或重复");expected+=1;}Ok(indexed.into_iter().map(|(_,_,r)|r).collect())}
fn default_expression(part:&str)->Result<Option<String>>{
    let ts=crate::table_objects::tokens(part)?;let Some(i)=ts.iter().position(|t|t.word("DEFAULT"))else{return Ok(None)};let first=ts.get(i+1).ok_or_else(||anyhow!("TABLE_DEFAULT_UNKNOWN"))?;let mut end=first.end;
    if first.kind==crate::table_objects::TokenKind::Word&&(first.text.starts_with('_')||first.text.eq_ignore_ascii_case("b")||first.text.eq_ignore_ascii_case("x")){if let Some(literal)=ts.get(i+2).filter(|t|t.kind==crate::table_objects::TokenKind::Literal){end=literal.end;}}
    if first.symbol("(")||ts.get(i+2).is_some_and(|t|t.symbol("(")){let begin=if first.symbol("("){i+1}else{i+2};let mut depth=0;for t in ts.iter().skip(begin){if t.symbol("("){depth+=1;}if t.symbol(")"){depth-=1;if depth==0{end=t.end;break;}}}ensure!(depth==0,"TABLE_DEFAULT_UNBALANCED");}
    Ok(Some(part[first.start..end].into()))
}

#[cfg(test)]mod tests{use super::*;
    #[test]fn mysql_session_metadata_requires_real_charset_and_collation(){
        let source:Record=serde_json::from_value(serde_json::json!({"charset_client":"utf8mb4","collation_connection":"utf8mb4_0900_ai_ci"})).unwrap();
        let mut table=Record::new();mysql_session_environment(&mut table,&source).unwrap();assert_eq!(get(&table,"session_charset_client").unwrap(),"utf8mb4");assert_eq!(get(&table,"session_collation_connection").unwrap(),"utf8mb4_0900_ai_ci");
        for field in ["charset_client","collation_connection"]{
            let mut invalid=source.clone();invalid.remove(field);assert!(mysql_session_environment(&mut Record::new(),&invalid).is_err(),"missing {field}");
            invalid.insert(field.into(),None);assert!(mysql_session_environment(&mut Record::new(),&invalid).is_err(),"null {field}");
            invalid.insert(field.into(),Some(String::new()));assert!(mysql_session_environment(&mut Record::new(),&invalid).is_err(),"empty {field}");
        }
    }
    #[test]fn pg_clear_capability_refuses_rls_force_rls_and_unknown_metadata(){
        let pg=DbKind::of("postgresql");let baseline:Record=serde_json::from_value(serde_json::json!({"rls":"false","force_rls":"false"})).unwrap();assert!(clear_capability(&pg,&baseline,false).supported);
        assert!(!clear_capability(&pg,&baseline,true).supported);
        for field in ["rls","force_rls"]{
            for value in [Some("true"),Some(""),None]{let mut table=baseline.clone();table.insert(field.into(),value.map(str::to_string));let capability=clear_capability(&pg,&table,false);assert!(!capability.supported);assert!(capability.reason.as_deref().unwrap().contains("清空表不可用"));}
            let mut table=baseline.clone();table.remove(field);let capability=clear_capability(&pg,&table,false);assert!(!capability.supported);assert!(capability.reason.as_deref().unwrap().contains(field));
        }
        let mut rls=baseline.clone();rls.insert("rls".into(),Some("true".into()));assert!(clear_capability(&pg,&rls,false).reason.unwrap().contains("RLS/FORCE RLS"));
    }
    #[test]fn mysql_primary_key_metadata_uses_real_ordinal_and_rejects_partial_rows(){let row=|name:&str,seq:&str|{let mut r=Record::new();r.insert("Key_name".into(),Some("PRIMARY".into()));r.insert("Seq_in_index".into(),Some(seq.into()));r.insert("Column_name".into(),Some(name.into()));r};let sorted=ordered_mysql_indexes(vec![row("b","2"),row("a","1")]).unwrap();assert_eq!(get(&sorted[0],"Column_name").unwrap(),"a");assert_eq!(get(&sorted[1],"Column_name").unwrap(),"b");assert!(ordered_mysql_indexes(vec![row("b","2")]).is_err());}
    #[test]fn mysql_default_metadata_keeps_bit_charset_literals_and_function_precision(){
        for (column,expected) in [("flag bit(4) DEFAULT b'1010'","b'1010'"),("label varchar(30) DEFAULT _utf8mb4'中文'","_utf8mb4'中文'"),("created timestamp(6) DEFAULT CURRENT_TIMESTAMP(6)","CURRENT_TIMESTAMP(6)"),("amount int DEFAULT -12","-12"),("label text DEFAULT 'src literal' COMMENT 'kept'","'src literal'")]{assert_eq!(default_expression(column).unwrap().as_deref(),Some(expected));}
    }
}
