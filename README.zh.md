<div align="center">

# YubiSign

**用你手上已有的 YubiKey,变成一个无助记词的多链硬件钱包。**

[English](README.md) | 简体中文

[![CI](https://github.com/stars-labs/yubisign/actions/workflows/ci.yml/badge.svg)](https://github.com/stars-labs/yubisign/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](#许可证)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange)](#)

为 **Solana、Ethereum、Sui、Bitcoin** 签名,密钥**在卡内生成、永不导出**——
没有助记词,不用浏览器插件,也不用再花钱买硬件钱包。

</div>

```console
$ yubisign list
OpenPGP (SIG slot):  Ethereum: 0xc370580ab2b42762347b76899abaa2a261c95c82
OpenPGP (AUT slot):  Ethereum: 0xeca4518f33df44ee11233139565a48b2225e389e
PIV slots (Ed25519 → Solana):
  slot 0x9a  Solana: CtodL3wQ1ySYhEfTwGdXJKnEGhbwdjhrRMGUTEGSzLNm
  slot 0x82  Solana: NYCYnX1iiUetEJJZo9f6fzX1KSXuGYepTWRHpncTJG6
  ... (24 个 PIV 账户)
```

## 为什么用 YubiSign

- 🌱 **没有助记词。** 丢币最常见的原因就是 24 词种子被弄丢或被偷。YubiSign 的
  密钥在 YubiKey 内部生成、无法导出——根本没有种子需要备份、泄露或被钓鱼。
- 🔌 **复用你已有的硬件。** 让 YubiKey 直接当硬件钱包,不用再买、再带一个设备。
- 🪪 **一把卡,多账户、多链。** 卡内 26 把独立密钥 → Solana / Sui / Aptos
  (Ed25519)与 Ethereum / Bitcoin / Cosmos(secp256k1)。
- 🦀 **开源 + 真机实证。** 极小的 Rust 内核,直接走 PC/SC 裸 APDU,不依赖厂商
  SDK。每条签名链路都在真实节点上验证过(见下)。

## 支持的链

| 体系 | 曲线 | Applet · 槽位 | 账户数 |
|------|------|---------------|--------|
| Solana、Sui、Aptos… | Ed25519 | PIV(固件 5.7+)`9A/9C/9D/9E`+`82`–`95` | ~24 |
| Ethereum、Bitcoin、Cosmos… | secp256k1 | OpenPGP `SIG`+`AUT` | ~2 |

> 没有 BIP32 派生——每个账户都是卡内一把独立密钥。secp256k1 账户受限于 OpenPGP
> 的槽位数;如果要**很多** EVM/BTC 账户请用支持 BIP32 的设备。YubiSign 的最佳
> 场景:在你已有的硬件上,放很多 Ed25519 账户 + 少量 secp256k1 账户。

## 快速上手

```bash
git clone https://github.com/stars-labs/yubisign && cd yubisign
cargo build --release -p yubisign

yubisign list                                                   # 列出所有账户
yubisign address --applet piv --slot 9a --curve ed25519         # 一个 Solana 地址
yubisign address --applet openpgp --slot sig --curve secp256k1  # 一把 Ethereum/Bitcoin 密钥
yubisign ssh-to-solana "ssh-ed25519 AAAA..."                    # SSH 公钥 → Solana 地址
```

前置条件:`pcscd` 运行中、用 `ykman` 配置 PIV、PIV 上的 Ed25519 需要固件
**5.7+**。完整文档、密钥配置和库 API:**[yubisign/README.md](yubisign/README.md)**。

## 真机验证

一把 YubiKey(固件 5.7.4)上的每个账户都在本地节点上**签名并广播了真实交易**,
共 50 笔:

| 链 | 节点 | 结果 |
|----|------|------|
| Solana | surfpool | **24/24 广播成功** |
| Sui | `sui` 本地网 | **24/24 执行成功** |
| Bitcoin | `bitcoind -regtest` | **2/2 确认** |
| Ethereum | anvil | 广播并打包 |

复现见 **[yubisign/multichain-tests/](yubisign/multichain-tests/README.md)**。

## 工作原理

| Applet · 槽 | 取公钥 | 签名 |
|-------------|--------|------|
| OpenPGP SIG | GET PUBLIC KEY(CRT `B6`) | PSO:COMPUTE DIGITAL SIGNATURE |
| OpenPGP AUT | GET PUBLIC KEY(CRT `A4`) | INTERNAL AUTHENTICATE |
| PIV 槽 | 解析槽内证书 | GENERAL AUTHENTICATE(扩展长度 APDU) |

各链的编码(地址推导、Sui 的 Blake2b intent 摘要、Bitcoin BIP143 + low-S DER、
Ethereum EIP-155 的 `v` 恢复)在 crate 文档中有说明。

## 许可证

MIT OR Apache-2.0
