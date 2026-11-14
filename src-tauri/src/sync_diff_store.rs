//! Small previews avoid a disk database; larger previews spill once without rereading either source.
use super::*;
use std::collections::{BTreeMap, HashMap};
use super::super::diff_memory_budget::Budget;

#[cfg(test)]
#[path = "sync_diff_store_tests.rs"] mod tests;

const SMALL_ROWS: usize = 2048;
const SMALL_BYTES: u64 = 2 * 1024 * 1024;
struct Context { names: Vec<String>, keys: Vec<String>, metadata: crate::models::TableSchema }
pub(super) struct Store {
    disk: Option<Index>, scratch: Option<Scratch>,
    source: BTreeMap<String, Vec<Option<String>>>, target: BTreeMap<String, Vec<Option<String>>>,
    source_context: Option<Context>, target_context: Option<Context>,
    memory_bytes: u64, pub(super) source_rows: u64, pub(super) target_rows: u64,
    // Lease and owned hash maps drop after all context/workspace buffers.
    adaptive: Option<Adaptive>,
}
impl Store {
    pub(super) fn new() -> Self {
        Self { disk: None, scratch: None, adaptive: None, source: BTreeMap::new(), target: BTreeMap::new(),
            source_context: None, target_context: None, memory_bytes: 0, source_rows: 0, target_rows: 0 }
    }
    pub(super) fn with_budget(budget: Budget) -> Self {
        let mut store = Self::new();
        store.adaptive = Some(Adaptive { budget, source: HashMap::new(), target: HashMap::new(), source_capacity: 0, target_capacity: 0 });
        store
    }
    pub(super) fn is_adaptive(&self) -> bool { self.adaptive.is_some() }
    pub(super) fn memory_stats(&self, backend: &str) -> Option<DiffMemoryStats> {
        self.adaptive.as_ref().map(|a| a.budget.stats(backend))
    }
    pub(super) fn backend(&self) -> &'static str { if self.disk.is_some() { "disk" } else { "memory" } }
    pub(super) async fn batch_boundary(&mut self) -> Result<()> {
        let check = self.adaptive.as_mut().map(|a|a.budget.reserve(0)).unwrap_or(Ok(()));
        if let Err(error) = check {
            if self.disk.is_none() && self.adaptive.as_ref().is_some_and(|a|a.budget.allow_spill()) {
                self.spill_adaptive().await?;
                // If even the promised minimum workspace cannot be honoured, fail
                // explicitly after reclaiming; never announce an imaginary RSS cap.
                self.adaptive.as_mut().unwrap().budget.reserve(0)?;
            } else { return Err(error); }
        }
        Ok(())
    }
    // Native reader consumes its decoded row: no unreserved full-row clone.
    pub(super) async fn push_owned(&mut self, source: bool, names: &[String], row: Vec<Option<String>>, keys: &[String], meta: &crate::models::TableSchema) -> Result<()> {
        if self.adaptive.is_none() { return self.push(source,names,&row,keys,meta).await; }
        validate_record(names,&row,meta)?;
        checked_encoding(names,&row,keys)?;
        let count = if source { self.source_rows } else { self.target_rows };
        if count >= MAX_ROWS { bail!("差异预览单侧行数超过1000万预算"); }
        let context = if source { &mut self.source_context } else { &mut self.target_context };
        if context.is_none() { *context = Some(Context { names: names.to_vec(), keys: keys.to_vec(), metadata: meta.clone() }); }
        if let Some(disk) = &mut self.disk { disk.push_budgeted(source,names,&row,keys,meta).await?; }
        else {
            // Encoding is bounded before allocating, inside the already reserved workspace.
            let key = budget_key(names,&row,keys)?;
            let payload = owned_cost(&key,&row)?;
            let adaptive = self.adaptive.as_mut().ok_or_else(||anyhow!("内存预算缺失"))?;
            let records = if source { &adaptive.source } else { &adaptive.target };
            if records.contains_key(&key) { bail!("{}存在重复主键值，无法安全比较",if source {"源表"} else {"目标表"}); }
            let reservation = adaptive.reserve_entry(source,payload);
            match reservation {
                Ok(()) => {
                    let adaptive = self.adaptive.as_mut().unwrap();
                    let records = if source { &mut adaptive.source } else { &mut adaptive.target };
                    records.insert(key,row);
                    adaptive.budget.commit(payload);
                    if source { self.source_rows += 1; } else { self.target_rows += 1; }
                    return Ok(());
                }
                Err(error) => {
                    if !self.adaptive.as_ref().unwrap().budget.allow_spill() { return Err(error); }
                    drop(key);
                    self.spill_adaptive().await?;
                    self.disk.as_mut().ok_or_else(||anyhow!("差异spill未初始化"))?.push_budgeted(source,names,&row,keys,meta).await?;
                }
            }
        }
        if source { self.source_rows += 1; } else { self.target_rows += 1; }
        Ok(())
    }
    async fn spill_adaptive(&mut self) -> Result<()> {
        if self.disk.is_some() { return Ok(()); }
        let scratch = Scratch::new()?;
        let disk = Index::open(&scratch).await?;
        self.scratch = Some(scratch); self.disk = Some(disk);
        for source in [true,false] {
            // Owned iterator retains only the old bucket allocation; row payloads are
            // moved to disk in <=128-row batches and dropped/released progressively.
            let adaptive = self.adaptive.as_mut().ok_or_else(||anyhow!("预算缺失"))?;
            let records = if source { std::mem::take(&mut adaptive.source) } else { std::mem::take(&mut adaptive.target) };
            let capacity = if source { std::mem::take(&mut adaptive.source_capacity) } else { std::mem::take(&mut adaptive.target_capacity) };
            let context = if source { self.source_context.as_ref() } else { self.target_context.as_ref() };
            let mut pending = 0usize;
            for (key,row) in records {
                let cost = owned_cost(&key,&row)?;
                let context = context.ok_or_else(||anyhow!("差异缓存列身份缺失"))?;
                let disk = self.disk.as_mut().unwrap();
                disk.push_budgeted(source,&context.names,&row,&context.keys,&context.metadata).await?;
                drop(key); drop(row);
                self.adaptive.as_mut().unwrap().budget.release(cost);
                pending += 1;
                if pending == 128 { disk.flush().await?; pending = 0; }
            }
            // Iterator is gone: only NOW release the retained hash bucket capacity.
            self.disk.as_mut().unwrap().flush().await?;
            self.adaptive.as_mut().unwrap().budget.release(capacity);
        }
        Ok(())
    }
    async fn spill(&mut self) -> Result<()> {
        if self.disk.is_some() { return Ok(()); }
        let scratch = Scratch::new()?;
        let disk = Index::open(&scratch).await?;
        self.scratch = Some(scratch); self.disk = Some(disk);
        for source in [true, false] {
            let records = if source { std::mem::take(&mut self.source) } else { std::mem::take(&mut self.target) };
            if records.is_empty() { continue; }
            let context = if source { self.source_context.as_ref() } else { self.target_context.as_ref() }
                .ok_or_else(|| anyhow!("差异缓存的列身份缺失"))?;
            let disk = self.disk.as_mut().ok_or_else(|| anyhow!("差异磁盘索引缺失"))?;
            for (_, row) in records {
                disk.push(source, &context.names, &row, &context.keys, &context.metadata).await?;
            }
        }
        self.memory_bytes = 0;
        Ok(())
    }
    pub(super) async fn push(&mut self, source: bool, names: &[String], row: &[Option<String>], keys: &[String], meta: &crate::models::TableSchema) -> Result<()> {
        let context = if source { &mut self.source_context } else { &mut self.target_context };
        if context.is_none() { *context = Some(Context { names: names.to_vec(), keys: keys.to_vec(), metadata: meta.clone() }); }
        if let Some(disk) = &mut self.disk {
            disk.push(source, names, row, keys, meta).await?;
        } else {
            validate_record(names, row, meta)?;
            for name in keys {
                let index = names.iter().position(|column| column == name).ok_or_else(|| anyhow!("差异键列缺失：{name}"))?;
                if row[index].is_none() { bail!("差异键 {name} 为 NULL，无法安全比较"); }
            }
            let key = row_key(names, row, keys).ok_or_else(|| anyhow!("差异完整键缺失"))?;
            let body = serde_json::to_vec(row)?;
            if body.len() > RECORD_BYTES || key.len() > RECORD_BYTES { bail!("差异记录编码超过预算"); }
            let cost = (key.len() as u64).saturating_mul(3).saturating_add(body.len() as u64).saturating_add(256);
            let records = if source { &self.source } else { &self.target };
            if records.contains_key(&key) { bail!("{}存在重复主键值，无法安全比较", if source { "源表" } else { "目标表" }); }
            if self.source.len() + self.target.len() >= SMALL_ROWS || self.memory_bytes.saturating_add(cost) > SMALL_BYTES {
                // Release the encoded sizing copy before creating/migrating the disk index.
                drop(body);
                self.spill().await?;
                self.disk.as_mut().ok_or_else(|| anyhow!("差异spill未初始化"))?.push(source, names, row, keys, meta).await?;
            } else {
                drop(body);
                if source { self.source.insert(key, row.to_vec()); } else { self.target.insert(key, row.to_vec()); }
                self.memory_bytes += cost;
            }
        }
        if source { self.source_rows += 1; } else { self.target_rows += 1; }
        Ok(())
    }
    pub(super) async fn report(&mut self, snames: &[String], tnames: &[String], cols: &[(&SyncColumnMapping,&ColumnInfo)], pk: &[String], target_meta: &crate::models::TableSchema, report: &mut SyncDiffReport) -> Result<()> {
        if let Some(disk) = &mut self.disk {
            disk.report(snames, tnames, cols, pk, target_meta, report).await?;
            if let Some(a) = &mut self.adaptive {
                let actual = std::fs::metadata(&disk.path)?.len();
                a.budget.spill_bytes(actual);
            }
            return Ok(());
        }
        if let Some(a) = &self.adaptive {
            report.source_rows = self.source_rows; report.target_rows = self.target_rows; report.keyed = true;
            report.missing_in_target = a.source.keys().filter(|key| !a.target.contains_key(*key)).count() as u64;
            report.extra_in_target = a.target.keys().filter(|key| !a.source.contains_key(*key)).count() as u64;
            // Keep the old exact numeric-affinity checks, including exponent rejection.
            let numeric_meta = crate::models::TableSchema { columns: cols.iter().map(|(m,c)| {
                let mut column = (**c).clone();
                if target_meta.columns.iter().any(|tc|tc.name==m.target && numeric_family(&tc.data_type)) { column.data_type="NUMERIC".into(); }
                column
            }).collect(), ..target_meta.clone() };
            let mut target_numeric = target_meta.clone();
            for column in &mut target_numeric.columns {
                if cols.iter().any(|(m,c)|m.target==column.name && numeric_family(&c.data_type)) { column.data_type="NUMERIC".into(); }
            }
            let comparison = cols.iter().map(|(mapping,column)| {
                let source = snames.iter().position(|name|name==&mapping.source).ok_or_else(||anyhow!("数据列缺失: {}",mapping.source))?;
                let target = tnames.iter().position(|name|name==&mapping.target);
                let numeric = numeric_family(&column.data_type) || target_meta.columns.iter().find(|c|c.name==mapping.target).is_some_and(|c|numeric_family(&c.data_type));
                Ok((source,target,numeric))
            }).collect::<Result<Vec<_>>>()?;
            for (key,source) in &a.source {
                let Some(target) = a.target.get(key) else { continue; };
                // The same numeric-expansion guards still run before comparing.
                validate_record(snames,source,&numeric_meta)?; validate_record(tnames,target,&target_numeric)?;
                let differs = comparison.iter().any(|(si,ti,numeric)| {
                    let left = source.get(*si).and_then(Option::as_deref);
                    let right = ti.and_then(|ti|target.get(ti)).and_then(Option::as_deref);
                    // Identical byte strings (or both NULL) necessarily yield the
                    // same deterministic normalized_cell; avoid two clones/parses.
                    left != right && normalized_cell(left,*numeric) != normalized_cell(right,*numeric)
                });
                if differs {
                    report.changed += 1;
                    if report.changed_samples.len() < 5 {
                        report.changed_samples.push(pk.iter().map(|key| snames.iter().position(|n|n==key).and_then(|i|source.get(i)).and_then(Option::as_deref).unwrap_or("NULL").to_owned()).collect::<Vec<_>>().join(", "));
                    }
                }
            }
            return Ok(());
        }
        report.source_rows = self.source_rows; report.target_rows = self.target_rows; report.keyed = true;
        report.missing_in_target = self.source.keys().filter(|key| !self.target.contains_key(*key)).count() as u64;
        report.extra_in_target = self.target.keys().filter(|key| !self.source.contains_key(*key)).count() as u64;
        let numeric_meta = crate::models::TableSchema { columns: cols.iter().map(|(mapping,column)| {
            let mut column = (**column).clone();
            if target_meta.columns.iter().any(|c| c.name == mapping.target && numeric_family(&c.data_type)) { column.data_type = "NUMERIC".into(); }
            column
        }).collect(), ..target_meta.clone() };
        let mut target_numeric = target_meta.clone();
        for column in &mut target_numeric.columns {
            if cols.iter().any(|(mapping,c)| mapping.target == column.name && numeric_family(&c.data_type)) { column.data_type = "NUMERIC".into(); }
        }
        for (key, source) in &self.source {
            let Some(target) = self.target.get(key) else { continue; };
            validate_record(snames, source, &numeric_meta)?; validate_record(tnames, target, &target_numeric)?;
            if row_differs(snames, source, tnames, target, cols, Some(target_meta))? {
                report.changed += 1;
                if report.changed_samples.len() < 5 {
                    report.changed_samples.push(pk.iter().map(|key| {
                        snames.iter().position(|name| name == key).and_then(|i| source.get(i)).and_then(Option::as_deref).unwrap_or("NULL").to_owned()
                    }).collect::<Vec<_>>().join(", "));
                }
            }
        }
        Ok(())
    }
    pub(super) async fn close(mut self) -> Result<()> {
        let close = if let Some(disk) = self.disk.take() { disk.connection.close().await } else { Ok(()) };
        let cleanup = self.scratch.take().map(|scratch| scratch.clean()).unwrap_or(Ok(()));
        match (close, cleanup) { (Ok(()),Ok(())) => Ok(()), (close, cleanup) => Err(anyhow!("差异索引资源清理未确认：close={} cleanup={}",close.is_ok(),cleanup.is_ok())) }
    }
}

struct Adaptive {
    source: HashMap<String, Vec<Option<String>>>, target: HashMap<String, Vec<Option<String>>>,
    source_capacity: u64, target_capacity: u64,
    // Drop LAST, after all owned maps/capacities have actually been dropped.
    budget: Budget,
}
impl Adaptive {
    fn reserve_entry(&mut self, source: bool, payload: u64) -> Result<()> {
        let records = if source { &mut self.source } else { &mut self.target };
        let charged = if source { &mut self.source_capacity } else { &mut self.target_capacity };
        if records.len() == records.capacity() {
            let needed = records.len().checked_add(1).ok_or_else(||anyhow!("索引容量溢出"))?;
            // HashMap implementation has opaque node/control overhead. Charge a
            // deliberately conservative power-of-two bucket upper bound, including
            // the ENTIRE new table while the old table is still alive in rehash.
            let buckets = needed.checked_next_power_of_two().and_then(|n|n.checked_mul(2)).ok_or_else(||anyhow!("索引容量溢出"))?;
            let new_cost = (buckets as u64).checked_mul(128).ok_or_else(||anyhow!("索引计费溢出"))?;
            self.budget.reserve(new_cost)?;
            if let Err(error) = records.try_reserve(1) {
                self.budget.cancel_reservation(new_cost);
                return Err(anyhow!("MEMORY_ALLOCATION_FAILED: 索引try_reserve失败，未重跑/伪造报告: {error}"));
            }
            self.budget.commit(new_cost);
            self.budget.release(*charged);
            *charged = new_cost;
        }
        self.budget.reserve(payload)
    }
}
fn owned_cost(key: &String, row: &Vec<Option<String>>) -> Result<u64> {
    let mut bytes = key.capacity().checked_add(row.capacity().checked_mul(std::mem::size_of::<Option<String>>()).ok_or_else(||anyhow!("行容量溢出"))?).and_then(|n|n.checked_add(64)).ok_or_else(||anyhow!("行容量溢出"))?;
    for value in row.iter().flatten() { bytes = bytes.checked_add(value.capacity()).and_then(|n|n.checked_add(32)).ok_or_else(||anyhow!("字符串容量溢出"))?; }
    Ok(bytes as u64)
}
// Exact UTF-8 JSON escaped-size pass over borrowed cells, not allocating a JSON
// sizing copy and pretending it was a pre-allocation check. The workspace already
// covers decoding; this verifies bounded encoding/comparison before row_key/push.
fn json_cell_bytes(value: Option<&str>) -> Result<usize> {
    let Some(value) = value else { return Ok(4); };
    value.bytes().try_fold(2usize, |n,b|n.checked_add(match b { b'"'|b'\\'|b'\n'|b'\r'|b'\t'|8|12=>2, 0..=31=>6, _=>1 }).ok_or_else(||anyhow!("JSON长度溢出")))
}
pub(super) fn budget_body(row: &[Option<String>]) -> Result<Vec<u8>> {
    let size = row.iter().try_fold(2usize, |n,v|n.checked_add(json_cell_bytes(v.as_deref())?).and_then(|n|n.checked_add(1)).ok_or_else(||anyhow!("JSON长度溢出")))?;
    if size>RECORD_BYTES { bail!("差异编码超过16MiB工作预算"); }
    let mut body=Vec::new();
    body.try_reserve_exact(size).map_err(|e|anyhow!("MEMORY_ALLOCATION_FAILED: JSON暂存申请失败: {e}"))?;
    serde_json::to_writer(&mut body,row)?;
    Ok(body)
}
pub(super) fn budget_key(names: &[String], row: &[Option<String>], keys: &[String]) -> Result<String> {
    let mut values=Vec::new();
    values.try_reserve_exact(keys.len()).map_err(|e|anyhow!("MEMORY_ALLOCATION_FAILED: 主键列暂存失败: {e}"))?;
    let mut size=2usize;
    for key in keys {
        let i=names.iter().position(|n|n==key).ok_or_else(||anyhow!("差异键列缺失: {key}"))?;
        let value=row.get(i).ok_or_else(||anyhow!("差异完整键缺失"))?;
        if value.is_none() { bail!("差异键 {key} 为NULL，无法安全比较"); }
        size=size.checked_add(json_cell_bytes(value.as_deref())?).and_then(|n|n.checked_add(1)).ok_or_else(||anyhow!("主键编码长度溢出"))?;
        values.push(value);
    }
    if size>RECORD_BYTES { bail!("差异完整主键编码超过16MiB预算"); }
    let mut buffer=Vec::new();
    buffer.try_reserve_exact(size).map_err(|e|anyhow!("MEMORY_ALLOCATION_FAILED: JSON主键暂存申请失败: {e}"))?;
    serde_json::to_writer(&mut buffer,&values)?;
    String::from_utf8(buffer).map_err(Into::into)
}
fn checked_encoding(names: &[String], row: &[Option<String>], keys: &[String]) -> Result<()> {
    let body = row.iter().try_fold(2usize, |n,v|n.checked_add(json_cell_bytes(v.as_deref())?).and_then(|n|n.checked_add(1)).ok_or_else(||anyhow!("记录长度溢出")))?;
    let mut key_bytes = 2usize;
    for key in keys {
        let i = names.iter().position(|n|n==key).ok_or_else(||anyhow!("差异键列缺失：{key}"))?;
        let value = row.get(i).and_then(Option::as_deref).ok_or_else(||anyhow!("差异键 {key} 为NULL，无法安全比较"))?;
        key_bytes = key_bytes.checked_add(json_cell_bytes(Some(value))?).and_then(|n|n.checked_add(1)).ok_or_else(||anyhow!("键长度溢出"))?;
    }
    if body > RECORD_BYTES || key_bytes > RECORD_BYTES { bail!("差异记录编码超过16MiB预算，分配前拒绝而非截断"); }
    let decoded = row.iter().flatten().map(|s|s.capacity()).sum::<usize>() + row.len() * std::mem::size_of::<Option<String>>();
    // Conservative pair decode/normalization + two disk-body copies + key/bind
    // copies and up to five retained changed samples (sample construction briefly
    // clones then joins). Native/metadata slack is a soft guard, NOT a proof about
    // SQLx or RSS. Very wide legal rows may be explicitly rejected, never truncated.
    let transient = decoded.checked_mul(4).and_then(|n|n.checked_add(body.checked_mul(2)?)).and_then(|n|n.checked_add(key_bytes.checked_mul(7)?)).and_then(|n|n.checked_add(8*1024*1024)).ok_or_else(||anyhow!("临时工作区长度溢出"))?;
    if transient as u64 > super::super::diff_memory_budget::WORKSPACE { bail!("MEMORY_BUDGET_ROW_TOO_LARGE: 完整行需要超过48MiB最坏临时工作区；未截断/重跑"); }
    Ok(())
}
