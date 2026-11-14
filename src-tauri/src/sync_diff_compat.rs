//! Existing in-memory comparison kept as the default until the bounded strategy passes latency A/B.
//! It preserves existing protocol coverage; explicit bounded preview has separate resource limits.
use super::*;
pub(super) async fn compare(source:&Db,source_kind:&DbKind,target:&Db,target_kind:&DbKind,req:&SyncDiffRequest,
    source_meta:&crate::models::TableSchema,target_meta:Option<&crate::models::TableSchema>,cols:&[(&SyncColumnMapping,&ColumnInfo)],mut report:SyncDiffReport)->Result<SyncDiffReport>{
    let (snames,srows)=transfer::fetch_transfer_rows(source,source_kind,&req.source_schema,&req.source_table,req.where_clause.as_deref(),None).await?;
    report.source_rows=srows.len() as u64;
    let sk=cols.iter().filter(|(_,c)|c.primary_key).map(|(m,_)|m.source.clone()).collect::<Vec<_>>();
    let tk=cols.iter().filter(|(_,c)|c.primary_key).map(|(m,_)|m.target.clone()).collect::<Vec<_>>();
    let Some(target_meta)=target_meta else {
        report.missing_in_target=report.source_rows;report.keyed=!sk.is_empty();
        report.target_missing_columns=cols.iter().map(|(m,_)|m.target.clone()).collect();
        report.message=Some("目标表不存在：全部源行都将作为缺失行写入（结构模式或启用建表时自动创建）。".into());
        return Ok(report);
    };
    let (tnames,trows)=transfer::fetch_transfer_rows(target,target_kind,&req.target_schema,&req.target_table,None,None).await?;
    report.target_rows=trows.len() as u64;
    if sk.is_empty(){report.message=Some("源表没有主键：仅统计行数与结构差异；缺失/多出/内容差异需要主键才能比较。".into());return Ok(report);}
    report.keyed=true;
    let source_keys=keyed_rows(&snames,&srows,&sk,"源表")?;
    let target_keys=keyed_rows(&tnames,&trows,&tk,"目标表")?;
    report.missing_in_target=source_keys.keys().filter(|key|!target_keys.contains_key(*key)).count() as u64;
    report.extra_in_target=target_keys.keys().filter(|key|!source_keys.contains_key(*key)).count() as u64;
    for(key,row)in source_keys {
        let Some(target)=target_keys.get(&key) else {continue;};
        if row_differs(&snames,row,&tnames,target,cols,Some(target_meta))? {
            report.changed+=1;
            if report.changed_samples.len()<5 {report.changed_samples.push(sk.iter().map(|key|snames.iter().position(|name|name==key).and_then(|i|row.get(i)).and_then(Option::as_deref).unwrap_or("NULL").to_owned()).collect::<Vec<_>>().join(", "));}
        }
    }
    let _=source_meta; Ok(report)
}
