//! Original Google all-JAR; local gRPC fixture is not Google Cloud certification.
use super::*;
fn request() -> ConnectionRequest {
    ConnectionRequest { kind: crate::models::DbKind::of("spanner"), host: "127.0.0.1".into(), port: 1,
        user: String::new(), password: String::new(), database: "probe-project/probe-instance/probe-db".into(),
        file: String::new(), params: "ssl=false".into(), timeout_secs: 20 }
}
#[test]
fn spanner_cloud_tokens_and_explicit_local_emulator_do_not_mix() {
    let v = vendor("spanner").unwrap(); let mut req = request();
    let url = connection_url(&req, &v, &params(&v, &req).unwrap().unwrap()).unwrap();
    assert!(url.starts_with("jdbc:cloudspanner://127.0.0.1:1/projects/probe-project/instances/probe-instance/databases/probe-db;usePlainText=true;"));
    req.password = "SYNTHETIC_TOKEN.abc-123".into(); req.user = "token".into();
    assert!(connection_url(&req, &v, &params(&v, &req).unwrap().unwrap()).is_err(), "do not send tokens to plaintext");
    req.host = "spanner.googleapis.com".into(); req.port = 443; req.params.clear();
    let cloud = connection_url(&req, &v, &[]).unwrap();
    assert!(cloud.contains(";usePlainText=false;")); assert!(!cloud.contains(&req.password));
    for host in ["localhost", "evil.example", "spanner.googleapis.com.evil.example", "127.0.0.1"] {
        req.host = host.into(); assert!(connection_url(&req, &v, &[]).is_err());
    }
    req.host = "spanner.googleapis.com".into();
    for bad in ["project/instance", "project/instance/db;usePlainText=true", "project/instance/db?x", "project/instance/db space"] {
        req.database = bad.into(); assert!(connection_url(&req, &v, &[]).is_err());
    }
    req.database = "projects/probe-project/instances/probe-instance/databases/probe-db".into();
    assert!(connection_url(&req, &v, &[]).is_ok());
    req.password.clear(); assert!(connection_url(&req, &v, &[]).is_err(), "no implicit ADC");
    for setting in ["credentials=file.json", "oauthtoken=unprotected", "endpoint=https://evil.example", "autoConfigEmulator=true"] {
        req.params = setting.into(); assert!(params(&v, &req).unwrap().is_none(), "preserve explicit custom configuration");
    }
    req = request(); req.params = "schema=ignored".into();
    assert!(connection_url(&req, &v, &params(&v, &req).unwrap().unwrap()).is_err());
}
fn stage_original(root: &Path) -> PathBuf {
    let v = vendor("spanner").unwrap(); let dir = package_dir_for(&v); fs::create_dir_all(&dir).unwrap();
    fs::copy(root.join("cloud-iris-20261107/artifacts/google-cloud-spanner-jdbc-2.47.0-single-jar-with-dependencies.jar"), dir.join("driver.jar")).unwrap();
    fs::copy(root.join("cloud-final-20261107/evidence/google-cloud-spanner-jdbc-2.47.0.pom"), dir.join("official.pom")).unwrap();
    fs::write(dir.join("manifest.json"), serde_json::to_vec(&Manifest { format:1, kind:v.kind.into(), version:v.version.into(), class:v.class.into() }).unwrap()).unwrap();
    verify_package(&dir, &v).unwrap()
}
#[tokio::test]
#[ignore="original Spanner all-JAR, Java11/17, and real local HTTP/2 gRPC fixture; isolated copied exe only"]
async fn original_spanner_package_default_route_observes_real_grpc_rejection() {
    let portable = crate::storage::portable_root();
    assert!(portable.starts_with(std::env::temp_dir()) && portable.file_name().unwrap().to_string_lossy().starts_with("navkit-jdbc-command-tests-"));
    let root = PathBuf::from(std::env::var_os("NAVKIT_JDBC_RESEARCH_ROOT").unwrap());
    let jar = stage_original(&root); let v = vendor("spanner").unwrap(); let dir = jar.parent().unwrap();
    let ready = status_at(&v, dir, "spanner"); assert_eq!(ready.status, "ready", "{}", ready.message);
    assert_eq!(std::env::split_paths(&classpath_for(&jar).unwrap()).collect::<Vec<_>>(), [jar.clone()]);
    for name in ["driver.jar", "official.pom"] {
        let path = dir.join(name); let saved = fs::read(&path).unwrap(); fs::remove_file(&path).unwrap();
        assert!(verify_package(dir, &v).is_err()); assert!(classpath_for(&jar).is_err()); assert_ne!(status_at(&v, dir, "spanner").status, "ready");
        fs::write(&path, &saved).unwrap(); let mut corrupt = saved.clone(); corrupt[0] ^= 1; fs::write(&path, corrupt).unwrap();
        assert!(verify_package(dir, &v).is_err()); fs::write(path, saved).unwrap();
    }
    let mut req = request(); req.port = std::env::var("NAVKIT_SPANNER_FIXTURE_PORT").unwrap().parse().unwrap();
    let sentinel = std::env::var("NAVKIT_SPANNER_FIXTURE_SENTINEL").unwrap();
    let route = resolve_default(&req).unwrap().expect("actual managed default route");
    assert_eq!(route.driver_class, v.class); assert_eq!(route.target, jar.to_string_lossy());
    let error = match crate::db::open(&req).await { Ok(_) => panic!("denied gRPC must not report success"), Err(error) => format!("{error:#}") };
    assert!(error.contains(&sentinel), "the genuine gRPC auth sentinel must reach the real adapter: {error}");
    for bad in ["NoClassDefFoundError", "ClassNotFoundException", "sidecar 已退出", "无效 JSON"] { assert!(!error.contains(bad), "{error}"); }
    assert_eq!(digest_file(&jar).unwrap(), v.jar.sha256);
    let downloaded = download("spanner", Some(v.version)).await.expect("complete official HTTPS download");
    assert_eq!(downloaded.status, "ready", "{}", downloaded.message); assert_eq!(verify_package(dir, &v).unwrap(), jar);
    println!("spanner: original package/components/negative integrity/Java/default route/real gRPC auth rejection/official HTTPS download PASS; cloud server and TLS UNVERIFIED");
}
