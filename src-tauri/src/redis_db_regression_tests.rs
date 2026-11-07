//! Focused regressions for lossless Redis replies and TLS fail-closed options.
//! Included by redis_db after its current full-test snapshot finishes.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};

fn request(params: &str) -> ConnectionRequest {
    ConnectionRequest {
        kind: DbKind::of("redis"), host: "127.0.0.1".into(), port: 6379,
        user: "".into(), password: "do-not-leak-this-password".into(),
        database: "".into(), file: "".into(), params: params.into(), timeout_secs: 3,
    }
}

#[test]
fn binary_query_replies_are_lossless_and_not_preview_truncated() {
    for bytes in [vec![0xff, 0x00], (0..=255u8).collect::<Vec<_>>(), vec![0xff; 8193]] {
        let table = value_to_table(redis::Value::BulkString(bytes.clone()));
        let encoded = table[0][0].as_ref().unwrap().strip_prefix("base64:").unwrap();
        assert_eq!(STANDARD.decode(encoded).unwrap(), bytes);
    }
    assert_eq!(value_to_table(redis::Value::BulkString(vec![0xff, 0x00])), vec![vec![Some("base64:/wA=".into())]]);
    let text = "中文原样".repeat(4097);
    assert_eq!(value_to_table(redis::Value::BulkString(text.as_bytes().to_vec())), vec![vec![Some(text)]]);
    assert_eq!(value_to_table(redis::Value::Array(vec![redis::Value::BulkString(vec![0xff, 0x00])])), vec![vec![Some("base64:/wA=".into())]]);
}

#[test]
fn tls_modes_and_json_requests_never_silently_downgrade() {
    for params in [
        "SSLMode=REQUIRE", "ssl-mode=verify-full", "sslmode=preferred", "sslmode=bad",
        "ssl=false;sslmode=require", r#"{"sslmode":"require"}"#, r#"{"tls":true}"#,
        r#"{"ssl":true"#, r#"["ssl",true]"#,
    ] {
        let error = connection_info(&request(params)).unwrap_err().to_string();
        assert!(!error.contains("do-not-leak-this-password"));
    }
    for params in ["sslmode=disable", "ssl-mode=disabled", r#"{"sslmode":"disable","db":1}"#] {
        let info = connection_info(&request(params)).unwrap();
        assert!(matches!(info.addr, redis::ConnectionAddr::Tcp(..)));
    }
}
