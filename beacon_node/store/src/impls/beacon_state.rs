use crate::*;
use ssz::{DecodeError, Encode};
use ssz_derive::Encode;

pub fn store_full_state<E: EthSpec>(
    state_root: &Hash256,
    state: &BeaconState<E>,
    ops: &mut Vec<KeyValueStoreOp>,
) -> Result<(), Error> {
    let bytes = {
        let _overhead_timer = metrics::start_timer(&metrics::BEACON_STATE_WRITE_OVERHEAD_TIMES);
        StorageContainer::new(state).as_ssz_bytes()
    };
    metrics::inc_counter_by(&metrics::BEACON_STATE_WRITE_BYTES, bytes.len() as u64);
    metrics::inc_counter(&metrics::BEACON_STATE_WRITE_COUNT);
    ops.push(KeyValueStoreOp::PutKeyValue(
        DBColumn::BeaconState,
        state_root.as_slice().to_vec(),
        bytes,
    ));
    Ok(())
}

pub fn get_full_state<KV: KeyValueStore<E>, E: EthSpec>(
    db: &KV,
    state_root: &Hash256,
    spec: &ChainSpec,
) -> Result<Option<BeaconState<E>>, Error> {
    let total_timer = metrics::start_timer(&metrics::BEACON_STATE_READ_TIMES);

    match db.get_bytes(DBColumn::BeaconState, state_root.as_slice())? {
        Some(bytes) => {
            let overhead_timer = metrics::start_timer(&metrics::BEACON_STATE_READ_OVERHEAD_TIMES);
            let container = StorageContainer::from_ssz_bytes(&bytes, spec)?;

            metrics::stop_timer(overhead_timer);
            metrics::stop_timer(total_timer);
            metrics::inc_counter(&metrics::BEACON_STATE_READ_COUNT);
            metrics::inc_counter_by(&metrics::BEACON_STATE_READ_BYTES, bytes.len() as u64);

            Ok(Some(container.try_into()?))
        }
        None => Ok(None),
    }
}

/// A container for storing `BeaconState` components.
// TODO: would be more space efficient with the caches stored separately and referenced by hash
#[derive(Encode)]
pub struct StorageContainer<E: EthSpec> {
    state: BeaconState<E>,
    committee_caches: Vec<Arc<CommitteeCache>>,
}

impl<E: EthSpec> StorageContainer<E> {
    /// Create a new instance for storing a `BeaconState`.
    pub fn new(state: &BeaconState<E>) -> Self {
        Self {
            state: state.clone(),
            committee_caches: state.committee_caches().to_vec(),
        }
    }

    pub fn from_ssz_bytes(bytes: &[u8], spec: &ChainSpec) -> Result<Self, ssz::DecodeError> {
        // We need to use the slot-switching `from_ssz_bytes` of `BeaconState`, which doesn't
        // compose with the other SSZ utils, so we duplicate some parts of `ssz_derive` here.
        let mut builder = ssz::SszDecoderBuilder::new(bytes);

        builder.register_anonymous_variable_length_item()?;
        builder.register_type::<Vec<CommitteeCache>>()?;

        let mut decoder = builder.build()?;

        let state = decoder.decode_next_with(|bytes| BeaconState::from_ssz_bytes(bytes, spec))?;
        let committee_caches = decoder.decode_next()?;

        Ok(Self {
            state,
            committee_caches,
        })
    }
}

impl<E: EthSpec> TryInto<BeaconState<E>> for StorageContainer<E> {
    type Error = Error;

    fn try_into(mut self) -> Result<BeaconState<E>, Error> {
        let mut state = self.state;

        for i in (0..CACHED_EPOCHS).rev() {
            if i >= self.committee_caches.len() {
                return Err(Error::SszDecodeError(DecodeError::BytesInvalid(
                    "Insufficient committees for BeaconState".to_string(),
                )));
            };

            state.committee_caches_mut()[i] = self.committee_caches.remove(i);
        }

        Ok(state)
    }
}
