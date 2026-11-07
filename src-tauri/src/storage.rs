//! 便携存储：所有配置写入 exe 同目录的 ./data/，密码使用 AES-256-GCM 加密。
//! 绝写入 AppData 或注册表，整个文件夹拷走即用。

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};
use zeroize::Zeroizing;

use crate::models::{AppSettings, ConnectionConfig, ConnectionMovePosition, Favorite, RecentItem};

/// 便携目录根（exe 所在目录）。
pub fn portable_root() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.to_path_buf();
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// ./data 目录。
pub fn data_dir() -> PathBuf {
    let dir = portable_root().join("data");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// ./data/logs 目录。
pub fn log_dir() -> PathBuf {
    let dir = data_dir().join("logs");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// ./data/cache 目录。
#[allow(dead_code)]
pub fn cache_dir() -> PathBuf {
    let dir = data_dir().join("cache");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// ./data/config.json 路径。
pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

/// ./data/connections.json 路径。
pub fn connections_path() -> PathBuf {
    data_dir().join("connections.json")
}

/// ./data/favorites.json 路径。
pub fn favorites_path() -> PathBuf {
    data_dir().join("favorites.json")
}

/// ./data/recent.json 路径。
pub fn recent_path() -> PathBuf {
    data_dir().join("recent.json")
}

/// 过滤文件 / 目录名中的非法字符（Windows 保留字符）。
fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "_".into()
    } else {
        cleaned
    }
}

/// ./data/jobs.json 路径（自动运行任务）。
pub fn jobs_path() -> PathBuf {
    data_dir().join("jobs.json")
}

/// ./data/job_history.json 路径（自动运行历史）。
pub fn job_history_path() -> PathBuf {
    data_dir().join("job_history.json")
}

/// ./data/backups/<连接>/<数据库> 目录（备份文件存放处）。
pub fn backup_dir(conn_id: &str, schema: &str) -> PathBuf {
    let dir = data_dir()
        .join("backups")
        .join(sanitize_name(conn_id))
        .join(sanitize_name(schema));
    let _ = fs::create_dir_all(&dir);
    dir
}

// ---------------------------------------------------------------- 加密

static KEY_LOCK: Mutex<()> = Mutex::new(());

/// 解密时绝不生成密钥；损坏的密钥也绝不被随机新密钥覆盖。
fn master_key_at(dir: &Path, create: bool) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let _guard = KEY_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("本机密钥锁不可用"))?;
    let path = dir.join("key.bin");
    match read_bytes_limited(&path, 32) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut key = Zeroizing::new([0u8; 32]);
            key.copy_from_slice(&bytes);
            Ok(key)
        }
        Ok(_) => anyhow::bail!("本机加密密钥损坏，已拒绝覆盖密钥"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            if dir.join("datasource_sync.enc").exists() {
                anyhow::bail!("已有加密同步状态但本机密钥缺失，已拒绝生成替代密钥");
            }
            fs::create_dir_all(dir)?;
            let mut key = Zeroizing::new([0u8; 32]);
            rand::thread_rng().fill_bytes(key.as_mut());
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(key.as_ref())?;
            file.sync_all()?;
            Ok(key)
        }
        Err(_) => {
            anyhow::bail!("本机加密密钥缺失、损坏或不可读，请恢复原 key.bin 或重新输入连接密码")
        }
    }
}

/// 本机密文与旧密码格式兼容；同步状态通过独立 AAD 隔离用途。
pub(crate) fn encrypt_bytes_at(dir: &Path, plain: &[u8], aad: &[u8]) -> anyhow::Result<Vec<u8>> {
    let key = master_key_at(dir, true)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref())
        .map_err(|_| anyhow::anyhow!("本机加密初始化失败"))?;
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
        .map_err(|_| anyhow::anyhow!("本机加密失败"))?;
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ct);
    Ok(blob)
}

pub(crate) fn decrypt_bytes_at(
    dir: &Path,
    blob: &[u8],
    aad: &[u8],
) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    if blob.len() < 28 {
        anyhow::bail!("本机密文损坏，已拒绝解密");
    }
    let key = master_key_at(dir, false)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref())
        .map_err(|_| anyhow::anyhow!("本机解密初始化失败"))?;
    let plain = cipher
        .decrypt(
            Nonce::from_slice(&blob[..12]),
            Payload {
                msg: &blob[12..],
                aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("本机密码或同步状态解密失败，密文损坏或密钥不匹配"))?;
    Ok(Zeroizing::new(plain))
}

fn encrypt_password_at(dir: &Path, plain: &str) -> anyhow::Result<String> {
    if plain.is_empty() {
        return Ok(String::new());
    }
    Ok(B64.encode(encrypt_bytes_at(dir, plain.as_bytes(), b"")?))
}

fn decrypt_password_at(dir: &Path, encoded: &str) -> anyhow::Result<String> {
    if encoded.is_empty() {
        return Ok(String::new());
    }
    if encoded.len() > 1024 * 1024 {
        anyhow::bail!("本机连接密码密文过大");
    }
    let blob = B64
        .decode(encoded)
        .map_err(|_| anyhow::anyhow!("本机连接密码密文损坏"))?;
    let plain = decrypt_bytes_at(dir, &blob, b"")?;
    String::from_utf8(plain.to_vec()).map_err(|_| anyhow::anyhow!("本机连接密码不是有效 UTF-8"))
}

pub fn encrypt_password(plain: &str) -> anyhow::Result<String> {
    encrypt_password_at(&data_dir(), plain)
}

/// 空密码与解密失败明确区分，调用者必须检查 Result。
pub fn decrypt_password(encoded: &str) -> anyhow::Result<String> {
    decrypt_password_at(&data_dir(), encoded)
}

// ---------------------------------------------------------------- 持久化

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct ConnectionsFile {
    #[serde(default)]
    connections: Vec<ConnectionConfig>,
    #[serde(default)]
    groups: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FavoritesFile {
    #[serde(default)]
    items: Vec<Favorite>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RecentFile {
    #[serde(default)]
    items: Vec<RecentItem>,
}

/// 全局存储句柄。
pub struct Store {
    settings: Mutex<AppSettings>,
    connection_state: Mutex<ConnectionsState>,
    connection_gate: Arc<RwLock<()>>,
    root: PathBuf,
    favorites: Mutex<Vec<Favorite>>,
    recent: Mutex<Vec<RecentItem>>,
}

#[derive(Default)]
struct ConnectionsState {
    file: ConnectionsFile,
    revision: u64,
    disk_hash: Option<[u8; 32]>,
    load_error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ConnectionSnapshot {
    pub connections: Vec<ConnectionConfig>,
    pub groups: Vec<String>,
    pub revision: u64,
}

/// 可跨 await 持有，冻结所有连接/分组/排序写入，防止远端 PUT 后丢失本地编辑。
pub(crate) struct ConnectionReservation {
    _gate: OwnedRwLockWriteGuard<()>,
    revision: u64,
}

impl Store {
    /// 从 ./data 载入全部配置。
    pub fn load() -> Self {
        Self::load_at(&data_dir())
    }

    pub fn load_at(root: &Path) -> Self {
        let settings = read_json::<AppSettings>(&root.join("config.json")).unwrap_or_default();
        let mut connection_state = ConnectionsState::default();
        match read_bytes_limited(&root.join("connections.json"), 16 * 1024 * 1024) {
            Ok(bytes) => {
                connection_state.disk_hash = Some(Sha256::digest(&bytes).into());
                match serde_json::from_slice(&bytes) {
                    Ok(file) => connection_state.file = file,
                    Err(_) => {
                        connection_state.load_error =
                            Some("连接配置文件损坏，已拒绝同步或覆盖".into())
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                connection_state.load_error = Some("连接配置文件不可读，已拒绝同步或覆盖".into())
            }
        }
        let ff = read_json::<FavoritesFile>(&root.join("favorites.json")).unwrap_or_default();
        let rf = read_json::<RecentFile>(&root.join("recent.json")).unwrap_or_default();
        Store {
            settings: Mutex::new(settings),
            connection_state: Mutex::new(connection_state),
            connection_gate: Arc::new(RwLock::new(())),
            root: root.to_path_buf(),
            favorites: Mutex::new(ff.items),
            recent: Mutex::new(rf.items),
        }
    }

    #[cfg(test)]
    pub(crate) fn empty_at(root: &Path) -> Self {
        Store {
            settings: Mutex::new(AppSettings::default()),
            connection_state: Mutex::new(ConnectionsState::default()),
            connection_gate: Arc::new(RwLock::new(())),
            root: root.to_path_buf(),
            favorites: Mutex::new(vec![]),
            recent: Mutex::new(vec![]),
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn ensure_encryption_key(&self) -> anyhow::Result<()> {
        let encrypted = self.connections().iter().any(|c| !c.password.is_empty());
        let _key = master_key_at(&self.root, !encrypted)?;
        Ok(())
    }

    pub(crate) fn encrypt_password(&self, plain: &str) -> anyhow::Result<String> {
        // 已有密文时不能在丢失密钥后另造新密钥。
        if !plain.is_empty() && self.connections().iter().any(|c| !c.password.is_empty()) {
            let _key = master_key_at(&self.root, false)?;
        }
        encrypt_password_at(&self.root, plain)
    }

    pub(crate) fn decrypt_password(&self, encoded: &str) -> anyhow::Result<String> {
        decrypt_password_at(&self.root, encoded)
    }

    pub fn settings(&self) -> AppSettings {
        self.settings.lock().unwrap().clone()
    }

    pub fn set_settings(&self, s: AppSettings) {
        let mut g = self.settings.lock().unwrap();
        *g = s;
        write_json(&config_path(), &*g);
    }

    pub fn connections(&self) -> Vec<ConnectionConfig> {
        self.connection_state
            .lock()
            .unwrap()
            .file
            .connections
            .clone()
    }

    pub fn groups(&self) -> Vec<String> {
        self.connection_state.lock().unwrap().file.groups.clone()
    }

    pub fn save_connections(
        &self,
        conns: Vec<ConnectionConfig>,
        groups: Vec<String>,
    ) -> Result<(), String> {
        self.update_connections(move |c, g| {
            *c = conns;
            *g = groups;
            Ok(())
        })
    }

    /// 读、改、checked 写盘、更新内存属于同一临界区；调用者不能在锁外读改写。
    pub(crate) fn update_connections<R>(
        &self,
        edit: impl FnOnce(&mut Vec<ConnectionConfig>, &mut Vec<String>) -> Result<R, String>,
    ) -> Result<R, String> {
        self.update_connections_at(&self.root.join("connections.json"), edit)
    }

    fn update_connections_at<R>(
        &self,
        path: &Path,
        edit: impl FnOnce(&mut Vec<ConnectionConfig>, &mut Vec<String>) -> Result<R, String>,
    ) -> Result<R, String> {
        let _gate = self
            .connection_gate
            .clone()
            .try_read_owned()
            .map_err(|_| "数据源正在同步应用，请稍后重试编辑".to_string())?;
        let mut state = self
            .connection_state
            .lock()
            .map_err(|_| "连接存储锁不可用".to_string())?;
        self.check_disk(&state, path)?;
        let mut next = state.file.clone();
        let result = edit(&mut next.connections, &mut next.groups)?;
        if next != state.file || state.disk_hash.is_none() {
            self.write_connections_locked(&mut state, path, next)?;
        }
        Ok(result)
    }

    pub(crate) fn snapshot_checked(&self) -> Result<ConnectionSnapshot, String> {
        let state = self
            .connection_state
            .lock()
            .map_err(|_| "连接存储锁不可用".to_string())?;
        self.check_disk(&state, &self.root.join("connections.json"))?;
        Ok(ConnectionSnapshot {
            connections: state.file.connections.clone(),
            groups: state.file.groups.clone(),
            revision: state.revision,
        })
    }

    fn check_disk(&self, state: &ConnectionsState, path: &Path) -> Result<(), String> {
        if let Some(error) = &state.load_error {
            return Err(error.clone());
        }
        // 测试旧排序接口可指定独立临时文件；生产所有写入使用同一路径。
        if path == self.root.join("connections.json") {
            let hash = match read_bytes_limited(path, 16 * 1024 * 1024) {
                Ok(bytes) => Some(Sha256::digest(&bytes).into()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err("连接配置文件不可读，已拒绝覆盖".into()),
            };
            if hash != state.disk_hash {
                return Err("连接配置文件在外部改变，请重启载入后重新预览".into());
            }
        }
        Ok(())
    }

    fn write_connections_locked(
        &self,
        state: &mut ConnectionsState,
        path: &Path,
        next: ConnectionsFile,
    ) -> Result<(), String> {
        let revision = state
            .revision
            .checked_add(1)
            .ok_or("连接 revision 已耗尽")?;
        let bytes =
            serde_json::to_vec_pretty(&next).map_err(|_| "序列化连接配置失败".to_string())?;
        atomic_write(path, &bytes)
            .map_err(|_| "保存连接配置失败，内存与 revision 未改变".to_string())?;
        state.file = next;
        state.disk_hash = Some(Sha256::digest(&bytes).into());
        state.revision = revision;
        Ok(())
    }

    pub(crate) fn reserve_connections(
        &self,
        revision: u64,
    ) -> Result<ConnectionReservation, String> {
        let gate = self
            .connection_gate
            .clone()
            .try_write_owned()
            .map_err(|_| "连接配置正在编辑，请重新预览".to_string())?;
        let state = self
            .connection_state
            .lock()
            .map_err(|_| "连接存储锁不可用".to_string())?;
        self.check_disk(&state, &self.root.join("connections.json"))?;
        if state.revision != revision {
            return Err("预览后本机连接、分组或排序已改变，请重新预览".into());
        }
        Ok(ConnectionReservation {
            _gate: gate,
            revision,
        })
    }

    pub(crate) fn save_reserved(
        &self,
        reservation: &ConnectionReservation,
        conns: Vec<ConnectionConfig>,
        groups: Vec<String>,
    ) -> Result<(), String> {
        let mut state = self
            .connection_state
            .lock()
            .map_err(|_| "连接存储锁不可用".to_string())?;
        let path = self.root.join("connections.json");
        self.check_disk(&state, &path)?;
        if state.revision != reservation.revision {
            return Err("本机预览 revision 失效，请重新预览".into());
        }
        let next = ConnectionsFile {
            connections: conns,
            groups,
        };
        if next != state.file || state.disk_hash.is_none() {
            self.write_connections_locked(&mut state, &path, next)?;
        }
        Ok(())
    }

    /// connections.json 数组顺序就是同组连接顺序，旧文件无需迁移。
    pub fn move_connection(
        &self,
        id: &str,
        group: &str,
        target_id: Option<&str>,
        position: Option<ConnectionMovePosition>,
    ) -> Result<Vec<ConnectionConfig>, String> {
        self.move_connection_at(
            &self.root.join("connections.json"),
            id,
            group,
            target_id,
            position,
        )
    }

    fn move_connection_at(
        &self,
        path: &Path,
        id: &str,
        group: &str,
        target_id: Option<&str>,
        position: Option<ConnectionMovePosition>,
    ) -> Result<Vec<ConnectionConfig>, String> {
        self.update_connections_at(path, |connections, groups| {
            let next = reordered_connections(connections, groups, id, group, target_id, position)?;
            *connections = next.clone();
            Ok(next)
        })
    }

    pub fn favorites(&self) -> Vec<Favorite> {
        self.favorites.lock().unwrap().clone()
    }

    pub fn save_favorites(&self, items: Vec<Favorite>) {
        {
            let mut f = self.favorites.lock().unwrap();
            *f = items;
        }
        let items = self.favorites.lock().unwrap().clone();
        write_json(&favorites_path(), &FavoritesFile { items });
    }

    pub fn recent(&self) -> Vec<RecentItem> {
        self.recent.lock().unwrap().clone()
    }

    pub fn push_recent(&self, item: RecentItem, max: u32) {
        let mut list = self.recent.lock().unwrap();
        list.retain(|x| x.id != item.id);
        list.insert(0, item);
        let max = max.max(1) as usize;
        list.truncate(max);
        write_json(
            &recent_path(),
            &RecentFile {
                items: list.clone(),
            },
        );
    }

    pub fn clear_recent(&self) {
        let mut list = self.recent.lock().unwrap();
        list.clear();
        write_json(&recent_path(), &RecentFile { items: Vec::new() });
    }
}

/// 读取 JSON 文件。
pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 原子写入 JSON 文件。
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    let _ = write_json_checked(path, value);
}

/// 验证完整落点后移动一项；不改名、不改密码、不改其他连接的相对顺序。
fn reordered_connections(
    connections: &[ConnectionConfig],
    groups: &[String],
    id: &str,
    group: &str,
    target_id: Option<&str>,
    position: Option<ConnectionMovePosition>,
) -> Result<Vec<ConnectionConfig>, String> {
    if target_id.is_some() != position.is_some() {
        return Err("排序目标与位置必须同时指定".into());
    }
    let source_index = connections
        .iter()
        .position(|c| c.id == id)
        .ok_or("连接不存在")?;
    // 兼容旧文件：groups 未列出但已有连接使用的分组也是有效文件夹。
    if !group.is_empty()
        && !groups.iter().any(|g| g == group)
        && !connections.iter().any(|c| c.group == group)
    {
        return Err("目标分组不存在".into());
    }
    if let Some(target_id) = target_id {
        if target_id == id {
            return Err("不能将连接拖到自身".into());
        }
        let target = connections
            .iter()
            .find(|c| c.id == target_id)
            .ok_or("排序目标连接不存在")?;
        if target.group != group {
            return Err("排序目标不属于目标分组".into());
        }
    }
    let mut next = connections.to_vec();
    let mut source = next.remove(source_index);
    source.group = group.into();
    let index = if let Some(target_id) = target_id {
        let index = next
            .iter()
            .position(|c| c.id == target_id)
            .ok_or("排序目标连接不存在")?;
        index + usize::from(matches!(position, Some(ConnectionMovePosition::After)))
    } else {
        next.iter()
            .rposition(|c| c.group == group)
            .map(|index| index + 1)
            .unwrap_or(if group.is_empty() { 0 } else { next.len() })
    };
    next.insert(index, source);
    Ok(next)
}

/// 树移动必须收到写盘失败，不能复用忽略 IO 错误的旧设置写入接口。
fn write_json_checked<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let text = serde_json::to_vec_pretty(value)?;
    atomic_write(path, &text)
}

/// 独占临时文件 + fsync + 同目录原子替换；失败时原文件保留。
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "文件缺少父目录"))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".navkit-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub(crate) fn read_bytes_limited(path: &Path, limit: usize) -> std::io::Result<Zeroizing<Vec<u8>>> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > limit as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "文件过大",
        ));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "文件过大",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tree_order_tests {
    use super::*;
    use crate::models::DbKind;

    fn config(id: &str, group: &str) -> ConnectionConfig {
        ConnectionConfig {
            id: id.into(),
            name: format!("name-{}", id),
            kind: DbKind::of("sqlite"),
            host: String::new(),
            port: 0,
            user: String::new(),
            password: "encrypted-untouched".into(),
            database: String::new(),
            file: "file.sqlite".into(),
            params: String::new(),
            group: group.into(),
            read_only: true,
            keep_alive: true,
        }
    }
    fn store() -> Store {
        let state = Store::empty_at(
            &std::env::temp_dir().join(format!("navkit-tree-fixture-{}", uuid::Uuid::new_v4())),
        );
        state.connection_state.lock().unwrap().file = ConnectionsFile {
            connections: vec![
                config("a", ""),
                config("b", ""),
                config("c", "folder"),
                config("d", "folder"),
            ],
            groups: vec!["folder".into(), "empty".into()],
        };
        state
    }
    fn ids(connections: &[ConnectionConfig], group: &str) -> Vec<String> {
        connections
            .iter()
            .filter(|c| c.group == group)
            .map(|c| c.id.clone())
            .collect()
    }

    #[test]
    fn move_position_wire_values_reject_unknown_positions() {
        assert_eq!(
            serde_json::to_string(&ConnectionMovePosition::Before).unwrap(),
            "\"before\""
        );
        assert_eq!(
            serde_json::to_string(&ConnectionMovePosition::After).unwrap(),
            "\"after\""
        );
        assert!(matches!(
            serde_json::from_str::<ConnectionMovePosition>("\"before\"").unwrap(),
            ConnectionMovePosition::Before
        ));
        assert!(serde_json::from_str::<ConnectionMovePosition>("\"invalid\"").is_err());
    }

    #[test]
    fn same_group_before_after_and_cross_group_root_append() {
        let state = store();
        let connections = state.connections();
        let groups = state.groups();
        let next = reordered_connections(
            &connections,
            &groups,
            "a",
            "",
            Some("b"),
            Some(ConnectionMovePosition::After),
        )
        .unwrap();
        assert_eq!(ids(&next, ""), vec!["b", "a"]);
        let next = reordered_connections(
            &next,
            &groups,
            "a",
            "folder",
            Some("d"),
            Some(ConnectionMovePosition::Before),
        )
        .unwrap();
        assert_eq!(ids(&next, "folder"), vec!["c", "a", "d"]);
        let moved = next.iter().find(|c| c.id == "a").unwrap();
        assert_eq!(moved.password, "encrypted-untouched");
        assert!(moved.read_only && moved.keep_alive);
        let next = reordered_connections(&next, &groups, "c", "", None, None).unwrap();
        assert_eq!(ids(&next, ""), vec!["b", "c"]);
        let next = reordered_connections(&next, &groups, "b", "empty", None, None).unwrap();
        assert_eq!(ids(&next, "empty"), vec!["b"]);
        assert_eq!(ids(&next, "folder"), vec!["a", "d"]);
    }

    #[test]
    fn rejects_missing_self_wrong_group_and_incomplete_targets() {
        let state = store();
        let connections = state.connections();
        let groups = state.groups();
        for (id, group, target, position) in [
            ("missing", "", None, None),
            ("a", "missing", None, None),
            ("a", "", Some("a"), Some(ConnectionMovePosition::Before)),
            ("a", "", Some("c"), Some(ConnectionMovePosition::Before)),
            (
                "a",
                "",
                Some("missing"),
                Some(ConnectionMovePosition::After),
            ),
            ("a", "", Some("b"), None),
            ("a", "", None, Some(ConnectionMovePosition::After)),
        ] {
            assert!(
                reordered_connections(&connections, &groups, id, group, target, position).is_err()
            );
        }
        assert_eq!(ids(&state.connections(), ""), vec!["a", "b"]);
        // 旧连接文件中的隐式分组继续可用。
        assert!(reordered_connections(&connections, &[], "a", "folder", None, None).is_ok());
    }

    #[test]
    fn roundtrip_persists_order_and_write_failure_does_not_commit_memory() {
        let root = std::env::temp_dir().join(format!("navkit-tree-order-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.join("connections.json");
        let state = store();
        state
            .move_connection_at(
                &path,
                "b",
                "folder",
                Some("c"),
                Some(ConnectionMovePosition::Before),
            )
            .unwrap();
        // 第二次写到同一文件验证 Windows 原子替换，而不只是首次创建。
        state
            .move_connection_at(
                &path,
                "d",
                "folder",
                Some("b"),
                Some(ConnectionMovePosition::Before),
            )
            .unwrap();
        let loaded: ConnectionsFile = read_json(&path).unwrap();
        assert_eq!(ids(&loaded.connections, "folder"), vec!["d", "b", "c"]);
        assert!(loaded.groups.contains(&"empty".into()));
        let before = serde_json::to_string(&state.connections()).unwrap();
        let (before_revision, before_disk_hash) = {
            let locked = state.connection_state.lock().unwrap();
            (locked.revision, locked.disk_hash.clone())
        };
        let blocked = root.join("not-a-directory");
        fs::write(&blocked, "blocked").unwrap();
        let error = state
            .move_connection_at(&blocked.join("connections.json"), "a", "folder", None, None)
            .unwrap_err();
        assert_eq!(error, "保存连接配置失败，内存与 revision 未改变");
        assert_eq!(serde_json::to_string(&state.connections()).unwrap(), before);
        {
            let locked = state.connection_state.lock().unwrap();
            assert_eq!(locked.revision, before_revision);
            assert_eq!(locked.disk_hash, before_disk_hash);
        }
        assert_eq!(
            ids(
                &read_json::<ConnectionsFile>(&path).unwrap().connections,
                "folder"
            ),
            vec!["d", "b", "c"]
        );
        fs::remove_dir_all(root).unwrap();
    }
}

/// 追加写入日志行。
pub fn log_line(tag: &str, message: &str) {
    use std::io::Write;
    let path = log_dir().join("navkit.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{}] {} {}", now, tag, message);
    }
}

/// SQL 执行日志保留上限：最近 2000 条。
pub const SQL_LOG_MAX_ENTRIES: usize = 2000;
/// SQL 执行日志保留窗口：30 天（先到为准，超限自动清理）。
pub const SQL_LOG_MAX_AGE_DAYS: i64 = 30;
/// 文件超过 8MB 时触发压缩重写。
const SQL_LOG_COMPACT_BYTES: u64 = 8 * 1024 * 1024;

/// ./data/logs/sql_log.jsonl 路径（SQL 执行日志）。
pub fn sql_log_path() -> PathBuf {
    log_dir().join("sql_log.jsonl")
}

fn sql_log_cutoff() -> String {
    (chrono::Local::now() - chrono::Duration::days(SQL_LOG_MAX_AGE_DAYS))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// 压缩 SQL 执行日志：仅保留 30 天内最近 2000 条。失败时保留原文件，不影响主流程。
fn compact_sql_log_at(path: &Path) {
    let Ok(content) = fs::read_to_string(path) else { return };
    let cutoff = sql_log_cutoff();
    let mut entries: Vec<crate::models::SqlLogEntry> = content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|e: &crate::models::SqlLogEntry| e.ts.as_str() >= cutoff.as_str())
        .collect();
    if entries.len() > SQL_LOG_MAX_ENTRIES {
        entries.drain(..entries.len() - SQL_LOG_MAX_ENTRIES);
    }
    let mut buf = String::new();
    for entry in &entries {
        if let Ok(line) = serde_json::to_string(entry) {
            buf.push_str(&line);
            buf.push('\n');
        }
    }
    if buf.len() < content.len() {
        let _ = fs::write(path, buf);
    }
}

/// 追加一条 SQL 执行日志；文件超过 8MB 时先压缩。
pub fn append_sql_log_at(path: &Path, entry: &crate::models::SqlLogEntry) {
    if fs::metadata(path).map(|m| m.len()).unwrap_or(0) > SQL_LOG_COMPACT_BYTES {
        compact_sql_log_at(path);
    }
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        if let Ok(line) = serde_json::to_string(entry) {
            let _ = writeln!(f, "{}", line);
        }
    }
}

/// 读取 SQL 执行日志（最新优先，最多 limit 条，过滤 30 天前的旧记录）。
pub fn read_sql_log_at(path: &Path, limit: usize) -> Vec<crate::models::SqlLogEntry> {
    let Ok(content) = fs::read_to_string(path) else { return Vec::new() };
    let cutoff = sql_log_cutoff();
    content
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<crate::models::SqlLogEntry>(line).ok())
        .filter(|e| e.ts.as_str() >= cutoff.as_str())
        .take(limit)
        .collect()
}

/// 清空 SQL 执行日志。
pub fn clear_sql_log_at(path: &Path) {
    let _ = fs::remove_file(path);
}

/// 追加一条 SQL 执行日志（便携 data 目录）。
pub fn append_sql_log(entry: &crate::models::SqlLogEntry) {
    append_sql_log_at(&sql_log_path(), entry);
}

/// 读取 SQL 执行日志（便携 data 目录，最新优先）。
pub fn read_sql_log(limit: usize) -> Vec<crate::models::SqlLogEntry> {
    read_sql_log_at(&sql_log_path(), limit)
}

/// 清空 SQL 执行日志（便携 data 目录）。
pub fn clear_sql_log() {
    clear_sql_log_at(&sql_log_path())
}

/// 按扩展名过滤文件的辅助函数（导入导出向导用）。
#[allow(dead_code)]
pub fn list_files(dir: &Path, exts: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let name = p.to_string_lossy().to_string();
            let lower = name.to_ascii_lowercase();
            if exts.is_empty() || exts.iter().any(|x| lower.ends_with(x)) {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

/// 从连接配置构造显示名到 id 的映射（自动补全用）。
#[allow(dead_code)]
pub fn connection_name_map(conns: &[ConnectionConfig]) -> HashMap<String, String> {
    conns
        .iter()
        .map(|c| (c.name.clone(), c.id.clone()))
        .collect()
}

#[cfg(test)]
mod sql_log_tests {
    use super::*;
    use crate::models::SqlLogEntry;

    fn entry(ts: &str, sql: &str) -> SqlLogEntry {
        SqlLogEntry {
            ts: ts.into(), connection_id: "c1".into(), connection_name: "本地 MySQL".into(),
            kind: "mysql".into(), database: "shop".into(), sql: sql.into(), ok: true,
            elapsed_ms: 42, rows: 3, message: String::new(), origin: "query".into(),
        }
    }

    #[test]
    fn append_read_and_age_filter() {
        let path = std::env::temp_dir().join(format!("navkit-sql-log-{}.jsonl", uuid::Uuid::new_v4()));
        append_sql_log_at(&path, &entry("2099-01-01 10:00:00", "SELECT 1"));
        append_sql_log_at(&path, &entry("2099-01-02 10:00:00", "SELECT 2"));
        // 30 天窗口之外的旧记录被过滤（2099 是未来时间，不会过期；这里验证解析与最新优先）
        let log = read_sql_log_at(&path, 10);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].sql, "SELECT 2");
        // limit 生效
        assert_eq!(read_sql_log_at(&path, 1).len(), 1);
        clear_sql_log_at(&path);
        assert!(read_sql_log_at(&path, 10).is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn compact_keeps_last_entries_and_drops_garbage_lines() {
        let path = std::env::temp_dir().join(format!("navkit-sql-log-{}.jsonl", uuid::Uuid::new_v4()));
        let mut content = String::from("this-is-not-json\n");
        // 100 条过期记录 + 2100 条新记录；先按 30 天窗口过滤，再按 2000 条上限保留最新
        let old_base = (chrono::Local::now() - chrono::Duration::days(SQL_LOG_MAX_AGE_DAYS + 10)).format("%Y-%m-%d %H:%M:%S").to_string();
        let new_base = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        for i in 0..100 { content.push_str(&serde_json::to_string(&entry(&old_base, &format!("OLD {i}"))).unwrap()); content.push('\n'); }
        for i in 0..2100 { content.push_str(&serde_json::to_string(&entry(&new_base, &format!("NEW {i}"))).unwrap()); content.push('\n'); }
        std::fs::write(&path, content).unwrap();
        compact_sql_log_at(&path);
        let log = read_sql_log_at(&path, usize::MAX);
        // 过期记录全部清理；条数上限触发，仅保留最新 2000 条
        assert_eq!(log.len(), SQL_LOG_MAX_ENTRIES);
        assert!(log[0].sql.starts_with("NEW 2099"));
        assert!(!log.iter().any(|e| e.sql.starts_with("OLD")));
        assert!(log.iter().any(|e| e.sql == "NEW 2099"));
        std::fs::remove_file(&path).unwrap();
    }
}
