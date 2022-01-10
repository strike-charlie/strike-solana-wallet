<p align="center">
  <a href="https://strikeprotocols.com">
    <img alt="Strike" src="https://strikeprotocols.com/wp-content/uploads/2021/11/strike-4C-whitetype@4x.png" width="250" />
  </a>
</p>

# Overview

The Strike Wallet is a Solana multi-approver program-based wallet suitable for
use by institutions and enterprises requiring highly-secure access to the
Solana ecosystem. It supports SOL and SPL tokens, staking and dApps. The
multi-approver protocol applies to transfers and dApp transactions, policy
changes, and recovery, with a different approver policy for each of these.

# Building

## **1. Install rustc, cargo and rustfmt.**

```bash
$ curl https://sh.rustup.rs -sSf | sh
$ source $HOME/.cargo/env
$ rustup component add rustfmt
```

## **2. Download the source code.**

```bash
$ git clone https://github.com/StrikeProtocols/strike-wallet.git
$ cd strike-wallet
```

## **3. Build**

```bash
$ make build
```

# Testing

## **1. Install the [solana CLI tools](https://docs.solana.com/cli/install-solana-cli-tools)**

## **2. Start the local test validator**

```bash
$ solana-test-validator
```

## **3. In a new terminal, run the test suite**

```bash
$ make test
```

## **4. Code coverage**

```bash
$ make coverage
```

Afterwards, coverage report is available in `target/cov/LATEST/`.

# Getting Help

Join us on our [Discord server](https://discord.gg/aVBUCmNU)
