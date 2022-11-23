use std::collections::{BTreeMap, HashMap};
use std::default::Default;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, ensure};
use async_trait::async_trait;
use clap::Parser;
use convert_case::{Case, Casing};
use path_absolutize::Absolutize;
use tera::{Context, Tera};
use termcolor::{BufferWriter, Color, ColorChoice, ColorSpec, WriteColor};
use tokio::try_join;
use walkdir::WalkDir;

use crate::common::init::Network;
use crate::common::types::{
    CliCommand, CliConfig, CliError, CliTypedResult, ConfigSearchMode, EncodingOptions,
    MoveManifestAccountWrapper, PrivateKeyInputOptions, ProfileOptions, PromptOptions, RngArgs,
};
use crate::common::utils::read_line;
use crate::move_tool::FrameworkPackageArgs;

const GIT_TEMPLATE: &str = "https://github.com/mkurnikov/aptos-templates.git";

/// Creates a new "Move" package at the given location.
///
/// Examples:
/// $ aptos new my_package
/// $ aptos new ~/demo/my_package2 --named_addresses self=_,std=0x1
/// $ aptos new /tmp/my_package3 --name DemoPackage --assume-yes
/// $ aptos new /tmp/my_package --name ExampleProject --example-script true --example-coin true --assume-yes --skip-profile-creation
#[derive(Parser)]
#[clap(verbatim_doc_comment)]
pub struct NewPackage {
    /// Directory to create the new Move package
    /// The folder name can be used as the package name.
    /// If the directory does not exist, it will be created
    ///
    /// Example:
    /// my_project
    /// ~/path/to/my_new_package
    /// /tmp/project_1
    #[clap(verbatim_doc_comment, value_parser)]
    pub(crate) package_dir: PackageDir,

    /// Name of the new Move package
    #[clap(long, display_order = 1)]
    pub(crate) name: Option<String>,

    /// Add an example with dApp to the package
    #[clap(long, display_order = 2)]
    pub(crate) add_js: Option<bool>,

    /// Add an example with coins to the package
    #[clap(long, display_order = 3)]
    pub(crate) add_coin: Option<bool>,

    /// Do not create a "default" profile
    #[clap(long, display_order = 4)]
    pub(crate) create_profile: Option<bool>,

    #[clap(flatten)]
    pub(crate) framework_package_args: FrameworkPackageArgs,
}

#[async_trait]
impl CliCommand<()> for NewPackage {
    fn command_name(&self) -> &'static str {
        "NewPackage"
    }

    async fn execute(self) -> CliTypedResult<()> {
        let package_dir = self.package_dir.as_ref();
        println!(
            "The project will be created in the directory: {}",
            &package_dir.to_string_lossy()
        );

        let package_name = self.ask_package_name()?;
        println!("Package name: {}", &package_name);

        let add_coin_module = self.ask_add_coin_module();
        let add_dapp = self.ask_add_dapp();
        let run_aptos_init = self.ask_run_aptos_init();

        println!(
            "Creating a package directory {}",
            package_dir.to_string_lossy().to_string().as_str()
        );
        fs::create_dir_all(package_dir)
            .map_err(|err| anyhow!("Failed to create a directory {package_dir:?}.\n{err}"))?;

        let profile_address_hex = if run_aptos_init {
            self.aptos_init_profile_default(package_dir).await?
        } else {
            "_".to_string()
        };

        // if coin module is requested, then all the necessary directories will be created with that
        if !add_coin_module {
            self.init_move_dir(package_dir, &package_name).await?;
            fs::create_dir(package_dir.join("tests"))
                .map_err(|err| CliError::UnexpectedError(err.to_string()))?;

            // fail fast if no need for any templates
            if !add_dapp {
                return Ok(());
            }
        }

        // git template
        let templates_root_path = git_download_aptos_templates()?;
        let package_lowercase_name = package_name.to_case(Case::Snake);
        let tera_context = Context::from_serialize(
            [
                ("package_name".to_string(), package_name),
                ("package_lowercase_name".to_string(), package_lowercase_name),
                ("default_address".to_string(), profile_address_hex.clone()),
                ("address".to_string(), profile_address_hex),
            ]
            .into_iter()
            .collect::<HashMap<String, String>>(),
        )
        .map_err(|err| anyhow!("Tera context: {err}"))?;

        if add_coin_module {
            let tera_coin_module = Tera::new(&format!(
                "{}/_coin/**/*",
                templates_root_path.to_string_lossy()
            ))
            .map_err(|_| CliError::UnexpectedError("tera error".to_string()))?;

            for (from, to, subpath) in
                walk_dir_for_tera(&templates_root_path.join("_coin"), package_dir)
            {
                if to.exists() {
                    continue;
                }

                if from.is_dir() {
                    fs::create_dir(to).map_err(|err| anyhow!("Create dir: {err}. {from:?}"))?;
                    continue;
                }

                let r = tera_coin_module
                    .render(&subpath, &tera_context)
                    .map_err(|err| anyhow!("Tera render: {err}. {subpath}"))?;
                fs::write(to, r).map_err(|err| anyhow!("{err}. {subpath}"))?;
            }
        }

        if add_dapp {
            let tera_coin_module = Tera::new(&format!(
                "{}/_typescript/**/*",
                templates_root_path.to_string_lossy()
            ))
            .map_err(|_| CliError::UnexpectedError("tera error".to_string()))?;

            for (from, to, subpath) in
                walk_dir_for_tera(&templates_root_path.join("_typescript"), package_dir)
            {
                if to.exists() {
                    continue;
                }

                if from.is_dir() {
                    fs::create_dir(to).map_err(|err| anyhow!("Create dir: {err}. {from:?}"))?;
                    continue;
                }

                let r = tera_coin_module
                    .render(&subpath, &tera_context)
                    .map_err(|err| anyhow!("Tera render: {err}. {subpath}"))?;
                fs::write(to, r).map_err(|err| anyhow!("{err}. {subpath}"))?;
            }
        }

        Ok(())
    }
}

impl NewPackage {
    #[inline]
    fn ask_package_name(&self) -> anyhow::Result<String> {
        let package_name = match &self.name {
            Some(name) => name.clone(),
            None => {
                let default_name = self.package_dir.to_package_name();

                println!("\nEnter the package name [default: {default_name}]: ");
                let package_name = read_line("Package name")?.trim().to_string();
                println!();

                if package_name.is_empty() {
                    default_name
                } else {
                    package_name
                }
            }
        };

        Ok(package_name)
    }

    #[inline]
    fn ask_add_coin_module(&self) -> bool {
        if let Some(add_example) = self.add_coin {
            add_example
        } else {
            ask_yes_no("Add an example coin module to the package? ", false)
        }
    }

    #[inline]
    fn ask_add_dapp(&self) -> bool {
        if let Some(selected_value) = self.add_js {
            selected_value
        } else {
            ask_yes_no("Add a sample dApp to the package? ", false)
        }
    }

    #[inline]
    fn ask_run_aptos_init(&self) -> bool {
        if let Some(value) = self.create_profile {
            value
        } else {
            ask_yes_no("Configure Aptos account? ", false)
        }
    }

    /// $ aptos move init
    #[inline]
    async fn init_move_dir(&self, package_dir: &Path, package_name: &str) -> anyhow::Result<()> {
        self.framework_package_args.init_move_dir(
            package_dir,
            package_name,
            BTreeMap::default(),
            PromptOptions::default(),
        )?;
        Ok(())
    }

    /// $ aptos init --profile default
    #[inline]
    async fn aptos_init_profile_default(&self, package_dir: &Path) -> anyhow::Result<String> {
        println!(
            "Creating a {profile_name} profile for a package",
            profile_name = "`default`"
        );
        std::env::set_current_dir(&package_dir).map_err(|err| anyhow!("{err}"))?;
        crate::common::init::InitTool {
            network: Some(Network::Devnet),
            rest_url: None,
            faucet_url: None,
            skip_faucet: false,
            rng_args: RngArgs::default(),
            private_key_options: PrivateKeyInputOptions::default(),
            profile_options: ProfileOptions::default(),
            prompt_options: PromptOptions::yes(),
            encoding_options: EncodingOptions::default(),
        }
        .execute()
        .await?;

        let address_hex = CliConfig::load_profile(Some("default"), ConfigSearchMode::CurrentDir)?
            .ok_or_else(|| anyhow!("The config file could not be found .aptos/config.yaml"))?
            .account
            .ok_or_else(|| anyhow!("the address is not specified in the profile `default`"))?
            .to_hex_literal();
        Ok(address_hex)
    }
}

// ===

#[derive(Clone)]
pub struct PackageDir(PathBuf);

impl PackageDir {
    fn to_package_name(&self) -> String {
        self.0
            .file_name()
            .map(|name| name.to_string_lossy().to_case(Case::UpperCamel))
            .unwrap_or_default()
    }
}

impl AsRef<Path> for PackageDir {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl From<PackageDir> for PathBuf {
    fn from(value: PackageDir) -> PathBuf {
        value.0
    }
}

impl FromStr for PackageDir {
    type Err = anyhow::Error;

    fn from_str(path: &str) -> std::result::Result<Self, Self::Err> {
        let package_dir = PathBuf::from(path).absolutize()?.to_path_buf();

        if !package_dir.exists() {
            return Ok(PackageDir(package_dir));
        }

        let is_empty = package_dir
            .read_dir()
            .map_err(|_| anyhow!("Couldn't read the directory {package_dir:?}"))?
            .filter_map(|item| item.ok())
            .next()
            .is_none();
        ensure!(is_empty, "The directory is not empty {package_dir:?}");

        Ok(PackageDir(package_dir))
    }
}

// ===

fn walk_dir_for_tera<'a>(
    from_dir: &'a Path,
    to_dir: &'a Path,
) -> impl Iterator<Item = (PathBuf, PathBuf, String)> + 'a {
    let from_str = from_dir.to_string_lossy().to_string();

    WalkDir::new(from_dir)
        .into_iter()
        .filter_map(|path| path.ok())
        .map(|path| path.into_path())
        .skip(1)
        .map(move |path| {
            let sub = path
                .to_string_lossy()
                .to_string()
                .trim_start_matches(&from_str)
                .trim_matches('/')
                .to_string();
            (path, to_dir.join(&sub), sub)
        })
}

fn ask_yes_no(text: &str, default: bool) -> bool {
    println!("{text}[{}]", if default { "Y/n" } else { "y/N" });
    let result = loop {
        let insert_text = match read_line("yes_no") {
            Ok(result) => result,
            Err(err) => {
                println!("{err}");
                continue;
            }
        };
        match insert_text.to_lowercase().trim() {
            "" => break default,
            "y" | "yes" => break true,
            "n" | "no" => break false,
            _ => {
                print!("Please enter 'y' or 'n'")
            }
        }
    };
    println!("{}", if result { "yes" } else { "no" });
    result
}

fn git_download_aptos_templates() -> anyhow::Result<PathBuf> {
    let tmp_dir = std::env::temp_dir().join("aptos_templates");
    if !tmp_dir.exists() {
        println!("Download: {GIT_TEMPLATE}");
        git2::Repository::clone(GIT_TEMPLATE, &tmp_dir)?;
    }

    Ok(tmp_dir)
}
