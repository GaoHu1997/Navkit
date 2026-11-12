#[tokio::test]async fn sqlite_rebuild_adds_and_inserts_columns_before_table_constraints(){
    let f=Fixture::new().await;
    f.sql("CREATE TABLE parent(id INTEGER PRIMARY KEY); INSERT INTO parent VALUES(1)").await;
    f.sql("CREATE TABLE src(id INT NOT NULL,x TEXT,g TEXT GENERATED ALWAYS AS(x||' literal') STORED,PRIMARY KEY(id),CONSTRAINT kept_unique UNIQUE(x),FOREIGN KEY(id) REFERENCES parent(id)); INSERT INTO src(rowid,id,x) VALUES(42,1,'kept')").await;
    let before=f.opened("src").await;let mut d=before.public.design.clone();
    let mut added=d.columns[1].clone();added.name="added".into();added.orig_name=None;added.default_value=Some("'new default'".into());d.columns.insert(1,added);
    d.checks=Some(vec![CheckDesign{name:"positive".into(),orig_name:None,expression:"id>0".into(),generated_name:false}]);
    let (steps,rebuild)=design_steps(&before,&d).unwrap();assert!(rebuild);assert!(steps[0].sql.find("\"added\"").unwrap()<steps[0].sql.find("PRIMARY KEY").unwrap());
    let result=f.execute(TableWork::Design(d)).await;assert!(result.result.is_ok(),"{:?}",result.result.err());assert!(result.design.unwrap().receipt.verified);
    let after=f.opened("src").await;assert_eq!(after.public.schema.columns.iter().map(|c|c.name.as_str()).collect::<Vec<_>>(),vec!["id","added","x","g"]);assert_eq!(after.public.schema.foreign_keys.len(),1);assert!(after.ddl.contains("kept_unique"));
    let rows:Vec<(i64,i64,String,String,String)>=sqlx::query_as("SELECT rowid,id,added,x,g FROM src").fetch_all(&f.pool).await.unwrap();assert_eq!(rows,vec![(42,1,"new default".into(),"kept".into(),"kept literal".into())]);
    assert!(sqlx::query("INSERT INTO src(id,x) VALUES(-1,'bad')").execute(&f.pool).await.is_err());assert!(sqlx::query("INSERT INTO src(id,x) VALUES(2,'kept')").execute(&f.pool).await.is_err());f.finish().await;
}
#[tokio::test]async fn sqlite_rebuild_appended_column_and_strict_keep_table_primary_key(){
    let f=Fixture::new().await;f.sql("CREATE TABLE src(id INTEGER NOT NULL,x TEXT,PRIMARY KEY(id)); INSERT INTO src VALUES(1,'keep')").await;
    let b=f.opened("src").await;let mut d=b.public.design.clone();let mut added=d.columns[1].clone();added.name="last".into();added.orig_name=None;d.columns.push(added);d.options.as_mut().unwrap().strict=Some(true);
    let result=f.execute(TableWork::Design(d)).await;assert!(result.result.is_ok(),"{:?}",result.result.err());assert!(result.design.unwrap().receipt.verified);
    let after=f.opened("src").await;assert_eq!(after.public.options.strict,Some(true));assert_eq!(after.public.design.primary_key,vec!["id"]);assert_eq!(after.public.schema.columns.iter().map(|c|c.name.as_str()).collect::<Vec<_>>(),vec!["id","x","last"]);
    let row:(i64,String,Option<String>)=sqlx::query_as("SELECT id,x,last FROM src").fetch_one(&f.pool).await.unwrap();assert_eq!(row,(1,"keep".into(),None));f.finish().await;
}
#[tokio::test]async fn sqlite_inserted_column_without_other_changes_uses_ordered_rebuild(){
    let f=Fixture::new().await;f.sql("CREATE TABLE src(id INTEGER NOT NULL,x TEXT,PRIMARY KEY(id)); INSERT INTO src VALUES(1,'keep')").await;
    let b=f.opened("src").await;let mut d=b.public.design.clone();let mut added=d.columns[1].clone();added.name="middle".into();added.orig_name=None;d.columns.insert(1,added);
    assert!(design_steps(&b,&d).unwrap().1,"SQLite ALTER ADD would silently append a requested inserted column");
    let result=f.execute(TableWork::Design(d)).await;assert!(result.result.is_ok(),"{:?}",result.result.err());assert!(result.design.unwrap().receipt.verified);
    let after=f.opened("src").await;assert_eq!(after.public.schema.columns.iter().map(|c|c.name.as_str()).collect::<Vec<_>>(),vec!["id","middle","x"]);
    let row:(i64,Option<String>,String)=sqlx::query_as("SELECT id,middle,x FROM src").fetch_one(&f.pool).await.unwrap();assert_eq!(row,(1,None,"keep".into()));f.finish().await;
}
#[tokio::test]async fn sqlite_rebuild_rejects_new_column_shadowing_verified_hidden_rowid_before_send(){
    for (ddl,alias) in [("CREATE TABLE src(x TEXT)","rowid"),("CREATE TABLE src(rowid INTEGER,x TEXT)","_rowid_"),("CREATE TABLE src(rowid INTEGER,_rowid_ INTEGER,x TEXT)","oid")]{
        let f=Fixture::new().await;f.sql(ddl).await;f.sql(&format!("INSERT INTO src({alias},x) VALUES(42,'a'),(90,'b')")).await;
        let before=f.opened("src").await;assert_eq!(before.rowid.as_deref(),Some(alias));let original=before.ddl.clone();let mut d=before.public.design.clone();
        let mut added=d.columns.iter().find(|c|c.name=="x").unwrap().clone();added.name=alias.to_ascii_uppercase();added.orig_name=None;added.data_type="INTEGER".into();added.default_value=None;d.columns.insert(0,added);
        let error=design_steps(&before,&d).unwrap_err();assert!(error.to_string().contains("TABLE_REBUILD_ROWID_SHADOWED"),"{error}");
        let result=f.execute(TableWork::Design(d)).await;assert!(result.result.is_err());assert!(result.design.is_none());
        let after=f.opened("src").await;assert_eq!(after.ddl,original);let rows:Vec<i64>=sqlx::query_scalar(&format!("SELECT {alias} FROM src ORDER BY {alias}")).fetch_all(&f.pool).await.unwrap();assert_eq!(rows,vec![42,90]);assert_eq!(after.public.schema.columns.len(),before.public.schema.columns.len());f.finish().await;
    }
}
