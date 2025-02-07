use super::*;
use rayon::prelude::*;
use std::fmt::{Debug, Display, Formatter};
use std::path::{Path, PathBuf};
use types::ForkName;

mod bls_aggregate_sigs;
mod bls_aggregate_verify;
mod bls_batch_verify;
mod bls_eth_aggregate_pubkeys;
mod bls_eth_fast_aggregate_verify;
mod bls_fast_aggregate_verify;
mod bls_sign_msg;
mod bls_verify_msg;
mod common;
mod compute_columns_for_custody_groups;
mod epoch_processing;
mod fork;
mod fork_choice;
mod genesis_initialization;
mod genesis_validity;
mod get_custody_groups;
mod kzg_blob_to_kzg_commitment;
mod kzg_compute_blob_kzg_proof;
mod kzg_compute_cells_and_kzg_proofs;
mod kzg_compute_kzg_proof;
mod kzg_recover_cells_and_kzg_proofs;
mod kzg_verify_blob_kzg_proof;
mod kzg_verify_blob_kzg_proof_batch;
mod kzg_verify_cell_kzg_proof_batch;
mod kzg_verify_kzg_proof;
mod light_client_verify_is_better_update;
mod merkle_proof_validity;
mod operations;
mod rewards;
mod sanity_blocks;
mod sanity_slots;
mod shuffling;
mod ssz_generic;
mod ssz_static;
mod transition;

pub use self::fork_choice::*;
pub use bls_aggregate_sigs::*;
pub use bls_aggregate_verify::*;
pub use bls_batch_verify::*;
pub use bls_eth_aggregate_pubkeys::*;
pub use bls_eth_fast_aggregate_verify::*;
pub use bls_fast_aggregate_verify::*;
pub use bls_sign_msg::*;
pub use bls_verify_msg::*;
pub use common::SszStaticType;
pub use compute_columns_for_custody_groups::*;
pub use epoch_processing::*;
pub use fork::ForkTest;
pub use genesis_initialization::*;
pub use genesis_validity::*;
pub use get_custody_groups::*;
pub use kzg_blob_to_kzg_commitment::*;
pub use kzg_compute_blob_kzg_proof::*;
pub use kzg_compute_cells_and_kzg_proofs::*;
pub use kzg_compute_kzg_proof::*;
pub use kzg_recover_cells_and_kzg_proofs::*;
pub use kzg_verify_blob_kzg_proof::*;
pub use kzg_verify_blob_kzg_proof_batch::*;
pub use kzg_verify_cell_kzg_proof_batch::*;
pub use kzg_verify_kzg_proof::*;
pub use light_client_verify_is_better_update::*;
pub use merkle_proof_validity::*;
pub use operations::*;
pub use rewards::RewardsTest;
pub use sanity_blocks::*;
pub use sanity_slots::*;
pub use shuffling::*;
pub use ssz_generic::*;
pub use ssz_static::*;
pub use transition::TransitionTest;

/// Used for running feature tests for future forks that have not yet been added to `ForkName`.
/// This runs tests in the directory named by the feature instead of the fork name. This has been
/// the pattern used in the `consensus-spec-tests` repository:
/// `consensus-spec-tests/tests/general/[feature_name]/[runner_name].`
/// e.g. consensus-spec-tests/tests/general/peerdas/ssz_static
///
/// The feature tests can be run with one of the following methods:
/// 1. `handler.run_for_feature(feature_name)` for new tests that are not on existing fork, i.e. a
///     new handler. This will be temporary and the test will need to be updated to use
///     `handle.run()` once the feature is incorporated into a fork.
/// 2. `handler.run()` for tests that are already on existing forks, but with new test vectors for
///     the feature. In this case the `handler.is_enabled_for_feature` will need to be implemented
///     to return `true` for the feature in order for the feature test vector to be tested.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum FeatureName {
    // TODO(fulu): to be removed once we start using Fulu types for test vectors.
    // Existing SSZ types for PeerDAS (Fulu) are the same as Electra, so the test vectors get
    // loaded as Electra types (default serde behaviour for untagged enums).
    Fulu,
}

impl FeatureName {
    pub fn list_all() -> Vec<FeatureName> {
        vec![FeatureName::Fulu]
    }

    /// `ForkName` to use when running the feature tests.
    pub fn fork_name(&self) -> ForkName {
        match self {
            FeatureName::Fulu => ForkName::Electra,
        }
    }
}

impl Display for FeatureName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            FeatureName::Fulu => f.write_str("fulu"),
        }
    }
}

pub trait LoadCase: Sized {
    /// Load the test case from a test case directory.
    fn load_from_dir(_path: &Path, _fork_name: ForkName) -> Result<Self, Error>;
}

pub trait Case: Debug + Sync {
    /// An optional field for implementing a custom description.
    ///
    /// Defaults to "no description".
    fn description(&self) -> String {
        "no description".to_string()
    }

    /// Whether or not this test exists for the given `fork_name`.
    ///
    /// Returns `true` by default.
    fn is_enabled_for_fork(_fork_name: ForkName) -> bool {
        true
    }

    /// Whether or not this test exists for the given `feature_name`. This is intended to be used
    /// for features that have not been added to a fork yet, and there is usually a separate folder
    /// for the feature in the `consensus-spec-tests` repository.
    ///
    /// Returns `false` by default.
    fn is_enabled_for_feature(_feature_name: FeatureName) -> bool {
        false
    }

    /// Execute a test and return the result.
    ///
    /// `case_index` reports the index of the case in the set of test cases. It is not strictly
    /// necessary, but it's useful when troubleshooting specific failing tests.
    fn result(&self, case_index: usize, fork_name: ForkName) -> Result<(), Error>;
}

#[derive(Debug)]
pub struct Cases<T> {
    pub test_cases: Vec<(PathBuf, T)>,
}

impl<T: Case> Cases<T> {
    pub fn test_results(&self, fork_name: ForkName, use_rayon: bool) -> Vec<CaseResult> {
        if use_rayon {
            self.test_cases
                .into_par_iter()
                .enumerate()
                .map(|(i, (ref path, ref tc))| {
                    CaseResult::new(i, path, tc, tc.result(i, fork_name))
                })
                .collect()
        } else {
            self.test_cases
                .iter()
                .enumerate()
                .map(|(i, (ref path, ref tc))| {
                    CaseResult::new(i, path, tc, tc.result(i, fork_name))
                })
                .collect()
        }
    }
}
