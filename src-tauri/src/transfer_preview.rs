//! CSV preview bounds decoding before csv::Reader allocates a whole hostile record.
use std::fs::File;
use std::io::{self, BufReader, Read};
use anyhow::{bail, Context, Result};
use crate::transfer_stream::{DecodeReader, DEFAULT_MAX_RECORD_BYTES};

const SAMPLE_BYTES: u64 = 8 * 1024 * 1024;
// Allows one final large record past the sample target, but never the entire input.
const DECODE_BYTES: u64 = SAMPLE_BYTES + DEFAULT_MAX_RECORD_BYTES;
struct BoundedRead<R> { inner: R, left: u64 }
impl<R: Read> Read for BoundedRead<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() { return Ok(0); }
        if self.left == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "CSV 预览解码超过字节预算；请缩小样本或检查超大记录"));
        }
        let allowed = bytes.len().min(self.left.min(usize::MAX as u64) as usize);
        let read = self.inner.read(&mut bytes[..allowed])?;
        self.left -= read as u64;
        Ok(read)
    }
}

pub(super) fn read(path: &str, delimiter: &str, encoding: &str, limit: usize) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let delimiter = super::validate_delimiter(delimiter)?;
    let file = File::open(path).with_context(|| format!("无法打开 CSV 预览文件：{path}"))?;
    if !file.metadata()?.is_file() { bail!("CSV 预览输入必须是普通文件"); }
    let decoded = DecodeReader::new(BufReader::new(file), encoding)?;
    let bounded = BoundedRead { inner: decoded, left: DECODE_BYTES };
    let mut reader = csv::ReaderBuilder::new().delimiter(delimiter as u8).has_headers(false).flexible(false).from_reader(bounded);
    let mut records = reader.records();
    let Some(header) = records.next() else { return Ok((Vec::new(), Vec::new())); };
    let header = header.context("CSV 表头无效")?;
    if header.as_slice().len() as u64 > DEFAULT_MAX_RECORD_BYTES { bail!("CSV 预览表头超过记录上限"); }
    let names = header.iter().map(str::to_owned).collect::<Vec<_>>();
    super::validate_names(&names)?;
    let mut rows = Vec::new();
    let mut sample_bytes = 0u64;
    for record in records.take(limit.min(1000)) {
        let record = record.context("CSV 预览记录无效")?;
        let bytes = record.as_slice().len() as u64;
        if bytes > DEFAULT_MAX_RECORD_BYTES { bail!("CSV 预览单条记录超过字节上限"); }
        sample_bytes = sample_bytes.checked_add(bytes).ok_or_else(|| anyhow::anyhow!("CSV 样本字节计数溢出"))?;
        rows.push(record.iter().map(str::to_owned).collect());
        if sample_bytes >= SAMPLE_BYTES { break; }
    }
    Ok((names, rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_budget_errors_instead_of_fabricating_eof() {
        let mut reader = BoundedRead { inner: &b"0123456789"[..], left: 4 };
        let mut bytes = [0u8; 8];
        assert_eq!(reader.read(&mut bytes).unwrap(), 4);
        assert_eq!(&bytes[..4], b"0123");
        assert_eq!(reader.read(&mut bytes).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
