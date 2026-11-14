use super::*;
fn meta()->crate::models::TableSchema {
    crate::models::TableSchema{table:"t".into(),database:"main".into(),columns:vec![ColumnInfo{name:"id".into(),data_type:"INTEGER".into(),nullable:false,default:None,primary_key:true,auto_increment:false,comment:None,length:None,precision:None,scale:None},ColumnInfo{name:"v".into(),data_type:"TEXT".into(),nullable:true,default:None,primary_key:false,auto_increment:false,comment:None,length:None,precision:None,scale:None}],indexes:vec![],foreign_keys:vec![],comment:None,engine:None,collation:None,row_count:None}
}
use super::super::super::diff_memory_budget::{AvailableMemory,Controller,DiffMemoryOptions};
use std::sync::{Arc,atomic::{AtomicU64,Ordering}};
struct Injected(AtomicU64);
impl AvailableMemory for Injected { fn available(&self)->Result<u64> { Ok(self.0.load(Ordering::SeqCst)) } }
fn budget(n:u64)->(Arc<Injected>,Arc<Controller>,Store) { let p=Arc::new(Injected(AtomicU64::new(4*1024*1024*1024)));let c=Controller::new(p.clone());let b=c.start(&DiffMemoryOptions{mode:"manual".into(),limit_bytes:Some(n)}).unwrap();(p,c,Store::with_budget(b)) }
fn report()->SyncDiffReport { SyncDiffReport{source_rows:0,target_rows:0,keyed:false,missing_in_target:0,extra_in_target:0,changed:0,changed_samples:vec![],target_missing_columns:vec![],target_extra_columns:vec![],message:None,memory:None} }
#[tokio::test]
async fn small_diff_uses_no_disk_and_keeps_exact_keys() {
    let names=vec!["id".into(),"v".into()];let keys=vec!["id".into()];let metadata=meta();let mut store=Store::new();
    store.push(true,&names,&[Some("1".into()),Some("old".into())],&keys,&metadata).await.unwrap();
    store.push(false,&names,&[Some("1".into()),Some("new".into())],&keys,&metadata).await.unwrap();
    assert!(store.disk.is_none());assert!(store.scratch.is_none());
    let mapping=vec![SyncColumnMapping{source:"id".into(),target:"id".into(),target_type:None},SyncColumnMapping{source:"v".into(),target:"v".into(),target_type:None}];
    let cols=mapping.iter().zip(&metadata.columns).collect::<Vec<_>>();let mut report=report();
    store.report(&names,&names,&cols,&keys,&metadata,&mut report).await.unwrap();assert_eq!(report.changed,1);assert_eq!(report.source_rows,1);store.close().await.unwrap();
}
#[tokio::test]
async fn large_diff_spills_once_migrates_records_and_cleans_file() {
    let names=vec!["id".into(),"v".into()];let keys=vec!["id".into()];let metadata=meta();let mut store=Store::new();
    for source in [true,false] { for id in 0..2200 {store.push(source,&names,&[Some(id.to_string()),Some("value'中".into())],&keys,&metadata).await.unwrap();} }
    assert!(store.source.is_empty()&&store.target.is_empty());let path=store.scratch.as_ref().unwrap().path.clone();assert!(path.exists());
    let mapping=vec![SyncColumnMapping{source:"id".into(),target:"id".into(),target_type:None},SyncColumnMapping{source:"v".into(),target:"v".into(),target_type:None}];
    let cols=mapping.iter().zip(&metadata.columns).collect::<Vec<_>>();let mut report=report();
    store.report(&names,&names,&cols,&keys,&metadata,&mut report).await.unwrap();
    assert_eq!((report.source_rows,report.target_rows,report.changed,report.missing_in_target,report.extra_in_target),(2200,2200,0,0,0));store.close().await.unwrap();assert!(!path.exists());
}
#[tokio::test]
async fn adaptive_100k_sufficient_budget_memory_and_64m_single_spill() {
    let names=vec!["id".into(),"v".into()];let keys=vec!["id".into()];let metadata=meta();
    for limit in [512u64*1024*1024,64u64*1024*1024] {
        let(_,controller,mut store)=budget(limit);
        for source in [true,false] { for id in 0..100000 {
            store.push_owned(source,&names,vec![Some(id.to_string()),Some("value'中\nNULL".into())],&keys,&metadata).await.unwrap();
        } }
        assert_eq!(store.disk.is_some(),limit==64*1024*1024);
        let path=store.scratch.as_ref().map(|s|s.path.clone());
        let mapping=vec![SyncColumnMapping{source:"id".into(),target:"id".into(),target_type:None},SyncColumnMapping{source:"v".into(),target:"v".into(),target_type:None}];
        let cols=mapping.iter().zip(&metadata.columns).collect::<Vec<_>>();let mut report=report();
        store.report(&names,&names,&cols,&keys,&metadata,&mut report).await.unwrap();
        assert_eq!((report.source_rows,report.target_rows,report.changed,report.missing_in_target,report.extra_in_target),(100000,100000,0,0,0));
        let stats=store.memory_stats(store.backend()).unwrap();assert!(stats.peak_bytes<=limit);assert_eq!(stats.spilled_bytes>0,limit==64*1024*1024);
        store.close().await.unwrap();assert_eq!(controller.used(),0);if let Some(path)=path {assert!(!path.exists());}
    }
}
#[tokio::test]
async fn adaptive_pressure_spills_same_read_and_reclaims_payloads() {
    let(p,c,mut store)=budget(512*1024*1024);let names=vec!["id".into(),"v".into()];let keys=vec!["id".into()];let metadata=meta();
    for id in 0..10000 {store.push_owned(true,&names,vec![Some(id.to_string()),Some("source".into())],&keys,&metadata).await.unwrap();}
    assert!(store.disk.is_none());
    // External headroom supports the promised workspace but not a new rehash.
    p.0.store(460*1024*1024,Ordering::SeqCst);c.invalidate();
    for id in 10000..20000 {store.push_owned(true,&names,vec![Some(id.to_string()),Some("source".into())],&keys,&metadata).await.unwrap();}
    assert!(store.disk.is_some());assert_eq!(store.source_rows,20000);assert!(store.adaptive.as_ref().unwrap().source.is_empty());
    assert!(store.memory_stats("disk").unwrap().pressure_events>0);
    store.close().await.unwrap();assert_eq!(c.used(),0);
}
#[tokio::test]
async fn adaptive_speed_and_pipeline_errors_cleanup_budget() {
    let names=vec!["id".into(),"v".into()];let keys=vec!["id".into()];let metadata=meta();
    let p=Arc::new(Injected(AtomicU64::new(4*1024*1024*1024)));let c=Controller::new(p.clone());
    let b=c.start(&DiffMemoryOptions{mode:"speed".into(),limit_bytes:None}).unwrap();let mut store=Store::with_budget(b);
    store.push_owned(true,&names,vec![Some("one".into()),None],&keys,&metadata).await.unwrap();
    p.0.store(1,Ordering::SeqCst);c.invalidate();let error=store.batch_boundary().await.unwrap_err();
    assert!(error.to_string().contains("MEMORY_BUDGET_EXCEEDED"));assert!(store.disk.is_none());store.close().await.unwrap();assert_eq!(c.used(),0);
    let(p,c,mut store)=budget(512*1024*1024);
    store.push_owned(true,&names,vec![Some("x".into()),None],&keys,&metadata).await.unwrap();
    assert!(store.push_owned(true,&names,vec![Some("x".into()),Some("duplicate".into())],&keys,&metadata).await.is_err());
    p.0.store(1,Ordering::SeqCst);c.invalidate();assert!(store.batch_boundary().await.is_err());
    let path=store.scratch.as_ref().map(|s|s.path.clone());store.close().await.unwrap();assert_eq!(c.used(),0);if let Some(path)=path {assert!(!path.exists());}
}
#[test]
fn budget_json_matches_complete_old_key_and_rejects_oversized_before_encoding() {
    let names=vec!["id".into(),"v".into()];let keys=vec!["id".into()];
    for value in ["", "NULL", "a\n\u{0000}\\\"中", "01", "1"] {
        let row=vec![Some(value.into()),None];assert_eq!(budget_key(&names,&row,&keys).unwrap(),row_key(&names,&row,&keys).unwrap());
        assert_eq!(budget_body(&row).unwrap(),serde_json::to_vec(&row).unwrap());
    }
    let row=vec![Some("\u{0000}".repeat(3*1024*1024)),None];assert!(checked_encoding(&names,&row,&keys).is_err());
}
