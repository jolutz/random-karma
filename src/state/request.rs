use random_karma::{worker_agent::RequestMetadata, SolverStrategy};

/// Monotonic identities used to correlate worker responses and invalidate datasets.
#[derive(Debug, Default)]
pub struct RequestState {
    next_request_id: u64,
    dataset_generation: u64,
    active: Option<RequestMetadata>,
}

impl RequestState {
    pub fn begin(
        &mut self,
        target: u32,
        lap_count: usize,
        player_count: usize,
        timeout_ms: f64,
        tolerance_percent: f64,
        strategy: SolverStrategy,
    ) -> RequestMetadata {
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let metadata = RequestMetadata {
            request_id: self.next_request_id,
            dataset_generation: self.dataset_generation,
            target,
            lap_count,
            player_count,
            timeout_ms,
            tolerance_percent,
            strategy,
        };
        self.active = Some(metadata.clone());
        metadata
    }

    pub fn accepts(&self, metadata: &RequestMetadata) -> bool {
        self.active.as_ref() == Some(metadata)
            && metadata.dataset_generation == self.dataset_generation
    }

    pub fn finish(&mut self, metadata: &RequestMetadata) -> bool {
        if self.accepts(metadata) {
            self.active = None;
            true
        } else {
            false
        }
    }

    pub fn cancel(&mut self) {
        self.active = None;
    }

    pub fn replace_dataset(&mut self) -> u64 {
        self.dataset_generation = self.dataset_generation.wrapping_add(1);
        self.active = None;
        self.dataset_generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin(state: &mut RequestState, target: u32) -> RequestMetadata {
        state.begin(target, 2, 3, 1_000.0, 0.5, SolverStrategy::Bounded)
    }

    #[test]
    fn only_latest_request_is_accepted() {
        let mut state = RequestState::default();
        let first = begin(&mut state, 100);
        let second = begin(&mut state, 200);

        assert!(!state.accepts(&first));
        assert!(state.accepts(&second));
        assert!(!state.finish(&first));
        assert!(state.finish(&second));
        assert!(!state.accepts(&second));
    }

    #[test]
    fn replacing_dataset_invalidates_in_flight_response() {
        let mut state = RequestState::default();
        let old = begin(&mut state, 100);
        assert_eq!(state.replace_dataset(), 1);
        assert!(!state.accepts(&old));

        let new = begin(&mut state, 100);
        assert_eq!(new.dataset_generation, 1);
        assert!(new.request_id > old.request_id);
        assert!(state.accepts(&new));
    }
}
