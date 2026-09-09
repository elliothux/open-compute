use super::*;

#[test]
fn workflow_batch_item_operation_ids_are_stable_and_instance_scoped() {
    let batch = WorkflowOperationId::generate();
    let first = workflow_batch_item_operation_id(batch, 0).unwrap();
    assert_eq!(first, workflow_batch_item_operation_id(batch, 0).unwrap());
    assert_ne!(first, workflow_batch_item_operation_id(batch, 1).unwrap());
}
