//! Whether a start is a test copy, whose window `run` puts off-screen and
//! shows without focus (`display::automation`), and which opens nothing on
//! the person's screen. Its own test binary: it sets process environment
//! variables, which other tests would see.

use clauderust_lib::commands::os_open;
use clauderust_lib::display::automation;

#[test]
fn a_test_copy_is_recognised_from_its_environment() {
    const OFF: &str = "CLAUDERUST_OFFSCREEN";
    const WV2: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";
    std::env::remove_var(OFF);
    std::env::remove_var(WV2);
    assert!(!automation(), "a normal start");

    std::env::set_var(OFF, "1");
    assert!(automation());
    std::env::set_var(OFF, "0");
    assert!(!automation());
    std::env::set_var(OFF, "true");
    assert!(!automation(), "only 1 turns it on");
    std::env::remove_var(OFF);

    // a debugging port marks a test copy only in a development build: in a
    // release build the variable may be set for another app of the user
    std::env::set_var(WV2, "--remote-debugging-port=9333 --disable-gpu");
    assert_eq!(automation(), cfg!(debug_assertions));
    std::env::set_var(WV2, "--disable-gpu");
    assert!(!automation());
    std::env::remove_var(WV2);

    // a test copy hands nothing to the system's opener: a click on a link in a test once opened the
    // person's browser. (In this test, not one of its own: the variable is the whole process's.)
    std::env::set_var(OFF, "1");
    assert!(os_open("https://example.com/", || -> Result<(), String> { panic!("opened while automated") }).is_ok());
    std::env::remove_var(OFF);
    let mut opened = false;
    assert!(os_open("x", || -> Result<(), String> { opened = true; Ok(()) }).is_ok());
    assert!(opened, "a normal copy opens it");
    assert_eq!(os_open("x", || Err("no handler")).unwrap_err().kind(), "open_failed");
}

/// Every use of the system's opener (browser, a file's app, a folder) goes through
/// `commands::os_open`, which a test copy never lets through.
#[test]
fn every_way_to_the_systems_opener_goes_through_os_open() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut uses = 0;
    for e in std::fs::read_dir(&src).unwrap().flatten() {
        let text = std::fs::read_to_string(e.path()).unwrap_or_default();
        for (i, line) in text.lines().enumerate() {
            if line.contains(".opener()") {
                uses += 1;
                assert!(line.contains("os_open("), "{}:{}: the opener outside os_open: {line}", e.path().display(), i + 1);
            }
        }
    }
    assert!(uses >= 6, "links, files, folders, attachments, the editor's fallback, MCP sign-in: {uses}");
}
