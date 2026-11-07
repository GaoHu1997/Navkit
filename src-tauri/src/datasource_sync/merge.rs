//! 按 UUID 三方合并；名称只关联分组，不创建 groupId，也不丢弃空分组。
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::{Choice, Conflict, Summary};

pub(super) fn group_key(group: &str) -> String {
    format!("group:{}", super::crypto::hash(group.as_bytes()))
}
pub(super) fn order_key(group: &str) -> String {
    format!("order:{}", super::crypto::hash(group.as_bytes()))
}

fn display_group(group: &str, secrets: &[Zeroizing<String>]) -> String {
    let mut text = Zeroizing::new(group.to_string());
    for secret in secrets {
        let next = text.replace(secret.as_str(), "••••");
        text.zeroize();
        *text = next;
    }
    safe_text(&text, "")
}
use crate::{models::ConnectionConfig, storage::Store};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub format: String,
    pub version: u32,
    pub connections: Vec<ConnectionConfig>,
    pub groups: Vec<String>,
    pub tombstones: BTreeSet<String>,
    #[serde(default)]
    pub group_tombstones: BTreeSet<String>,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        for c in &mut self.connections {
            c.password.zeroize();
            c.params.zeroize();
        }
    }
}

impl Snapshot {
    pub fn new(
        connections: Vec<ConnectionConfig>,
        groups: Vec<String>,
        tombstones: BTreeSet<String>,
    ) -> Self {
        Self {
            format: "navkit-datasources".into(),
            version: 1,
            connections,
            groups,
            tombstones,
            group_tombstones: BTreeSet::new(),
        }
    }
    pub fn empty() -> Self {
        Self::new(vec![], vec![], BTreeSet::new())
    }

    pub fn capture(store: &Store, base: Option<&Self>) -> Result<(Self, u64), String> {
        let saved = store.snapshot_checked()?;
        let mut connections = saved.connections;
        for c in &mut connections {
            c.password = store.decrypt_password(&c.password).map_err(|_| {
                "本机连接密码解密失败，已拒绝上传或覆盖；请恢复密钥或重新输入密码".to_string()
            })?;
        }
        let mut groups = saved.groups;
        // 兼容旧连接文件中的隐式分组；仅按名称补齐，不迁移任何 id。
        for c in &connections {
            if !c.group.is_empty() && !groups.contains(&c.group) {
                groups.push(c.group.clone());
            }
        }
        let mut tombstones = base.map(|b| b.tombstones.clone()).unwrap_or_default();
        if let Some(base) = base {
            for c in &base.connections {
                if !connections.iter().any(|l| l.id == c.id) {
                    tombstones.insert(c.id.clone());
                }
            }
        }
        for c in &connections {
            tombstones.remove(&c.id);
        }
        let mut group_tombstones = base.map(|b| b.group_tombstones.clone()).unwrap_or_default();
        if let Some(base) = base {
            for group in &base.groups {
                if !groups.contains(group) {
                    group_tombstones.insert(group.clone());
                }
            }
        }
        for group in &groups {
            group_tombstones.remove(group);
        }
        let mut snapshot = Self::new(connections, groups, tombstones);
        snapshot.group_tombstones = group_tombstones;
        snapshot.validate()?;
        Ok((snapshot, saved.revision))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != "navkit-datasources" || self.version != 1 {
            return Err("同步快照格式或版本无效".into());
        }
        if self.connections.len() > 5000
            || self.groups.len() > 2000
            || self.tombstones.len() > 20000
            || self.group_tombstones.len() > 20000
        {
            return Err("同步快照的数据源、分组或删除记录数量超限".into());
        }
        let mut groups = BTreeSet::new();
        for group in &self.groups {
            if group.is_empty()
                || group.len() > 512
                || group.chars().any(char::is_control)
                || !groups.insert(group)
                || self.group_tombstones.contains(group)
            {
                return Err("同步快照包含无效或重复分组".into());
            }
        }
        let mut ids = BTreeSet::new();
        for c in &self.connections {
            if !valid_id(&c.id) || !ids.insert(&c.id) || self.tombstones.contains(&c.id) {
                return Err("同步快照包含无效、重复或已删除的连接 ID".into());
            }
            if c.kind.0.is_empty()
                || c.kind.0.len() > 128
                || c.name.len() > 4096
                || c.password.len() > 65536
                || [&c.host, &c.user, &c.database, &c.file, &c.params]
                    .iter()
                    .any(|v| v.len() > 512 * 1024)
            {
                return Err("同步快照的连接字段无效或过大".into());
            }
            if !c.group.is_empty() && !groups.contains(&c.group) {
                return Err("同步快照的分组引用不完整".into());
            }
        }
        if self.tombstones.iter().any(|id| !valid_id(id)) {
            return Err("同步快照包含无效删除记录".into());
        }
        if self
            .group_tombstones
            .iter()
            .any(|g| g.is_empty() || g.len() > 512 || g.chars().any(char::is_control))
        {
            return Err("同步快照包含无效分组删除记录".into());
        }
        Ok(())
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 160 && !id.chars().any(char::is_control)
}

pub(super) fn safe_text(text: &str, password: &str) -> String {
    let text = if password.is_empty() {
        text.to_string()
    } else {
        text.replace(password, "••••")
    };
    text.chars().filter(|c| !c.is_control()).take(80).collect()
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['_', '-'], "");
    key.contains("password")
        || key.contains("passwd")
        || key.contains("token")
        || key.contains("secret")
        || matches!(
            key.as_str(),
            "pwd" | "apikey" | "accesskey" | "privatekey" | "authorization" | "cookie"
        )
}

/// 只提取需遮盖的参数值，不把参数或解析错误传到 IPC。
fn parameter_secrets(params: &str) -> Vec<Zeroizing<String>> {
    fn visit(value: &serde_json::Value, secret: bool, out: &mut Vec<Zeroizing<String>>) {
        match value {
            serde_json::Value::String(value) => {
                if secret && !value.is_empty() {
                    out.push(Zeroizing::new(value.clone()));
                }
                if let Ok(url) = url::Url::parse(value) {
                    if let Some(password) = url.password().filter(|p| !p.is_empty()) {
                        out.push(Zeroizing::new(password.into()));
                    }
                    for (key, value) in url
                        .query_pairs()
                        .filter(|(k, v)| sensitive_key(k) && !v.is_empty())
                    {
                        let _ = key;
                        out.push(Zeroizing::new(value.into_owned()));
                    }
                }
            }
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    visit(value, secret || sensitive_key(key), out);
                }
            }
            serde_json::Value::Array(array) => {
                for value in array {
                    visit(value, secret, out);
                }
            }
            _ => {}
        }
    }
    let mut out = vec![];
    let json = params
        .trim()
        .strip_prefix(crate::driver_manager::PARAMS_PREFIX)
        .unwrap_or(params);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
        visit(&value, false, &mut out);
    }
    for part in params.split(['&', ';', '\n']) {
        for (key, value) in url::form_urlencoded::parse(part.as_bytes()) {
            if sensitive_key(key.trim()) && !value.is_empty() {
                out.push(Zeroizing::new(value.into_owned()));
            }
        }
    }
    out
}

fn safe_connection_text(c: &ConnectionConfig, text: &str) -> String {
    let mut text = Zeroizing::new(if c.password.is_empty() {
        text.to_string()
    } else {
        text.replace(&c.password, "••••")
    });
    for secret in parameter_secrets(&c.params) {
        let next = text.replace(secret.as_str(), "••••");
        text.zeroize();
        *text = next;
    }
    safe_text(&text, "")
}

pub(super) fn label(c: &ConnectionConfig) -> String {
    safe_connection_text(c, &c.name)
}

fn description(c: Option<&ConnectionConfig>, deleted: bool) -> String {
    let Some(c) = c else {
        return if deleted {
            "已删除（有删除记录）"
        } else {
            "不存在 / 已删除"
        }
        .into();
    };
    // params、文件路径、用户和密码不进入摘要；URL 主机只显示 host 部分。
    let host = if let Ok(url) = url::Url::parse(&c.host) {
        url.host_str().unwrap_or("").to_string()
    } else if c.host.contains(['@', '?', '#', '/', '\\']) {
        "（地址已隐藏）".into()
    } else {
        safe_connection_text(c, &c.host)
    };
    format!(
        "名称：{}；类型：{}；主机：{}；分组：{}",
        label(c),
        safe_connection_text(c, c.kind.id()),
        safe_connection_text(c, &host),
        safe_connection_text(c, &c.group)
    )
}

type Records<'a> = BTreeMap<&'a str, &'a ConnectionConfig>;
fn records(s: &Snapshot) -> Records<'_> {
    s.connections.iter().map(|c| (c.id.as_str(), c)).collect()
}

fn organization(
    s: &Snapshot,
) -> (
    Vec<String>,
    BTreeMap<String, String>,
    BTreeMap<String, Vec<String>>,
) {
    (
        s.groups.clone(),
        s.connections
            .iter()
            .map(|c| (c.id.clone(), c.group.clone()))
            .collect(),
        orders(s),
    )
}

fn orders(s: &Snapshot) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> =
        s.groups.iter().map(|g| (g.clone(), vec![])).collect();
    out.entry(String::new()).or_default();
    for c in &s.connections {
        out.entry(c.group.clone()).or_default().push(c.id.clone());
    }
    out
}

fn rename_like(base: &Snapshot, side: &Snapshot) -> bool {
    let removed = base.groups.iter().any(|g| !side.groups.contains(g));
    let added = side.groups.iter().any(|g| !base.groups.contains(g));
    let moved = base.connections.iter().any(|b| {
        side.connections
            .iter()
            .any(|s| s.id == b.id && s.group != b.group)
    });
    removed && (added || moved)
}

fn group_changed(base: &Snapshot, side: &Snapshot, group: &str) -> bool {
    base.connections
        .iter()
        .filter(|c| c.group == group)
        .collect::<Vec<_>>()
        != side
            .connections
            .iter()
            .filter(|c| c.group == group)
            .collect::<Vec<_>>()
}

fn projection(list: &[String], allowed: &BTreeSet<String>) -> Vec<String> {
    list.iter()
        .filter(|id| allowed.contains(*id))
        .cloned()
        .collect()
}

/// 保留主序列的顺序，并把另一端独有项插在其已有邻居旁。
fn integrate(primary: &[String], secondary: &[String], allowed: &BTreeSet<String>) -> Vec<String> {
    let mut out = projection(primary, allowed);
    for (i, id) in secondary.iter().enumerate() {
        if !allowed.contains(id) || out.contains(id) {
            continue;
        }
        let before = secondary[..i]
            .iter()
            .rev()
            .find_map(|p| out.iter().position(|x| x == p));
        let after = secondary[i + 1..]
            .iter()
            .find_map(|p| out.iter().position(|x| x == p));
        // 同一锚点之间的并发新增项排在主序列已有项之后，不把原有新项顶到后面。
        let at = match (before, after) {
            (Some(before), Some(after)) if before < after => after,
            (Some(before), Some(_)) => before + 1,
            (None, Some(after)) => after,
            (_, None) => out.len(),
        };
        out.insert(at, id.clone());
    }
    for id in allowed {
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

pub(super) struct Merged {
    pub snapshot: Snapshot,
    pub conflicts: Vec<Conflict>,
    pub warnings: Vec<String>,
}

pub(super) fn merge(
    base: Option<&Snapshot>,
    local: &Snapshot,
    remote: &Snapshot,
    choices: &BTreeMap<String, Choice>,
) -> Result<Merged, String> {
    let l = records(local);
    let r = records(remote);
    let b = base.map(records).unwrap_or_default();
    let mut ids = BTreeSet::new();
    ids.extend(l.keys().copied());
    ids.extend(r.keys().copied());
    ids.extend(b.keys().copied());
    let mut merged = BTreeMap::<String, ConnectionConfig>::new();
    let mut conflicts = vec![];
    let mut warnings = vec![];
    let mut rejected_groups = BTreeSet::new();
    let mut accepted_groups = BTreeSet::new();
    let mut display_secrets = vec![];
    for c in local.connections.iter().chain(&remote.connections) {
        if !c.password.is_empty() {
            display_secrets.push(Zeroizing::new(c.password.clone()));
        }
        display_secrets.extend(parameter_secrets(&c.params));
    }
    // 无基线设备不能默默带回已删除的空分组；用户仍可明确选择重新创建。
    for group in local
        .groups
        .iter()
        .chain(&remote.groups)
        .collect::<BTreeSet<_>>()
    {
        let stale_local = local.groups.contains(group)
            && !remote.groups.contains(group)
            && remote.group_tombstones.contains(group);
        let stale_remote = remote.groups.contains(group)
            && !local.groups.contains(group)
            && local.group_tombstones.contains(group);
        // 基线已有的分组删改统一交给 organization，避免两个选择互相覆盖。
        let conflict = if let Some(base) = base {
            (stale_local || stale_remote) && !base.groups.contains(group)
        } else {
            stale_local || stale_remote
        };
        if conflict {
            let key = group_key(group);
            let default = if stale_local {
                Choice::Remote
            } else {
                Choice::Local
            };
            conflicts.push(Conflict {
                key: key.clone(),
                label: format!("分组「{}」删除", display_group(group, &display_secrets)),
                local_summary: if local.groups.contains(group) {
                    "分组存在"
                } else {
                    "分组已删除"
                }
                .into(),
                remote_summary: if remote.groups.contains(group) {
                    "分组存在"
                } else {
                    "分组已删除"
                }
                .into(),
            });
            let choice = choices.get(&key).copied().unwrap_or(default);
            if choice == Choice::Local && !local.groups.contains(group)
                || choice == Choice::Remote && !remote.groups.contains(group)
            {
                rejected_groups.insert(group.clone());
            } else {
                accepted_groups.insert(group.clone());
            }
        }
    }
    for id in ids {
        let left = l.get(id).copied();
        let right = r.get(id).copied();
        let old = b.get(id).copied();
        let stale_left =
            old.is_none() && left.is_some() && right.is_none() && remote.tombstones.contains(id);
        let stale_right =
            old.is_none() && right.is_some() && left.is_none() && local.tombstones.contains(id);
        let (selected, conflict, default) = if stale_left {
            (right, true, Choice::Remote)
        } else if stale_right {
            (left, true, Choice::Local)
        } else if left == right {
            (left, false, Choice::Local)
        } else if old.is_some() && left == old {
            (right, false, Choice::Remote)
        } else if old.is_some() && right == old {
            (left, false, Choice::Local)
        } else if old.is_none() && left.is_none() {
            (right, false, Choice::Remote)
        } else if old.is_none() && right.is_none() {
            (left, false, Choice::Local)
        } else {
            (left, true, Choice::Local)
        };
        let selected = if conflict {
            let key = format!("connection:{id}");
            conflicts.push(Conflict {
                key: key.clone(),
                label: left.or(right).map(label).unwrap_or_else(|| "数据源".into()),
                local_summary: description(left, local.tombstones.contains(id)),
                remote_summary: description(right, remote.tombstones.contains(id)),
            });
            match choices.get(&key).copied().unwrap_or(default) {
                Choice::Local => left,
                Choice::Remote => right,
            }
        } else {
            selected
        };
        if let Some(c) = selected {
            merged.insert(id.into(), c.clone());
        }
    }

    let group_common: BTreeSet<String> = local
        .groups
        .iter()
        .filter(|g| remote.groups.contains(g))
        .cloned()
        .collect();
    let gp_l = projection(&local.groups, &group_common);
    let gp_r = projection(&remote.groups, &group_common);
    let gp_b = base
        .map(|b| projection(&b.groups, &group_common))
        .unwrap_or_default();
    let structure_conflict = base.is_some_and(|b| {
        organization(local) != organization(remote)
            && ((rename_like(b, local) && remote != b)
                || (rename_like(b, remote) && local != b)
                || b.groups.iter().any(|g| {
                    !local.groups.contains(g) && group_changed(b, remote, g)
                        || !remote.groups.contains(g) && group_changed(b, local, g)
                })
                || (local.groups != b.groups
                    && remote.groups != b.groups
                    && local.groups != remote.groups
                    && b.groups
                        .iter()
                        .any(|g| !local.groups.contains(g) || !remote.groups.contains(g))))
    }) || (gp_l != gp_b && gp_r != gp_b && gp_l != gp_r);
    let org_choice = choices
        .get("organization")
        .copied()
        .unwrap_or(Choice::Local);
    let mut groups = if structure_conflict {
        conflicts.push(Conflict {
            key: "organization".into(),
            label: "分组与连接组织".into(),
            local_summary: format!("{} 个分组；保留本机分组名、归属和顺序", local.groups.len()),
            remote_summary: format!("{} 个分组；保留远端分组名、归属和顺序", remote.groups.len()),
        });
        let side = if org_choice == Choice::Local {
            local
        } else {
            remote
        };
        let selected = records(side);
        for c in merged.values_mut() {
            if let Some(s) = selected.get(c.id.as_str()) {
                c.group = s.group.clone();
            } else if !c.group.is_empty()
                && !side.groups.contains(&c.group)
                && !accepted_groups.contains(&c.group)
            {
                c.group.clear();
                warnings.push(
                    "组织冲突选择后，另一端独有连接所属的旧分组不存在，连接保留在未分组中".into(),
                );
            }
        }
        side.groups.clone()
    } else {
        let mut allowed = BTreeSet::new();
        for group in local.groups.iter().chain(&remote.groups) {
            let existed = base.is_some_and(|b| b.groups.contains(group));
            if !existed || local.groups.contains(group) && remote.groups.contains(group) {
                allowed.insert(group.clone());
            }
        }
        for c in merged.values().filter(|c| !c.group.is_empty()) {
            allowed.insert(c.group.clone());
        }
        if base.is_some_and(|b| local.groups == b.groups) {
            integrate(&remote.groups, &local.groups, &allowed)
        } else {
            integrate(&local.groups, &remote.groups, &allowed)
        }
    };
    for group in accepted_groups {
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    groups.retain(|g| !rejected_groups.contains(g));
    for c in merged
        .values_mut()
        .filter(|c| rejected_groups.contains(&c.group))
    {
        c.group.clear();
        warnings.push("已确认分组删除；仍保留的连接放入未分组，不丢弃连接".into());
    }
    for c in merged.values().filter(|c| !c.group.is_empty()) {
        if !groups.contains(&c.group) {
            groups.push(c.group.clone());
        }
    }

    let lo = orders(local);
    let ro = orders(remote);
    let bo = base.map(orders).unwrap_or_default();
    let mut connections = vec![];
    for group in std::iter::once(&String::new()).chain(groups.iter()) {
        let allowed: BTreeSet<String> = merged
            .values()
            .filter(|c| &c.group == group)
            .map(|c| c.id.clone())
            .collect();
        let left = lo.get(group).cloned().unwrap_or_default();
        let right = ro.get(group).cloned().unwrap_or_default();
        let old = bo.get(group).cloned().unwrap_or_default();
        let common: BTreeSet<String> = left
            .iter()
            .filter(|id| right.contains(id) && allowed.contains(*id))
            .cloned()
            .collect();
        let lp = projection(&left, &common);
        let rp = projection(&right, &common);
        let bp = projection(&old, &common);
        let double_reorder = !structure_conflict && lp != bp && rp != bp && lp != rp;
        let remote_primary = if structure_conflict {
            org_choice == Choice::Remote
        } else if double_reorder {
            let key = order_key(group);
            conflicts.push(Conflict {
                key: key.clone(),
                label: if group.is_empty() {
                    "未分组连接顺序".into()
                } else {
                    format!("分组「{}」顺序", display_group(group, &display_secrets))
                },
                local_summary: format!("{} 项；采用本机相对顺序", left.len()),
                remote_summary: format!("{} 项；采用远端相对顺序", right.len()),
            });
            choices.get(&key).copied().unwrap_or(Choice::Local) == Choice::Remote
        } else {
            lp == bp && rp != bp
        };
        let order = if remote_primary {
            integrate(&right, &left, &allowed)
        } else {
            integrate(&left, &right, &allowed)
        };
        for id in order {
            if let Some(c) = merged.remove(&id) {
                connections.push(c);
            }
        }
    }
    connections.extend(merged.into_values());
    let mut tombstones = local
        .tombstones
        .union(&remote.tombstones)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut group_tombstones = local
        .group_tombstones
        .union(&remote.group_tombstones)
        .cloned()
        .collect::<BTreeSet<_>>();
    if let Some(base) = base {
        tombstones.extend(base.tombstones.iter().cloned());
    }
    if let Some(base) = base {
        group_tombstones.extend(base.group_tombstones.iter().cloned());
    }
    for id in l.keys().chain(r.keys()).chain(b.keys()) {
        if connections.iter().any(|c| c.id == *id) {
            tombstones.remove(*id);
        } else {
            tombstones.insert((*id).to_string());
        }
    }
    for group in local
        .groups
        .iter()
        .chain(&remote.groups)
        .chain(base.into_iter().flat_map(|b| b.groups.iter()))
    {
        if groups.contains(group) {
            group_tombstones.remove(group);
        } else {
            group_tombstones.insert(group.clone());
        }
    }
    let mut names = BTreeSet::new();
    if connections.iter().any(|c| !names.insert(c.name.clone())) {
        warnings.push("存在同名但 UUID 不同的数据源，已保留各自连接".into());
    }
    warnings.sort();
    warnings.dedup();
    let mut snapshot = Snapshot::new(connections, groups, tombstones);
    snapshot.group_tombstones = group_tombstones;
    snapshot.validate()?;
    Ok(Merged {
        snapshot,
        conflicts,
        warnings,
    })
}

fn content_equal(a: &ConnectionConfig, b: &ConnectionConfig) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    a.group.clear();
    b.group.clear();
    a == b
}

pub(super) fn summary(local: &Snapshot, remote: &Snapshot, target: &Snapshot) -> Summary {
    let l = records(local);
    let r = records(remote);
    let t = records(target);
    let ids: BTreeSet<_> = l.keys().chain(r.keys()).chain(t.keys()).copied().collect();
    let mut out = Summary {
        added: 0,
        updated: 0,
        deleted: 0,
        organization: organization(local) != organization(target)
            || organization(remote) != organization(target),
    };
    for id in ids {
        match t.get(id) {
            Some(c) => {
                if !l.contains_key(id) || !r.contains_key(id) {
                    out.added += 1;
                } else if !content_equal(l[id], c) || !content_equal(r[id], c) {
                    out.updated += 1;
                }
            }
            None if l.contains_key(id) || r.contains_key(id) => out.deleted += 1,
            None => {}
        }
    }
    out
}

pub(super) fn destructive_groups(local: &Snapshot, remote: &Snapshot, target: &Snapshot) -> bool {
    local
        .groups
        .iter()
        .chain(&remote.groups)
        .any(|g| !target.groups.contains(g))
}
