//! Deterministic work-conserving weighted deficit round-robin.

use open_compute_core::SchedulerKind;

const POOL_COUNT: usize = SchedulerKind::ALL.len();

#[derive(Clone, Debug)]
pub(super) struct FairSelector {
    cursor: usize,
    deficits: [u64; POOL_COUNT],
    weights: [u32; POOL_COUNT],
}

impl FairSelector {
    pub(super) fn new(weights: [u32; POOL_COUNT]) -> Self {
        Self {
            cursor: 0,
            deficits: [0; POOL_COUNT],
            weights,
        }
    }

    pub(super) fn select(
        &mut self,
        ready: [bool; POOL_COUNT],
        mut pool_permits: [usize; POOL_COUNT],
        global_permits: usize,
    ) -> Vec<SchedulerKind> {
        let mut selected = Vec::with_capacity(global_permits);
        while selected.len() < global_permits {
            if !has_runnable(ready, pool_permits) {
                break;
            }
            if self.next_with_deficit(ready, pool_permits).is_none() {
                for kind in SchedulerKind::ALL {
                    let index = kind.index();
                    if ready[index] && pool_permits[index] > 0 {
                        self.deficits[index] =
                            self.deficits[index].saturating_add(u64::from(self.weights[index]));
                    }
                }
            }
            let Some(index) = self.next_with_deficit(ready, pool_permits) else {
                break;
            };
            self.deficits[index] = self.deficits[index].saturating_sub(1);
            pool_permits[index] = pool_permits[index].saturating_sub(1);
            selected.push(SchedulerKind::ALL[index]);
            self.cursor = (index + 1) % POOL_COUNT;
        }
        selected
    }

    pub(super) fn refund(&mut self, kind: SchedulerKind, count: usize) {
        self.deficits[kind.index()] =
            self.deficits[kind.index()].saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }

    fn next_with_deficit(
        &self,
        ready: [bool; POOL_COUNT],
        pool_permits: [usize; POOL_COUNT],
    ) -> Option<usize> {
        (0..POOL_COUNT)
            .map(|offset| (self.cursor + offset) % POOL_COUNT)
            .find(|index| ready[*index] && pool_permits[*index] > 0 && self.deficits[*index] > 0)
    }
}

fn has_runnable(ready: [bool; POOL_COUNT], pool_permits: [usize; POOL_COUNT]) -> bool {
    (0..POOL_COUNT).any(|index| ready[index] && pool_permits[index] > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(values: &[SchedulerKind]) -> [usize; POOL_COUNT] {
        let mut result = [0; POOL_COUNT];
        for value in values {
            result[value.index()] += 1;
        }
        result
    }

    #[test]
    fn empty_and_cap_exhausted_pools_are_skipped() {
        let mut selector = FairSelector::new([1; POOL_COUNT]);
        assert!(
            selector
                .select([false; POOL_COUNT], [8; POOL_COUNT], 8)
                .is_empty()
        );
        assert_eq!(
            selector.select([true; POOL_COUNT], [0, 2, 0, 0], 8),
            vec![SchedulerKind::Queue, SchedulerKind::Queue]
        );
    }

    #[test]
    fn idle_pools_do_not_reserve_global_capacity() {
        let mut selector = FairSelector::new([1; POOL_COUNT]);
        let selected = selector.select([true, false, false, false], [64; POOL_COUNT], 16);
        assert_eq!(selected, vec![SchedulerKind::Alarm; 16]);
    }

    #[test]
    fn four_busy_pools_are_bounded_and_deterministic() {
        let mut first = FairSelector::new([1; POOL_COUNT]);
        let mut second = FairSelector::new([1; POOL_COUNT]);
        let expected = first.select([true; POOL_COUNT], [64; POOL_COUNT], 32);
        assert_eq!(
            expected,
            second.select([true; POOL_COUNT], [64; POOL_COUNT], 32)
        );
        assert_eq!(counts(&expected), [8, 8, 8, 8]);
        for window in expected.windows(4) {
            assert!(window.contains(&SchedulerKind::Alarm));
            assert!(window.contains(&SchedulerKind::Queue));
            assert!(window.contains(&SchedulerKind::Cron));
            assert!(window.contains(&SchedulerKind::Workflow));
        }
    }

    #[test]
    fn weights_converge_without_starving_a_ready_pool() {
        let mut selector = FairSelector::new([1, 4, 2, 1]);
        let selected = selector.select([true; POOL_COUNT], [512; POOL_COUNT], 400);
        assert_eq!(counts(&selected), [50, 200, 100, 50]);
        let longest_alarm_gap = selected
            .iter()
            .enumerate()
            .filter_map(|(index, kind)| (*kind == SchedulerKind::Alarm).then_some(index))
            .collect::<Vec<_>>()
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .max()
            .unwrap_or(0);
        assert!(longest_alarm_gap <= 16);
    }

    #[test]
    fn a_pool_becoming_ready_does_not_wait_for_backlog_to_empty() {
        let mut selector = FairSelector::new([1; POOL_COUNT]);
        assert_eq!(
            selector.select([false, true, false, false], [64; POOL_COUNT], 8),
            vec![SchedulerKind::Queue; 8]
        );
        let next = selector.select([true, true, false, false], [64; POOL_COUNT], 4);
        assert!(next.contains(&SchedulerKind::Alarm));
        assert!(next.contains(&SchedulerKind::Queue));
    }

    #[test]
    fn short_claim_refund_remains_available_to_the_same_round() {
        let mut selector = FairSelector::new([1; POOL_COUNT]);
        let selected = selector.select([true; POOL_COUNT], [8; POOL_COUNT], 4);
        assert_eq!(selected.len(), 4);
        selector.refund(SchedulerKind::Alarm, 1);
        assert_eq!(
            selector.select([true, false, false, false], [1, 0, 0, 0], 1),
            vec![SchedulerKind::Alarm]
        );
    }
}
