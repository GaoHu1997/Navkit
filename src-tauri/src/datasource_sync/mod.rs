//! 数据源配置同步：只包含 ConnectionConfig、分组、顺序与删除记录。
//! 所有秘密及预览快照均留在 Rust；本机状态与恢复记录使用本机 AES 密钥。
mod crypto;
mod merge;
mod webdav;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::State;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    commands::AppState,
    db,
    error::{AppError, CmdResult},
    models::ConnectionConfig,
    storage::{self, Store},
};
use merge::{Merged, Snapshot};
use webdav::{Cooldown, Dav, RemoteRead};

const PLAN_TTL: Duration = Duration::from_secs(180);
const LOCAL_AAD: &[u8] = b"navkit/datasource-sync/local-state/1";
const LOCAL_MAGIC: &[u8] = b"NAVKIT-SYNC-LOCAL\0\x01";
const MAX_LOCAL_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Serialize)]
pub struct ConfigView {
    pub url: String,
    pub user: String,
    pub auto_every: i32,
    pub remember_password: bool,
    pub password_set: bool,
    pub passphrase_set: bool,
    pub headers_set: bool,
    pub last_sync: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigInput {
    pub url: String,
    pub user: String,
    pub auto_every: i32,
    pub remember_password: bool,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub headers: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct Conflict {
    pub key: String,
    pub label: String,
    pub local_summary: String,
    pub remote_summary: String,
}

#[derive(Clone, Serialize)]
pub struct Summary {
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub organization: bool,
}

#[derive(Clone, Serialize)]
pub struct Preview {
    pub plan_id: String,
    pub conflicts: Vec<Conflict>,
    pub summary: Summary,
    pub warnings: Vec<String>,
    pub remote_exists: bool,
    pub has_changes: bool,
    pub requires_confirmation: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    Local,
    Remote,
}

#[derive(Serialize)]
pub struct SyncResult {
    pub connections: Vec<ConnectionConfig>,
    pub groups: Vec<String>,
    pub changed_ids: Vec<String>,
    pub deleted_ids: Vec<String>,
    pub warnings: Vec<String>,
    pub last_sync: String,
    pub recovery_required: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretConfig {
    url: String,
    user: String,
    auto_every: i32,
    remember_password: bool,
    password: String,
    passphrase: String,
    headers: String,
    last_sync: Option<String>,
}

impl Default for SecretConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            user: String::new(),
            auto_every: -1,
            remember_password: false,
            password: String::new(),
            passphrase: String::new(),
            headers: String::new(),
            last_sync: None,
        }
    }
}

impl Drop for SecretConfig {
    fn drop(&mut self) {
        self.password.zeroize();
        self.passphrase.zeroize();
        self.headers.zeroize();
    }
}

impl SecretConfig {
    fn view(&self) -> ConfigView {
        ConfigView {
            url: self.url.clone(),
            user: self.user.clone(),
            auto_every: self.auto_every,
            remember_password: self.remember_password,
            password_set: !self.password.is_empty(),
            passphrase_set: !self.passphrase.is_empty(),
            headers_set: !self.headers.trim().is_empty(),
            last_sync: self.last_sync.clone(),
        }
    }
    fn identity(&self) -> Result<String, String> {
        webdav::identity_fingerprint(&self.url, &self.user, &self.password, &self.headers)
    }
    fn validate_sync(&self) -> Result<(), String> {
        if self.url.is_empty() {
            return Err("请先配置 WebDAV 地址".into());
        }
        if self.passphrase.trim().chars().count() < 8 {
            return Err("同步需要至少 8 个字符的独立强口令；测试连接无需口令".into());
        }
        if self.passphrase == self.password {
            return Err("同步口令不能与 WebDAV 密码相同".into());
        }
        // 显式认证头覆盖 Basic 时，它也属于服务器已知凭据，不能复用为同步口令。
        let headers = webdav::parse_headers(&self.headers)?;
        if headers
            .values()
            .any(|v| v.to_str().ok() == Some(self.passphrase.as_str()))
        {
            return Err("同步口令不能复用 WebDAV 请求头中的凭据".into());
        }
        if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
            let mut parts = auth.split_ascii_whitespace();
            let scheme = parts.next().unwrap_or("");
            let token = parts.next().unwrap_or("");
            if token == self.passphrase {
                return Err("同步口令不能复用 WebDAV 认证头中的凭据".into());
            }
            if scheme.eq_ignore_ascii_case("basic") {
                use base64::Engine;
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(token) {
                    let bytes = Zeroizing::new(bytes);
                    if std::str::from_utf8(&bytes)
                        .ok()
                        .and_then(|s| s.split_once(':'))
                        .is_some_and(|(_, password)| password == self.passphrase)
                    {
                        return Err("同步口令不能与 WebDAV Basic 认证头中的密码相同".into());
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery {
    #[serde(default, alias = "endpoint")]
    identity: String,
    original_local: Snapshot,
    target: Snapshot,
    written_hash: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Persisted {
    format: String,
    version: u32,
    config: SecretConfig,
    #[serde(default)]
    identity: Option<String>,
    baseline: Option<Snapshot>,
    pending: Option<Recovery>,
}

impl Default for Persisted {
    fn default() -> Self {
        Self {
            format: "navkit-datasource-sync-local".into(),
            version: 1,
            config: SecretConfig::default(),
            identity: None,
            baseline: None,
            pending: None,
        }
    }
}

struct Plan {
    view: Preview,
    created: Instant,
    generation: u64,
    revision: u64,
    dependency_warnings: Vec<String>,
    local: Snapshot,
    remote: Snapshot,
    baseline: Option<Snapshot>,
    observed: RemoteRead,
    recovery: bool,
}

struct Runtime {
    stored: Persisted,
    plan: Option<Plan>,
    generation: u64,
    load_error: Option<String>,
}

pub struct Service {
    inner: Arc<Mutex<Runtime>>,
    operation: tokio::sync::Mutex<()>,
    cooldown: Cooldown,
}

impl Service {
    pub fn new(store: &Store) -> Self {
        let (stored, load_error) = match load_state(store.root()) {
            Ok(state) => (state, None),
            Err(error) => (Persisted::default(), Some(error)),
        };
        Self {
            inner: Arc::new(Mutex::new(Runtime {
                stored,
                plan: None,
                generation: 0,
                load_error,
            })),
            operation: tokio::sync::Mutex::new(()),
            cooldown: Arc::new(Mutex::new(None)),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Runtime>, String> {
        self.inner.lock().map_err(|_| "同步状态锁不可用".into())
    }

    fn config(&self) -> Result<SecretConfig, String> {
        let mut runtime = self.lock()?;
        purge(&mut runtime);
        if let Some(error) = &runtime.load_error {
            return Err(error.clone());
        }
        Ok(runtime.stored.config.clone())
    }

    fn dav(&self, config: &SecretConfig) -> Result<Dav, String> {
        Dav::new(
            &config.url,
            &config.user,
            &config.password,
            &config.headers,
            self.cooldown.clone(),
        )
    }

    pub fn get_config(&self) -> Result<ConfigView, String> {
        self.config().map(|c| c.view())
    }

    pub fn set_config(&self, store: &Store, input: ConfigInput) -> Result<ConfigView, String> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| "数据源同步正在执行，请稍后修改配置")?;
        if !matches!(input.auto_every, -1 | 3 | 15 | 30 | 60) {
            return Err("自动同步间隔只能是 -1、3、15、30 或 60".into());
        }
        if input.user.len() > 512 || input.user.chars().any(char::is_control) {
            return Err("WebDAV 用户名无效".into());
        }
        if input.password.as_ref().is_some_and(|s| s.len() > 8192)
            || input.passphrase.as_ref().is_some_and(|s| s.len() > 8192)
        {
            return Err("WebDAV 密码或同步口令过长".into());
        }
        let url = webdav::normalize_url(&input.url)?;
        if let Some(headers) = &input.headers {
            webdav::parse_headers(headers)?;
        }
        store
            .ensure_encryption_key()
            .map_err(|_| "本机密钥不可用，已拒绝另建密钥或保存同步秘密")?;
        let mut runtime = self.lock()?;
        if let Some(error) = &runtime.load_error {
            return Err(error.clone());
        }
        let mut next = runtime.stored.clone();
        let old_identity = next.identity.clone();
        let old_url = next.config.url.clone();
        let endpoint_changed = url != old_url || input.user != next.config.user;
        if endpoint_changed {
            // Credentials are not silently carried to a different DAV account.
            next.config.password.zeroize();
            next.config.passphrase.zeroize();
            next.config.headers.zeroize();
        }
        next.config.url = url;
        next.config.user = input.user;
        next.config.auto_every = input.auto_every;
        next.config.remember_password = input.remember_password;
        if let Some(password) = input.password {
            next.config.password.zeroize();
            next.config.password = password;
        }
        if let Some(passphrase) = input.passphrase {
            next.config.passphrase.zeroize();
            next.config.passphrase = passphrase;
        }
        if let Some(headers) = input.headers {
            next.config.headers.zeroize();
            next.config.headers = headers.replace("\r\n", "\n").trim().to_string();
        }
        let new_identity = next.config.identity()?;
        let has_success_state = next.baseline.is_some()
            || next.pending.is_some()
            || next.config.last_sync.is_some();
        let identity_changed = match old_identity.as_deref() {
            Some(old) => old != new_identity,
            None => has_success_state || endpoint_changed,
        };
        if endpoint_changed || identity_changed {
            next.config.last_sync = None;
            next.baseline = None;
            next.pending = None;
        }
        next.identity = Some(new_identity.clone());
        persist_state(store.root(), &next)?;
        let view = next.config.view();
        runtime.stored = next;
        runtime.plan = None;
        runtime.generation = runtime.generation.wrapping_add(1);
        if endpoint_changed {
            *self.cooldown.lock().map_err(|_| "WebDAV 冷却锁不可用")? = None;
        }
        Ok(view)
    }

    pub async fn test(&self) -> Result<String, String> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| "数据源同步正在执行")?;
        let config = self.config()?;
        self.dav(&config)?.test().await
    }

    pub async fn preview(&self, store: &Store) -> Result<Preview, String> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| "数据源同步正在执行，请稍后预览")?;
        let (config, stored_identity, baseline, pending, generation) = {
            let mut runtime = self.lock()?;
            purge(&mut runtime);
            if let Some(error) = &runtime.load_error {
                return Err(error.clone());
            }
            runtime.plan = None;
            (
                runtime.stored.config.clone(),
                runtime.stored.identity.clone(),
                runtime.stored.baseline.clone(),
                runtime.stored.pending.clone(),
                runtime.generation,
            )
        };
        config.validate_sync()?;
        let current_identity = config.identity()?;
        let identity_matches = stored_identity.as_deref() == Some(current_identity.as_str());
        let has_unbound_state = stored_identity.is_none()
            && (baseline.is_some() || pending.is_some() || config.last_sync.is_some());
        let mut baseline = if identity_matches { baseline } else { None };
        let (local, revision) = Snapshot::capture(store, baseline.as_ref())?;
        let dav = self.dav(&config)?;
        let observed = dav.read().await?;
        let remote_exists = observed.bytes.is_some();
        let pending = if identity_matches && !has_unbound_state {
            pending
        } else {
            None
        };
        let remote = if let Some(bytes) = &observed.bytes {
            let bytes = bytes.clone();
            let passphrase = Zeroizing::new(config.passphrase.clone());
            tokio::task::spawn_blocking(move || crypto::open(&bytes, &passphrase))
                .await
                .map_err(|_| "同步解密任务失败")??
        } else {
            Snapshot::empty()
        };
        let mut warnings = vec![];
        if has_unbound_state {
            warnings.push("旧版同步状态缺少认证身份摘要，已隔离旧基线并按首次同步处理".into());
        } else if stored_identity.is_some() && !identity_matches {
            warnings.push("WebDAV 地址或有效认证身份已变化，旧基线与恢复记录已隔离；远端快照将要求手动确认".into());
        }
        let recovery = pending.is_some();
        if let Some(pending) = &pending {
            warnings.push(
                "上次同步尚未完成本机落盘或成功基线保存；保留原基线并重新合并恢复记录".into(),
            );
            if pending.identity != current_identity {
                return Err("同步恢复记录属于不同认证身份，已拒绝混用".into());
            }
            if observed
                .bytes
                .as_ref()
                .is_some_and(|b| crypto::hash(b) == pending.written_hash)
                && remote == pending.target
            {
                baseline = Some(pending.original_local.clone());
            }
        }
        let mut plan = Plan {
            view: Preview {
                plan_id: uuid::Uuid::new_v4().to_string(),
                conflicts: vec![],
                summary: Summary {
                    added: 0,
                    updated: 0,
                    deleted: 0,
                    organization: false,
                },
                warnings: vec![],
                remote_exists,
                has_changes: false,
                requires_confirmation: false,
            },
            created: Instant::now(),
            generation,
            revision,
            dependency_warnings: vec![],
            local,
            remote,
            baseline,
            observed,
            recovery,
        };
        let merged = resolve(&plan, &BTreeMap::new())?;
        let summary = merge::summary(&plan.local, &plan.remote, &merged.snapshot);
        let first_existing = remote_exists && plan.baseline.is_none();
        if first_existing {
            warnings.push("首次连接已有远端快照，需要手动确认合并后才能建立本机基线".into());
        }
        if plan.observed.missing_dirs.len() > 1 {
            warnings.push("WebDAV 基础目录不存在，提交时需要创建父目录；请手动确认地址".into());
        }
        if plan.observed.bytes.is_some() && plan.observed.etag.is_none() {
            warnings.push("远端没有可靠的强 ETag，提交将被拒绝；请修正 WebDAV 服务端配置".into());
        }
        let dependency_warnings = dependency_warnings(store.root(), &merged.snapshot);
        warnings.extend(dependency_warnings.iter().cloned());
        plan.dependency_warnings = dependency_warnings;
        warnings.extend(merged.warnings);
        warnings.sort();
        warnings.dedup();
        plan.view.requires_confirmation = !merged.conflicts.is_empty()
            || first_existing
            || summary.deleted > 0
            || merge::destructive_groups(&plan.local, &plan.remote, &merged.snapshot)
            || plan.recovery
            || plan.observed.missing_dirs.len() > 1
            || (plan.observed.bytes.is_some() && plan.observed.etag.is_none());
        plan.view.has_changes = !remote_exists
            || plan.local != merged.snapshot
            || plan.remote != merged.snapshot
            || !merged.conflicts.is_empty();
        plan.view.conflicts = merged.conflicts;
        plan.view.summary = summary;
        plan.view.warnings = warnings;
        if store.snapshot_checked()?.revision != revision {
            return Err("预览期间本机连接配置已改变，请重新预览".into());
        }
        let view = plan.view.clone();
        self.lock()?.plan = Some(plan);
        // 到期主动清除内存中的预览密码，不依赖下一次 IPC 才清理。
        let inner = self.inner.clone();
        let id = view.plan_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(PLAN_TTL).await;
            if let Ok(mut runtime) = inner.lock() {
                if runtime
                    .plan
                    .as_ref()
                    .is_some_and(|p| p.view.plan_id == id && p.created.elapsed() >= PLAN_TTL)
                {
                    runtime.plan = None;
                }
            }
        });
        Ok(view)
    }

    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let mut runtime = self.lock()?;
        purge(&mut runtime);
        if runtime.plan.as_ref().is_some_and(|p| p.view.plan_id == id) {
            runtime.plan = None;
        }
        Ok(())
    }

    pub async fn commit(
        &self,
        state: &AppState,
        id: &str,
        choices: BTreeMap<String, Choice>,
        automatic: bool,
    ) -> Result<SyncResult, String> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| "数据源同步正在执行，请稍后提交")?;
        let (plan, config, stored) = {
            let mut runtime = self.lock()?;
            purge(&mut runtime);
            if let Some(error) = &runtime.load_error {
                return Err(error.clone());
            }
            if !runtime.plan.as_ref().is_some_and(|p| p.view.plan_id == id) {
                return Err("同步预览不存在、已取消或已过期，请重新预览".into());
            }
            let plan = runtime.plan.take().expect("刚检查的预览");
            if plan.generation != runtime.generation {
                return Err("同步配置在预览后改变，请重新预览".into());
            }
            (plan, runtime.stored.config.clone(), runtime.stored.clone())
        };
        config.validate_sync()?;
        if automatic && plan.view.requires_confirmation {
            return Err(
                "此预览包含冲突、首次接管、删除或恢复场景，后端拒绝自动同步，请手动重新预览并确认"
                    .into(),
            );
        }
        let keys: BTreeSet<_> = plan.view.conflicts.iter().map(|c| c.key.clone()).collect();
        if choices.keys().any(|k| !keys.contains(k))
            || keys.iter().any(|k| !choices.contains_key(k))
        {
            return Err("冲突选择缺失或包含未知 key，请重新预览并明确选择所有冲突".into());
        }
        let merged = resolve(&plan, &choices)?;
        let close_ids: Vec<String> = plan
            .local
            .connections
            .iter()
            .filter(|old| {
                merged
                    .snapshot
                    .connections
                    .iter()
                    .find(|c| c.id == old.id)
                    .map(|new| old.requires_reopen(new))
                    .unwrap_or(true)
            })
            .map(|c| c.id.clone())
            .collect();
        let changed_ids = merged
            .snapshot
            .connections
            .iter()
            .filter(|new| plan.local.connections.iter().find(|c| c.id == new.id) != Some(*new))
            .map(|c| c.id.clone())
            .collect();
        let deleted_ids = plan
            .local
            .connections
            .iter()
            .filter(|old| !merged.snapshot.connections.iter().any(|c| c.id == old.id))
            .map(|c| c.id.clone())
            .collect();
        let _activity = state.activity.exclusive().map_err(|e| e.to_string())?;
        // Driver mutations take the same activity gate before their store lock. Recheck
        // while exclusive so a dependency cannot disappear after preview and before apply.
        let current_dependency_warnings = dependency_warnings(state.store.root(), &merged.snapshot);
        let mut warnings = plan.view.warnings.clone();
        warnings.extend(merged.warnings);
        warnings.extend(current_dependency_warnings);
        warnings.sort();
        warnings.dedup();
        let _transactions = state
            .manual_transactions
            .inactive_for_sync(&close_ids)
            .map_err(|e| e.to_string())?;
        let reservation = state.store.reserve_connections(plan.revision)?;
        let saved = state.store.snapshot_checked()?;
        let mut local_connections = merged.snapshot.connections.clone();
        for c in &mut local_connections {
            let same_password = plan
                .local
                .connections
                .iter()
                .find(|old| old.id == c.id)
                .is_some_and(|old| old.password == c.password);
            c.password = if same_password {
                saved
                    .connections
                    .iter()
                    .find(|old| old.id == c.id)
                    .ok_or("本机连接快照不完整")?
                    .password
                    .clone()
            } else {
                state
                    .store
                    .encrypt_password(&c.password)
                    .map_err(|_| "本机连接密码重加密失败，已停止同步")?
            };
        }
        let dav = self.dav(&config)?;
        if plan.observed.bytes.is_some() && plan.observed.etag.is_none() {
            return Err("远端缺少可靠的强 ETag，已拒绝提交或无条件覆盖".into());
        }
        let current = dav.read().await?;
        if current.etag != plan.observed.etag
            || current.bytes.as_ref().map(|b| crypto::hash(b))
                != plan.observed.bytes.as_ref().map(|b| crypto::hash(b))
        {
            return Err("远端在预览后改变，请重新预览；未执行 PUT 或本机覆盖".into());
        }
        let remote_changes = plan.remote != merged.snapshot || current.bytes.is_none();
        let encrypted = if remote_changes {
            let snapshot = merged.snapshot.clone();
            let passphrase = Zeroizing::new(config.passphrase.clone());
            Some(
                tokio::task::spawn_blocking(move || crypto::seal(&snapshot, &passphrase))
                    .await
                    .map_err(|_| "同步加密任务失败")??,
            )
        } else {
            None
        };
        let written_hash = encrypted
            .as_ref()
            .or(current.bytes.as_ref())
            .map(|b| crypto::hash(b))
            .ok_or("同步写入内容为空")?;
        let mut staged = stored;
        staged.identity = Some(config.identity()?);
        staged.pending = Some(Recovery {
            identity: config.identity()?,
            original_local: plan.local.clone(),
            target: merged.snapshot.clone(),
            written_hash,
        });
        // 必须先有可恢复的本机加密日志；写不成日志时不能改远端。
        persist_state(state.store.root(), &staged)?;
        self.lock()?.stored = staged.clone();
        if let Some(bytes) = &encrypted {
            dav.put_verified(&current, bytes).await?;
        }
        state
            .store
            .save_reserved(
                &reservation,
                local_connections,
                merged.snapshot.groups.clone(),
            )
            .map_err(|_| {
                "远端阶段已完成，但本机连接落盘失败；保留加密恢复记录与原成功基线，请重新预览恢复"
                    .to_string()
            })?;
        for id in close_ids {
            db::close_with_derived(&state.registry, &id);
        }
        let last_sync = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        staged.baseline = Some(merged.snapshot.clone());
        staged.pending = None;
        staged.config.last_sync = Some(last_sync.clone());
        let recovery_required = persist_state(state.store.root(), &staged).is_err();
        if recovery_required {
            warnings.push("连接已应用，但成功基线保存失败；原基线和加密恢复记录仍保留，请手动重新预览恢复".into());
        } else {
            self.lock()?.stored = staged;
        }
        Ok(SyncResult {
            connections: state.store.connections(),
            groups: state.store.groups(),
            changed_ids,
            deleted_ids,
            warnings,
            last_sync: if recovery_required { String::new() } else { last_sync },
            recovery_required,
        })
    }
}

fn purge(runtime: &mut Runtime) {
    if runtime
        .plan
        .as_ref()
        .is_some_and(|p| p.created.elapsed() >= PLAN_TTL)
    {
        runtime.plan = None;
    }
}

fn resolve(plan: &Plan, choices: &BTreeMap<String, Choice>) -> Result<Merged, String> {
    if plan.observed.bytes.is_none() && plan.baseline.is_some() {
        let remote_chosen = choices.get("remote_missing") == Some(&Choice::Remote);
        let mut snapshot = if remote_chosen {
            Snapshot::empty()
        } else {
            plan.local.clone()
        };
        if remote_chosen {
            snapshot
                .tombstones
                .extend(plan.local.tombstones.iter().cloned());
            snapshot
                .group_tombstones
                .extend(plan.local.group_tombstones.iter().cloned());
            snapshot
                .group_tombstones
                .extend(plan.local.groups.iter().cloned());
            snapshot.group_tombstones.extend(
                plan.baseline
                    .as_ref()
                    .expect("已检查基线")
                    .groups
                    .iter()
                    .cloned(),
            );
            snapshot.group_tombstones.extend(
                plan.baseline
                    .as_ref()
                    .expect("已检查基线")
                    .group_tombstones
                    .iter()
                    .cloned(),
            );
            for c in plan.local.connections.iter().chain(
                plan.baseline
                    .as_ref()
                    .expect("已检查基线")
                    .connections
                    .iter(),
            ) {
                snapshot.tombstones.insert(c.id.clone());
            }
            snapshot.tombstones.extend(
                plan.baseline
                    .as_ref()
                    .expect("已检查基线")
                    .tombstones
                    .iter()
                    .cloned(),
            );
        }
        snapshot.validate()?;
        return Ok(Merged {
            snapshot,
            conflicts: vec![Conflict {
                key: "remote_missing".into(),
                label: "远端快照已消失".into(),
                local_summary: format!(
                    "保留本机 {} 个数据源并重建远端快照",
                    plan.local.connections.len()
                ),
                remote_summary: "确认远端删除，清空本机数据源并保留删除记录".into(),
            }],
            warnings: vec!["已建立基线的远端快照返回 404，需要明确选择保留本机或确认删除".into()],
        });
    }
    merge::merge(plan.baseline.as_ref(), &plan.local, &plan.remote, choices)
}

fn load_state(root: &Path) -> Result<Persisted, String> {
    let bytes =
        match storage::read_bytes_limited(&root.join("datasource_sync.enc"), MAX_LOCAL_BYTES) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Persisted::default()),
            Err(_) => return Err("本机加密同步状态不可读，已拒绝覆盖".into()),
        };
    let bytes = bytes
        .strip_prefix(LOCAL_MAGIC)
        .ok_or("本机加密同步状态格式无效")?;
    let plain = storage::decrypt_bytes_at(root, bytes, LOCAL_AAD)
        .map_err(|_| "本机同步状态解密失败，已拒绝覆盖；请恢复原密钥与状态文件")?;
    let mut state: Persisted =
        serde_json::from_slice(&plain).map_err(|_| "本机同步状态损坏，已拒绝覆盖")?;
    if state.format != "navkit-datasource-sync-local" || state.version != 1 {
        return Err("本机同步状态版本无效".into());
    }
    if webdav::normalize_url(&state.config.url)? != state.config.url
        || !matches!(state.config.auto_every, -1 | 3 | 15 | 30 | 60)
    {
        return Err("本机同步配置字段无效".into());
    }
    webdav::parse_headers(&state.config.headers)?;
    if let Some(base) = &state.baseline {
        base.validate()?;
    }
    let current_identity = state.config.identity()?;
    let mut invalid_recovery = false;
    if let Some(pending) = &state.pending {
        invalid_recovery = pending.original_local.validate().is_err()
            || pending.target.validate().is_err()
            || pending.identity != current_identity
            || pending.written_hash.len() != 64;
    }
    if !state.config.remember_password {
        state.config.password.zeroize();
        state.config.passphrase.zeroize();
        state.config.headers.zeroize();
        state.config.last_sync = None;
        state.identity = None;
        state.baseline = None;
        state.pending = None;
    } else if invalid_recovery || state.identity.as_deref().is_some_and(|saved| saved != current_identity) {
        // Old endpoint-only journals and mismatched credentials cannot prove that
        // their baseline describes the currently authenticated DAV resource.
        state.config.last_sync = None;
        state.baseline = None;
        state.pending = None;
        state.identity = None;
    } else if state.identity.is_none()
        && (state.baseline.is_some() || state.pending.is_some() || state.config.last_sync.is_some())
    {
        state.config.last_sync = None;
        state.baseline = None;
        state.pending = None;
    } else {
        state.identity = Some(current_identity);
    }
    Ok(state)
}

fn persist_state(root: &Path, state: &Persisted) -> Result<(), String> {
    let mut disk = state.clone();
    if !disk.config.remember_password {
        disk.config.password.zeroize();
        disk.config.passphrase.zeroize();
        disk.config.headers.zeroize();
    }
    let plain = Zeroizing::new(serde_json::to_vec(&disk).map_err(|_| "序列化本机同步状态失败")?);
    if plain.len() > MAX_LOCAL_BYTES - 64 {
        return Err("本机同步状态超出大小上限".into());
    }
    let encrypted = storage::encrypt_bytes_at(root, &plain, LOCAL_AAD)
        .map_err(|_| "本机同步状态加密失败；未修改远端")?;
    let mut bytes = LOCAL_MAGIC.to_vec();
    bytes.extend_from_slice(&encrypted);
    storage::atomic_write(&root.join("datasource_sync.enc"), &bytes)
        .map_err(|_| "保存本机加密同步状态失败".to_string())
}

fn dependency_warnings(root: &Path, snapshot: &Snapshot) -> Vec<String> {
    let mut warnings = vec![];
    let mut drivers = BTreeMap::new();
    // 只检查本机依赖的 id；驱动定义、SQL、设置与任务从不进入同步包。
    if let Ok(bytes) = storage::read_bytes_limited(&root.join("drivers.json"), 1024 * 1024) {
        #[derive(Deserialize)]
        struct DriverId {
            id: String,
            #[serde(default)]
            source: String,
            #[serde(default)]
            target: String,
        }
        if let Ok(probes) = serde_json::from_slice::<Vec<DriverId>>(&bytes) {
            for probe in probes {
                drivers.insert(
                    probe.id,
                    probe.source != "file" || local_file_available(&probe.target) == Some(true),
                );
            }
        }
    }
    for c in &snapshot.connections {
        let label = merge::label(c);
        if !c.file.is_empty() {
            match local_file_available(&c.file) {
                Some(false) => warnings.push(format!("数据源「{label}」的本机文件不存在；只同步路径，不创建文件或自动连接")),
                None => warnings.push(format!("数据源「{label}」使用共享、URI 或特殊文件路径；仅保留路径文本，请在本机确认依赖后手动连接")),
                _ => {}
            }
        }
        match crate::driver_manager::parse_params(&c.params) {
            Ok(Some(params)) if drivers.get(&params.driver_id) != Some(&true) => warnings.push(format!("数据源「{label}」缺少本机自定义驱动或驱动文件；driver_id 已保留，请在本机安装依赖后手动连接")),
            Err(_) => warnings.push(format!("数据源「{label}」的自定义驱动参数无效；原参数已保留，未自动连接")),
            _ => {}
        }
        if c.kind.meta().is_none() || c.kind.is_planned() {
            warnings.push(format!(
                "数据源「{label}」的类型在本机不可用；配置已保留，未自动连接"
            ));
        }
    }
    warnings
}

fn local_file_available(file: &str) -> Option<bool> {
    // 远端路径不可信，UNC / URI / Windows 设备路径不能在预览时触发网络或设备访问。
    if file.is_empty()
        || file.starts_with(['\\', '/'])
            && file.chars().nth(1).is_some_and(|c| c == '\\' || c == '/')
        || file.chars().any(char::is_control)
    {
        return None;
    }
    if let Ok(url) = url::Url::parse(file) {
        if url.scheme().len() != 1 {
            return None;
        }
    }
    Some(Path::new(file).is_file())
}

#[tauri::command(rename_all = "camelCase")]
pub fn datasource_sync_get_config(service: State<'_, Service>) -> CmdResult<ConfigView> {
    service.get_config().map_err(AppError::new)
}

#[tauri::command(rename_all = "camelCase")]
pub fn datasource_sync_set_config(
    state: State<'_, AppState>,
    service: State<'_, Service>,
    config: ConfigInput,
) -> CmdResult<ConfigView> {
    service
        .set_config(&state.store, config)
        .map_err(AppError::new)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn datasource_sync_test(service: State<'_, Service>) -> CmdResult<String> {
    service.test().await.map_err(AppError::new)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn datasource_sync_preview(
    state: State<'_, AppState>,
    service: State<'_, Service>,
) -> CmdResult<Preview> {
    service.preview(&state.store).await.map_err(AppError::new)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn datasource_sync_commit(
    state: State<'_, AppState>,
    service: State<'_, Service>,
    plan_id: String,
    choices: BTreeMap<String, Choice>,
    automatic: bool,
) -> CmdResult<SyncResult> {
    service
        .commit(&state, &plan_id, choices, automatic)
        .await
        .map_err(AppError::new)
}

#[tauri::command(rename_all = "camelCase")]
pub fn datasource_sync_cancel(service: State<'_, Service>, plan_id: String) -> CmdResult<()> {
    service.cancel(&plan_id).map_err(AppError::new)
}

#[cfg(test)]
mod tests;
