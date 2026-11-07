# -*- coding: utf-8 -*-
"""验收 #1/#2 整改：db.rs 注释跳过 + is_query 词边界 + 值解码补强 + 池缓存失效。"""
import io

# ============ db.rs ============
p = "src/db.rs"
s = io.open(p, encoding="utf-8").read()

# 1) 共享助手：剥离前导注释行
helper = """
/// 剥离语句块开头的前导独立注释行（`--` 整行 / `/* ... */` 独立块），
/// 返回剩余 SQL；纯注释返回空串。不碰行内注释与字符串字面量。
pub fn strip_leading_comments(stmt: &str) -> String {
    let mut s = stmt.trim_start();
    loop {
        if s.starts_with("--") {
            s = s.split_once('\\n').map(|(_, r)| r.trim_start()).unwrap_or("");
            continue;
        }
        if s.starts_with("/*") {
            match s.find("*/") {
                Some(i) => s = s[i + 2..].trim_start(),
                None => return String::new(),
            }
            continue;
        }
        break;
    }
    s.trim().to_string()
}

/// 判断一段语句剥离注释后是否还有可执行内容。
pub fn is_executable(stmt: &str) -> bool {
    !strip_leading_comments(stmt).is_empty()
}
"""
anchor = "/// 判断 SQL 是否返回数据。"
assert anchor in s, "anchor missing"
s = s.replace(anchor, helper + "\n" + anchor, 1)

# 2) is_query：取首词比对（紧凑写法 SELECT*FROM 也能识别）
old_iq = """    let up = s.to_ascii_uppercase();
    for kw in [
        "SELECT", "SHOW", "DESCRIBE", "DESC", "EXPLAIN", "PRAGMA", "WITH", "VALUES",
        "TABLE", "CALL",
    ] {
        // 关键字必须后接边界（空白/括号/结尾），避免把 `SELECTED` 当 SELECT。
        if let Some(rest) = up.strip_prefix(kw) {
            if rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == '(') {
                return true;
            }
        }
    }
    false
}"""
new_iq = """    // 取首词（连续 ASCII 字母）比对关键字，兼容 SELECT*FROM 等紧凑写法。
    let word: String = s
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    matches!(
        word.as_str(),
        "SELECT" | "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN" | "PRAGMA" | "WITH" | "VALUES"
            | "TABLE" | "CALL"
    )
}"""
assert old_iq in s, "is_query body"
s = s.replace(old_iq, new_iq)

# 3) 三分支统一 is_executable（替换粗糙版 + 原始空判断）
old3 = """                let trimmed = stmt.trim();
                if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with("/*") {
                    continue; // 纯注释块不执行（否则 MySQL 报 ER_EMPTY_QUERY）
                }"""
new3 = """                let trimmed = stmt.trim();
                if !is_executable(trimmed) {
                    continue; // 纯注释块不执行（否则 MySQL 报 ER_EMPTY_QUERY）
                }"""
n1 = s.count(old3)
s = s.replace(old3, new3)
old4 = """                let trimmed = stmt.trim();
                if trimmed.is_empty() {
                    continue;
                }"""
new4 = """                let trimmed = stmt.trim();
                if !is_executable(trimmed) {
                    continue; // 纯注释块不执行（否则 MySQL 报 ER_EMPTY_QUERY）
                }"""
n2 = s.count(old4)
s = s.replace(old4, new4)
io.open(p, "w", encoding="utf-8", newline="\n").write(s)
print(f"db.rs: is_query ok, branch patches sqlite={n1} others={n2}")

# ============ transfer.rs 复用共享助手 ============
p = "src/transfer.rs"
s = io.open(p, encoding="utf-8").read()
old5 = """/// 剥离语句块开头的独立注释行（`--` 至行尾），返回剩余 SQL。
/// 全是注释的块返回空串。只剥离"整行以 -- 开头"的行，不碰行内注释与字符串字面量。
fn strip_leading_comment_lines(stmt: &str) -> String {
    let mut out = String::new();
    for line in stmt.lines() {
        if out.is_empty() && line.trim_start().starts_with("--") {
            continue; // 仅丢弃块开头的前导注释行
        }
        out.push_str(line);
        out.push('\\n');
    }
    out.trim().to_string()
}

"""
if old5 in s:
    s = s.replace(old5, "")
old6 = ".map(|s| strip_leading_comment_lines(&s))"
assert old6 in s, "transfer call site"
s = s.replace(old6, ".map(|s| crate::db::strip_leading_comments(&s))")
io.open(p, "w", encoding="utf-8", newline="\n").write(s)
print("transfer.rs: reuse strip_leading_comments ok")

# ============ schema.rs：MySQL 无符号统计 u64 + PG 值阶梯一致 ============
p = "src/schema.rs"
s = io.open(p, encoding="utf-8").read()
old7 = """                let ai = r.try_get::<Option<i64>, _>("AUTO_INCREMENT").unwrap_or(None).unwrap_or(0);
                let dl = r.try_get::<Option<i64>, _>("DATA_LENGTH").unwrap_or(None).unwrap_or(0);
                let tr = r.try_get::<Option<i64>, _>("TABLE_ROWS").unwrap_or(None).unwrap_or(0);"""
new7 = """                // information_schema 统计列均为 bigint unsigned，i64 解码必失败
                let ai = r.try_get::<Option<u64>, _>("AUTO_INCREMENT").unwrap_or(None).unwrap_or(0) as i64;
                let dl = r.try_get::<Option<u64>, _>("DATA_LENGTH").unwrap_or(None).unwrap_or(0) as i64;
                let tr = r.try_get::<Option<u64>, _>("TABLE_ROWS").unwrap_or(None).unwrap_or(0) as i64;"""
assert old7 in s, "list_objects stats"
s = s.replace(old7, new7)
io.open(p, "w", encoding="utf-8", newline="\n").write(s)
print("schema.rs: unsigned stats ok")

# ============ values.rs：补 PG 数组类型（int4/int2/float4/bool/numeric/timestamp） ============
p = "src/values.rs"
s = io.open(p, encoding="utf-8").read()
old8 = """            if let Ok(v) = row.try_get::<Option<Vec<f64>>, _>(i) {
                return v.map(|x| {
                    format!(
                        "{{{}}}",
                        x.iter().map(|y| format_float(*y)).collect::<Vec<_>>().join(",")
                    )
                });
            }
            None
        })
        .collect()
}"""
new8 = """            if let Ok(v) = row.try_get::<Option<Vec<f64>>, _>(i) {
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
}"""
assert old8 in s, "pg arrays"
s = s.replace(old8, new8)

# BLOB：非 UTF-8 二进制渲染为 0x 十六进制，避免乱码被回写
old9 = """            // BYTEA
            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| String::from_utf8_lossy(&x).to_string());
            }"""
new9 = """            // BYTEA：非 UTF-8 的二进制以 0x 十六进制呈现，防止乱码被原样回写
            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| match std::str::from_utf8(&x) {
                    Ok(t) => t.to_string(),
                    Err(_) => format!("0x{}", x.iter().map(|b| format!("{:02X}", b)).collect::<String>()),
                });
            }"""
assert old9 in s, "pg blob"
s = s.replace(old9, new9)

old10 = """            // BLOB / BINARY
            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| String::from_utf8_lossy(&x).to_string());
            }"""
new10 = """            // BLOB / BINARY：非 UTF-8 的二进制以 0x 十六进制呈现
            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| match std::str::from_utf8(&x) {
                    Ok(t) => t.to_string(),
                    Err(_) => format!("0x{}", x.iter().map(|b| format!("{:02X}", b)).collect::<String>()),
                });
            }"""
assert old10 in s, "my blob"
s = s.replace(old10, new10)

old11 = """            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| String::from_utf8_lossy(&x).to_string());
            }
            None
        })
        .collect()
}"""
new11 = """            if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                return v.map(|x| match std::str::from_utf8(&x) {
                    Ok(t) => t.to_string(),
                    Err(_) => format!("0x{}", x.iter().map(|b| format!("{:02X}", b)).collect::<String>()),
                });
            }
            None
        })
        .collect()
}"""
assert old11 in s, "sqlite blob"
s = s.replace(old11, new11, 1)
io.open(p, "w", encoding="utf-8", newline="\n").write(s)
print("values.rs: arrays + blob hex ok")
