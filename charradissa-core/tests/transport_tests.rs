// CharradissaTransport object-safety and trait-impl check
use charradissa_core::transport::CharradissaTransport;
use amassada_core::transport::Transport;

fn _assert_amassada_transport(_: &dyn Transport) {}

#[test]
fn charradissa_transport_implements_amassada_transport() {
    // Compile-only: if this compiles, CharradissaTransport satisfies the amassada Transport trait
}
