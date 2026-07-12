//! Web Worker agent for offloading karma calculations to background threads.

use crate::{compute_jaccard_similarity, perform_multiple_runs_with_strategy, Car, SolverStrategy};
use futures::sink::SinkExt;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use yew_agent::reactor::{reactor, ReactorScope};

/// Complete identity of a worker request. Echoed for both success and failure
/// so callers can reject responses from superseded requests or datasets.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RequestMetadata {
    pub request_id: u64,
    pub dataset_generation: u64,
    pub target: u32,
    pub lap_count: usize,
    pub player_count: usize,
    pub timeout_ms: f64,
    pub tolerance_percent: f64,
    pub strategy: SolverStrategy,
}

/// Arguments for karma calculation tasks sent to workers.
#[derive(Serialize, Deserialize, Clone)]
pub struct KarmaArgs {
    pub cars: Vec<Car>,
    pub metadata: RequestMetadata,
}

/// A successful worker calculation with its complete request identity.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct KarmaSuccess {
    pub metadata: RequestMetadata,
    pub sets: Vec<Vec<usize>>,
    pub similarity: f64,
    pub calculated_target: u32,
}

/// A failed worker calculation with its complete request identity.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct KarmaFailure {
    pub metadata: RequestMetadata,
    pub error: String,
}

/// Worker responses always include full request metadata, including errors.
pub type KarmaResult = Result<KarmaSuccess, KarmaFailure>;

/// Worker reactor that processes karma calculation requests.
#[reactor]
pub async fn KarmaTask(mut scope: ReactorScope<KarmaArgs, KarmaResult>) {
    while let Some(args) = scope.next().await {
        let metadata = args.metadata.clone();
        let res = (|| {
            let sets = perform_multiple_runs_with_strategy(
                metadata.strategy,
                &args.cars,
                metadata.target,
                metadata.lap_count,
                metadata.player_count,
                metadata.timeout_ms,
                metadata.tolerance_percent,
            )
            .map_err(|e| KarmaFailure {
                metadata: metadata.clone(),
                error: e.to_string(),
            })?;

            let similarity = compute_jaccard_similarity(&sets).unwrap_or(0.0);
            Ok(KarmaSuccess {
                metadata,
                sets,
                similarity,
                calculated_target: args.metadata.target,
            })
        })();

        // Abort loop if all bridges dropped.
        if scope.send(res).await.is_err() {
            break;
        }
    }
}
