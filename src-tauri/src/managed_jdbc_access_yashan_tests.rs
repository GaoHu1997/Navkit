//! Real official Access file and Yashan JDBC packages through product APIs.
use super::*;
fn request(kind: &str) -> ConnectionRequest {
    ConnectionRequest { kind: crate::models::DbKind::of(kind), host: "127.0.0.1".into(), port: 1,
        user: String::new(), password: String::new(), database: String::new(), file: String::new(), params: String::new(), timeout_secs: 5 }
}
#[test]
fn file_and_network_urls_never_interpret_untrusted_driver_properties() {
    let file = std::env::temp_dir().join("navkit-access-url-test.accdb");
    let mut r = request("access"); r.file = file.to_string_lossy().into_owned(); r.params = "newDatabaseVersion=V2010".into();
    let v = vendor("access").unwrap();
    let pairs = params(&v, &r).unwrap().unwrap();
    assert_eq!(access_url(&r, &pairs).unwrap(), format!("jdbc:ucanaccess://{};immediatelyReleaseResources=true;newDatabaseVersion=V2010", r.file.replace('\\', "/")));
    for text in ["newDatabaseVersion=V1997", "newDatabaseVersion=V2010;newDatabaseVersion=V2010", "ssl=true", "schema=PUBLIC"] {
        r.params=text.into(); let parsed=params(&v,&r).unwrap().unwrap(); assert!(access_url(&r,&parsed).is_err(), "{text}");
    }
    r.file.push_str(";newDatabaseVersion=V2010"); assert!(access_url(&r,&pairs).is_err());
    r.file="relative.accdb".into(); assert!(access_url(&r,&pairs).is_err());
    let mut y=request("yashandb"); let v=vendor("yashandb").unwrap();
    assert_eq!(connection_url(&y,&v,&[]).unwrap(), "jdbc:yasdb://127.0.0.1:1/");
    y.database="yasdb".into(); assert_eq!(connection_url(&y,&v,&[]).unwrap(), "jdbc:yasdb://127.0.0.1:1/yasdb");
    for text in ["ssl=true", "tls=true", "sslmode=verify-full", "sslmode=prefer", "ssl_verify=false"] {
        y.params=text.into(); assert!(params(&v,&y).and_then(|p| connection_url(&y,&v,&p.unwrap())).is_err(), "{text}");
    }
    y.database="yasdb;ssl=false".into(); assert!(connection_url(&y,&v,&[]).is_err());
}
fn install_original(root: &Path, kind: &str) -> PathBuf {
    let v=vendor(kind).unwrap(); let dir=package_dir_for(&v); fs::create_dir_all(&dir).unwrap();
    let jar = if kind=="access" { root.join("packages/ucanaccess-5.3.0-uber-direct.jar") } else { root.join("packages/yashandb-jdbc-1.10.7.jar") };
    fs::copy(jar,dir.join("driver.jar")).unwrap();
    let pom=v.pom.as_ref().unwrap(); fs::copy(root.join("metadata").join(pom.url.rsplit('/').next().unwrap()),dir.join("official.pom")).unwrap();
    fs::write(dir.join("manifest.json"),serde_json::to_vec(&Manifest { format:1,kind:v.kind.into(),version:v.version.into(),class:v.class.into() }).unwrap()).unwrap();
    let jar=verify_package(&dir,&v).unwrap();
    assert_eq!(status_at(&v,&dir,kind).status,"ready");
    assert_eq!(std::env::split_paths(&classpath_for(&jar).unwrap()).count(),1);
    jar
}
fn contains(result: &crate::models::MultiResult, value: &str) -> bool {
    result.sets.iter().any(|set| set.rows.iter().any(|row| row.iter().any(|cell| cell.as_deref()==Some(value))))
}
#[tokio::test]
#[ignore = "real original Access/Yashan packages and Java; copied test exe only"]
async fn access_real_file_and_yashan_default_route_are_complete() {
    let portable=crate::storage::portable_root();
    assert!(portable.starts_with(std::env::temp_dir()) && portable.file_name().unwrap().to_string_lossy().starts_with("navkit-jdbc-command-tests-"));
    let root=PathBuf::from(std::env::var_os("NAVKIT_JDBC_ACCESS_YASHAN_ROOT").expect("actual official evidence"));
    let jar=install_original(&root,"access");
    let mut r=request("access"); r.file=portable.join("真实 Access O'Reilly.accdb").to_string_lossy().into_owned(); r.params="newDatabaseVersion=V2010".into(); r.timeout_secs=15;
    let route=resolve_default(&r).unwrap().expect("actual default Access JDBC"); assert_eq!(route.target,jar.to_string_lossy());
    let registry=crate::db::DbRegistry::default(); let id="access-real-file";
    crate::db::open_and_register(&registry,id,&r).await.unwrap();
    let kind=r.kind.clone();
    for sql in ["CREATE TABLE [DriverAcceptance] ([id] INTEGER PRIMARY KEY,[payload] VARCHAR(255),[nil] VARCHAR(40),[amount] DOUBLE)",
        "INSERT INTO [DriverAcceptance] VALUES(1,'中文;O''Reilly\\尾😀',NULL,12.5)",
        "INSERT INTO [DriverAcceptance] VALUES(2,'第二行',NULL,7.5)"] {
        crate::db::execute(&registry,id,sql,kind.clone()).await.unwrap();
    }
    let result=crate::db::execute(&registry,id,"SELECT [payload],[nil] FROM [DriverAcceptance] WHERE [id]=1",kind.clone()).await.unwrap();
    assert!(contains(&result,"中文;O'Reilly\\尾😀")); assert!(result.sets.iter().any(|s|s.rows.iter().any(|row|row.get(1)==Some(&None))));
    {
        let base=registry.read().get(id).cloned().unwrap(); let crate::db::Db::Http(http)=base.as_ref() else { panic!("actual JDBC adapter required") };
        let namespaces=http.containers().await.unwrap(); assert!(!namespaces.is_empty(),"file database needs a real navigable namespace");
        let mut namespace=None;
        for (name,_) in namespaces { if http.objects(&name).await.unwrap().iter().any(|n|n.eq_ignore_ascii_case("DriverAcceptance")) { namespace=Some(name);break; } }
        let namespace=namespace.expect("metadata must expose the created table");
        let page=http.rows(&namespace,"DriverAcceptance",1,1).await.unwrap(); assert_eq!(page.rows.len(),1); assert_eq!(page.rows[0][1].as_deref(),Some("第二行"));
        let zero=http.rows(&namespace,"DriverAcceptance",0,0).await.unwrap(); assert_eq!(zero.columns.len(),4); assert!(zero.rows.is_empty());
        let columns=http.table_columns(&namespace,"DriverAcceptance").await.unwrap().unwrap(); assert!(columns.iter().any(|c|c.name.eq_ignore_ascii_case("id")&&c.primary_key));
    }
    crate::db::execute(&registry,id,"UPDATE [DriverAcceptance] SET [payload]='已提交修改' WHERE [id]=2",kind.clone()).await.unwrap();
    assert!(crate::db::execute(&registry,id,"SELECT * FROM [DefinitelyMissingAcceptance]",kind.clone()).await.is_err());
    crate::db::close_with_derived(&registry,id); assert!(registry.read().is_empty());
    assert!(fs::metadata(&r.file).unwrap().len()>0); r.params.clear();
    crate::db::open_and_register(&registry,id,&r).await.unwrap();
    let persisted=crate::db::execute(&registry,id,"SELECT [payload] FROM [DriverAcceptance] WHERE [id]=2",kind.clone()).await.unwrap(); assert!(contains(&persisted,"已提交修改"));
    crate::db::execute(&registry,id,"DELETE FROM [DriverAcceptance] WHERE [id]=2",kind.clone()).await.unwrap();
    let count=crate::db::execute(&registry,id,"SELECT COUNT(*) FROM [DriverAcceptance]",kind).await.unwrap(); assert!(contains(&count,"1"));
    crate::db::close_with_derived(&registry,id);
    println!("access: real default JDBC file SQL/metadata/paging/Unicode/NULL/mutation/close/reopen PASS");
    let jar=install_original(&root,"yashandb"); let mut y=request("yashandb"); y.user="navkit_probe".into(); y.password="secret;never-in-error".into();
    let route=resolve_default(&y).unwrap().expect("actual default Yashan JDBC"); assert_eq!(route.target,jar.to_string_lossy());
    let error=match crate::db::open(&y).await { Ok(_)=>panic!("closed Yashan port connected"),Err(e)=>format!("{e:#}") };
    assert!(error.contains("JDBC")&&error.contains("08001"),"{error}");
    for forbidden in ["NoClassDefFoundError","ClassNotFoundException","sidecar 已退出",&y.password] { assert!(!error.contains(forbidden),"{error}"); }
    println!("yashandb: complete official package and default JDBC closed loopback route PASS; server UNVERIFIED");
}
