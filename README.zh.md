<div align="center">

# YubiWallet

**用你手上已有的 YubiKey,变成一个无助记词的多链硬件钱包。**

[English](README.md) | 简体中文

[![CI](https://github.com/stars-labs/yubiwallet/actions/workflows/ci.yml/badge.svg)](https://github.com/stars-labs/yubiwallet/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/yubiwallet.svg?logo=rust)](https://crates.io/crates/yubiwallet)
[![npm](https://img.shields.io/npm/v/@stars-labs/yubiwallet-wasm.svg?logo=npm)](https://www.npmjs.com/package/@stars-labs/yubiwallet-wasm)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](#许可证)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange)](#)

为 **Solana、Ethereum、Sui、Bitcoin** 签名,密钥**在卡内生成、永不导出**——
没有助记词,不用浏览器插件,也不用再花钱买硬件钱包。

</div>

```console
$ yubiwallet list
OpenPGP (SIG slot):  Ethereum: 0xc370580ab2b42762347b76899abaa2a261c95c82
OpenPGP (AUT slot):  Ethereum: 0xeca4518f33df44ee11233139565a48b2225e389e
PIV slots (Ed25519 → Solana):
  slot 0x9a  Solana: CtodL3wQ1ySYhEfTwGdXJKnEGhbwdjhrRMGUTEGSzLNm
  slot 0x82  Solana: NYCYnX1iiUetEJJZo9f6fzX1KSXuGYepTWRHpncTJG6
  ... (24 个 PIV 账户)
```

## 为什么用 YubiWallet

- 🌱 **没有助记词。** 丢币最常见的原因就是 24 词种子被弄丢或被偷。YubiWallet 的
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
> 的槽位数;如果要**很多** EVM/BTC 账户请用支持 BIP32 的设备。YubiWallet 的最佳
> 场景:在你已有的硬件上,放很多 Ed25519 账户 + 少量 secp256k1 账户。

## 快速上手

```bash
git clone https://github.com/stars-labs/yubiwallet && cd yubiwallet
cargo build --release -p yubiwallet

yubiwallet list                                                   # 列出所有账户
yubiwallet address --applet piv --slot 9a --curve ed25519         # 一个 Solana 地址
yubiwallet address --applet openpgp --slot sig --curve secp256k1  # 一把 Ethereum/Bitcoin 密钥
yubiwallet ssh-to-solana "ssh-ed25519 AAAA..."                    # SSH 公钥 → Solana 地址
```

前置条件:`pcscd` 运行中、用 `ykman` 配置 PIV、PIV 上的 Ed25519 需要固件
**5.7+**。完整文档、密钥配置和库 API:**[yubiwallet/README.md](yubiwallet/README.md)**。

## 真机验证

一把 YubiKey(固件 5.7.4)上的每个账户都在本地节点上**签名并广播了真实交易**,
共 50 笔:

| 链 | 节点 | 结果 |
|----|------|------|
| Solana | surfpool | **24/24 广播成功** |
| Sui | `sui` 本地网 | **24/24 执行成功** |
| Bitcoin | `bitcoind -regtest` | **2/2 确认** |
| Ethereum | anvil | 广播并打包 |

复现见 **[yubiwallet/multichain-tests/](yubiwallet/multichain-tests/README.md)**。

## 工作原理

| Applet · 槽 | 取公钥 | 签名 |
|-------------|--------|------|
| OpenPGP SIG | GET PUBLIC KEY(CRT `B6`) | PSO:COMPUTE DIGITAL SIGNATURE |
| OpenPGP AUT | GET PUBLIC KEY(CRT `A4`) | INTERNAL AUTHENTICATE |
| PIV 槽 | 解析槽内证书 | GENERAL AUTHENTICATE(扩展长度 APDU) |

各链的编码(地址推导、Sui 的 Blake2b intent 摘要、Bitcoin BIP143 + low-S DER、
Ethereum EIP-155 的 `v` 恢复)在 crate 文档中有说明。

## 接入你的应用

YubiWallet 是一个可嵌入的 Rust 内核——在 Web 端,还能让浏览器里的 **dApp** 直接
驱动 YubiKey 签名。按你代码运行的位置选择对应的接入方式:

| 你在写… | 用 | 作用 |
|---------|-----|------|
| 原生应用(Rust) | [`yubiwallet`](https://crates.io/crates/yubiwallet) crate | 直接与卡通信 |
| 浏览器扩展 | **native-messaging host** | 在浏览器与卡之间架桥 |
| 钱包 UI / dApp 逻辑(JS) | [`@stars-labs/yubiwallet-wasm`](https://www.npmjs.com/package/@stars-labs/yubiwallet-wasm) 包 | 地址推导 + 签名恢复(不签名) |

浏览器无法直接访问 YubiKey 的 PIV/OpenPGP 接口,所以扩展通过 Native Messaging
与本地的小程序 `yubiwallet-host` 通信,由它完成与卡的 I/O——**PIN 由 host 自己
收集,绝不经过扩展或网络**。开箱即用的模板(manifest + 跨浏览器安装脚本、带类型
的 TS 客户端、WASM 构建与用法)都在 **[integration/](integration/README.md)**;
完整设计见 **[docs/dapp-bridge-design.md](docs/dapp-bridge-design.md)**。

## 常见问题(备份 / 丢失 / 损坏)

**卡内生成的密钥能备份吗?**
取决于你怎么配置这个槽:
- **卡内生成**(`ykman piv keys generate`)——私钥在安全芯片里生成,**永远无法
  导出**。安全性最强,但**没有备份**:这把密钥没了,对应地址里的资产就无法找回。
- **卡外生成 + 导入**——先在软件里生成密钥、做好加密备份,再导入到一把或多把
  YubiKey。用一点安全性(密钥曾短暂离开设备)换取可恢复性。

按账户取舍:需要可恢复的资金 → 卡外生成 + 导入到 ≥2 把卡;冷存 / 极致安全 →
卡内生成,并接受"没有备份"。

**YubiKey 丢了怎么办?**
它有 PIN 保护——捡到的人没有 PIN 无法签名,且连续输错会锁死 applet(把 PUK /
Admin PIN 也耗尽会触发 reset,**直接清空密钥**)。所以:
- 有**备份**(同一把密钥导入到第二把卡,或有加密备份)?立刻用它把资产转到新地址。
- **只在卡内、没有备份?** 那你自己也转不出来——按丢失处理。这就是为什么有点
  余额就一定要做冗余。
- 永远要把默认 PIN 改掉。

**YubiKey 坏了 / 失效怎么办?**
和丢失一样:有卡外备份的密钥可以恢复到新卡;只在卡内的密钥无法恢复。建议常备
第二把已配置好的卡做冗余。

**别人能复制我的 YubiKey 吗?**
不能。安全芯片里的密钥不可导出,无法被拷贝出设备。

**忘记 PIN 了?**
- PIV:用 **PUK** 解锁(`ykman piv access unblock-pin`);PUK 也耗尽就只能 reset
  PIV applet(清空密钥)。
- OpenPGP:用 **Admin PIN(PW3)** 重置用户 PIN;PW3 丢了就只能 reset OpenPGP
  applet(清空密钥)。
所以 PIN / PUK / Admin PIN 也要安全地备份好。

**推荐的备份策略?**
1. **冗余**——卡外生成,把同一把密钥导入到 2–3 把 YubiKey,分地点保管。
2. **加密备份**——用 `age` / `gpg` 把卡外私钥加密后存到 ≥2 个地方。
3. **多签**——用钱包多签(Solana Squads、Gnosis Safe 等),不同 YubiKey 各持一把:
   丢一把设备、或一把密钥被攻破,都不会丢资产。大额首选。

**YubiWallet 会把我的密钥或助记词存在哪里吗?**
不会。没有助记词,YubiWallet 也不写任何密钥文件——它只和卡通信,签名在卡内完成
(输入 PIN 后)。

## 贡献与安全

欢迎贡献,见 [CONTRIBUTING.md](CONTRIBUTING.md)。依赖精简原则与威胁模型见
[SECURITY.md](SECURITY.md);安全问题请按该文件**私下**报告。

## 许可证

MIT OR Apache-2.0
