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
    #[clap(long, display_order = 2)]
    pub(crate) add_coin: Option<bool>,

    /// Do not create a "default" profile
    #[clap(long, display_order = 4)]
    pub(crate) skip_profile_creation: bool,

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

        // if coin module is requested, then all the necessary directories will be created with that
        if !add_coin_module {
            self.init_move_dir(package_dir, &package_name).await?;
            fs::create_dir(package_dir.join("tests"))
                .map_err(|err| CliError::UnexpectedError(err.to_string()))?;
        }

        if run_aptos_init {
            // TODO: run aptos init
        }

        // fail fast if no need for any templates
        if !add_coin_module && !add_dapp {
            return Ok(());
        }

        let templates_root_path = git_download_aptos_templates()?;
        let tera_coin_module = Tera::new(&format!(
            "{}/_coin/**/*",
            templates_root_path.to_string_lossy()
        ))
        .map_err(|_| CliError::UnexpectedError("tera error".to_string()))?;

        // TODO: use Tera with context to render _coin/ directory, it should be rendered on top of empty directory,
        // as
        //         if !add_coin_module {
        //             self.init_move_dir(package_dir, &package_name).await?;
        //             fs::create_dir(package_dir.join("tests"))
        //                 .map_err(|err| CliError::UnexpectedError(err.to_string()))?;
        //         }
        // now runs only without _coin/ added

        // let mut context = Context::new();
        // context.insert("package_name", &package_name);

        // TODO: try to use camel_case_to_lower_case filter in Tera context, instead of pre-defining variable

        // TODO: remove GitTemplate struct, no need for the deep structures. Do everything here, we will refactor later.

        // TODO: if add_js { add_js_app }, doesn't matter if 
        // let GitTemplate {
        //     package_name,
        //     profile_address_hex,
        //     add_coin_module: coin,
        //     add_dapp: script,
        //     package_dir,
        // } = self;
        // let package_lowercase_name = package_name.to_case(Case::Snake);

        // copy_path_recursive(
        //     &templates_root_path.join("_default/sources/"),
        //     &package_dir.join("sources"),
        // )?;
        // copy_path_recursive(
        //     &templates_root_path.join("_default/tests/"),
        //     &package_dir.join("tests"),
        // )?;

        // let move_toml_path = package_dir.join("Move.toml");
        // let mut move_toml = fs::read_to_string(&move_toml_path).map_err(|err| anyhow!("{err}"))?;
        // if !move_toml.contains("aptos-move/framework/aptos-framework") {
        //     move_toml += "\n\n[dependencies.AptosFramework]
        //         git = \"https://github.com/aptos-labs/aptos-core.git\"
        //         rev = \"main\"
        //         subdir = \"aptos-move/framework/aptos-framework\"\n";
        // }

        // if *coin {
        //     copy_path_recursive(
        //         &templates_root_path.join("_coin/sources/"),
        //         &package_dir.join("sources"),
        //     )?;
        //     copy_path_recursive(
        //         &templates_root_path.join("_coin/tests/"),
        //         &package_dir.join("tests"),
        //     )?;
        // }

        // if *script {
        //     let js_path = &package_dir.join("js");
        //     fs::create_dir(js_path).map_err(|err| anyhow!("{err}"))?;
        //     copy_path_recursive(&templates_root_path.join("_typescript/js/"), js_path)?;
        // }

        // fs::write(move_toml_path, move_toml).map_err(|err| anyhow!("{err}"))?;

        // replace_values_in_the_template(
        //     package_dir,
        //     &[
        //         ("package_name", package_name),
        //         ("package_lowercase_name", &package_lowercase_name),
        //         ("default_address", profile_address_hex),
        //     ],
        // )?;
        Ok(())

        // // Examples from the template
        // GitTemplate {
        //     package_name,
        //     profile_address_hex,
        //     add_coin_module,
        //     add_dapp,
        //     package_dir,
        // }
        // .copy_from_git_template()?;
        //
        // Ok(())
    }
}

impl NewPackage {
    #[inline]
    fn ask_package_name(&self) -> anyhow::Result<String> {
        let package_name = match &self.name {
            Some(name) => name.clone(),
            None => {
                let default_name = self.package_dir.to_package_name();

                print!("\nEnter the package name [default: {default_name}]: ");
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
        ask_yes_no("Configure Aptos account? ", false)
    }

    // #[inline]
    // async fn empty_package(
    //     &self,
    //     package_dir: &Path,
    //     package_name: &str,
    // ) -> anyhow::Result<()> {
    //     println!(
    //         "Creating a package directory {}",
    //         package_dir.to_string_lossy().to_string().as_str()
    //     );
    //     fs::create_dir_all(package_dir)
    //         .map_err(|err| anyhow!("Failed to create a directory {package_dir:?}.\n{err}"))?;
    //     // creates default Move.toml, sources/
    //     self.init_move_dir(package_dir, package_name)?;
    //     fs::create_dir(package_dir.join("tests"))?;
    //     Ok(())
    // }

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
        if self.skip_profile_creation {
            return Ok("_".to_string());
        }

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

struct GitTemplate<'a> {
    package_name: String,
    profile_address_hex: String,
    add_coin_module: bool,
    add_dapp: bool,
    package_dir: &'a Path,
}

impl GitTemplate<'_> {
    fn copy_from_git_template(&self) -> anyhow::Result<()> {
        let template_path = git_download_aptos_templates()?;

        let GitTemplate {
            package_name,
            profile_address_hex,
            add_coin_module: coin,
            add_dapp: script,
            package_dir,
        } = self;
        let package_lowercase_name = package_name.to_case(Case::Snake);

        copy_path_recursive(
            &template_path.join("_default/sources/"),
            &package_dir.join("sources"),
        )?;
        copy_path_recursive(
            &template_path.join("_default/tests/"),
            &package_dir.join("tests"),
        )?;

        let move_toml_path = package_dir.join("Move.toml");
        let mut move_toml = fs::read_to_string(&move_toml_path).map_err(|err| anyhow!("{err}"))?;
        if !move_toml.contains("aptos-move/framework/aptos-framework") {
            move_toml += "\n\n[dependencies.AptosFramework]
                git = \"https://github.com/aptos-labs/aptos-core.git\"
                rev = \"main\"
                subdir = \"aptos-move/framework/aptos-framework\"\n";
        }

        if *coin {
            copy_path_recursive(
                &template_path.join("_coin/sources/"),
                &package_dir.join("sources"),
            )?;
            copy_path_recursive(
                &template_path.join("_coin/tests/"),
                &package_dir.join("tests"),
            )?;

            if let Some(pos) = move_toml.find("[addresses]") {
                move_toml.insert_str(
                    pos + 11,
                    &format!("\ncoin_address = \"{profile_address_hex}\"\n"),
                );
            } else {
                move_toml += &format!(
                    "
            
                    [addresses]
                    coin_address = \"{profile_address_hex}\""
                );
            }
        }

        if *script {
            let js_path = &package_dir.join("js");
            fs::create_dir(js_path).map_err(|err| anyhow!("{err}"))?;
            copy_path_recursive(&template_path.join("_typescript/js/"), js_path)?;
        }

        fs::write(move_toml_path, move_toml).map_err(|err| anyhow!("{err}"))?;

        replace_values_in_the_template(
            package_dir,
            &[
                ("package_name", package_name),
                ("package_lowercase_name", &package_lowercase_name),
                ("default_address", profile_address_hex),
            ],
        )?;
        Ok(())
    }
}
// ===

fn ask_yes_no(text: &str, default: bool) -> bool {
    print!("{text}[{}]", if default { "Y/n" } else { "y/N" });
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

fn replace_values_in_the_template(
    package_dir: &Path,
    values: &[(&str, &String)],
) -> anyhow::Result<()> {
    let hash_map: HashMap<&str, &String> = values.iter().cloned().collect();
    for path in WalkDir::new(package_dir)
        .into_iter()
        .filter_map(|path| path.ok())
        .map(|path| path.into_path())
        .skip(1)
    {
        path_processing_30("Processing: ", &path, "");
        if path.is_file() {
            let content = fs::read_to_string(&path)?;
            let new_content = str_replace_position(&content, &hash_map);
            if content != new_content {
                fs::write(&path, new_content)?;
            }
        }

        let from_path = path.to_string_lossy().to_string();
        let to_path = str_replace_position(&from_path, &hash_map);
        if from_path != to_path {
            fs::rename(&from_path, to_path)?;
        }
    }
    println!("\rThe data is inserted into the template{}", " ".repeat(30));

    Ok(())
}

fn copy_path_recursive(from: &Path, to: &Path) -> anyhow::Result<()> {
    for copy_from in WalkDir::new(from)
        .into_iter()
        .filter_map(|path| path.ok())
        .map(|path| path.into_path())
        .skip(1)
    {
        path_processing_30("Copying: ", &copy_from, "");
        let copy_to = to.join(
            copy_from
                .to_string_lossy()
                .trim_start_matches(&from.to_string_lossy().to_string())
                .trim_start_matches('/'),
        );

        if copy_from.is_file() {
            fs::copy(copy_from, copy_to)?;
        } else if copy_from.is_dir() {
            fs::create_dir(copy_to)?;
        }
    }
    print!("\r");
    Ok(())
}

fn str_to_insert_position(text: &str) -> Vec<(&str, &str)> {
    let mut cur = 0;
    let mut result = Vec::new();

    let mut position;
    let mut position_index;

    while let Some(mut start_pos) = text[cur..].find("{{") {
        start_pos += cur;
        cur = start_pos;

        let end_pos = match text[start_pos..].find("}}") {
            None => continue,
            Some(pos) => start_pos + pos + 2,
        };
        cur = end_pos;

        position = &text[start_pos..end_pos];
        position_index = position.trim().trim_matches('{').trim_matches('}').trim();
        result.push((position_index, position));
    }

    result
}

fn str_replace_position(text: &str, key_value: &HashMap<&str, &String>) -> String {
    let mut result = text.to_string();
    for (key, position_str) in str_to_insert_position(text) {
        if let Some(value) = key_value.get(key) {
            result = result.replace(position_str, value);
        }
    }
    result
}

fn path_processing_30(pref: &str, path: &Path, suff: &str) {
    let path_str = path.to_string_lossy();
    let path_print = if path_str.len() > 30 {
        format!("..{}", &path_str[path_str.len() - 28..])
    } else {
        path_str.to_string()
    };
    print!("\r{}{:<30}{}", pref, path_print, suff);
}

// ===

fn fg_bold(text: &str) -> String {
    style_text(text, ColorSpec::new().set_bold(true).to_owned())
        .unwrap_or_else(|_| text.to_string())
}

fn fg_cyan(text: &str) -> String {
    style_text(text, ColorSpec::new().set_fg(Some(Color::Cyan)).to_owned())
        .unwrap_or_else(|_| text.to_string())
}

fn style_text(text: &str, color: ColorSpec) -> anyhow::Result<String> {
    let buffer_writer = BufferWriter::stderr(ColorChoice::Always);
    let mut buffer = buffer_writer.buffer();
    buffer.set_color(&color)?;
    write!(&mut buffer, "{text}")?;
    buffer.reset()?;
    Ok(String::from_utf8(buffer.into_inner())?)
}

#[cfg(test)]
mod test {
    use crate::move_tool::cli_package_new::str_to_insert_position;

    #[test]
    fn test_str_to_insert_position() {
        assert_eq!(
            vec![
                ("123", "{{123}}"),
                ("456", "{{ 456 }}"),
                ("789", "{{{789}}"),
            ],
            str_to_insert_position("{{123}}{{ 456 }}{{{789}}}")
        );
    }
}
