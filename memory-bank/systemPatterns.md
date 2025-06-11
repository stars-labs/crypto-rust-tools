# System Patterns - Crypto Rust Tools

*Last Updated: 2025-06-07*

## Architectural Patterns

### 1. Modular Crate Architecture
**Pattern**: Multi-crate workspace with specialized responsibilities

```rust
crypto-rust-tools/
├── solnana-mpc-frost/     // Core MPC implementation
├── ssh-to-solana/         // Utility crate
├── yubikey-crpyto/       // Hardware integration
├── webrtc-signal-server/  // Network infrastructure
└── webrtc-signal-server-cloudflare-worker/  // Deployment variant
```

**Benefits**:
- Clear separation of concerns
- Independent versioning and deployment
- Reusable components across different applications
- Easier testing and maintenance

**Implementation**:
- Cargo workspace configuration manages dependencies
- Shared traits and types in common modules
- Each crate has focused, single-responsibility design

### 2. Protocol State Machine Pattern
**Pattern**: Explicit state management for complex cryptographic protocols

```rust
#[derive(Debug, Clone)]
enum DkgState {
    Idle,
    Round1InProgress,
    Round1Complete,
    Round2InProgress,
    Round2Complete,
    Finalized,
    Error(String),
}

#[derive(Debug, Clone)]
enum NodeState {
    Initial,
    DkgProcess,
    Idle,
    SigningCommitment,
    SignatureGeneration { selected: bool },
    // ... other states
}
```

**Benefits**:
- Clear protocol flow visualization
- Prevents invalid state transitions
- Easier debugging and error handling
- Supports resumable operations

**Usage Locations**:
- `src/utils/state.rs`: Core state definitions
- `examples/eth_dkg.rs`: Ethereum DKG state machine
- `examples/solana_dkg.rs`: Solana DKG state machine

### 3. Generic Curve Support Pattern
**Pattern**: Trait-based abstraction over different elliptic curves

```rust
// Generic implementation that works with any FROST-compatible curve
async fn handle_dkg_round1<C>(state: Arc<Mutex<AppState<C>>>) 
where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    // Implementation works for Ed25519, Secp256k1, and future curves
}
```

**Benefits**:
- Single codebase supports multiple curves
- Type-safe curve operations
- Easy addition of new curves
- Performance optimization per curve

**Curve Implementations**:
- `Ed25519Sha512`: Solana compatibility
- `Secp256K1Sha256`: Ethereum compatibility
- Future: BLS12-381, Ristretto support

## Communication Patterns

### 1. P2P WebRTC Pattern
**Pattern**: Direct device-to-device communication without central servers

```rust
// WebRTC device connection management
pub struct DeviceManager<C: Ciphersuite> {
    connections: HashMap<String, Arc<RTCDeviceConnection>>,
    data_channels: HashMap<String, Arc<RTCDataChannel>>,
    state: Arc<Mutex<AppState<C>>>,
}

// Message routing through data channels
impl<C: Ciphersuite> DeviceManager<C> {
    async fn send_to_device(&self, device_id: &str, message: &DkgMessage<C>) {
        // Direct P2P message sending
    }
    
    async fn broadcast_to_all(&self, message: &DkgMessage<C>) {
        // Efficient broadcast to all connected devices
    }
}
```

**Benefits**:
- No single point of failure
- Reduced latency compared to server-mediated communication
- Enhanced privacy (direct device communication)
- Scalable architecture

**Implementation Details**:
- ICE candidate exchange for NAT traversal
- Data channel establishment for reliable messaging
- Connection state monitoring and recovery

### 2. Async Message Handling Pattern
**Pattern**: Non-blocking message processing with Tokio

```rust
// Message processing pipeline
async fn handle_websocket_message<C>(
    msg: ClientMsg,
    state: Arc<Mutex<AppState<C>>>,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
) where C: Ciphersuite + Send + Sync + 'static {
    match msg {
        ClientMsg::Offer { from, to, offer } => {
            // Handle WebRTC offer asynchronously
        }
        ClientMsg::Answer { from, to, answer } => {
            // Handle WebRTC answer asynchronously
        }
        ClientMsg::IceCandidate { from, to, candidate } => {
            // Handle ICE candidate asynchronously
        }
    }
}
```

**Benefits**:
- High concurrency without blocking
- Efficient resource utilization
- Responsive user interface
- Scalable to many concurrent operations

### 3. Command Channel Pattern
**Pattern**: Internal command routing with MPSC channels

```rust
// Internal command system for decoupled communication
#[derive(Debug, Clone)]
enum InternalCommand<C: Ciphersuite> {
    SendToServer(ClientMsg),
    TriggerDkgRound1,
    TriggerDkgRound2,
    FinalizeDkg,
    MeshReady,
    InitiateWebRTC(String),
}

// Command processing
async fn handle_internal_command<C>(
    cmd: InternalCommand<C>,
    state: Arc<Mutex<AppState<C>>>,
    // ... other parameters
) {
    match cmd {
        InternalCommand::TriggerDkgRound1 => {
            handle_trigger_dkg_round1(state).await;
        }
        // ... other command handlers
    }
}
```

**Benefits**:
- Decoupled component communication
- Type-safe command routing
- Easy testing and mocking
- Clear separation of concerns

## Data Management Patterns

### 1. Serialization Strategy Pattern
**Pattern**: Consistent serialization across network and storage

```rust
// Standardized serialization for DKG packages
fn serialize_round1_package<C: Ciphersuite>(
    package: &round1::Package<C>
) -> Result<String, SerializationError> {
    serde_json::to_string(package)
        .map_err(|e| SerializationError::Json(e))
}

// Blockchain-specific serialization
fn serialize_for_blockchain<C: Ciphersuite>(
    signature: &Signature<C>
) -> Result<Vec<u8>, BlockchainError> {
    match C::suite_id() {
        "FROST-ED25519-SHA512" => serialize_for_solana(signature),
        "FROST-SECP256K1-SHA256" => serialize_for_ethereum(signature),
        _ => Err(BlockchainError::UnsupportedCurve),
    }
}
```

**Benefits**:
- Consistent data formats across components
- Blockchain-specific optimizations
- Version compatibility management
- Error handling standardization

### 2. Key Package Management Pattern
**Pattern**: Secure storage and retrieval of cryptographic material

```rust
// Key package persistence
impl KeyPackageManager {
    fn save_key_package<C: Ciphersuite>(
        &self,
        identifier: &Identifier<C>,
        key_package: &KeyPackage<C>,
    ) -> Result<(), KeyStorageError> {
        let encrypted_data = self.encrypt_key_data(key_package)?;
        let filename = format!("key_package_{}.bin", identifier);
        fs::write(filename, encrypted_data)?;
        Ok(())
    }
    
    fn load_key_package<C: Ciphersuite>(
        &self,
        identifier: &Identifier<C>,
    ) -> Result<KeyPackage<C>, KeyStorageError> {
        let filename = format!("key_package_{}.bin", identifier);
        let encrypted_data = fs::read(filename)?;
        let key_package = self.decrypt_key_data(&encrypted_data)?;
        Ok(key_package)
    }
}
```

**Benefits**:
- Secure key material handling
- Automatic encryption/decryption
- File system abstraction
- Recovery and backup capabilities

## Error Handling Patterns

### 1. Hierarchical Error Types Pattern
**Pattern**: Structured error types for different system layers

```rust
// Application-level errors
#[derive(Debug, thiserror::Error)]
pub enum ApplicationError {
    #[error("Network error: {0}")]
    Network(#[from] NetworkError),
    #[error("Cryptographic error: {0}")]
    Crypto(#[from] CryptoError),
    #[error("Protocol error: {0}")]
    Protocol(#[from] ProtocolError),
}

// Network-specific errors
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error("WebRTC connection failed")]
    WebRTCConnectionFailed,
    #[error("Device disconnected: {device_id}")]
    DeviceDisconnected { device_id: String },
    #[error("Signaling server unreachable")]
    SignalingServerUnreachable,
}

// Protocol-specific errors
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("Invalid DKG round transition from {from:?} to {to:?}")]
    InvalidStateTransition { from: DkgState, to: DkgState },
    #[error("Missing DKG package from participant {participant}")]
    MissingDkgPackage { participant: String },
}
```

**Benefits**:
- Clear error categorization
- Structured error propagation
- Detailed error context
- Easy error matching and handling

### 2. Recovery Strategy Pattern
**Pattern**: Automatic recovery mechanisms for common failures

```rust
// Connection recovery with exponential backoff
impl ConnectionManager {
    async fn reconnect_with_backoff(&self, device_id: &str) -> Result<(), NetworkError> {
        let mut delay = Duration::from_millis(100);
        const MAX_RETRIES: u32 = 5;
        
        for attempt in 1..=MAX_RETRIES {
            match self.establish_connection(device_id).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt == MAX_RETRIES => return Err(e),
                Err(_) => {
                    tokio::time::sleep(delay).await;
                    delay *= 2; // Exponential backoff
                }
            }
        }
        
        unreachable!()
    }
}

// DKG state recovery
impl DkgManager {
    async fn recover_from_failure(&mut self) -> Result<(), ProtocolError> {
        match &self.current_state {
            DkgState::Round1InProgress => {
                // Restart round 1 if incomplete
                self.restart_round1().await
            }
            DkgState::Round2InProgress => {
                // Resume round 2 if possible
                self.resume_round2().await
            }
            DkgState::Error(reason) => {
                // Analyze error and determine recovery strategy
                self.analyze_and_recover(reason).await
            }
            _ => Ok(()),
        }
    }
}
```

**Benefits**:
- Automatic failure recovery
- Reduced manual intervention
- Improved system reliability
- Graceful degradation

## Testing Patterns

### 1. Example-Driven Testing Pattern
**Pattern**: Comprehensive examples that serve as integration tests

```rust
// Examples that double as integration tests
// examples/eth_dkg.rs - Full Ethereum DKG workflow
// examples/solana_dkg.rs - Full Solana DKG workflow
// examples/dkg.rs - Basic multi-curve DKG demonstration

// Each example includes:
// 1. Multi-participant simulation
// 2. Complete protocol execution
// 3. Result validation
// 4. Error handling demonstration
```

**Benefits**:
- Real-world usage validation
- Documentation through code
- Integration test coverage
- User onboarding examples

### 2. Curve-Agnostic Testing Pattern
**Pattern**: Generic tests that work across all supported curves

```rust
// Generic test functions
async fn test_dkg_round_trip<C>() 
where 
    C: Ciphersuite + Send + Sync + 'static,
    // ... trait bounds
{
    let participants = create_test_participants::<C>(5, 3);
    let result = execute_full_dkg::<C>(participants).await;
    assert!(result.is_ok());
    
    // Verify all participants have consistent results
    let key_packages = result.unwrap();
    verify_consistent_key_packages(&key_packages);
}

// Test with multiple curves
#[tokio::test]
async fn test_ed25519_dkg() {
    test_dkg_round_trip::<Ed25519Sha512>().await;
}

#[tokio::test]
async fn test_secp256k1_dkg() {
    test_dkg_round_trip::<Secp256K1Sha256>().await;
}
```

**Benefits**:
- Comprehensive curve coverage
- Reduced test code duplication
- Consistent test quality
- Easy addition of new curves

## Performance Patterns

### 1. Lazy Initialization Pattern
**Pattern**: Deferred expensive operations until needed

```rust
// Lazy static initialization for expensive resources
lazy_static! {
    static ref WEBRTC_API: webrtc::api::API = {
        let mut media_engine = MediaEngine::default();
        let mut registry = Registry::new();
        register_default_interceptors(&mut registry, &mut media_engine).unwrap();
        APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build()
    };
}

// Lazy connection establishment
impl DeviceManager {
    async fn get_or_create_connection(&self, device_id: &str) -> Arc<RTCDeviceConnection> {
        if let Some(connection) = self.connections.get(device_id) {
            return connection.clone();
        }
        
        // Create connection only when needed
        let connection = self.create_connection(device_id).await;
        self.connections.insert(device_id.to_string(), connection.clone());
        connection
    }
}
```

**Benefits**:
- Reduced startup time
- Memory efficiency
- Resource conservation
- On-demand scalability

### 2. Batching Pattern
**Pattern**: Efficient batch processing of operations

```rust
// Batch message processing
impl MessageProcessor {
    async fn process_message_batch(&self, messages: Vec<DkgMessage>) {
        // Group messages by type for efficient processing
        let mut round1_messages = Vec::new();
        let mut round2_messages = Vec::new();
        
        for message in messages {
            match message {
                DkgMessage::Round1(msg) => round1_messages.push(msg),
                DkgMessage::Round2(msg) => round2_messages.push(msg),
            }
        }
        
        // Process each batch efficiently
        if !round1_messages.is_empty() {
            self.process_round1_batch(round1_messages).await;
        }
        if !round2_messages.is_empty() {
            self.process_round2_batch(round2_messages).await;
        }
    }
}
```

**Benefits**:
- Improved throughput
- Reduced system call overhead
- Better resource utilization
- Scalable message processing

---

*System patterns documentation provides the architectural foundation for understanding and extending the Crypto Rust Tools codebase.*