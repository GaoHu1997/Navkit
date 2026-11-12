//! Tiberius 导出适配：只暴露固定会话锁及严格、逐单元格的无损解码。
use anyhow::{anyhow, bail, Result};
use chrono::{NaiveDateTime, Timelike};
use std::fmt::Write;
use tiberius::{Client, ColumnData, FromSql};
use tokio_util::compat::Compat;

impl super::MssqlDb {
    pub(crate) async fn export_session(&self) -> tokio::sync::MutexGuard<'_, Client<Compat<tokio::net::TcpStream>>> { self.client.lock().await }

    pub(crate) fn export_cell(cell: &ColumnData<'static>, budget: usize) -> Result<Option<String>> {
        macro_rules! text { ($value:expr) => { $value.as_ref().map(ToString::to_string) }; }
        fn datetime(value: NaiveDateTime) -> String { format!("{}.{:07}", value.format("%Y-%m-%d %H:%M:%S"), value.nanosecond()/100) }
        let value = match cell {
            ColumnData::U8(value) => text!(value), ColumnData::I16(value) => text!(value),
            ColumnData::I32(value) => text!(value), ColumnData::I64(value) => text!(value),
            ColumnData::F32(value) => text!(value), ColumnData::F64(value) => text!(value),
            ColumnData::Bit(value) => text!(value), ColumnData::Guid(value) => text!(value),
            ColumnData::Numeric(value) => text!(value), ColumnData::Xml(value) => text!(value),
            ColumnData::String(value) => match value { Some(value) if value.len() > budget => bail!("SQL Server 文本值超过单条记录上限"), Some(value) => Some(value.to_string()), None => None },
            ColumnData::Binary(value) => match value {
                Some(bytes) => {
                    let length = bytes.len().checked_mul(2).and_then(|length| length.checked_add(2)).ok_or_else(|| anyhow!("SQL Server 二进制长度溢出"))?;
                    if length > budget { bail!("SQL Server 二进制值超过单条记录上限"); }
                    let mut value = String::with_capacity(length); value.push_str("0x"); for byte in bytes.iter() { write!(&mut value, "{byte:02X}")?; } Some(value)
                }, None => None,
            },
            ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => <NaiveDateTime as FromSql>::from_sql(cell)?.map(datetime),
            ColumnData::Date(_) => <chrono::NaiveDate as FromSql>::from_sql(cell)?.map(|date| date.to_string()),
            ColumnData::Time(_) => <chrono::NaiveTime as FromSql>::from_sql(cell)?.map(|time| format!("{}.{:07}", time.format("%H:%M:%S"), time.nanosecond()/100)),
            // TDS 的 datetime2 部分为 UTC；由驱动恢复本地时间和原始偏移，保留七位小数。
            ColumnData::DateTimeOffset(_) => <chrono::DateTime<chrono::FixedOffset> as FromSql>::from_sql(cell)?
                .map(|value| format!("{}{}", datetime(value.naive_local()), value.format("%:z"))),
        };
        if value.as_ref().is_some_and(|value| value.len() > budget) { bail!("SQL Server 值超过单条记录上限"); }
        Ok(value)
    }
}

#[cfg(test)] mod tests {
    use super::*;
    use std::borrow::Cow;
    #[test] fn native_values_preserve_binary_rowversion_exact_int_and_real_null() {
        assert_eq!(super::super::MssqlDb::export_cell(&ColumnData::Binary(Some(Cow::Borrowed(&[0,255,128,39]))), 100).unwrap().as_deref(), Some("0x00FF8027"));
        assert_eq!(super::super::MssqlDb::export_cell(&ColumnData::I64(Some(i64::MAX)), 100).unwrap().as_deref(), Some("9223372036854775807"));
        assert_eq!(super::super::MssqlDb::export_cell(&ColumnData::String(None), 100).unwrap(), None);
        assert!(super::super::MssqlDb::export_cell(&ColumnData::Binary(Some(Cow::Borrowed(&[0;8]))), 17).is_err());
    }
    #[test] fn datetimeoffset_roundtrip_preserves_instant_offset_and_seven_fraction_digits() {
        use tiberius::ToSql;
        for (source,expected) in [("2026-11-11T12:00:00.1234567+08:00","2026-11-11 12:00:00.1234567+08:00"),("2026-01-01T00:05:00.0000001-07:30","2026-01-01 00:05:00.0000001-07:30")] {
            let value=chrono::DateTime::parse_from_rfc3339(source).unwrap();
            let ColumnData::DateTimeOffset(wire)=value.to_sql() else { panic!("datetimeoffset 必须使用真实 TDS 类型"); };
            let cell=ColumnData::DateTimeOffset(wire);
            assert_eq!(super::super::MssqlDb::export_cell(&cell,100).unwrap().as_deref(),Some(expected));
        }
    }
}
