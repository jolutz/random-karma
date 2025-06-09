//! Web Worker agent for offloading karma calculations to background threads.

use crate::{compute_jaccard_similarity, perform_multiple_runs, Car};
use futures::sink::SinkExt;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use yew_agent::reactor::{reactor, ReactorScope};

/// Arguments for karma calculation tasks sent to workers.
#[derive(Serialize, Deserialize, Clone)]
pub struct KarmaArgs {
    pub cars: Vec<Car>,
    pub target: u32,
    pub lap_count: usize,
    pub player_count: usize,
    pub timeout_ms: f64,
    pub tolerance_percent: f64,
}

/// Result type for karma calculation containing subsets, similarity, target, lap count, and player count.
type KarmaResult = Result<(Vec<Vec<usize>>, f64, u32, usize, usize), String>;

/// Worker reactor that processes karma calculation requests.
///
/// Receives `KarmaArgs` and returns either:
/// - `Ok((subsets, similarity, target, lap_count, player_count))` on success
/// - `Err(error_message)` on failure
#[reactor]
pub async fn KarmaTask(mut scope: ReactorScope<KarmaArgs, KarmaResult>) {
    while let Some(args) = scope.next().await {
        let res = (|| {
            let sets = perform_multiple_runs(
                &args.cars,
                args.target,
                args.lap_count,
                args.player_count,
                args.timeout_ms,
                args.tolerance_percent,
            )
            .map_err(|e| format!("{}", e))?;

            let sim = compute_jaccard_similarity(&sets).unwrap_or(0.0); // Default to 0 similarity if calculation fails

            Ok((sets, sim, args.target, args.lap_count, args.player_count))
        })();

        // abort loop if all bridges dropped
        if scope.send(res).await.is_err() {
            break;
        }
    }
}
