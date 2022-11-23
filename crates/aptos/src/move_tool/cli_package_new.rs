use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, ensure};
use async_trait::async_trait;
use clap::Parser;
use convert_case::{Case, Casing};
use path_absolutize::Absolutize;
use termcolor::{BufferWriter, Color, ColorChoice, ColorSpec, WriteColor};
use tokio::try_join;
use walkdir::WalkDir;

use crate::common::init::Network;

use crate::common::types::{
    CliCommand, CliConfig, CliTypedResult, ConfigSearchMode, EncodingOptions,
    MoveManifestAccountWrapper, PrivateKeyInputOptions, ProfileOptions, PromptOptions, RngArgs,
};
use crate::common::utils::read_line;
use crate::move_tool::{FrameworkPackageArgs, InitPackage};

const GIT_TEMPLATE: &str = "https://github.com/mkurnikov/aptos-templates.git";

/// Creates a new "Move" package at the given location.
///
/// Examples:
/// $ aptos new my_package
/// $ aptos new ~/demo/my_package2 --named_addresses self=_,std=0x1
/// $ aptos new /tmp/my_package3 --name DemoPackage --assume-yes
/// $ aptos new /tmp/my_package --name ExampleProject --script --coin --assume-yes --skip-profile-creation
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
    pub(crate) example_script: Option<bool>,

    /// Add an example with coins to the package
    #[clap(long, display_order = 3)]
    pub(crate) example_coin: Option<bool>,

    /// Do not create a "default" profile
    #[clap(long, display_order = 4)]
    pub(crate) skip_profile_creation: bool,

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
        let package_dir = self.package_dir.as_ref();
        println!(
            "The project will be created in the directory: {}",
            fg_cyan(&package_dir.to_string_lossy())
        );

        let package_name = self.package_name()?;
        println!("Package name: {}", fg_cyan(&package_name));

        let profile_address_hex = self.empty_package(package_dir, &package_name).await?;

        let coin = self.example_coin();
        let script = self.example_script();

        if !coin && !script {
            return Ok(());
        }

        // Examples from the template
        GitTemplate {
            package_name,
            profile_address_hex,
            coin,
            script,
            package_dir,
        }
        .copy_from_git_template()?;

        Ok(())
    }
}

impl NewPackage {
    #[inline]
    fn package_name(&self) -> anyhow::Result<String> {
        let package_name = match &self.name {
            Some(name) => name.clone(),
            None => {
                let default_name = self.package_dir.to_package_name();

                if self.prompt_options.assume_yes {
                    default_name
                } else {
                    println!("\nEnter the package name [Default: {default_name}]: ");
                    let package_name = read_line("Package name")?.trim().to_case(Case::UpperCamel);
                    println!();

                    if package_name.is_empty() {
                        default_name
                    } else {
                        package_name
                    }
                }
            }
        };

        Ok(package_name)
    }

    #[inline]
    fn example_coin(&self) -> bool {
        if let Some(add_example) = self.example_coin {
            add_example
        } else if self.prompt_options.assume_yes {
            true
        } else if self.prompt_options.assume_no {
            false
        } else {
            ask_yes_no("Add an example module coin to the package? ", false)
        }
    }

    #[inline]
    fn example_script(&self) -> bool {
        if let Some(selected_value) = self.example_script {
            selected_value
        } else if self.prompt_options.assume_yes {
            true
        } else if self.prompt_options.assume_no {
            false
        } else {
            ask_yes_no("Add an example dApp to the package? ", false)
        }
    }

    #[inline]
    async fn empty_package(
        &self,
        package_dir: &Path,
        package_name: &str,
    ) -> anyhow::Result<String> {
        println!(
            "Creating a package directory {}",
            fg_cyan(package_dir.to_string_lossy().to_string().as_str())
        );
        fs::create_dir_all(package_dir)
            .map_err(|err| anyhow!("Failed to create a directory {package_dir:?}.\n{err}"))?;
        fs::create_dir(package_dir.join("tests"))?;
        let (.., profile_address_hex) = try_join!(
            self.aptos_move_init(package_dir, package_name),
            self.aptos_init_profile_default(package_dir)
        )?;
        Ok(profile_address_hex)
    }

    /// $ aptos move init
    #[inline]
    async fn aptos_move_init(&self, package_dir: &Path, package_name: &str) -> anyhow::Result<()> {
        InitPackage {
            name: package_name.to_string(),
            package_dir: Some(package_dir.to_path_buf()),
            named_addresses: self.named_addresses.clone(),
            prompt_options: self.prompt_options,
            framework_package_args: self.framework_package_args.clone(),
        }
        .execute()
        .await?;
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
            profile_name = fg_cyan("`default`")
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
    coin: bool,
    script: bool,
    package_dir: &'a Path,
}

impl GitTemplate<'_> {
    fn copy_from_git_template(&self) -> anyhow::Result<()> {
        let template_path = git_download_template()?;

        let GitTemplate {
            package_name,
            profile_address_hex,
            coin,
            script,
            package_dir,
        } = self;
        let package_lowercase_name = package_name.to_case(Case::Snake);

        cp_r(
            &template_path.join("_default/sources/"),
            &package_dir.join("sources"),
        )?;
        cp_r(
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
            cp_r(
                &template_path.join("_coin/sources/"),
                &package_dir.join("sources"),
            )?;
            cp_r(
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
            cp_r(&template_path.join("_typescript/js/"), js_path)?;
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
        todo!()
    }
}
// ===

fn ask_yes_no(text: &str, default: bool) -> bool {
    println!("{text}[Default: {}]", if default { "yes" } else { "no" });
    let result = loop {
        let insert_text = match read_line("yes_no") {
            Ok(result) => result,
            Err(err) => {
                println!("{err}");
                continue;
            }
        };
        match insert_text.to_lowercase().trim() {
            "" | "y" | "yes" => break true,
            "n" | "no" => break false,
            _ => {
                println!(
                    "Please enter {yes} or {no}",
                    yes = fg_bold("yes"),
                    no = fg_bold("no")
                )
            }
        }
    };
    println!("{}", if result { "yes" } else { "no" });
    result
}

fn git_download_template() -> anyhow::Result<PathBuf> {
    let tmp_dir = std::env::temp_dir().join("aptos_template");
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

fn cp_r(from: &Path, to: &Path) -> anyhow::Result<()> {
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
