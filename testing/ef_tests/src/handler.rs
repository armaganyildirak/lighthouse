use crate::cases::{self, Case, Cases, EpochTransition, LoadCase, Operation};
use crate::type_name::TypeName;
use crate::{type_name, FeatureName};
use derivative::Derivative;
use std::fs::{self, DirEntry};
use std::marker::PhantomData;
use std::path::PathBuf;
use types::{BeaconState, EthSpec, ForkName};

pub trait Handler {
    type Case: Case + LoadCase;

    fn config_name() -> &'static str {
        "general"
    }

    fn runner_name() -> &'static str;

    fn handler_name(&self) -> String;

    // Add forks here to exclude them from EF spec testing. Helpful for adding future or
    // unspecified forks.
    fn disabled_forks(&self) -> Vec<ForkName> {
        vec![ForkName::Fulu]
    }

    fn is_enabled_for_fork(&self, fork_name: ForkName) -> bool {
        Self::Case::is_enabled_for_fork(fork_name)
    }

    fn is_enabled_for_feature(&self, feature_name: FeatureName) -> bool {
        Self::Case::is_enabled_for_feature(feature_name)
    }

    fn run(&self) {
        for fork_name in ForkName::list_all() {
            if !self.disabled_forks().contains(&fork_name) && self.is_enabled_for_fork(fork_name) {
                self.run_for_fork(fork_name);
            }
        }

        // Run feature tests for future forks that are not yet added to `ForkName`.
        // This runs tests in the directory named by the feature instead of the fork name.
        // e.g. consensus-spec-tests/tests/general/[feature_name]/[runner_name]
        // e.g. consensus-spec-tests/tests/general/peerdas/ssz_static
        for feature_name in FeatureName::list_all() {
            if self.is_enabled_for_feature(feature_name) {
                self.run_for_feature(feature_name);
            }
        }
    }

    fn use_rayon() -> bool {
        true
    }

    fn run_for_fork(&self, fork_name: ForkName) {
        let fork_name_str = fork_name.to_string();

        let handler_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("consensus-spec-tests")
            .join("tests")
            .join(Self::config_name())
            .join(&fork_name_str)
            .join(Self::runner_name())
            .join(self.handler_name());

        // Iterate through test suites
        let as_directory = |entry: Result<DirEntry, std::io::Error>| -> Option<DirEntry> {
            entry
                .ok()
                .filter(|e| e.file_type().map(|ty| ty.is_dir()).unwrap())
        };

        let test_cases = fs::read_dir(&handler_path)
            .unwrap_or_else(|e| panic!("handler dir {} exists: {:?}", handler_path.display(), e))
            .filter_map(as_directory)
            .flat_map(|suite| fs::read_dir(suite.path()).expect("suite dir exists"))
            .filter_map(as_directory)
            .map(|test_case_dir| {
                let path = test_case_dir.path();

                let case = Self::Case::load_from_dir(&path, fork_name).expect("test should load");
                (path, case)
            })
            .collect();

        let results = Cases { test_cases }.test_results(fork_name, Self::use_rayon());

        let name = format!(
            "{}/{}/{}",
            fork_name_str,
            Self::runner_name(),
            self.handler_name()
        );
        crate::results::assert_tests_pass(&name, &handler_path, &results);
    }

    fn run_for_feature(&self, feature_name: FeatureName) {
        let feature_name_str = feature_name.to_string();
        let fork_name = feature_name.fork_name();

        let handler_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("consensus-spec-tests")
            .join("tests")
            .join(Self::config_name())
            .join(&feature_name_str)
            .join(Self::runner_name())
            .join(self.handler_name());

        // Iterate through test suites
        let as_directory = |entry: Result<DirEntry, std::io::Error>| -> Option<DirEntry> {
            entry
                .ok()
                .filter(|e| e.file_type().map(|ty| ty.is_dir()).unwrap())
        };

        let test_cases = fs::read_dir(&handler_path)
            .unwrap_or_else(|e| panic!("handler dir {} exists: {:?}", handler_path.display(), e))
            .filter_map(as_directory)
            .flat_map(|suite| fs::read_dir(suite.path()).expect("suite dir exists"))
            .filter_map(as_directory)
            .map(|test_case_dir| {
                let path = test_case_dir.path();
                let case = Self::Case::load_from_dir(&path, fork_name).expect("test should load");
                (path, case)
            })
            .collect();

        let results = Cases { test_cases }.test_results(fork_name, Self::use_rayon());

        let name = format!(
            "{}/{}/{}",
            feature_name_str,
            Self::runner_name(),
            self.handler_name()
        );
        crate::results::assert_tests_pass(&name, &handler_path, &results);
    }
}

macro_rules! bls_eth_handler {
    ($runner_name: ident, $case_name:ident, $handler_name:expr) => {
        #[derive(Derivative)]
        #[derivative(Default(bound = ""))]
        pub struct $runner_name;

        impl Handler for $runner_name {
            type Case = cases::$case_name;

            fn runner_name() -> &'static str {
                "bls"
            }

            fn handler_name(&self) -> String {
                $handler_name.into()
            }
        }
    };
}

macro_rules! bls_handler {
    ($runner_name: ident, $case_name:ident, $handler_name:expr) => {
        #[derive(Derivative)]
        #[derivative(Default(bound = ""))]
        pub struct $runner_name;

        impl Handler for $runner_name {
            type Case = cases::$case_name;

            fn runner_name() -> &'static str {
                "bls"
            }

            fn config_name() -> &'static str {
                "bls12-381-tests"
            }

            fn handler_name(&self) -> String {
                $handler_name.into()
            }

            fn run(&self) {
                let fork_name = ForkName::Base;
                let fork_name_str = fork_name.to_string();
                let handler_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("consensus-spec-tests")
                    .join(Self::config_name())
                    .join(self.handler_name());

                let as_file = |entry: Result<DirEntry, std::io::Error>| -> Option<DirEntry> {
                    entry
                        .ok()
                        .filter(|e| e.file_type().map(|ty| ty.is_file()).unwrap_or(false))
                };
                let test_cases: Vec<(PathBuf, Self::Case)> = fs::read_dir(&handler_path)
                    .expect("handler dir exists")
                    .filter_map(as_file)
                    .map(|test_case_path| {
                        let path = test_case_path.path();
                        let case =
                            Self::Case::load_from_dir(&path, fork_name).expect("test should load");

                        (path, case)
                    })
                    .collect();

                let results = Cases { test_cases }.test_results(fork_name, Self::use_rayon());

                let name = format!(
                    "{}/{}/{}",
                    fork_name_str,
                    Self::runner_name(),
                    self.handler_name()
                );
                crate::results::assert_tests_pass(&name, &handler_path, &results);
            }
        }
    };
}

bls_handler!(BlsAggregateSigsHandler, BlsAggregateSigs, "aggregate");
bls_handler!(BlsSignMsgHandler, BlsSign, "sign");
bls_handler!(BlsBatchVerifyHandler, BlsBatchVerify, "batch_verify");
bls_handler!(BlsVerifyMsgHandler, BlsVerify, "verify");
bls_handler!(
    BlsAggregateVerifyHandler,
    BlsAggregateVerify,
    "aggregate_verify"
);
bls_handler!(
    BlsFastAggregateVerifyHandler,
    BlsFastAggregateVerify,
    "fast_aggregate_verify"
);
bls_eth_handler!(
    BlsEthAggregatePubkeysHandler,
    BlsEthAggregatePubkeys,
    "eth_aggregate_pubkeys"
);
bls_eth_handler!(
    BlsEthFastAggregateVerifyHandler,
    BlsEthFastAggregateVerify,
    "eth_fast_aggregate_verify"
);

/// Handler for SSZ types.
pub struct SszStaticHandler<T, E> {
    supported_forks: Vec<ForkName>,
    _phantom: PhantomData<(T, E)>,
}

impl<T, E> Default for SszStaticHandler<T, E> {
    fn default() -> Self {
        Self::for_forks(ForkName::list_all())
    }
}

impl<T, E> SszStaticHandler<T, E> {
    pub fn for_forks(supported_forks: Vec<ForkName>) -> Self {
        SszStaticHandler {
            supported_forks,
            _phantom: PhantomData,
        }
    }

    pub fn base_only() -> Self {
        Self::for_forks(vec![ForkName::Base])
    }

    pub fn altair_only() -> Self {
        Self::for_forks(vec![ForkName::Altair])
    }

    pub fn bellatrix_only() -> Self {
        Self::for_forks(vec![ForkName::Bellatrix])
    }

    pub fn capella_only() -> Self {
        Self::for_forks(vec![ForkName::Capella])
    }

    pub fn deneb_only() -> Self {
        Self::for_forks(vec![ForkName::Deneb])
    }

    pub fn electra_only() -> Self {
        Self::for_forks(vec![ForkName::Electra])
    }

    pub fn fulu_only() -> Self {
        Self::for_forks(vec![ForkName::Fulu])
    }

    pub fn altair_and_later() -> Self {
        Self::for_forks(ForkName::list_all()[1..].to_vec())
    }

    pub fn merge_and_later() -> Self {
        Self::for_forks(ForkName::list_all()[2..].to_vec())
    }

    pub fn capella_and_later() -> Self {
        Self::for_forks(ForkName::list_all()[3..].to_vec())
    }

    pub fn deneb_and_later() -> Self {
        Self::for_forks(ForkName::list_all()[4..].to_vec())
    }

    pub fn electra_and_later() -> Self {
        Self::for_forks(ForkName::list_all()[5..].to_vec())
    }

    pub fn fulu_and_later() -> Self {
        Self::for_forks(ForkName::list_all()[6..].to_vec())
    }

    pub fn pre_electra() -> Self {
        Self::for_forks(ForkName::list_all()[0..5].to_vec())
    }
}

/// Handler for SSZ types that implement `CachedTreeHash`.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct SszStaticTHCHandler<T, E>(PhantomData<(T, E)>);

/// Handler for SSZ types that don't implement `ssz::Decode`.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct SszStaticWithSpecHandler<T, E>(PhantomData<(T, E)>);

impl<T, E> Handler for SszStaticHandler<T, E>
where
    T: cases::SszStaticType + tree_hash::TreeHash + ssz::Decode + TypeName,
    E: TypeName,
{
    type Case = cases::SszStatic<T>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "ssz_static"
    }

    fn handler_name(&self) -> String {
        T::name().into()
    }

    fn is_enabled_for_fork(&self, fork_name: ForkName) -> bool {
        self.supported_forks.contains(&fork_name)
    }

    fn is_enabled_for_feature(&self, feature_name: FeatureName) -> bool {
        // TODO(fulu): to be removed once Fulu types start differing from Electra. We currently run Fulu tests as a
        // "feature" - this means we use Electra types for Fulu SSZ tests (except for PeerDAS types, e.g. `DataColumnSidecar`).
        //
        // This ensures we only run the tests **once** for `Fulu`, using the types matching the
        // correct fork, e.g. `Fulu` uses SSZ types from `Electra` as of spec test version
        // `v1.5.0-beta.0`, therefore the `Fulu` tests should get included when testing Deneb types.
        //
        // e.g. Fulu test vectors are executed in the 2nd line below, but excluded in the 1st
        // line when testing the type `AttestationElectra`:
        //
        // ```
        // SszStaticHandler::<AttestationBase<MainnetEthSpec>, MainnetEthSpec>::pre_electra().run();
        // SszStaticHandler::<AttestationElectra<MainnetEthSpec>, MainnetEthSpec>::electra_only().run();
        // ```
        feature_name == FeatureName::Fulu
            && self.supported_forks.contains(&feature_name.fork_name())
    }
}

impl<E> Handler for SszStaticTHCHandler<BeaconState<E>, E>
where
    E: EthSpec + TypeName,
{
    type Case = cases::SszStaticTHC<BeaconState<E>>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "ssz_static"
    }

    fn handler_name(&self) -> String {
        BeaconState::<E>::name().into()
    }

    fn is_enabled_for_feature(&self, feature_name: FeatureName) -> bool {
        feature_name == FeatureName::Fulu
    }
}

impl<T, E> Handler for SszStaticWithSpecHandler<T, E>
where
    T: TypeName,
    E: EthSpec + TypeName,
    cases::SszStaticWithSpec<T>: Case + LoadCase,
{
    type Case = cases::SszStaticWithSpec<T>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "ssz_static"
    }

    fn handler_name(&self) -> String {
        T::name().into()
    }

    fn is_enabled_for_feature(&self, feature_name: FeatureName) -> bool {
        feature_name == FeatureName::Fulu
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct ShufflingHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for ShufflingHandler<E> {
    type Case = cases::Shuffling<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "shuffling"
    }

    fn handler_name(&self) -> String {
        "core".into()
    }

    fn is_enabled_for_fork(&self, fork_name: ForkName) -> bool {
        fork_name == ForkName::Base
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct SanityBlocksHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for SanityBlocksHandler<E> {
    type Case = cases::SanityBlocks<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "sanity"
    }

    fn handler_name(&self) -> String {
        "blocks".into()
    }

    fn is_enabled_for_fork(&self, _fork_name: ForkName) -> bool {
        // NOTE: v1.1.0-beta.4 doesn't mark the historical blocks test as requiring real crypto, so
        // only run these tests with real crypto for now.
        cfg!(not(feature = "fake_crypto"))
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct SanitySlotsHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for SanitySlotsHandler<E> {
    type Case = cases::SanitySlots<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "sanity"
    }

    fn handler_name(&self) -> String {
        "slots".into()
    }

    fn is_enabled_for_fork(&self, fork_name: ForkName) -> bool {
        // Some sanity tests compute sync committees, which requires real crypto.
        fork_name == ForkName::Base || cfg!(not(feature = "fake_crypto"))
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct RandomHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for RandomHandler<E> {
    type Case = cases::SanityBlocks<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "random"
    }

    fn handler_name(&self) -> String {
        "random".into()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct EpochProcessingHandler<E, T>(PhantomData<(E, T)>);

impl<E: EthSpec + TypeName, T: EpochTransition<E>> Handler for EpochProcessingHandler<E, T> {
    type Case = cases::EpochProcessing<E, T>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "epoch_processing"
    }

    fn handler_name(&self) -> String {
        T::name().into()
    }
}

pub struct RewardsHandler<E: EthSpec> {
    handler_name: &'static str,
    _phantom: PhantomData<E>,
}

impl<E: EthSpec> RewardsHandler<E> {
    pub fn new(handler_name: &'static str) -> Self {
        Self {
            handler_name,
            _phantom: PhantomData,
        }
    }
}

impl<E: EthSpec + TypeName> Handler for RewardsHandler<E> {
    type Case = cases::RewardsTest<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "rewards"
    }

    fn handler_name(&self) -> String {
        self.handler_name.to_string()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct ForkHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for ForkHandler<E> {
    type Case = cases::ForkTest<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "fork"
    }

    fn handler_name(&self) -> String {
        "fork".into()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct TransitionHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for TransitionHandler<E> {
    type Case = cases::TransitionTest<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "transition"
    }

    fn handler_name(&self) -> String {
        "core".into()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct FinalityHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for FinalityHandler<E> {
    // Reuse the blocks case runner.
    type Case = cases::SanityBlocks<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "finality"
    }

    fn handler_name(&self) -> String {
        "finality".into()
    }
}

pub struct ForkChoiceHandler<E> {
    handler_name: String,
    _phantom: PhantomData<E>,
}

impl<E: EthSpec> ForkChoiceHandler<E> {
    pub fn new(handler_name: &str) -> Self {
        Self {
            handler_name: handler_name.into(),
            _phantom: PhantomData,
        }
    }
}

impl<E: EthSpec + TypeName> Handler for ForkChoiceHandler<E> {
    type Case = cases::ForkChoiceTest<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "fork_choice"
    }

    fn handler_name(&self) -> String {
        self.handler_name.clone()
    }

    fn use_rayon() -> bool {
        // The fork choice tests use `block_on` which can cause panics with rayon.
        false
    }

    fn is_enabled_for_fork(&self, fork_name: ForkName) -> bool {
        // We no longer run on_merge_block tests since removing merge support.
        if self.handler_name == "on_merge_block" {
            return false;
        }

        // Tests are no longer generated for the base/phase0 specification.
        if fork_name == ForkName::Base {
            return false;
        }

        // No FCU override tests prior to bellatrix.
        if self.handler_name == "should_override_forkchoice_update"
            && !fork_name.bellatrix_enabled()
        {
            return false;
        }

        // These tests check block validity (which may include signatures) and there is no need to
        // run them with fake crypto.
        cfg!(not(feature = "fake_crypto"))
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct OptimisticSyncHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for OptimisticSyncHandler<E> {
    type Case = cases::ForkChoiceTest<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "sync"
    }

    fn handler_name(&self) -> String {
        "optimistic".into()
    }

    fn use_rayon() -> bool {
        // The opt sync tests use `block_on` which can cause panics with rayon.
        false
    }

    fn is_enabled_for_fork(&self, fork_name: ForkName) -> bool {
        fork_name.bellatrix_enabled() && cfg!(not(feature = "fake_crypto"))
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct GenesisValidityHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for GenesisValidityHandler<E> {
    type Case = cases::GenesisValidity<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "genesis"
    }

    fn handler_name(&self) -> String {
        "validity".into()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct GenesisInitializationHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for GenesisInitializationHandler<E> {
    type Case = cases::GenesisInitialization<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "genesis"
    }

    fn handler_name(&self) -> String {
        "initialization".into()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct KZGBlobToKZGCommitmentHandler<E>(PhantomData<E>);

impl<E: EthSpec> Handler for KZGBlobToKZGCommitmentHandler<E> {
    type Case = cases::KZGBlobToKZGCommitment<E>;

    fn config_name() -> &'static str {
        "general"
    }

    fn runner_name() -> &'static str {
        "kzg"
    }

    fn handler_name(&self) -> String {
        "blob_to_kzg_commitment".into()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct KZGComputeBlobKZGProofHandler<E>(PhantomData<E>);

impl<E: EthSpec> Handler for KZGComputeBlobKZGProofHandler<E> {
    type Case = cases::KZGComputeBlobKZGProof<E>;

    fn config_name() -> &'static str {
        "general"
    }

    fn runner_name() -> &'static str {
        "kzg"
    }

    fn handler_name(&self) -> String {
        "compute_blob_kzg_proof".into()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct KZGComputeKZGProofHandler<E>(PhantomData<E>);

impl<E: EthSpec> Handler for KZGComputeKZGProofHandler<E> {
    type Case = cases::KZGComputeKZGProof<E>;

    fn config_name() -> &'static str {
        "general"
    }

    fn runner_name() -> &'static str {
        "kzg"
    }

    fn handler_name(&self) -> String {
        "compute_kzg_proof".into()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct KZGVerifyBlobKZGProofHandler<E>(PhantomData<E>);

impl<E: EthSpec> Handler for KZGVerifyBlobKZGProofHandler<E> {
    type Case = cases::KZGVerifyBlobKZGProof<E>;

    fn config_name() -> &'static str {
        "general"
    }

    fn runner_name() -> &'static str {
        "kzg"
    }

    fn handler_name(&self) -> String {
        "verify_blob_kzg_proof".into()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct KZGVerifyBlobKZGProofBatchHandler<E>(PhantomData<E>);

impl<E: EthSpec> Handler for KZGVerifyBlobKZGProofBatchHandler<E> {
    type Case = cases::KZGVerifyBlobKZGProofBatch<E>;

    fn config_name() -> &'static str {
        "general"
    }

    fn runner_name() -> &'static str {
        "kzg"
    }

    fn handler_name(&self) -> String {
        "verify_blob_kzg_proof_batch".into()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct KZGVerifyKZGProofHandler<E>(PhantomData<E>);

impl<E: EthSpec> Handler for KZGVerifyKZGProofHandler<E> {
    type Case = cases::KZGVerifyKZGProof<E>;

    fn config_name() -> &'static str {
        "general"
    }

    fn runner_name() -> &'static str {
        "kzg"
    }

    fn handler_name(&self) -> String {
        "verify_kzg_proof".into()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct GetCustodyGroupsHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for GetCustodyGroupsHandler<E> {
    type Case = cases::GetCustodyGroups<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "networking"
    }

    fn handler_name(&self) -> String {
        "get_custody_groups".into()
    }

    fn is_enabled_for_feature(&self, feature_name: FeatureName) -> bool {
        feature_name == FeatureName::Fulu
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct ComputeColumnsForCustodyGroupHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for ComputeColumnsForCustodyGroupHandler<E> {
    type Case = cases::ComputeColumnsForCustodyGroups<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "networking"
    }

    fn handler_name(&self) -> String {
        "compute_columns_for_custody_group".into()
    }

    fn is_enabled_for_feature(&self, feature_name: FeatureName) -> bool {
        feature_name == FeatureName::Fulu
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct KZGComputeCellsAndKZGProofHandler<E>(PhantomData<E>);

impl<E: EthSpec> Handler for KZGComputeCellsAndKZGProofHandler<E> {
    type Case = cases::KZGComputeCellsAndKZGProofs<E>;

    fn config_name() -> &'static str {
        "general"
    }

    fn runner_name() -> &'static str {
        "kzg"
    }

    fn handler_name(&self) -> String {
        "compute_cells_and_kzg_proofs".into()
    }

    fn is_enabled_for_feature(&self, feature_name: FeatureName) -> bool {
        feature_name == FeatureName::Fulu
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct KZGVerifyCellKZGProofBatchHandler<E>(PhantomData<E>);

impl<E: EthSpec> Handler for KZGVerifyCellKZGProofBatchHandler<E> {
    type Case = cases::KZGVerifyCellKZGProofBatch<E>;

    fn config_name() -> &'static str {
        "general"
    }

    fn runner_name() -> &'static str {
        "kzg"
    }

    fn handler_name(&self) -> String {
        "verify_cell_kzg_proof_batch".into()
    }

    fn is_enabled_for_feature(&self, feature_name: FeatureName) -> bool {
        feature_name == FeatureName::Fulu
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct KZGRecoverCellsAndKZGProofHandler<E>(PhantomData<E>);

impl<E: EthSpec> Handler for KZGRecoverCellsAndKZGProofHandler<E> {
    type Case = cases::KZGRecoverCellsAndKZGProofs<E>;

    fn config_name() -> &'static str {
        "general"
    }

    fn runner_name() -> &'static str {
        "kzg"
    }

    fn handler_name(&self) -> String {
        "recover_cells_and_kzg_proofs".into()
    }

    fn is_enabled_for_feature(&self, feature_name: FeatureName) -> bool {
        feature_name == FeatureName::Fulu
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct BeaconStateMerkleProofValidityHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for BeaconStateMerkleProofValidityHandler<E> {
    type Case = cases::BeaconStateMerkleProofValidity<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "light_client"
    }

    fn handler_name(&self) -> String {
        "single_merkle_proof/BeaconState".into()
    }

    fn is_enabled_for_fork(&self, fork_name: ForkName) -> bool {
        fork_name.altair_enabled()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct KzgInclusionMerkleProofValidityHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for KzgInclusionMerkleProofValidityHandler<E> {
    type Case = cases::KzgInclusionMerkleProofValidity<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "merkle_proof"
    }

    fn handler_name(&self) -> String {
        "single_merkle_proof".into()
    }

    fn is_enabled_for_fork(&self, fork_name: ForkName) -> bool {
        fork_name.deneb_enabled()
    }

    fn is_enabled_for_feature(&self, feature_name: FeatureName) -> bool {
        feature_name == FeatureName::Fulu
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct BeaconBlockBodyMerkleProofValidityHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for BeaconBlockBodyMerkleProofValidityHandler<E> {
    type Case = cases::BeaconBlockBodyMerkleProofValidity<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "light_client"
    }

    fn handler_name(&self) -> String {
        "single_merkle_proof/BeaconBlockBody".into()
    }

    fn is_enabled_for_fork(&self, fork_name: ForkName) -> bool {
        fork_name.capella_enabled()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct LightClientUpdateHandler<E>(PhantomData<E>);

impl<E: EthSpec + TypeName> Handler for LightClientUpdateHandler<E> {
    type Case = cases::LightClientVerifyIsBetterUpdate<E>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "light_client"
    }

    fn handler_name(&self) -> String {
        "update_ranking".into()
    }

    fn is_enabled_for_fork(&self, fork_name: ForkName) -> bool {
        // Enabled in Altair
        fork_name.altair_enabled()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct OperationsHandler<E, O>(PhantomData<(E, O)>);

impl<E: EthSpec + TypeName, O: Operation<E>> Handler for OperationsHandler<E, O> {
    type Case = cases::Operations<E, O>;

    fn config_name() -> &'static str {
        E::name()
    }

    fn runner_name() -> &'static str {
        "operations"
    }

    fn handler_name(&self) -> String {
        O::handler_name()
    }
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct SszGenericHandler<H>(PhantomData<H>);

impl<H: TypeName> Handler for SszGenericHandler<H> {
    type Case = cases::SszGeneric;

    fn config_name() -> &'static str {
        "general"
    }

    fn runner_name() -> &'static str {
        "ssz_generic"
    }

    fn is_enabled_for_fork(&self, fork_name: ForkName) -> bool {
        // SSZ generic tests are genesis only
        fork_name == ForkName::Base
    }

    fn handler_name(&self) -> String {
        H::name().into()
    }
}

// Supported SSZ generic handlers
pub struct BasicVector;
type_name!(BasicVector, "basic_vector");
pub struct Bitlist;
type_name!(Bitlist, "bitlist");
pub struct Bitvector;
type_name!(Bitvector, "bitvector");
pub struct Boolean;
type_name!(Boolean, "boolean");
pub struct Uints;
type_name!(Uints, "uints");
pub struct Containers;
type_name!(Containers, "containers");
