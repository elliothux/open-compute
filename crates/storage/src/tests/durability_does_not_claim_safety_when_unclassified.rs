#[test]
fn durability_does_not_claim_safety_when_unclassified() {
    use crate::FilesystemDurability;
    assert!(
        FilesystemDurability::ApparentlyLocal
            .doctor_warning()
            .is_none()
    );
    assert!(
        FilesystemDurability::NetworkOrRemote
            .doctor_warning()
            .is_some()
    );
    assert!(
        FilesystemDurability::Unclassified
            .doctor_warning()
            .is_some()
    );
}
