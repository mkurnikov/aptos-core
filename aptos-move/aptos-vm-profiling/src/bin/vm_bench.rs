// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use aptos_vm_profiling::coin_transfer::CoinTransferCmd;
use aptos_vm_profiling::package_test_bench::PackageTestCmd;
use aptos_vm_profiling::{coin_transfer, package_test_bench};
use clap::{Parser, Subcommand};
use std::env;
use std::path::Path;

#[derive(Parser)]
struct CliParams {
    #[clap(short, long, default_value_t = 5)]
    pub n_iterations: usize,

    #[clap(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    PackageTest(PackageTestCmd),
    CoinTransfer(CoinTransferCmd),
}

const PACKAGE_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn main() -> Result<()> {
    let args = CliParams::parse();

    let artifacts_out = Path::new(PACKAGE_ROOT).join("out");
    match args.cmd {
        Cmd::PackageTest(cmd) => package_test_bench::etna_bench(cmd, &artifacts_out)?,
        Cmd::CoinTransfer(cmd) => coin_transfer::run_transactions_bench(cmd, &artifacts_out)?,
    }

    Ok(())
}
