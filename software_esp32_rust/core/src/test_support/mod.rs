//! Shared test helpers (port of test/native/support) and the generators of the fuzz tests.

pub mod rng;
pub mod stm_golden;

// for the module ports; not every port uses every generator
#[allow(unused_imports)]
pub use rng::{CRand, Lcg, Rng};

/// Asserts that `got` is the text `want`; prints both escaped (`b"K\xc3\xbcche"` reads
/// `K\xc3\xbcche`) instead of byte lists.
#[track_caller]
pub fn assert_text(got: &[u8], want: impl AsRef<[u8]>) {
    let want = want.as_ref();
    assert!(
        got == want,
        "got \"{}\", want \"{}\"",
        got.escape_ascii(),
        want.escape_ascii()
    );
}
