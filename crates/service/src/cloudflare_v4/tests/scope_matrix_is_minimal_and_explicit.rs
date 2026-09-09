use super::*;

#[test]
fn scope_matrix_is_minimal_and_explicit() {
    let context = |role| V4RequestContext {
        role,
        request_id: open_compute_core::RequestId::generate(),
    };
    for permission in [
        V4Permission::Read,
        V4Permission::ProductWrite,
        V4Permission::Maintenance,
    ] {
        assert!(context(V4Role::Admin).require(permission).is_ok());
    }
    assert!(
        context(V4Role::Deployer)
            .require(V4Permission::Read)
            .is_ok()
    );
    assert!(
        context(V4Role::Deployer)
            .require(V4Permission::ProductWrite)
            .is_ok()
    );
    assert!(
        context(V4Role::Deployer)
            .require(V4Permission::Maintenance)
            .is_err()
    );
    assert!(
        context(V4Role::ReadOnly)
            .require(V4Permission::Read)
            .is_ok()
    );
    assert!(
        context(V4Role::ReadOnly)
            .require(V4Permission::ProductWrite)
            .is_err()
    );
}
