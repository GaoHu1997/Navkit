//! Strict adapter evidence validation; no authorization, SQL compilation or plan storage.
use std::collections::BTreeSet;
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use super::{DbValue, JdbcCapabilities, JdbcMetadataSnapshot, JdbcMutationOutcome, JdbcObjectId, PROTOCOL_VERSION, FRAME_LIMIT};

/// Reject duplicate keys at EVERY level before converting to serde_json::Value.
/// Otherwise a forged repeated count/kind/session field can overwrite evidence.
pub fn decode_json(text: &str) -> Result<Value> {
    if text.len() > FRAME_LIMIT { bail!("JDBC response exceeds the frame limit"); }
    let mut decoder = serde_json::Deserializer::from_str(text);
    let value = JsonSeed(0).deserialize(&mut decoder).context("JDBC response JSON is malformed or ambiguous")?;
    decoder.end().context("JDBC response has trailing JSON content")?;
    Ok(value)
}
struct JsonSeed(usize);
impl<'de> DeserializeSeed<'de> for JsonSeed {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> std::result::Result<Value, D::Error> {
        if self.0 > 32 { return Err(D::Error::custom("JDBC JSON exceeds depth 32")); }
        decoder.deserialize_any(JsonVisitor(self.0))
    }
}
struct JsonVisitor(usize);
impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("bounded unambiguous JSON") }
    fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Value, E> { Ok(Value::Null) }
    fn visit_none<E: serde::de::Error>(self) -> std::result::Result<Value, E> { Ok(Value::Null) }
    fn visit_bool<E: serde::de::Error>(self, value: bool) -> std::result::Result<Value, E> { Ok(Value::Bool(value)) }
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> std::result::Result<Value, E> { Ok(Value::Number(value.into())) }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> std::result::Result<Value, E> { Ok(Value::Number(value.into())) }
    fn visit_f64<E: serde::de::Error>(self, value: f64) -> std::result::Result<Value, E> {
        Number::from_f64(value).map(Value::Number).ok_or_else(|| E::custom("Nonfinite JDBC JSON number"))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<Value, E> {
        if value.len() > 16 * 1024 * 1024 { return Err(E::custom("JDBC JSON value exceeds 16 MiB")); }
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> std::result::Result<Value, E> {
        if value.len() > 16 * 1024 * 1024 { return Err(E::custom("JDBC JSON value exceeds 16 MiB")); }
        Ok(Value::String(value))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> std::result::Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(JsonSeed(self.0 + 1))? {
            if values.len() >= 16384 { return Err(A::Error::custom("JDBC JSON array exceeds 16384 entries")); }
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> std::result::Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if key.len() > 1024 || values.len() >= 16384 || values.contains_key(&key) {
                return Err(A::Error::custom("Duplicate, oversized or excessive JDBC JSON object key"));
            }
            values.insert(key, object.next_value_seed(JsonSeed(self.0 + 1))?);
        }
        Ok(Value::Object(values))
    }
}

pub fn exact_fields(value: &Value, expected: &[&str]) -> Result<()> {
    let object = value.as_object().ok_or_else(|| anyhow!("JDBC contract expected an object"))?;
    if object.len() != expected.len() || expected.iter().any(|name| !object.contains_key(*name)) {
        bail!("JDBC contract has missing or unexpected fields");
    }
    Ok(())
}
fn uuid(value: &str) -> bool { uuid::Uuid::parse_str(value).is_ok() }
fn revision(value: &str) -> bool { value.len() == 64 && value.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) }
fn object_valid(object: &JdbcObjectId) -> bool {
    [&object.catalog, &object.schema, &object.name].into_iter().all(|name| !name.is_empty() && name.len() <= 4096 && !name.contains('\0'))
}
fn known_names(key: &[String], columns: &BTreeSet<String>) -> bool {
    !key.is_empty() && key.iter().all(|name| columns.contains(name)) && key.iter().collect::<BTreeSet<_>>().len() == key.len()
}

pub fn validate_scalar(line: &str) -> Result<()> {
    let value = decode_json(line)?;
    if let Some(error) = value.get("error") {
        exact_fields(&value, &["error"])?;
        if !error.is_string() { bail!("JDBC scalar error must be a string"); }
    } else {
        exact_fields(&value, &["columns", "rows"])?;
        let columns = value.get("columns").and_then(Value::as_array).ok_or_else(|| anyhow!("JDBC scalar columns missing"))?;
        let rows = value.get("rows").and_then(Value::as_array).ok_or_else(|| anyhow!("JDBC scalar rows missing"))?;
        if columns.len() > 1024 || columns.iter().any(|column| !column.is_string()) || rows.len() > 10000
            || rows.iter().any(|row| row.as_array().map_or(true, |values| values.len() != columns.len() || values.iter().any(|item| !item.is_string() && !item.is_null()))) {
            bail!("JDBC scalar response is malformed or exceeds the explicit preview bounds");
        }
    }
    Ok(())
}

pub fn validate_result(action: &str, payload: &Value, envelope: &Value) -> Result<()> {
    if envelope.get("error").is_some() { return Ok(()); }
    let session = envelope.get("session_id").and_then(Value::as_str).ok_or_else(|| anyhow!("JDBC session missing"))?;
    let result = envelope.get("result").cloned().ok_or_else(|| anyhow!("JDBC result missing"))?;
    match action {
        "capabilities" => {
            exact_fields(&result, &["protocol_version", "session_id", "adapter_id", "product", "product_version", "driver_name", "driver_version", "catalog", "schema", "read_only", "transactions", "metadata", "row_write", "ddl_write", "bound_types", "reason"])?;
            let caps: JdbcCapabilities = serde_json::from_value(result).context("JDBC capability contract mismatch")?;
            if caps.protocol_version != PROTOCOL_VERSION || caps.session_id != session || !uuid(&caps.session_id)
                || caps.adapter_id.is_empty() || caps.ddl_write
                || caps.row_write && (!caps.transactions || !caps.metadata || caps.read_only)
                || !caps.row_write && caps.reason.as_deref().map_or(true, str::is_empty) {
                bail!("JDBC capability evidence is contradictory or stale");
            }
            let types: BTreeSet<_> = caps.bound_types.iter().map(String::as_str).collect();
            let verified = ["null", "text", "integer", "decimal", "float", "bool", "binary", "date", "time", "timestamp", "timestamptz", "uuid"];
            if types.len() != caps.bound_types.len() || types.iter().any(|kind| !verified.contains(kind))
                || caps.row_write && (!types.contains("integer") || !types.contains("decimal") || !types.contains("null")) {
                bail!("JDBC capability claims unverified or incomplete typed bindings");
            }
        }
        "metadata" => {
            exact_fields(&result, &["object", "session_id", "backend_object_id", "revision", "table_type", "definition", "columns", "primary_key", "unique_keys", "indexes", "foreign_keys", "constraints", "triggers", "completeness"])?;
            for column in result.get("columns").and_then(Value::as_array).ok_or_else(|| anyhow!("JDBC columns missing"))? {
                exact_fields(column, &["name", "jdbc_type", "type_name", "ordinal", "precision", "scale", "nullable", "auto_increment", "generated", "read_only", "default_value", "generation_expression", "writable_type"])?;
            }
            let expected: JdbcObjectId = serde_json::from_value(payload.get("object").cloned().ok_or_else(|| anyhow!("JDBC requested object missing"))?)?;
            let metadata: JdbcMetadataSnapshot = serde_json::from_value(result).context("JDBC metadata contract mismatch")?;
            if metadata.object != expected || !object_valid(&metadata.object) || metadata.session_id != session
                || !revision(&metadata.revision) || metadata.backend_object_id.parse::<u64>().is_err()
                || !matches!(metadata.table_type.as_str(), "BASE TABLE" | "TABLE" | "VIEW")
                || metadata.definition.as_deref().map_or(true, str::is_empty)
                || metadata.columns.is_empty() || metadata.columns.len() > 1024 {
                bail!("JDBC metadata object/session/revision/completeness mismatch");
            }
            let columns: BTreeSet<_> = metadata.columns.iter().map(|column| column.name.clone()).collect();
            if columns.len() != metadata.columns.len() || metadata.columns.iter().enumerate().any(|(index, column)|
                column.ordinal != index as u32 + 1 || column.name.is_empty() || column.type_name.is_empty()) {
                bail!("JDBC metadata column identity/order is incomplete");
            }
            if !metadata.primary_key.is_empty() && (!known_names(&metadata.primary_key, &columns) || !metadata.unique_keys.contains(&metadata.primary_key))
                || metadata.unique_keys.iter().any(|key| !known_names(key, &columns)) {
                bail!("JDBC metadata primary/unique key evidence is incomplete");
            }
            let sections = ["columns", "keys", "indexes", "foreign_keys", "constraints", "triggers", "definition"];
            if metadata.completeness.len() != sections.len() || sections.iter().any(|section| metadata.completeness.get(*section).map(String::as_str) != Some("known"))
                || [&metadata.indexes, &metadata.foreign_keys, &metadata.constraints, &metadata.triggers].into_iter().any(|part| !part.is_array()) {
                bail!("JDBC metadata unknown sections cannot be treated as empty success");
            }
        }
        "row_plan" => {
            exact_fields(&result, &["operation_id", "status", "affected_rows", "matched_rows", "verified", "transaction", "connection_poisoned", "mismatches", "readback", "generated_keys"])?;
            let outcome: JdbcMutationOutcome = serde_json::from_value(result).context("JDBC row evidence contract mismatch")?;
            let operation = payload.get("operation_id").and_then(Value::as_str).ok_or_else(|| anyhow!("JDBC operation missing"))?;
            let kind = payload.get("kind").and_then(Value::as_str).ok_or_else(|| anyhow!("JDBC operation kind missing"))?;
            if outcome.operation_id != operation || !uuid(operation)
                || !matches!(outcome.matched_rows.as_deref(), None | Some("0" | "1" | "2")) {
                bail!("JDBC mutation identity/count mismatch; outcome unknown, do not replay");
            }
            match outcome.status.as_str() {
                "applied" | "unchanged" => {
                    let count = if outcome.status == "applied" { "1" } else { "0" };
                    if !outcome.verified || outcome.transaction != "committed" || outcome.affected_rows.as_deref() != Some(count)
                        || outcome.matched_rows.as_deref() != Some("1") || outcome.status == "unchanged" && kind != "update"
                        || kind == "delete" && !outcome.readback.is_empty() || kind != "delete" && outcome.readback.is_empty()
                        || kind != "insert" && !outcome.generated_keys.is_empty() {
                        bail!("JDBC successful receipt lacks exact commit/count/readback evidence");
                    }
                }
                "conflict" | "failed" => {
                    if outcome.verified || outcome.affected_rows.as_deref() != Some("0") || !matches!(outcome.transaction.as_str(), "rolled_back" | "none")
                        || outcome.mismatches.is_empty() || !outcome.readback.is_empty() || !outcome.generated_keys.is_empty() {
                        bail!("JDBC refused mutation fabricates success/committed rows");
                    }
                }
                "unknown" => {
                    if outcome.verified || outcome.affected_rows.is_some() || outcome.transaction != "unknown" || !outcome.connection_poisoned
                        || outcome.mismatches.is_empty() || !outcome.readback.is_empty() || !outcome.generated_keys.is_empty() {
                        bail!("JDBC unknown commit must retain null count and destroy its physical connection");
                    }
                }
                _ => bail!("JDBC mutation returned an unrecognized status"),
            }
            for pairs in [&outcome.readback, &outcome.generated_keys] {
                if pairs.len() > 1024 || pairs.iter().map(|(name, _)| name).collect::<BTreeSet<_>>().len() != pairs.len() {
                    bail!("JDBC readback has duplicate or excessive columns");
                }
                for (name, value) in pairs { if name.is_empty() || name.len() > 4096 { bail!("JDBC readback column identity invalid"); } validate_value(value)?; }
            }
        }
        _ => bail!("JDBC unknown relational operation"),
    }
    Ok(())
}

fn validate_value(value: &DbValue) -> Result<()> {
    let valid = match value {
        DbValue::Null | DbValue::Bool(_) => true,
        DbValue::Float(number) => number.is_finite(),
        DbValue::Integer(text) => text.parse::<i64>().map_or(false, |number| number.to_string() == *text),
        DbValue::Decimal(text) => {
            let number = text.strip_prefix('-').unwrap_or(text);
            let mut parts = number.split('.');
            let whole = parts.next().unwrap_or("");
            let fraction = parts.next();
            !whole.is_empty() && whole.bytes().all(|c| c.is_ascii_digit()) && parts.next().is_none()
                && fraction.map_or(true, |part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
        }
        DbValue::Binary(text) => STANDARD.decode(text).map_or(false, |bytes| bytes.len() <= 16 * 1024 * 1024 && STANDARD.encode(bytes) == *text),
        DbValue::Uuid(text) => uuid(text),
        DbValue::Text(_) => true,
        DbValue::Date(text) | DbValue::Time(text) | DbValue::Timestamp(text) | DbValue::Timestamptz(text) => !text.is_empty(),
        // Java emits canonical integer strings, never JSON floating/integer guesses
        // or vendor-specific JSON/LOB values that have not been verified.
        _ => false,
    };
    let size = match value {
        DbValue::Text(text) | DbValue::Integer(text) | DbValue::Decimal(text) | DbValue::Binary(text) | DbValue::Uuid(text)
        | DbValue::Date(text) | DbValue::Time(text) | DbValue::Timestamp(text) | DbValue::Timestamptz(text) => text.len(), _ => 0,
    };
    if !valid || size > 16 * 1024 * 1024 { bail!("JDBC typed readback is malformed, lossy or unsupported"); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> (String, String) { (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string()) }
    fn outcome(status: &str) -> (Value, Value) {
        let (operation, session) = identity();
        (serde_json::json!({"operation_id":operation,"kind":"update"}), serde_json::json!({"session_id":session,"result":{
            "operation_id":operation,"status":status,"affected_rows":"1","matched_rows":"1","verified":true,
            "transaction":"committed","connection_poisoned":false,"mismatches":[],
            "readback":[["ID",{"kind":"integer","value":"9223372036854775807"}]],"generated_keys":[]}}))
    }
    #[test] fn scalar_compatibility_is_preserved_without_guessing_counts() {
        assert!(validate_scalar(r#"{"columns":["值"],"rows":[[null],[""]]}"#).is_ok());
        assert!(validate_scalar(r#"{"error":"driver [42000]: query rejected"}"#).is_ok());
        for line in [r#"{"columns":["ID"],"rows":[[1]]}"#, r#"{"columns":["ID"],"rows":[[]]}"#,
            r#"{"error":"bad","columns":[],"rows":[]}"#, r#"{"columns":[],"rows":[],"rows":[[]]}"#] { assert!(validate_scalar(line).is_err()); }
    }
    #[test] fn duplicate_fields_at_every_level_are_corruption() {
        for input in [r#"{"protocol":2,"protocol":1}"#, r#"{"result":{"verified":true,"verified":false}}"#,
            r#"{"readback":[["ID",{"kind":"integer","value":"1","kind":"text"}]]}"#] { assert!(decode_json(input).is_err()); }
        assert!(decode_json("{} trailing").is_err());
        assert!(decode_json(&format!("{}null{}", "[".repeat(34), "]".repeat(34))).is_err());
    }
    #[test] fn contradictory_success_unknown_and_counts_are_rejected() {
        let (payload, envelope) = outcome("applied");
        assert!(validate_result("row_plan", &payload, &envelope).is_ok());
        for (key, value) in [("affected_rows", serde_json::json!(1)), ("matched_rows", serde_json::json!("2")),
            ("transaction", serde_json::json!("rolled_back")), ("verified", serde_json::json!(false)), ("operation_id", serde_json::json!("wrong"))] {
            let mut bad = envelope.clone(); bad["result"][key] = value;
            assert!(validate_result("row_plan", &payload, &bad).is_err(), "{key}");
        }
        let mut unknown = envelope.clone(); unknown["result"]["status"] = serde_json::json!("unknown");
        unknown["result"]["transaction"] = serde_json::json!("unknown"); unknown["result"]["verified"] = serde_json::json!(false);
        unknown["result"]["affected_rows"] = Value::Null; unknown["result"]["readback"] = serde_json::json!([]);
        unknown["result"]["mismatches"] = serde_json::json!(["ACK missing"]);
        assert!(validate_result("row_plan", &payload, &unknown).is_err());
        unknown["result"]["connection_poisoned"] = serde_json::json!(true);
        assert!(validate_result("row_plan", &payload, &unknown).is_ok());
    }
    #[test] fn precise_readback_requires_canonical_type_tags() {
        assert!(validate_value(&DbValue::Integer("9223372036854775807".into())).is_ok());
        assert!(validate_value(&DbValue::Decimal("12345678901234567890.123456789012".into())).is_ok());
        assert!(validate_value(&DbValue::Binary("AP8BgA==".into())).is_ok());
        for value in [DbValue::Int(1), DbValue::Integer("01".into()), DbValue::Integer("9223372036854775808".into()),
            DbValue::Decimal("1e20".into()), DbValue::Binary("AP8BgA".into()), DbValue::Json("{}".into())] {
            assert!(validate_value(&value).is_err(), "{value:?}");
        }
    }
    #[test] fn metadata_must_match_complete_object_identity() {
        let (_, session) = identity();
        let object = serde_json::json!({"catalog":"TEST","schema":"PUBLIC","name":"T"});
        let payload = serde_json::json!({"object":object});
        let envelope = serde_json::json!({"session_id":session,"result":{
            "object":object,"session_id":session,"backend_object_id":"3","revision":"a".repeat(64),"table_type":"BASE TABLE","definition":"CREATE TABLE T(ID BIGINT)",
            "columns":[{"name":"ID","jdbc_type":-5,"type_name":"BIGINT","ordinal":1,"precision":64,"scale":0,"nullable":false,
                "auto_increment":false,"generated":false,"read_only":false,"default_value":null,"generation_expression":null,"writable_type":true}],
            "primary_key":["ID"],"unique_keys":[["ID"]],"indexes":[],"foreign_keys":[],"constraints":[],"triggers":[],
            "completeness":{"columns":"known","keys":"known","indexes":"known","foreign_keys":"known","constraints":"known","triggers":"known","definition":"known"}}});
        assert!(validate_result("metadata", &payload, &envelope).is_ok());
        let mut stale = envelope.clone(); stale["result"]["object"]["schema"] = serde_json::json!("OTHER");
        assert!(validate_result("metadata", &payload, &stale).is_err());
        let mut unknown = envelope; unknown["result"]["completeness"]["keys"] = serde_json::json!("unknown");
        assert!(validate_result("metadata", &payload, &unknown).is_err());
    }
}
