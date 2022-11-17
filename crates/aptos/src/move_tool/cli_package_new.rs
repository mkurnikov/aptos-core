use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use clap::Parser;
use convert_case::Casing;

use crate::common::types::{
    CliCommand, CliError, CliTypedResult, MoveManifestAccountWrapper, PromptOptions,
};
use crate::common::utils::read_line;
use crate::move_tool::{FrameworkPackageArgs, InitPackage};

/// @todo
/// Creates a new Move package at the given location
///
/// This will create a directory for a Move package and a corresponding
/// <PACKAGE_NAME>
/// ─┐
///  ├📂 `tests` folder
///  │ └─ `tests/<PACKAGE_NAME>_tests.move`
///  ├📂 `sources` folder
///  │ └─ `sources/<PACKAGE_NAME>.move`
///  └─  `Move.toml` file.
///
/// Examples:
/// $ aptos new ~/my_package
/// $ aptos new ~/my_package --named_addresses self=_,std=0x1
/// $ aptos new ~/my_package --name DemoPackage --assume-yes
#[derive(Parser)]
#[clap(verbatim_doc_comment)]
pub struct NewPackage {
    /// Directory to create the new Move package
    /// The folder name can be used as the package name.
    /// If the directory does not exist, it will be created
    ///
    /// Example:
    /// ~/path/to/my_new_package
    /// ./MyNewPackage
    #[clap(verbatim_doc_comment, value_parser)]
    pub(crate) package_dir: PathBuf,

    /// Name of the new Move package
    #[clap(long, display_order = 1)]
    pub(crate) name: Option<String>,

    /// Named addresses for the move binary.
    /// Allows for an address to be put into the Move.toml, or a placeholder `_`
    ///
    /// Example: alice=0x1234,bob=0x5678,greg=_
    ///
    /// Note: This will fail if there are duplicates in the Move.toml file remove those first.
    #[clap(verbatim_doc_comment, long, parse(try_from_str = crate::common::utils::parse_map), default_value = "", display_order=2)]
    pub(crate) named_addresses: BTreeMap<String, MoveManifestAccountWrapper>,

    #[clap(flatten)]
    pub(crate) prompt_options: PromptOptions,

    #[clap(flatten)]
    pub(crate) framework_package_args: FrameworkPackageArgs,
}

#[async_trait]
impl CliCommand<()> for NewPackage {
    fn command_name(&self) -> &'static str {
        "NewPackage"
    }

    async fn execute(self) -> CliTypedResult<()> {
        let package_dir = &self.package_dir;
        if package_dir.exists() {
            let is_empty = package_dir
                .read_dir()
                .map_err(|_| {
                    CliError::CommandArgumentError(format!(
                        "Couldn't read the catalog {package_dir:?}"
                    ))
                })?
                .filter_map(|item| item.ok())
                .next()
                .is_none();
            if !is_empty {
                return Err(CliError::CommandArgumentError(format!(
                    "The directory is not empty {package_dir:?}"
                )));
            }
        }

        let package_name = match self.name {
            Some(name) => name.clone(),
            None => {
                let default = package_dir
                    .file_name()
                    .map(|name| {
                        name.to_string_lossy()
                            .to_case(convert_case::Case::UpperCamel)
                    })
                    .unwrap_or_default();

                if self.prompt_options.assume_yes {
                    default
                } else {
                    eprintln!("Enter the package name [defaults to {default:?}]:");
                    let package_name = read_line("Package name")?.trim().to_string();

                    if package_name.is_empty() {
                        default
                    } else {
                        package_name
                    }
                }
            }
        };

        if !package_dir.exists() {
            if !self.prompt_options.assume_yes {
                loop {
                    println!(
                        r#"Create a package at {:?} [Expected: yes | no ]"#,
                        if package_dir.is_absolute() || package_dir.starts_with(".") {
                            package_dir.to_string_lossy().to_string()
                        } else {
                            format!("./{}", package_dir.to_string_lossy())
                        }
                    );
                    match read_line("Create")?.trim().to_lowercase().as_str() {
                        "yes" | "y" => break,
                        "no" | "n" => {
                            return Err(CliError::CommandArgumentError(
                                "Canceling package creation".to_string(),
                            ))
                        }
                        _ => println!("Incorrect input. Please try again"),
                    }
                }
            }
            fs::create_dir_all(package_dir).map_err(|err| {
                CliError::CommandArgumentError(format!(
                    "Failed to create a directory {package_dir:?}.\n{err:?}"
                ))
            })?;
        }

        InitPackage {
            name: package_name.clone(),
            package_dir: Some(self.package_dir.clone()),
            named_addresses: self.named_addresses,
            prompt_options: self.prompt_options,
            framework_package_args: self.framework_package_args,
        }
        .execute()
        .await?;

        let move_module_path = package_dir.join(format!("sources/{package_name}.move"));
        fs::write(&move_module_path, format!("module _::{package_name} {{}}")).map_err(|err| {
            CliError::UnexpectedError(format!(
                "Failed to create a file with the module {move_module_path:?}.\n{err:?}"
            ))
        })?;

        let tests_dir = package_dir.join("tests");
        let move_module_path = tests_dir.join(format!("{package_name}_test.move"));
        fs::create_dir(&tests_dir).map_err(|err| {
            CliError::UnexpectedError(format!(
                "Failed to create a directory for tests {tests_dir:?}.\n{err:?}"
            ))
        })?;
        fs::write(
            &move_module_path,
            format!("#[test_only]\n\nmodule _::{package_name}_test {{}}"),
        )
        .map_err(|err| {
            CliError::UnexpectedError(format!(
                "Failed to create a file with test {move_module_path:?}.\n{err:?}"
            ))
        })?;

        Ok(())
    }
}
