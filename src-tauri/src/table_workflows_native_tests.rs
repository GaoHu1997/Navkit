//! 仅显式隔离环境启用；从不探测默认端口或读取产品连接配置。
use super::*;
use crate::execution::Registry;
use std::str::FromStr;
async fn execute(db:Arc<Db>,binding:&Binding,manual:&crate::manual_transaction::Registry,registry:&Registry,work:TableWork)->BoundResult{let mut request=crate::execution_contracts::ExecutionRequest{execution_id:String::new(),connection_id:binding.connection.clone(),namespace:binding.namespace.clone(),owner_id:binding.owner.clone(),sql:workflow::marker(&work).unwrap(),transaction_owner:None,timeout_ms:Some(30000),origin:Some("isolated-table-live".into()),execution_grant:None,cancel_before_send:false};registry.grant_request(&mut request,binding.clone()).unwrap();let entry=registry.register(&request,binding.clone(),30).unwrap();let ordinary=manual.ordinary(&[&binding.connection]).await.unwrap();run(db,entry,ordinary,work,false).await}
async fn exercise(db:Arc<Db>,namespace:String,mysql:bool){
    let binding=Binding{principal:"native-live-fixture".into(),owner:"native-live-owner".into(),connection:uuid::Uuid::new_v4().to_string(),namespace:namespace.clone(),fingerprint:uuid::Uuid::new_v4().to_string()};let manual=crate::manual_transaction::Registry::default();let registry=Registry::default();let kind=DbKind::of(if mysql{"mysql"}else{"postgresql"});
    let q=|name:&str|db::qualified(&kind,&namespace,name);let mut session=Session::acquire(&db).await.unwrap();
    // SHOW CREATE TRIGGER 的 Created 列实际返回 TIMESTAMP，DATETIME 也需独立覆盖。
    if mysql{assert_eq!(session.scalar("SELECT CAST('2026-11-13 08:09:10.123456' AS DATETIME(6))").await.unwrap(),"2026-11-13 08:09:10.123456");}
    let table=if mysql{format!("CREATE TABLE {}(id BIGINT PRIMARY KEY AUTO_INCREMENT,parent BIGINT NULL,label VARCHAR(80) COMMENT '列注释',g VARCHAR(100) GENERATED ALWAYS AS (concat(label,'tail')) STORED,CONSTRAINT positive CHECK(length(label)>0),CONSTRAINT self_parent FOREIGN KEY(parent) REFERENCES {}(id),INDEX label_index(label)) ENGINE=InnoDB COMMENT='原表注释'",q("src"),q("src"))}else{format!("CREATE TABLE {}(id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,parent BIGINT REFERENCES {}(id),serial_value BIGSERIAL,label TEXT,g TEXT GENERATED ALWAYS AS (label||'tail') STORED,CONSTRAINT positive CHECK(id>0)) WITH (fillfactor=80)",q("src"),q("src"))};session.execute(&table).await.unwrap();session.execute(&format!("CREATE TABLE {}(value TEXT)",q("audit"))).await.unwrap();
    if mysql{session.execute(&format!("CREATE TRIGGER {} AFTER INSERT ON {} FOR EACH ROW INSERT INTO {} VALUES(NEW.label)",q("src_trigger"),q("src"),q("audit"))).await.unwrap();}else{session.execute(&format!("COMMENT ON TABLE {} IS '原表注释'",q("src"))).await.unwrap();session.execute(&format!("COMMENT ON COLUMN {}.label IS '列注释'",q("src"))).await.unwrap();session.execute(&format!("CREATE INDEX raw_expression ON {}(lower(label)) WHERE parent IS NULL",q("src"))).await.unwrap();session.execute(&format!("CREATE FUNCTION {}() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN INSERT INTO {} VALUES(NEW.label); RETURN NEW; END'",q("audit_insert"),q("audit"))).await.unwrap();session.execute(&format!("CREATE TRIGGER src_trigger AFTER INSERT ON {} FOR EACH ROW EXECUTE FUNCTION {}()",q("src"),q("audit_insert"))).await.unwrap();}
    if mysql{let raw=session.rows(&format!("SHOW CREATE TRIGGER {}",q("src_trigger"))).await.unwrap();let created=get(&raw[0],"Created").unwrap();assert!(chrono::NaiveDateTime::parse_from_str(created,"%Y-%m-%d %H:%M:%S%.f").is_ok(),"{created}");}
    session.execute(&format!("INSERT INTO {}(label) VALUES('A中文'),('a中文')",q("src"))).await.unwrap();session.release(false).await.unwrap();
    let read=|table:&str|table.to_string();let ordinary=manual.ordinary(&[&binding.connection]).await.unwrap();let before=read_owned(db.clone(),binding.clone(),read("src"),false,ordinary).await.unwrap();assert_eq!(before.public.schema.columns.len(),if mysql{4}else{5});assert_eq!(before.public.objects.triggers.len(),1);assert_eq!(before.public.objects.checks.len(),1);
    // 回归 1100：新建目标不能在只锁定源表的 LOCK TABLES 模式下执行。
    let structure=workflow::TableOperation{connection_id:binding.connection.clone(),schema:namespace.clone(),table:"src".into(),kind:"copy_structure".into(),target_table:Some("structure_only".into()),base_revision:before.public.revision.clone(),source_fingerprint:binding.fingerprint.clone()};
    let result=execute(db.clone(),&binding,&manual,&registry,TableWork::Operation(structure.clone())).await;
    assert!(result.result.is_ok(),"{:?}",result.result.err());let receipt=result.operation.unwrap();assert!(receipt.verified);assert_eq!(receipt.copied_rows,0);
    let mut session=Session::acquire(&db).await.unwrap();assert_eq!(session.scalar(&format!("SELECT count(*) FROM {}",q("structure_only"))).await.unwrap(),"0");assert_eq!(session.scalar(&format!("SELECT count(*) FROM {}",q("audit"))).await.unwrap(),"2");session.release(false).await.unwrap();
    // 重名必须在发送目标 DDL 前失败，不能覆盖已有结构或行数据。
    let result=execute(db.clone(),&binding,&manual,&registry,TableWork::Operation(structure)).await;
    assert!(result.result.unwrap_err().message.contains("TABLE_COPY_TARGET_EXISTS"));assert!(result.operation.is_none());
    let operation=workflow::TableOperation{connection_id:binding.connection.clone(),schema:namespace.clone(),table:"src".into(),kind:"copy_data".into(),target_table:Some("copied".into()),base_revision:before.public.revision.clone(),source_fingerprint:binding.fingerprint.clone()};let result=execute(db.clone(),&binding,&manual,&registry,TableWork::Operation(operation)).await;assert!(result.result.is_ok(),"{:?}",result.result.err());assert!(result.operation.unwrap().verified);
    // MySQL 目录读回 int 和带引号的默认字面量；此处验证复制后可继续设计，不混入类型别名/默认值表示的归一化测试。
    let ordinary=manual.ordinary(&[&binding.connection]).await.unwrap();let before=read_owned(db.clone(),binding.clone(),read("src"),false,ordinary).await.unwrap();let mut d=before.public.design.clone();d.comment=Some("真实保存注释".into());d.checks.as_mut().unwrap().push(design::CheckDesign{name:"label_nonempty".into(),orig_name:None,expression:"length(label)>0".into(),generated_name:false});d.options=Some(if mysql{design::TableOptionsDesign{row_format:Some("DYNAMIC".into()),..Default::default()}}else{design::TableOptionsDesign{fillfactor:Some(75),..Default::default()}});d.columns.push(design::ColumnDesign{name:"added".into(),orig_name:None,data_type:(if mysql{"int"}else{"INTEGER"}).into(),length:None,scale:None,nullable:true,default_value:Some((if mysql{"'7'"}else{"7"}).into()),auto_increment:false,comment:Some("新增列注释".into()),unsigned:false,zerofill:false});let result=execute(db.clone(),&binding,&manual,&registry,TableWork::Design(d)).await;assert!(result.result.is_ok(),"{:?}",result.result.err());assert!(result.design.unwrap().receipt.verified);
    let ordinary=manual.ordinary(&[&binding.connection]).await.unwrap();let before=read_owned(db.clone(),binding.clone(),read("src"),false,ordinary).await.unwrap();let clear=workflow::TableOperation{connection_id:binding.connection.clone(),schema:namespace,table:"src".into(),kind:"clear".into(),target_table:None,base_revision:before.public.revision.clone(),source_fingerprint:binding.fingerprint.clone()};let result=execute(db.clone(),&binding,&manual,&registry,TableWork::Operation(clear)).await;assert!(result.result.is_ok(),"{:?}",result.result.err());assert!(result.operation.unwrap().verified);
}
#[tokio::test]#[ignore="仅 NAVKIT_TABLE_WORKFLOWS_MYSQL_URL 显式隔离实例；需要建库/目录/触发器权限"]
async fn mysql_isolated_live_metadata_copy_design_clear(){let url=std::env::var("NAVKIT_TABLE_WORKFLOWS_MYSQL_URL").expect("必须明确配置隔离测试实例 URL");let options=sqlx::mysql::MySqlConnectOptions::from_str(&url).unwrap();let admin=sqlx::MySqlPool::connect_with(options.clone()).await.unwrap();let namespace=format!("navkit_table_{}",uuid::Uuid::new_v4().simple());sqlx::query(&format!("CREATE DATABASE `{namespace}` CHARACTER SET utf8mb4")).execute(&admin).await.unwrap();let pool=sqlx::MySqlPool::connect_with(options.database(&namespace)).await.unwrap();exercise(Arc::new(Db::Mysql(pool.clone())),namespace.clone(),true).await;pool.close().await;sqlx::query(&format!("DROP DATABASE `{namespace}`")).execute(&admin).await.unwrap();admin.close().await;}
async fn exercise_plain_mysql(db:Arc<Db>,namespace:String){
    let binding=Binding{principal:"plain-copy-fixture".into(),owner:"plain-copy-owner".into(),connection:uuid::Uuid::new_v4().to_string(),namespace:namespace.clone(),fingerprint:uuid::Uuid::new_v4().to_string()};
    let manual=crate::manual_transaction::Registry::default();let registry=Registry::default();let kind=DbKind::of("mysql");let q=|name:&str|db::qualified(&kind,&namespace,name);
    let mut session=Session::acquire(&db).await.unwrap();
    session.execute(&format!("CREATE TABLE {} (id INT NOT NULL,grp INT DEFAULT NULL,name VARCHAR(100) DEFAULT NULL,price DECIMAL(10,2) DEFAULT NULL,created DATETIME DEFAULT NULL,PRIMARY KEY(id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci",q("items"))).await.unwrap();
    session.execute(&format!("INSERT INTO {} VALUES(1,10,'A中文',12.34,'2026-11-13 08:09:10'),(2,NULL,'a中文',NULL,NULL)",q("items"))).await.unwrap();
    // 保留旧语句的失败证据，确保实测确实覆盖截图中的 1100。
    session.execute(&format!("LOCK TABLES {} WRITE",q("items"))).await.unwrap();
    let error=session.execute(&format!("CREATE TABLE {} (id INT)",q("blocked_copy"))).await.unwrap_err();assert!(error.to_string().contains("not locked with LOCK TABLES"),"{error:#}");
    session.execute("UNLOCK TABLES").await.unwrap();session.release(false).await.unwrap();
    let ordinary=manual.ordinary(&[&binding.connection]).await.unwrap();let before=read_owned(db.clone(),binding.clone(),"items".into(),false,ordinary).await.unwrap();assert_eq!(before.public.schema.columns.len(),5);
    let structure=workflow::TableOperation{connection_id:binding.connection.clone(),schema:namespace.clone(),table:"items".into(),kind:"copy_structure".into(),target_table:Some("items_copy".into()),base_revision:before.public.revision.clone(),source_fingerprint:binding.fingerprint.clone()};
    let result=execute(db.clone(),&binding,&manual,&registry,TableWork::Operation(structure.clone())).await;assert!(result.result.is_ok(),"{:?}",result.result.err());let receipt=result.operation.unwrap();assert!(receipt.verified);assert_eq!(receipt.copied_rows,0);
    let mut session=Session::acquire(&db).await.unwrap();assert_eq!(session.scalar(&format!("SELECT count(*) FROM {}",q("items_copy"))).await.unwrap(),"0");session.execute(&format!("INSERT INTO {}(id,name) VALUES(99,'must remain')",q("items_copy"))).await.unwrap();session.release(false).await.unwrap();
    let result=execute(db.clone(),&binding,&manual,&registry,TableWork::Operation(structure)).await;assert!(result.result.unwrap_err().message.contains("TABLE_COPY_TARGET_EXISTS"));assert!(result.operation.is_none());
    let data=workflow::TableOperation{connection_id:binding.connection.clone(),schema:namespace.clone(),table:"items".into(),kind:"copy_data".into(),target_table:Some("items_with_data".into()),base_revision:before.public.revision.clone(),source_fingerprint:binding.fingerprint.clone()};
    let result=execute(db.clone(),&binding,&manual,&registry,TableWork::Operation(data)).await;assert!(result.result.is_ok(),"{:?}",result.result.err());let receipt=result.operation.unwrap();assert!(receipt.verified);assert_eq!(receipt.copied_rows,2);
    let mut session=Session::acquire(&db).await.unwrap();assert_eq!(session.digest(&before,"items").await.unwrap(),session.digest(&before,"items_with_data").await.unwrap());assert_eq!(session.scalar(&format!("SELECT count(*) FROM {}",q("items"))).await.unwrap(),"2");assert_eq!(session.scalar(&format!("SELECT name FROM {} WHERE id=99",q("items_copy"))).await.unwrap(),"must remain");assert_eq!(session.scalar(&format!("SELECT count(*) FROM {}",q("items_copy"))).await.unwrap(),"1");session.release(false).await.unwrap();
}
#[tokio::test]#[ignore="仅 NAVKIT_TABLE_WORKFLOWS_MYSQL_URL 显式隔离实例；截图同款普通表"]
async fn mysql_isolated_live_plain_copy_structure_and_data(){let url=std::env::var("NAVKIT_TABLE_WORKFLOWS_MYSQL_URL").expect("必须明确配置隔离测试实例 URL");let options=sqlx::mysql::MySqlConnectOptions::from_str(&url).unwrap();let admin=sqlx::MySqlPool::connect_with(options.clone()).await.unwrap();let namespace=format!("navkit_plain_{}",uuid::Uuid::new_v4().simple());sqlx::query(&format!("CREATE DATABASE `{namespace}` CHARACTER SET utf8mb4")).execute(&admin).await.unwrap();let pool=sqlx::MySqlPool::connect_with(options.database(&namespace)).await.unwrap();exercise_plain_mysql(Arc::new(Db::Mysql(pool.clone())),namespace.clone()).await;pool.close().await;sqlx::query(&format!("DROP DATABASE `{namespace}`")).execute(&admin).await.unwrap();admin.close().await;}
#[tokio::test]#[ignore="仅 NAVKIT_TABLE_WORKFLOWS_PG_URL 显式隔离数据库；UUID schema"]
async fn pg_isolated_live_metadata_copy_serial_identity_design_clear(){let url=std::env::var("NAVKIT_TABLE_WORKFLOWS_PG_URL").expect("必须明确配置隔离测试库 URL");let pool=sqlx::PgPool::connect(&url).await.unwrap();let namespace=format!("navkit_table_{}",uuid::Uuid::new_v4().simple());sqlx::query(&format!("CREATE SCHEMA \"{namespace}\"")).execute(&pool).await.unwrap();exercise(Arc::new(Db::Postgres(pool.clone())),namespace.clone(),false).await;sqlx::query(&format!("DROP SCHEMA \"{namespace}\" CASCADE")).execute(&pool).await.unwrap();pool.close().await;}

#[test]
fn mysql_copy_create_releases_source_only_lock(){
    let sql="CREATE TABLE `fixture`.`items_copy` (`id` INT NOT NULL, PRIMARY KEY (`id`)) ENGINE=InnoDB";
    assert!(mysql_step_requires_unlock(true,0,sql).unwrap());
    assert!(!mysql_step_requires_unlock(false,0,sql).unwrap());
    assert!(!mysql_step_requires_unlock(true,1,sql).unwrap());
}

#[test]
fn mysql_copy_dml_and_alter_keep_table_locks(){
    for sql in ["INSERT INTO `copy` (`id`) SELECT `id` FROM `src`", "DELETE FROM `src`", "ALTER TABLE `copy` AUTO_INCREMENT=9"]{
        assert!(!mysql_step_requires_unlock(true,1,sql).unwrap(),"{sql}");
        assert!(!mysql_step_requires_unlock(false,0,sql).unwrap(),"{sql}");
    }
}

#[test]
fn mysql_trigger_ddl_releases_and_reacquires_table_locks(){
    for sql in ["CREATE TRIGGER `tr` AFTER INSERT ON `copy` FOR EACH ROW SET @seen=1", "CREATE DEFINER=`root`@`localhost` TRIGGER `tr` AFTER INSERT ON `copy` FOR EACH ROW SET @seen=1", "/* boundary */ DROP TRIGGER IF EXISTS `tr`"]{
        assert!(mysql_step_requires_unlock(false,1,sql).unwrap(),"{sql}");
    }
}

#[test]
fn mysql_unlock_detection_ignores_trigger_names_and_literals(){
    for sql in ["CREATE TABLE `copy` (`trigger` INT, note VARCHAR(40) DEFAULT 'TRIGGER')", "CREATE INDEX `TRIGGER` ON `src` (`id`)", "ALTER TABLE `src` ADD COLUMN `trigger` INT"]{
        assert!(!mysql_step_requires_unlock(false,0,sql).unwrap(),"{sql}");
    }
    assert!(mysql_step_requires_unlock(true,0,"CREATE TABLE `unterminated").is_err());
}

#[test]fn mysql_implicit_receipt_uses_supported_c1_remote_states_without_panicking(){
    let binding=Binding{principal:"window".into(),owner:"owner".into(),connection:"connection".into(),namespace:"fixture".into(),fingerprint:"fingerprint".into()};let registry=Registry::default();let mut request=crate::execution_contracts::ExecutionRequest{execution_id:String::new(),connection_id:binding.connection.clone(),namespace:binding.namespace.clone(),owner_id:binding.owner.clone(),sql:"TABLE_OPERATION test-fingerprint".into(),transaction_owner:None,timeout_ms:Some(1000),origin:None,execution_grant:None,cancel_before_send:false};registry.grant_request(&mut request,binding.clone()).unwrap();let entry=registry.register(&request,binding,1).unwrap();
    for (verified,applied,uncertain,stopped,expected) in [(true,true,false,false,"committed"),(false,true,false,false,"unknown"),(false,false,false,false,"none"),(false,false,true,false,"unknown"),(true,true,false,true,"unknown")]{let state=mysql_remote_state(verified,applied,uncertain,stopped);assert_eq!(state,expected);entry.record_remote_transaction(state);}
}
