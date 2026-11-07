//! Mandatory, pinned runtime dependencies of the exact managed JDBC releases.
//! These remain original official JAR/POM files; no global classpath or neighbour JAR guessing.
use super::{check_asset, safe_relative, spec, DownloadSpec};
use anyhow::{anyhow, bail, Result};
use std::{collections::HashSet, io::Read, path::{Path, PathBuf}};

pub(super) struct Dependency {
    pub name: &'static str,
    pub pom_name: &'static str,
    pub jar: DownloadSpec,
    pub pom: DownloadSpec,
    entries: usize,
    expanded: u64,
    anchor: &'static str,
    major: u16,
}
const BSON: Dependency = Dependency {
    name: "bson-3.8.0.jar", pom_name: "bson-3.8.0.pom",
    jar: spec("https://repo.maven.apache.org/maven2/org/mongodb/bson/3.8.0/bson-3.8.0.jar", 479203, "d30b5aeba3ae9b7c68c8a6103b41918c5f7318972007b9b92033ee861762d87e"),
    pom: spec("https://repo.maven.apache.org/maven2/org/mongodb/bson/3.8.0/bson-3.8.0.pom", 1334, "32c2703382dbb92286d4977450646279a8eb42761e5edd4df595be6121b85136"),
    entries: 394, expanded: 982896, anchor: "org/bson/BsonDocument.class", major: 50,
};
const JAXB: Dependency = Dependency {
    name: "jaxb-api-2.3.1.jar", pom_name: "jaxb-api-2.3.1.pom",
    jar: spec("https://repo.maven.apache.org/maven2/javax/xml/bind/jaxb-api/2.3.1/jaxb-api-2.3.1.jar", 128076, "88b955a0df57880a26a74708bc34f74dcaf8ebf4e78843a28b50eae945732b06"),
    pom: spec("https://repo.maven.apache.org/maven2/javax/xml/bind/jaxb-api/2.3.1/jaxb-api-2.3.1.pom", 20160, "12b20cf922773445c3445c2883cbf671fa982111e9bf9f875020f9313b3814b1"),
    entries: 136, expanded: 255598, anchor: "javax/xml/bind/DatatypeConverter.class", major: 52,
};
const ACTIVATION: Dependency = Dependency {
    name: "javax.activation-api-1.2.0.jar", pom_name: "javax.activation-api-1.2.0.pom",
    jar: spec("https://repo.maven.apache.org/maven2/javax/activation/javax.activation-api/1.2.0/javax.activation-api-1.2.0.jar", 56674, "43fdef0b5b6ceb31b0424b208b930c74ab58fac2ceeb7b3f6fd3aeb8b5ca4393"),
    pom: spec("https://repo.maven.apache.org/maven2/javax/activation/javax.activation-api/1.2.0/javax.activation-api-1.2.0.pom", 4919, "da2926f3c8be898643cc10acdec6de0b0351a57fb2735770fa0177b06ade71b9"),
    entries: 41, expanded: 118688, anchor: "javax/activation/DataSource.class", major: 49,
};
// Ignite 2.18.0's exact non-optional closure. Keep each original POM, including
// cache-api's version-specific JSR-107 license; do not relabel the closure Apache-2.0.
const CACHE: Dependency = Dependency {
    name: "cache-api-1.0.0.jar", pom_name: "cache-api-1.0.0.pom",
    jar: spec("https://repo.maven.apache.org/maven2/javax/cache/cache-api/1.0.0/cache-api-1.0.0.jar", 49789, "d2836213144722e2eb1d7461f6e2ea3349d6fbbf9cd5f345c7133bc29bd39ca1"),
    pom: spec("https://repo.maven.apache.org/maven2/javax/cache/cache-api/1.0.0/cache-api-1.0.0.pom", 11413, "424dd1d4e7e92bffb4bb9f18acf7f53835d218fb9788336b5fdc69cff5c675ef"),
    entries: 79, expanded: 89815, anchor: "javax/cache/CacheException.class", major: 50,
};
const ANNOTATIONS: Dependency = Dependency {
    name: "annotations-16.0.3.jar", pom_name: "annotations-16.0.3.pom",
    jar: spec("https://repo.maven.apache.org/maven2/org/jetbrains/annotations/16.0.3/annotations-16.0.3.jar", 18587, "04b16e8d2309bf7771fbee16187b76f63af6ccd023cf664ec846e4e8e65c5b3f"),
    pom: spec("https://repo.maven.apache.org/maven2/org/jetbrains/annotations/16.0.3/annotations-16.0.3.pom", 1322, "cba9993e59dade3210593edaa2c9c7c3abfefe3f387a95eea4c48fce05f3bfff"),
    entries: 46, expanded: 19152, anchor: "org/jetbrains/annotations/NotNull.class", major: 52,
};
const IGNITE_COMMONS: Dependency = Dependency {
    name: "ignite-commons-2.18.0.jar", pom_name: "ignite-commons-2.18.0.pom",
    jar: spec("https://repo.maven.apache.org/maven2/org/apache/ignite/ignite-commons/2.18.0/ignite-commons-2.18.0.jar", 278750, "daf592b18a476677a07db0c7b5ee3ad102dcaed30a10b984f3cf34641537c2dc"),
    pom: spec("https://repo.maven.apache.org/maven2/org/apache/ignite/ignite-commons/2.18.0/ignite-commons-2.18.0.pom", 4357, "a1df88945b33030d576a6932e17ea67b815970ffdbbea451c02bb71973d8b2a8"),
    entries: 231, expanded: 569379, anchor: "org/apache/ignite/IgniteCheckedException.class", major: 55,
};
const IGNITE_BINARY_API: Dependency = Dependency {
    name: "ignite-binary-api-2.18.0.jar", pom_name: "ignite-binary-api-2.18.0.pom",
    jar: spec("https://repo.maven.apache.org/maven2/org/apache/ignite/ignite-binary-api/2.18.0/ignite-binary-api-2.18.0.jar", 175399, "388c77689a1bd75a7e6cd30de693de9ee71d6918efee3cc519c74a77be2350e3"),
    pom: spec("https://repo.maven.apache.org/maven2/org/apache/ignite/ignite-binary-api/2.18.0/ignite-binary-api-2.18.0.pom", 4382, "da22917cf2d5035f98a62b06b1892753a464e1ee8f8fd408b4280f23806b7ad3"),
    entries: 123, expanded: 376683, anchor: "org/apache/ignite/binary/BinaryBasicIdMapper.class", major: 55,
};
const IGNITE_BINARY_IMPL: Dependency = Dependency {
    name: "ignite-binary-impl-2.18.0.jar", pom_name: "ignite-binary-impl-2.18.0.pom",
    jar: spec("https://repo.maven.apache.org/maven2/org/apache/ignite/ignite-binary-impl/2.18.0/ignite-binary-impl-2.18.0.jar", 221444, "a766b0388817ddb53f92955503f4d07e35bd7c78d145592d6922f165157c8f7d"),
    pom: spec("https://repo.maven.apache.org/maven2/org/apache/ignite/ignite-binary-impl/2.18.0/ignite-binary-impl-2.18.0.pom", 4765, "6d4ca9899b1355b2e16ffc7e2ad33137c4c8dbdf47492294f0ce1af12abcf486"),
    entries: 113, expanded: 500647, anchor: "org/apache/ignite/internal/binary/BinariesFactoryImpl.class", major: 55,
};
const IGNITE_UNSAFE: Dependency = Dependency {
    name: "ignite-grid-unsafe-2.18.0.jar", pom_name: "ignite-grid-unsafe-2.18.0.pom",
    jar: spec("https://repo.maven.apache.org/maven2/org/apache/ignite/ignite-grid-unsafe/2.18.0/ignite-grid-unsafe-2.18.0.jar", 48595, "0102be8425d5b8a2ea187e7fae567f015cc266993e0509674831c87ab0ffb319"),
    pom: spec("https://repo.maven.apache.org/maven2/org/apache/ignite/ignite-grid-unsafe/2.18.0/ignite-grid-unsafe-2.18.0.pom", 4386, "751cc953696a8719dc5dec285f2b4a71ff6d40c5e0bd335ba4fec2e4f015454b"),
    entries: 34, expanded: 104604, anchor: "org/apache/ignite/internal/util/GridUnsafe.class", major: 55,
};
// The checker dependency is executable code in ordinary Xugu geometry serialization,
// not just compile-time annotations. Keep the original MIT POM and JAR.
const XUGU_CHECKER: Dependency = Dependency {
    name: "checker-qual-3.5.0.jar", pom_name: "checker-qual-3.5.0.pom",
    jar: spec("https://repo.maven.apache.org/maven2/org/checkerframework/checker-qual/3.5.0/checker-qual-3.5.0.jar", 214381, "729990b3f18a95606fc2573836b6958bcdb44cb52bfbd1b7aa9c339cff35a5a4"),
    pom: spec("https://repo.maven.apache.org/maven2/org/checkerframework/checker-qual/3.5.0/checker-qual-3.5.0.pom", 2165, "2836b3b8a78edb31a1803592e60fc767b21f2d190764631ba6efa0837bb35721"),
    entries: 382, expanded: 275472, anchor: "org/checkerframework/checker/nullness/NullnessUtil.class", major: 52,
};
pub(super) fn specs(kind: &str) -> &'static [Dependency] {
    match kind {
        "informix" => &[BSON], "kylin" => &[JAXB, ACTIVATION], "xugu" => &[XUGU_CHECKER],
        "ignite" => &[CACHE, ANNOTATIONS, IGNITE_COMMONS, IGNITE_BINARY_API, IGNITE_BINARY_IMPL, IGNITE_UNSAFE],
        _ => &[],
    }
}
pub(super) fn verify(dir: &Path, kind: &str) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for dep in specs(kind) {
        let path = dir.join(dep.name);
        check_asset(&path, &dep.jar)?;
        check_asset(&dir.join(dep.pom_name), &dep.pom)?;
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path)?)?;
        if archive.len() != dep.entries { bail!("JDBC 运行依赖 ZIP 条目数不符：{}", dep.name); }
        let mut names = HashSet::new();
        let mut expanded = 0u64;
        let mut anchor = false;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_owned();
            if entry.enclosed_name().is_none() || !safe_relative(name.trim_end_matches('/'))
                || !names.insert(name.clone()) || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                bail!("JDBC 运行依赖含不安全条目：{}", dep.name);
            }
            if entry.is_dir() { continue; }
            expanded = expanded.checked_add(entry.size()).ok_or_else(|| anyhow!("JDBC 依赖大小溢出"))?;
            if expanded > dep.expanded { bail!("JDBC 运行依赖超过固定展开大小：{}", dep.name); }
            let size = entry.size();
            if name == dep.anchor {
                if size > 1024 * 1024 { bail!("JDBC 运行依赖主类过大"); }
                let mut bytes = Vec::new();
                entry.by_ref().take(size + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 != size || bytes.len() < 10 || bytes[..4] != [0xca,0xfe,0xba,0xbe]
                    || u16::from_be_bytes([bytes[6],bytes[7]]) != dep.major {
                    bail!("JDBC 运行依赖字节码不符：{}", dep.name);
                }
                anchor = true;
            } else if std::io::copy(&mut entry.by_ref().take(size + 1), &mut std::io::sink())? != size {
                bail!("JDBC 运行依赖 ZIP 条目被截断：{}", dep.name);
            }
        }
        if expanded != dep.expanded || !anchor { bail!("JDBC 运行依赖不完整：{}", dep.name); }
        paths.push(path);
    }
    Ok(paths)
}
