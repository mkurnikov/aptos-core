use crate::utils::fs_read_benchmark_artifact;
use aptos_block_executor::txn_provider::default::DefaultTxnProvider;
use aptos_cached_packages::head_release_bundle;
use aptos_framework::{BuildOptions, BuiltPackage};
use aptos_keygen::KeyGen;
use aptos_transaction_simulation::{AccountData, InMemoryStateStore, SimulationStateStore};
use aptos_types::transaction::signature_verified_transaction::SignatureVerifiedTransaction;
use aptos_types::transaction::{AuxiliaryInfo, Script, Transaction, TransactionPayload};
use aptos_types::write_set::WriteSet;
use aptos_vm::aptos_vm::AptosVMBlockExecutor;
use aptos_vm::VMBlockExecutor;
use aptos_vm_genesis::generate_test_genesis;
use clap::Args;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{env, fs};

const PACKAGE_ROOT: &str = env!("CARGO_MANIFEST_DIR");
const CACHED_TRANSACTIONS_NAME: &str = "serialized_txns.mrb";
const CACHED_GENESIS_NAME: &str = "genesis.mrb";

const NUM_TXNS: u64 = 10;
const ALICE_SEED: [u8; 32] = [9; 32];
const BOB_SEED: [u8; 32] = [10; 32];

#[derive(Args)]
pub struct CoinTransferCmd {
    #[clap(short, long, default_value_t = 5)]
    pub n_iterations: usize,

    /// Compile package and serialize it to the file system
    #[clap(long)]
    pub prepare: bool,
}

#[inline(never)]
pub fn run_transactions_bench(cmd: CoinTransferCmd, artifacts_out: &PathBuf) -> anyhow::Result<()> {
    println!("Running aptos_coin::transfer transactions benchmark");

    if cmd.prepare {
        prepare_txns(&artifacts_out);
        println!("Transactions has been cached to the filesystem.");
    }

    let genesis_blob = fs_read_benchmark_artifact(artifacts_out.join(CACHED_GENESIS_NAME));
    let genesis_write_set: WriteSet = bcs::from_bytes(&genesis_blob)?;

    let serialized_txns = fs_read_benchmark_artifact(artifacts_out.join(CACHED_TRANSACTIONS_NAME));
    let txns: Vec<SignatureVerifiedTransaction> = bcs::from_bytes(&serialized_txns)?;

    let state_store = InMemoryStateStore::new();
    state_store.apply_write_set(&genesis_write_set)?;

    let alice = AccountData::new_from_seed(&mut KeyGen::from_seed(ALICE_SEED), 100_000_000, 0);
    let bob = AccountData::new_from_seed(&mut KeyGen::from_seed(BOB_SEED), 100_000_000, 0);

    state_store.add_account_data(&alice)?;
    state_store.add_account_data(&bob)?;

    let vm_executor = AptosVMBlockExecutor::new();
    let txn_provider: DefaultTxnProvider<SignatureVerifiedTransaction, AuxiliaryInfo> =
        DefaultTxnProvider::new_without_info(txns);

    let mut iteration_times = vec![];
    for _i in 0..cmd.n_iterations {
        let iteration_elapsed =
            run_transactions_bench_once(&vm_executor, state_store.clone(), txn_provider.clone())?;
        iteration_times.push(iteration_elapsed);
    }
    println!("{:?}", iteration_times);
    println!("min: {:?}", iteration_times.iter().min().unwrap());
    Ok(())
}

#[inline(never)]
fn run_transactions_bench_once(
    vm_executor: &AptosVMBlockExecutor,
    state_store: InMemoryStateStore,
    txn_provider: DefaultTxnProvider<SignatureVerifiedTransaction, AuxiliaryInfo>,
) -> anyhow::Result<Duration> {
    let num_txns = txn_provider.get_txns().len();
    let before = Instant::now();

    let outputs = vm_executor.execute_block_no_limit(&txn_provider, &state_store)?;

    let elapsed = before.elapsed();
    for i in 0..num_txns {
        assert!(
            outputs[i].status().status().unwrap().is_success(),
            "transaction number {i} failed"
        );
    }

    Ok(elapsed)
}

fn prepare_txns(artifacts_out: &PathBuf) {
    if !artifacts_out.exists() {
        fs::create_dir(&artifacts_out).unwrap();
    }

    let bench_transaction_root = Path::new(&PACKAGE_ROOT).join("bench-transaction");
    let build_options = BuildOptions {
        skip_fetch_latest_git_deps: true,
        ..BuildOptions::default()
    };
    let built_package = BuiltPackage::build(bench_transaction_root.clone(), build_options).unwrap();
    let first_script_bytecode = built_package.extract_script_code()[0].clone();

    let (genesis, _) = generate_test_genesis(head_release_bundle(), Some(1));
    let genesis_blob = bcs::to_bytes(genesis.write_set()).unwrap();
    fs::write(artifacts_out.join(CACHED_GENESIS_NAME), genesis_blob)
        .expect("location should be writeable");

    let alice = AccountData::new_from_seed(&mut KeyGen::from_seed(ALICE_SEED), 100_000_000, 0);
    // let bob = AccountData::new_from_seed(&mut KeyGen::from_seed(BOB_SEED), 100_000_000, 0);

    let txns: Vec<SignatureVerifiedTransaction> = (0..NUM_TXNS)
        .map(|seq_num| {
            let payload = TransactionPayload::Script(Script::new(
                first_script_bytecode.clone(),
                vec![],
                vec![],
            ));
            Transaction::UserTransaction(
                alice
                    .account()
                    .transaction()
                    .gas_unit_price(100)
                    .payload(payload)
                    .sequence_number(seq_num)
                    .sign(),
            )
            .into()
        })
        .collect();
    let txns = bcs::to_bytes(&txns).unwrap();
    fs::write(
        Path::new(&artifacts_out).join(CACHED_TRANSACTIONS_NAME),
        txns,
    )
    .expect("location should be writeable");
}
