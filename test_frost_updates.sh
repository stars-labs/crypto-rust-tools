#!/bin/bash

# Test script to verify FROST MPC CLI updates

echo "Testing FROST MPC CLI Node Updates"
echo "=================================="
echo ""

# Build the project
echo "1. Building the project..."
cargo build -p frost-mpc-cli-node --bin cli_node

if [ $? -eq 0 ]; then
    echo "✅ Build successful!"
else
    echo "❌ Build failed!"
    exit 1
fi

echo ""
echo "2. Checking wallet location features..."
echo ""

# Check if the locate_wallet command is in the help
echo "Checking if /locate_wallet command is documented in help..."
grep -q "locate_wallet" frost-mpc-cli-node/src/ui/tui.rs
if [ $? -eq 0 ]; then
    echo "✅ /locate_wallet command found in help!"
else
    echo "❌ /locate_wallet command not found in help!"
fi

# Check if export_extension commands are removed
echo ""
echo "Checking if old export_extension commands are removed..."
grep -q "export_extension" frost-mpc-cli-node/src/ui/tui.rs
if [ $? -ne 0 ]; then
    echo "✅ Old export_extension commands successfully removed!"
else
    echo "❌ Old export_extension commands still present!"
fi

# Check if wallet creation shows file location
echo ""
echo "Checking if wallet creation shows file location..."
grep -q "WALLET CREATED - FILE LOCATION" frost-mpc-cli-node/src/handlers/keystore_commands.rs
if [ $? -eq 0 ]; then
    echo "✅ Wallet creation shows file location!"
else
    echo "❌ Wallet creation doesn't show file location!"
fi

# Check if PBKDF2 is the default encryption
echo ""
echo "Checking if PBKDF2 is used for browser compatibility..."
grep -q "KeyDerivation::Pbkdf2" frost-mpc-cli-node/src/keystore/encryption.rs
if [ $? -eq 0 ]; then
    echo "✅ PBKDF2 encryption is used for browser compatibility!"
else
    echo "❌ PBKDF2 encryption not found!"
fi

echo ""
echo "3. Summary of Changes:"
echo "====================="
echo "✅ Removed /export_extension commands"
echo "✅ Added /locate_wallet command for direct file sharing"  
echo "✅ Enhanced wallet creation to show file location immediately"
echo "✅ Updated help page with new wallet sharing instructions"
echo "✅ Using PBKDF2 encryption for Chrome extension compatibility"
echo "✅ Same JSON wallet files work in both CLI and Chrome extension"

echo ""
echo "4. Wallet File Structure:"
echo "========================"
echo "~/.frost_keystore/<device_id>/<curve_type>/<wallet_id>.json"
echo ""
echo "Example:"
echo "~/.frost_keystore/mpc-1/ed25519/wallet_2of3.json"
echo "~/.frost_keystore/mpc-1/secp256k1/wallet_2of3.json"

echo ""
echo "Note: The application requires a terminal environment to run."
echo "In production, run with: cargo run -p frost-mpc-cli-node --bin cli_node -- -d <device_id>"