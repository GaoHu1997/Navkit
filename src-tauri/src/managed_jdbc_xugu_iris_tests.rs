//! Actual pinned Xugu/IRIS artifacts and product default routes, never vendor server PASS.
use super::*;
fn request(kind: &str) -> ConnectionRequest {
    ConnectionRequest { kind: crate::models::DbKind::of(kind), host: "127.0.0.1".into(), port: 1,
        user: "navkit_local_probe".into(), password: "do-not-leak;@secret".into(), database: String::new(),
        file: String::new(), params: String::new(), timeout_secs: 12 }
}
#[test]
fn xugu_and_iris_urls_preserve_identity_and_reject_tls_downgrade() {
    for (kind, prefix, database) in [("xugu", "xugu", "SYSTEM"), ("iris", "IRIS", "USER")] {
        let v=vendor(kind).unwrap(); let mut req=request(kind);
        assert_eq!(connection_url(&req,&v,&[]).unwrap(),format!("jdbc:{prefix}://127.0.0.1:1/{database}"));
        req.database="ActualNamespace".into();
        assert!(connection_url(&req,&v,&[]).unwrap().ends_with("/ActualNamespace"));
        for bad in ["ssl=true","tls=true","sslmode=verify-full","sslmode=prefer","ssl_verify=false","schema=ignored"] {
            req.params=bad.into();
            assert!(params(&v,&req).and_then(|pairs| connection_url(&req,&v,&pairs.unwrap())).is_err(),"{kind}: {bad}");
        }
        req.params.clear();
        for bad in ["SYSTEM;password=x","USER?ssl=false","USER/path","USER\0"] {
            req.database=bad.into(); assert!(connection_url(&req,&v,&[]).is_err(),"{kind}: {bad:?}");
        }
        req.database=database.into(); req.host="127.0.0.1;SSL=0".into();
        assert!(connection_url(&req,&v,&[]).is_err());
    }
    assert_eq!(dependencies::specs("xugu").iter().map(|d|d.name).collect::<Vec<_>>(),["checker-qual-3.5.0.jar"]);
    assert!(dependencies::specs("iris").is_empty());
    let iris = vendor("iris").unwrap();
    let cache = vendor("cache").unwrap();
    assert_ne!(cache.class, iris.class, "Caché must use its own original driver, not the IRIS package");
    assert_ne!(cache.jar.sha256, iris.jar.sha256);
    assert_eq!(cache.class, "com.intersys.jdbc.CacheDriver");
}
fn stage_original(root: &Path, kind: &str) -> PathBuf {
    let v=vendor(kind).unwrap(); let dir=package_dir_for(&v); fs::create_dir_all(&dir).unwrap();
    if kind=="xugu" {
        let evidence=root.join("access-yashan-20261107");
        fs::copy(evidence.join("packages/xugu-jdbc-12.3.9.jar"),dir.join("driver.jar")).unwrap();
        fs::copy(evidence.join("metadata/xugu-jdbc-12.3.9.pom"),dir.join("official.pom")).unwrap();
        for dep in dependencies::specs(kind) {
            fs::copy(evidence.join("packages").join(dep.name),dir.join(dep.name)).unwrap();
            fs::copy(evidence.join("metadata").join(dep.pom_name),dir.join(dep.pom_name)).unwrap();
        }
    } else {
        let evidence=root.join("cloud-iris-20261107");
        fs::copy(evidence.join("artifacts/intersystems-jdbc-3.11.0.jar"),dir.join("driver.jar")).unwrap();
        fs::copy(evidence.join("evidence/intersystems-jdbc-3.11.0.pom"),dir.join("official.pom")).unwrap();
    }
    fs::write(dir.join("manifest.json"),serde_json::to_vec(&Manifest{format:1,kind:v.kind.into(),version:v.version.into(),class:v.class.into()}).unwrap()).unwrap();
    let jar=verify_package(&dir,&v).unwrap();
    assert_eq!(digest_file(&jar).unwrap(),v.jar.sha256,"original complete signed/unsigned JAR bytes unchanged");
    jar
}
#[tokio::test]
#[ignore="original official Xugu/IRIS packages and Java; copied test exe only"]
async fn xugu_complete_dependency_and_signed_iris_default_routes_are_real() {
    let portable=crate::storage::portable_root();
    assert!(portable.starts_with(std::env::temp_dir()) && portable.file_name().unwrap().to_string_lossy().starts_with("navkit-jdbc-command-tests-"));
    let root=PathBuf::from(std::env::var_os("NAVKIT_JDBC_RESEARCH_ROOT").expect("official original artifacts root"));
    for kind in ["xugu","iris"] {
        let v=vendor(kind).unwrap(); let jar=stage_original(&root,kind); let dir=jar.parent().unwrap();
        let ready=status_at(&v,dir,kind); assert_eq!(ready.status,"ready","{kind}: {}",ready.message);
        assert_eq!(std::env::split_paths(&classpath_for(&jar).unwrap()).count(),1+dependencies::specs(kind).len());
        if kind=="xugu" {
            let bare=crate::jdbc_db::driver_runtime_status(&jar,v.class,v.probe_url).unwrap_err();
            assert!(format!("{bare:#}").contains("NullnessUtil"),"bare main JAR must not pass executable dependency probe: {bare:#}");
        }
        let req=request(kind); let route=resolve_default(&req).unwrap().expect("actual default managed JDBC route");
        assert_eq!(route.target,jar.to_string_lossy()); assert_eq!(route.driver_class,v.class); assert!(!route.url.contains(&req.password));
        let error=match crate::db::open(&req).await {Ok(_)=>panic!("{kind}: closed loopback connected"),Err(error)=>format!("{error:#}")};
        assert!(error.contains("JDBC"),"{kind}: {error}");
        assert!(error.contains(if kind=="iris" {"08S01"} else {"xugu50025"}),"{kind}: not actual driver transport error: {error}");
        for invalid in ["NoClassDefFoundError","ClassNotFoundException","sidecar 已退出",&req.password] {assert!(!error.contains(invalid),"{kind}: {error}");}
        for dep in dependencies::specs(kind) {
            let file=dir.join(dep.name); let hidden=dir.join(format!("{}.missing",dep.name)); fs::rename(&file,&hidden).unwrap();
            assert!(verify_package(dir,&v).is_err()); assert!(classpath_for(&jar).is_err()); assert_ne!(status_at(&v,dir,kind).status,"ready");
            fs::rename(hidden,file).unwrap();
        }
        fs::write(dir.join("unverified-neighbour.jar"),b"PK-fake").unwrap(); assert!(verify_package(dir,&v).is_err());
        fs::remove_file(dir.join("unverified-neighbour.jar")).unwrap();
        assert_eq!(digest_file(&jar).unwrap(),v.jar.sha256);
        println!("{kind}: complete original package/dependencies/runtime/default JDBC closed loopback route PASS; server UNVERIFIED");
    }
}
