//! Truthful catalogue-wide live-service matrix. Unconfigured services are never counted as PASS.
//! Run with NAVKIT_DS_CONFIG=<absolute JSON path> and NAVKIT_DS_REPORT=<report path>:
//! cargo test --manifest-path src-tauri/Cargo.toml --test datasource_matrix -- --ignored --nocapture
//! Each JSON item: { request: ConnectionRequest, query, expected, container, object }.
//! Provision a unique sentinel beforehand; expected must occur in the returned rows.
use navkit_lib::{catalog::CATALOG, db, models::*, schema};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet}, time::{Duration, Instant}};

#[derive(Deserialize)]
struct Fixture {
    request: ConnectionRequest,
    query: String,
    expected: String,
    container: String,
    object: String,
    #[serde(default)]
    setup: Vec<String>,
    #[serde(default)]
    invalid_query: Option<String>,
    #[serde(default)]
    reject_connection: Option<ConnectionRequest>,
    #[serde(default)]
    cleanup: Vec<String>,
}

#[tokio::test]
#[ignore = "requires explicitly provisioned real services; emits honest UNVERIFIED entries"]
async fn all_datasources_live_matrix() {
    let output = std::env::var("NAVKIT_DS_REPORT").expect("Set NAVKIT_DS_REPORT to an evidence JSON path");
    let input = std::env::var("NAVKIT_DS_CONFIG_JSON").map(String::into_bytes).unwrap_or_else(|_| {
        let path = std::env::var("NAVKIT_DS_CONFIG").expect("Set NAVKIT_DS_CONFIG or NAVKIT_DS_CONFIG_JSON");
        std::fs::read(path).expect("read fixtures")
    });
    let fixtures: Vec<Fixture> = serde_json::from_slice(&input).expect("valid fixtures");
    let mut seen = HashSet::new();
    for fixture in &fixtures {
        assert!(fixture.request.kind.meta().is_some(), "unknown fixture kind");
        assert!(seen.insert(fixture.request.kind.id().to_string()), "duplicate fixture kind");
        assert!(!fixture.expected.is_empty() && !fixture.object.is_empty() && !fixture.query.is_empty(), "live acceptance requires a sentinel, query and browse object");
    }
    let mut report = Vec::<Value>::new();
    let mut failures = Vec::new();
    for meta in CATALOG {
        let Some(f) = fixtures.iter().find(|f| f.request.kind.id() == meta.id) else {
            report.push(json!({"kind": meta.id,"label":meta.label,"status":"UNVERIFIED","reason":"No provisioned server/credentials/sentinel fixture. Protocol-family reuse is not product-level acceptance."}));
            continue;
        };
        let started = Instant::now();
        let reg: db::DbRegistry = parking_lot::RwLock::new(HashMap::new());
        let id = format!("matrix-{}", meta.id);
        let outcome = tokio::time::timeout(Duration::from_secs(90), async {
            db::open_and_register(&reg, &id, &f.request).await?;
            for command in &f.setup {
                db::execute(&reg, &id, command, f.request.kind.clone()).await?;
            }
            let connection = db::get_db(&reg, &id)?;
            let containers = schema::list_databases(connection.as_ref(), f.request.kind.clone()).await?;
            anyhow::ensure!(containers.iter().any(|(name,_)| name == &f.container), "sentinel container not in real tree");
            let objects = schema::list_tables(connection.as_ref(), f.request.kind.clone(), &f.container).await?;
            anyhow::ensure!(objects.iter().any(|(name,_)| name == &f.object), "sentinel object not in real tree");
            let page = schema::page_table(connection.as_ref(), f.request.kind.clone(), &PageRequest {
                connection_id: id.clone(), schema:f.container.clone(),table:f.object.clone(),offset:0,limit:50,
                order_by:None,ascending:true,filters:vec![],filter_logic:FilterLogic::And,order_rules:None,
            }).await?;
            anyhow::ensure!(!page.columns.is_empty(), "browse returned no columns");
            anyhow::ensure!(page.rows.iter().flatten().any(|v| v.as_deref().is_some_and(|s| s.contains(&f.expected))), "browse did not return provisioned sentinel");
            let result = db::execute(&reg, &id, &f.query, f.request.kind.clone()).await?;
            anyhow::ensure!(result.sets.iter().flat_map(|set| &set.rows).flatten().any(|v| v.as_deref().is_some_and(|s| s.contains(&f.expected))), "query did not return provisioned sentinel");
            if let Some(invalid) = &f.invalid_query {
                anyhow::ensure!(db::execute(&reg, &id, invalid, f.request.kind.clone()).await.is_err(), "invalid command was accepted as success");
            }
            if let Some(rejected) = &f.reject_connection {
                anyhow::ensure!(db::open(rejected).await.is_err(), "invalid credentials/endpoint were accepted as success");
            }
            Ok::<Value, anyhow::Error>(json!({"containers":containers.len(),"objects":objects.len(),"browse_rows":page.rows.len(),"query_sets":result.sets.len(),"sentinel_match":true,"invalid_query_rejected":f.invalid_query.is_some(),"invalid_connection_rejected":f.reject_connection.is_some()}))
        }).await;
        let mut entry = json!({"kind":meta.id,"label":meta.label,"elapsed_ms":started.elapsed().as_millis()});
        match outcome {
            Ok(Ok(details)) => { entry["status"] = json!("PASS"); entry["details"] = details; }
            Ok(Err(error)) => { entry["status"] = json!("FAIL"); entry["reason"] = json!(format!("{error:#}")); failures.push(meta.id); }
            Err(_) => { entry["status"] = json!("FAIL"); entry["reason"] = json!("live acceptance timed out after 90 seconds"); failures.push(meta.id); }
        }
        if !f.cleanup.is_empty() && db::get_db(&reg, &id).is_ok() {
            let mut errors = Vec::new();
            for command in &f.cleanup {
                match tokio::time::timeout(Duration::from_secs(10), db::execute(&reg, &id, command, f.request.kind.clone())).await {
                    Ok(Ok(_)) => {},
                    Ok(Err(error)) => errors.push(format!("{error:#}")),
                    Err(_) => errors.push("cleanup timed out".to_string()),
                }
            }
            entry["cleanup"] = json!({"commands":f.cleanup.len(),"errors":errors});
            if !errors.is_empty() {
                entry["status"] = json!("FAIL");
                entry["reason"] = json!("sentinel cleanup failed; see cleanup.errors");
                if !failures.contains(&meta.id) { failures.push(meta.id); }
            }
        }
        // Dropping the isolated registry releases all database pools/sidecars.
        drop(reg);
        report.push(entry);
    }
    let counts = json!({"total":report.len(),"pass":report.iter().filter(|v| v["status"] == "PASS").count(),"fail":failures.len(),"unverified":report.iter().filter(|v| v["status"] == "UNVERIFIED").count()});
    std::fs::write(&output, serde_json::to_vec_pretty(&json!({"counts":counts,"rows":report})).unwrap()).expect("write evidence");
    println!("Matrix: {counts}; evidence={output}");
    assert!(failures.is_empty(), "Provisioned data sources failed: {failures:?}");
    if std::env::var("NAVKIT_DS_REQUIRE_ALL").as_deref() == Ok("1") {
        assert_eq!(seen.len(), CATALOG.len(), "REQUIRE_ALL: missing real-service fixtures");
    }
}

#[test]
fn every_catalogue_entry_has_an_executable_route() {
    let mut ids = HashSet::new();
    for meta in CATALOG {
        assert!(ids.insert(meta.id), "duplicate catalogue ID {}", meta.id);
        assert!(!meta.planned, "{} is still only planned", meta.id);
        if meta.protocol == Protocol::Http { assert!(!meta.http.is_empty(), "HTTP entry {} has no adapter", meta.id); }
    }
}
