# -*- coding: utf-8 -*-
"""一次性修复脚本：values.rs 类型解码阶梯 + db.rs describe/连接参数修正。"""
import io

# ---------------- values.rs ----------------
p = "src/values.rs"
s = io.open(p, encoding="utf-8").read()

# PG 分支：去掉 try_get_raw 短路，改为 Option<T> 阶梯
pg_old_head = """        .map(|i| {
            // NULL 直接短路，避免后续按具体类型解码报错。
            if let Ok(is_null) = row.try_get_raw(i) {
                if is_null.is_null() {
                    return None;
                }
            }
            if let Ok(v) = row.try_get::<String, _>(i) {
                return Some(v);
            }"""
pg_new_head = """        .map(|i| {
            if let Ok(v) = row.try_get::<Option<String>, _>(i) {
                return v;
            }"""
assert pg_old_head in s, "pg head"
s = s.replace(pg_old_head, pg_new_head)

# 通用：把 Some/None 阶梯转成 Option 阶梯（PG 主体）
pg_body_old = """            if let Ok(v) = row.try_get::<bool, _>(i) {
                return Some(if v { "1".into() } else { "0".into() });
            }
            if let Ok(v) = row.try_get::<i64, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<i32, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<i16, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<f64, _>(i) {
                return Some(format_float(v));
            }
            if let Ok(v) = row.try_get::<f32, _>(i) {
                return Some(format_float(v as f64));
            }
            if let Ok(v) = row.try_get::<bigdecimal::BigDecimal, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<chrono::NaiveDateTime, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<chrono::DateTime<chrono::Utc>, _>(i) {
                return Some(v.naive_utc().to_string());
            }
            if let Ok(v) = row.try_get::<chrono::NaiveDate, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<chrono::NaiveTime, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<uuid::Uuid, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<serde_json::Value, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<Vec<u8>, _>(i) {
                return Some(String::from_utf8_lossy(&v).to_string());
            }
            // 数组类型：text[] / int8[] / float8[]
            if let Ok(v) = row.try_get::<Vec<String>, _>(i) {
                return Some(format!("{{{}}}", v.join(",")));
            }
            if let Ok(v) = row.try_get::<Vec<i64>, _>(i) {
                return Some(format!("{{{}}}", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")));
            }
            if let Ok(v) = row.try_get::<Vec<f64>, _>(i) {
                return Some(format!("{{{}}}", v.iter().map(|x| format_float(*x)).collect::<Vec<_>>().join(",")));
            }
            None
        })
        .collect()
}

/// MySQL 行 -> 各列的字符串表示。"""
pg_body_new = """            if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
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
            if let Ok(v) = row.try_get::<Option<sqlx::types::BigDecimal>, _>(i) {
                return v.map(|x| x.to_string());
            }
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
            if let Ok(v) = row.try_get::<Option<uuid::Uuid>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<serde_json::Value>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| String::from_utf8_lossy(&x).to_string());
            }
            // 数组类型：text[] / int8[] / float8[]
            if let Ok(v) = row.try_get::<Option<Vec<String>>, _>(i) {
                return v.map(|x| format!("{{{}}}", x.join(",")));
            }
            if let Ok(v) = row.try_get::<Option<Vec<i64>>, _>(i) {
                return v.map(|x| format!("{{{}}}", x.iter().map(|y| y.to_string()).collect::<Vec<_>>().join(",")));
            }
            if let Ok(v) = row.try_get::<Option<Vec<f64>>, _>(i) {
                return v.map(|x| format!("{{{}}}", x.iter().map(|y| format_float(*y)).collect::<Vec<_>>().join(",")));
            }
            None
        })
        .collect()
}

/// MySQL 行 -> 各列的字符串表示。"""
assert pg_body_old in s, "pg body"
s = s.replace(pg_body_old, pg_body_new)

# MySQL 分支
my_old = """        .map(|i| {
            if let Ok(is_null) = row.try_get_raw(i) {
                if is_null.is_null() {
                    return None;
                }
            }
            if let Ok(v) = row.try_get::<String, _>(i) {
                return Some(v);
            }
            if let Ok(v) = row.try_get::<i64, _>(i) {
                return Some(v.to_string());
            }
            // BIGINT UNSIGNED（自增主键、COUNT 等）
            if let Ok(v) = row.try_get::<u64, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<i32, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<i16, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<f64, _>(i) {
                return Some(format_float(v));
            }
            if let Ok(v) = row.try_get::<f32, _>(i) {
                return Some(format_float(v as f64));
            }
            // DECIMAL / NUMERIC
            if let Ok(v) = row.try_get::<bigdecimal::BigDecimal, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<bool, _>(i) {
                return Some(if v { "1".into() } else { "0".into() });
            }
            if let Ok(v) = row.try_get::<chrono::NaiveDateTime, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<chrono::NaiveDate, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<chrono::NaiveTime, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<Vec<u8>, _>(i) {
                return Some(String::from_utf8_lossy(&v).to_string());
            }
            None
        })
        .collect()
}"""
my_new = """        .map(|i| {
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
            if let Ok(v) = row.try_get::<Option<chrono::NaiveDateTime>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<chrono::NaiveDate>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<chrono::NaiveTime>, _>(i) {
                return v.map(|x| x.to_string());
            }
            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| String::from_utf8_lossy(&x).to_string());
            }
            None
        })
        .collect()
}"""
assert my_old in s, "my body"
s = s.replace(my_old, my_new)

# SQLite 分支
sl_old = """        .map(|i| {
            if let Ok(is_null) = row.try_get_raw(i) {
                if is_null.is_null() {
                    return None;
                }
            }
            if let Ok(v) = row.try_get::<String, _>(i) {
                return Some(v);
            }
            if let Ok(v) = row.try_get::<i64, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<f64, _>(i) {
                return Some(format_float(v));
            }
            if let Ok(v) = row.try_get::<bool, _>(i) {
                return Some(if v { "1".into() } else { "0".into() });
            }
            if let Ok(v) = row.try_get::<Vec<u8>, _>(i) {
                return Some(String::from_utf8_lossy(&v).to_string());
            }
            None
        })
        .collect()
}"""
sl_new = """        .map(|i| {
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
                return v.map(|x| String::from_utf8_lossy(&x).to_string());
            }
            None
        })
        .collect()
}"""
assert sl_old in s, "sqlite body"
s = s.replace(sl_old, sl_new)

io.open(p, "w", encoding="utf-8", newline="\n").write(s)
print("values.rs patched")

# ---------------- db.rs ----------------
p = "src/db.rs"
s = io.open(p, encoding="utf-8").read()

old1 = "match sqlx::query(sql).describe(pool).await {"
new1 = "match pool.describe(sql).await {"
assert old1 in s, "sqlite describe"
s = s.replace(old1, new1)

old2 = """            _ => {
                opts = opts.socket(k.as_str(), v.as_str());
            }"""
new2 = """            _ => {
                // 其它连接参数忽略（不支持 unix socket 等高级选项）。
            }"""
assert old2 in s, "socket"
s = s.replace(old2, new2)

old3 = """    if !extra_options.is_empty() {
        opts = opts.options(&extra_options);
    }"""
new3 = """    for (k, v) in &extra_options {
        opts = opts.options([(k.as_str(), v.as_str())]);
    }"""
assert old3 in s, "pg options"
s = s.replace(old3, new3)

io.open(p, "w", encoding="utf-8", newline="\n").write(s)
print("db.rs patched")
