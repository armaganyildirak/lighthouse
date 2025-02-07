use crate::common::{altair::BaseRewardPerIncrement, decrease_balance, increase_balance};
use crate::per_block_processing::errors::{BlockProcessingError, SyncAggregateInvalid};
use crate::{signature_sets::sync_aggregate_signature_set, VerifySignatures};
use safe_arith::SafeArith;
use std::borrow::Cow;
use types::consts::altair::{PROPOSER_WEIGHT, SYNC_REWARD_WEIGHT, WEIGHT_DENOMINATOR};
use types::{
    BeaconState, BeaconStateError, ChainSpec, EthSpec, PublicKeyBytes, SyncAggregate, Unsigned,
};

pub fn process_sync_aggregate<E: EthSpec>(
    state: &mut BeaconState<E>,
    aggregate: &SyncAggregate<E>,
    proposer_index: u64,
    verify_signatures: VerifySignatures,
    spec: &ChainSpec,
) -> Result<(), BlockProcessingError> {
    let current_sync_committee = state.current_sync_committee()?.clone();

    // Verify sync committee aggregate signature signing over the previous slot block root
    if verify_signatures.is_true() {
        // This decompression could be avoided with a cache, but we're not likely
        // to encounter this case in practice due to the use of pre-emptive signature
        // verification (which uses the `ValidatorPubkeyCache`).
        let decompressor = |pk_bytes: &PublicKeyBytes| pk_bytes.decompress().ok().map(Cow::Owned);

        // Check that the signature is over the previous block root.
        let previous_slot = state.slot().saturating_sub(1u64);
        let previous_block_root = *state.get_block_root(previous_slot)?;

        let signature_set = sync_aggregate_signature_set(
            decompressor,
            aggregate,
            state.slot(),
            previous_block_root,
            state,
            spec,
        )?;

        // If signature set is `None` then the signature is valid (infinity).
        if signature_set.is_some_and(|signature| !signature.verify()) {
            return Err(SyncAggregateInvalid::SignatureInvalid.into());
        }
    }

    // Compute participant and proposer rewards
    let (participant_reward, proposer_reward) = compute_sync_aggregate_rewards(state, spec)?;

    // Apply participant and proposer rewards
    let committee_indices = state.get_sync_committee_indices(&current_sync_committee)?;

    let proposer_index = proposer_index as usize;
    let mut proposer_balance = *state
        .balances()
        .get(proposer_index)
        .ok_or(BeaconStateError::BalancesOutOfBounds(proposer_index))?;

    for (participant_index, participation_bit) in committee_indices
        .into_iter()
        .zip(aggregate.sync_committee_bits.iter())
    {
        if participation_bit {
            // Accumulate proposer rewards in a temp var in case the proposer has very low balance, is
            // part of the sync committee, does not participate and its penalties saturate.
            if participant_index == proposer_index {
                proposer_balance.safe_add_assign(participant_reward)?;
            } else {
                increase_balance(state, participant_index, participant_reward)?;
            }
            proposer_balance.safe_add_assign(proposer_reward)?;
        } else if participant_index == proposer_index {
            proposer_balance = proposer_balance.saturating_sub(participant_reward);
        } else {
            decrease_balance(state, participant_index, participant_reward)?;
        }
    }

    *state.get_balance_mut(proposer_index)? = proposer_balance;

    Ok(())
}

/// Compute the `(participant_reward, proposer_reward)` for a sync aggregate.
///
/// The `state` should be the pre-state from the same slot as the block containing the aggregate.
pub fn compute_sync_aggregate_rewards<E: EthSpec>(
    state: &BeaconState<E>,
    spec: &ChainSpec,
) -> Result<(u64, u64), BlockProcessingError> {
    let total_active_balance = state.get_total_active_balance()?;
    let total_active_increments =
        total_active_balance.safe_div(spec.effective_balance_increment)?;
    let total_base_rewards = BaseRewardPerIncrement::new(total_active_balance, spec)?
        .as_u64()
        .safe_mul(total_active_increments)?;
    let max_participant_rewards = total_base_rewards
        .safe_mul(SYNC_REWARD_WEIGHT)?
        .safe_div(WEIGHT_DENOMINATOR)?
        .safe_div(E::slots_per_epoch())?;
    let participant_reward = max_participant_rewards.safe_div(E::SyncCommitteeSize::to_u64())?;
    let proposer_reward = participant_reward
        .safe_mul(PROPOSER_WEIGHT)?
        .safe_div(WEIGHT_DENOMINATOR.safe_sub(PROPOSER_WEIGHT)?)?;
    Ok((participant_reward, proposer_reward))
}
