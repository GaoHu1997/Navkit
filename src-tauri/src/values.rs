//! 统一的行值读取工具：把 sqlx 的行转成字符串向量。
//!
//! 之前版本只尝试 String / Vec<u8> / bool / i64 / f64 少数几种类型，
//! PostgreSQL 的 TIMESTAMP / DATE / NUMERIC / UUID / JSON、
//! MySQL 的 DECIMAL / DATETIME / BIGINT UNSIGNED 全部解码失败显示 NULL。
//! 这里按驱动各自的类型面逐级尝试（Option<T> 阶梯天然处理 NULL），
//! 覆盖常用类型。

use sqlx::postgres::PgRow;
use sqlx::{Column, Row, TypeInfo, ValueRef};

/// MySQL 的 GEOMETRY 内部格式不是标准 WKB：4 字节小端 SRID 后跟 MySQL 自有字节序，
/// 且坐标按 (y, x) 存放。直接把它交给 ST_GeomFromWKB 会得到坐标错位的几何。
/// 这里转换为标准 WKB（x, y）并单独保留 SRID。
pub(crate) fn format_mysql_geometry(bytes: &[u8]) -> anyhow::Result<String> {
    anyhow::ensure!(bytes.len() >= 9, "MySQL GEOMETRY 的 SRID/WKB 数据不完整");
    let srid = u32::from_le_bytes(bytes[..4].try_into()?);
    let wkb = mysql_internal_to_wkb(&bytes[4..])?;
    Ok(format!("SRID={srid};WKB={}", format_binary(&wkb)))
}

/// Only our explicit spatial representation is accepted as a geometry SQL literal.
pub(crate) fn mysql_geometry_sql(value: &str) -> anyhow::Result<String> {
    let (srid, wkb) = value.strip_prefix("SRID=").and_then(|value| value.split_once(";WKB=0x"))
        .ok_or_else(|| anyhow::anyhow!("GEOMETRY 缺少无损 SRID/WKB 标记"))?;
    anyhow::ensure!(!srid.is_empty() && srid.bytes().all(|byte| byte.is_ascii_digit()), "GEOMETRY SRID 无效");
    let srid: u32 = srid.parse()?;
    anyhow::ensure!(wkb.len() >= 10 && wkb.len() % 2 == 0 && wkb.bytes().all(|byte| byte.is_ascii_hexdigit()) && matches!(&wkb[..2], "00" | "01"), "GEOMETRY WKB 无效");
    let raw = wkb.as_bytes().chunks(2).map(|pair| {
        let text = std::str::from_utf8(pair).unwrap_or("00");
        u8::from_str_radix(text, 16).unwrap_or(0)
    }).collect::<Vec<u8>>();
    validate_wkb(&raw, 0)?;
    Ok(format!("ST_GeomFromWKB(X'{}', {srid})", wkb.to_ascii_uppercase()))
}

fn read_u32(bytes: &[u8], offset: usize, little: bool) -> anyhow::Result<u32> {
    let slice = bytes.get(offset..offset + 4).ok_or_else(|| anyhow::anyhow!("GEOMETRY 数据截断"))?;
    Ok(if little { u32::from_le_bytes(slice.try_into()?) } else { u32::from_be_bytes(slice.try_into()?) })
}
fn read_f64(bytes: &[u8], offset: usize, little: bool) -> anyhow::Result<f64> {
    let slice = bytes.get(offset..offset + 8).ok_or_else(|| anyhow::anyhow!("GEOMETRY 坐标数据截断"))?;
    Ok(if little { f64::from_le_bytes(slice.try_into()?) } else { f64::from_be_bytes(slice.try_into()?) })
}

/// 递归转换 MySQL 内部几何：输出标准 WKB。offset 返回消费后的位置。
fn mysql_geometry_body(bytes: &[u8], offset: usize, out: &mut Vec<u8>) -> anyhow::Result<usize> {
    let little = match bytes.get(offset) {
        Some(0) => false,
        Some(1) => true,
        Some(_) => return Err(anyhow::anyhow!("GEOMETRY 字节序标记无效")),
        None => return Err(anyhow::anyhow!("GEOMETRY 数据截断")),
    };
    // MySQL 在 SRID 后使用自身字节序；标准 WKB 保留原字节序标记，但坐标按 (x,y) 写出。
    out.push(if little { 1 } else { 0 });
    let kind = read_u32(bytes, offset + 1, little)?;
    let base = kind & 0xFF;
    let has_srid = kind & 0x2000_0000 != 0;
    let mut cursor = offset + 5;
    if has_srid { cursor += 4; }
    let wkb_type = base & 0xFF;
    out.extend_from_slice(&((if little { wkb_type.to_le_bytes() } else { wkb_type.to_be_bytes() }) as [u8; 4]));
    let mut write_f64 = |value: f64, out: &mut Vec<u8>| {
        out.extend_from_slice(&(if little { value.to_le_bytes() } else { value.to_be_bytes() }));
    };
    match base & 0xFF {
        1 | 2 => {
            let count = if base & 0xFF == 1 { 1 } else { read_u32(bytes, cursor, little)? as usize };
            if base & 0xFF == 2 {
                // 标准 WKB 的 LINESTRING 也必须带 points 计数，MySQL 内部格式同样有这一层。
                let count_bytes = if little { (count as u32).to_le_bytes() } else { (count as u32).to_be_bytes() };
                out.extend_from_slice(&count_bytes);
                cursor += 4;
            }
            for _ in 0..count {
                // MySQL 内部坐标顺序为 (y, x)，标准 WKB 为 (x, y)。
                let y = read_f64(bytes, cursor, little)?;
                let x = read_f64(bytes, cursor + 8, little)?;
                write_f64(x, out); write_f64(y, out);
                cursor += 16;
            }
        }
        3 => {
            let rings = read_u32(bytes, cursor, little)? as usize; cursor += 4;
            out.extend_from_slice(&(if little { (rings as u32).to_le_bytes() } else { (rings as u32).to_be_bytes() }));
            for _ in 0..rings {
                let points = read_u32(bytes, cursor, little)? as usize; cursor += 4;
                out.extend_from_slice(&(if little { (points as u32).to_le_bytes() } else { (points as u32).to_be_bytes() }));
                for _ in 0..points {
                    let y = read_f64(bytes, cursor, little)?;
                    let x = read_f64(bytes, cursor + 8, little)?;
                    write_f64(x, out); write_f64(y, out);
                    cursor += 16;
                }
            }
        }
        4..=7 => {
            let parts = read_u32(bytes, cursor, little)? as usize; cursor += 4;
            // 复合类型在标准 WKB 中同样以 parts 计数开头。
            let count_bytes = if little { (parts as u32).to_le_bytes() } else { (parts as u32).to_be_bytes() };
            out.extend_from_slice(&count_bytes);
            for _ in 0..parts {
                cursor = mysql_geometry_body(bytes, cursor, out)?;
            }
        }
        _ => return Err(anyhow::anyhow!("未知的 GEOMETRY 类型 {base}")),
    }
    Ok(cursor)
}

fn mysql_internal_to_wkb(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len());
    let consumed = mysql_geometry_body(bytes, 0, &mut out)?;
    anyhow::ensure!(consumed == bytes.len(), "GEOMETRY 数据存在未解析尾部");
    Ok(out)
}

/// 只验证标准 WKB 能完整解析，避免把损坏的几何写进导出。
fn validate_wkb(bytes: &[u8], offset: usize) -> anyhow::Result<usize> {
    let little = match bytes.get(offset) {
        Some(0) => false,
        Some(1) => true,
        _ => return Err(anyhow::anyhow!("WKB 字节序标记无效")),
    };
    let base = read_u32(bytes, offset + 1, little)? & 0xFF;
    let mut cursor = offset + 5;
    match base {
        1 | 2 => cursor += if base == 1 { 16 } else { 16 * read_u32(bytes, cursor, little)? as usize + 4 },
        3 => {
            let rings = read_u32(bytes, cursor, little)? as usize; cursor += 4;
            for _ in 0..rings { let points = read_u32(bytes, cursor, little)? as usize; cursor += 4 + 16 * points; }
        }
        4..=7 => {
            let parts = read_u32(bytes, cursor, little)? as usize; cursor += 4;
            for _ in 0..parts { cursor = validate_wkb(bytes, cursor)?; }
        }
        _ => return Err(anyhow::anyhow!("未知的 WKB 类型 {base}")),
    }
    anyhow::ensure!(cursor <= bytes.len(), "WKB 数据截断");
    Ok(cursor)
}

/// Avoid normalizing months to days or days to time: PostgreSQL stores all three
/// independently, and their signs may differ. i128 also handles i64::MIN exactly.
pub(crate) fn format_pg_interval(months: i32, days: i32, microseconds: i64) -> String {
    let micros = i128::from(microseconds);
    let magnitude = micros.abs();
    let seconds = magnitude / 1_000_000;
    let fraction = magnitude % 1_000_000;
    let sign = if micros < 0 { "-" } else { "" };
    let seconds = if fraction == 0 { format!("{sign}{seconds}") } else {
        format!("{sign}{seconds}.{:06}", fraction).trim_end_matches('0').to_owned()
    };
    format!("{months} mons {days} days {seconds} seconds")
}

fn pg_point_float(value: f64) -> String {
    if value == f64::INFINITY { "Infinity".into() }
    else if value == f64::NEG_INFINITY { "-Infinity".into() }
    else { value.to_string() }
}

/// Binary MONEY has no fractional-scale metadata. Resolve it on the same server
/// session, whose lc_monetary determines the money -> numeric cast. Call after a
/// buffered fetch, never while a result stream is still borrowing the connection.
pub async fn pg_resolve_money_values(connection: &mut sqlx::PgConnection, row: &PgRow, values: &mut [Option<String>]) -> anyhow::Result<()> {
    anyhow::ensure!(values.len() == row.len(), "PostgreSQL 行值与列数不一致");
    for (index, column) in row.columns().iter().enumerate() {
        if !column.type_info().name().eq_ignore_ascii_case("MONEY") { continue; }
        let raw = row.try_get_raw(index)?;
        if raw.is_null() { values[index] = None; continue; }
        let text: String = if raw.format() == sqlx::postgres::PgValueFormat::Binary {
            let money: sqlx::postgres::types::PgMoney = row.try_get(index)?;
            crate::sql_history::query_scalar("SELECT ($1::money)::numeric::text").bind(money).fetch_one(&mut *connection).await?
        } else {
            crate::sql_history::query_scalar("SELECT ($1::money)::numeric::text").bind(raw.as_str().map_err(|error| anyhow::anyhow!(error))?).fetch_one(&mut *connection).await?
        };
        values[index] = Some(text);
    }
    Ok(())
}

/// Binary is never implicitly decoded as UTF-8: even ASCII bytes retain their type.
pub fn format_binary(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut text = String::with_capacity(2 + bytes.len() * 2);
    text.push_str("0x");
    for byte in bytes {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 15) as usize] as char);
    }
    text
}

pub(crate) fn is_binary_type(name: &str) -> bool {
    matches!(name.to_ascii_uppercase().as_str(), "BYTEA" | "BINARY" | "VARBINARY" | "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB")
}

/// 浮点数格式化：整数则不带小数。
pub fn format_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{}", f)
    }
}

/// Unsupported non-NULL values stay visibly distinct from SQL NULL. Export
/// checks this marker and reports a typed unsupported-value failure instead.
fn unsupported_value(data_type: &str) -> String {
    format!("[unsupported non-NULL {data_type}: lossless decoding unavailable]")
}
pub(crate) fn is_unsupported_value(value: &str) -> bool {
    value.starts_with("[unsupported non-NULL ") && value.ends_with(": lossless decoding unavailable]")
}

/// PostgreSQL 行 -> 各列的字符串表示。
pub fn pg_row_values(row: &PgRow, count: usize) -> Vec<Option<String>> {
    (0..count)
        .map(|i| {
            let raw = row.try_get_raw(i).ok()?;
            if raw.is_null() { return None; }
            let info = raw.type_info();
            let ty = info.name();
            if matches!(ty, "INTERVAL" | "POINT" | "MONEY") {
                if raw.format() == sqlx::postgres::PgValueFormat::Text {
                    return Some(raw.as_str().map(str::to_owned).unwrap_or_else(|_| unsupported_value(ty)));
                }
                // MONEY scale is session/locale dependent; the asynchronous resolver
                // or the export's numeric::text projection must resolve it server-side.
                let decoded = raw.as_bytes().ok().and_then(|bytes| match ty {
                    "INTERVAL" if bytes.len() == 16 => Some(format_pg_interval(
                        i32::from_be_bytes(bytes[12..16].try_into().ok()?),
                        i32::from_be_bytes(bytes[8..12].try_into().ok()?),
                        i64::from_be_bytes(bytes[..8].try_into().ok()?),
                    )),
                    "POINT" if bytes.len() == 16 => Some(format!("({},{})",
                        pg_point_float(f64::from_be_bytes(bytes[..8].try_into().ok()?)),
                        pg_point_float(f64::from_be_bytes(bytes[8..].try_into().ok()?)),
                    )),
                    _ => None,
                });
                return Some(decoded.unwrap_or_else(|| unsupported_value(ty)));
            }
            // Some drivers also allow String decoding for ASCII BLOB values.
            // Inspect the runtime type first so binary identity is never lost.
            if row.try_get_raw(i).map(|v| is_binary_type(v.type_info().name())).unwrap_or(false) {
                return row.try_get::<Option<Vec<u8>>, _>(i).ok().flatten().map(|x| format_binary(&x));
            }
            // 与 dbx 一致：按列的静态类型“分类一次、单次解码”，避免对每个单元格
            // 逐级 try_get 二三十次（每格失败解码一次即触发一次错误构造）。
            // 只对已知类型快路径；其它类型仍走下方安全的逐级阶梯，输出保持等价。
            match ty {
                "BOOL" => return row.try_get::<Option<bool>, _>(i).ok().flatten().map(|b| if b { "1".into() } else { "0".into() }),
                "INT2" => return row.try_get::<Option<i16>, _>(i).ok().flatten().map(|x| x.to_string()),
                "INT4" => return row.try_get::<Option<i32>, _>(i).ok().flatten().map(|x| x.to_string()),
                "INT8" => return row.try_get::<Option<i64>, _>(i).ok().flatten().map(|x| x.to_string()),
                "FLOAT4" => return row.try_get::<Option<f32>, _>(i).ok().flatten().map(|x| format_float(x as f64)),
                "FLOAT8" => return row.try_get::<Option<f64>, _>(i).ok().flatten().map(format_float),
                "NUMERIC" => return row.try_get::<Option<sqlx::types::BigDecimal>, _>(i).ok().flatten().map(|x| x.to_string()),
                "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" => return row.try_get::<Option<String>, _>(i).ok().flatten(),
                "TIMESTAMP" => return row.try_get::<Option<chrono::NaiveDateTime>, _>(i).ok().flatten().map(|x| x.to_string()),
                "TIMESTAMPTZ" => return row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(i).ok().flatten().map(|x| x.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)),
                "DATE" => return row.try_get::<Option<chrono::NaiveDate>, _>(i).ok().flatten().map(|x| x.to_string()),
                "TIME" => return row.try_get::<Option<chrono::NaiveTime>, _>(i).ok().flatten().map(|x| x.to_string()),
                "UUID" => return row.try_get::<Option<uuid::Uuid>, _>(i).ok().flatten().map(|x| x.to_string()),
                "JSON" | "JSONB" => return row.try_get::<Option<sqlx::types::Json<Box<serde_json::value::RawValue>>>, _>(i).ok().flatten().map(|x| x.0.get().to_owned()),
                _ => {}
            }
            if let Ok(v) = row.try_get::<Option<String>, _>(i) {
                return v;
            }
            if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
                return v.map(|b| if b { "1".into() } else { "0".into() });
            }
            if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<i32>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<i16>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
                return v.map(format_float);
            }
            if let Ok(v) = row.try_get::<Option<f32>, _>(i) {
                return v.map(|x| format_float(x as f64));
            }
            // NUMERIC / DECIMAL
            if let Ok(v) = row.try_get::<Option<sqlx::types::BigDecimal>, _>(i) {
                return v.map(|x| x.to_string());
            }
            // TIMESTAMP / TIMESTAMPTZ / DATE / TIME
            if let Ok(v) = row.try_get::<Option<chrono::NaiveDateTime>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(i) {
                return v.map(|x| x.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true));
            }
            if let Ok(v) = row.try_get::<Option<chrono::NaiveDate>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<chrono::NaiveTime>, _>(i) {
                return v.map(|x| x.to_string());
            }
            // UUID
            if let Ok(v) = row.try_get::<Option<uuid::Uuid>, _>(i) {
                return v.map(|x| x.to_string());
            }
            // JSON/JSONB numeric lexemes must not pass through serde_json::Value/f64.
            if let Ok(v) = row.try_get::<Option<sqlx::types::Json<Box<serde_json::value::RawValue>>>, _>(i) {
                return v.map(|x| x.0.get().to_owned());
            }
            // BYTEA：非 UTF-8 的二进制以 0x 十六进制呈现，防止乱码被原样回写
            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| format_binary(&x));
            }
            // 数组类型：text[] / int8[] / float8[]
            if let Ok(v) = row.try_get::<Option<Vec<String>>, _>(i) {
                return v.map(|x| format!("{{{}}}", x.join(",")));
            }
            if let Ok(v) = row.try_get::<Option<Vec<i64>>, _>(i) {
                return v.map(|x| {
                    format!(
                        "{{{}}}",
                        x.iter().map(|y| y.to_string()).collect::<Vec<_>>().join(",")
                    )
                });
            }
            if let Ok(v) = row.try_get::<Option<Vec<f64>>, _>(i) {
                return v.map(|x| {
                    format!(
                        "{{{}}}",
                        x.iter().map(|y| format_float(*y)).collect::<Vec<_>>().join(",")
                    )
                });
            }
            // int4[] / int2[] / float4[] / bool[] / numeric[] / timestamp[]
            if let Ok(v) = row.try_get::<Option<Vec<i32>>, _>(i) {
                return v.map(|x| {
                    format!(
                        "{{{}}}",
                        x.iter().map(|y| y.to_string()).collect::<Vec<_>>().join(",")
                    )
                });
            }
            if let Ok(v) = row.try_get::<Option<Vec<f32>>, _>(i) {
                return v.map(|x| {
                    format!(
                        "{{{}}}",
                        x.iter().map(|y| format_float(*y as f64)).collect::<Vec<_>>().join(",")
                    )
                });
            }
            if let Ok(v) = row.try_get::<Option<Vec<bool>>, _>(i) {
                return v.map(|x| {
                    format!(
                        "{{{}}}",
                        x.iter().map(|y| if *y { "t" } else { "f" }).collect::<Vec<_>>().join(",")
                    )
                });
            }
            if let Ok(v) = row.try_get::<Option<Vec<sqlx::types::BigDecimal>>, _>(i) {
                return v.map(|x| {
                    format!(
                        "{{{}}}",
                        x.iter().map(|y| y.to_string()).collect::<Vec<_>>().join(",")
                    )
                });
            }
            if let Ok(v) = row.try_get::<Option<Vec<chrono::NaiveDateTime>>, _>(i) {
                return v.map(|x| {
                    format!(
                        "{{{}}}",
                        x.iter().map(|y| y.to_string()).collect::<Vec<_>>().join(",")
                    )
                });
            }
            Some(unsupported_value(ty))
        })
        .collect()
}

/// MySQL 行 -> 各列的字符串表示。
pub fn my_row_values(row: &sqlx::mysql::MySqlRow, count: usize) -> Vec<Option<String>> {
    (0..count)
        .map(|i| {
            let raw = row.try_get_raw(i).ok()?;
            if raw.is_null() { return None; }
            let info = raw.type_info();
            let ty = info.name();
            if ty.eq_ignore_ascii_case("GEOMETRY") {
                // Vec<u8>'s Type compatibility excludes GEOMETRY, although its
                // Decode implementation can read the bytes without conversion.
                return Some(row.try_get_unchecked::<Vec<u8>, _>(i).ok()
                    .and_then(|bytes| format_mysql_geometry(&bytes).ok())
                    .unwrap_or_else(|| unsupported_value(ty)));
            }
            // Some drivers also allow String decoding for ASCII BLOB values.
            // Inspect the runtime type first so binary identity is never lost.
            if row.try_get_raw(i).map(|v| is_binary_type(v.type_info().name())).unwrap_or(false) {
                return row.try_get::<Option<Vec<u8>>, _>(i).ok().flatten().map(|x| format_binary(&x));
            }
            // 按列的静态类型一次性分派（等价 dbx 的分类解码），未知类型回退到下方阶梯。
            match ty {
                "TINYINT" | "SMALLINT" | "INT" | "MEDIUMINT" | "BIGINT" => return row.try_get::<Option<i64>, _>(i).ok().flatten().map(|x| x.to_string()),
                // YEAR is a distinct unsigned two-byte MySQL protocol type. SQLx's
                // signed integer compatibility intentionally excludes it, so do not
                // return early on the failed i64 decode and lose the generic u64 path.
                "YEAR" => return row.try_get::<Option<u64>, _>(i).ok().flatten().map(|x| x.to_string()),
                "FLOAT" => return row.try_get::<Option<f32>, _>(i).ok().flatten().map(|x| format_float(x as f64)),
                "DOUBLE" => return row.try_get::<Option<f64>, _>(i).ok().flatten().map(format_float),
                "DECIMAL" => return row.try_get::<Option<sqlx::types::BigDecimal>, _>(i).ok().flatten().map(|x| x.to_string()),
                "VARCHAR" | "CHAR" | "TINYTEXT" | "TEXT" | "MEDIUMTEXT" | "LONGTEXT" => return row.try_get::<Option<String>, _>(i).ok().flatten(),
                "DATETIME" => return row.try_get::<Option<chrono::NaiveDateTime>, _>(i).ok().flatten().map(|x| x.to_string()),
                "DATE" => return row.try_get::<Option<chrono::NaiveDate>, _>(i).ok().flatten().map(|x| x.to_string()),
                "TIME" => return row.try_get::<Option<chrono::NaiveTime>, _>(i).ok().flatten().map(|x| x.to_string()),
                _ => {}
            }
            if let Ok(v) = row.try_get::<Option<String>, _>(i) {
                return v;
            }
            if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
                return v.map(|x| x.to_string());
            }
            // BIGINT UNSIGNED（自增主键、COUNT 等）
            if let Ok(v) = row.try_get::<Option<u64>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<i32>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<i16>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
                return v.map(format_float);
            }
            if let Ok(v) = row.try_get::<Option<f32>, _>(i) {
                return v.map(|x| format_float(x as f64));
            }
            // DECIMAL / NUMERIC
            if let Ok(v) = row.try_get::<Option<sqlx::types::BigDecimal>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
                return v.map(|b| if b { "1".into() } else { "0".into() });
            }
            // DATETIME / DATE / TIME / TIMESTAMP
            if let Ok(v) = row.try_get::<Option<chrono::NaiveDateTime>, _>(i) {
                return v.map(|x| x.to_string());
            }
            // MySQL TIMESTAMP 只能解码为 DateTime<Utc>/<Local>；二进制传输是 UTC 时刻，按本地会话时区还原字面值。
            if let Ok(v) = row.try_get::<Option<chrono::DateTime<chrono::Local>>, _>(i) {
                return v.map(|x| x.naive_local().to_string());
            }
            if let Ok(v) = row.try_get::<Option<chrono::NaiveDate>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<chrono::NaiveTime>, _>(i) {
                return v.map(|x| x.to_string());
            }
            // JSON keeps the server's numeric lexemes without JavaScript/f64 conversion.
            if let Ok(v) = row.try_get::<Option<sqlx::types::Json<Box<serde_json::value::RawValue>>>, _>(i) {
                return v.map(|x| x.0.get().to_owned());
            }
            // BLOB / BINARY：以 0x 十六进制保留二进制类型
            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| format_binary(&x));
            }
            Some(unsupported_value(ty))
        })
        .collect()
}

/// SQLite 行 -> 各列的字符串表示。
///
/// SQLite 的值类型由**运行时存储类**决定（`sqlite3_value_type`：NULL/INTEGER/REAL/TEXT/BLOB），
/// 因此按每格的存储类一次性分派，避免对每个单元格逐级 `try_get`。
pub fn sqlite_row_values(row: &sqlx::sqlite::SqliteRow, count: usize) -> Vec<Option<String>> {
    (0..count)
        .map(|i| {
            let raw = row.try_get_raw(i).ok()?;
            if raw.is_null() { return None; }
            let info = raw.type_info();
            match info.name() {
                "TEXT" => row.try_get::<Option<String>, _>(i).ok().flatten(),
                "INTEGER" => row.try_get::<Option<i64>, _>(i).ok().flatten().map(|x| x.to_string()),
                "REAL" => row.try_get::<Option<f64>, _>(i).ok().flatten().map(format_float),
                "BLOB" => row.try_get::<Option<Vec<u8>>, _>(i).ok().flatten().map(|x| format_binary(&x)),
                name => {
                    // 兜底：非常规声明类型（BOOLEAN/DATE/DATETIME/…）仍走原有安全阶梯。
                    if is_binary_type(name) {
                        return row.try_get::<Option<Vec<u8>>, _>(i).ok().flatten().map(|x| format_binary(&x));
                    }
                    if let Ok(v) = row.try_get::<Option<String>, _>(i) { return v; }
                    if let Ok(v) = row.try_get::<Option<i64>, _>(i) { return v.map(|x| x.to_string()); }
                    if let Ok(v) = row.try_get::<Option<f64>, _>(i) { return v.map(format_float); }
                    if let Ok(v) = row.try_get::<Option<bool>, _>(i) { return v.map(|b| if b { "1".into() } else { "0".into() }); }
                    if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) { return v.map(|x| format_binary(&x)); }
                    None
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod spatial_and_interval_tests {
    use super::*;

    fn geometry_bytes(srid: u32, wkb: &[u8]) -> Vec<u8> {
        let mut bytes = srid.to_le_bytes().to_vec();
        bytes.extend_from_slice(wkb);
        bytes
    }

    #[test]
    fn mysql_geometry_keeps_srid_and_wkb_bytes_and_roundtrips_to_sql() {
        // POINT(1 2), little-endian WKB: byte order 1, type 1, x=1.0, y=2.0.
        // MySQL 内部为 (y, x)；输入 y=2, x=1，输出标准 WKB 必须为 (x=1, y=2)。
        let internal = [0x01, 0x01, 0x00, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x00, 0x40, 0, 0, 0, 0, 0, 0, 0xF0, 0x3F];
        let expected = [0x01, 0x01, 0x00, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0xF0, 0x3F, 0, 0, 0, 0, 0, 0, 0x00, 0x40];
        let text = format_mysql_geometry(&geometry_bytes(4326, &internal)).unwrap();
        assert_eq!(text, format!("SRID=4326;WKB=0x{}", expected.iter().map(|b| format!("{b:02X}")).collect::<String>()));
        let sql = mysql_geometry_sql(&text).unwrap();
        assert!(sql.starts_with("ST_GeomFromWKB(X'0101000000"), "{sql}");
        assert!(sql.ends_with(", 4326)"), "{sql}");
        assert!(mysql_geometry_sql("SRID=4326;WKB=0x").is_err());
        assert!(mysql_geometry_sql("POINT(1 2)").is_err());
        assert!(format_mysql_geometry(&[0, 0, 0, 0, 2]).is_err());
    }

    #[test]
    fn mysql_compound_geometries_emit_standard_wkb_counts() {
        // 真实 MySQL 8.0.46 的 MULTIPOINT / MULTILINESTRING / GEOMETRYCOLLECTION 内部字节。
        let multipoint = [0x01,0x04,0x00,0x00,0x00,0x02,0x00,0x00,0x00,
            0x01,0x01,0x00,0x00,0x00,0,0,0,0,0,0,0x00,0x40,0,0,0,0,0,0,0xF0,0x3F,
            0x01,0x01,0x00,0x00,0x00,0,0,0,0,0,0,0x10,0x40,0,0,0,0,0,0,0x08,0x40];
        let text = format_mysql_geometry(&geometry_bytes(4326, &multipoint)).unwrap();
        assert!(text.contains("WKB=0x010400000002000000"), "标准 WKB 必须带 parts 计数: {text}");
        let collection = [0x01,0x07,0x00,0x00,0x00,0x02,0x00,0x00,0x00,
            0x01,0x01,0x00,0x00,0x00,0,0,0,0,0,0,0x00,0x40,0,0,0,0,0,0,0xF0,0x3F,
            0x01,0x02,0x00,0x00,0x00,0x02,0x00,0x00,0x00,0,0,0,0,0,0,0x10,0x40,0,0,0,0,0,0,0x08,0x40,0,0,0,0,0,0,0x18,0x40,0,0,0,0,0,0,0x14,0x40];
        let text = format_mysql_geometry(&geometry_bytes(4326, &collection)).unwrap();
        assert!(text.contains("WKB=0x010700000002000000"), "集合必须带 parts 计数: {text}");
    }

    #[test]
    fn pg_interval_preserves_months_days_and_microseconds() {
        assert_eq!(format_pg_interval(14, 3, 1_500_000), "14 mons 3 days 1.5 seconds");
        assert_eq!(format_pg_interval(-14, -3, -1_500_000), "-14 mons -3 days -1.5 seconds");
        assert_eq!(format_pg_interval(0, 0, 0), "0 mons 0 days 0 seconds");
        assert_eq!(format_pg_interval(0, 0, i64::MIN), "0 mons 0 days -9223372036854.775808 seconds");
    }

    #[test]
    fn unsupported_marker_is_detectable_and_not_confused_with_data() {
        let marker = unsupported_value("GEOMETRY");
        assert!(is_unsupported_value(&marker));
        assert!(!is_unsupported_value("NULL"));
        assert!(!is_unsupported_value("SRID=0;WKB=0x00"));
    }
}
