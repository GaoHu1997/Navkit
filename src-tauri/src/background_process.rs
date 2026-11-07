//! Background subprocesses must not allocate or attach a Windows console.
//!
//! Keep java.exe (not javaw.exe): source-launch diagnostics and the JDBC protocol
//! still use ordinary stdin/stdout/stderr handles. CREATE_NO_WINDOW changes only
//! console allocation; it does not detach the process or disable captured errors.
use std::ffi::OsStr;

pub fn command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

pub fn tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    // From<std::process::Command> retains the Windows creation flags and handles.
    command(program).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, process::Stdio};
    use tokio::io::AsyncWriteExt;

    const PROBE_ENV: &str = "NAVKIT_BACKGROUND_PROCESS_TEST";
    const INPUT: &str = "NAVKIT_PIPE_中文\n";
    const OUTPUT: &str = "NAVKIT_BACKGROUND_PIPE_OK";
    const ERROR: &str = "NAVKIT_BACKGROUND_STDERR_OK";
    const PROBE_TEST: &str = "background_process::tests::child_probe";

    // Called as a real child test executable by both subprocess constructors.
    #[test]
    fn child_probe() {
        if std::env::var(PROBE_ENV).as_deref() != Ok("1") {
            return;
        }
        #[cfg(windows)]
        {
            #[link(name = "kernel32")]
            extern "system" {
                fn GetConsoleWindow() -> *mut std::ffi::c_void;
            }
            assert!(
                unsafe { GetConsoleWindow() }.is_null(),
                "background subprocess unexpectedly allocated or attached a console"
            );
        }
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
        assert_eq!(input, INPUT, "hidden subprocess must retain its stdin pipe");
        println!("{OUTPUT}");
        eprintln!("{ERROR}");
    }

    fn check_output(output: std::process::Output) {
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(output.status.success(), "stdout: {stdout}\nstderr: {stderr}");
        assert!(stdout.contains(OUTPUT), "stdout pipe lost: {stdout}");
        assert!(stderr.contains(ERROR), "stderr pipe lost: {stderr}");
    }

    #[test]
    fn standard_probe_has_no_console_and_retains_stdio() {
        let mut child = command(std::env::current_exe().unwrap())
            .args(["--exact", PROBE_TEST, "--nocapture"])
            .env(PROBE_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(INPUT.as_bytes()).unwrap();
        check_output(child.wait_with_output().unwrap());
    }

    #[tokio::test]
    async fn async_sidecar_has_no_console_and_retains_stdio() {
        let mut child = tokio_command(std::env::current_exe().unwrap())
            .args(["--exact", PROBE_TEST, "--nocapture"])
            .env(PROBE_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(INPUT.as_bytes()).await.unwrap();
        input.shutdown().await.unwrap();
        drop(input);
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            child.wait_with_output(),
        ).await.unwrap().unwrap();
        check_output(output);
    }

    #[test]
    fn java_probes_and_sidecars_use_background_constructors() {
        for source in [include_str!("jdbc_db.rs"), include_str!("h2_db.rs")] {
            assert!(!source.contains("std::process::Command::new"));
            assert!(!source.contains("tokio::process::Command::new"));
            assert!(source.contains("crate::background_process::command("));
            assert!(source.contains("crate::background_process::tokio_command("));
        }
    }
}
