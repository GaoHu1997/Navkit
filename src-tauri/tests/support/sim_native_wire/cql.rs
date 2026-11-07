//! Cassandra native v4: SASL PLAIN, topology/schema discovery and mutable CQL records.
use super::*;
use anyhow::{anyhow, bail, ensure, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type Cell = Option<Vec<u8>>;
fn short(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend((value.len() as u16).to_be_bytes());
    bytes.extend(value.as_bytes());
}
fn text(value: &str) -> Cell { Some(value.as_bytes().to_vec()) }
fn rows(columns: &[(&str, u16)], records: Vec<Vec<Cell>>) -> Vec<u8> {
    let mut bytes = 2i32.to_be_bytes().to_vec(); // RESULT ROWS
    bytes.extend(1i32.to_be_bytes()); // GLOBAL_TABLES_SPEC
    bytes.extend((columns.len() as i32).to_be_bytes());
    short(&mut bytes, "fixture");
    short(&mut bytes, "items");
    for (name, kind) in columns {
        short(&mut bytes, name);
        bytes.extend(kind.to_be_bytes());
        if matches!(*kind, 0x20 | 0x22) { bytes.extend(0x0du16.to_be_bytes()); }
        if *kind == 0x21 { bytes.extend(0x0du16.to_be_bytes()); bytes.extend(0x0du16.to_be_bytes()); }
    }
    bytes.extend((records.len() as i32).to_be_bytes());
    for row in records {
        for cell in row {
            match cell {
                Some(value) => { bytes.extend((value.len() as i32).to_be_bytes()); bytes.extend(value); }
                None => bytes.extend((-1i32).to_be_bytes()),
            }
        }
    }
    bytes
}
fn error(code: i32, message: &str) -> Vec<u8> {
    let mut bytes = code.to_be_bytes().to_vec(); short(&mut bytes, message); bytes
}
fn literals(input: &str) -> Result<Vec<Option<String>>> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') { chars.next(); }
        if chars.peek().is_none() { break; }
        if chars.peek() == Some(&'\'') {
            chars.next();
            let mut value = String::new();
            let mut closed = false;
            while let Some(ch) = chars.next() {
                if ch == '\'' {
                    if chars.peek() == Some(&'\'') { chars.next(); value.push('\''); }
                    else { closed = true; break; }
                } else { value.push(ch); }
            }
            ensure!(closed, "CQL unclosed literal");
            ensure!(chars.peek().is_none_or(|c| c.is_whitespace() || *c == ','), "CQL literal suffix");
            out.push(Some(value));
        } else {
            let mut value = String::new();
            while chars.peek().is_some_and(|c| *c != ',') { value.push(chars.next().unwrap()); }
            let value = value.trim();
            ensure!(value.eq_ignore_ascii_case("null"), "SIM only quoted text or NULL is supported");
            out.push(None);
        }
    }
    Ok(out)
}
fn metadata(lower: &str) -> Option<Vec<u8>> {
    Some(if lower == "select release_version from system.local" {
        rows(&[("release_version", 0x0d)], vec![vec![text("4.1.0-SIM")]])
    } else if lower.starts_with("select host_id, rpc_address, data_center, rack, tokens from system.") {
        let records = if lower.ends_with("system.local") {
            let mut tokens = 1i32.to_be_bytes().to_vec(); put_string(&mut tokens, "0");
            vec![vec![Some(vec![1; 16]), Some(vec![127, 0, 0, 1]), text("dc1"), text("rack1"), Some(tokens)]]
        } else { vec![] };
        rows(&[("host_id", 0x0c), ("rpc_address", 0x10), ("data_center", 0x0d), ("rack", 0x0d), ("tokens", 0x22)], records)
    } else if lower.contains("system_schema.keyspaces") {
        if lower.contains("replication") {
            let mut map = 2i32.to_be_bytes().to_vec();
            for item in ["class", "org.apache.cassandra.locator.SimpleStrategy", "replication_factor", "1"] { put_string(&mut map, item); }
            rows(&[("keyspace_name", 0x0d), ("replication", 0x21)], vec![vec![text("fixture"), Some(map)]])
        } else { rows(&[("keyspace_name", 0x0d)], vec![vec![text("fixture")]]) }
    } else if lower.contains("system_schema.tables") {
        if lower.starts_with("select table_name") {
            let records = if lower.contains("keyspace_name='fixture'") { vec![vec![text("items")]] } else { vec![] };
            rows(&[("table_name", 0x0d)], records)
        } else { rows(&[("keyspace_name", 0x0d), ("table_name", 0x0d)], vec![vec![text("fixture"), text("items")]]) }
    } else if lower.contains("system_schema.views") {
        rows(&[("keyspace_name", 0x0d), ("view_name", 0x0d), ("base_table_name", 0x0d)], vec![])
    } else if lower.contains("system_schema.types") {
        rows(&[("keyspace_name", 0x0d), ("type_name", 0x0d), ("field_names", 0x20), ("field_types", 0x20)], vec![])
    } else if lower.contains("system_schema.columns") {
        rows(&[("keyspace_name", 0x0d), ("table_name", 0x0d), ("column_name", 0x0d), ("kind", 0x0d), ("position", 9), ("type", 0x0d)],
            ["id", "payload", "nullable_value"].iter().enumerate().map(|(index, name)| vec![text("fixture"), text("items"), text(name), text(if index == 0 { "partition_key" } else { "regular" }), Some((if index == 0 { 0i32 } else { -1i32 }).to_be_bytes().to_vec()), text("text")]).collect())
    } else if lower.starts_with("select schema_version from system.") {
        rows(&[("schema_version", 0x0c)], vec![vec![Some(vec![1; 16])]])
    } else { return None; })
}
fn query(sql: &str, state: &Records) -> Result<Vec<u8>> {
    let sql = sql.trim().trim_end_matches(';').trim();
    let lower = sql.to_ascii_lowercase();
    if lower.starts_with("use ") {
        ensure!(sql[4..].trim().trim_matches('"') == "fixture", "SIM unknown keyspace");
        let mut bytes = 3i32.to_be_bytes().to_vec(); short(&mut bytes, "fixture"); return Ok(bytes);
    }
    if let Some(result) = metadata(&lower) { return Ok(result); }
    if lower.starts_with("insert into fixture.items (id,payload,nullable_value) values ") {
        let begin = "insert into fixture.items (id,payload,nullable_value) values ".len();
        let values = sql[begin..].trim();
        ensure!(values.starts_with('(') && values.ends_with(')'), "SIM value tuple");
        let row = literals(&values[1..values.len()-1])?;
        ensure!(row.len() == 3 && row[0].is_some(), "SIM expected three values and non-null id");
        let mut records = state.lock().unwrap();
        if let Some(existing) = records.iter_mut().find(|r| r[0] == row[0]) { *existing = row; } else { records.push(row); }
        return Ok(1i32.to_be_bytes().to_vec());
    }
    if let Some(suffix) = lower.strip_prefix("select * from \"fixture\".\"items\"").or_else(|| lower.strip_prefix("select * from fixture.items")) {
        let suffix = suffix.trim();
        let limit = if suffix.is_empty() { 10_000 } else { suffix.strip_prefix("limit ").ok_or_else(|| anyhow!("SIM unsupported SELECT suffix"))?.trim().parse::<usize>()? };
        ensure!(limit > 0, "CQL LIMIT must be positive");
        let records = state.lock().unwrap().iter().take(limit).map(|r| r.iter().map(|v| v.as_ref().map(|s| s.as_bytes().to_vec())).collect()).collect();
        return Ok(rows(&[("id", 0x0d), ("payload", 0x0d), ("nullable_value", 0x0d)], records));
    }
    bail!("SIM unsupported CQL or missing table")
}

pub async fn start() -> Result<Server> {
    let state = seed();
    serve(move |mut stream, log| {
        let state = state.clone();
        async move {
            let mut authenticated = false;
            loop {
                let version = match stream.read_u8().await { Ok(v) => v, Err(_) => break };
                let flags = stream.read_u8().await?;
                let sequence = stream.read_i16().await?;
                let opcode = stream.read_u8().await?;
                let size = stream.read_i32().await?;
                ensure!(version == 4 && flags == 0 && (0..=1_000_000).contains(&size), "CQL frame header");
                let mut input = vec![0; size as usize]; stream.read_exact(&mut input).await?;
                let (response_opcode, response) = match opcode {
                    5 => { let mut bytes = 1u16.to_be_bytes().to_vec(); short(&mut bytes, "CQL_VERSION"); bytes.extend(1u16.to_be_bytes()); short(&mut bytes, "3.0.0"); (6, bytes) }
                    1 => { let mut bytes = vec![]; short(&mut bytes, "org.apache.cassandra.auth.PasswordAuthenticator"); (3, bytes) }
                    15 => {
                        ensure!(input.len() >= 4, "CQL auth length");
                        let length = i32::from_be_bytes(input[..4].try_into()?);
                        ensure!(length >= 0 && length as usize == input.len()-4, "CQL auth token length");
                        authenticated = &input[4..] == b"\0fixture\0secret";
                        log.lock().unwrap().push(format!("authenticate:{authenticated}"));
                        if authenticated { (16, 0i32.to_be_bytes().to_vec()) } else { (0, error(0x0100, "SIM authentication rejected")) }
                    }
                    11 if authenticated => (2, vec![]),
                    7 if authenticated => {
                        ensure!(input.len() >= 4, "CQL query length");
                        let length = u32::from_be_bytes(input[..4].try_into()?) as usize;
                        ensure!(length <= input.len()-4, "CQL query bounds");
                        let sql = std::str::from_utf8(&input[4..4+length])?;
                        log.lock().unwrap().push(sql.to_owned());
                        match query(sql, &state) {
                            Ok(bytes) => (8, bytes),
                            Err(error_value) => { log.lock().unwrap().push(format!("query_rejected:{error_value:#}")); (0, error(0x2200, &format!("{error_value:#}"))) }
                        }
                    }
                    _ => (0, error(0x2100, "SIM unsupported command or unauthenticated")),
                };
                stream.write_u8(0x84).await?; stream.write_u8(0).await?; stream.write_i16(sequence).await?;
                stream.write_u8(response_opcode).await?; stream.write_i32(response.len() as i32).await?; stream.write_all(&response).await?;
            }
            Ok(())
        }
    }).await
}
