use super::{http_command_statements, split_protocol_commands};

#[test]
fn json_dsl_is_not_reinterpreted_as_sql() {
    let json = r#"{"filter":"中文;O'Reilly\\尾\"; still one command","nullable":null}"#;
    for kind in ["elasticsearch","opensearch","couchdb","milvus","qdrant","chroma","weaviate","meilisearch","victoriametrics","rabbitmq","pulsar","nacos","consul","dynamodb","etcd","mqtt","kafka","zookeeper","rocketmq"] {
        assert_eq!(http_command_statements(kind,json),vec![json.to_string()],"{kind}");
    }
}
#[test]
fn actual_sql_adapters_retain_statement_batches_and_literal_semicolons() {
    for kind in ["h2","druid","rqlite","cassandra","nebulagraph","tdengine","iotdb","influxdb","turso","d1","solr"] {
        assert_eq!(http_command_statements(kind,"SELECT '中文;O''Reilly\\尾'; SELECT 2"),vec!["SELECT '中文;O''Reilly\\尾'".to_string()," SELECT 2".to_string()],"{kind}");
    }
}
#[test]
fn redis_cli_preserves_escaped_quotes_semicolons_and_real_batches() {
    let first = r#"SET k "中文;O'Reilly\\尾\"; still the same value""#;
    let command=format!("{first}; GET k; ");
    assert_eq!(split_protocol_commands(&command),vec![first.to_string()," GET k".to_string()]);
    assert_eq!(split_protocol_commands(r#"SET k "a\\"; GET k"#).len(),2,"even backslashes do not escape the closing quote");
}
#[test]
fn mongo_nested_json_preserves_strings_and_does_not_split_malformed_payloads() {
    let first=r#"items {"filter":{"payload":"a\";中文"},"list":[null,{"x":";"}]}"#;
    assert_eq!(split_protocol_commands(&format!("{first}; count items {{}}")),vec![first.to_string()," count items {}".to_string()]);
    let broken=r#"items {"filter":[1}; count items {}"#;
    assert_eq!(split_protocol_commands(broken),vec![broken.to_string()]);
    let unclosed=r#"SET k "broken; DEL k"#;
    assert_eq!(split_protocol_commands(unclosed),vec![unclosed.to_string()]);
}
