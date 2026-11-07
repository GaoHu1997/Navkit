//! Real original artifacts + mandatory dependency and default-route regression.
use super::*;
const PRODUCTS: &[&str] = &["trino", "prestosql", "databricks", "db2", "informix", "saphana", "teradata", "vertica", "exasol", "dameng", "firebird", "oracle", "hive", "kylin", "snowflake"];
fn asset_name(spec: &DownloadSpec) -> &str { spec.url.rsplit('/').next().unwrap() }
pub(super) fn stage(dir: &Path, evidence: &Path, v: &Vendor) -> Result<()> {
    fs::create_dir_all(dir)?;
    fs::copy(evidence.join(asset_name(&v.jar)), dir.join("driver.jar"))?;
    if let Some(pom) = &v.pom { fs::copy(evidence.join(asset_name(pom)), dir.join("official.pom"))?; }
    if let Some(license) = &v.license { fs::copy(evidence.join("LICENSE"), dir.join("LICENSE"))?; check_asset(&dir.join("LICENSE"), license)?; }
    for dep in dependencies::specs(v.kind) {
        fs::copy(evidence.join(dep.name), dir.join(dep.name))?;
        fs::copy(evidence.join(dep.pom_name), dir.join(dep.pom_name))?;
    }
    fs::write(dir.join("manifest.json"), serde_json::to_vec(&Manifest { format: 1, kind: v.kind.into(), version: v.version.into(), class: v.class.into() })?)?;
    Ok(())
}
#[test]
fn only_fixed_mandatory_runtime_dependencies_enter_managed_classpaths() {
    assert_eq!(dependencies::specs("informix").iter().map(|d| d.name).collect::<Vec<_>>(), ["bson-3.8.0.jar"]);
    assert_eq!(dependencies::specs("kylin").iter().map(|d| d.name).collect::<Vec<_>>(), ["jaxb-api-2.3.1.jar", "javax.activation-api-1.2.0.jar"]);
    for kind in PRODUCTS.iter().copied().filter(|k| !["informix", "kylin"].contains(k)) { assert!(dependencies::specs(kind).is_empty(), "{kind}"); }
    let custom = std::env::temp_dir().join("navkit-user-custom-driver.jar");
    assert_eq!(classpath_for(&custom).unwrap(), std::env::join_paths([custom]).unwrap());
}
#[tokio::test]
#[ignore = "real pinned registry artifacts and Java; run only copied test exe in navkit-jdbc-command-tests-* temp"]
async fn every_official_package_dependency_and_default_route_is_real() {
    let portable = crate::storage::portable_root();
    assert!(portable.starts_with(std::env::temp_dir()) && portable.file_name().unwrap().to_string_lossy().starts_with("navkit-jdbc-command-tests-"));
    let root = PathBuf::from(std::env::var_os("NAVKIT_JDBC_REGISTRY_ROOT").expect("set downloaded fixed official registry root"));
    for &kind in PRODUCTS {
        let v = vendor(kind).unwrap();
        let dir = package_dir(kind);
        stage(&dir, &root.join(kind), &v).unwrap();
        let jar = verify_package(&dir, &v).unwrap();
        let ready = status_at(&v, &dir, kind);
        assert_eq!(ready.status, "ready", "{kind}: {}", ready.message);
        let cp = classpath_for(&jar).unwrap();
        let paths: Vec<_> = std::env::split_paths(&cp).collect();
        assert_eq!(paths.len(), 1 + dependencies::specs(kind).len());
        assert_eq!(paths[0], jar);
        let mut request = ConnectionRequest { kind: crate::models::DbKind::of(kind), host: String::new(), port: 1,
            user: String::new(), password: String::new(), database: String::new(), file: String::new(), params: String::new(), timeout_secs: 3 };
        request.host = if kind == "snowflake" { "org-account.snowflakecomputing.com".into() } else { "127.0.0.1".into() };
        request.port = 1; request.database = "navkit".into(); request.user = "navkit_probe".into();
        request.password = "navkit_probe_password".into(); request.timeout_secs = 3;
        if kind == "informix" { request.params = "informixserver=navkit_probe".into(); }
        if kind == "exasol" { request.database.clear(); }
        if kind == "databricks" { request.user = "token".into(); request.params = "httpPath=/sql/1.0/warehouses/navkit-probe;ssl=true".into(); }
        let resolved = resolve_default(&request).unwrap().expect("managed default route");
        assert_eq!(resolved.target, jar.to_string_lossy()); assert_eq!(resolved.driver_class, v.class);
        assert!(!resolved.url.contains(&request.password));
        // Snowflake host is an account DNS name: never contact it with test credentials.
        // Its closed-loopback dependency probe is run separately with an explicit local URL.
        if kind != "snowflake" {
            let result = crate::db::open(&request).await;
            let error = match result { Ok(_) => panic!("{kind}: closed port unexpectedly connected"), Err(e) => format!("{e:#}") };
            assert!(error.contains("JDBC"), "{kind}: {error}");
            assert!(!error.contains(&request.password), "{kind}: credential leaked");
            assert!(!error.contains("NoClassDefFoundError") && !error.contains("ClassNotFoundException"), "{kind}: {error}");
        }
        for dep in dependencies::specs(kind) {
            let dependency = dir.join(dep.name); let hidden = dir.join(format!("{}.missing", dep.name));
            fs::rename(&dependency, &hidden).unwrap();
            assert!(verify_package(&dir, &v).is_err(), "{kind}: missing dependency passed validation");
            assert!(classpath_for(&jar).is_err(), "{kind}: missing dependency passed classpath validation");
            fs::rename(&hidden, &dependency).unwrap();
        }
        fs::write(dir.join("unverified-neighbour.jar"), b"PK-fake").unwrap();
        assert!(verify_package(&dir, &v).is_err());
        fs::remove_file(dir.join("unverified-neighbour.jar")).unwrap();
        assert_eq!(status_at(&v, &dir, kind).status, "ready");
        println!("{kind}: complete fixed package/runtime/default JDBC route PASS; server UNVERIFIED");
    }
    for alias in ["kyuubi", "impala", "spark"] {
        assert_eq!(status(alias).unwrap().status, "ready");
        assert_eq!(package_dir(alias), package_dir("hive"));
    }
}
