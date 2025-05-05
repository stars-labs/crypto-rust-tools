// ANCHOR: dkg_import
use std::collections::BTreeMap;

use frost_secp256k1 as frost;
// Add Debug derive to DKG packages if not already present in the library
// (Assuming they might not have it based on the print statements)
// If they already derive Debug, this change is not needed.
// Example (conceptual - apply if needed in frost_secp256k1 library or locally if possible):
// #[derive(Clone, Debug, Serialize, Deserialize)] pub struct Round1Package { ... }
// #[derive(Clone, Debug, Serialize, Deserialize)] pub struct Round2Package { ... }
// #[derive(Clone, Debug, Serialize, Deserialize)] pub struct KeyPackage { ... }
// #[derive(Clone, Debug, Serialize, Deserialize)] pub struct PublicKeyPackage { ... }

fn main() -> Result<(), frost::Error> {
    let mut rng = rand::rngs::OsRng;

    let max_signers = 5;
    let min_signers = 3;
    println!(
        "Starting Secp256k1 DKG with max_signers={}, min_signers={}",
        max_signers, min_signers
    );
    // ANCHOR_END: dkg_import

    ////////////////////////////////////////////////////////////////////////////
    // Key generation, Round 1
    ////////////////////////////////////////////////////////////////////////////
    println!("\n=== DKG Round 1 ===");

    // Keep track of each participant's round 1 secret package.
    // In practice each participant will keep its copy; no one
    // will have all the participant's packages.
    let mut round1_secret_packages = BTreeMap::new();

    // Keep track of all round 1 packages sent to the given participant.
    // This is used to simulate the broadcast; in practice the packages
    // will be sent through some communication channel.
    let mut received_round1_packages = BTreeMap::new();

    // For each participant, perform the first part of the DKG protocol.
    // In practice, each participant will perform this on their own environments.
    for participant_index in 1..=max_signers {
        let participant_identifier = participant_index.try_into().expect("should be nonzero");
        println!(
            "\nParticipant {:?} generating Round 1 package...",
            participant_identifier
        );
        // ANCHOR: dkg_part1
        let (round1_secret_package, round1_package) =
            frost::keys::dkg::part1(participant_identifier, max_signers, min_signers, &mut rng)?;
        // ANCHOR_END: dkg_part1
        println!(
            "  Participant {:?} generated Round 1 package: {:?}", // Using Debug for Round1Package
            participant_identifier, round1_package
        );

        // Store the participant's secret package for later use.
        // In practice each participant will store it in their own environment.
        round1_secret_packages.insert(participant_identifier, round1_secret_package);

        // "Send" the round 1 package to all other participants. In this
        // test this is simulated using a BTreeMap; in practice this will be
        // sent through some communication channel.
        println!(
            "  Participant {:?} broadcasting Round 1 package...",
            participant_identifier
        );
        for receiver_participant_index in 1..=max_signers {
            if receiver_participant_index == participant_index {
                continue;
            }
            let receiver_participant_identifier: frost::Identifier = receiver_participant_index
                .try_into()
                .expect("should be nonzero");
            received_round1_packages
                .entry(receiver_participant_identifier)
                .or_insert_with(BTreeMap::new)
                .insert(participant_identifier, round1_package.clone());
        }
    }
    println!(
        "\nRound 1 broadcast complete. Received packages:\n{:#?}", // Pretty print received packages
        received_round1_packages
    );

    ////////////////////////////////////////////////////////////////////////////
    // Key generation, Round 2
    ////////////////////////////////////////////////////////////////////////////
    println!("\n=== DKG Round 2 ===");

    // Keep track of each participant's round 2 secret package.
    // In practice each participant will keep its copy; no one
    // will have all the participant's packages.
    let mut round2_secret_packages = BTreeMap::new();

    // Keep track of all round 2 packages sent to the given participant.
    // This is used to simulate the broadcast; in practice the packages
    // will be sent through some communication channel.
    let mut received_round2_packages = BTreeMap::new();

    // For each participant, perform the second part of the DKG protocol.
    // In practice, each participant will perform this on their own environments.
    for participant_index in 1..=max_signers {
        let participant_identifier = participant_index.try_into().expect("should be nonzero");
        println!(
            "\nParticipant {:?} processing received Round 1 packages and generating Round 2 packages...",
            participant_identifier
        );
        let round1_secret_package = round1_secret_packages
            .remove(&participant_identifier)
            .expect("Secret package should exist"); // Use expect for clarity
        let round1_packages = &received_round1_packages[&participant_identifier];
        // ANCHOR: dkg_part2
        let (round2_secret_package, round2_packages) =
            frost::keys::dkg::part2(round1_secret_package, round1_packages)?;
        // ANCHOR_END: dkg_part2
        println!(
            "  Participant {:?} generated Round 2 packages for others: {:?}",
            participant_identifier,
            round2_packages.keys().collect::<Vec<_>>() // Show who packages are for
        );

        // Store the participant's secret package for later use.
        // In practice each participant will store it in their own environment.
        round2_secret_packages.insert(participant_identifier, round2_secret_package);

        // "Send" the round 2 package to all other participants. In this
        // test this is simulated using a BTreeMap; in practice this will be
        // sent through some communication channel.
        // Note that, in contrast to the previous part, here each other participant
        // gets its own specific package.
        println!(
            "  Participant {:?} sending Round 2 packages...",
            participant_identifier
        );
        for (receiver_identifier, round2_package) in round2_packages {
            received_round2_packages
                .entry(receiver_identifier)
                .or_insert_with(BTreeMap::new)
                .insert(participant_identifier, round2_package);
        }
    }
    println!(
        "\nRound 2 sending complete. Received packages:\n{:#?}", // Pretty print received packages
        received_round2_packages
    );

    ////////////////////////////////////////////////////////////////////////////
    // Key generation, final computation
    ////////////////////////////////////////////////////////////////////////////
    println!("\n=== DKG Final Computation ===");

    // Keep track of each participant's long-lived key package.
    // In practice each participant will keep its copy; no one
    // will have all the participant's packages.
    let mut key_packages = BTreeMap::new();

    // Keep track of each participant's public key package.
    // In practice, if there is a Coordinator, only they need to store the set.
    // If there is not, then all candidates must store their own sets.
    // All participants will have the same exact public key package.
    let mut pubkey_packages = BTreeMap::new();

    // For each participant, perform the third part of the DKG protocol.
    // In practice, each participant will perform this on their own environments.
    for participant_index in 1..=max_signers {
        let participant_identifier = participant_index.try_into().expect("should be nonzero");
        println!(
            "\nParticipant {:?} performing final computation...",
            participant_identifier
        );
        let round2_secret_package = &round2_secret_packages[&participant_identifier];
        let round1_packages = &received_round1_packages[&participant_identifier];
        let round2_packages = &received_round2_packages[&participant_identifier];
        // ANCHOR: dkg_part3
        let (key_package, pubkey_package) =
            frost::keys::dkg::part3(round2_secret_package, round1_packages, round2_packages)?;
        // ANCHOR_END: dkg_part3
        println!(
            "  Participant {:?} generated KeyPackage: {:?}", // Using Debug for KeyPackage
            participant_identifier, key_package
        );
        println!(
            "  Participant {:?} generated PublicKeyPackage: {:?}", // Using Debug for PublicKeyPackage
            participant_identifier, pubkey_package
        );
        key_packages.insert(participant_identifier, key_package);
        pubkey_packages.insert(participant_identifier, pubkey_package);
    }

    println!("\n=== DKG Complete ===");
    println!("Generated Key Packages:\n{:#?}", key_packages); // Pretty print final key packages
    println!("Generated Public Key Packages:\n{:#?}", pubkey_packages); // Pretty print final public key packages

    // Verify all participants have the same public key package
    let first_pubkey_package = pubkey_packages.values().next().unwrap();
    for (id, pkp) in &pubkey_packages {
        if pkp != first_pubkey_package {
            // Use panic! in example for immediate feedback on failure
            panic!(
                "Error: Participant {:?} has a different PublicKeyPackage!",
                id
            );
        }
    }
    println!("\nAll participants generated the same PublicKeyPackage successfully.");
    println!(
        "Group Verifying Key: {:?}",
        first_pubkey_package.verifying_key() // Print the group verifying key
    );

    // With its own key package and the pubkey package, each participant can now proceed
    // to sign with FROST.
    println!("\nSecp256k1 DKG process finished successfully.");
    Ok::<(), frost::Error>(())
}
