//! Generic SQL namespace discovery. A failed directory query is never a synthetic catalog.
use anyhow::{anyhow, Result};
use std::future::Future;

use crate::models::DbKind;

type MetadataRows = (Vec<String>, Vec<Vec<Option<String>>>);

fn candidates(kind: &DbKind) -> &'static [&'static str] {
    match kind.id() {
        "clickhouse" | "hive" | "inceptor" | "argodb" => &["SHOW DATABASES"],
        "dameng" => &["SELECT NAME FROM V$DATABASE"],
        "sqlserver" | "mssql" => &[
            "SELECT name FROM sys.databases WHERE state_desc = 'ONLINE' ORDER BY name",
        ],
        "oracle" => &["SELECT username FROM all_users ORDER BY username"],
        "db2" => &["SELECT schemaname FROM syscat.schemata ORDER BY schemaname"],
        "informix" => &["SELECT name FROM sysmaster:sysdatabases"],
        "teradata" => &["SELECT DatabaseName FROM dbc.Databases ORDER BY DatabaseName"],
        "saphana" => &["SELECT SCHEMA_NAME FROM SYS.SCHEMAS ORDER BY SCHEMA_NAME"],
        "snowflake" | "redshift" | "bigquery" | "dremio" | "trino" | "prestosql"
        | "exasol" | "vertica" | "spark" | "impala" | "kyuubi" | "kylin" => &[
            "SELECT schema_name FROM information_schema.schemata ORDER BY schema_name",
            "SHOW DATABASES",
            "SHOW SCHEMAS",
        ],
        _ => &[
            "SELECT schema_name FROM information_schema.schemata ORDER BY schema_name",
            "SHOW DATABASES",
        ],
    }
}

/// The injected query is the same read-only metadata transport used by the live adapter.
/// The first successful query is authoritative even when the visible catalog is empty.
pub(super) async fn list_with<Q, F>(kind: &DbKind, mut query: Q) -> Result<Vec<(String, String)>>
where
    Q: FnMut(&'static str) -> F,
    F: Future<Output = Result<MetadataRows>>,
{
    // Access addresses one opened file and has no independent database/schema selector.
    // DuckDB and unknown ODBC sources do have queryable namespaces; they must not use this case.
    if kind.id() == "access" {
        return Ok(vec![("main".to_string(), "main".to_string())]);
    }
    let mut errors = Vec::new();
    for sql in candidates(kind) {
        match query(sql).await {
            Ok((columns, rows)) => {
                let name_index = columns.iter().position(|column| {
                    ["name", "database", "database_name", "databasename", "schema_name", "schemaname", "username"]
                        .iter().any(|name| column.eq_ignore_ascii_case(name))
                }).unwrap_or(0);
                return Ok(rows.into_iter().filter_map(|row| {
                    let name = row.get(name_index)?.clone()?;
                    if name.trim().is_empty() { return None; }
                    Some((name.clone(), name))
                }).collect());
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
    Err(anyhow!("{} 的命名空间目录查询全部失败：{}", kind.label(), errors.join("；")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::ready;

    #[tokio::test]
    async fn all_directory_queries_failing_propagates_instead_of_main() {
        for kind in ["sqlserver", "clickhouse", "oracle", "duckdb", "unknown_odbc"] {
            let mut queries = Vec::new();
            let error = list_with(&DbKind::of(kind), |sql| {
                queries.push(sql);
                ready(Err(anyhow!("catalog permission denied")))
            }).await.unwrap_err();
            assert!(!queries.is_empty(), "{kind}");
            assert!(error.to_string().contains("catalog permission denied"), "{kind}: {error}");
        }
    }

    #[tokio::test]
    async fn successful_empty_catalog_does_not_try_a_different_namespace_or_main() {
        let mut calls = 0;
        let namespaces = list_with(&DbKind::of("duckdb"), |_| {
            calls += 1;
            ready(Ok((vec!["schema_name".into()], Vec::new())))
        }).await.unwrap();
        assert!(namespaces.is_empty());
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn failed_candidate_can_fall_back_to_real_results_or_real_empty_catalog() {
        for empty in [false, true] {
            let mut calls = 0;
            let namespaces = list_with(&DbKind::of("unknown_odbc"), |_| {
                calls += 1;
                ready(if calls == 1 { Err(anyhow!("unsupported SQL")) } else {
                    Ok((vec!["Database".into()], if empty { vec![] } else { vec![vec![Some("real_catalog".into())]] }))
                })
            }).await.unwrap();
            assert_eq!(calls, 2);
            assert_eq!(namespaces, if empty { vec![] } else { vec![("real_catalog".into(), "real_catalog".into())] });
        }
    }

    #[tokio::test]
    async fn dialect_directory_columns_preserve_exact_names_without_null_or_blank_options() {
        for (kind, column) in [("sqlserver", "name"), ("oracle", "USERNAME"), ("db2", "SCHEMANAME"), ("saphana", "SCHEMA_NAME"), ("teradata", "DatabaseName")] {
            let namespaces = list_with(&DbKind::of(kind), |sql| {
                assert!(!sql.starts_with("CREATE") && !sql.starts_with("DROP"));
                ready(Ok((vec!["metadata".into(), column.into()], vec![
                    vec![Some("ignored".into()), Some("真实.目录".into())],
                    vec![None, None], vec![None, Some(" ".into())],
                ])))
            }).await.unwrap();
            assert_eq!(namespaces, vec![("真实.目录".into(), "真实.目录".into())], "{kind}");
        }
    }

    #[tokio::test]
    async fn access_is_the_explicit_single_file_namespace_without_directory_queries() {
        let namespaces = list_with(&DbKind::of("access"), |_| {
            panic!("the opened Access file has no namespace directory");
            #[allow(unreachable_code)]
            ready(Err(anyhow!("must not query")))
        }).await.unwrap();
        assert_eq!(namespaces, vec![("main".into(), "main".into())]);
    }
}
