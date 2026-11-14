//! MySQL 导出专用会话：基表保持 InnoDB 门禁，视图按真实依赖逐个核验。
use std::collections::{HashSet, VecDeque};
use anyhow::{anyhow, bail, Context, Result};
use futures::TryStreamExt;
use sqlx::{Executor, MySql, MySqlPool, Row, pool::PoolConnection};
use crate::{db::qualified, models::{DbKind, ExportOptions}, transfer::TaskControl};

/// 解析服务器 SHOW CREATE VIEW 的规范定义，只用于核对目录依赖是否完整。
/// 不从用户 SQL 猜测引擎；目录缺权限、存储函数或未知关系语法均明确拒绝。
fn dependencies(ddl: &str, namespace: &str) -> Result<HashSet<(String, String)>> {
    if ddl.len() > crate::transfer_stream::DEFAULT_MAX_RECORD_BYTES as usize { bail!("视图定义超过导出元数据上限"); }
    let mut chars = ddl.chars().peekable(); let mut tokens = Vec::new();
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() { continue; }
        if ch == '\'' || ch == '"' || ch == '`' {
            let mut word = String::new(); let mut closed = false;
            while let Some(next) = chars.next() {
                if next == ch {
                    if chars.peek() == Some(&ch) { chars.next(); word.push(ch); }
                    else { closed = true; break; }
                } else if next == '\\' && ch != '`' { if let Some(next) = chars.next() { word.push(next); } }
                else { word.push(next); }
            }
            if !closed { bail!("服务器视图定义含未闭合引用，不能确认完整依赖"); }
            tokens.push(if ch == '`' { (word, true) } else { (String::new(), true) });
        } else if ch.is_alphanumeric() || ch == '_' || ch == '$' {
            let mut word = ch.to_string();
            while chars.peek().is_some_and(|next| next.is_alphanumeric() || *next == '_' || *next == '$') { word.push(chars.next().unwrap()); }
            tokens.push((word, false));
        } else { tokens.push((ch.to_string(), false)); }
        if tokens.len() > 131_072 { bail!("视图依赖元数据超过上限"); }
    }
    let mut queries = vec![false]; let mut from = vec![false]; let mut found = HashSet::new();
    for index in 0..tokens.len() {
        let (word, quoted) = &tokens[index]; let depth = queries.len() - 1;
        if !quoted && word == "(" { queries.push(false); from.push(false); continue; }
        if !quoted && word == ")" { if queries.len() > 1 { queries.pop(); from.pop(); } continue; }
        // SHOW CREATE 将存储函数保留为限定名；其内部读表不在 VIEW_TABLE_USAGE 中。
        if index + 3 < tokens.len() && tokens[index+1].0 == "." && tokens[index+3].0 == "(" {
            bail!("视图引用存储函数，无法从表依赖目录核验其事务引擎；未发送数据读取");
        }
        if !quoted && word.eq_ignore_ascii_case("SELECT") { queries[depth] = true; }
        if !quoted && ["WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "UNION"].iter().any(|key| word.eq_ignore_ascii_case(key)) { from[depth] = false; }
        let start = !quoted && queries[depth] && (word.eq_ignore_ascii_case("FROM") || word.eq_ignore_ascii_case("JOIN") || word.eq_ignore_ascii_case("STRAIGHT_JOIN") || (word == "," && from[depth]));
        if !start { continue; }
        if word.eq_ignore_ascii_case("FROM") { from[depth] = true; }
        let Some((first, _)) = tokens.get(index+1) else { bail!("视图关系引用不完整"); };
        if first == "(" { continue; }
        if first.eq_ignore_ascii_case("DUAL") { continue; }
        if first.is_empty() || [".", ")", ";"].contains(&first.as_str()) { bail!("视图关系语法不能核验"); }
        let object = if tokens.get(index+2).is_some_and(|token| token.0 == ".") {
            let second = tokens.get(index+3).filter(|token| !token.0.is_empty()).ok_or_else(|| anyhow!("视图限定依赖不完整"))?;
            (first.clone(), second.0.clone())
        } else { (namespace.to_owned(), first.clone()) };
        found.insert(object);
        if found.len() > 256 { bail!("视图依赖集合超过 256 个对象"); }
    }
    Ok(found)
}

async fn prepare(connection: &mut sqlx::MySqlConnection, kind: &DbKind, options: &ExportOptions, task: &TaskControl, shared: bool) -> Result<Option<String>> {
    let namespace = if options.schema.is_empty() { crate::sql_history::query_scalar::<_, Option<String>>("SELECT DATABASE()").fetch_one(&mut *connection).await?.ok_or_else(|| anyhow!("请选择导出数据库"))? } else { options.schema.clone() };
    let mut queue = VecDeque::from([(namespace.clone(), options.table.clone())]); let mut visited = HashSet::new(); let mut root_ddl = None;
    // 专用只读事务；完成后真实 ROLLBACK，失败则关闭连接，绝不把未排空事务回池。
    if !shared {
    crate::sql_history::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").execute(&mut *connection).await?;
    // START TRANSACTION 不在 MySQL 预处理语句白名单内，服务端返回 1295；必须走文本协议（无参数 &str 即 COM_QUERY）。
    // TiDB 不支持 `WITH CONSISTENT SNAPSHOT, READ ONLY` 的逗号并列写法（解析到逗号即 1064 语法错误）；
    // 上一句已设 REPEATABLE READ，事务内一致性快照由 MVCC 保证，只读改用标准的 `START TRANSACTION READ ONLY`。
    (&mut *connection).execute("START TRANSACTION READ ONLY").await?;
    }
    while let Some((schema, name)) = queue.pop_front() {
        task.check()?;
        if !visited.insert((schema.clone(), name.clone())) { continue; }
        if visited.len() > 256 { bail!("视图递归依赖超过导出核验上限"); }
        let row = crate::sql_history::query("SELECT TABLE_TYPE, ENGINE FROM information_schema.tables WHERE table_schema = ? AND table_name = ?")
            .bind(&schema).bind(&name).fetch_optional(&mut *connection).await?
            .ok_or_else(|| anyhow!("导出依赖 {}.{} 不存在或当前账号无法查看其引擎", schema, name))?;
        // information_schema.TABLE_TYPE 在 MySQL 8 返回 BINARY 排序规则，直接按 String 解码会失败。
        let object_type = crate::schema::mysql_get(&row, "TABLE_TYPE")
            .ok_or_else(|| anyhow!("导出依赖 {}.{} 不存在或当前账号无法查看其引擎", schema, name))?;
        let engine = crate::schema::mysql_get(&row, "ENGINE");
        if object_type.eq_ignore_ascii_case("BASE TABLE") {
            if !engine.as_deref().is_some_and(|engine| engine.eq_ignore_ascii_case("InnoDB")) {
                bail!("导出依赖 {}.{} 未确认是 InnoDB 基表（引擎 {}），没有发布文件", schema, name, engine.as_deref().unwrap_or("未知"));
            }
            continue;
        }
        if !object_type.eq_ignore_ascii_case("VIEW") { bail!("导出依赖对象类型尚未验证：{}.{}（{}）", schema, name, object_type); }
        let row = crate::sql_history::query(&format!("SHOW CREATE VIEW {}", qualified(kind, &schema, &name))).fetch_optional(&mut *connection).await?
            .ok_or_else(|| anyhow!("视图定义不可见，请核对 SHOW VIEW 权限"))?;
        let ddl: String = row.try_get(1).context("无法读取真实 CREATE VIEW，需 SHOW VIEW 权限")?;
        // SQL 视图仅写定义，不读取依赖数据；其引擎不构成 DDL 导出的门禁。
        if options.format.eq_ignore_ascii_case("sql") && schema == namespace && name == options.table { return Ok(Some(ddl)); }
        let expected = dependencies(&ddl, &schema)?;
        let mut actual = HashSet::new();
        let mut stream = crate::sql_history::query("SELECT TABLE_SCHEMA, TABLE_NAME FROM information_schema.VIEW_TABLE_USAGE WHERE VIEW_SCHEMA = ? AND VIEW_NAME = ?")
            .bind(&schema).bind(&name).fetch(&mut *connection);
        while let Some(row) = stream.try_next().await.context("无法查看视图依赖目录；需要支持 VIEW_TABLE_USAGE 的 MySQL 版本及相应元数据权限")? {
            actual.insert((row.try_get::<String, _>(0)?, row.try_get::<String, _>(1)?));
            if actual.len() > 256 { bail!("视图依赖目录超过导出上限"); }
        }
        drop(stream);
        if expected != actual { bail!("视图 {}.{} 的目录依赖与真实定义不一致，可能缺少依赖元数据权限；未发送导出 SELECT", schema, name); }
        queue.extend(actual);
        if schema == namespace && name == options.table { root_ddl = Some(ddl); }
    }
    Ok(root_ddl.filter(|_| options.format.eq_ignore_ascii_case("sql")))
}

pub(super) async fn session(pool: &MySqlPool, kind: &DbKind, options: &ExportOptions, task: &TaskControl) -> Result<(PoolConnection<MySql>, Option<String>, bool)> {
    task.check()?; let mut connection = pool.acquire().await?;
    let prepared = async {
        let shared = crate::transfer::transfer_backup_native::mysql_shared_reader(&mut connection,&options.schema,&options.table).await?;
        let ddl = prepare(&mut connection, kind, options, task, shared).await?;
        Ok::<_,anyhow::Error>((ddl,shared))
    }.await;
    match prepared {
        Ok((ddl,shared)) => Ok((connection, ddl, !shared)),
        Err(error) => match connection.close().await {
            Ok(()) => Err(error),
            Err(close) => Err(error.context(format!("TRANSFER_EFFECTS_UNKNOWN: MySQL 导出会话关闭未确认，目标未发布：{close}"))),
        },
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn canonical_view_dependencies_include_nested_joins_and_exclude_literals() {
        let ddl = "CREATE ALGORITHM=UNDEFINED DEFINER=`u`@`h` SQL SECURITY DEFINER VIEW `d`.`v` AS select `a`.`id`, 'from `evil`.`x`' from `d`.`a` join (select `b`.`id` from `other`.`b`) `q` on `a`.`id`=`q`.`id`, `d`.`c`";
        assert_eq!(dependencies(ddl, "d").unwrap(), HashSet::from([("d".into(),"a".into()),("other".into(),"b".into()),("d".into(),"c".into())]));
        assert!(dependencies("CREATE VIEW `d`.`v` AS select 'FROM hidden'", "d").unwrap().is_empty());
        assert!(dependencies("CREATE VIEW `d`.`v` AS select `d`.`reads_hidden`()", "d").is_err());
    }
}
