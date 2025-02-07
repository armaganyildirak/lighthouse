//! The `SyncManager` facilities the block syncing logic of lighthouse. The current networking
//! specification provides two methods from which to obtain blocks from peers. The `BlocksByRange`
//! request and the `BlocksByRoot` request. The former is used to obtain a large number of
//! blocks and the latter allows for searching for blocks given a block-hash.
//!
//! These two RPC methods are designed for two type of syncing.
//! - Long range (batch) sync, when a client is out of date and needs to the latest head.
//! - Parent lookup - when a peer provides us a block whose parent is unknown to us.
//!
//! Both of these syncing strategies are built into the `SyncManager`.
//!
//! Currently the long-range (batch) syncing method functions by opportunistically downloading
//! batches blocks from all peers who know about a chain that we do not. When a new peer connects
//! which has a later head that is greater than `SLOT_IMPORT_TOLERANCE` from our current head slot,
//! the manager's state becomes `Syncing` and begins a batch syncing process with this peer. If
//! further peers connect, this process is run in parallel with those peers, until our head is
//! within `SLOT_IMPORT_TOLERANCE` of all connected peers.
//!
//! ## Batch Syncing
//!
//! See `RangeSync` for further details.
//!
//! ## Parent Lookup
//!
//! When a block with an unknown parent is received and we are in `Regular` sync mode, the block is
//! queued for lookup. A round-robin approach is used to request the parent from the known list of
//! fully sync'd peers. If `PARENT_FAIL_TOLERANCE` attempts at requesting the block fails, we
//! drop the propagated block and downvote the peer that sent it to us.
//!
//! Block Lookup
//!
//! To keep the logic maintained to the syncing thread (and manage the request_ids), when a block
//! needs to be searched for (i.e if an attestation references an unknown block) this manager can
//! search for the block and subsequently search for parents if needed.

use super::backfill_sync::{BackFillSync, ProcessResult, SyncStart};
use super::block_lookups::BlockLookups;
use super::network_context::{
    CustodyByRootResult, RangeBlockComponent, RangeRequestId, RpcEvent, SyncNetworkContext,
};
use super::peer_sampling::{Sampling, SamplingConfig, SamplingResult};
use super::peer_sync_info::{remote_sync_type, PeerSyncType};
use super::range_sync::{RangeSync, RangeSyncType, EPOCHS_PER_BATCH};
use crate::network_beacon_processor::{ChainSegmentProcessId, NetworkBeaconProcessor};
use crate::service::NetworkMessage;
use crate::status::ToStatusMessage;
use crate::sync::block_lookups::{
    BlobRequestState, BlockComponent, BlockRequestState, CustodyRequestState, DownloadResult,
};
use crate::sync::network_context::PeerGroup;
use beacon_chain::block_verification_types::AsBlock;
use beacon_chain::validator_monitor::timestamp_now;
use beacon_chain::{
    AvailabilityProcessingStatus, BeaconChain, BeaconChainTypes, BlockError, EngineState,
};
use futures::StreamExt;
use lighthouse_network::rpc::RPCError;
use lighthouse_network::service::api_types::{
    BlobsByRangeRequestId, BlocksByRangeRequestId, ComponentsByRangeRequestId, CustodyRequester,
    DataColumnsByRangeRequestId, DataColumnsByRootRequestId, DataColumnsByRootRequester, Id,
    SamplingId, SamplingRequester, SingleLookupReqId, SyncRequestId,
};
use lighthouse_network::types::{NetworkGlobals, SyncState};
use lighthouse_network::SyncInfo;
use lighthouse_network::{PeerAction, PeerId};
use lru_cache::LRUTimeCache;
use slog::{crit, debug, error, info, o, trace, warn, Logger};
use std::ops::Sub;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use types::{
    BlobSidecar, DataColumnSidecar, EthSpec, ForkContext, Hash256, SignedBeaconBlock, Slot,
};

#[cfg(test)]
use types::ColumnIndex;

/// The number of slots ahead of us that is allowed before requesting a long-range (batch)  Sync
/// from a peer. If a peer is within this tolerance (forwards or backwards), it is treated as a
/// fully sync'd peer.
///
/// This means that we consider ourselves synced (and hence subscribe to all subnets and block
/// gossip if no peers are further than this range ahead of us that we have not already downloaded
/// blocks for.
pub const SLOT_IMPORT_TOLERANCE: usize = 32;

/// Suppress duplicated `UnknownBlockHashFromAttestation` events for some duration of time. In
/// practice peers are likely to send the same root during a single slot. 30 seconds is a rather
/// arbitrary number that covers a full slot, but allows recovery if sync get stuck for a few slots.
const NOTIFIED_UNKNOWN_ROOT_EXPIRY_SECONDS: u64 = 30;

#[derive(Debug)]
/// A message that can be sent to the sync manager thread.
pub enum SyncMessage<E: EthSpec> {
    /// A useful peer has been discovered.
    AddPeer(PeerId, SyncInfo),

    /// Force trigger range sync for a set of peers given a head they claim to have imported. Used
    /// by block lookup to trigger range sync if a parent chain grows too large.
    AddPeersForceRangeSync {
        peers: Vec<PeerId>,
        head_root: Hash256,
        /// Sync lookup may not know the Slot of this head. However this situation is very rare.
        head_slot: Option<Slot>,
    },

    /// A block has been received from the RPC.
    RpcBlock {
        request_id: SyncRequestId,
        peer_id: PeerId,
        beacon_block: Option<Arc<SignedBeaconBlock<E>>>,
        seen_timestamp: Duration,
    },

    /// A blob has been received from the RPC.
    RpcBlob {
        request_id: SyncRequestId,
        peer_id: PeerId,
        blob_sidecar: Option<Arc<BlobSidecar<E>>>,
        seen_timestamp: Duration,
    },

    /// A data columns has been received from the RPC
    RpcDataColumn {
        request_id: SyncRequestId,
        peer_id: PeerId,
        data_column: Option<Arc<DataColumnSidecar<E>>>,
        seen_timestamp: Duration,
    },

    /// A block with an unknown parent has been received.
    UnknownParentBlock(PeerId, Arc<SignedBeaconBlock<E>>, Hash256),

    /// A blob with an unknown parent has been received.
    UnknownParentBlob(PeerId, Arc<BlobSidecar<E>>),

    /// A data column with an unknown parent has been received.
    UnknownParentDataColumn(PeerId, Arc<DataColumnSidecar<E>>),

    /// A peer has sent an attestation that references a block that is unknown. This triggers the
    /// manager to attempt to find the block matching the unknown hash.
    UnknownBlockHashFromAttestation(PeerId, Hash256),

    /// Request to start sampling a block. Caller should ensure that block has data before sending
    /// the request.
    SampleBlock(Hash256, Slot),

    /// A peer has disconnected.
    Disconnect(PeerId),

    /// An RPC Error has occurred on a request.
    RpcError {
        peer_id: PeerId,
        request_id: SyncRequestId,
        error: RPCError,
    },

    /// A batch has been processed by the block processor thread.
    BatchProcessed {
        sync_type: ChainSegmentProcessId,
        result: BatchProcessResult,
    },

    /// Block processed
    BlockComponentProcessed {
        process_type: BlockProcessType,
        result: BlockProcessingResult,
    },

    /// Sample data column verified
    SampleVerified {
        id: SamplingId,
        result: Result<(), String>,
    },

    /// A block from gossip has completed processing,
    GossipBlockProcessResult { block_root: Hash256, imported: bool },
}

/// The type of processing specified for a received block.
#[derive(Debug, Clone)]
pub enum BlockProcessType {
    SingleBlock { id: Id },
    SingleBlob { id: Id },
    SingleCustodyColumn(Id),
}

impl BlockProcessType {
    pub fn id(&self) -> Id {
        match self {
            BlockProcessType::SingleBlock { id }
            | BlockProcessType::SingleBlob { id }
            | BlockProcessType::SingleCustodyColumn(id) => *id,
        }
    }
}

#[derive(Debug)]
pub enum BlockProcessingResult {
    Ok(AvailabilityProcessingStatus),
    Err(BlockError),
    Ignored,
}

/// The result of processing multiple blocks (a chain segment).
#[derive(Debug)]
pub enum BatchProcessResult {
    /// The batch was completed successfully. It carries whether the sent batch contained blocks.
    Success {
        sent_blocks: usize,
        imported_blocks: usize,
    },
    /// The batch processing failed. It carries whether the processing imported any block.
    FaultyFailure {
        imported_blocks: usize,
        penalty: PeerAction,
    },
    NonFaultyFailure,
}

/// The primary object for handling and driving all the current syncing logic. It maintains the
/// current state of the syncing process, the number of useful peers, downloaded blocks and
/// controls the logic behind both the long-range (batch) sync and the on-going potential parent
/// look-up of blocks.
pub struct SyncManager<T: BeaconChainTypes> {
    /// A reference to the underlying beacon chain.
    chain: Arc<BeaconChain<T>>,

    /// A receiving channel sent by the message processor thread.
    input_channel: mpsc::UnboundedReceiver<SyncMessage<T::EthSpec>>,

    /// A network context to contact the network service.
    network: SyncNetworkContext<T>,

    /// The object handling long-range batch load-balanced syncing.
    range_sync: RangeSync<T>,

    /// Backfill syncing.
    backfill_sync: BackFillSync<T>,

    block_lookups: BlockLookups<T>,
    /// debounce duplicated `UnknownBlockHashFromAttestation` for the same root peer tuple. A peer
    /// may forward us thousands of a attestations, each one triggering an individual event. Only
    /// one event is useful, the rest generating log noise and wasted cycles
    notified_unknown_roots: LRUTimeCache<(PeerId, Hash256)>,

    sampling: Sampling<T>,

    /// The logger for the import manager.
    log: Logger,
}

/// Spawns a new `SyncManager` thread which has a weak reference to underlying beacon
/// chain. This allows the chain to be
/// dropped during the syncing process which will gracefully end the `SyncManager`.
pub fn spawn<T: BeaconChainTypes>(
    executor: task_executor::TaskExecutor,
    beacon_chain: Arc<BeaconChain<T>>,
    network_send: mpsc::UnboundedSender<NetworkMessage<T::EthSpec>>,
    beacon_processor: Arc<NetworkBeaconProcessor<T>>,
    sync_recv: mpsc::UnboundedReceiver<SyncMessage<T::EthSpec>>,
    fork_context: Arc<ForkContext>,
    log: slog::Logger,
) {
    assert!(
        beacon_chain.spec.max_request_blocks(fork_context.current_fork()) as u64 >= T::EthSpec::slots_per_epoch() * EPOCHS_PER_BATCH,
        "Max blocks that can be requested in a single batch greater than max allowed blocks in a single request"
    );

    // create an instance of the SyncManager
    let mut sync_manager = SyncManager::new(
        beacon_chain,
        network_send,
        beacon_processor,
        sync_recv,
        SamplingConfig::Default,
        fork_context,
        log.clone(),
    );

    // spawn the sync manager thread
    debug!(log, "Sync Manager started");
    executor.spawn(async move { Box::pin(sync_manager.main()).await }, "sync");
}

impl<T: BeaconChainTypes> SyncManager<T> {
    pub(crate) fn new(
        beacon_chain: Arc<BeaconChain<T>>,
        network_send: mpsc::UnboundedSender<NetworkMessage<T::EthSpec>>,
        beacon_processor: Arc<NetworkBeaconProcessor<T>>,
        sync_recv: mpsc::UnboundedReceiver<SyncMessage<T::EthSpec>>,
        sampling_config: SamplingConfig,
        fork_context: Arc<ForkContext>,
        log: slog::Logger,
    ) -> Self {
        let network_globals = beacon_processor.network_globals.clone();
        Self {
            chain: beacon_chain.clone(),
            input_channel: sync_recv,
            network: SyncNetworkContext::new(
                network_send,
                beacon_processor.clone(),
                beacon_chain.clone(),
                fork_context.clone(),
                log.clone(),
            ),
            range_sync: RangeSync::new(
                beacon_chain.clone(),
                log.new(o!("service" => "range_sync")),
            ),
            backfill_sync: BackFillSync::new(
                beacon_chain.clone(),
                network_globals,
                log.new(o!("service" => "backfill_sync")),
            ),
            block_lookups: BlockLookups::new(log.new(o!("service"=> "lookup_sync"))),
            notified_unknown_roots: LRUTimeCache::new(Duration::from_secs(
                NOTIFIED_UNKNOWN_ROOT_EXPIRY_SECONDS,
            )),
            sampling: Sampling::new(sampling_config, log.new(o!("service" => "sampling"))),
            log: log.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn active_single_lookups(&self) -> Vec<super::block_lookups::BlockLookupSummary> {
        self.block_lookups.active_single_lookups()
    }

    #[cfg(test)]
    pub(crate) fn active_parent_lookups(&self) -> Vec<Vec<Hash256>> {
        self.block_lookups
            .active_parent_lookups()
            .iter()
            .map(|c| c.chain.clone())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn get_range_sync_chains(
        &self,
    ) -> Result<Option<(RangeSyncType, Slot, Slot)>, &'static str> {
        self.range_sync.state()
    }

    #[cfg(test)]
    pub(crate) fn get_failed_chains(&mut self) -> Vec<Hash256> {
        self.block_lookups.get_failed_chains()
    }

    #[cfg(test)]
    pub(crate) fn insert_failed_chain(&mut self, block_root: Hash256) {
        self.block_lookups.insert_failed_chain(block_root);
    }

    #[cfg(test)]
    pub(crate) fn active_sampling_requests(&self) -> Vec<Hash256> {
        self.sampling.active_sampling_requests()
    }

    #[cfg(test)]
    pub(crate) fn get_sampling_request_status(
        &self,
        block_root: Hash256,
        index: &ColumnIndex,
    ) -> Option<super::peer_sampling::Status> {
        self.sampling.get_request_status(block_root, index)
    }

    #[cfg(test)]
    pub(crate) fn range_sync_state(&self) -> super::range_sync::SyncChainStatus {
        self.range_sync.state()
    }

    #[cfg(test)]
    pub(crate) fn update_execution_engine_state(&mut self, state: EngineState) {
        self.handle_new_execution_engine_state(state);
    }

    fn network_globals(&self) -> &NetworkGlobals<T::EthSpec> {
        self.network.network_globals()
    }

    /* Input Handling Functions */

    /// A peer has connected which has blocks that are unknown to us.
    ///
    /// This function handles the logic associated with the connection of a new peer. If the peer
    /// is sufficiently ahead of our current head, a range-sync (batch) sync is started and
    /// batches of blocks are queued to download from the peer. Batched blocks begin at our latest
    /// finalized head.
    ///
    /// If the peer is within the `SLOT_IMPORT_TOLERANCE`, then it's head is sufficiently close to
    /// ours that we consider it fully sync'd with respect to our current chain.
    fn add_peer(&mut self, peer_id: PeerId, remote: SyncInfo) {
        // ensure the beacon chain still exists
        let status = self.chain.status_message();
        let local = SyncInfo {
            head_slot: status.head_slot,
            head_root: status.head_root,
            finalized_epoch: status.finalized_epoch,
            finalized_root: status.finalized_root,
        };

        let sync_type = remote_sync_type(&local, &remote, &self.chain);

        // update the state of the peer.
        let is_still_connected = self.update_peer_sync_state(&peer_id, &local, &remote, &sync_type);
        if is_still_connected {
            match sync_type {
                PeerSyncType::Behind => {} // Do nothing
                PeerSyncType::Advanced => {
                    self.range_sync
                        .add_peer(&mut self.network, local, peer_id, remote);
                }
                PeerSyncType::FullySynced => {
                    // Sync considers this peer close enough to the head to not trigger range sync.
                    // Range sync handles well syncing large ranges of blocks, of a least a few blocks.
                    // However this peer may be in a fork that we should sync but we have not discovered
                    // yet. If the head of the peer is unknown, attempt block lookup first. If the
                    // unknown head turns out to be on a longer fork, it will trigger range sync.
                    //
                    // A peer should always be considered `Advanced` if its finalized root is
                    // unknown and ahead of ours, so we don't check for that root here.
                    //
                    // TODO: This fork-choice check is potentially duplicated, review code
                    if !self.chain.block_is_known_to_fork_choice(&remote.head_root) {
                        self.handle_unknown_block_root(peer_id, remote.head_root);
                    }
                }
            }
        }

        self.update_sync_state();

        // Try to make progress on custody requests that are waiting for peers
        for (id, result) in self.network.continue_custody_by_root_requests() {
            self.on_custody_by_root_result(id, result);
        }
    }

    /// Trigger range sync for a set of peers that claim to have imported a head unknown to us.
    fn add_peers_force_range_sync(
        &mut self,
        peers: &[PeerId],
        head_root: Hash256,
        head_slot: Option<Slot>,
    ) {
        let status = self.chain.status_message();
        let local = SyncInfo {
            head_slot: status.head_slot,
            head_root: status.head_root,
            finalized_epoch: status.finalized_epoch,
            finalized_root: status.finalized_root,
        };

        let head_slot = head_slot.unwrap_or_else(|| {
            debug!(self.log,
                "On add peers force range sync assuming local head_slot";
                "local_head_slot" => local.head_slot,
                "head_root" => ?head_root
            );
            local.head_slot
        });

        let remote = SyncInfo {
            head_slot,
            head_root,
            // Set finalized to same as local to trigger Head sync
            finalized_epoch: local.finalized_epoch,
            finalized_root: local.finalized_root,
        };

        for peer_id in peers {
            self.range_sync
                .add_peer(&mut self.network, local.clone(), *peer_id, remote.clone());
        }
    }

    /// Handles RPC errors related to requests that were emitted from the sync manager.
    fn inject_error(&mut self, peer_id: PeerId, request_id: SyncRequestId, error: RPCError) {
        trace!(self.log, "Sync manager received a failed RPC");
        match request_id {
            SyncRequestId::SingleBlock { id } => {
                self.on_single_block_response(id, peer_id, RpcEvent::RPCError(error))
            }
            SyncRequestId::SingleBlob { id } => {
                self.on_single_blob_response(id, peer_id, RpcEvent::RPCError(error))
            }
            SyncRequestId::DataColumnsByRoot(req_id) => {
                self.on_data_columns_by_root_response(req_id, peer_id, RpcEvent::RPCError(error))
            }
            SyncRequestId::BlocksByRange(req_id) => {
                self.on_blocks_by_range_response(req_id, peer_id, RpcEvent::RPCError(error))
            }
            SyncRequestId::BlobsByRange(req_id) => {
                self.on_blobs_by_range_response(req_id, peer_id, RpcEvent::RPCError(error))
            }
            SyncRequestId::DataColumnsByRange(req_id) => {
                self.on_data_columns_by_range_response(req_id, peer_id, RpcEvent::RPCError(error))
            }
        }
    }

    /// Handles a peer disconnect.
    ///
    /// It is important that a peer disconnect retries all the batches/lookups as
    /// there is no way to guarantee that libp2p always emits a error along with
    /// the disconnect.
    fn peer_disconnect(&mut self, peer_id: &PeerId) {
        // Inject a Disconnected error on all requests associated with the disconnected peer
        // to retry all batches/lookups
        for request_id in self.network.peer_disconnected(peer_id) {
            self.inject_error(*peer_id, request_id, RPCError::Disconnected);
        }

        // Remove peer from all data structures
        self.range_sync.peer_disconnect(&mut self.network, peer_id);
        let _ = self
            .backfill_sync
            .peer_disconnected(peer_id, &mut self.network);
        self.block_lookups.peer_disconnected(peer_id);

        // Regardless of the outcome, we update the sync status.
        self.update_sync_state();
    }

    /// Prune stale requests that are waiting for peers
    fn prune_requests(&mut self) {
        // continue_custody_by_root_requests attempts to make progress on all requests. If some
        // exceed the stale duration limit they will fail and return a result. Re-using
        // `continue_custody_by_root_requests` is just a convenience to have less code.
        for (id, result) in self.network.continue_custody_by_root_requests() {
            self.on_custody_by_root_result(id, result);
        }
    }

    /// Updates the syncing state of a peer.
    /// Return true if the peer is still connected and known to the peers DB
    fn update_peer_sync_state(
        &mut self,
        peer_id: &PeerId,
        local_sync_info: &SyncInfo,
        remote_sync_info: &SyncInfo,
        sync_type: &PeerSyncType,
    ) -> bool {
        // NOTE: here we are gracefully handling two race conditions: Receiving the status message
        // of a peer that is 1) disconnected 2) not in the PeerDB.

        let new_state = sync_type.as_sync_status(remote_sync_info);
        let rpr = new_state.as_str();
        // Drop the write lock
        let update_sync_status = self
            .network_globals()
            .peers
            .write()
            .update_sync_status(peer_id, new_state.clone());
        if let Some(was_updated) = update_sync_status {
            let is_connected = self.network_globals().peers.read().is_connected(peer_id);
            if was_updated {
                debug!(
                    self.log,
                    "Peer transitioned sync state";
                    "peer_id" => %peer_id,
                    "new_state" => rpr,
                    "our_head_slot" => local_sync_info.head_slot,
                    "our_finalized_epoch" => local_sync_info.finalized_epoch,
                    "their_head_slot" => remote_sync_info.head_slot,
                    "their_finalized_epoch" => remote_sync_info.finalized_epoch,
                    "is_connected" => is_connected
                );

                // A peer has transitioned its sync state. If the new state is "synced" we
                // inform the backfill sync that a new synced peer has joined us.
                if new_state.is_synced() {
                    self.backfill_sync.fully_synced_peer_joined();
                }
            }
            is_connected
        } else {
            error!(self.log, "Status'd peer is unknown"; "peer_id" => %peer_id);
            false
        }
    }

    /// Updates the global sync state, optionally instigating or pausing a backfill sync as well as
    /// logging any changes.
    ///
    /// The logic for which sync should be running is as follows:
    /// - If there is a range-sync running (or required) pause any backfill and let range-sync
    ///   complete.
    /// - If there is no current range sync, check for any requirement to backfill and either
    ///   start/resume a backfill sync if required. The global state will be BackFillSync if a
    ///   backfill sync is running.
    /// - If there is no range sync and no required backfill and we have synced up to the currently
    ///   known peers, we consider ourselves synced.
    fn update_sync_state(&mut self) {
        let new_state: SyncState = match self.range_sync.state() {
            Err(e) => {
                crit!(self.log, "Error getting range sync state"; "error" => %e);
                return;
            }
            Ok(state) => match state {
                None => {
                    // No range sync, so we decide if we are stalled or synced.
                    // For this we check if there is at least one advanced peer. An advanced peer
                    // with Idle range is possible since a peer's status is updated periodically.
                    // If we synced a peer between status messages, most likely the peer has
                    // advanced and will produce a head chain on re-status. Otherwise it will shift
                    // to being synced
                    let mut sync_state = {
                        let head = self.chain.best_slot();
                        let current_slot = self.chain.slot().unwrap_or_else(|_| Slot::new(0));

                        let peers = self.network_globals().peers.read();
                        if current_slot >= head
                            && current_slot.sub(head) <= (SLOT_IMPORT_TOLERANCE as u64)
                            && head > 0
                        {
                            SyncState::Synced
                        } else if peers.advanced_peers().next().is_some() {
                            SyncState::SyncTransition
                        } else if peers.synced_peers().next().is_none() {
                            SyncState::Stalled
                        } else {
                            // There are no peers that require syncing and we have at least one synced
                            // peer
                            SyncState::Synced
                        }
                    };

                    // If we would otherwise be synced, first check if we need to perform or
                    // complete a backfill sync.
                    #[cfg(not(feature = "disable-backfill"))]
                    if matches!(sync_state, SyncState::Synced) {
                        // Determine if we need to start/resume/restart a backfill sync.
                        match self.backfill_sync.start(&mut self.network) {
                            Ok(SyncStart::Syncing {
                                completed,
                                remaining,
                            }) => {
                                sync_state = SyncState::BackFillSyncing {
                                    completed,
                                    remaining,
                                };
                            }
                            Ok(SyncStart::NotSyncing) => {} // Ignore updating the state if the backfill sync state didn't start.
                            Err(e) => {
                                error!(self.log, "Backfill sync failed to start"; "error" => ?e);
                            }
                        }
                    }

                    // Return the sync state if backfilling is not required.
                    sync_state
                }
                Some((RangeSyncType::Finalized, start_slot, target_slot)) => {
                    // If there is a backfill sync in progress pause it.
                    #[cfg(not(feature = "disable-backfill"))]
                    self.backfill_sync.pause();

                    SyncState::SyncingFinalized {
                        start_slot,
                        target_slot,
                    }
                }
                Some((RangeSyncType::Head, start_slot, target_slot)) => {
                    // If there is a backfill sync in progress pause it.
                    #[cfg(not(feature = "disable-backfill"))]
                    self.backfill_sync.pause();

                    SyncState::SyncingHead {
                        start_slot,
                        target_slot,
                    }
                }
            },
        };

        let old_state = self.network_globals().set_sync_state(new_state);
        let new_state = self.network_globals().sync_state.read().clone();
        if !new_state.eq(&old_state) {
            info!(self.log, "Sync state updated"; "old_state" => %old_state, "new_state" => %new_state);
            // If we have become synced - Subscribe to all the core subnet topics
            // We don't need to subscribe if the old state is a state that would have already
            // invoked this call.
            if new_state.is_synced()
                && !matches!(
                    old_state,
                    SyncState::Synced { .. } | SyncState::BackFillSyncing { .. }
                )
            {
                self.network.subscribe_core_topics();
            }
        }
    }

    /// The main driving future for the sync manager.
    async fn main(&mut self) {
        let check_ee = self.chain.execution_layer.is_some();
        let mut check_ee_stream = {
            // some magic to have an instance implementing stream even if there is no execution layer
            let ee_responsiveness_watch: futures::future::OptionFuture<_> = self
                .chain
                .execution_layer
                .as_ref()
                .map(|el| el.get_responsiveness_watch())
                .into();
            futures::stream::iter(ee_responsiveness_watch.await).flatten()
        };

        // min(LOOKUP_MAX_DURATION_*) is 15 seconds. The cost of calling prune_lookups more often is
        // one iteration over the single lookups HashMap. This map is supposed to be very small < 10
        // unless there is a bug.
        let mut prune_lookups_interval = tokio::time::interval(Duration::from_secs(15));

        let mut prune_requests = tokio::time::interval(Duration::from_secs(15));

        let mut register_metrics_interval = tokio::time::interval(Duration::from_secs(5));

        // process any inbound messages
        loop {
            tokio::select! {
                Some(sync_message) = self.input_channel.recv() => {
                    self.handle_message(sync_message);
                },
                Some(engine_state) = check_ee_stream.next(), if check_ee => {
                    self.handle_new_execution_engine_state(engine_state);
                }
                _ = prune_lookups_interval.tick() => {
                    self.block_lookups.prune_lookups();
                }
                _ = prune_requests.tick() => {
                    self.prune_requests();
                }
                _ = register_metrics_interval.tick() => {
                    self.network.register_metrics();
                }
            }
        }
    }

    pub(crate) fn handle_message(&mut self, sync_message: SyncMessage<T::EthSpec>) {
        match sync_message {
            SyncMessage::AddPeer(peer_id, info) => {
                self.add_peer(peer_id, info);
            }
            SyncMessage::AddPeersForceRangeSync {
                peers,
                head_root,
                head_slot,
            } => {
                self.add_peers_force_range_sync(&peers, head_root, head_slot);
            }
            SyncMessage::RpcBlock {
                request_id,
                peer_id,
                beacon_block,
                seen_timestamp,
            } => {
                self.rpc_block_received(request_id, peer_id, beacon_block, seen_timestamp);
            }
            SyncMessage::RpcBlob {
                request_id,
                peer_id,
                blob_sidecar,
                seen_timestamp,
            } => self.rpc_blob_received(request_id, peer_id, blob_sidecar, seen_timestamp),
            SyncMessage::RpcDataColumn {
                request_id,
                peer_id,
                data_column,
                seen_timestamp,
            } => self.rpc_data_column_received(request_id, peer_id, data_column, seen_timestamp),
            SyncMessage::UnknownParentBlock(peer_id, block, block_root) => {
                let block_slot = block.slot();
                let parent_root = block.parent_root();
                debug!(self.log, "Received unknown parent block message"; "block_root" => %block_root, "parent_root" => %parent_root);
                self.handle_unknown_parent(
                    peer_id,
                    block_root,
                    parent_root,
                    block_slot,
                    BlockComponent::Block(DownloadResult {
                        value: block.block_cloned(),
                        block_root,
                        seen_timestamp: timestamp_now(),
                        peer_group: PeerGroup::from_single(peer_id),
                    }),
                );
            }
            SyncMessage::UnknownParentBlob(peer_id, blob) => {
                let blob_slot = blob.slot();
                let block_root = blob.block_root();
                let parent_root = blob.block_parent_root();
                debug!(self.log, "Received unknown parent blob message"; "block_root" => %block_root, "parent_root" => %parent_root);
                self.handle_unknown_parent(
                    peer_id,
                    block_root,
                    parent_root,
                    blob_slot,
                    BlockComponent::Blob(DownloadResult {
                        value: blob,
                        block_root,
                        seen_timestamp: timestamp_now(),
                        peer_group: PeerGroup::from_single(peer_id),
                    }),
                );
            }
            SyncMessage::UnknownParentDataColumn(peer_id, data_column) => {
                let data_column_slot = data_column.slot();
                let block_root = data_column.block_root();
                let parent_root = data_column.block_parent_root();
                debug!(self.log, "Received unknown parent data column message"; "block_root" => %block_root, "parent_root" => %parent_root);
                self.handle_unknown_parent(
                    peer_id,
                    block_root,
                    parent_root,
                    data_column_slot,
                    BlockComponent::DataColumn(DownloadResult {
                        value: data_column,
                        block_root,
                        seen_timestamp: timestamp_now(),
                        peer_group: PeerGroup::from_single(peer_id),
                    }),
                );
            }
            SyncMessage::UnknownBlockHashFromAttestation(peer_id, block_root) => {
                if !self.notified_unknown_roots.contains(&(peer_id, block_root)) {
                    self.notified_unknown_roots.insert((peer_id, block_root));
                    debug!(self.log, "Received unknown block hash message"; "block_root" => ?block_root, "peer" => ?peer_id);
                    self.handle_unknown_block_root(peer_id, block_root);
                }
            }
            SyncMessage::SampleBlock(block_root, block_slot) => {
                debug!(self.log, "Received SampleBlock message"; "block_root" => %block_root, "slot" => block_slot);
                if let Some((requester, result)) = self
                    .sampling
                    .on_new_sample_request(block_root, &mut self.network)
                {
                    self.on_sampling_result(requester, result)
                }
            }
            SyncMessage::Disconnect(peer_id) => {
                debug!(self.log, "Received disconnected message"; "peer_id" => %peer_id);
                self.peer_disconnect(&peer_id);
            }
            SyncMessage::RpcError {
                peer_id,
                request_id,
                error,
            } => self.inject_error(peer_id, request_id, error),
            SyncMessage::BlockComponentProcessed {
                process_type,
                result,
            } => self
                .block_lookups
                .on_processing_result(process_type, result, &mut self.network),
            SyncMessage::GossipBlockProcessResult {
                block_root,
                imported,
            } => self.block_lookups.on_external_processing_result(
                block_root,
                imported,
                &mut self.network,
            ),
            SyncMessage::BatchProcessed { sync_type, result } => match sync_type {
                ChainSegmentProcessId::RangeBatchId(chain_id, epoch) => {
                    self.range_sync.handle_block_process_result(
                        &mut self.network,
                        chain_id,
                        epoch,
                        result,
                    );
                    self.update_sync_state();
                }
                ChainSegmentProcessId::BackSyncBatchId(epoch) => {
                    match self.backfill_sync.on_batch_process_result(
                        &mut self.network,
                        epoch,
                        &result,
                    ) {
                        Ok(ProcessResult::Successful) => {}
                        Ok(ProcessResult::SyncCompleted) => self.update_sync_state(),
                        Err(error) => {
                            error!(self.log, "Backfill sync failed"; "error" => ?error);
                            // Update the global status
                            self.update_sync_state();
                        }
                    }
                }
            },
            SyncMessage::SampleVerified { id, result } => {
                if let Some((requester, result)) =
                    self.sampling
                        .on_sample_verified(id, result, &mut self.network)
                {
                    self.on_sampling_result(requester, result)
                }
            }
        }
    }

    fn handle_unknown_parent(
        &mut self,
        peer_id: PeerId,
        block_root: Hash256,
        parent_root: Hash256,
        slot: Slot,
        block_component: BlockComponent<T::EthSpec>,
    ) {
        match self.should_search_for_block(Some(slot), &peer_id) {
            Ok(_) => {
                self.block_lookups.search_child_and_parent(
                    block_root,
                    block_component,
                    peer_id,
                    &mut self.network,
                );
            }
            Err(reason) => {
                debug!(self.log, "Ignoring unknown parent request"; "block_root" => %block_root, "parent_root" => %parent_root, "reason" => reason);
            }
        }
    }

    fn handle_unknown_block_root(&mut self, peer_id: PeerId, block_root: Hash256) {
        match self.should_search_for_block(None, &peer_id) {
            Ok(_) => {
                self.block_lookups
                    .search_unknown_block(block_root, &[peer_id], &mut self.network);
            }
            Err(reason) => {
                debug!(self.log, "Ignoring unknown block request"; "block_root" => %block_root, "reason" => reason);
            }
        }
    }

    fn should_search_for_block(
        &mut self,
        block_slot: Option<Slot>,
        peer_id: &PeerId,
    ) -> Result<(), &'static str> {
        if !self.network_globals().sync_state.read().is_synced() {
            let Some(block_slot) = block_slot else {
                return Err("not synced");
            };

            let head_slot = self.chain.canonical_head.cached_head().head_slot();

            // if the block is far in the future, ignore it. If its within the slot tolerance of
            // our current head, regardless of the syncing state, fetch it.
            if (head_slot >= block_slot
                && head_slot.sub(block_slot).as_usize() > SLOT_IMPORT_TOLERANCE)
                || (head_slot < block_slot
                    && block_slot.sub(head_slot).as_usize() > SLOT_IMPORT_TOLERANCE)
            {
                return Err("not synced");
            }
        }

        if !self.network_globals().peers.read().is_connected(peer_id) {
            return Err("peer not connected");
        }
        if !self.network.is_execution_engine_online() {
            return Err("execution engine offline");
        }
        Ok(())
    }

    fn handle_new_execution_engine_state(&mut self, engine_state: EngineState) {
        self.network.update_execution_engine_state(engine_state);

        match engine_state {
            EngineState::Online => {
                // Resume sync components.

                // - Block lookups:
                //   We start searching for blocks again. This is done by updating the stored ee online
                //   state. No further action required.

                // - Parent lookups:
                //   We start searching for parents again. This is done by updating the stored ee
                //   online state. No further action required.

                // - Range:
                //   Actively resume.
                self.range_sync.resume(&mut self.network);

                // - Backfill:
                //   Not affected by ee states, nothing to do.
            }

            EngineState::Offline => {
                // Pause sync components.

                // - Block lookups:
                //   Disabled while in this state. We drop current requests and don't search for new
                //   blocks.
                let dropped_single_blocks_requests =
                    self.block_lookups.drop_single_block_requests();

                // - Range:
                //   We still send found peers to range so that it can keep track of potential chains
                //   with respect to our current peers. Range will stop processing batches in the
                //   meantime. No further action from the manager is required for this.

                // - Backfill: Not affected by ee states, nothing to do.

                // Some logs.
                if dropped_single_blocks_requests > 0 {
                    debug!(self.log, "Execution engine not online. Dropping active requests.";
                        "dropped_single_blocks_requests" => dropped_single_blocks_requests,
                    );
                }
            }
        }
    }

    fn rpc_block_received(
        &mut self,
        request_id: SyncRequestId,
        peer_id: PeerId,
        block: Option<Arc<SignedBeaconBlock<T::EthSpec>>>,
        seen_timestamp: Duration,
    ) {
        match request_id {
            SyncRequestId::SingleBlock { id } => self.on_single_block_response(
                id,
                peer_id,
                RpcEvent::from_chunk(block, seen_timestamp),
            ),
            SyncRequestId::BlocksByRange(id) => self.on_blocks_by_range_response(
                id,
                peer_id,
                RpcEvent::from_chunk(block, seen_timestamp),
            ),
            _ => {
                crit!(self.log, "bad request id for block"; "peer_id" => %peer_id  );
            }
        }
    }

    fn on_single_block_response(
        &mut self,
        id: SingleLookupReqId,
        peer_id: PeerId,
        block: RpcEvent<Arc<SignedBeaconBlock<T::EthSpec>>>,
    ) {
        if let Some(resp) = self.network.on_single_block_response(id, peer_id, block) {
            self.block_lookups
                .on_download_response::<BlockRequestState<T::EthSpec>>(
                    id,
                    resp.map(|(value, seen_timestamp)| {
                        (value, PeerGroup::from_single(peer_id), seen_timestamp)
                    }),
                    &mut self.network,
                )
        }
    }

    fn rpc_blob_received(
        &mut self,
        request_id: SyncRequestId,
        peer_id: PeerId,
        blob: Option<Arc<BlobSidecar<T::EthSpec>>>,
        seen_timestamp: Duration,
    ) {
        match request_id {
            SyncRequestId::SingleBlob { id } => self.on_single_blob_response(
                id,
                peer_id,
                RpcEvent::from_chunk(blob, seen_timestamp),
            ),
            SyncRequestId::BlobsByRange(id) => self.on_blobs_by_range_response(
                id,
                peer_id,
                RpcEvent::from_chunk(blob, seen_timestamp),
            ),
            _ => {
                crit!(self.log, "bad request id for blob"; "peer_id" => %peer_id);
            }
        }
    }

    fn rpc_data_column_received(
        &mut self,
        request_id: SyncRequestId,
        peer_id: PeerId,
        data_column: Option<Arc<DataColumnSidecar<T::EthSpec>>>,
        seen_timestamp: Duration,
    ) {
        match request_id {
            SyncRequestId::DataColumnsByRoot(req_id) => {
                self.on_data_columns_by_root_response(
                    req_id,
                    peer_id,
                    RpcEvent::from_chunk(data_column, seen_timestamp),
                );
            }
            SyncRequestId::DataColumnsByRange(id) => self.on_data_columns_by_range_response(
                id,
                peer_id,
                RpcEvent::from_chunk(data_column, seen_timestamp),
            ),
            _ => {
                crit!(self.log, "bad request id for data_column"; "peer_id" => %peer_id);
            }
        }
    }

    fn on_single_blob_response(
        &mut self,
        id: SingleLookupReqId,
        peer_id: PeerId,
        blob: RpcEvent<Arc<BlobSidecar<T::EthSpec>>>,
    ) {
        if let Some(resp) = self.network.on_single_blob_response(id, peer_id, blob) {
            self.block_lookups
                .on_download_response::<BlobRequestState<T::EthSpec>>(
                    id,
                    resp.map(|(value, seen_timestamp)| {
                        (value, PeerGroup::from_single(peer_id), seen_timestamp)
                    }),
                    &mut self.network,
                )
        }
    }

    fn on_data_columns_by_root_response(
        &mut self,
        req_id: DataColumnsByRootRequestId,
        peer_id: PeerId,
        data_column: RpcEvent<Arc<DataColumnSidecar<T::EthSpec>>>,
    ) {
        if let Some(resp) =
            self.network
                .on_data_columns_by_root_response(req_id, peer_id, data_column)
        {
            match req_id.requester {
                DataColumnsByRootRequester::Sampling(id) => {
                    if let Some((requester, result)) =
                        self.sampling
                            .on_sample_downloaded(id, peer_id, resp, &mut self.network)
                    {
                        self.on_sampling_result(requester, result)
                    }
                }
                DataColumnsByRootRequester::Custody(custody_id) => {
                    if let Some(result) = self
                        .network
                        .on_custody_by_root_response(custody_id, req_id, peer_id, resp)
                    {
                        self.on_custody_by_root_result(custody_id.requester, result);
                    }
                }
            }
        }
    }

    fn on_blocks_by_range_response(
        &mut self,
        id: BlocksByRangeRequestId,
        peer_id: PeerId,
        block: RpcEvent<Arc<SignedBeaconBlock<T::EthSpec>>>,
    ) {
        if let Some(resp) = self.network.on_blocks_by_range_response(id, peer_id, block) {
            self.on_range_components_response(
                id.parent_request_id,
                peer_id,
                RangeBlockComponent::Block(resp),
            );
        }
    }

    fn on_blobs_by_range_response(
        &mut self,
        id: BlobsByRangeRequestId,
        peer_id: PeerId,
        blob: RpcEvent<Arc<BlobSidecar<T::EthSpec>>>,
    ) {
        if let Some(resp) = self.network.on_blobs_by_range_response(id, peer_id, blob) {
            self.on_range_components_response(
                id.parent_request_id,
                peer_id,
                RangeBlockComponent::Blob(resp),
            );
        }
    }

    fn on_data_columns_by_range_response(
        &mut self,
        id: DataColumnsByRangeRequestId,
        peer_id: PeerId,
        data_column: RpcEvent<Arc<DataColumnSidecar<T::EthSpec>>>,
    ) {
        if let Some(resp) = self
            .network
            .on_data_columns_by_range_response(id, peer_id, data_column)
        {
            self.on_range_components_response(
                id.parent_request_id,
                peer_id,
                RangeBlockComponent::CustodyColumns(resp),
            );
        }
    }

    fn on_custody_by_root_result(
        &mut self,
        requester: CustodyRequester,
        response: CustodyByRootResult<T::EthSpec>,
    ) {
        // TODO(das): get proper timestamp
        let seen_timestamp = timestamp_now();
        self.block_lookups
            .on_download_response::<CustodyRequestState<T::EthSpec>>(
                requester.0,
                response.map(|(columns, peer_group)| (columns, peer_group, seen_timestamp)),
                &mut self.network,
            );
    }

    fn on_sampling_result(&mut self, requester: SamplingRequester, result: SamplingResult) {
        match requester {
            SamplingRequester::ImportedBlock(block_root) => {
                debug!(self.log, "Sampling result"; "block_root" => %block_root, "result" => ?result);

                match result {
                    Ok(_) => {
                        // Notify the fork-choice of a successful sampling result to mark the block
                        // branch as safe.
                        if let Err(e) = self
                            .network
                            .beacon_processor()
                            .send_sampling_completed(block_root)
                        {
                            warn!(self.log, "Error sending sampling result"; "block_root" => ?block_root, "reason" => ?e);
                        }
                    }
                    Err(e) => {
                        warn!(self.log, "Sampling failed"; "block_root" => %block_root, "reason" => ?e);
                    }
                }
            }
        }
    }

    /// Handles receiving a response for a range sync request that should have both blocks and
    /// blobs.
    fn on_range_components_response(
        &mut self,
        range_request_id: ComponentsByRangeRequestId,
        peer_id: PeerId,
        range_block_component: RangeBlockComponent<T::EthSpec>,
    ) {
        if let Some(resp) = self
            .network
            .range_block_component_response(range_request_id, range_block_component)
        {
            match resp {
                Ok(blocks) => {
                    match range_request_id.requester {
                        RangeRequestId::RangeSync { chain_id, batch_id } => {
                            self.range_sync.blocks_by_range_response(
                                &mut self.network,
                                peer_id,
                                chain_id,
                                batch_id,
                                range_request_id.id,
                                blocks,
                            );
                            self.update_sync_state();
                        }
                        RangeRequestId::BackfillSync { batch_id } => {
                            match self.backfill_sync.on_block_response(
                                &mut self.network,
                                batch_id,
                                &peer_id,
                                range_request_id.id,
                                blocks,
                            ) {
                                Ok(ProcessResult::SyncCompleted) => self.update_sync_state(),
                                Ok(ProcessResult::Successful) => {}
                                Err(_error) => {
                                    // The backfill sync has failed, errors are reported
                                    // within.
                                    self.update_sync_state();
                                }
                            }
                        }
                    }
                }
                Err(_) => match range_request_id.requester {
                    RangeRequestId::RangeSync { chain_id, batch_id } => {
                        self.range_sync.inject_error(
                            &mut self.network,
                            peer_id,
                            batch_id,
                            chain_id,
                            range_request_id.id,
                        );
                        self.update_sync_state();
                    }
                    RangeRequestId::BackfillSync { batch_id } => match self
                        .backfill_sync
                        .inject_error(&mut self.network, batch_id, &peer_id, range_request_id.id)
                    {
                        Ok(_) => {}
                        Err(_) => self.update_sync_state(),
                    },
                },
            }
        }
    }
}

impl From<Result<AvailabilityProcessingStatus, BlockError>> for BlockProcessingResult {
    fn from(result: Result<AvailabilityProcessingStatus, BlockError>) -> Self {
        match result {
            Ok(status) => BlockProcessingResult::Ok(status),
            Err(e) => BlockProcessingResult::Err(e),
        }
    }
}

impl From<BlockError> for BlockProcessingResult {
    fn from(e: BlockError) -> Self {
        BlockProcessingResult::Err(e)
    }
}
