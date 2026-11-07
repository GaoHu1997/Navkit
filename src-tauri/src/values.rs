//! 统一的行值读取工具：把 sqlx 的行转成字符串向量。
//!
//! 之前版本只尝试 String / Vec<u8> / bool / i64 / f64 少数几种类型，
//! PostgreSQL 的 TIMESTAMP / DATE / NUMERIC / UUID / JSON、
//! MySQL 的 DECIMAL / DATETIME / BIGINT UNSIGNED 全部解码失败显示 NULL。
//! 这里按驱动各自的类型面逐级尝试（Option<T> 阶梯天然处理 NULL），
//! 覆盖常用类型。

use sqlx::postgres::PgRow;
use sqlx::Row;

/// 浮点数格式化：整数则不带小数。
pub fn format_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{}", f)
    }
}

/// PostgreSQL 行 -> 各列的字符串表示。
pub fn pg_row_values(row: &PgRow, count: usize) -> Vec<Option<String>> {
    (0..count)
        .map(|i| {
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
                return v.map(|x| x.naive_utc().to_string());
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
            // JSON / JSONB
            if let Ok(v) = row.try_get::<Option<serde_json::Value>, _>(i) {
                return v.map(|x| x.to_string());
            }
            // BYTEA：非 UTF-8 的二进制以 0x 十六进制呈现，防止乱码被原样回写
            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| match std::str::from_utf8(&x) {
                    Ok(t) => t.to_string(),
                    Err(_) => format!("0x{}", x.iter().map(|b| format!("{:02X}", b)).collect::<String>()),
                });
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
            None
        })
        .collect()
}

/// MySQL 行 -> 各列的字符串表示。
pub fn my_row_values(row: &sqlx::mysql::MySqlRow, count: usize) -> Vec<Option<String>> {
    (0..count)
        .map(|i| {
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
            if let Ok(v) = row.try_get::<Option<chrono::NaiveDate>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<chrono::NaiveTime>, _>(i) {
                return v.map(|x| x.to_string());
            }
            // BLOB / BINARY：非 UTF-8 的二进制以 0x 十六进制呈现
            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| match std::str::from_utf8(&x) {
                    Ok(t) => t.to_string(),
                    Err(_) => format!("0x{}", x.iter().map(|b| format!("{:02X}", b)).collect::<String>()),
                });
            }
            None
        })
        .collect()
}

/// SQLite 行 -> 各列的字符串表示。
pub fn sqlite_row_values(row: &sqlx::sqlite::SqliteRow, count: usize) -> Vec<Option<String>> {
    (0..count)
        .map(|i| {
            if let Ok(v) = row.try_get::<Option<String>, _>(i) {
                return v;
            }
            if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
                return v.map(format_float);
            }
            if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
                return v.map(|b| if b { "1".into() } else { "0".into() });
            }
            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| match std::str::from_utf8(&x) {
                    Ok(t) => t.to_string(),
                    Err(_) => format!("0x{}", x.iter().map(|b| format!("{:02X}", b)).collect::<String>()),
                });
            }
            None
        })
        .collect()
}
