//! Provides easy ways to run a beacon node or validator client in-process.
//!
//! Intended to be used for testing and simulation purposes. Not for production.

use beacon_node::ProductionBeaconNode;
use environment::RuntimeContext;
use eth2::{reqwest::ClientBuilder, BeaconNodeHttpClient, Timeouts};
use sensitive_url::SensitiveUrl;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::{Builder as TempBuilder, TempDir};
use tokio::time::timeout;
use types::EthSpec;
use validator_client::ProductionValidatorClient;
use validator_dir::insecure_keys::build_deterministic_validator_dirs;

pub use beacon_node::{ClientConfig, ClientGenesis, ProductionClient};
pub use beacon_node_fallback::ApiTopic;
pub use environment;
pub use eth2;
pub use execution_layer::test_utils::{
    Config as MockServerConfig, MockExecutionConfig, MockServer,
};
pub use validator_client::Config as ValidatorConfig;

/// The global timeout for HTTP requests to the beacon node.
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
/// The timeout for a beacon node to start up.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Provides a beacon node that is running in the current process on a given tokio executor (it
/// is _local_ to this process).
///
/// Intended for use in testing and simulation. Not for production.
pub struct LocalBeaconNode<E: EthSpec> {
    pub client: ProductionClient<E>,
    pub datadir: TempDir,
}

impl<E: EthSpec> LocalBeaconNode<E> {
    /// Starts a new, production beacon node on the tokio runtime in the given `context`.
    ///
    /// The node created is using the same types as the node we use in production.
    pub async fn production(
        context: RuntimeContext<E>,
        mut client_config: ClientConfig,
    ) -> Result<Self, String> {
        // Creates a temporary directory that will be deleted once this `TempDir` is dropped.
        let datadir = TempBuilder::new()
            .prefix("lighthouse_node_test_rig")
            .tempdir()
            .expect("should create temp directory for client datadir");

        client_config.set_data_dir(datadir.path().into());
        client_config.network.network_dir = PathBuf::from(datadir.path()).join("network");

        timeout(
            STARTUP_TIMEOUT,
            ProductionBeaconNode::new(context, client_config),
        )
        .await
        .map_err(|_| format!("Beacon node startup timed out after {:?}", STARTUP_TIMEOUT))?
        .map(move |client| Self {
            client: client.into_inner(),
            datadir,
        })
    }
}

impl<E: EthSpec> LocalBeaconNode<E> {
    /// Returns a `RemoteBeaconNode` that can connect to `self`. Useful for testing the node as if
    /// it were external this process.
    pub fn remote_node(&self) -> Result<BeaconNodeHttpClient, String> {
        let listen_addr = self
            .client
            .http_api_listen_addr()
            .ok_or("A remote beacon node must have a http server")?;

        let beacon_node_url: SensitiveUrl = SensitiveUrl::parse(
            format!("http://{}:{}", listen_addr.ip(), listen_addr.port()).as_str(),
        )
        .map_err(|e| format!("Unable to parse beacon node URL: {:?}", e))?;
        let beacon_node_http_client = ClientBuilder::new()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| format!("Unable to build HTTP client: {:?}", e))?;
        Ok(BeaconNodeHttpClient::from_components(
            beacon_node_url,
            beacon_node_http_client,
            Timeouts::set_all(HTTP_TIMEOUT),
        ))
    }
}

pub fn testing_client_config() -> ClientConfig {
    let mut client_config = ClientConfig::default();

    // Setting ports to `0` means that the OS will choose some available port.
    client_config
        .network
        .set_ipv4_listening_address(std::net::Ipv4Addr::UNSPECIFIED, 0, 0, 0);
    client_config.network.upnp_enabled = false;
    client_config.http_api.enabled = true;
    client_config.http_api.listen_port = 0;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("should get system time")
        .as_secs();

    client_config.genesis = ClientGenesis::Interop {
        validator_count: 8,
        genesis_time: now,
    };

    // Simulator tests expect historic states to be available for post-run checks.
    client_config.chain.reconstruct_historic_states = true;

    // Specify a constant count of beacon processor workers. Having this number
    // too low can cause annoying HTTP timeouts, especially on Github runners
    // with 2 logical CPUs.
    client_config.beacon_processor.max_workers = 4;

    client_config
}

pub fn testing_validator_config() -> ValidatorConfig {
    ValidatorConfig {
        init_slashing_protection: true,
        disable_auto_discover: false,
        ..ValidatorConfig::default()
    }
}

/// Contains the directories for a `LocalValidatorClient`.
///
/// This struct is separate to `LocalValidatorClient` to allow for pre-computation of validator
/// keypairs since the task is quite resource intensive.
pub struct ValidatorFiles {
    pub validator_dir: TempDir,
    pub secrets_dir: TempDir,
}

impl ValidatorFiles {
    /// Creates temporary data and secrets dirs.
    pub fn new() -> Result<Self, String> {
        let datadir = TempBuilder::new()
            .prefix("lighthouse-validator-client")
            .tempdir()
            .map_err(|e| format!("Unable to create VC data dir: {:?}", e))?;

        let secrets_dir = TempBuilder::new()
            .prefix("lighthouse-validator-client-secrets")
            .tempdir()
            .map_err(|e| format!("Unable to create VC secrets dir: {:?}", e))?;

        Ok(Self {
            validator_dir: datadir,
            secrets_dir,
        })
    }

    /// Creates temporary data and secrets dirs, preloaded with keystores.
    pub fn with_keystores(keypair_indices: &[usize]) -> Result<Self, String> {
        let this = Self::new()?;

        build_deterministic_validator_dirs(
            this.validator_dir.path().into(),
            this.secrets_dir.path().into(),
            keypair_indices,
        )
        .map_err(|e| format!("Unable to build validator directories: {:?}", e))?;

        Ok(this)
    }
}

/// Provides a validator client that is running in the current process on a given tokio executor (it
/// is _local_ to this process).
///
/// Intended for use in testing and simulation. Not for production.
pub struct LocalValidatorClient<E: EthSpec> {
    pub client: ProductionValidatorClient<E>,
    pub files: ValidatorFiles,
}

impl<E: EthSpec> LocalValidatorClient<E> {
    /// Creates a validator client with insecure deterministic keypairs. The validator directories
    /// are created in a temp dir then removed when the process exits.
    ///
    /// The validator created is using the same types as the node we use in production.
    pub async fn production_with_insecure_keypairs(
        context: RuntimeContext<E>,
        config: ValidatorConfig,
        files: ValidatorFiles,
    ) -> Result<Self, String> {
        Self::new(context, config, files).await
    }

    /// Creates a validator client that attempts to read keys from the default data dir.
    ///
    /// - The validator created is using the same types as the node we use in production.
    /// - It is recommended to use `production_with_insecure_keypairs` for testing.
    pub async fn production(
        context: RuntimeContext<E>,
        config: ValidatorConfig,
    ) -> Result<Self, String> {
        let files = ValidatorFiles::new()?;

        Self::new(context, config, files).await
    }

    async fn new(
        context: RuntimeContext<E>,
        mut config: ValidatorConfig,
        files: ValidatorFiles,
    ) -> Result<Self, String> {
        config.validator_dir = files.validator_dir.path().into();
        config.secrets_dir = files.secrets_dir.path().into();

        let mut client = ProductionValidatorClient::new(context, config).await?;

        client
            .start_service()
            .await
            .expect("should start validator services");
        Ok(Self { client, files })
    }
}

/// Provides an execution engine api server that is running in the current process on a given tokio executor (it
/// is _local_ to this process).
///
/// Intended for use in testing and simulation. Not for production.
pub struct LocalExecutionNode<E: EthSpec> {
    pub server: MockServer<E>,
    pub datadir: TempDir,
}

impl<E: EthSpec> LocalExecutionNode<E> {
    pub fn new(context: RuntimeContext<E>, config: MockExecutionConfig) -> Self {
        let datadir = TempBuilder::new()
            .prefix("lighthouse_node_test_rig_el")
            .tempdir()
            .expect("should create temp directory for client datadir");
        let jwt_file_path = datadir.path().join("jwt.hex");
        if let Err(e) = std::fs::write(jwt_file_path, config.jwt_key.hex_string()) {
            panic!("Failed to write jwt file {}", e);
        }
        let spec = Arc::new(E::default_spec());
        Self {
            server: MockServer::new_with_config(
                &context.executor.handle().unwrap(),
                config,
                spec,
                None,
            ),
            datadir,
        }
    }
}
