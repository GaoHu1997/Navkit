//! Exact official remaining packages, not vendor server availability.
use super::*;
#[test]
fn remaining_vendor_urls_and_package_identity_are_explicit() {
    let mut req = ConnectionRequest { kind: crate::models::DbKind::of("dremio"), host: "127.0.0.1".into(), port: 1,
        user: "navkit_probe".into(), password: "not-in-url;@\\".into(), database: String::new(), file: String::new(), params: String::new(), timeout_secs: 3 };
    for kind in ["dremio", "ignite"] {
        req.kind = crate::models::DbKind::of(kind);
        let v = vendor(kind).unwrap();
        assert!(v.kinds.contains(&kind));
        let url = connection_url(&req, &v, &[]).unwrap();
        assert!(!url.contains(&req.password));
        assert_eq!(url, if kind == "dremio" { "jdbc:dremio:direct=127.0.0.1:1" } else { "jdbc:ignite:thin://127.0.0.1:1/PUBLIC" });
        for input in ["ssl=true", "tls=true", "sslmode=require", "sslmode=prefer", "ssl_verify=false"] {
            req.params = input.into();
            assert!(params(&v, &req).and_then(|p| connection_url(&req, &v, &p.unwrap())).is_err(), "{kind}: {input} must not silently downgrade");
        }
        req.params.clear();
    }
    assert!(vendor("dremio").unwrap().pom.is_none());
    assert_eq!(dependencies::specs("ignite").len(), 6);
    assert!(!supports("ignite3"));
}
#[tokio::test]
#[ignore = "real original Dremio and Ignite artifacts; copied test exe only; set NAVKIT_JDBC_REMAINING_ROOT"]
async fn two_remaining_complete_packages_and_default_routes_are_real() {
    let portable = crate::storage::portable_root();
    assert!(portable.starts_with(std::env::temp_dir()) && portable.file_name().unwrap().to_string_lossy().starts_with("navkit-jdbc-command-tests-"));
    let root = PathBuf::from(std::env::var_os("NAVKIT_JDBC_REMAINING_ROOT").expect("original official assets root"));
    for kind in ["dremio", "ignite"] {
        let v = vendor(kind).unwrap();
        let evidence = root.join(if kind == "dremio" { "dremio-legacy" } else { "ignite" });
        let dir = package_dir_for(&v);
        package_tests::stage(&dir, &evidence, &v).unwrap();
        let jar = verify_package(&dir, &v).unwrap();
        if kind == "dremio" { assert!(!dir.join("official.pom").exists()); }
        let ready = status_at(&v, &dir, kind);
        assert_eq!(ready.status, "ready", "{kind}: {}", ready.message);
        assert_eq!(std::env::split_paths(&classpath_for(&jar).unwrap()).count(), 1 + dependencies::specs(kind).len());
        let req = ConnectionRequest { kind: crate::models::DbKind::of(kind), host: "127.0.0.1".into(), port: 1,
            user: "navkit_probe".into(), password: "remaining-secret;@\\".into(), database: String::new(), file: String::new(), params: String::new(), timeout_secs: 4 };
        let route = resolve_default(&req).unwrap().expect("actual managed default JDBC route");
        assert_eq!(route.target, jar.to_string_lossy()); assert_eq!(route.driver_class, v.class);
        let error = match crate::db::open(&req).await { Ok(_) => panic!("{kind}: closed port connected"), Err(e) => format!("{e:#}") };
        assert!(error.contains("JDBC"), "{kind}: {error}");
        for invalid in ["NoClassDefFoundError", "ClassNotFoundException", "InaccessibleObjectException", &req.password] { assert!(!error.contains(invalid), "{kind}: {error}"); }
        assert!(!error.contains("sidecar 已退出"), "{kind}: process exited instead of returning the real network failure: {error}");
        for dep in dependencies::specs(kind) {
            let target = dir.join(dep.name); let hidden = dir.join(format!("{}.missing", dep.name));
            fs::rename(&target, &hidden).unwrap();
            assert!(verify_package(&dir, &v).is_err()); assert!(classpath_for(&jar).is_err());
            fs::rename(&hidden, &target).unwrap();
        }
        fs::write(dir.join("unverified-neighbour.jar"), b"PK-fake").unwrap();
        assert!(verify_package(&dir, &v).is_err());
        fs::remove_file(dir.join("unverified-neighbour.jar")).unwrap();
        println!("{kind}: complete fixed package/runtime/default JDBC route PASS; server UNVERIFIED");
    }
}
