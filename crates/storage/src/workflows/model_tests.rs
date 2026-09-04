use super::WorkflowReservationState;
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
