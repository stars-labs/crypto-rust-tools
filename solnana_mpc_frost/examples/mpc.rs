use frost_core::Identifier;
use frost_core::keys::{
    KeyPackage, PublicKeyPackage, SecretShare, VerifiableSecretSharingCommitment,
};
use frost_ed25519::Ed25519Sha512;
use frost_ed25519::rand_core::OsRng;
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

fn parse_args() -> (u16, u16, u16) {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: {} <index> <total> <threshold>", args[0]);
        std::process::exit(1);
    }
    let index = args[1].parse().expect("Invalid index");
    let total = args[2].parse().expect("Invalid total");
    let threshold = args[3].parse().expect("Invalid threshold");
    (index, total, threshold)
}

// Simple send/recv helpers for TCP
fn send_to(addr: &str, data: &[u8]) {
    let mut stream = loop {
        if let Ok(s) = TcpStream::connect(addr) {
            break s;
        }
        thread::sleep(Duration::from_millis(100));
    };
    stream.write_all(data).unwrap();
}

fn recv_from(listener: &TcpListener, buf: &mut Vec<u8>) {
    let (mut stream, _) = listener.accept().unwrap();
    let mut tmp = [0u8; 4096];
    let n = stream.read(&mut tmp).unwrap();
    buf.extend_from_slice(&tmp[..n]);
}

fn main() {
    let (index, total, threshold) = parse_args();
    let my_addr = format!("127.0.0.1:1000{}", index);
    let mut rng = OsRng;

    // Listen for incoming connections
    let listener = TcpListener::bind(&my_addr).expect("Failed to bind");
    println!("Node {} listening on {}", index, my_addr);

    // Generate all shares and commitments using the trusted dealer approach
    let (shares, pubkey_package) = frost_core::keys::generate_with_dealer::<Ed25519Sha512, _>(
        total,
        threshold,
        frost_core::keys::IdentifierList::Default,
        &mut rng,
    )
    .expect("Dealer keygen failed");

    // Extract commitments map from the shares using public getter methods
    let mut commitments_map = BTreeMap::new();
    for share in shares.values() {
        commitments_map.insert(share.identifier(), share.commitment().clone());
    }

    // Each node picks its own identifier and share
    let my_identifier = match Identifier::<Ed25519Sha512>::try_from(index) {
        Ok(id) => id,
        Err(_) => {
            eprintln!("ERROR: Invalid identifier for index {}", index);
            std::process::exit(1);
        }
    };

    // Debug: print all identifiers in shares and the current identifier
    // Remove or comment out after debugging
    println!(
        "DEBUG: my_identifier = {:?}, shares.keys = {:?}",
        my_identifier,
        shares.keys().collect::<Vec<_>>()
    );

    let my_secret_share = match shares.get(&my_identifier) {
        Some(share) => share.clone(),
        None => {
            eprintln!(
                "ERROR: No secret share found for identifier {:?}. \
                This usually means each node generated its own random shares. \
                For a multi-process demo, generate shares once and distribute them to each node.",
                my_identifier
            );
            std::process::exit(1);
        }
    };
    let my_commitment = match commitments_map.get(&my_identifier) {
        Some(commitment) => commitment.clone(),
        None => {
            eprintln!(
                "ERROR: No commitment found for identifier {:?}. \
                This usually means each node generated its own random shares or commitments. \
                For a multi-process demo, generate shares and commitments once and distribute them to each node.",
                my_identifier
            );
            std::process::exit(1);
        }
    };

    // Broadcast commitment to all other nodes
    let commitment_bytes = bincode::serialize(&my_commitment).unwrap();
    for peer in 1..=total {
        if peer == index {
            continue;
        }
        let peer_id = Identifier::<Ed25519Sha512>::try_from(peer).unwrap();
        let peer_addr = format!("127.0.0.1:1000{}", peer);
        send_to(&peer_addr, &commitment_bytes);
    }

    // Receive commitments from others
    let mut all_commitments = BTreeMap::new();
    all_commitments.insert(my_identifier.clone(), my_commitment.clone());
    for i in 1..=total {
        if i == index {
            continue;
        }
        let mut buf = Vec::new();
        recv_from(&listener, &mut buf);
        let c: VerifiableSecretSharingCommitment<Ed25519Sha512> =
            bincode::deserialize(&buf).unwrap();
        let peer_id = Identifier::<Ed25519Sha512>::try_from(i).unwrap();
        all_commitments.insert(peer_id, c);
    }

    // DKG round: generate shares for each participant
    let mut shares_map = BTreeMap::new();
    for (peer_id, _) in &all_commitments {
        match shares.get(peer_id) {
            Some(share) => {
                shares_map.insert(peer_id.clone(), share.clone());
            }
            None => {
                eprintln!(
                    "ERROR: No share found for peer_id {:?} in shares. \
                    This usually means the identifier mapping is inconsistent. \
                    shares.keys = {:?}, all_commitments.keys = {:?}",
                    peer_id,
                    shares.keys().collect::<Vec<_>>(),
                    all_commitments.keys().collect::<Vec<_>>()
                );
                std::process::exit(1);
            }
        }
    }

    // Send shares to peers
    for i in 1..=total {
        if i == index {
            continue;
        }
        let peer_id = Identifier::<Ed25519Sha512>::try_from(i).unwrap();
        let share = shares_map.get(&peer_id).unwrap();
        let peer_addr = format!("127.0.0.1:1000{}", i);
        let share_bytes = bincode::serialize(share).unwrap();
        send_to(&peer_addr, &share_bytes);
    }

    // Receive shares from others
    let mut received_shares = BTreeMap::new();
    received_shares.insert(
        my_identifier.clone(),
        shares_map.get(&my_identifier).unwrap().clone(),
    );
    for i in 1..=total {
        if i == index {
            continue;
        }
        let mut buf = Vec::new();
        recv_from(&listener, &mut buf);
        let share: SecretShare<Ed25519Sha512> = bincode::deserialize(&buf).unwrap();
        let peer_id = Identifier::<Ed25519Sha512>::try_from(i).unwrap();
        received_shares.insert(peer_id, share);
    }

    // Manually construct KeyPackage for this participant
    let signing_share = received_shares
        .get(&my_identifier)
        .unwrap()
        .signing_share()
        .clone();

    // Convert all_commitments to the expected type: BTreeMap<Identifier, &VerifiableSecretSharingCommitment>
    let all_commitments_ref: BTreeMap<_, _> =
        all_commitments.iter().map(|(k, v)| (*k, v)).collect();

    let group_pubkey_pkg =
        PublicKeyPackage::<Ed25519Sha512>::from_dkg_commitments(&all_commitments_ref).unwrap();

    let verifying_share = group_pubkey_pkg
        .verifying_shares()
        .get(&my_identifier)
        .unwrap()
        .clone();
    let key_package = KeyPackage::new(
        my_identifier.clone(),
        signing_share,
        verifying_share,
        *group_pubkey_pkg.verifying_key(),
        threshold,
    );

    // Print key package and group public key
    println!(
        "Node {} KeyPackage: {}",
        index,
        hex::encode(key_package.serialize().unwrap())
    );
    println!(
        "Group PublicKey: {}",
        hex::encode(group_pubkey_pkg.verifying_key().serialize().unwrap())
    );
}
