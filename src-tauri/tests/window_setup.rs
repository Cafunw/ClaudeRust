//! Whether a start is a test copy, whose window `run` puts off-screen and
//! shows without focus (`display::automation`). Its own test binary: it sets
//! process environment variables, which other tests would see.

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
}
