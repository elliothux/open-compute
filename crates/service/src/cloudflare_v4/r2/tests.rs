use super::{current_named_resource, valid_bucket_name};
use open_compute_core::{AccountId, BindingKind, ResourceAvailability, ResourceId, ResourceState};
use open_compute_storage::ResourceRecord;

fn resource(name: &str, state: ResourceState) -> ResourceRecord {
    ResourceRecord {
        id: ResourceId::generate(),
        account_id: AccountId::generate(),
        kind: BindingKind::R2Bucket,
        name: name.to_owned(),
        state,
        availability: ResourceAvailability::Healthy,
        availability_code: None,
        spec_generation: 1,
        driver_schema_version: 1,
        created_at_ms: 1,
        updated_at_ms: 1,
        deleted_at_ms: (state == ResourceState::Tombstoned).then_some(1),
    }
}

#[test]
fn bucket_names_match_the_pinned_wrangler_contract() {
    assert!(valid_bucket_name("abc"));
    assert!(valid_bucket_name(&format!("a{}z", "b".repeat(61))));
    assert!(!valid_bucket_name("ab"));
    assert!(!valid_bucket_name(&format!("a{}z", "b".repeat(62))));
    assert!(!valid_bucket_name("Upper"));
    assert!(!valid_bucket_name("-leading"));
    assert!(!valid_bucket_name("trailing-"));
}

#[test]
fn put_by_name_recovery_ignores_tombstones_and_selects_creating_resource() {
    let tombstone = resource("reused-name", ResourceState::Tombstoned);
    let creating = resource("reused-name", ResourceState::Creating);
    let creating_id = creating.id;
    let selected = current_named_resource(vec![tombstone, creating], "reused-name")
        .expect("creating resource remains recoverable");
    assert_eq!(selected.id, creating_id);
    assert_eq!(selected.state, ResourceState::Creating);
}
