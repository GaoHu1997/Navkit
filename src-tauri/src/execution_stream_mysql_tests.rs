//! Loopback MySQL 8 wire-protocol mock that drives the **real sqlx `MySql` driver**
//! through `execution::run`, to verify opt-in streaming first-page semantics.
//!
//! No external service and no user database is touched: the mock binds
//! `127.0.0.1:0` and implements only what sqlx 0.8.6 needs:
//! initial handshake -> OK (empty password / `mysql_native_password`),
//! `COM_QUERY` (`SET` init / `PING` / `KILL`), and
//! `COM_STMT_PREPARE` / `COM_STMT_EXECUTE` / `COM_STMT_CLOSE` with column
//! definitions, binary rows and classic EOF packets.
//!
//! The data query sends `page_rows` rows, then blocks on a oneshot before the
//! tail so `StreamProgress::wait_first` must return a `Running` provisional
//! preview. Releasing the tail must let the worker finish, `Entry::finish`
//! must publish `Succeeded`, and the full spool (total > 10_000) must still
//! contain the exact last row. A later server error must fail the receipt and
//! revoke the spool; a targeted `KILL QUERY` must cancel.
//!
//! Registered as a test-only submodule of execution_stream.rs.

use super::{StreamOptions, StreamProgress};
use crate::db::Db;
use crate::execution::{Binding, Registry};
use crate::execution_contracts::{ExecutionRequest, ExecutionState};
use crate::models::DbKind;
use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlSslMode};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex, Notify};

const SQL: &str = "SELECT id, name FROM bench";
const TOTAL_ROWS: usize = 10_050;
const PAGE_ROWS: usize = 200;

const TYPE_LONGLONG: u8 = 0x08;
const TYPE_VARSTRING: u8 = 0xfd;
const FLAG_UNSIGNED: u16 = 0x0020;

#[derive(Clone, Copy, PartialEq)]
enum Tail {
    Complete,
    Error,
}

#[derive(Clone, Copy)]
enum Kind {
    Data,
    ConnId,
    Kill,
    Noop,
}

impl Kind {
    fn columns(self) -> u16 {
        match self {
            Kind::Data => 2,
            Kind::ConnId => 1,
            Kind::Kill | Kind::Noop => 0,
        }
    }
}

struct Gate {
    rows: usize,
    page_rows: usize,
    tail: Tail,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    kill: Notify,
    killed: AtomicBool,
}

fn kind_of(sql: &str) -> Kind {
    let upper = sql.trim_start().to_ascii_uppercase();
    if upper.starts_with("KILL") {
        Kind::Kill
    } else if upper.contains("CONNECTION_ID") {
        Kind::ConnId
    } else if upper.starts_with("SELECT") {
        Kind::Data
    } else {
        Kind::Noop
    }
}

fn push_lenenc_int(out: &mut Vec<u8>, value: u64) {
    if value < 251 {
        out.push(value as u8);
    } else if value < 65_536 {
        out.push(0xfc);
        out.extend_from_slice(&(value as u16).to_le_bytes());
    } else if value < 16_777_216 {
        out.push(0xfd);
        let bytes = (value as u32).to_le_bytes();
        out.extend_from_slice(&bytes[..3]);
    } else {
        out.push(0xfe);
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn push_lenenc_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    push_lenenc_int(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn ok() -> Vec<u8> {
    // 0x00 OK, affected_rows=0, last_insert_id=0, status=AUTOCOMMIT, warnings=0
    vec![0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00]
}

fn eof() -> Vec<u8> {
    // classic EOF (DEPRECATE_EOF deliberately not negotiated)
    vec![0xfe, 0x00, 0x00, 0x02, 0x00]
}

fn err(code: u16, state: &[u8; 5], message: &str) -> Vec<u8> {
    let mut out = vec![0xffu8];
    out.extend_from_slice(&code.to_le_bytes());
    out.push(b'#');
    out.extend_from_slice(state);
    out.extend_from_slice(message.as_bytes());
    out
}

fn prepare_ok(statement_id: u32, columns: u16) -> Vec<u8> {
    let mut out = vec![0x00u8];
    out.extend_from_slice(&statement_id.to_le_bytes());
    out.extend_from_slice(&columns.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // params
    out.push(0x00); // reserved
    out.extend_from_slice(&0u16.to_le_bytes()); // warnings
    out
}

fn column_def(name: &str, type_id: u8, flags: u16, max_size: u32) -> Vec<u8> {
    let mut out = Vec::new();
    push_lenenc_bytes(&mut out, b"def");
    push_lenenc_bytes(&mut out, b"bench");
    push_lenenc_bytes(&mut out, name.as_bytes()); // table alias
    push_lenenc_bytes(&mut out, name.as_bytes()); // table
    push_lenenc_bytes(&mut out, name.as_bytes()); // column alias
    push_lenenc_bytes(&mut out, name.as_bytes()); // column name
    out.push(0x0c); // next_length
    out.extend_from_slice(&33u16.to_le_bytes()); // utf8_general_ci
    out.extend_from_slice(&max_size.to_le_bytes());
    out.push(type_id);
    out.extend_from_slice(&flags.to_le_bytes());
    out.push(0); // decimals
    out.extend_from_slice(&[0u8, 0u8]); // filler
    out
}

fn column_defs(kind: Kind) -> Vec<Vec<u8>> {
    match kind {
        Kind::Data => vec![
            column_def("id", TYPE_VARSTRING, 0, 1024),
            column_def("name", TYPE_VARSTRING, 0, 1024),
        ],
        Kind::ConnId => vec![column_def("CONNECTION_ID()", TYPE_LONGLONG, FLAG_UNSIGNED, 20)],
        Kind::Kill | Kind::Noop => Vec::new(),
    }
}

fn binary_str_row(values: &[&str]) -> Vec<u8> {
    let mut out = vec![0x00u8]; // 0x00 header
    let null_bitmap_len = (values.len() + 9) / 8;
    out.extend(std::iter::repeat(0u8).take(null_bitmap_len));
    for value in values {
        push_lenenc_bytes(&mut out, value.as_bytes());
    }
    out
}

fn binary_u64_row(value: u64) -> Vec<u8> {
    let mut out = vec![0x00u8];
    out.push(0u8); // null bitmap for a single column: (1 + 9) / 8 == 1
    out.extend_from_slice(&value.to_le_bytes());
    out
}

fn handshake() -> Vec<u8> {
    // MYSQL(bit0=long password) | FOUND_ROWS | LONG_FLAG | PROTOCOL_41
    // | TRANSACTIONS | SECURE_CONNECTION | MULTI_RESULTS | PS_MULTI_RESULTS
    // | PLUGIN_AUTH.  SSL and DEPRECATE_EOF are intentionally absent.
    let capabilities: u32 = 0x0000_0001
        | 0x0000_0002
        | 0x0000_0004
        | 0x0000_0200
        | 0x0000_2000
        | 0x0000_8000
        | 0x0002_0000
        | 0x0004_0000
        | 0x0008_0000;

    let mut out = Vec::new();
    out.push(10); // protocol version
    out.extend_from_slice(b"8.0.36\0");
    out.extend_from_slice(&7u32.to_le_bytes()); // connection id
    out.extend_from_slice(b"abcdefgh"); // auth plugin data, part 1 (8 bytes)
    out.push(0); // filler
    out.extend_from_slice(&(capabilities as u16).to_le_bytes());
    out.push(33); // server default collation
    out.extend_from_slice(&0x0002u16.to_le_bytes()); // SERVER_STATUS_AUTOCOMMIT
    out.extend_from_slice(&((capabilities >> 16) as u16).to_le_bytes());
    out.push(21); // auth plugin data length
    out.extend_from_slice(&[0u8; 6]); // reserved
    // The MYSQL compatibility bit is set, so sqlx advances these 4 bytes.
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(b"ijklmnopqrst"); // auth plugin data, part 2 (12 bytes)
    out.push(0); // NUL terminator
    out.extend_from_slice(b"mysql_native_password\0");
    out
}

async fn write_packet(stream: &mut TcpStream, sequence: u8, payload: &[u8]) -> io::Result<()> {
    let len = payload.len();
    let header = [
        (len & 0xff) as u8,
        ((len >> 8) & 0xff) as u8,
        ((len >> 16) & 0xff) as u8,
        sequence,
    ];
    let mut packet = Vec::with_capacity(4 + payload.len());
    packet.extend_from_slice(&header);
    packet.extend_from_slice(payload);
    stream.write_all(&packet).await?;
    Ok(())
}

async fn read_packet(stream: &mut TcpStream) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await?;
    let len = header[0] as usize | ((header[1] as usize) << 8) | ((header[2] as usize) << 16);
    let sequence = header[3];
    let mut payload = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut payload).await?;
    }
    Ok((sequence, payload))
}

/// Incrementing writer for one command response. sqlx does not validate the
/// sequence id, but the mock keeps it correct so the trace stays protocol-legal.
struct Out<'a> {
    stream: &'a mut TcpStream,
    sequence: u8,
}

impl<'a> Out<'a> {
    fn new(stream: &'a mut TcpStream, sequence: u8) -> Self {
        Self { stream, sequence }
    }

    async fn packet(&mut self, payload: &[u8]) -> io::Result<()> {
        write_packet(&mut *self.stream, self.sequence, payload).await?;
        self.sequence = self.sequence.wrapping_add(1);
        Ok(())
    }
}

async fn data_stream(out: &mut Out<'_>, gate: &Arc<Gate>) -> io::Result<()> {
    out.packet(&[0x02]).await?; // column count (length-encoded integer)
    for def in column_defs(Kind::Data) {
        out.packet(&def).await?;
    }
    out.packet(&eof()).await?;

    let total = gate.rows;
    let page = gate.page_rows.min(total);
    for index in 0..page {
        let id = index.to_string();
        let name = format!("row{index}");
        out.packet(&binary_str_row(&[id.as_str(), name.as_str()])).await?;
    }

    // Hold the tail until the test releases it, or until a KILL arrives.
    let receiver = {
        let mut guard = gate.release.lock().await;
        guard.take()
    };
    if let Some(receiver) = receiver {
        tokio::select! {
            _ = receiver => {}
            _ = gate.kill.notified() => {
                return out.packet(&err(1317, b"70100", "Query execution was interrupted")).await;
            }
        }
    }
    if gate.killed.load(Ordering::SeqCst) {
        return out.packet(&err(1317, b"70100", "Query execution was interrupted")).await;
    }
    if gate.tail == Tail::Error {
        return out.packet(&err(2013, b"HY000", "Lost connection to MySQL server during query")).await;
    }
    for index in page..total {
        if gate.killed.load(Ordering::SeqCst) {
            return out.packet(&err(1317, b"70100", "Query execution was interrupted")).await;
        }
        let id = index.to_string();
        let name = format!("row{index}");
        out.packet(&binary_str_row(&[id.as_str(), name.as_str()])).await?;
    }
    out.packet(&eof()).await
}

async fn query(out: &mut Out<'_>, sql: &str, gate: &Arc<Gate>) -> io::Result<()> {
    let upper = sql.trim_start().to_ascii_uppercase();
    if upper.starts_with("KILL") {
        gate.killed.store(true, Ordering::SeqCst);
        gate.kill.notify_one();
        return out.packet(&ok()).await;
    }
    if upper.contains("CONNECTION_ID") {
        out.packet(&[0x01]).await?;
        for def in column_defs(Kind::ConnId) {
            out.packet(&def).await?;
        }
        out.packet(&eof()).await?;
        out.packet(&binary_u64_row(42)).await?;
        out.packet(&eof()).await?;
        return Ok(());
    }
    // `SET ...` (connection init) and anything else that is not a result set.
    out.packet(&ok()).await
}

async fn execute(out: &mut Out<'_>, kind: Kind, gate: &Arc<Gate>) -> io::Result<()> {
    match kind {
        Kind::Noop => out.packet(&ok()).await,
        Kind::Kill => {
            gate.killed.store(true, Ordering::SeqCst);
            gate.kill.notify_one();
            out.packet(&ok()).await
        }
        Kind::ConnId => {
            out.packet(&[0x01]).await?;
            for def in column_defs(Kind::ConnId) {
                out.packet(&def).await?;
            }
            out.packet(&eof()).await?;
            out.packet(&binary_u64_row(42)).await?;
            out.packet(&eof()).await
        }
        Kind::Data => data_stream(out, gate).await,
    }
}

async fn handle_conn(mut stream: TcpStream, gate: Arc<Gate>) -> io::Result<()> {
    stream.set_nodelay(true)?;
    write_packet(&mut stream, 0, &handshake()).await?;
    let (client_sequence, _response) = read_packet(&mut stream).await?;
    write_packet(&mut stream, client_sequence.wrapping_add(1), &ok()).await?;

    let mut statements: HashMap<u32, Kind> = HashMap::new();
    let mut next_statement_id: u32 = 1;
    loop {
        let (client_sequence, packet) = match read_packet(&mut stream).await {
            Ok(value) => value,
            Err(_) => return Ok(()),
        };
        if packet.is_empty() {
            return Ok(());
        }
        match packet[0] {
            0x01 => return Ok(()), // COM_QUIT
            0x0e => write_packet(&mut stream, client_sequence.wrapping_add(1), &ok()).await?, // COM_PING
            0x03 => {
                // COM_QUERY: connection init `SET ...`, optional PING/KILL, or a
                // bare SELECT. The executor uses prepared statements elsewhere.
                let sql = String::from_utf8_lossy(&packet[1..]).into_owned();
                let mut out = Out::new(&mut stream, client_sequence.wrapping_add(1));
                query(&mut out, &sql, &gate).await?;
            }
            0x16 => {
                // COM_STMT_PREPARE
                let sql = String::from_utf8_lossy(&packet[1..]).into_owned();
                let kind = kind_of(&sql);
                let statement_id = next_statement_id;
                next_statement_id = next_statement_id.wrapping_add(1);
                statements.insert(statement_id, kind);
                let mut out = Out::new(&mut stream, client_sequence.wrapping_add(1));
                out.packet(&prepare_ok(statement_id, kind.columns())).await?;
                if kind.columns() > 0 {
                    for def in column_defs(kind) {
                        out.packet(&def).await?;
                    }
                    out.packet(&eof()).await?;
                }
            }
            0x17 => {
                // COM_STMT_EXECUTE
                let statement_id = if packet.len() >= 5 {
                    u32::from_le_bytes([packet[1], packet[2], packet[3], packet[4]])
                } else {
                    0
                };
                let kind = statements.get(&statement_id).copied().unwrap_or(Kind::Noop);
                let mut out = Out::new(&mut stream, client_sequence.wrapping_add(1));
                execute(&mut out, kind, &gate).await?;
            }
            0x19 => {
                // COM_STMT_CLOSE has no response.
                if packet.len() >= 5 {
                    statements.remove(&u32::from_le_bytes([
                        packet[1], packet[2], packet[3], packet[4],
                    ]));
                }
            }
            _ => write_packet(&mut stream, client_sequence.wrapping_add(1), &ok()).await?,
        }
    }
}

async fn start_mock(
    rows: usize,
    page_rows: usize,
    tail: Tail,
) -> (
    u16,
    tokio::task::JoinHandle<()>,
    Arc<Gate>,
    oneshot::Sender<()>,
) {
    let (release_tx, release_rx) = oneshot::channel();
    let gate = Arc::new(Gate {
        rows,
        page_rows,
        tail,
        release: Mutex::new(Some(release_rx)),
        kill: Notify::new(),
        killed: AtomicBool::new(false),
    });
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind loopback MySQL mock");
    let port = listener.local_addr().expect("mock local addr").port();
    let accept_gate = gate.clone();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, _peer)) => {
                        let gate = accept_gate.clone();
                        connections.spawn(async move { let _ = handle_conn(stream, gate).await; });
                    }
                    Err(_) => break,
                },
                _ = connections.join_next(), if !connections.is_empty() => {},
            }
        }
    });
    (port, server, gate, release_tx)
}

async fn connect_pool(port: u16) -> MySqlPool {
    let options = MySqlConnectOptions::new()
        .host("127.0.0.1")
        .port(port)
        .username("root")
        .ssl_mode(MySqlSslMode::Disabled);
    MySqlPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await
        .expect("loopback MySQL pool")
}

fn make_entry(
    page_rows: usize,
    sql: &str,
) -> (
    Registry,
    Arc<crate::execution::Entry>,
    Arc<StreamProgress>,
    Arc<crate::query_spool::Spool>,
) {
    let registry = Registry::default();
    let binding = Binding {
        principal: "main".into(),
        owner: "tab".into(),
        connection: "c".into(),
        namespace: "db".into(),
        fingerprint: "test".into(),
    };
    let mut request = ExecutionRequest {
        execution_id: String::new(),
        connection_id: "c".into(),
        namespace: "db".into(),
        owner_id: "tab".into(),
        sql: sql.into(),
        transaction_owner: None,
        timeout_ms: Some(0),
        origin: Some("query".into()),
        execution_grant: None,
        cancel_before_send: false,
    };
    registry
        .grant_request(&mut request, binding.clone())
        .expect("execution grant");
    let entry = registry
        .register(&request, binding, 0)
        .expect("register execution");
    let progress = StreamProgress::new(StreamOptions { page_rows });
    entry.attach_stream(progress.clone());
    let spool = crate::query_spool::create("c", "tab", "db", "main");
    // Streaming marks the spool provisional until `Entry::finish` verifies it.
    entry.attach_spool(spool.clone());
    (registry, entry, progress, spool)
}

fn spawn_worker(
    entry: Arc<crate::execution::Entry>,
    pool: MySqlPool,
) -> tokio::task::JoinHandle<crate::execution_contracts::ExecutionOutcome> {
    tokio::spawn(async move {
        let db = Db::Mysql(pool);
        let kind = DbKind::of("mysql");
        let (result, confirmed) = crate::execution::run(&db, &kind, SQL, &entry, false).await;
        entry.finish(result, confirmed, false)
    })
}

#[tokio::test]
async fn stream_first_page_then_full_spool_last_row_is_exact() {
    let (port, server, _gate, release_tx) =
        start_mock(TOTAL_ROWS, PAGE_ROWS, Tail::Complete).await;
    let pool = connect_pool(port).await;
    let (_registry, entry, progress, spool) = make_entry(PAGE_ROWS, SQL);
    let worker = spawn_worker(entry.clone(), pool.clone());

    let first = tokio::time::timeout(Duration::from_secs(30), progress.wait_first(&entry))
        .await
        .expect("preview did not arrive before the timeout");
    assert_eq!(first.status.state, ExecutionState::Running);
    assert!(first.outcome.is_none(), "preview must arrive before the receipt");
    let preview = first.preview.expect("preview at the page boundary");
    assert!(preview.provisional);
    assert_eq!(preview.rows.len(), PAGE_ROWS);
    assert_eq!(preview.observed_rows, PAGE_ROWS as u64);
    assert_eq!(
        preview.rows[0],
        vec![Some("0".into()), Some("row0".into())]
    );
    assert!(preview.spool_id.is_some(), "preview names the same spool");

    release_tx.send(()).expect("release the mock tail");
    let outcome = tokio::time::timeout(Duration::from_secs(60), worker)
        .await
        .expect("streaming worker did not settle")
        .expect("streaming worker panicked");
    assert_eq!(outcome.state, ExecutionState::Succeeded, "{:?}", outcome.error);
    assert!(!outcome.resources_pending);

    let snapshot = progress.snapshot(&entry);
    assert!(snapshot.preview.is_none());
    let receipt = snapshot.outcome.expect("final receipt");
    assert!(
        receipt.result.expect("receipt result").sets[0].rows.is_empty(),
        "the published receipt must not retain preview rows"
    );

    let set = spool.set(0).expect("spool set");
    set.ensure_complete().expect("verified complete spool");
    let last = set
        .fetch((TOTAL_ROWS - 1) as u64, 1)
        .expect("last spool page");
    assert_eq!(
        last,
        vec![vec![
            Some((TOTAL_ROWS - 1).to_string()),
            Some(format!("row{}", TOTAL_ROWS - 1)),
        ]]
    );

    crate::query_spool::drop_spool(&spool.id);
    pool.close().await;
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn stream_late_server_error_fails_and_revokes_spool() {
    let (port, server, _gate, release_tx) = start_mock(TOTAL_ROWS, PAGE_ROWS, Tail::Error).await;
    let pool = connect_pool(port).await;
    let (_registry, entry, progress, spool) = make_entry(PAGE_ROWS, SQL);
    let worker = spawn_worker(entry.clone(), pool.clone());

    let first = tokio::time::timeout(Duration::from_secs(30), progress.wait_first(&entry))
        .await
        .expect("preview did not arrive before the timeout");
    assert!(first.preview.is_some(), "preview before the tail error");

    release_tx.send(()).expect("release the failing tail");
    let outcome = tokio::time::timeout(Duration::from_secs(60), worker)
        .await
        .expect("failing worker did not settle")
        .expect("failing worker panicked");
    assert_eq!(outcome.state, ExecutionState::Failed, "{:?}", outcome.error);
    assert!(!outcome.resources_pending);

    let snapshot = progress.snapshot(&entry);
    assert!(snapshot.preview.is_none(), "failure revokes the provisional preview");
    assert!(snapshot.outcome.is_some());
    assert!(
        !crate::query_spool::exists(&spool.id),
        "failed execution must drop the spool"
    );

    let set = spool.set(0).expect("set is still reachable through this arc");
    assert!(set.ensure_complete().is_err(), "failed spool cannot be exported");
    assert!(set.fetch(0, 1).is_err(), "failed spool cannot be paged");

    pool.close().await;
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn stream_cancel_after_preview_uses_targeted_kill_query() {
    let (port, server, _gate, _release_tx) =
        start_mock(TOTAL_ROWS, PAGE_ROWS, Tail::Complete).await;
    let pool = connect_pool(port).await;
    let (_registry, entry, progress, spool) = make_entry(PAGE_ROWS, SQL);
    let worker = spawn_worker(entry.clone(), pool.clone());

    let first = tokio::time::timeout(Duration::from_secs(30), progress.wait_first(&entry))
        .await
        .expect("preview did not arrive before the timeout");
    assert!(first.preview.is_some(), "preview before the targeted stop");

    entry.request_stop(false);
    let outcome = tokio::time::timeout(Duration::from_secs(60), worker)
        .await
        .expect("cancelled worker did not settle")
        .expect("cancelled worker panicked");
    assert_eq!(outcome.state, ExecutionState::Cancelled, "{:?}", outcome.error);
    assert!(
        !crate::query_spool::exists(&spool.id),
        "cancelled execution must drop the spool"
    );
    assert!(progress.snapshot(&entry).preview.is_none());

    pool.close().await;
    server.abort();
    let _ = server.await;
}

/// Opt-in live check against a real MySQL, e.g.
/// `NAVKIT_EXECUTION_MYSQL_URL='mysql://root@127.0.0.1:3306/db'`.
/// Skipped unless explicitly run with --ignored and a dedicated MySQL URL.
/// Only SELECT 1 is sent; this is not a large-query performance acceptance test.
#[tokio::test]
#[ignore = "requires dedicated NAVKIT_EXECUTION_MYSQL_URL; never counted as offline coverage"]
async fn live_mysql_streaming_is_env_gated_and_read_only() {
    let url = std::env::var("NAVKIT_EXECUTION_MYSQL_URL").expect("dedicated NAVKIT_EXECUTION_MYSQL_URL is required");
    let options: MySqlConnectOptions = url
        .parse()
        .expect("NAVKIT_EXECUTION_MYSQL_URL must be a valid MySQL URL");
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("live MySQL pool");
    let sql = "SELECT 1";
    let (_registry, entry, progress, spool) = make_entry(PAGE_ROWS, sql);
    let db = Db::Mysql(pool.clone());
    let kind = DbKind::of("mysql");
    let (result, confirmed) = crate::execution::run(&db, &kind, sql, &entry, false).await;
    entry.finish(result, confirmed, false);
    let snapshot = tokio::time::timeout(Duration::from_secs(30), progress.wait_first(&entry))
        .await
        .expect("live stream did not settle");
    let settled = snapshot.outcome.or_else(|| progress.snapshot(&entry).outcome);
    assert!(
        settled.is_some(),
        "live streaming must reach a terminal receipt, not a provisional preview"
    );
    crate::query_spool::drop_spool(&spool.id);
    pool.close().await;
}
