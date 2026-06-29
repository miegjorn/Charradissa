//! Tests for hs_token resolution — regression for the bug where MATRIX_HS_TOKEN
//! was ignored and AppserviceState received MATRIX_AS_TOKEN instead, causing
//! every Synapse → Charradissa transaction to return 403.

use charradissa_core::config::hs_token;
use std::sync::Mutex;

/// Shared lock to serialize env-var mutation across parallel tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn uses_matrix_hs_token_when_set() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("MATRIX_HS_TOKEN", "explicit-hs-secret");
    assert_eq!(hs_token("as-secret"), "explicit-hs-secret");
    std::env::remove_var("MATRIX_HS_TOKEN");
}

#[test]
fn falls_back_to_as_token_when_matrix_hs_token_unset() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("MATRIX_HS_TOKEN");
    assert_eq!(hs_token("as-secret"), "as-secret");
}
