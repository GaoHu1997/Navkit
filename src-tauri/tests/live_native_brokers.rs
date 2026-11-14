//! Real official Kafka/Moquette/ZooKeeper acceptance; never substitutes a mock SDK.
//! Requires explicitly started owned loopback services and NAVKIT_LIVE_NATIVE=1.
use anyhow::{anyhow,ensure,Result};
use navkit_lib::{db::{self,DbRegistry},models::*,schema};
use serde_json::{json,Value};
use std::{path::{Path,PathBuf},time::{Duration,Instant}};

fn request(kind:&str,port:u16,database:&str)->ConnectionRequest {
    ConnectionRequest {kind:DbKind::of(kind),host:"127.0.0.1".into(),port,user:String::new(),password:String::new(),database:database.into(),file:String::new(),params:"ssl=false".into(),timeout_secs:15}
}
fn page(id:&str,container:&str,object:&str,offset:u64,limit:u64)->PageRequest {
    PageRequest {connection_id:id.into(),schema:container.into(),table:object.into(),offset,limit,include_total:true,order_by:None,ascending:true,filters:vec![],filter_logic:Default::default(),order_rules:None}
}
fn has(rows:&[Vec<Option<String>>],text:&str)->bool {rows.iter().flatten().any(|v|v.as_deref()==Some(text))}
async fn command(reg:&DbRegistry,id:&str,req:&ConnectionRequest,value:Value)->Result<MultiResult> { db::execute(reg,id,&value.to_string(),req.kind.clone()).await }
async fn verify(kind:&str,steps:&mut Vec<Value>)->Result<Value> {
    let token=uuid::Uuid::new_v4().simple().to_string();
    let original=format!("{token}:中文;O'Reilly\\尾🙂");let changed=format!("changed:{original}");
    let reg=DbRegistry::default();let id=format!("native-live-{kind}-{token}");
    let topic=std::env::var("NAVKIT_KAFKA_TOPIC").unwrap_or_else(|_|"navkit_kafka_cassandra_20261106_161900_cdb67244".into());
    let root=format!("/navkit_live_{token}");let group=format!("{root}/group");let object=format!("{group}/item");
    let mqtt_topic=format!("navkit_live_{token}/item");
    let mut req=match kind {"kafka"=>request(kind,19092,&topic),"mqtt"=>request(kind,1883,&format!("navkit_live_{token}/#")),_=>request(kind,2181,"/")};
    db::open_and_register(&reg,&id,&req).await?;
    steps.push(json!({"step":"actual_product_authenticated_or_anonymous_session","pass":true,"auth_mode":"local acceptance server configured anonymous; password authentication not claimed"}));
    let result:Result<Value>=async {
        let (container,object)=match kind {
            "kafka"=>{
                for value in [&original,&changed] {
                    let sent=command(&reg,&id,&req,json!({"operation":"produce","topic":topic,"partition":0,"key":null,"value":value})).await?;
                    ensure!(sent.sets.iter().any(|s| !s.rows.is_empty()),"broker did not return acknowledged produce offsets");
                }
                (topic.clone(),"0".to_string())
            }
            "mqtt"=>{
                command(&reg,&id,&req,json!({"operation":"publish","topic":mqtt_topic,"payload":original,"retain":true})).await?;
                let read=command(&reg,&id,&req,json!({"operation":"subscribe","topic":mqtt_topic,"window_ms":300})).await?;
                ensure!(read.sets.iter().any(|s|has(&s.rows,&original)),"real MQTT PUBACK message was not observed");
                command(&reg,&id,&req,json!({"operation":"publish","topic":mqtt_topic,"payload":changed,"retain":true})).await?;
                (req.database.clone(),mqtt_topic.clone())
            }
            _=>{
                for (path,data) in [(&root,"owned acceptance root"),(&group,"owned group"),(&object,&*original)] {
                    command(&reg,&id,&req,json!({"operation":"create","path":path,"data":data})).await?;
                }
                command(&reg,&id,&req,json!({"operation":"set","path":object,"data":changed})).await?;
                db::close_with_derived(&reg,&id);req.database=root.clone();
                db::open_and_register(&reg,&id,&req).await?;
                (group.clone(),object.clone())
            }
        };
        steps.push(json!({"step":"actual_product_write_and_acknowledgement","pass":true,"dynamic_uuid":token}));
        let connection=db::get_db(&reg,&id)?;
        let containers=schema::list_databases(connection.as_ref(),req.kind.clone()).await?;
        ensure!(containers.iter().any(|(n,_)|n==&container),"live container missing: {containers:?}");
        let objects=schema::list_tables(connection.as_ref(),req.kind.clone(),&container).await?;
        ensure!(objects.iter().any(|(n,_)|n==&object),"live object missing: {objects:?}");
        let structure=schema::table_schema(connection.as_ref(),req.kind.clone(),&container,&object).await?;
        ensure!(!structure.columns.is_empty(),"live schema returned no columns");
        let all=schema::page_table(connection.as_ref(),req.kind.clone(),&page(&id,&container,&object,0,10000)).await?;
        ensure!(has(&all.rows,&changed),"live browse lost complete changed Unicode sentinel");
        ensure!(all.rows.iter().all(|r|r.len()==all.columns.len()),"row width differs from actual metadata");
        let zero=schema::page_table(connection.as_ref(),req.kind.clone(),&page(&id,&container,&object,0,0)).await?;
        ensure!(zero.rows.is_empty(),"zero limit was ignored");
        let at_one=schema::page_table(connection.as_ref(),req.kind.clone(),&page(&id,&container,&object,1,1)).await?;
        ensure!(at_one.rows.len()<=1,"offset/limit did not bound result");
        if kind=="kafka" {ensure!(at_one.rows==all.rows.iter().skip(1).take(1).cloned().collect::<Vec<_>>(),"Kafka paging differs from real offset");ensure!(all.rows.iter().any(|r|r.get(2)==Some(&None)&&r.get(3).and_then(Option::as_deref)==Some(changed.as_str())),"Kafka null key was not preserved");}
        if kind=="zookeeper" {ensure!(at_one.rows.is_empty(),"single znode ignored offset=1");}
        steps.push(json!({"step":"actual_metadata_browse_exact_unicode_and_paging","pass":true,"rows":all.rows.len(),"columns":all.columns.len(),"zero_limit":true}));
        let query=match kind {"kafka"=>json!({"operation":"fetch","topic":container,"partition":0,"offset":0,"limit":10000}),"mqtt"=>json!({"operation":"subscribe","topic":object,"window_ms":300}),_=>json!({"operation":"get","path":object})};
        let read=command(&reg,&id,&req,query).await?;
        ensure!(read.sets.iter().any(|s|has(&s.rows,&changed)),"query did not read back changed state");
        ensure!(command(&reg,&id,&req,json!({"operation":"INVALID"})).await.is_err(),"invalid operation succeeded");
        let invalid=match kind {"kafka"=>json!({"operation":"fetch","topic":container,"partition":999,"limit":1}),"mqtt"=>json!({"operation":"subscribe","topic":"invalid/#/suffix","window_ms":10}),_=>json!({"operation":"get","path":format!("{root}/missing")})};
        ensure!(command(&reg,&id,&req,invalid).await.is_err(),"invalid partition/filter/missing znode was accepted");
        let mut tls=req.clone();tls.params="ssl=true".into();ensure!(db::open(&tls).await.is_err(),"unsupported TLS silently downgraded");
        steps.push(json!({"step":"actual_read_after_mutation_and_negative_requests","pass":true,"unknown_command_rejected":true,"invalid_object_or_filter_rejected":true,"tls_downgrade_rejected":true}));
        Ok(json!({"sentinel_exact_unicode_preserved":true,"state_changed":true,"metadata":true,"paging":true,"auth_mode":"anonymous official loopback server"}))
    }.await;
    let mut cleanup_errors=Vec::new();
    if kind=="zookeeper" {
        for path in [&object,&group,&root] {
            if let Err(e)=command(&reg,&id,&req,json!({"operation":"delete","path":path})).await {cleanup_errors.push(format!("{path}: {e:#}"));}
        }
    } else if kind=="mqtt" {
        if let Err(e)=command(&reg,&id,&req,json!({"operation":"publish","topic":mqtt_topic,"payload":"","retain":true})).await {cleanup_errors.push(format!("{e:#}"));}
    }
    db::close_with_derived(&reg,&id);
    steps.push(json!({"step":"owned_fixture_cleanup","pass":cleanup_errors.is_empty(),"errors":cleanup_errors,"kafka_append_only_records_retained":kind=="kafka"}));
    ensure!(cleanup_errors.is_empty(),"owned cleanup failed: {cleanup_errors:?}");
    result
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
#[ignore="Requires explicitly started official loopback Kafka, Moquette MQTT and ZooKeeper services"]
async fn real_native_brokers_mutation_metadata_errors_and_paging() {
    assert_eq!(std::env::var("NAVKIT_LIVE_NATIVE").as_deref(),Ok("1"),"Explicit live service configuration required");
    let mut rows=Vec::new();
    for kind in ["kafka","mqtt","zookeeper"] {
        let begin=Instant::now();let mut steps=Vec::new();
        let outcome=tokio::time::timeout(Duration::from_secs(90),verify(kind,&mut steps)).await;
        let (status,details,error)=match outcome {Ok(Ok(details))=>("LIVE_REFERENCE_PASS",details,Value::Null),Ok(Err(e))=>("FAIL",Value::Null,json!(format!("{e:#}"))),Err(_)=>("FAIL",Value::Null,json!("90 second live verification deadline"))};
        println!("{kind}: {status} {error}");
        rows.push(json!({"kind":kind,"status":status,"evidence_type":"OFFICIAL_LOCAL_SERVICE_ACTUAL_PRODUCT_APIS","elapsed_ms":begin.elapsed().as_millis(),"steps":steps,"details":details,"error":error,"limitations":["Only configured official local version and anonymous acceptance settings tested","No authenticated production cluster, HA or TLS server certification claimed"]}));
    }
    let failed=rows.iter().filter(|r|r["status"]=="FAIL").count();
    let path=std::env::var("NAVKIT_LIVE_NATIVE_REPORT").map(PathBuf::from).unwrap_or_else(|_|Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/acceptance/simulation/native/live-brokers-report.json"));
    if let Some(p)=path.parent(){std::fs::create_dir_all(p).unwrap();}
    std::fs::write(path,serde_json::to_vec_pretty(&json!({"counts":{"covered":rows.len(),"pass":rows.len()-failed,"fail":failed},"rows":rows})).unwrap()).unwrap();
    assert_eq!(failed,0,"See per-kind real service evidence");
}
