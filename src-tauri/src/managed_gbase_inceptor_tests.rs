//! Original vendor ZIP/JAR, real user-click download, and actual default product routes.
use super::*;
fn request(kind: &str) -> ConnectionRequest {
    ConnectionRequest { kind: crate::models::DbKind::of(kind), host: "127.0.0.1".into(), port: 1,
        user: "navkit_vendor_probe".into(), password: "do-not-leak;@vendor-secret".into(), database: String::new(),
        file: String::new(), params: String::new(), timeout_secs: 5 }
}
#[test]
fn gbase_and_inceptor_keep_product_identity_and_reject_unverified_extensions() {
    for (kind, expected) in [("gbase", "jdbc:gbase://127.0.0.1:1/test?useSSL=false&connectTimeout=5000&socketTimeout=5000"),
                              ("inceptor", "jdbc:inceptor2://127.0.0.1:1/default;auth=noSasl")] {
        let v = vendor(kind).unwrap(); let mut req = request(kind);
        assert_eq!(connection_url(&req, &v, &[]).unwrap(), expected);
        assert!(dependencies::specs(kind).is_empty());
        for bad in ["ssl=true", "tls=true", "sslmode=verify-full", "sslmode=prefer", "ssl_verify=false", "schema=ignored", "catalog=ignored"] {
            req.params = bad.into();
            assert!(params(&v, &req).and_then(|pairs| connection_url(&req, &v, &pairs.unwrap())).is_err(), "{kind}: {bad}");
        }
        for explicit in ["auth=kerberos", "transportMode=http", "kerberosPrincipal=fake", "zooKeeperNamespace=fake"] {
            req.params = explicit.into(); assert!(params(&v, &req).unwrap().is_none(), "preserve custom options, never silently ignore {explicit}");
        }
        req.params.clear();
        for bad in ["test;password=x", "default?ssl=true", "test/path", "db\0"] {
            req.database = bad.into(); assert!(connection_url(&req, &v, &[]).is_err());
        }
        req.database = "actual_database".into();
        assert!(connection_url(&req, &v, &[]).unwrap().contains("/actual_database"));
        req.host = "127.0.0.1;ssl=false".into(); assert!(connection_url(&req, &v, &[]).is_err());
    }
    assert!(!supports("gbase8s")); assert!(!supports("gbase8c"));
    assert_ne!(vendor("argodb").unwrap().class, vendor("inceptor").unwrap().class);
    assert_ne!(vendor("argodb").unwrap().jar.sha256, vendor("inceptor").unwrap().jar.sha256);
}
fn manifest(dir: &Path, v: &Vendor) {
    fs::write(dir.join("manifest.json"), serde_json::to_vec(&Manifest { format:1, kind:v.kind.into(), version:v.version.into(), class:v.class.into() }).unwrap()).unwrap();
}
fn stage(root: &Path, v: &Vendor) -> PathBuf {
    let dir = package_dir_for(v); fs::create_dir_all(&dir).unwrap();
    if v.kind == "gbase" {
        fs::copy(root.join("packages/gbase-connector-java-9.5.0.10-build1.zip"), dir.join("official.zip")).unwrap();
        let inventory = root.join("inspection/gbase8a-official");
        fs::copy(inventory.join("00001-gbase-connector-java-9.5.0.10-build1_gbase-connector-java-9.5.0.10-build1-bin.jar"), dir.join("driver.jar")).unwrap();
        fs::copy(inventory.join("00002-gbase-connector-java-9.5.0.10-build1_readme.txt"), dir.join("release-readme.txt")).unwrap();
    } else {
        fs::copy(root.join("packages/inceptor-driver-8.37.3-official-navigation.jar"), dir.join("driver.jar")).unwrap();
    }
    manifest(&dir, v); verify_package(&dir, v).unwrap()
}
#[tokio::test]
#[ignore="real official GBase/Inceptor artifacts, both Java runtimes, and complete HTTPS downloads; copied test exe only"]
async fn original_gbase_zip_and_inceptor_downloads_use_real_default_routes() {
    let portable = crate::storage::portable_root();
    assert!(portable.starts_with(std::env::temp_dir()) && portable.file_name().unwrap().to_string_lossy().starts_with("navkit-jdbc-command-tests-"));
    let root = PathBuf::from(std::env::var_os("NAVKIT_VENDOR_RESEARCH_ROOT").expect("fixed original vendor evidence root"));
    assert!(std::net::TcpStream::connect_timeout(&"127.0.0.1:1".parse().unwrap(), Duration::from_millis(250)).is_err());
    for kind in ["gbase", "inceptor"] {
        let v = vendor(kind).unwrap(); let jar = stage(&root, &v); let dir = jar.parent().unwrap();
        assert_eq!(status_at(&v, dir, kind).status, "ready");
        assert_eq!(std::env::split_paths(&classpath_for(&jar).unwrap()).collect::<Vec<_>>(), [jar.clone()]);
        for name in std::iter::once("driver.jar").chain(vendor_assets::extra_files(kind).iter().copied()) {
            let path = dir.join(name); let saved = fs::read(&path).unwrap();
            fs::remove_file(&path).unwrap();
            assert!(verify_package(dir, &v).is_err()); assert!(classpath_for(&jar).is_err()); assert_ne!(status_at(&v, dir, kind).status, "ready");
            fs::write(&path, &saved).unwrap();
            let mut corrupt = saved.clone(); corrupt[0] ^= 1; fs::write(&path, corrupt).unwrap();
            assert!(verify_package(dir, &v).is_err()); fs::write(path, saved).unwrap();
        }
        fs::write(dir.join("unverified-neighbour.jar"), b"PK-fake").unwrap(); assert!(verify_package(dir, &v).is_err());
        fs::remove_file(dir.join("unverified-neighbour.jar")).unwrap();
        let req = request(kind); let route = resolve_default(&req).unwrap().expect("actual default managed vendor route");
        assert_eq!(route.target, jar.to_string_lossy()); assert_eq!(route.driver_class, v.class); assert!(!route.url.contains(&req.password));
        let error = match crate::db::open(&req).await { Ok(_) => panic!("closed loopback unexpectedly connected"), Err(error) => format!("{error:#}") };
        assert!(error.contains("JDBC"), "{kind}: {error}");
        assert!(error.contains("Connection refused") || error.contains("拒绝") || error.contains("Communications link failure"), "{kind}: expected actual closed-port transport, got {error}");
        for bad in ["NoClassDefFoundError", "ClassNotFoundException", "sidecar 已退出", &req.password] { assert!(!error.contains(bad), "{kind}: {error}"); }
        let downloaded = download(kind, Some(v.version)).await.expect("real fixed official complete user-click download");
        assert_eq!(downloaded.status, "ready", "{kind}: {}", downloaded.message);
        assert_eq!(downloaded.version, v.version); assert!(!downloaded.can_download);
        assert_eq!(verify_package(dir, &v).unwrap(), jar); assert_eq!(digest_file(&jar).unwrap(), v.jar.sha256);
        println!("{kind}: original package/components/negative integrity/Java/default route/official HTTPS download PASS; server UNVERIFIED");
    }
}
