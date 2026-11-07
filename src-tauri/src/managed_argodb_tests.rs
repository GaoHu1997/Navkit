//! New official Quark package for the ArgoDB Gateway: never the legacy Inceptor/Hive JAR.
use super::*;
fn request()->ConnectionRequest{ConnectionRequest{kind:crate::models::DbKind::of("argodb"),host:"127.0.0.1".into(),port:1,user:String::new(),password:String::new(),database:"default".into(),file:String::new(),params:String::new(),timeout_secs:5}}
#[test]
fn argodb_uses_exact_quark_not_hive_or_inceptor(){
    let v=vendor("argodb").unwrap();let mut req=request();
    assert_eq!(v.class,"io.transwarp.jdbc.QuarkDriver");assert_ne!(v.jar.sha256,vendor("inceptor").unwrap().jar.sha256);assert_ne!(v.class,vendor("hive").unwrap().class);
    assert_eq!(connection_url(&req,&v,&[]).unwrap(),"jdbc:transwarp2://127.0.0.1:1/default;auth=noSasl");assert!(dependencies::specs("argodb").is_empty());
    for raw in ["ssl=true","sslmode=verify-full","ssl_verify=false","schema=ignored","catalog=ignored"]{req.params=raw.into();assert!(params(&v,&req).and_then(|pairs|connection_url(&req,&v,&pairs.unwrap())).is_err());}
    for raw in ["authentication=kerberos","principal=vendor/node@REALM","transportMode=http","auth=ldap"]{req.params=raw.into();assert!(params(&v,&req).unwrap().is_none(),"explicit extended bindings must not be ignored");}
    req.params.clear();req.database="bad;password=secret".into();assert!(connection_url(&req,&v,&[]).is_err());
    req.database="default".into();req.password="not-noSasl".into();assert!(connection_url(&req,&v,&[]).is_err(),"noSasl route must not silently discard credentials or impersonate LDAP");
}
#[tokio::test]
#[ignore="original official Quark package, actual default product route and full HTTPS download; copied executable only"]
async fn original_argodb_quark_default_route_and_full_download(){
    let root=crate::storage::portable_root();assert!(root.starts_with(std::env::temp_dir())&&root.file_name().unwrap().to_string_lossy().starts_with("navkit-jdbc-command-tests-"));
    let original=PathBuf::from(std::env::var_os("NAVKIT_ARGODB_ORIGINAL_JAR").expect("official unchanged Quark original"));let v=vendor("argodb").unwrap();check_asset(&original,&v.jar).unwrap();
    let dir=package_dir_for(&v);fs::create_dir_all(&dir).unwrap();let jar=dir.join("driver.jar");fs::copy(&original,&jar).unwrap();
    fs::write(dir.join("manifest.json"),serde_json::to_vec(&Manifest{format:1,kind:v.kind.into(),version:v.version.into(),class:v.class.into()}).unwrap()).unwrap();
    verify_package(&dir,&v).unwrap();assert_eq!(status_at(&v,&dir,"argodb").status,"ready");assert_eq!(std::env::split_paths(&classpath_for(&jar).unwrap()).collect::<Vec<_>>(),[jar.clone()]);
    let bytes=fs::read(&jar).unwrap();fs::remove_file(&jar).unwrap();assert!(verify_package(&dir,&v).is_err());assert!(classpath_for(&jar).is_err());fs::write(&jar,&bytes).unwrap();let mut bad=bytes;bad[0]^=1;fs::write(&jar,bad).unwrap();assert!(verify_package(&dir,&v).is_err());fs::copy(&original,&jar).unwrap();
    fs::write(dir.join("unverified-neighbour.jar"),b"PK-fake").unwrap();assert!(verify_package(&dir,&v).is_err());fs::remove_file(dir.join("unverified-neighbour.jar")).unwrap();
    assert!(std::net::TcpStream::connect_timeout(&"127.0.0.1:1".parse().unwrap(),Duration::from_millis(250)).is_err());let req=request();let route=resolve_default(&req).unwrap().expect("actual ArgoDB default managed route");assert_eq!(route.driver_class,v.class);assert_eq!(route.target,jar.to_string_lossy());
    let error=match crate::db::open(&req).await{Ok(_)=>panic!("closed loopback unexpectedly connected"),Err(e)=>format!("{e:#}")};
    assert!(error.contains("Connection refused")||error.contains("拒绝"),"real TCP error required: {error}");for bad in ["NoClassDefFoundError","ClassNotFoundException","sidecar 已退出"]{assert!(!error.contains(bad),"{error}");}
    let downloaded=download("argodb",Some(v.version)).await.expect("actual official full HTTPS original download");assert_eq!(downloaded.status,"ready","{}",downloaded.message);assert_eq!(downloaded.version,v.version);assert_eq!(digest_file(&verify_package(&dir,&v).unwrap()).unwrap(),v.jar.sha256);check_asset(&original,&v.jar).unwrap();
    println!("argodb: original Quark/class/SPI/URL/default TCP/integrity/full official HTTPS download PASS; vendor server/authentication/TLS UNVERIFIED");
}
