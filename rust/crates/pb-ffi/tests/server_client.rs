//! Live test against a running upscaler-bridge: set PB_TEST_SERVER and
//! PB_TEST_KEY (skipped otherwise).
use pixelboost_core::server::ServerClient;

#[test]
fn round_trip() {
    let Ok(base) = std::env::var("PB_TEST_SERVER") else { return };
    let key = std::env::var("PB_TEST_KEY").unwrap_or_default();
    let c = ServerClient::new(base, key, "client-test".into());
    assert_eq!(c.health().unwrap(), "2.0.0");
    let img = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testimages/src/g4249.jpg");
    let id = c.upload_export(img.into(), "client test".into(), 1).unwrap();
    let list = c.list_exports().unwrap();
    assert!(list.iter().any(|e| e.id == id && e.label == "client test" && e.size_bytes > 0));
    let dest = std::env::temp_dir().join("pb_client_dl.jpg");
    c.download_export(id.clone(), dest.to_string_lossy().into()).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), std::fs::read(img).unwrap());
    c.delete_export(id).unwrap();
    c.log_upscale(r#"{"source_width":1,"source_height":1,"technique":"t","scale_factor":4,"processing_ms":1,"success":true}"#.into());
    c.log_action("test".into(), "{}".into());
    std::thread::sleep(std::time::Duration::from_millis(800));
}
