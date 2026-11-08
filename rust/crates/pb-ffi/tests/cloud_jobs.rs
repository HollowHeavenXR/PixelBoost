//! Live cloud-job round trip: set PB_TEST_SERVER and PB_TEST_KEY.
use pixelboost_core::server::{DownloadListener, ServerClient};
use std::sync::Arc;

struct P;
impl DownloadListener for P {
    fn on_progress(&self, _r: u64, _t: u64) {}
}

#[test]
fn cloud_job() {
    let Ok(base) = std::env::var("PB_TEST_SERVER") else { return };
    let key = std::env::var("PB_TEST_KEY").unwrap_or_default();
    let c = ServerClient::new(base, key, "pb-e2e-test".into());
    let img = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testimages/src/g4249.jpg");
    let job = c.submit_job(img.into(), r#"{"model":"lineart","scale":2}"#.into(), "e2e".into()).unwrap();
    assert!(job.status == "queued" || job.status == "running");
    let t0 = std::time::Instant::now();
    let done = loop {
        let j = c.get_job(job.id.clone()).unwrap();
        if j.status == "done" || j.status == "failed" {
            break j;
        }
        assert!(t0.elapsed().as_secs() < 300);
        std::thread::sleep(std::time::Duration::from_secs(2));
    };
    assert_eq!(done.status, "done", "{}", done.error);
    assert_eq!((done.out_width, done.out_height), (1700, 1006));
    assert!(c.list_jobs().unwrap().iter().any(|j| j.id == job.id));
    let dest = std::env::temp_dir().join("pb_e2e.png");
    c.download_job(job.id.clone(), dest.to_string_lossy().into(), Arc::new(P)).unwrap();
    assert_eq!(std::fs::metadata(&dest).unwrap().len(), done.result_bytes);
    c.download_job_thumb(job.id.clone(), std::env::temp_dir().join("pb_e2e_t.jpg").to_string_lossy().into()).unwrap();
    c.delete_job(job.id).unwrap();
    println!("cloud job ok in {:.1}s, {} bytes", t0.elapsed().as_secs_f64(), done.result_bytes);
}
