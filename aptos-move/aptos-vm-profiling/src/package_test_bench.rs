use crate::utils::fs_read_benchmark_artifact;
use aptos_framework::extended_checks;
use aptos_gas_meter::{StandardGasAlgebra, StandardGasMeter};
use aptos_gas_schedule::{
    InitialGasSchedule, MiscGasParameters, NativeGasParameters, VMGasParameters,
    LATEST_GAS_FEATURE_VERSION,
};
use aptos_types::on_chain_config::{
    aptos_test_feature_flags_genesis, Features, TimedFeaturesBuilder,
};
use aptos_vm::natives;
use aptos_vm::natives::unit_test_extensions_hook;
use aptos_vm_types::resolver::NoopBlockSynchronizationKillSwitch;
use aptos_vm_types::storage::StorageGasParameters;
use clap::Args;
use legacy_move_compiler::compiled_unit::CompiledUnit;
use legacy_move_compiler::unit_test::NamedOrBytecodeModule;
use move_binary_format::CompiledModule;
use move_bytecode_utils::Modules;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::{IdentStr, Identifier};
use move_core_types::language_storage::ModuleId;
use move_core_types::value::{serialize_values, MoveValue};
use move_model::metadata::LanguageVersion;
use move_package::{BuildConfig, CompilerConfig};
use move_vm_runtime::config::VMConfig;
use move_vm_runtime::data_cache::TransactionDataCache;
use move_vm_runtime::module_traversal::{TraversalContext, TraversalStorage};
use move_vm_runtime::move_vm::MoveVM;
use move_vm_runtime::native_extensions::NativeContextExtensions;
use move_vm_runtime::native_functions::NativeFunctionTable;
use move_vm_runtime::{
    AsUnsyncModuleStorage, InstantiatedFunctionLoader, LazyLoader, LegacyLoaderConfig,
    RuntimeEnvironment,
};
use move_vm_test_utils::InMemoryStorage;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, Instant};
use move_vm_types::gas::UnmeteredGasMeter;

#[derive(Args)]
pub struct PackageTestCmd {
    #[clap(short, long, default_value_t = 5)]
    pub n_iterations: usize,

    /// Compile package and serialize it to the file system
    #[clap(long)]
    pub prepare: bool,

    #[clap(long, default_value_t = String::from("/home/mkurnikov/code/etna/move/perp"))]
    pub package_root: String,

    #[clap(long, default_value_t = String::from("0x4e110::twap_tests::test_twap_success"))]
    pub name: String,

    #[clap(long, default_values_t = ["0x4e110".to_string(), "0x456".to_string(), "0x789".to_string()], value_delimiter = ','
    )]
    pub signers: Vec<String>,
}

pub fn etna_bench(cmd: PackageTestCmd, artifacts_out: &PathBuf) -> anyhow::Result<()> {
    println!("Running Etna benchmark");

    let prepared_modules_path = artifacts_out.join("serialized_test_modules.mrb");

    if cmd.prepare {
        prepare_modules_to_fs(PathBuf::from(cmd.package_root), &prepared_modules_path);
        println!("Package modules has been cached to the filesystem.");
    }

    let serialized_modules = fs_read_benchmark_artifact(prepared_modules_path);

    let modules = bcs::from_bytes::<Vec<Vec<u8>>>(&serialized_modules)
        .unwrap()
        .into_iter()
        .map(|it| CompiledModule::deserialize(&it).unwrap())
        .collect::<Vec<_>>();

    let natives = aptos_debug_natives(NativeGasParameters::zeros(), MiscGasParameters::zeros());
    let genesis = aptos_test_feature_flags_genesis();
    let vm_config = VMConfig {
        paranoid_type_checks: false,
        // paranoid_ref_checks: false,
        ..VMConfig::default()
    };
    let runtime_environment = RuntimeEnvironment::new_with_config(natives, vm_config);
    assert!(
        runtime_environment.vm_config().enable_lazy_loading,
        "lazy_loading should be enabled, as we use hard-coded LazyLoader later"
    );
    let mut storage_state = setup_test_storage(modules.iter(), runtime_environment)?;
    storage_state.apply(genesis)?;

    let name_parts = cmd.name.split("::").collect::<Vec<_>>();
    assert_eq!(name_parts.len(), 3);
    let address = name_parts[0];
    let module_name = name_parts[1];
    let function_name = name_parts[2];

    let module_id = ModuleId::new(
        address_from_hex(address),
        Identifier::from_str(module_name).unwrap(),
    );
    let function_name = IdentStr::new(&function_name).unwrap();
    let signers = cmd
        .signers
        .iter()
        .map(|it| MoveValue::Signer(address_from_hex(it)))
        .collect::<Vec<_>>();
    let args = serialize_values(signers.iter());

    let mut iteration_times = vec![];
    for _i in 0..cmd.n_iterations {
        let iteration_elapsed = run_bench_once(
            storage_state.clone(),
            module_id.clone(),
            function_name,
            args.clone(),
        )?;

        iteration_times.push(iteration_elapsed);
    }
    println!("{:?}", iteration_times);
    println!("min: {:?}", iteration_times.iter().min().unwrap());

    Ok(())
}

fn prepare_modules_to_fs(package_root: PathBuf, out_path: &PathBuf) {
    let build_config = BuildConfig {
        skip_fetch_latest_git_deps: true,
        dev_mode: true,
        test_mode: true,
        compiler_config: CompilerConfig {
            language_version: Some(LanguageVersion::V2_2),
            known_attributes: extended_checks::get_all_attribute_names().clone(),
            ..CompilerConfig::default()
        },
        ..BuildConfig::default()
    };

    let mut writer = std::io::stdout();
    // saves to disk internally
    let compiled_package = build_config
        .compile_package(&package_root, &mut writer)
        .expect("package serialized into build/ directory");
    let module_info = compiled_package
        .all_compiled_units()
        .filter_map(|unit| match unit {
            CompiledUnit::Module(m) => {
                Some((m.module.self_id(), NamedOrBytecodeModule::Named(m.clone())))
            },
            _ => None,
        })
        .chain(
            compiled_package
                .bytecode_deps
                .clone()
                .into_values()
                .into_iter()
                .map(|module| (module.self_id(), NamedOrBytecodeModule::Bytecode(module))),
        )
        .collect::<BTreeMap<_, _>>();
    let modules = module_info.values().map(|info| match info {
        NamedOrBytecodeModule::Named(named_compiled_module) => &named_compiled_module.module,
        NamedOrBytecodeModule::Bytecode(compiled_module) => compiled_module,
    });
    let serialized_modules = modules
        .map(|it| {
            let mut bin = vec![];
            it.serialize_for_version(Some(8), &mut bin).unwrap();
            bin
        })
        .collect::<Vec<_>>();
    fs::write(out_path, bcs::to_bytes(&serialized_modules).unwrap()).unwrap();
}

fn run_bench_once(
    storage: InMemoryStorage,
    module_id: ModuleId,
    function_name: &IdentStr,
    args: Vec<Vec<u8>>,
) -> anyhow::Result<Duration> {
    let module_storage = storage.as_unsync_module_storage();
    let traversal_storage = TraversalStorage::new();

    let mut extensions = NativeContextExtensions::default();
    unit_test_extensions_hook(&mut extensions);

    let mut vm_gas_params = VMGasParameters::initial();
    vm_gas_params.txn.max_execution_gas = u64::MAX.into();
    vm_gas_params.txn.max_io_gas = u64::MAX.into();

    let mut gas_meter = StandardGasMeter::new(StandardGasAlgebra::new(
        LATEST_GAS_FEATURE_VERSION,
        vm_gas_params,
        StorageGasParameters::unlimited(),
        false,
        10_000_000_000_000,
        &NoopBlockSynchronizationKillSwitch {},
    ));

    let mut traversal_context = TraversalContext::new(&traversal_storage);
    let mut data_cache = TransactionDataCache::empty();

    let loader = LazyLoader::new(&module_storage);
    let function = loader.load_instantiated_function(
        &LegacyLoaderConfig::unmetered(),
        &mut gas_meter,
        &mut traversal_context,
        &module_id,
        function_name,
        // No type args for now.
        &[],
    )?;

    let before = Instant::now();
    let _ = MoveVM::execute_loaded_function(
        function,
        args,
        &mut data_cache,
        &mut gas_meter,
        &mut traversal_context,
        &mut extensions,
        &loader,
        &storage,
    )?;

    let elapsed = before.elapsed();
    Ok(elapsed)
}

fn aptos_debug_natives(
    native_gas_parameters: NativeGasParameters,
    misc_gas_params: MiscGasParameters,
) -> NativeFunctionTable {
    // As a side effect, also configure for unit testing
    // natives::configure_for_unit_test();
    // extended_checks::configure_extended_checks_for_unit_test();
    // Return all natives -- build with the 'testing' feature, therefore containing
    // debug related functions.
    natives::aptos_natives(
        LATEST_GAS_FEATURE_VERSION,
        native_gas_parameters,
        misc_gas_params,
        TimedFeaturesBuilder::enable_all().build(),
        Features::default(),
    )
}

/// Setup storage state with the set of modules that will be needed for all tests
fn setup_test_storage<'a>(
    modules: impl Iterator<Item = &'a CompiledModule>,
    runtime_environment: RuntimeEnvironment,
) -> anyhow::Result<InMemoryStorage> {
    let mut storage = InMemoryStorage::new_with_runtime_environment(runtime_environment);
    let modules = Modules::new(modules);
    for module in modules
        .compute_dependency_graph()
        .compute_topological_order()?
    {
        let mut module_bytes = Vec::new();
        module.serialize_for_version(Some(module.version), &mut module_bytes)?;
        storage.add_module_bytes(module.self_addr(), module.self_name(), module_bytes.into());
    }

    Ok(storage)
}

#[inline]
fn address_from_hex(hex: &str) -> AccountAddress {
    AccountAddress::from_hex_literal(hex).unwrap()
}
