use std::sync::Once;

static TRACING_INIT: Once = Once::new();

fn init_tracing() {
    TRACING_INIT.call_once(|| {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    });
}

#[test]
fn integration_harness_initializes_tracing_subscriber() {
    init_tracing();
    tracing::info!("integration tracing initialized");
}
