//! Helper functions and an extension trait for Ethereum 2 ENRs.

pub use discv5::enr::CombinedKey;

use super::enr_ext::CombinedKeyExt;
use super::ENR_FILENAME;
use crate::types::{Enr, EnrAttestationBitfield, EnrSyncCommitteeBitfield};
use crate::NetworkConfig;
use alloy_rlp::bytes::Bytes;
use libp2p::identity::Keypair;
use lighthouse_version::{client_name, version};
use slog::{debug, warn};
use ssz::{Decode, Encode};
use ssz_types::BitVector;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;
use std::str::FromStr;
use types::{ChainSpec, EnrForkId, EthSpec};

use super::enr_ext::{EnrExt, QUIC6_ENR_KEY, QUIC_ENR_KEY};

/// The ENR field specifying the fork id.
pub const ETH2_ENR_KEY: &str = "eth2";
/// The ENR field specifying the attestation subnet bitfield.
pub const ATTESTATION_BITFIELD_ENR_KEY: &str = "attnets";
/// The ENR field specifying the sync committee subnet bitfield.
pub const SYNC_COMMITTEE_BITFIELD_ENR_KEY: &str = "syncnets";
/// The ENR field specifying the peerdas custody group count.
pub const PEERDAS_CUSTODY_GROUP_COUNT_ENR_KEY: &str = "cgc";

/// Extension trait for ENR's within Eth2.
pub trait Eth2Enr {
    /// The attestation subnet bitfield associated with the ENR.
    fn attestation_bitfield<E: EthSpec>(&self) -> Result<EnrAttestationBitfield<E>, &'static str>;

    /// The sync committee subnet bitfield associated with the ENR.
    fn sync_committee_bitfield<E: EthSpec>(
        &self,
    ) -> Result<EnrSyncCommitteeBitfield<E>, &'static str>;

    /// The peerdas custody group count associated with the ENR.
    fn custody_group_count<E: EthSpec>(&self, spec: &ChainSpec) -> Result<u64, &'static str>;

    fn eth2(&self) -> Result<EnrForkId, &'static str>;
}

impl Eth2Enr for Enr {
    fn attestation_bitfield<E: EthSpec>(&self) -> Result<EnrAttestationBitfield<E>, &'static str> {
        let bitfield_bytes: Bytes = self
            .get_decodable(ATTESTATION_BITFIELD_ENR_KEY)
            .ok_or("ENR attestation bitfield non-existent")?
            .map_err(|_| "Invalid RLP Encoding")?;

        BitVector::<E::SubnetBitfieldLength>::from_ssz_bytes(&bitfield_bytes)
            .map_err(|_| "Could not decode the ENR attnets bitfield")
    }

    fn sync_committee_bitfield<E: EthSpec>(
        &self,
    ) -> Result<EnrSyncCommitteeBitfield<E>, &'static str> {
        let bitfield_bytes: Bytes = self
            .get_decodable(SYNC_COMMITTEE_BITFIELD_ENR_KEY)
            .ok_or("ENR sync committee bitfield non-existent")?
            .map_err(|_| "Invalid RLP Encoding")?;

        BitVector::<E::SyncCommitteeSubnetCount>::from_ssz_bytes(&bitfield_bytes)
            .map_err(|_| "Could not decode the ENR syncnets bitfield")
    }

    fn custody_group_count<E: EthSpec>(&self, spec: &ChainSpec) -> Result<u64, &'static str> {
        let cgc = self
            .get_decodable::<u64>(PEERDAS_CUSTODY_GROUP_COUNT_ENR_KEY)
            .ok_or("ENR custody group count non-existent")?
            .map_err(|_| "Could not decode the ENR custody group count")?;

        if (spec.custody_requirement..=spec.number_of_custody_groups).contains(&cgc) {
            Ok(cgc)
        } else {
            Err("Invalid custody group count in ENR")
        }
    }

    fn eth2(&self) -> Result<EnrForkId, &'static str> {
        let eth2_bytes: Bytes = self
            .get_decodable(ETH2_ENR_KEY)
            .ok_or("ENR has no eth2 field")?
            .map_err(|_| "Invalid RLP Encoding")?;

        EnrForkId::from_ssz_bytes(&eth2_bytes).map_err(|_| "Could not decode EnrForkId")
    }
}

/// Either use the given ENR or load an ENR from file if it exists and matches the current NodeId
/// and sequence number.
/// If an ENR exists, with the same NodeId, this function checks to see if the loaded ENR from
/// disk is suitable to use, otherwise we increment the given ENR's sequence number.
pub fn use_or_load_enr(
    enr_key: &CombinedKey,
    local_enr: &mut Enr,
    config: &NetworkConfig,
    log: &slog::Logger,
) -> Result<(), String> {
    let enr_f = config.network_dir.join(ENR_FILENAME);
    if let Ok(mut enr_file) = File::open(enr_f.clone()) {
        let mut enr_string = String::new();
        match enr_file.read_to_string(&mut enr_string) {
            Err(_) => debug!(log, "Could not read ENR from file"),
            Ok(_) => {
                match Enr::from_str(&enr_string) {
                    Ok(disk_enr) => {
                        // if the same node id, then we may need to update our sequence number
                        if local_enr.node_id() == disk_enr.node_id() {
                            if compare_enr(local_enr, &disk_enr) {
                                debug!(log, "ENR loaded from disk"; "file" => ?enr_f);
                                // the stored ENR has the same configuration, use it
                                *local_enr = disk_enr;
                                return Ok(());
                            }

                            // same node id, different configuration - update the sequence number
                            // Note: local_enr is generated with default(0) attnets value,
                            // so a non default value in persisted enr will also update sequence number.
                            let new_seq_no = disk_enr.seq().checked_add(1).ok_or("ENR sequence number on file is too large. Remove it to generate a new NodeId")?;
                            local_enr.set_seq(new_seq_no, enr_key).map_err(|e| {
                                format!("Could not update ENR sequence number: {:?}", e)
                            })?;
                            debug!(log, "ENR sequence number increased"; "seq" =>  new_seq_no);
                        }
                    }
                    Err(e) => {
                        warn!(log, "ENR from file could not be decoded"; "error" => ?e);
                    }
                }
            }
        }
    }

    save_enr_to_disk(&config.network_dir, local_enr, log);

    Ok(())
}

/// Loads an ENR from file if it exists and matches the current NodeId and sequence number. If none
/// exists, generates a new one.
///
/// If an ENR exists, with the same NodeId, this function checks to see if the loaded ENR from
/// disk is suitable to use, otherwise we increment our newly generated ENR's sequence number.
pub fn build_or_load_enr<E: EthSpec>(
    local_key: Keypair,
    config: &NetworkConfig,
    enr_fork_id: &EnrForkId,
    log: &slog::Logger,
    spec: &ChainSpec,
) -> Result<Enr, String> {
    // Build the local ENR.
    // Note: Discovery should update the ENR record's IP to the external IP as seen by the
    // majority of our peers, if the CLI doesn't expressly forbid it.
    let enr_key = CombinedKey::from_libp2p(local_key)?;
    let mut local_enr = build_enr::<E>(&enr_key, config, enr_fork_id, spec)?;

    use_or_load_enr(&enr_key, &mut local_enr, config, log)?;
    Ok(local_enr)
}

/// Builds a lighthouse ENR given a `NetworkConfig`.
pub fn build_enr<E: EthSpec>(
    enr_key: &CombinedKey,
    config: &NetworkConfig,
    enr_fork_id: &EnrForkId,
    spec: &ChainSpec,
) -> Result<Enr, String> {
    let mut builder = discv5::enr::Enr::builder();
    let (maybe_ipv4_address, maybe_ipv6_address) = &config.enr_address;

    if let Some(ip) = maybe_ipv4_address {
        builder.ip4(*ip);
    }

    if let Some(ip) = maybe_ipv6_address {
        builder.ip6(*ip);
    }

    if let Some(udp4_port) = config.enr_udp4_port {
        builder.udp4(udp4_port.get());
    }

    if let Some(udp6_port) = config.enr_udp6_port {
        builder.udp6(udp6_port.get());
    }

    // Add EIP 7636 client information
    if !config.private {
        builder.client_info(client_name().to_string(), version().to_string(), None);
    }

    // Add QUIC fields to the ENR.
    // Since QUIC is used as an alternative transport for the libp2p protocols,
    // the related fields should only be added when both QUIC and libp2p are enabled
    if !config.disable_quic_support {
        // If we are listening on ipv4, add the quic ipv4 port.
        if let Some(quic4_port) = config.enr_quic4_port.or_else(|| {
            config
                .listen_addrs()
                .v4()
                .and_then(|v4_addr| v4_addr.quic_port.try_into().ok())
        }) {
            builder.add_value(QUIC_ENR_KEY, &quic4_port.get());
        }

        // If we are listening on ipv6, add the quic ipv6 port.
        if let Some(quic6_port) = config.enr_quic6_port.or_else(|| {
            config
                .listen_addrs()
                .v6()
                .and_then(|v6_addr| v6_addr.quic_port.try_into().ok())
        }) {
            builder.add_value(QUIC6_ENR_KEY, &quic6_port.get());
        }
    }

    // If the ENR port is not set, and we are listening over that ip version, use the listening port instead.
    let tcp4_port = config.enr_tcp4_port.or_else(|| {
        config
            .listen_addrs()
            .v4()
            .and_then(|v4_addr| v4_addr.tcp_port.try_into().ok())
    });
    if let Some(tcp4_port) = tcp4_port {
        builder.tcp4(tcp4_port.get());
    }

    let tcp6_port = config.enr_tcp6_port.or_else(|| {
        config
            .listen_addrs()
            .v6()
            .and_then(|v6_addr| v6_addr.tcp_port.try_into().ok())
    });
    if let Some(tcp6_port) = tcp6_port {
        builder.tcp6(tcp6_port.get());
    }

    // set the `eth2` field on our ENR
    builder.add_value::<Bytes>(ETH2_ENR_KEY, &enr_fork_id.as_ssz_bytes().into());

    // set the "attnets" field on our ENR
    let bitfield = BitVector::<E::SubnetBitfieldLength>::new();

    builder.add_value::<Bytes>(
        ATTESTATION_BITFIELD_ENR_KEY,
        &bitfield.as_ssz_bytes().into(),
    );

    // set the "syncnets" field on our ENR
    let bitfield = BitVector::<E::SyncCommitteeSubnetCount>::new();

    builder.add_value::<Bytes>(
        SYNC_COMMITTEE_BITFIELD_ENR_KEY,
        &bitfield.as_ssz_bytes().into(),
    );

    // only set `cgc` if PeerDAS fork epoch has been scheduled
    if spec.is_peer_das_scheduled() {
        let custody_group_count = if config.subscribe_all_data_column_subnets {
            spec.number_of_custody_groups
        } else {
            spec.custody_requirement
        };
        builder.add_value(PEERDAS_CUSTODY_GROUP_COUNT_ENR_KEY, &custody_group_count);
    }

    builder
        .build(enr_key)
        .map_err(|e| format!("Could not build Local ENR: {:?}", e))
}

/// Defines the conditions under which we use the locally built ENR or the one stored on disk.
/// If this function returns true, we use the `disk_enr`.
fn compare_enr(local_enr: &Enr, disk_enr: &Enr) -> bool {
    // take preference over disk_enr address if one is not specified
    (local_enr.ip4().is_none() || local_enr.ip4() == disk_enr.ip4())
        &&
    (local_enr.ip6().is_none() || local_enr.ip6() == disk_enr.ip6())
        // tcp ports must match
        && local_enr.tcp4() == disk_enr.tcp4()
        && local_enr.tcp6() == disk_enr.tcp6()
        // quic ports must match
        && local_enr.quic4() == disk_enr.quic4()
        && local_enr.quic6() == disk_enr.quic6()
        // must match on the same fork
        && local_enr.get_decodable::<Bytes>(ETH2_ENR_KEY) == disk_enr.get_decodable(ETH2_ENR_KEY)
        // take preference over disk udp port if one is not specified
        && (local_enr.udp4().is_none() || local_enr.udp4() == disk_enr.udp4())
        && (local_enr.udp6().is_none() || local_enr.udp6() == disk_enr.udp6())
        // we need the ATTESTATION_BITFIELD_ENR_KEY and SYNC_COMMITTEE_BITFIELD_ENR_KEY and
        // PEERDAS_CUSTODY_GROUP_COUNT_ENR_KEY key to match, otherwise we use a new ENR. This will
        // likely only be true for non-validating nodes.
        && local_enr.get_decodable::<Bytes>(ATTESTATION_BITFIELD_ENR_KEY) == disk_enr.get_decodable(ATTESTATION_BITFIELD_ENR_KEY)
        && local_enr.get_decodable::<Bytes>(SYNC_COMMITTEE_BITFIELD_ENR_KEY) == disk_enr.get_decodable(SYNC_COMMITTEE_BITFIELD_ENR_KEY)
        && local_enr.get_decodable::<Bytes>(PEERDAS_CUSTODY_GROUP_COUNT_ENR_KEY) == disk_enr.get_decodable(PEERDAS_CUSTODY_GROUP_COUNT_ENR_KEY)
}

/// Loads enr from the given directory
pub fn load_enr_from_disk(dir: &Path) -> Result<Enr, String> {
    let enr_f = dir.join(ENR_FILENAME);
    let mut enr_file =
        File::open(enr_f).map_err(|e| format!("Failed to open enr file: {:?}", e))?;
    let mut enr_string = String::new();
    match enr_file.read_to_string(&mut enr_string) {
        Err(_) => Err("Could not read ENR from file".to_string()),
        Ok(_) => Enr::from_str(&enr_string)
            .map_err(|e| format!("ENR from file could not be decoded: {:?}", e)),
    }
}

/// Saves an ENR to disk
pub fn save_enr_to_disk(dir: &Path, enr: &Enr, log: &slog::Logger) {
    let _ = std::fs::create_dir_all(dir);
    match File::create(dir.join(Path::new(ENR_FILENAME)))
        .and_then(|mut f| f.write_all(enr.to_base64().as_bytes()))
    {
        Ok(_) => {
            debug!(log, "ENR written to disk");
        }
        Err(e) => {
            warn!(
                log,
                "Could not write ENR to file"; "file" => format!("{:?}{:?}",dir, ENR_FILENAME),  "error" => %e
            );
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::config::Config as NetworkConfig;
    use types::{Epoch, MainnetEthSpec};

    type E = MainnetEthSpec;

    fn make_fulu_spec() -> ChainSpec {
        let mut spec = E::default_spec();
        spec.fulu_fork_epoch = Some(Epoch::new(10));
        spec
    }

    fn build_enr_with_config(config: NetworkConfig, spec: &ChainSpec) -> (Enr, CombinedKey) {
        let keypair = libp2p::identity::secp256k1::Keypair::generate();
        let enr_key = CombinedKey::from_secp256k1(&keypair);
        let enr_fork_id = EnrForkId::default();
        let enr = build_enr::<E>(&enr_key, &config, &enr_fork_id, spec).unwrap();
        (enr, enr_key)
    }

    #[test]
    fn custody_group_count_default() {
        let config = NetworkConfig {
            subscribe_all_data_column_subnets: false,
            ..NetworkConfig::default()
        };
        let spec = make_fulu_spec();

        let enr = build_enr_with_config(config, &spec).0;

        assert_eq!(
            enr.custody_group_count::<E>(&spec).unwrap(),
            spec.custody_requirement,
        );
    }

    #[test]
    fn custody_group_count_all() {
        let config = NetworkConfig {
            subscribe_all_data_column_subnets: true,
            ..NetworkConfig::default()
        };
        let spec = make_fulu_spec();
        let enr = build_enr_with_config(config, &spec).0;

        assert_eq!(
            enr.custody_group_count::<E>(&spec).unwrap(),
            spec.number_of_custody_groups,
        );
    }

    #[test]
    fn test_encode_decode_eth2_enr() {
        let (enr, _key) = build_enr_with_config(NetworkConfig::default(), &E::default_spec());
        // Check all Eth2 Mappings are decodeable
        enr.eth2().unwrap();
        enr.attestation_bitfield::<MainnetEthSpec>().unwrap();
        enr.sync_committee_bitfield::<MainnetEthSpec>().unwrap();
    }

    #[test]
    fn test_eth2_enr_encodings() {
        let enr_str = "enr:-Mm4QEX9fFRi1n4H3M9sGIgFQ6op1IysTU4Gz6tpIiOGRM1DbJtIih1KgGgv3Xl-oUlwco3HwdXsbYuXStBuNhUVIPoBh2F0dG5ldHOIAAAAAAAAAACDY3NjBIRldGgykI-3hTFgAAA4AOH1BQAAAACCaWSCdjSCaXCErBAADoRxdWljgiMpiXNlY3AyNTZrMaECph91xMyTVyE5MVj6lBpPgz6KP2--Kr9lPbo6_GjrfRKIc3luY25ldHMAg3RjcIIjKIN1ZHCCIyg";
        //let my_enr_str = "enr:-Ma4QM2I1AxBU116QcMV2wKVrSr5Nsko90gMVkstZO4APysQCEwJJJeuTvODKmv7fDsLhVFjrlidVNhBOxSZ8sZPbCWCCcqHYXR0bmV0c4gAAAAAAAAMAIRldGgykGqVoakEAAAA__________-CaWSCdjSCaXCEJq-HPYRxdWljgiMziXNlY3AyNTZrMaECMPAnmmHQpD1k6DuOxWVoFXBoTYY6Wuv9BP4lxauAlmiIc3luY25ldHMAg3RjcIIjMoN1ZHCCIzI";
        let enr = Enr::from_str(enr_str).unwrap();
        enr.eth2().unwrap();
        enr.attestation_bitfield::<MainnetEthSpec>().unwrap();
        enr.sync_committee_bitfield::<MainnetEthSpec>().unwrap();
    }
}
