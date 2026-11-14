//! Actual navkit_lib APIs against stateful native loopback protocols.
//! Simulation evidence only: these are not vendor deployments or authentication certifications.
#[path = "support/sim_native_wire/mod.rs"]
mod sim_native_wire;

use anyhow::{anyhow, ensure, Result};
use navkit_lib::{db, models::*, schema};
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};

type Data = Vec<Vec<Option<String>>>;
fn request(kind: &str, port: u16) -> ConnectionRequest {
    ConnectionRequest { kind: DbKind::of(kind), host: "127.0.0.1".into(), port,
        user: if kind == "mongodb" { "" } else { "fixture" }.into(),
        password: if kind == "mongodb" { "" } else { "secret" }.into(),
        database: "fixture".into(), file: String::new(), params: String::new(), timeout_secs: 5 }
}
fn page(id: &str, object: &str, offset: u64, limit: u64) -> PageRequest {
    PageRequest { connection_id: id.into(), schema: "fixture".into(), table: object.into(), offset, limit, include_total: true,
        order_by: None, ascending: true, filters: Vec::new(), filter_logic: FilterLogic::And, order_rules: None }
}
fn record<T: Serialize>(steps: &mut Vec<Value>, name: &str, result: Result<T>) {
    match result {
        Ok(value) => steps.push(json!({"step":name,"status":"PASS","evidence":value})),
        Err(error) => steps.push(json!({"step":name,"status":"FAIL","error":format!("{error:#}")})),
    }
}
fn rejected<T>(result: Result<T>) -> Result<Value> {
    let error = result.err().ok_or_else(|| anyhow!("expected an actual product Err; received Ok"))?;
    Ok(json!({"actual_product_err":format!("{error:#}")}))
}
fn report_path() -> PathBuf {
    std::env::var_os("NAVKIT_SIM_NATIVE_WIRE_REPORT").map(PathBuf::from).unwrap_or_else(||
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tools/acceptance/simulation/native/report-native-wire.json"))
}
fn persist(rows: &[Value]) {
    let path = report_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let failed = rows.iter().filter(|r| r["status"] != "SIMULATED_PROTOCOL_PASS").count();
    std::fs::write(&path, serde_json::to_vec_pretty(&json!({
        "schema_version":1,"evidence_type":"STATEFUL_NATIVE_WIRE_SIMULATION_NOT_VENDOR_CERTIFICATION",
        "counts":{"covered":rows.len(),"simulated_pass":rows.len()-failed,"fail":failed,"vendor_real_pass":0},"rows":rows
    })).unwrap()).unwrap();
}
fn corpus() -> Data {
    (0..3).map(|index| {
        let id = uuid::Uuid::new_v4().to_string();
        vec![Some(id.clone()), Some(format!("{id}|中文;O'Reilly\\路径😀|{index}")),
            match index { 0 => None, 1 => Some(String::new()), _ => Some("NULL".into()) }]
    }).collect()
}
fn names(kind: &str, browse: bool) -> Vec<&'static str> {
    match kind {
        "mongodb" => vec!["_id", "payload", "nullable_value"],
        "nebulagraph" if browse => vec!["v"],
        "rocketmq" => vec!["queue_id", "queue_offset", "body", "topic", "properties", "store_timestamp"],
        _ => vec!["id", "payload", "nullable_value"],
    }
}
fn expected(kind: &str, browse: bool, data: &Data) -> Data {
    match kind {
        "nebulagraph" if browse => data.iter().map(|row| vec![Some(json!({
            "items.id":row[0],"items.payload":row[1],"items.nullable_value":row[2]
        }).to_string())]).collect(),
        "rocketmq" => data.iter().enumerate().map(|(index, row)| vec![Some("0".into()),Some(index.to_string()),
            Some(json!({"id":row[0],"payload":row[1],"nullable_value":row[2]}).to_string()),Some("fixture".into()),Some(String::new()),Some("1700000000000".into())]).collect(),
        _ => data.clone(),
    }
}
fn equal(columns: &[ResultColumn], rows: &Data, wanted_names: &[&str], wanted: &Data) -> Result<Value> {
    let actual_names = columns.iter().map(|column| column.name.as_str()).collect::<Vec<_>>();
    ensure!(actual_names == wanted_names, "column mismatch actual={actual_names:?} expected={wanted_names:?}");
    ensure!(rows == wanted, "complete row mismatch actual={rows:?} expected={wanted:?}");
    Ok(json!({"columns":actual_names,"rows":rows,"expected_rows":wanted,"comparison":"complete ordered equality"}))
}
async fn write_record(kind: &str, registry: &db::DbRegistry, id: &str, row: &[Option<String>], mongo_records: &sim_native_wire::Records) -> Result<Value> {
    if kind == "mongodb" {
        // Product Mongo command surface is read/count only: provision typed fixture state.
        mongo_records.lock().unwrap().push(row.to_vec());
        return Ok(json!({"method":"typed fixture provisioning; not a product Mongo write claim","row":row}));
    }
    let stmt = match kind {
        "cassandra" => {
            let values = row.iter().map(|value| value.as_ref().map_or_else(|| "NULL".into(), |s| format!("'{}'", s.replace('\'', "''")))).collect::<Vec<String>>().join(",");
            format!("INSERT INTO fixture.items (id,payload,nullable_value) VALUES ({values})")
        }
        "nebulagraph" => {
            let values = row.iter().map(|value| value.as_ref().map_or_else(|| "NULL".into(), |s| json!(s).to_string())).collect::<Vec<_>>().join(",");
            format!("INSERT VERTEX items(id,payload,nullable_value) VALUES {}:({values})", json!(row[0]))
        }
        "rocketmq" => json!({"operation":"send","topic":"fixture","body":json!({"id":row[0],"payload":row[1],"nullable_value":row[2]}).to_string()}).to_string(),
        _ => unreachable!(),
    };
    let result = db::execute(registry, id, &stmt, DbKind::of(kind)).await?;
    ensure!(result.sets.len() == 1 && result.sets[0].ok, "product write response {result:?}");
    Ok(json!({"method":"actual db::execute","statement":stmt,"result":result}))
}
fn read_statement(kind: &str) -> String {
    match kind {
        "mongodb" => "items {}".into(),
        "cassandra" => "SELECT * FROM fixture.items LIMIT 20".into(),
        "nebulagraph" => "MATCH (v:`items`) RETURN id(v) AS id,v.items.payload AS payload,v.items.nullable_value AS nullable_value SKIP 0 LIMIT 20".into(),
        _ => json!({"operation":"pull","topic":"fixture","queue":"broker:0","offset":0,"limit":20}).to_string(),
    }
}
async fn verify(kind: &str, server: &sim_native_wire::Server, steps: &mut Vec<Value>, mongo_records: &sim_native_wire::Records, count_fault: &Arc<AtomicBool>, data: &Data) -> Result<()> {
    let req = request(kind, server.port);
    let id = format!("native-wire-{kind}-{}", uuid::Uuid::new_v4());
    let registry: db::DbRegistry = parking_lot::RwLock::new(HashMap::new());
    // open and registration are separate real public entrypoints.
    let direct = db::open(&req).await?; drop(direct);
    record(steps, "actual_db_open", Ok(json!({"host":"127.0.0.1","port":server.port,"anonymous":kind=="mongodb"})));
    db::open_and_register(&registry, &id, &req).await?;
    record(steps, "actual_db_open_and_register", Ok(json!({"connection_id":id})));
    let conn = db::get_db(&registry, &id)?;
    let object = match kind { "nebulagraph" => "tag:items", "rocketmq" => "broker:0", _ => "items" };
    for (index, row) in data.iter().take(2).enumerate() {
        record(steps, &format!("initial_dynamic_record_{index}"), write_record(kind, &registry, &id, row, mongo_records).await);
    }
    record(steps, "actual_schema_list_databases", async {
        let list = schema::list_databases(conn.as_ref(), req.kind.clone()).await?;
        ensure!(list.iter().map(|(name,_)|name.as_str()).collect::<Vec<_>>() == ["fixture"], "unexpected containers {list:?}"); Ok(json!(list))
    }.await);
    record(steps, "actual_schema_list_tables", async {
        let list = schema::list_tables(conn.as_ref(), req.kind.clone(), "fixture").await?;
        ensure!(list.iter().map(|(name,_)|name.as_str()).collect::<Vec<_>>() == [object], "unexpected objects {list:?}"); Ok(json!(list))
    }.await);
    record(steps, "actual_schema_table_schema", async {
        let table = schema::table_schema(conn.as_ref(), req.kind.clone(), "fixture", object).await?;
        ensure!(table.columns.iter().map(|column|column.name.as_str()).collect::<Vec<_>>() == names(kind,true), "table schema {table:?}");
        if kind == "mongodb" { ensure!(table.row_count == Some(2), "Mongo count mismatch {table:?}"); }
        Ok(json!(table))
    }.await);
    let initial = data[..2].to_vec();
    record(steps, "actual_schema_page_full_exact", async {
        let result = schema::page_table(conn.as_ref(), req.kind.clone(), &page(&id, object, 0, 20)).await?;
        equal(&result.columns, &result.rows, &names(kind,true), &expected(kind,true,&initial))
    }.await);
    for offset in [0u64, 1] {
        record(steps, &format!("actual_schema_page_limit1_offset{offset}_exact"), async {
            let result = schema::page_table(conn.as_ref(), req.kind.clone(), &page(&id, object, offset, 1)).await?;
            let rows = expected(kind,true,&initial).into_iter().skip(offset as usize).take(1).collect();
            equal(&result.columns, &result.rows, &names(kind,true), &rows)
        }.await);
    }
    record(steps, "actual_schema_page_limit0_empty", async {
        let result = schema::page_table(conn.as_ref(), req.kind.clone(), &page(&id, object, 0, 0)).await?;
        ensure!(result.rows.is_empty(), "limit=0 returned rows: {:?}",result.rows); Ok(json!(result))
    }.await);
    record(steps, "actual_db_execute_full_exact", async {
        let result = db::execute(&registry,&id,&read_statement(kind),req.kind.clone()).await?;
        ensure!(result.sets.len()==1,"expected one complete result: {result:?}");
        equal(&result.sets[0].columns,&result.sets[0].rows,&names(kind,false),&expected(kind,false,&initial))
    }.await);
    record(steps, "new_dynamic_state_after_reads", write_record(kind,&registry,&id,&data[2],mongo_records).await);
    record(steps, "actual_db_execute_observes_new_record_exact", async {
        let result=db::execute(&registry,&id,&read_statement(kind),req.kind.clone()).await?;
        ensure!(result.sets.len()==1,"expected one result: {result:?}");
        equal(&result.sets[0].columns,&result.sets[0].rows,&names(kind,false),&expected(kind,false,data))
    }.await);
    record(steps, "actual_schema_page_observes_new_state_exact", async {
        let result=schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,object,0,20)).await?;
        equal(&result.columns,&result.rows,&names(kind,true),&expected(kind,true,data))
    }.await);
    record(steps, "actual_schema_page_beyond_end_empty", async {
        let result=schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,object,50,1)).await?;
        ensure!(result.rows.is_empty(),"beyond-end rows {:?}",result.rows); Ok(json!(result))
    }.await);
    let invalid = match kind { "mongodb"=>"items {invalid-json}".to_owned(), "rocketmq"=>json!({"operation":"definitely_invalid"}).to_string(), _=>"DEFINITELY INVALID NATIVE COMMAND".into() };
    record(steps,"actual_illegal_command_err",rejected(db::execute(&registry,&id,&invalid,req.kind.clone()).await));
    let missing=match kind {"nebulagraph"=>"tag:missing_object","rocketmq"=>"missing-broker:99",_=>"missing_object"};
    record(steps,"actual_missing_object_page_err",rejected(schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,missing,0,1)).await));
    record(steps,"actual_missing_object_schema_err",rejected(schema::table_schema(conn.as_ref(),req.kind.clone(),"fixture",missing).await));
    if kind=="mongodb" {
        record(steps,"actual_count_after_new_state",async {
            let result=db::execute(&registry,&id,"count items {}",req.kind.clone()).await?;
            ensure!(result.sets.len()==1 && result.sets[0].rows==vec![vec![Some("3".to_owned())]],"count result {result:?}"); Ok(json!(result))
        }.await);
        record(steps,"actual_filter_special_text_and_null_exact",async {
            let query=format!("items {}",json!({"payload":data[0][1],"nullable_value":null}));
            let result=db::execute(&registry,&id,&query,req.kind.clone()).await?;
            ensure!(result.sets.len()==1,"expected one filtered result");
            equal(&result.sets[0].columns,&result.sets[0].rows,&names(kind,false),&vec![data[0].clone()])
        }.await);
        count_fault.store(true,Ordering::SeqCst);
        record(steps,"actual_count_business_fault_page_err",rejected(schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,object,0,1)).await));
        record(steps,"actual_count_business_fault_schema_err",rejected(schema::table_schema(conn.as_ref(),req.kind.clone(),"fixture",object).await));
        record(steps,"actual_count_business_fault_execute_err",rejected(db::execute(&registry,&id,"count items {}",req.kind.clone()).await));
        count_fault.store(false,Ordering::SeqCst);
    }
    let mut wrong=req.clone(); wrong.user="wrongcreds".into(); wrong.password="wrongcreds".into();
    let auth_events=server.events().len();
    record(steps,"actual_bad_auth_rejected",async {
        let evidence=rejected(db::open(&wrong).await)?;
        let events=server.events(); let events=&events[auth_events..];
        let signal=if kind=="mongodb" {"authentication_rejected:18"} else if kind=="rocketmq" {"acl=false"} else {"authenticate:false"};
        ensure!(events.iter().any(|line|line.contains(signal)),"actual auth rejection did not reach native wire: {events:?}");
        Ok(json!({"error":evidence,"native_auth_reject_events":events,"mongo_error_code":if kind=="mongodb"{json!(18)}else{Value::Null}}))
    }.await);
    if kind!="mongodb" {
        let mut wrong_secret=req.clone(); wrong_secret.password="wrong-secret".into();
        record(steps,"actual_correct_user_wrong_secret_rejected",rejected(db::open(&wrong_secret).await));
    }
    if kind=="rocketmq" {
        record(steps,"independent_acl_rejects_tampered_signed_body",sim_native_wire::rocket::reject_tampered_body(server.port).await);
        record(steps,"actual_product_queue_unchanged_after_rejected_tamper",async {
            let result=db::execute(&registry,&id,&read_statement(kind),req.kind.clone()).await?;
            ensure!(result.sets.len()==1,"expected one result");
            equal(&result.sets[0].columns,&result.sets[0].rows,&names(kind,false),&expected(kind,false,data))
        }.await);
    }
    drop(conn); db::close_all(&registry);
    Ok(())
}

#[tokio::test(flavor="multi_thread", worker_threads=4)]
async fn four_native_protocols_use_actual_product_apis() {
    let mut rows=Vec::new();
    for kind in ["mongodb","cassandra","nebulagraph","rocketmq"] {
        let started=Instant::now(); let mut steps=Vec::new();
        let mongo_records=sim_native_wire::seed(); let count_fault=Arc::new(AtomicBool::new(false)); let data=corpus();
        let server=match kind {
            "mongodb"=>sim_native_wire::mongo::start(mongo_records.clone(),count_fault.clone()).await,
            "cassandra"=>sim_native_wire::cql::start().await,
            "nebulagraph"=>sim_native_wire::nebula::start().await,
            _=>sim_native_wire::rocket::start().await,
        };
        let mut row=json!({"kind":kind,"evidence_type":"STATEFUL_LOOPBACK_NATIVE_PROTOCOL_SIMULATION","vendor_server_status":"NOT_TESTED",
            "corpus":data,"port_policy":"owned 127.0.0.1:0 only","limits":["Not a real vendor service or certification.","Strict documented fixture subset; no TLS/vendor deployment claims."],
            "mutation_route":if kind=="mongodb" {"typed simulation provisioning; product supports read/count only"} else {"actual db::execute"},
            "null_encoding":if kind=="rocketmq" {"JSON null inside actual UTF-8 message body; Remoting has no nullable message cell"} else {"native NULL cell; Nebula browse vertex nullable property"}});
        match server {
            Ok(server)=>{
                match tokio::time::timeout(Duration::from_secs(90),verify(kind,&server,&mut steps,&mongo_records,&count_fault,&data)).await {
                    Ok(Ok(()))=>{}, Ok(Err(error))=>record::<Value>(&mut steps,"product_route",Err(error)),
                    Err(_)=>record::<Value>(&mut steps,"product_route",Err(anyhow!("90 second protocol deadline"))),
                }
                row["wire_events"]=json!(server.events()); row["port"]=json!(server.port);
                record(&mut steps,"listener_and_connection_tasks_cleanup",server.shutdown().await);
            }
            Err(error)=>record::<Value>(&mut steps,"fixture_start",Err(error)),
        }
        row["status"]=json!(if steps.iter().any(|step|step["status"]=="FAIL") {"SIMULATED_PROTOCOL_FAIL"} else {"SIMULATED_PROTOCOL_PASS"});
        row["elapsed_ms"]=json!(started.elapsed().as_millis()); row["steps"]=json!(steps);
        println!("{kind}: {}",row["status"]); rows.push(row); persist(&rows);
    }
    assert!(rows.len()==4 && rows.iter().all(|row|row["status"]=="SIMULATED_PROTOCOL_PASS"),"native wire simulation failed; report={}",report_path().display());
}
