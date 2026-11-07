//! Fixed official JVM options for the two exact managed driver families.
//! These are not user-supplied command-line options and never include credentials.
use anyhow::{bail, Result};
const DREMIO: &[&str] = &["--add-opens=java.base/java.nio=ALL-UNNAMED"];
const IGNITE_11: &[&str] = &[
    "--add-exports=java.base/jdk.internal.misc=ALL-UNNAMED",
    "--add-exports=java.base/sun.nio.ch=ALL-UNNAMED",
    "--add-exports=java.management/com.sun.jmx.mbeanserver=ALL-UNNAMED",
    "--add-exports=jdk.internal.jvmstat/sun.jvmstat.monitor=ALL-UNNAMED",
    "--add-exports=java.base/sun.reflect.generics.reflectiveObjects=ALL-UNNAMED",
    "--add-opens=jdk.management/com.sun.management.internal=ALL-UNNAMED",
    "--add-opens=java.base/java.nio=ALL-UNNAMED", "--illegal-access=permit",
];
const IGNITE_17: &[&str] = &[
    "--add-opens=java.base/jdk.internal.access=ALL-UNNAMED",
    "--add-opens=java.base/jdk.internal.misc=ALL-UNNAMED",
    "--add-opens=java.base/sun.nio.ch=ALL-UNNAMED",
    "--add-opens=java.base/sun.util.calendar=ALL-UNNAMED",
    "--add-opens=java.management/com.sun.jmx.mbeanserver=ALL-UNNAMED",
    "--add-opens=jdk.internal.jvmstat/sun.jvmstat.monitor=ALL-UNNAMED",
    "--add-opens=java.base/sun.reflect.generics.reflectiveObjects=ALL-UNNAMED",
    "--add-opens=jdk.management/com.sun.management.internal=ALL-UNNAMED",
    "--add-opens=java.base/java.io=ALL-UNNAMED",
    "--add-opens=java.base/java.nio=ALL-UNNAMED",
    "--add-opens=java.base/java.net=ALL-UNNAMED",
    "--add-opens=java.base/java.util=ALL-UNNAMED",
    "--add-opens=java.base/java.util.concurrent=ALL-UNNAMED",
    "--add-opens=java.base/java.util.concurrent.locks=ALL-UNNAMED",
    "--add-opens=java.base/java.util.concurrent.atomic=ALL-UNNAMED",
    "--add-opens=java.base/java.lang=ALL-UNNAMED",
    "--add-opens=java.base/java.lang.invoke=ALL-UNNAMED",
    "--add-opens=java.base/java.math=ALL-UNNAMED",
    "--add-opens=java.sql/java.sql=ALL-UNNAMED",
    "--add-opens=java.base/java.lang.reflect=ALL-UNNAMED",
    "--add-opens=java.base/java.time=ALL-UNNAMED",
    "--add-opens=java.base/java.text=ALL-UNNAMED",
    "--add-opens=java.management/sun.management=ALL-UNNAMED",
    "--add-opens=java.desktop/java.awt.font=ALL-UNNAMED",
];
pub(super) fn for_driver(class: &str, java_version: &str) -> Result<&'static [&'static str]> {
    if !matches!(class, "com.dremio.jdbc.Driver" | "org.apache.ignite.IgniteJdbcThinDriver") { return Ok(&[]); }
    let major = java_version.split('"').nth(1).and_then(|v| v.split('.').next()).and_then(|v| v.parse::<u32>().ok());
    match (class, major) {
        ("com.dremio.jdbc.Driver", Some(11 | 17)) => Ok(DREMIO),
        ("org.apache.ignite.IgniteJdbcThinDriver", Some(11)) => Ok(IGNITE_11),
        ("org.apache.ignite.IgniteJdbcThinDriver", Some(17)) => Ok(IGNITE_17),
        _ => bail!("Dremio legacy / Ignite 2 托管驱动仅核验 Java 11 和 17，不能猜测其它版本的模块参数"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_driver_and_jdk_choose_the_entire_verified_option_set() {
        assert_eq!(for_driver("org.apache.ignite.IgniteJdbcThinDriver", "openjdk version \"11.0.28\"").unwrap(), IGNITE_11);
        assert_eq!(for_driver("org.apache.ignite.IgniteJdbcThinDriver", "openjdk version \"17.0.16\"").unwrap(), IGNITE_17);
        assert_eq!(IGNITE_11.len(), 8); assert_eq!(IGNITE_17.len(), 24);
        assert_eq!(for_driver("com.dremio.jdbc.Driver", "openjdk version \"17.0.16\"").unwrap(), DREMIO);
        assert!(for_driver("com.dremio.jdbc.Driver", "openjdk version \"21.0.1\"").is_err());
        assert!(for_driver("org.apache.ignite.jdbc.IgniteJdbcDriver", "openjdk version \"17.0.16\"").unwrap().is_empty());
        assert!(for_driver("org.h2.Driver", "openjdk version \"21.0.1\"").unwrap().is_empty());
    }
}
