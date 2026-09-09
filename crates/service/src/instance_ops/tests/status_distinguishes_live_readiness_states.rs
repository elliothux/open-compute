use super::*;

#[test]
fn status_distinguishes_live_readiness_states() {
    assert_eq!(
        inspect_live_descriptor_state("ready", Some("200 OK")),
        "ready"
    );
    assert_eq!(
        inspect_live_descriptor_state("ready", Some("503 Service Unavailable")),
        "degraded"
    );
    assert_eq!(inspect_live_descriptor_state("degraded", None), "degraded");
    assert_eq!(inspect_live_descriptor_state("failed", None), "failed");
    assert_eq!(inspect_live_descriptor_state("starting", None), "starting");
}
