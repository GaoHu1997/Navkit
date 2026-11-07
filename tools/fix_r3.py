# -*- coding: utf-8 -*-
"""验收 #1 整改（后端剩余）：池缓存失效、解密报错、found_rows、定时任务按库执行。"""
import io

# ============ db.rs：close_prefix + found_rows ============
p = "src/db.rs"
s = io.open(p, encoding="utf-8").read()

old = """/// 关闭全部连接。"""
new = """/// 关闭一个连接及其全部按库派生池（key = `{id}` 或 `{id}@{db}`）。
pub fn close_with_derived(registry: &DbRegistry, id: &str) {
    let prefix = format!("{}@", id);
    registry
        .write()
        .retain(|k, _| !(k == id || k.starts_with(&prefix)));
}

/// 关闭全部连接。"""
assert old in s
s = s.replace(old, new, 1)

old = """    let mut opts = MySqlConnectOptions::new()
        .host(req.host.as_str())
        .port(port)
        .username(user)
        .password(req.password.as_str())
        .charset("utf8mb4")
        .collation("utf8mb4_general_ci");"""
new = """    let mut opts = MySqlConnectOptions::new()
        .host(req.host.as_str())
        .port(port)
        .username(user)
        .password(req.password.as_str())
        .charset("utf8mb4")
        .collation("utf8mb4_general_ci")
        // affected 返回"命中行数"而非"实际变更行数"，
        // 否则把 1.50 改成 1.5 这类值不变更新会返回 0，前端误判为失败。
        .found_rows(true);"""
assert old in s
s = s.replace(old, new, 1)
io.open(p, "w", encoding="utf-8", newline="\n").write(s)
print("db.rs ok")

# ============ commands.rs ============
p = "src/commands.rs"
s = io.open(p, encoding="utf-8").read()

# 1) 解密失败显式报错的助手
old = """/// 应用全局状态。"""
new = """/// 解密连接密码：密文存在但解密为空 = key.bin 缺失/不匹配，
/// 显式报错而不是拿空密码去连（用户只会看到 Access denied，无从排查）。
fn decrypt_or_err(cfg: &ConnectionConfig) -> anyhow::Result<String> {
    let pw = storage::decrypt_password(&cfg.password);
    if !cfg.password.is_empty() && pw.is_empty() {
        return Err(anyhow!(
            "连接密码解密失败（data/key.bin 缺失或与保存时不一致），请打开连接属性重新输入密码"
        ));
    }
    Ok(pw)
}

/// 应用全局状态。"""
assert old in s
s = s.replace(old, new, 1)

# 2) 三个解密调用点换成助手
old = """    let password = storage::decrypt_password(&cfg.password);
    let mut req = db::request_from_config(cfg, &password);
    req.timeout_secs = state.store.settings().query_timeout_secs;
    db::open_and_register(&state.registry, &id, &req).await?;"""
new = """    let password = decrypt_or_err(cfg)?;
    let mut req = db::request_from_config(cfg, &password);
    req.timeout_secs = state.store.settings().query_timeout_secs;
    // 重新打开前先清掉旧池（含按库派生池），保证用最新凭据
    db::close_with_derived(&state.registry, &id);
    db::open_and_register(&state.registry, &id, &req).await?;"""
assert old in s
s = s.replace(old, new, 1)

old = """                let password = storage::decrypt_password(&cfg.password);
                let mut req = db::request_from_config(&cfg, &password);
                req.database = d.clone();"""
new = """                let password = decrypt_or_err(&cfg)?;
                let mut req = db::request_from_config(&cfg, &password);
                req.database = d.clone();"""
assert old in s
s = s.replace(old, new, 1)

old = """            let password = storage::decrypt_password(&cfg.password);
            let mut req = db::request_from_config(cfg, &password);
            req.timeout_secs = state.store.settings().query_timeout_secs;
            db::open_and_register(&registry, &job.connection_id, &req).await?;
        }
        let conn_db = db::get_db(&registry, &job.connection_id)?;
        // 任务 SQL 面向指定库执行（MySQL 协议族先 USE）
        if !job.database.is_empty() && kind.is_mysql_family() {
            let _ = crate::schema::exec_affected(
                conn_db.as_ref(),
                kind,
                &format!("USE {}", db::quote_ident(kind, &job.database)),
            )
            .await;
        }
        db::execute(&registry, &job.connection_id, &job.sql, kind)
            .await
            .map_err(AppError::from)"""
new = """            let password = decrypt_or_err(cfg)?;
            let mut req = db::request_from_config(cfg, &password);
            req.timeout_secs = state.store.settings().query_timeout_secs;
            db::open_and_register(&registry, &job.connection_id, &req).await?;
        }
        // 任务 SQL 面向指定库执行：复用 {id}@{db} 按库连接池（PG 无 USE，
        // 之前 PG 任务会静默跑在连接默认库上）。
        let target_key = if !job.database.is_empty() && kind != DbKind::Sqlite {
            let key = format!("{}@{}", job.connection_id, job.database);
            if state.registry.read().get(&key).is_none() {
                let conns = state.store.connections();
                let cfg = conns
                    .iter()
                    .find(|c| c.id == job.connection_id)
                    .ok_or_else(|| AppError::new("任务配置的连接不存在"))?
                    .clone();
                let password = decrypt_or_err(&cfg)?;
                let mut req = db::request_from_config(&cfg, &password);
                req.database = job.database.clone();
                req.timeout_secs = state.store.settings().query_timeout_secs;
                db::open_and_register(&registry, &key, &req).await?;
            }
            key
        } else {
            job.connection_id.clone()
        };
        db::execute(&registry, &target_key, &job.sql, kind)
            .await
            .map_err(AppError::from)"""
assert old in s
s = s.replace(old, new, 1)

# 3) delete_connection / save_connection 清派生池
old = """    let mut conns = state.store.connections();
    conns.retain(|c| c.id != id);
    let groups = state.store.groups();
    state.store.save_connections(conns.clone(), groups);
    db::close(&state.registry, &id);
    Ok(conns)
}"""
new = """    let mut conns = state.store.connections();
    conns.retain(|c| c.id != id);
    let groups = state.store.groups();
    state.store.save_connections(conns.clone(), groups);
    db::close_with_derived(&state.registry, &id);
    Ok(conns)
}"""
assert old in s
s = s.replace(old, new, 1)

old = """    match conns.iter_mut().find(|c| c.id == cfg.id) {
        Some(slot) => *slot = cfg,
        None => conns.push(cfg),
    }"""
new = """    let changed_id = cfg.id.clone();
    match conns.iter_mut().find(|c| c.id == cfg.id) {
        Some(slot) => *slot = cfg,
        None => conns.push(cfg),
    }
    // 配置/密码变更后关闭旧池（含按库派生池），下次打开用新配置
    db::close_with_derived(&state.registry, &changed_id);"""
assert old in s
s = s.replace(old, new, 1)

io.open(p, "w", encoding="utf-8", newline="\n").write(s)
print("commands.rs ok")
