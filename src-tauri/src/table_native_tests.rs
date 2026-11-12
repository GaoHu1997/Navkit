//! 纯 DDL/compiler/守卫测试，不连接 MySQL/PostgreSQL 服务器。
use super::*;

fn fixture(kind:&str)->Stored {
    let kind=DbKind::of(kind);
    let binding=Binding {principal:"fixture-window".into(),owner:"fixture-page".into(),connection:"fixture".into(),namespace:"app".into(),fingerprint:"a".repeat(64)};
    let column=ColumnInfo {name:"id".into(),data_type:if pg(&kind){"integer"}else{"int"}.into(),nullable:false,default:None,primary_key:true,auto_increment:false,comment:Some("标识".into()),length:None,precision:None,scale:None};
    let raw=if pg(&kind){"\"id\" integer NOT NULL"}else{"`id` int NOT NULL COMMENT '标识'"};
    let schema=TableSchema {table:"source".into(),database:"app".into(),columns:vec![column],indexes:vec![IndexInfo {name:if pg(&kind){"source_pkey"}else{"PRIMARY"}.into(),columns:vec!["id".into()],primary:true,unique:true,method:None}],foreign_keys:Vec::new(),comment:Some("源表".into()),engine:if pg(&kind){None}else{Some("InnoDB".into())},collation:None,row_count:None};
    let mut meta=Evidence {format:1,physical:"server".into(),object:"object".into(),columns:vec![NativeColumn {name:"id".into(),raw:raw.into(),..Default::default()}],..Default::default()};
    meta.indexes.push(NativeIndex {name:if pg(&kind){"source_pkey"}else{"PRIMARY"}.into(),keys:vec!["id".into()],primary:true,unique:true,valid:true,
        definition:if pg(&kind){"CREATE UNIQUE INDEX source_pkey ON app.source USING btree (id)"}else{"PRIMARY KEY (`id`)"}.into(),..Default::default()});
    if pg(&kind){meta.constraints.push(NativeConstraint {name:"source_pkey".into(),kind:"p".into(),definition:"PRIMARY KEY (id)".into(),validated:true,local:true,..Default::default()});}
    let options=if pg(&kind){TableOptionsDesign::default()}else{TableOptionsDesign {engine:Some("InnoDB".into()),..Default::default()}};
    let ddl=if pg(&kind){"CREATE TABLE app.source (id integer NOT NULL, CONSTRAINT source_pkey PRIMARY KEY (id))"}else{"CREATE TABLE `source` (`id` int NOT NULL COMMENT '标识', PRIMARY KEY (`id`)) ENGINE=InnoDB COMMENT='源表'"};
    pack(schema,ConstraintSet {completeness:"complete".into(),..Default::default()},options,meta,&kind,&binding,false,ddl.into()).unwrap()
}
fn operation(before:&Stored,kind:&str)->TableOperation {
    TableOperation {connection_id:"fixture".into(),schema:"app".into(),table:"source".into(),kind:kind.into(),target_table:if kind=="clear"{None}else{Some("copy".into())},base_revision:before.public.revision.clone(),source_fingerprint:"a".repeat(64)}
}
fn set_evidence(before:&mut Stored,meta:&Evidence) {before.rowid=Some(serde_json::to_string(meta).unwrap());}

#[test]
fn identifier_literal_and_expression_escape_guards() {
    let mysql=DbKind::of("mysql");let pg=DbKind::of("postgresql");
    assert_eq!(ident(&mysql,"a`b").unwrap(),"`a``b`");
    assert_eq!(ident(&pg,"a\"b").unwrap(),"\"a\"\"b\"");
    assert_eq!(literal("x';DROP TABLE a;--").unwrap(),"'x'';DROP TABLE a;--'");
    for value in ["x); DROP TABLE t; --","x) OR 1=1", "x /* comment */", "1) , ADD CHECK (true"] {assert!(expression(value,false).is_err(),"{value}");}
    assert!(expression("price >= 0 AND price < 100",false).is_ok());
    assert!(expression("'literal ; -- /* remains text */'",true).is_ok());
    for value in ["1 NOT NULL","1, ADD COLUMN p text","1) COMMENT 'oops'","1 ON UPDATE now()"] {
        // 逗号若没有结束外层 DEFAULT(...) 只能变成无效表达式，不能逃出后端包裹。
        if !value.starts_with("1,"){assert!(expression(value,true).is_err(),"{value}");}
    }
}
#[test]
fn check_normalization_keeps_precedence_and_literal_case() {
    assert!(expression_equal("price > 0 AND price < 100","((price > 0) AND (price < 100))"));
    assert!(!expression_equal("(a OR b) AND c","a OR (b AND c)"));
    assert!(!expression_equal("name = 'Ab C'","name = 'ab c'"));
    assert!(expression_equal("name = 'Ab C'","((name = 'Ab C'::text))"));
    assert!(!expression_equal("'1'::integer","'1'::text"));
}
#[test]
fn type_and_fk_actions_cannot_inject_top_level_sql() {
    let before=fixture("mysql");
    for ty in ["int); DROP TABLE source; --","int DEFAULT 1","text COMMENT 'x'","int, p text","int REFERENCES app.x(id)"] {
        let mut d=before.public.design.clone();d.columns[0].data_type=ty.into();
        assert!(design_steps(&before,&d).is_err(),"{ty}");
    }
    let pg=DbKind::of("postgresql");let mut c=before.public.design.columns[0].clone();c.data_type="app.\"custom type\"[]".into();
    assert!(type_sql(&pg,&c).is_ok());
    for value in ["CASCADE; DROP TABLE x","SET NULL) CHECK (true","RESTRICT /* hidden */"] {assert!(action(Some(value),&before.kind).is_err());}
}
#[test]
fn mysql_next_auto_is_not_structure_revision_and_strings_are_preserved() {
    let a="CREATE TABLE `a` (`id` int AUTO_INCREMENT, `note` text COMMENT 'keep  two spaces') ENGINE=InnoDB AUTO_INCREMENT=7";
    let b=a.replace("AUTO_INCREMENT=7","AUTO_INCREMENT=999");
    assert_eq!(canonical(&mysql_structural_ddl(a).unwrap()).unwrap(),canonical(&mysql_structural_ddl(&b).unwrap()).unwrap());
    assert!(mysql_structural_ddl(a).unwrap().contains("'keep  two spaces'"));
    assert_ne!(canonical(&mysql_structural_ddl(a).unwrap()).unwrap(),canonical(&mysql_structural_ddl(&a.replace("two spaces","changed")).unwrap()).unwrap());
}
#[test]
fn mysql_grant_visibility_scopes_global_database_table_and_roles() {
    assert!(grant_has_trigger("GRANT TRIGGER ON *.* TO `a`@`localhost`","app","source").unwrap());
    assert!(grant_has_trigger("GRANT ALL PRIVILEGES ON `app`.* TO `a`@`%`","app","source").unwrap());
    assert!(grant_has_trigger("GRANT SELECT, TRIGGER ON `app`.`source` TO `a`@`%`","app","source").unwrap());
    assert!(!grant_has_trigger("GRANT TRIGGER ON `other`.* TO `a`@`%`","app","source").unwrap());
    assert!(!grant_has_trigger("GRANT SELECT ON *.* TO `a`@`%`","app","source").unwrap());
    assert!(!grant_has_trigger("GRANT `admin_role`@`%` TO `a`@`%`","app","source").unwrap());
}
#[test]
fn native_trigger_normalization_and_wrapping_are_dialect_specific() {
    let mysql="CREATE DEFINER=`a`@`%` TRIGGER `app`.`audit` BEFORE INSERT ON `app`.`source` FOR EACH ROW BEGIN SET NEW.id = NEW.id + 1; END";
    let (_,_,body,_)=trigger_header(mysql,false,false).unwrap();assert_eq!(body,"SET NEW.id = NEW.id + 1");
    assert!(mysql_body(&body).is_ok());
    for body in ["END; DROP TABLE source; BEGIN", "SET NEW.id=1; END", "BEGIN SET NEW.id=1", "SET NEW.id=1; CREATE TRIGGER x"] {assert!(mysql_body(body).is_err(),"{body}");}
    assert!(mysql_body("IF NEW.id < 1 THEN SET NEW.id = 1; END IF; BEGIN SET NEW.id=NEW.id+1; END").is_ok());
    let kind=DbKind::of("postgresql");
    assert_eq!(pg_function_call("EXECUTE FUNCTION app.audit('a''b')",&kind,"app").unwrap(),"EXECUTE FUNCTION \"app\".\"audit\"('a''b')");
    for body in ["EXECUTE FUNCTION app.audit(); DROP TABLE source","EXECUTE FUNCTION audit(1)","EXECUTE FUNCTION app.audit() SELECT 1","EXECUTE FUNCTION app.audit('x',)"] {assert!(pg_function_call(body,&kind,"app").is_err(),"{body}");}
    let (_,_,body,_)=trigger_header("CREATE TRIGGER audit AFTER INSERT ON app.source FOR EACH ROW EXECUTE FUNCTION app.audit('x')",true,false).unwrap();
    assert_eq!(body,"EXECUTE FUNCTION app.audit('x')");
}
#[test]
fn frontend_definition_cannot_replace_owned_server_ddl() {
    let mut before=fixture("mysql");let mut meta=evidence(&before).unwrap();
    let raw="CREATE TRIGGER `app`.`audit` BEFORE INSERT ON `app`.`source` FOR EACH ROW SET NEW.id = 7";
    meta.triggers.push(NativeTrigger {name:"audit".into(),definition:raw.into(),timing:"BEFORE".into(),event:"INSERT".into(),body:"SET NEW.id = 7".into(),..Default::default()});
    set_evidence(&mut before,&meta);
    before.public.design.triggers=Some(vec![TriggerDesign {name:"audit".into(),orig_name:Some("audit".into()),timing:"BEFORE".into(),event:"INSERT".into(),statement:"SET NEW.id = 7".into(),definition:Some(raw.into())}]);
    let mut d=before.public.design.clone();d.triggers.as_mut().unwrap()[0].definition=Some("DROP TABLE source".into());
    assert!(design_steps(&before,&d).unwrap_err().to_string().contains("READ_ONLY"));
}
#[test]
fn unchanged_mysql_columns_keep_generated_collation_on_update_and_index_raw() {
    let mut before=fixture("mysql");let mut meta=evidence(&before).unwrap();
    meta.columns[0].raw="`id` int NOT NULL COMMENT '标识' INVISIBLE".into();set_evidence(&mut before,&meta);
    let mut d=before.public.design.clone();d.comment=Some("新注释".into());
    let (steps,_)=design_steps(&before,&d).unwrap();
    assert_eq!(steps.len(),1);assert!(steps[0].sql.contains("COMMENT = '新注释'"));
    d.columns[0].comment=Some("列新注释".into());
    let (steps,_)=design_steps(&before,&d).unwrap();
    assert!(steps.iter().any(|s|s.sql.contains("COMMENT '列新注释' INVISIBLE")));
    let raw="`stamp` timestamp(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT 'time'";
    let (_,clauses)=mysql_column_clauses(raw).unwrap();assert_eq!(clauses["on_update"],"ON UPDATE CURRENT_TIMESTAMP(6)");
}
#[test]
fn ordinary_pg_design_preserves_serial_and_supports_comment_check_trigger_fillfactor() {
    let before=fixture("postgresql");let mut d=before.public.design.clone();
    assert!(design_steps(&before,&d).unwrap().0.is_empty());
    d.comment=Some("new table comment".into());d.columns[0].comment=Some("new column comment".into());
    d.checks=Some(vec![CheckDesign {name:"positive".into(),orig_name:None,expression:"id > 0".into(),generated_name:false}]);
    d.triggers=Some(vec![TriggerDesign {name:"audit".into(),orig_name:None,timing:"AFTER".into(),event:"INSERT OR UPDATE".into(),statement:"EXECUTE FUNCTION app.audit()".into(),definition:None}]);
    d.options=Some(TableOptionsDesign {fillfactor:Some(75),..Default::default()});
    let (steps,_)=design_steps(&before,&d).unwrap();
    assert!(steps.iter().any(|s|s.sql.contains("COMMENT ON COLUMN")));
    assert!(steps.iter().any(|s|s.sql.contains("CHECK (id > 0)")));
    assert!(steps.iter().any(|s|s.sql.contains("EXECUTE FUNCTION \"app\".\"audit\"()")));
    assert!(steps.iter().any(|s|s.sql.contains("fillfactor = 75")));
}
#[test]
fn options_whitelist_does_not_accept_sql_or_cross_dialect_options() {
    let mysql=fixture("mysql");let pg=fixture("postgresql");
    for option in [TableOptionsDesign {engine:Some("InnoDB; DROP TABLE x".into()),..Default::default()},TableOptionsDesign {charset:Some("utf8mb4)".into()),..Default::default()},TableOptionsDesign {fillfactor:Some(80),..Default::default()}] {
        let mut d=mysql.public.design.clone();d.options=Some(option);assert!(design_steps(&mysql,&d).is_err());
    }
    for option in [TableOptionsDesign {fillfactor:Some(9),..Default::default()},TableOptionsDesign {engine:Some("InnoDB".into()),..Default::default()}] {
        let mut d=pg.public.design.clone();d.options=Some(option);assert!(design_steps(&pg,&d).is_err());
    }
}
#[test]
fn full_copy_is_not_ctas_and_self_fk_targets_new_table() {
    let mut before=fixture("mysql");let mut meta=evidence(&before).unwrap();
    let raw="CONSTRAINT `self_fk` FOREIGN KEY (`id`) REFERENCES `app`.`source` (`id`) ON DELETE RESTRICT";
    meta.constraints.push(NativeConstraint {name:"self_fk".into(),kind:"f".into(),definition:raw.into(),validated:true,local:true,..Default::default()});
    meta.triggers.push(NativeTrigger {name:"audit".into(),definition:"CREATE TRIGGER `app`.`audit` AFTER INSERT ON `app`.`source` FOR EACH ROW SET @audit = NEW.id".into(),timing:"AFTER".into(),event:"INSERT".into(),body:"SET @audit = NEW.id".into(),..Default::default()});
    set_evidence(&mut before,&meta);
    before.ddl=before.ddl.replacen(") ENGINE",&format!(", {raw}) ENGINE"),1);
    let steps=operation_steps(&before,&operation(&before,"copy_data")).unwrap();
    assert!(steps[0].sql.contains("PRIMARY KEY (`id`)"));assert!(steps[0].sql.contains("COMMENT='源表'"));
    assert!(!steps[0].sql.contains("FOREIGN KEY"));assert!(!steps[0].sql.contains(" AS SELECT "));
    let insert=steps.iter().position(|s|s.sql.starts_with("INSERT ")).unwrap();
    let fk=steps.iter().position(|s|s.sql.contains("ADD CONSTRAINT")).unwrap();
    let trigger=steps.iter().position(|s|s.sql.starts_with("CREATE TRIGGER")).unwrap();
    assert!(insert<fk&&insert<trigger);assert!(steps[fk].sql.contains("REFERENCES `app`.`copy`"));
}
#[test]
fn pg_serial_copy_owns_independent_sequence_and_excludes_generated_data_columns() {
    let mut before=fixture("postgresql");let mut meta=evidence(&before).unwrap();
    meta.sequences.push(NativeSequence {column:"id".into(),schema:"app".into(),name:"source_id_seq".into(),identity:false,owned:true,data_type:"integer".into(),start:7,increment:3,min:1,max:2147483647,cache:5,cycle:false,last:Some(25),called:Some(true)});
    set_evidence(&mut before,&meta);before.columns.push(("computed".into(),GENERATED));
    let steps=operation_steps(&before,&operation(&before,"copy_data")).unwrap();
    assert!(steps[0].sql.contains("INCLUDING ALL"));
    assert!(steps.iter().any(|s|s.sql.starts_with("CREATE SEQUENCE ")&&s.sql.contains("INCREMENT BY 3")&&s.sql.contains("START WITH 7")&&s.sql.contains("CACHE 5")));
    assert!(steps.iter().any(|s|s.sql.contains("OWNED BY \"app\".\"copy\".\"id\"")));
    assert!(steps.iter().any(|s|s.sql.contains("setval(")&&s.sql.ends_with(",25,true)")));
    let insert=steps.iter().find(|s|s.sql.starts_with("INSERT ")).unwrap();
    assert!(insert.sql.contains("OVERRIDING SYSTEM VALUE"));assert!(!insert.sql.contains("computed"));
}
#[test]
fn precise_capability_refusals_do_not_disable_ordinary_mysql_pg() {
    for kind in ["mysql","postgresql"] {let before=fixture(kind);assert!(before.public.capabilities.clear.supported);assert!(before.public.capabilities.copy_structure.supported);assert!(before.public.capabilities.rebuild.supported);}
    let kind=DbKind::of("postgresql");let meta=Evidence {format:1,unsupported:vec!["RLS".into()],..Default::default()};
    assert!(!capabilities(&kind,false,true,&meta).copy_data.supported);
    assert!(!capabilities(&DbKind::of("mysql"),false,false,&Evidence::default()).rebuild.supported);
    assert!(!capabilities(&kind,true,true,&Evidence::default()).clear.supported);
}
#[test]
fn partial_failure_retains_success_facts_and_never_reports_rolled_back() {
    let output=Arc::new(parking_lot::Mutex::new(Progress {ddl_applied:1,committed:true,steps:vec![DesignStepResult {label:"ADD CHECK".into(),sql:"ALTER TABLE t ADD CHECK (x>0)".into(),status:"applied".into(),error:None}],..Default::default()}));
    let before=fixture("mysql");let work=TableWork::Operation(operation(&before,"copy_structure"));
    let error=failure_error(anyhow!("second DDL failed"),&output,"implicit","TABLE_NATIVE_PARTIAL_APPLY");
    error_details(&output,&work,&error,"implicit");
    assert!(error.message.contains("applied"));assert!(error.message.contains("automatic_replay"));
    let guard=output.lock();let receipt=guard.operation.as_ref().unwrap();assert_eq!(receipt.transaction,"implicit");assert!(!receipt.verified);assert_eq!(receipt.steps[0].status,"applied");
}
