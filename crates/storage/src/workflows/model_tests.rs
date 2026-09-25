use super::{WorkflowReservationState, WorkflowTarget};
use open_compute_core::{InstanceId, VersionId, WorkerId, WorkflowId, WorkflowVersionId};
use std::str::FromStr as _;

#[test]
fn reservation_state_sql_round_trip_is_exact() {
    for (state, encoded) in [
        (WorkflowReservationState::Reserved, "reserved"),
        (WorkflowReservationState::Bound, "bound"),
    ] {
        assert_eq!(state.as_str(), encoded);
        assert_eq!(WorkflowReservationState::from_str(encoded), Ok(state));
    }
    assert_eq!(WorkflowReservationState::from_str("ready"), Err(()));
}

#[test]
fn workflow_target_serializes_only_instance_identity() {
    let instance_id = InstanceId::generate();
    let target = WorkflowTarget {
        instance_id,
        definition_id: WorkflowId::generate(),
        definition_name: "jobs".to_owned(),
        workflow_version_id: WorkflowVersionId::generate(),
        worker_id: WorkerId::generate(),
        worker_version_id: VersionId::generate(),
        worker_code_sha256: [1; 32],
        class_name: "Jobs".to_owned(),
        loader_schema_version: 1,
        capability_version: 1,
        descriptor_sha256: [2; 32],
    };
    let mut json = serde_json::to_value(&target).unwrap();
    assert_eq!(json["instanceId"], instance_id.to_string());
    assert!(json.get("accountId").is_none());
    assert_eq!(
        serde_json::from_value::<WorkflowTarget>(json.clone()).unwrap(),
        target
    );
    let object = json.as_object_mut().unwrap();
    let id = object.remove("instanceId").unwrap();
    object.insert("accountId".to_owned(), id);
    assert!(serde_json::from_value::<WorkflowTarget>(json).is_err());
}
