//! Regression tests for issue #10: the daemon must read its listen port from a
//! non-colliding env var. The kubelet injects a legacy service-link variable
//! `CHARRADISSA_PORT=tcp://<ip>:<port>` for the `charradissa` Service, which used
//! to clobber the daemon's own port config.

use charradissa_core::config::listen_port;

#[test]
fn listen_port_reads_renamed_var_and_ignores_legacy_collision() {
    // Simulate the kubelet-injected legacy variable. The daemon must NOT read it.
    std::env::set_var("CHARRADISSA_PORT", "tcp://10.0.0.1:8448");
    std::env::remove_var("CHARRADISSA_LISTEN_PORT");
    assert_eq!(
        listen_port(),
        "8448",
        "must fall back to the default and ignore the colliding CHARRADISSA_PORT"
    );

    // The renamed variable is the one the daemon honours.
    std::env::set_var("CHARRADISSA_LISTEN_PORT", "9000");
    assert_eq!(
        listen_port(),
        "9000",
        "must read the renamed CHARRADISSA_LISTEN_PORT"
    );

    std::env::remove_var("CHARRADISSA_LISTEN_PORT");
    std::env::remove_var("CHARRADISSA_PORT");
}
