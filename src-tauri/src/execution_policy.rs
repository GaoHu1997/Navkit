//! 可证明的保守只读子集：完整解析整个批次，拒绝函数/运算符/转换/CTE/视图/虚表。
//! 不是 SELECT 前缀或函数名白名单。SQLite VM authorizer 是独立第二层。
use crate::error::{AppError, CmdResult};

#[derive(Debug, Clone, PartialEq)]
enum Token { Word(String), Name(String), Literal, Number(String), Star, Comma, Dot, Semi }
fn lex(sql: &str) -> CmdResult<Vec<Token>> {
    let chars: Vec<char> = sql.chars().collect(); let mut i = 0; let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() { i += 1; continue; }
        if c == '-' && chars.get(i+1) == Some(&'-') {
            i += 2; while i < chars.len() && chars[i] != '\n' { i += 1; } continue;
        }
        if c == '/' && chars.get(i+1) == Some(&'*') {
            // MySQL 可执行注释、优化器提示及嵌套注释不进入此子集。
            if matches!(chars.get(i+2), Some('!') | Some('+')) || matches!(chars.get(i+2), Some('M') | Some('m')) && chars.get(i+3) == Some(&'!') { return Err(denied()); }
            i += 2; let mut closed = false;
            while i+1 < chars.len() {
                if chars[i] == '/' && chars[i+1] == '*' { return Err(denied()); }
                if chars[i] == '*' && chars[i+1] == '/' { i += 2; closed = true; break; }
                i += 1;
            }
            if !closed { return Err(denied()); } continue;
        }
        if c == '\'' || c == '"' || c == '`' {
            let quote = c; i += 1; let mut value = String::new(); let mut closed = false;
            while i < chars.len() {
                if chars[i] == quote {
                    if chars.get(i+1) == Some(&quote) { value.push(quote); i += 2; continue; }
                    i += 1; closed = true; break;
                }
                if chars[i] == '\0' || chars[i] == '\\' { return Err(denied()); }
                value.push(chars[i]); i += 1;
            }
            if !closed { return Err(denied()); }
            out.push(if quote == '\'' { Token::Literal } else { Token::Name(value) }); continue;
        }
        if c.is_ascii_digit() {
            let begin = i; while i < chars.len() && chars[i].is_ascii_digit() { i += 1; }
            out.push(Token::Number(chars[begin..i].iter().collect())); continue;
        }
        if c.is_alphabetic() || c == '_' {
            let begin = i; i += 1; while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') { i += 1; }
            out.push(Token::Word(chars[begin..i].iter().collect())); continue;
        }
        out.push(match c { '*' => Token::Star, ',' => Token::Comma, '.' => Token::Dot, ';' => Token::Semi, _ => return Err(denied()) }); i += 1;
    }
    Ok(out)
}
fn denied() -> AppError { AppError::coded("READ_ONLY_UNPROVEN", "只读执行仅支持常量 SELECT 或单个实体表的列投影及整数 LIMIT/OFFSET；表达式、函数、CTE、视图和控制语句不能证明无副作用，已在发送前拒绝") }
fn word(t: Option<&Token>, expected: &str) -> bool { matches!(t, Some(Token::Word(w)) if w.eq_ignore_ascii_case(expected)) }
fn name(t: Option<&Token>) -> Option<String> {
    match t { Some(Token::Name(n)) => Some(n.clone()), Some(Token::Word(n)) if !["SELECT","FROM","INTO","WHERE","LIMIT","OFFSET","AS","COLLATE","UNION","NULL","RETURNING"].iter().any(|w| n.eq_ignore_ascii_case(w)) => Some(n.clone()), _ => None }
}
#[derive(Debug)]
pub struct ReadPlan { pub tables: Vec<String> }
/// 可写查询也不能用旧 SQL IPC 伪造跨请求事务或改变绑定命名空间。
pub fn ordinary_native_batch(sql: &str) -> CmdResult<()> {
    let blocks = crate::manual_transaction::tokenize(sql).map_err(AppError::from)?;
    // A trigger body legitimately contains `BEGIN ... END` and its own `;` boundaries, so the
    // splitter yields a trailing `END` block. Accept that block only as the closure of a
    // `CREATE TRIGGER ... BEGIN` seen earlier. Bare controls, and any control inside the body,
    // still fail closed.
    let mut open_bodies = 0usize;
    for block in blocks {
        let first = block.first().map(String::as_str).unwrap_or("");
        let closure = first == "END" && block.len() == 1;
        if open_bodies > 0 {
            if closure { open_bodies -= 1; continue; }
            if protected_control_keyword(first) { return Err(protected_control()); }
            continue;
        }
        if protected_control_keyword(first) { return Err(protected_control()); }
        if first == "CREATE" && block.iter().any(|token| token.as_str() == "TRIGGER") {
            // When the first body statement ends with `;` it shares this block, so a control
            // keyword after `BEGIN` must be rejected before the splitter hides it elsewhere.
            if let Some(begin) = block.iter().position(|token| token.as_str() == "BEGIN") {
                let ends = block.iter().filter(|token| token.as_str() == "END").count();
                for token in &block[begin + 1..] {
                    let name = token.as_str();
                    if protected_control_keyword(name) && name != "END" { return Err(protected_control()); }
                    // Only the leading keyword can be a statement; later `SET` may belong to
                    // `UPDATE ... SET`, which is a legitimate trigger-body statement.
                    break;
                }
                if ends == 0 { open_bodies += 1; }
            }
        }
    }
    Ok(())
}
fn protected_control_keyword(name: &str) -> bool {
    matches!(name, "BEGIN" | "START" | "COMMIT" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" | "END" | "USE" | "SET" | "RESET" | "DISCARD" | "ATTACH" | "DETACH" | "LOCK" | "UNLOCK" | "PRAGMA" | "LISTEN" | "UNLISTEN")
}
fn protected_control() -> AppError {
    // 常见场景：mysqldump 导出的脚本开头带 USE / SET，用户直接丢进"运行 SQL 文件"。
    // 明确告诉用户删哪几行，而不是只说"控制语句被拒"。
    AppError::coded(
        "EXECUTION_PROTECTED_CONTROL",
        "事务/会话/命名空间控制语句（USE、BEGIN/COMMIT、SET、LOCK 等）不能出现在批次中：运行 SQL 文件固定作用于当前所选库。请删除文件中的这类语句（mysqldump 导出的脚本常见于开头）后重试；批次没有发送",
    )
}
pub fn readonly_batch(sql: &str) -> CmdResult<ReadPlan> {
    let tokens = lex(sql)?; let mut tables = Vec::new(); let mut count = 0;
    for statement in tokens.split(|t| *t == Token::Semi).filter(|s| !s.is_empty()) {
        let mut i = 0;
        if !word(statement.get(i), "SELECT") { return Err(denied()); } i += 1;
        let mut identifiers = false;
        loop {
            match statement.get(i) {
                Some(Token::Literal | Token::Number(_)) => { i += 1; }
                t if word(t, "NULL") => { i += 1; }
                Some(Token::Star) => { identifiers = true; i += 1; }
                t if name(t).is_some() => { identifiers = true; i += 1; }
                _ => return Err(denied()),
            }
            if statement.get(i) != Some(&Token::Comma) { break; } i += 1;
        }
        let mut from = false;
        if word(statement.get(i), "FROM") {
            i += 1; let table = name(statement.get(i)).ok_or_else(denied)?; i += 1;
            // 不允许跨命名空间或对象解析歧义。
            tables.push(table); from = true;
        }
        if identifiers && !from { return Err(denied()); }
        if word(statement.get(i), "LIMIT") {
            i += 1; if !matches!(statement.get(i), Some(Token::Number(n)) if n.parse::<u64>().is_ok()) { return Err(denied()); } i += 1;
            if word(statement.get(i), "OFFSET") { i += 1; if !matches!(statement.get(i), Some(Token::Number(n)) if n.parse::<u64>().is_ok()) { return Err(denied()); } i += 1; }
        }
        if i != statement.len() { return Err(denied()); } count += 1;
    }
    if count == 0 { return Err(denied()); }
    Ok(ReadPlan { tables })
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn trigger_body_begin_end_is_not_transaction_control() {
        for sql in [
            "CREATE TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO audit VALUES('x'); END",
            "CREATE TEMP TRIGGER \"a\" BEFORE UPDATE ON \"t\" FOR EACH ROW WHEN NEW.x>0 BEGIN UPDATE audit SET n=n+1; DELETE FROM log; END",
            "CREATE TRIGGER inline AFTER INSERT ON t BEGIN INSERT INTO audit VALUES(1) END",
            "CREATE TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO a VALUES(1); END; CREATE TRIGGER tr2 AFTER DELETE ON t BEGIN DELETE FROM a; END",
        ] { assert!(ordinary_native_batch(sql).is_ok(), "{sql}"); }
    }
    #[test] fn real_transaction_control_stays_rejected_including_inside_trigger_bodies() {
        for sql in [
            "BEGIN", "begin;", "START TRANSACTION", "COMMIT", "ROLLBACK", "SAVEPOINT a", "END",
            "BEGIN; SELECT 1", "PRAGMA query_only=OFF", "ATTACH DATABASE 'x' AS y", "USE db", "SET autocommit=1", "LOCK TABLES t WRITE",
            "CREATE TRIGGER tr AFTER INSERT ON t BEGIN COMMIT; END",
            "CREATE TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO a VALUES(1); ROLLBACK; END",
            "CREATE TRIGGER tr AFTER INSERT ON t BEGIN PRAGMA foreign_keys=OFF; END",
            "CREATE TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO a VALUES(1); END; COMMIT",
            "CREATE TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO a VALUES(1); END; BEGIN",
        ] { assert!(ordinary_native_batch(sql).is_err(), "must reject: {sql}"); }
    }
    #[tokio::test]
    async fn accepted_trigger_batch_really_creates_a_working_sqlite_trigger() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        let batch = "CREATE TABLE items(id INTEGER); CREATE TRIGGER audit_insert AFTER INSERT ON items BEGIN INSERT INTO items VALUES(NEW.id + 100); END";
        assert!(ordinary_native_batch(batch).is_ok());
        crate::sql_history::query("CREATE TABLE items(id INTEGER)").execute(&pool).await.unwrap();
        crate::sql_history::query("CREATE TRIGGER audit_insert AFTER INSERT ON items BEGIN INSERT INTO items VALUES(NEW.id + 100); END").execute(&pool).await.unwrap();
        crate::sql_history::query("INSERT INTO items VALUES(1)").execute(&pool).await.unwrap();
        let count: i64 = crate::sql_history::query_scalar("SELECT count(*) FROM items").fetch_one(&pool).await.unwrap();
        assert_eq!(count, 2);
        pool.close().await;
    }
    #[test] fn whole_batch_and_side_effects_rejected() {
        for sql in ["SELECT 1; DELETE FROM t", "WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x", "SELECT 1 INTO x", "EXPLAIN ANALYZE DELETE FROM t", "SELECT f()", "SELECT 1 + 2", "SELECT a::custom FROM t", "SELECT a COLLATE evil FROM t", "/*!50000 DELETE FROM t */SELECT 1", "SELECT * FROM a.b", "SELECT * FROM t WHERE a=1", "PRAGMA query_only=OFF"] { assert!(readonly_batch(sql).is_err(), "{sql}"); }
        assert_eq!(readonly_batch("SELECT '中文;DROP',NULL,9223372036854775807; SELECT \"a\" FROM \"t\" LIMIT 2 OFFSET 1").unwrap().tables, vec!["t"]);
    }
}
