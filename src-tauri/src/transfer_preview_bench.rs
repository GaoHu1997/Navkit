//! Test-only historical preview oracle and sampled process memory; no production fallback.
use super::*;
use std::io::Write;
#[path = "performance_rss_probe.rs"] mod memory;
struct Directory(std::path::PathBuf);
impl Directory {
    fn new()->Self {let root=std::env::temp_dir().join(format!("navkit-preview-ab-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&root).unwrap();Self(root)}
    fn path(&self,name:&str)->std::path::PathBuf {self.0.join(name)}
}
impl Drop for Directory {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
// Reconstruct only the previous preview implementation, using its actual read_text
// and CSV settings. This is confined to tests and inputs <= its old 256MiB limit.
fn previous_preview(path:&str,limit:usize)->Result<(Vec<String>,Vec<Vec<String>>)> {
    let text=read_text(path,"utf8")?;
    let mut csv=csv::ReaderBuilder::new().delimiter(b',').has_headers(false).flexible(false).from_reader(text.trim_start_matches('\u{feff}').as_bytes());
    let mut names=Vec::new();let mut rows=Vec::new();
    for (index,record) in csv.records().enumerate() {
        let values=record?.iter().map(str::to_owned).collect::<Vec<_>>();
        if index==0{names=values}else{rows.push(values)}
    }
    validate_names(&names)?;rows.truncate(limit.min(1000));Ok((names,rows))
}
#[test]
#[ignore="explicit valid-input preview latency/process-RSS A/B"]
fn preview_csv_actual_ab_benchmark() {
    let dir=Directory::new();let path=dir.path("valid.csv");
    let rows=std::env::var("NAVKIT_PREVIEW_BENCH_ROWS").ok().and_then(|v|v.parse::<usize>().ok()).unwrap_or(200_000);
    assert!((20..=1_000_000).contains(&rows));
    let mut writer=csv::Writer::from_path(&path).unwrap();writer.write_record(["id","v"]).unwrap();
    for index in 0..rows {writer.write_record([index.to_string(),format!("row-{index}-中文 quote\" comma, newline\n{}","payload".repeat(10))]).unwrap();}
    writer.flush().unwrap();drop(writer);
    let mut old=Vec::new();let mut new=Vec::new();
    for round in 0..6 {
        let modes=if round%2==0{[false,true]}else{[true,false]};
        for bounded in modes {
            let probe=memory::Probe::start();let start=std::time::Instant::now();
            let result=if bounded{preview_csv(path.to_str().unwrap(),",","utf8",20)}else{previous_preview(path.to_str().unwrap(),20)}.unwrap();
            let seconds=start.elapsed().as_secs_f64();
            assert_eq!(result.0,vec!["id","v"]);assert_eq!(result.1.len(),20);assert_eq!(result.1[19][0],"19");
            assert_eq!(result.1[0][1],format!("row-0-中文 quote\" comma, newline\n{}","payload".repeat(10)));
            println!("PREVIEW_AB round={round} mode={} source_rows={rows} source_bytes={} sample_rows=20 seconds={seconds:.6} rss={}",if bounded{"bounded"}else{"old_full_read"},std::fs::metadata(&path).unwrap().len(),probe.finish());
            if round>0{if bounded{new.push(seconds)}else{old.push(seconds)}}
        }
    }
    old.sort_by(f64::total_cmp);new.sort_by(f64::total_cmp);
    println!("PREVIEW_MEDIAN rows={rows} old_s={:.6} bounded_s={:.6} ratio={:.3} policy=valid_input_same_sample_not_full_file_validation",old[2],new[2],old[2]/new[2]);
}
#[test]
fn preview_oracle_matches_sample_for_valid_quoted_rows() {
    let dir=Directory::new();let path=dir.path("input.csv");
    let mut file=File::create(&path).unwrap();file.write_all("\u{feff}id,v\r\n1,\"中,quote\"\r\n2,\"a\nline\"\r\n".as_bytes()).unwrap();drop(file);
    for limit in [0,1,2,20]{assert_eq!(previous_preview(path.to_str().unwrap(),limit).unwrap(),preview_csv(path.to_str().unwrap(),",","utf8",limit).unwrap());}
}
