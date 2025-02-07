//! Contains logic about identifying which Sync to perform given PeerSyncInfo of ourselves and
//! of a remote.

use beacon_chain::{BeaconChain, BeaconChainTypes};
use lighthouse_network::SyncInfo;

/// The type of Range sync that should be done relative to our current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeSyncType {
    /// A finalized chain sync should be started with this peer.
    Finalized,
    /// A head chain sync should be started with this peer.
    Head,
}

impl RangeSyncType {
    /// Determines the type of sync given our local `PeerSyncInfo` and the remote's
    /// `PeerSyncInfo`.
    pub fn new<T: BeaconChainTypes>(
        chain: &BeaconChain<T>,
        local_info: &SyncInfo,
        remote_info: &SyncInfo,
    ) -> RangeSyncType {
        // Check for finalized chain sync
        //
        // The condition is:
        // -  The remotes finalized epoch is greater than our current finalized epoch and we have
        //    not seen the finalized hash before.

        if remote_info.finalized_epoch > local_info.finalized_epoch
            && !chain.block_is_known_to_fork_choice(&remote_info.finalized_root)
        {
            RangeSyncType::Finalized
        } else {
            RangeSyncType::Head
        }
    }

    /// Get a `str` representation of the `RangeSyncType`.
    pub fn as_str(&self) -> &'static str {
        match self {
            RangeSyncType::Finalized => "Finalized",
            RangeSyncType::Head => "Head",
        }
    }
}
