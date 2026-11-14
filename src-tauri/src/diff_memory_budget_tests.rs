use super::*;
use std::sync::atomic::{AtomicU64,Ordering};
struct Injected(AtomicU64);
impl AvailableMemory for Injected { fn available(&self)->Result<u64> { let n=self.0.load(Ordering::SeqCst); if n==u64::MAX { bail!("injected sampler failure"); } Ok(n) } }
fn setup(n:u64)->(Arc<Injected>,Arc<Controller>) { let p=Arc::new(Injected(AtomicU64::new(n))); let c=Controller::new(p.clone()); (p,c) }
fn opts(n:u64)->DiffMemoryOptions { DiffMemoryOptions{mode:"manual".into(),limit_bytes:Some(n)} }
const M:u64=1024*1024;
#[test] fn minimum_invalid_and_clamp() {let (_,c)=setup(4*1024*M); assert!(c.start(&opts(20*M)).is_err());assert!(c.start(&DiffMemoryOptions{mode:"bad".into(),limit_bytes:None}).is_err()); assert!(c.start(&DiffMemoryOptions{mode:"manual".into(),limit_bytes:None}).is_err()); let p=c.preview(Some(&opts(u64::MAX))).unwrap();assert!(p.suggested_limit_bytes<4*1024*M);assert!(p.suggested_limit_bytes<=p.global_limit_bytes);}
#[test] fn failed_sampler_never_guesses() {for n in [0,128*M,u64::MAX] {let (_,c)=setup(n);assert!(c.start(&opts(64*M)).is_err());assert_eq!(c.used(),0);}}
#[test] fn reservations_fail_without_leaking() {let (_,c)=setup(4*1024*M);let mut b=c.start(&opts(64*M)).unwrap();assert!(b.reserve(17*M).is_err());assert_eq!(c.used(),WORKSPACE);b.reserve(8*M).unwrap();b.cancel_reservation(8*M);assert_eq!(c.used(),WORKSPACE);drop(b);assert_eq!(c.used(),0);}
#[test] fn concurrent_cap_and_release() {let (_,c)=setup(1024*M);let mut a=c.start(&opts(256*M)).unwrap();let mut b=c.start(&opts(256*M)).unwrap();a.reserve(170*M).unwrap();a.commit(170*M);assert!(b.reserve(170*M).is_err());drop(a);b.reserve(170*M).unwrap();b.commit(170*M);drop(b);assert_eq!(c.used(),0);}
#[test] fn normal_self_memory_decline_does_not_shrink_ceiling() {let(p,c)=setup(4*1024*M);let mut b=c.start(&opts(512*M)).unwrap();b.reserve(100*M).unwrap();b.commit(100*M);p.0.store(4*1024*M-148*M,Ordering::SeqCst);c.invalidate();b.reserve(100*M).unwrap();b.commit(100*M);assert_eq!(b.stats("memory").limit_bytes,512*M);assert_eq!(b.stats("memory").pressure_events,0);drop(b);assert_eq!(c.used(),0);}
#[test]
fn reader_prefetch_is_reserved_before_open_and_small_budget_stays_single_row() {
    let(_,controller)=setup(4*1024*M);
    let mut small=controller.start(&opts(64*M)).unwrap();
    assert_eq!(small.reader_buffer_rows(),1);assert_eq!(small.stats("memory").peak_bytes,WORKSPACE);
    drop(small);assert_eq!(controller.used(),0);
    let mut large=controller.start(&opts(512*M)).unwrap();
    assert_eq!(large.reader_buffer_rows(),8);
    assert_eq!(large.stats("memory").used_bytes,WORKSPACE+112*M);
    drop(large);assert_eq!(controller.used(),0);
}
#[test] fn external_pressure_and_failed_refresh_stop_growth() {let(p,c)=setup(4*1024*M);let mut b=c.start(&opts(512*M)).unwrap();p.0.store(16*M,Ordering::SeqCst);c.invalidate();assert!(b.reserve(M).is_err());p.0.store(u64::MAX,Ordering::SeqCst);c.invalidate();assert!(b.reserve(M).is_err());assert_eq!(b.stats("memory").pressure_events,2);drop(b);assert_eq!(c.used(),0);}
