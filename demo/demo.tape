# YubiSign demo — render a GIF with: vhs demo/demo.tape
# Requires https://github.com/charmbracelet/vhs and a YubiKey inserted.
# `list`/`address` read public keys only (no PIN); ssh-to-solana needs no card.
Output demo/yubisign.gif

Set FontSize 16
Set Width 1100
Set Height 720
Set Theme "Dracula"
Set Padding 20

# Release the card from gpg's scdaemon so the tool can talk to it.
Hide
Type "gpgconf --kill scdaemon 2>/dev/null; clear"
Enter
Show

Type "# Your YubiKey is now a multi-chain hardware wallet"
Enter
Sleep 1500ms

Type "yubisign list"
Enter
Sleep 3s

Type "yubisign address --applet openpgp --slot sig --curve secp256k1"
Enter
Sleep 3s

Type 'yubisign ssh-to-solana "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICKoK0n5DHVMeDCw3XVZUYxhgdQIYJADIyaSF7J8Rz1E"'
Enter
Sleep 3s
