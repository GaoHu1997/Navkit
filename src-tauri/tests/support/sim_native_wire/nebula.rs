//! NebulaGraph v3 unframed Thrift Binary authenticate / executeJson subset.
//! Vertex JSON follows vesoft Nebula Vertex::toJson: tag.property keys, with meta id/type.
use super::*;
use anyhow::{anyhow, bail, ensure, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn string(stream: &mut TcpStream) -> Result<String> {
    let size = stream.read_i32().await?;
    ensure!((0..=1_000_000).contains(&size), "Thrift string length");
    let mut bytes = vec![0; size as usize]; stream.read_exact(&mut bytes).await?;
    Ok(String::from_utf8(bytes)?)
}
fn field(bytes: &mut Vec<u8>, kind: u8, id: i16) { bytes.push(kind); bytes.extend(id.to_be_bytes()); }
fn success(columns: &[&str], rows: Vec<Vec<Value>>) -> Value {
    json!({"errors":[{"code":0}],"results":[{"columns":columns,"data":rows.into_iter().map(|row|json!({"row":row})).collect::<Vec<_>>() }]})
}
fn scalar(cell: &Option<String>) -> Value { cell.as_ref().map_or(Value::Null, |s| json!(s)) }
fn tuple(input: &str) -> Result<Vec<Option<String>>> {
    let mut input = input.trim();
    let mut values = Vec::new();
    while !input.is_empty() {
        let (value, used) = if input.starts_with('"') {
            let mut escaped = false;
            let end = input.char_indices().skip(1).find_map(|(index, ch)| {
                if escaped { escaped = false; return None; }
                if ch == '\\' { escaped = true; return None; }
                (ch == '"').then_some(index + 1)
            }).ok_or_else(|| anyhow!("nGQL unclosed JSON string"))?;
            (Some(serde_json::from_str::<String>(&input[..end])?), end)
        } else if input.get(..4).is_some_and(|s| s.eq_ignore_ascii_case("NULL")) { (None, 4) }
        else { bail!("SIM nGQL supports string / NULL properties only") };
        values.push(value);
        input = input[used..].trim_start();
        if input.is_empty() { break; }
        input = input.strip_prefix(',').ok_or_else(|| anyhow!("nGQL tuple delimiter"))?.trim_start();
        ensure!(!input.is_empty(), "nGQL trailing comma");
    }
    Ok(values)
}
fn execute(sql: &str, state: &Records) -> Result<Value> {
    let mut stmt = sql.trim().trim_end_matches(';').trim();
    // Only consume an actual leading USE, never split on semicolons in values.
    if stmt.get(..4).is_some_and(|s| s.eq_ignore_ascii_case("USE ")) {
        let remainder = stmt[4..].trim_start();
        let (space, rest) = if let Some(quoted) = remainder.strip_prefix('`') {
            let end = quoted.find('`').ok_or_else(|| anyhow!("nGQL USE quote"))?;
            (&quoted[..end], quoted[end+1..].trim_start())
        } else { remainder.split_once(';').unwrap_or((remainder, "")) };
        ensure!(space.trim() == "fixture", "SIM missing graph space");
        stmt = rest.strip_prefix(';').unwrap_or(rest).trim();
        if stmt.is_empty() { return Ok(success(&[], vec![])); }
    }
    let lower = stmt.to_ascii_lowercase();
    if lower == "show spaces" { return Ok(success(&["Name"], vec![vec![json!("fixture")]])); }
    if lower == "show tags" { return Ok(success(&["Name"], vec![vec![json!("items")]])); }
    if lower == "show edges" { return Ok(success(&["Name"], vec![])); }
    let prefix = "insert vertex items(id,payload,nullable_value) values ";
    if lower.starts_with(prefix) {
        let input = stmt[prefix.len()..].trim();
        let mut strings = serde_json::Deserializer::from_str(input).into_iter::<String>();
        let id = strings.next().ok_or_else(|| anyhow!("nGQL missing vertex id"))??;
        let values = input[strings.byte_offset()..].trim().strip_prefix(':').ok_or_else(|| anyhow!("nGQL vertex colon"))?.trim();
        ensure!(values.starts_with('(') && values.ends_with(')'), "nGQL vertex tuple");
        let row = tuple(&values[1..values.len()-1])?;
        ensure!(row.len() == 3 && row[0].as_deref() == Some(id.as_str()), "nGQL id / property tuple");
        let mut records = state.lock().unwrap();
        if let Some(existing) = records.iter_mut().find(|r| r[0] == row[0]) { *existing = row; } else { records.push(row); }
        return Ok(success(&[], vec![]));
    }
    if lower.starts_with("match (v:`items`) return ") {
        let (projection, pagination) = lower["match (v:`items`) return ".len()..].split_once(" skip ").unwrap_or((&lower["match (v:`items`) return ".len()..], ""));
        let (offset, limit) = if pagination.is_empty() { (0, 10_000) } else {
            let (offset, limit) = pagination.split_once(" limit ").ok_or_else(|| anyhow!("nGQL pagination"))?;
            (offset.trim().parse::<usize>()?, limit.trim().parse::<usize>()?)
        };
        let records = state.lock().unwrap();
        if projection == "v" {
            // A graph vertex is ONE JSON object column, not three scalar columns.
            let data = records.iter().skip(offset).take(limit).map(|row| json!({
                "row":[{"items.id":scalar(&row[0]),"items.payload":scalar(&row[1]),"items.nullable_value":scalar(&row[2])}],
                "meta":[{"id":scalar(&row[0]),"type":"vertex"}]
            })).collect::<Vec<_>>();
            return Ok(json!({"errors":[{"code":0}],"results":[{"columns":["v"],"data":data}]}));
        }
        ensure!(projection == "id(v) as id,v.items.payload as payload,v.items.nullable_value as nullable_value", "SIM unsupported graph projection");
        return Ok(success(&["id", "payload", "nullable_value"], records.iter().skip(offset).take(limit).map(|row| row.iter().map(scalar).collect()).collect()));
    }
    bail!("SIM unsupported graph command or missing tag")
}

pub async fn start() -> Result<Server> {
    let records = seed();
    serve(move |mut stream, log| {
        let records = records.clone();
        async move {
            let mut authenticated = false;
            loop {
                let header = match stream.read_u32().await { Ok(v) => v, Err(_) => break };
                ensure!(header == 0x80010001 || header == 0x80010004, "Thrift non-binary CALL");
                let method = string(&mut stream).await?;
                let sequence = stream.read_i32().await?;
                let mut args = BTreeMap::new();
                let mut session = 0;
                loop {
                    let kind = stream.read_u8().await?;
                    if kind == 0 { break; }
                    let id = stream.read_i16().await?;
                    match kind {
                        11 => { args.insert(id, string(&mut stream).await?); }
                        10 => { session = stream.read_i64().await?; }
                        _ => bail!("unsupported Thrift argument {kind}"),
                    }
                }
                let mut response = 0x80010002u32.to_be_bytes().to_vec();
                put_string(&mut response, &method); response.extend(sequence.to_be_bytes());
                match method.as_str() {
                    "authenticate" => {
                        authenticated = args.get(&1).map(String::as_str) == Some("fixture") && args.get(&2).map(String::as_str) == Some("secret");
                        log.lock().unwrap().push(format!("authenticate:{authenticated}"));
                        field(&mut response, 12, 0); // authenticate_result.success
                        field(&mut response, 8, 1); response.extend((if authenticated { 0i32 } else { -1001i32 }).to_be_bytes());
                        if authenticated { field(&mut response, 10, 3); response.extend(42i64.to_be_bytes()); }
                        else { field(&mut response, 11, 2); put_string(&mut response, "SIM invalid credentials"); }
                        response.extend([0, 0]);
                    }
                    "executeJson" => {
                        let sql = args.get(&2).cloned().unwrap_or_default();
                        log.lock().unwrap().push(sql.clone());
                        let result = if !authenticated || session != 42 { json!({"errors":[{"code":-1001,"message":"invalid session"}],"results":[]}) }
                        else { match execute(&sql, &records) {
                            Ok(result) => result,
                            Err(error) => { log.lock().unwrap().push(format!("query_rejected:{error:#}")); json!({"errors":[{"code":-1004,"message":format!("{error:#}")}],"results":[]}) }
                        }};
                        field(&mut response, 11, 0); put_string(&mut response, &result.to_string()); response.push(0);
                    }
                    "signout" => { authenticated = false; response.push(0); }
                    _ => bail!("unsupported Thrift method {method}"),
                }
                if header != 0x80010004 { stream.write_all(&response).await?; }
            }
            Ok(())
        }
    }).await
}
