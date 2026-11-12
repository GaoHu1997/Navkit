//! 明确的五字段 Cron 与可注入时钟。IANA 解释统一使用 chrono-tz，不由浏览器触发。
//! DOM/DOW：含 * 的字段按传统 Cron AND 分支，否则两个受限字段采用 OR。
use super::contracts::*;
use anyhow::{anyhow, ensure, Result};
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use std::{collections::BTreeSet, sync::Mutex, time::Instant};

pub trait SchedulerClock: Send + Sync {
    fn now_utc(&self) -> DateTime<Utc>;
    fn monotonic_millis(&self) -> u64;
}
pub struct SystemSchedulerClock { started: Instant }
impl Default for SystemSchedulerClock { fn default() -> Self { Self { started:Instant::now() } } }
impl SchedulerClock for SystemSchedulerClock {
    fn now_utc(&self) -> DateTime<Utc> { Utc::now() }
    fn monotonic_millis(&self) -> u64 { self.started.elapsed().as_millis().min(u64::MAX as u128) as u64 }
}
/// 固定测试时钟可分别模拟墙钟回拨/休眠与单调时钟推进；生产不使用系统时间 monkey-patch。
pub struct ControlledSchedulerClock { state: Mutex<(DateTime<Utc>,u64)> }
impl ControlledSchedulerClock {
    pub fn new(now: DateTime<Utc>) -> Self { Self { state:Mutex::new((now,0)) } }
    pub fn set_wall(&self, now: DateTime<Utc>) { self.state.lock().expect("clock lock").0=now; }
    pub fn advance(&self, wall: Duration, monotonic_ms: u64) { let mut s=self.state.lock().expect("clock lock"); s.0 += wall; s.1=s.1.saturating_add(monotonic_ms); }
}
impl SchedulerClock for ControlledSchedulerClock {
    fn now_utc(&self) -> DateTime<Utc> { self.state.lock().expect("clock lock").0 }
    fn monotonic_millis(&self) -> u64 { self.state.lock().expect("clock lock").1 }
}

#[derive(Debug,Clone)]
struct CronField { values:Vec<u32>, wildcard:bool }
#[derive(Debug,Clone)]
pub struct Cron5 { minute:CronField, hour:CronField, day:CronField, month:CronField, weekday:CronField }
fn digits(s:&str) -> Result<u32> { ensure!(!s.is_empty() && s.len()<=3 && s.bytes().all(|b| b.is_ascii_digit()),"SCHEDULE_CRON_INVALID: Cron 只接受整数、*、列表、范围和步长"); s.parse().map_err(|_|anyhow!("SCHEDULE_CRON_INVALID: 数字超出范围")) }
fn parse_field(raw:&str,min:u32,max:u32,sunday:bool) -> Result<CronField> {
    ensure!(!raw.is_empty() && raw.len()<=256,"SCHEDULE_CRON_INVALID: 字段为空或过长");
    let mut values=BTreeSet::new(); let mut wildcard=false;
    for item in raw.split(',') {
        ensure!(!item.is_empty(),"SCHEDULE_CRON_INVALID: 列表不能有空项");
        let mut split=item.split('/'); let base=split.next().unwrap_or("");
        let step=split.next().map(digits).transpose()?.unwrap_or(1);
        ensure!(split.next().is_none() && step>0 && step<=max-min+1,"SCHEDULE_CRON_INVALID: 步长必须在字段范围以内且大于零");
        let (start,end)=if base=="*" { wildcard=true; (min,max) } else if let Some((a,b))=base.split_once('-') { ensure!(!b.contains('-'),"SCHEDULE_CRON_INVALID: 范围格式无效"); (digits(a)?,digits(b)?) } else { let n=digits(base)?; (n,if item.contains('/') {max}else{n}) };
        ensure!(start>=min && end<=max && start<=end,"SCHEDULE_CRON_INVALID: 值/范围超界，不接受环绕范围");
        let mut n=start; while n<=end { values.insert(if sunday && n==7 {0}else{n}); match n.checked_add(step) {Some(next)=>n=next,None=>break} }
    }
    ensure!(!values.is_empty(),"SCHEDULE_CRON_INVALID: 字段没有可执行值");
    Ok(CronField {values:values.into_iter().collect(),wildcard})
}
impl Cron5 {
    pub fn parse(expression:&str) -> Result<Self> {
        ensure!(expression.len()<=1024,"SCHEDULE_CRON_INVALID: Cron 表达式过长");
        let fields:Vec<_>=expression.split_whitespace().collect();
        ensure!(fields.len()==5,"SCHEDULE_CRON_FIELDS: 必须恰好五字段：分 时 日 月 星期；不接受秒/年/@别名");
        Ok(Self {minute:parse_field(fields[0],0,59,false)?,hour:parse_field(fields[1],0,23,false)?,day:parse_field(fields[2],1,31,false)?,month:parse_field(fields[3],1,12,false)?,weekday:parse_field(fields[4],0,7,true)?})
    }
    pub fn matches_date(&self,date:NaiveDate) -> bool {
        if self.month.values.binary_search(&date.month()).is_err() {return false;}
        let dom=self.day.values.binary_search(&date.day()).is_ok();
        let dow=self.weekday.values.binary_search(&date.weekday().num_days_from_sunday()).is_ok();
        if self.day.wildcard || self.weekday.wildcard {dom && dow}else{dom || dow}
    }
    fn day_candidates(&self,date:NaiveDate,zone:Tz) -> Vec<DateTime<Utc>> {
        if !self.matches_date(date) {return vec![];}
        let mut result=Vec::new();
        for &hour in &self.hour.values {for &minute in &self.minute.values {
            let local=date.and_hms_opt(hour,minute,0).expect("validated cron field");
            let instant=match zone.from_local_datetime(&local) {
                LocalResult::None=>continue,
                LocalResult::Single(value)=>value.with_timezone(&Utc),
                LocalResult::Ambiguous(first,last)=>first.with_timezone(&Utc).min(last.with_timezone(&Utc)),
            };
            result.push(instant);
        }}
        result.sort_unstable(); result.dedup(); result
    }
    pub fn next_after(&self,after:DateTime<Utc>,zone:Tz) -> Result<DateTime<Utc>> {
        // 两个额外本地日涵盖历史国际日期线 ±24h 变化；按 UTC 取最小而不是凭墙钟顺序。
        let start=after.with_timezone(&zone).date_naive().pred_opt().ok_or_else(||anyhow!("SCHEDULE_TIME_RANGE"))?;
        let mut best:Option<DateTime<Utc>>=None; let mut found_day=None;
        for i in 0..=366*8+3 {
            let date=start.checked_add_signed(Duration::days(i)).ok_or_else(||anyhow!("SCHEDULE_TIME_RANGE: 日期超界"))?;
            if let Some(value)=self.day_candidates(date,zone).into_iter().filter(|t|*t>after).min() {
                best=Some(best.map_or(value,|old|old.min(value))); if found_day.is_none() {found_day=Some(i);}
            }
            if found_day.is_some_and(|day|i>=day+2) {return best.ok_or_else(||anyhow!("SCHEDULE_NO_OCCURRENCE"));}
        }
        Err(anyhow!("SCHEDULE_NO_OCCURRENCE: 八年内无匹配时刻（检查日期或时区）"))
    }
    pub fn latest_at_or_before(&self,before:DateTime<Utc>,zone:Tz) -> Result<DateTime<Utc>> {
        let start=before.with_timezone(&zone).date_naive().succ_opt().ok_or_else(||anyhow!("SCHEDULE_TIME_RANGE"))?;
        let mut best:Option<DateTime<Utc>>=None; let mut found_day=None;
        for i in 0..=366*8+3 {
            let date=start.checked_sub_signed(Duration::days(i)).ok_or_else(||anyhow!("SCHEDULE_TIME_RANGE: 日期超界"))?;
            if let Some(value)=self.day_candidates(date,zone).into_iter().filter(|t|*t<=before).max() {
                best=Some(best.map_or(value,|old|old.max(value))); if found_day.is_none() {found_day=Some(i);}
            }
            if found_day.is_some_and(|day|i>=day+2) {return best.ok_or_else(||anyhow!("SCHEDULE_NO_OCCURRENCE"));}
        }
        Err(anyhow!("SCHEDULE_NO_OCCURRENCE: 八年内无匹配时刻"))
    }
}
pub fn timezone(id:&str) -> Result<Tz> { ensure!(!id.is_empty() && id.len()<=128 && !id.chars().any(char::is_control),"SCHEDULE_TIMEZONE_INVALID: IANA 时区为空/非法"); id.parse().map_err(|_|anyhow!("SCHEDULE_TIMEZONE_INVALID: 未知 IANA 时区，不接受系统显示名或隐式本地时区")) }
pub fn validate_schedule(schedule:&ScheduleSpec) -> Result<()> {
    match schedule {
        ScheduleSpec::Manual=>{},
        ScheduleSpec::Interval {seconds,anchor_utc}=>{ ensure!((1..=31_536_000).contains(seconds),"SCHEDULE_INTERVAL_INVALID: 间隔必须是 1–31536000 秒"); ensure!((1970..=9999).contains(&anchor_utc.year()),"SCHEDULE_ANCHOR_INVALID: UTC 锚点必须在 1970–9999 年之间"); },
        ScheduleSpec::Calendar {cron,timezone:id,..}=>{ let parsed=Cron5::parse(cron)?; let zone=timezone(id)?; parsed.next_after(Utc.with_ymd_and_hms(2026,1,1,0,0,0).single().expect("date"),zone)?; },
    }
    Ok(())
}
fn interval_at(anchor:DateTime<Utc>,seconds:u64,n:i128) -> Result<DateTime<Utc>> {let secs=anchor.timestamp() as i128 + n * seconds as i128; let secs=i64::try_from(secs).map_err(|_|anyhow!("SCHEDULE_TIME_RANGE"))?; DateTime::from_timestamp(secs,anchor.timestamp_subsec_nanos()).ok_or_else(||anyhow!("SCHEDULE_TIME_RANGE: 时间超界"))}
pub fn next_run_after(schedule:&ScheduleSpec,after:DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
    match schedule {
        ScheduleSpec::Manual=>Ok(None),
        ScheduleSpec::Interval {seconds,anchor_utc}=>{ensure!(*seconds>0,"SCHEDULE_INTERVAL_INVALID"); if *anchor_utc>after {return Ok(Some(*anchor_utc));} let n=(after-*anchor_utc).num_seconds() as i128 / *seconds as i128+1; Ok(Some(interval_at(*anchor_utc,*seconds,n)?))},
        ScheduleSpec::Calendar {cron,timezone:id,..}=>Ok(Some(Cron5::parse(cron)?.next_after(after,timezone(id)?)?)),
    }
}
pub fn latest_run_at_or_before(schedule:&ScheduleSpec,before:DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
    match schedule {
        ScheduleSpec::Manual=>Ok(None),
        ScheduleSpec::Interval {seconds,anchor_utc}=>{ensure!(*seconds>0,"SCHEDULE_INTERVAL_INVALID"); if *anchor_utc>before {return Ok(None);} let n=(before-*anchor_utc).num_seconds() as i128 / *seconds as i128; Ok(Some(interval_at(*anchor_utc,*seconds,n)?))},
        ScheduleSpec::Calendar {cron,timezone:id,..}=>Ok(Some(Cron5::parse(cron)?.latest_at_or_before(before,timezone(id)?)?)),
    }
}
#[derive(Debug,Clone,Serialize,PartialEq,Eq)]
pub struct NextOccurrence {pub utc:DateTime<Utc>,pub local:String,pub timezone:String}
pub fn preview_next_runs(schedule:&ScheduleSpec,after:DateTime<Utc>,count:usize) -> Result<Vec<NextOccurrence>> {
    ensure!((1..=5).contains(&count),"SCHEDULE_PREVIEW_LIMIT: 最多预览五次"); validate_schedule(schedule)?;
    let zone=match schedule {ScheduleSpec::Calendar {timezone:id,..}=>timezone(id)?,_=>chrono_tz::UTC};
    let mut cursor=after; let mut out=vec![];
    for _ in 0..count {let Some(next)=next_run_after(schedule,cursor)? else {break}; out.push(NextOccurrence {utc:next,local:next.with_timezone(&zone).format("%Y-%m-%d %H:%M:%S %:z").to_string(),timezone:zone.name().into()}); cursor=next;}
    Ok(out)
}
#[derive(Debug,Clone,PartialEq,Eq)]
pub struct DueDecision {pub run_at:Option<DateTime<Utc>>,pub next_run_utc:Option<DateTime<Utc>>,pub misfired:bool}
pub fn decide_due(schedule:&ScheduleSpec,due:DateTime<Utc>,now:DateTime<Utc>,policy:MisfirePolicy) -> Result<DueDecision> {
    if due>now {return Ok(DueDecision {run_at:None,next_run_utc:Some(due),misfired:false});}
    let misfired=(now-due).num_milliseconds()>TICK_LATE_TOLERANCE_MS;
    let run_at=if !misfired {Some(due)}else{match policy {MisfirePolicy::Skip=>None,MisfirePolicy::LatestOnce=>latest_run_at_or_before(schedule,now)?.filter(|t|*t>=due)}};
    Ok(DueDecision {run_at,next_run_utc:next_run_after(schedule,now.max(due))?,misfired})
}

#[cfg(test)] mod tests {
    use super::*;
    fn utc(s:&str)->DateTime<Utc>{DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)}
    fn calendar(cron:&str,zone:&str)->ScheduleSpec{ScheduleSpec::Calendar {cron:cron.into(),timezone:zone.into(),dst_gap:DstGap::Skip,dst_fold:DstFold::First}}
    #[test] fn five_fields_and_bad_ranges(){for x in ["* * * *","* * * * * *","@daily","*/0 * * * *","60 * * * *","0 24 * * *","0 0 0 * *","0 0 * 13 *","0 0 * * 8","1,,2 * * * *","0 0 * * 6-1"]{assert!(Cron5::parse(x).is_err(),"{x}");}}
    #[test] fn lists_ranges_steps(){let c=Cron5::parse("1,10-20/5 */6 * * 0,7").unwrap();assert_eq!(c.minute.values,vec![1,10,15,20]);assert_eq!(c.hour.values,vec![0,6,12,18]);assert_eq!(c.weekday.values,vec![0]);}
    #[test] fn traditional_dom_dow_or_and_wildcards(){let c=Cron5::parse("0 0 13 * 1").unwrap();assert!(c.matches_date(NaiveDate::from_ymd_opt(2026,1,12).unwrap()));assert!(c.matches_date(NaiveDate::from_ymd_opt(2026,1,13).unwrap()));assert!(!c.matches_date(NaiveDate::from_ymd_opt(2026,1,14).unwrap()));let c=Cron5::parse("0 0 */2 * 1").unwrap();assert!(!c.matches_date(NaiveDate::from_ymd_opt(2026,1,12).unwrap()));assert!(c.matches_date(NaiveDate::from_ymd_opt(2026,1,19).unwrap()));}
    #[test] fn dst_gap_skips_nonexistent_wall_time(){let s=calendar("30 2 * * *","America/New_York");assert_eq!(next_run_after(&s,utc("2026-03-08T00:00:00Z")).unwrap(),Some(utc("2026-03-09T06:30:00Z")));}
    #[test] fn dst_fold_runs_only_earlier_instant(){let s=calendar("30 1 * * *","America/New_York");let first=next_run_after(&s,utc("2026-11-01T00:00:00Z")).unwrap().unwrap();assert_eq!(first,utc("2026-11-01T05:30:00Z"));assert_eq!(next_run_after(&s,first).unwrap(),Some(utc("2026-11-02T06:30:00Z")));assert_eq!(latest_run_at_or_before(&s,utc("2026-11-01T06:45:00Z")).unwrap(),Some(first));}
    #[test] fn half_hour_dst_and_iana_validation(){let s=calendar("15 2 * * *","Australia/Lord_Howe");let list=preview_next_runs(&s,utc("2026-10-03T13:00:00Z"),2).unwrap();assert_eq!(list[0].local,"2026-10-05 02:15:00 +11:00");assert!(timezone("China Standard Time").is_err());assert!(timezone("Asia/Shanghai").is_ok());}
    #[test] fn interval_anchor_survives_restart_and_clock_rewind(){let s=ScheduleSpec::Interval {seconds:60,anchor_utc:utc("2026-01-01T00:00:07Z")};assert_eq!(next_run_after(&s,utc("2026-01-01T00:05:59Z")).unwrap(),Some(utc("2026-01-01T00:06:07Z")));let due=utc("2026-01-01T00:06:07Z");let d=decide_due(&s,due,utc("2026-01-01T00:04:00Z"),MisfirePolicy::LatestOnce).unwrap();assert_eq!(d.next_run_utc,Some(due));assert!(d.run_at.is_none());}
    #[test] fn bounded_misfire_skip_or_only_latest(){let s=ScheduleSpec::Interval {seconds:60,anchor_utc:utc("2026-01-01T00:00:07Z")};let due=utc("2026-01-01T00:01:07Z");let now=utc("2026-06-01T00:00:50Z");let d=decide_due(&s,due,now,MisfirePolicy::LatestOnce).unwrap();assert_eq!(d.run_at,Some(utc("2026-06-01T00:00:07Z")));assert_eq!(d.next_run_utc,Some(utc("2026-06-01T00:01:07Z")));assert!(decide_due(&s,due,now,MisfirePolicy::Skip).unwrap().run_at.is_none());}
    #[test] fn leap_year_and_impossible_calendar(){let s=calendar("0 0 29 2 *","UTC");assert_eq!(next_run_after(&s,utc("2096-03-01T00:00:00Z")).unwrap(),Some(utc("2104-02-29T00:00:00Z")));assert!(validate_schedule(&calendar("0 0 31 2 *","UTC")).is_err());}
    #[test] fn interval_preserves_submillisecond_anchor(){let s=ScheduleSpec::Interval {seconds:60,anchor_utc:utc("2026-01-01T00:00:00.123456789Z")};assert_eq!(next_run_after(&s,utc("2026-01-01T00:00:59.999999999Z")).unwrap(),Some(utc("2026-01-01T00:01:00.123456789Z")));assert_eq!(latest_run_at_or_before(&s,utc("2026-01-01T00:01:00.123456788Z")).unwrap(),Some(utc("2026-01-01T00:00:00.123456789Z")));}
    #[test] fn preview_and_controlled_clock_are_exact(){let c=ControlledSchedulerClock::new(utc("2026-01-01T00:00:00Z"));c.advance(Duration::hours(-1),5000);assert_eq!(c.monotonic_millis(),5000);assert_eq!(c.now_utc(),utc("2025-12-31T23:00:00Z"));assert_eq!(preview_next_runs(&calendar("0 9 * * *","Asia/Shanghai"),utc("2026-01-01T00:00:00Z"),5).unwrap().len(),5);}
}
