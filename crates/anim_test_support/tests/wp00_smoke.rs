//! WP00 smoke test (spec 9.3): workspace compiles and runtime works.
#[test]
fn workspace_smoke() {
    // integer sanity in debug - no GPU, no external tools
    let a: u64 = 900 * 1001 + 1;
    assert_eq!(a, 900901);
}
