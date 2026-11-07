use super::missing_odbc_exports;

fn missing(symbols: &[&str]) -> Option<&'static str> {
    missing_odbc_exports(|symbol| {
        let name = std::str::from_utf8(&symbol[..symbol.len() - 1]).unwrap();
        symbols.contains(&name)
    })
}

#[test]
fn complete_ansi_and_unicode_odbc_drivers_are_valid() {
    assert_eq!(missing(&["SQLAllocHandle", "SQLFreeHandle", "SQLDriverConnect", "SQLGetInfo"]), None);
    // Actual pinned Oracle 23.26 exports: ANSI names are intentionally absent.
    assert_eq!(missing(&["SQLAllocHandle", "SQLFreeHandle", "SQLConnectW", "SQLDriverConnectW", "SQLGetInfoW"]), None);
}

#[test]
fn unicode_suffixes_without_driver_manager_recognition_are_rejected() {
    assert!(missing(&["SQLAllocHandle", "SQLFreeHandle", "SQLDriverConnectW", "SQLGetInfoW"]).is_some());
    assert!(missing(&["SQLAllocHandle", "SQLFreeHandle", "SQLDriverConnectA", "SQLGetInfoA"]).is_some());
}

#[test]
fn incomplete_handles_connection_and_information_never_pass() {
    for symbols in [
        vec!["SQLFreeHandle", "SQLDriverConnect", "SQLGetInfo"],
        vec!["SQLAllocHandle", "SQLDriverConnect", "SQLGetInfo"],
        vec!["SQLAllocHandle", "SQLFreeHandle", "SQLGetInfo"],
        vec!["SQLAllocHandle", "SQLFreeHandle", "SQLDriverConnect"],
        vec!["SQLAllocHandle", "SQLFreeHandle", "SQLConnectW", "SQLDriverConnectW"],
        vec!["SQLAllocHandle", "SQLFreeHandle", "SQLConnectW", "SQLGetInfoW"],
    ] {
        assert!(missing(&symbols).is_some(), "incomplete native library was accepted: {symbols:?}");
    }
    assert!(missing(&[]).is_some());
}
