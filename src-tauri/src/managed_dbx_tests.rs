//! Explicit dbx originals on the self-hosted Navkit-drivers mirror; never substitute IRIS or label these as official downloads.
use super::*;
fn request(kind: &str) -> ConnectionRequest {
    ConnectionRequest { kind: crate::models::DbKind::of(kind), host: "127.0.0.1".into(), port: 1,
        user: "navkit_dbx_probe".into(), password: "dbx-probe;@中文-secret".into(), database: String::new(),
        file: String::new(), params: String::new(), timeout_secs: 5 }
}
#[test]
fn dbx_originals_keep_exact_provenance_urls_and_refuse_tls_downgrade() {
    for (kind, expected) in [("cache", "jdbc:Cache://127.0.0.1:1/USER"), ("sundb", "jdbc:sundb://127.0.0.1:1/test"), ("oscar", "jdbc:oscar://127.0.0.1:1/OSRDB")] {
        let v = vendor(kind).unwrap(); let mut req = request(kind);
        assert_eq!(connection_url(&req, &v, &[]).unwrap(), expected);
        assert!(v.jar.url.starts_with("https://raw.githubusercontent.com/GaoHu1997/Navkit-drivers/main/"));
        assert!(v.limits.contains("dbx 的固定提交 71ce03490dad3c9c3eaaa3bd62863b9a6fcdbb9a")); assert!(v.pom.is_none() && v.license.is_none());
        for bad in ["ssl=true", "tls=true", "sslmode=verify-full", "sslmode=prefer", "ssl_verify=false", "schema=ignored", "catalog=ignored"] {
            req.params = bad.into();
            assert!(params(&v, &req).and_then(|pairs| connection_url(&req, &v, &pairs.unwrap())).is_err(), "{kind}: {bad}");
        }
        req.params = "connection_string=explicit".into(); assert!(params(&v, &req).unwrap().is_none());
        for bad in ["USER;password=x", "default?ssl=true", "test/path", "db\0"] {
            req.database = bad.into(); assert!(connection_url(&req, &v, &[]).is_err());
        }
    }
    assert_ne!(vendor("cache").unwrap().class, vendor("iris").unwrap().class);
}
#[tokio::test]
#[ignore="fixed dbx original JARs, Java11/17 and actual HTTPS downloads; isolated copied executable only"]
async fn original_dbx_packages_default_routes_and_full_downloads() {
    let portable = crate::storage::portable_root();
    assert!(portable.starts_with(std::env::temp_dir()) && portable.file_name().unwrap().to_string_lossy().starts_with("navkit-jdbc-command-tests-"));
    let root = PathBuf::from(std::env::var_os("NAVKIT_DBX_DRIVER_ROOT").expect("pinned dbx original driver root"));
    assert!(std::net::TcpStream::connect_timeout(&"127.0.0.1:1".parse().unwrap(), Duration::from_millis(250)).is_err());
    for (kind, original) in [("cache", "cache/libs/CacheDB.jar"), ("sundb", "sundb/libs/sundb8-1.0.2.jar"), ("oscar", "oscar/libs/oscarJDBC8.jar")] {
        let v = vendor(kind).unwrap(); let dir = package_dir_for(&v); fs::create_dir_all(&dir).unwrap();
        let source = root.join(original); assert_eq!(digest_file(&source).unwrap(), v.jar.sha256);
        let jar = dir.join("driver.jar"); fs::copy(&source, &jar).unwrap();
        fs::write(dir.join("manifest.json"), serde_json::to_vec(&Manifest { format:1, kind:v.kind.into(), version:v.version.into(), class:v.class.into() }).unwrap()).unwrap();
        let ready = status_at(&v, &dir, kind); assert_eq!(ready.status, "ready", "{kind}: {}", ready.message);
        assert!(ready.message.contains("dbx 的固定提交")); assert!(!ready.message.contains("官方完整 JDBC 包"));
        assert_eq!(std::env::split_paths(&classpath_for(&jar).unwrap()).collect::<Vec<_>>(), [jar.clone()]);
        let saved = fs::read(&jar).unwrap(); fs::remove_file(&jar).unwrap();
        assert!(verify_package(&dir, &v).is_err()); assert!(classpath_for(&jar).is_err());
        fs::write(&jar, &saved).unwrap(); let mut corrupt = saved.clone(); corrupt[0] ^= 1; fs::write(&jar, corrupt).unwrap();
        assert!(verify_package(&dir, &v).is_err()); assert_ne!(status_at(&v, &dir, kind).status, "ready"); fs::write(&jar, saved).unwrap();
        fs::write(dir.join("unverified-neighbour.jar"), b"PK-fake").unwrap(); assert!(verify_package(&dir, &v).is_err());
        fs::remove_file(dir.join("unverified-neighbour.jar")).unwrap();
        let req = request(kind); let route = resolve_default(&req).unwrap().expect("default product managed route");
        assert_eq!(route.driver_class, v.class); assert_eq!(route.target, jar.to_string_lossy()); assert!(!route.url.contains(&req.password));
        let error = match crate::db::open(&req).await { Ok(_) => panic!("closed port must not succeed"), Err(error) => format!("{error:#}") };
        assert!(error.contains("Connection refused") || error.contains("拒绝") || error.contains("Communication link failure"), "{kind}: {error}");
        for bad in ["NoClassDefFoundError", "ClassNotFoundException", "sidecar 已退出", &req.password] { assert!(!error.contains(bad), "{kind}: {error}"); }
        let downloaded = download(kind, Some(v.version)).await.expect("complete fixed-commit original HTTPS download");
        assert_eq!(downloaded.status, "ready", "{kind}: {}", downloaded.message);
        assert!(downloaded.message.contains("dbx 的固定提交")); assert!(!downloaded.can_download);
        assert_eq!(verify_package(&dir, &v).unwrap(), jar); assert_eq!(digest_file(&source).unwrap(), v.jar.sha256);
        println!("{kind}: original DBX package/integrity/Java/default route/full HTTPS download PASS; vendor SQL/auth/TLS UNVERIFIED");
    }
}
