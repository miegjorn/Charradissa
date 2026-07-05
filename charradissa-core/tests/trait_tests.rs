use charradissa_core::backend::ChatBackend;
use charradissa_core::task::TaskManager;
use charradissa_core::farga::FargaWriter;

fn _assert_chat_backend_object_safe(_: &dyn ChatBackend) {}
fn _assert_task_manager_object_safe(_: &dyn TaskManager) {}
fn _assert_farga_writer_object_safe(_: &dyn FargaWriter) {}

#[test]
fn traits_are_object_safe() {
    // Compile-only: if this compiles, the traits are object-safe.
}
