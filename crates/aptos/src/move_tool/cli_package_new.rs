use std::collections::BTreeMap;
use std::fmt::Formatter;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::str::FromStr;

use crate::common::init::Network;
use anyhow::{anyhow, bail, ensure, Result};
use aptos_logger::log;
use async_trait::async_trait;
use clap::Parser;
use convert_case::{Case, Casing};
use itertools::Itertools;
use path_absolutize::Absolutize;
use termcolor::{BufferWriter, Color, ColorChoice, ColorSpec, WriteColor};

use crate::common::types::{
    CliCommand, CliConfig, CliError, CliTypedResult, ConfigSearchMode, EncodingOptions,
    MoveManifestAccountWrapper, PrivateKeyInputOptions, ProfileOptions, PromptOptions, RngArgs,
};
use crate::common::utils::read_line;
use crate::move_tool::{FrameworkPackageArgs, InitPackage};

/// Creates a new "Move" package at the given location.
///
/// Examples:
/// $ aptos new my_package
/// $ aptos new ~/demo/my_package2 --named_addresses self=_,std=0x1
/// $ aptos new /tmp/my_package3 --name DemoPackage --assume-yes
/// $ new /tmp/my_package --name ExampleProject --template-type 2 --assume-yes --skip-profile-creation
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

    /// Creation Template:
    /// 1. Only `Move.toml`
    /// 2. Empty package template
    /// 3. Example with coins
    #[clap(long, display_order = 2, verbatim_doc_comment)]
    pub(crate) template_type: Option<TemplateType>,

    /// Do not create a "default" profile
    #[clap(long, display_order = 3)]
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
            fg_cyan(&package_dir.to_string_lossy().to_string())
        );

        let package_name = self.package_name()?;
        println!("Package name: {}", fg_cyan(&package_name));

        let template_type = self.template_type(package_dir, &package_name)?;
        println!("Template type: {}", fg_cyan(&template_type.to_string()));

        self.ask_create_package(&template_type, package_dir, &package_name);

        if matches!(template_type, TemplateType::ExampleWithCoin) {
            todo!("download from github")
        }

        fs::create_dir_all(&package_dir)
            .map_err(|err| anyhow!("Failed to create a directory {package_dir:?}.\n{err}"))?;

        InitPackage {
            name: package_name.clone(),
            package_dir: Some(package_dir.to_path_buf()),
            named_addresses: self.named_addresses,
            prompt_options: self.prompt_options,
            framework_package_args: self.framework_package_args,
        }
        .execute()
        .await?;

        println!(
            "Creating a {profile_name} profile for a package",
            profile_name = fg_cyan("`default`")
        );

        let profile_address_hex = if !self.skip_profile_creation {
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

            CliConfig::load_profile(Some("default"), ConfigSearchMode::CurrentDir)?
                .ok_or_else(|| anyhow!("The config file could not be found .aptos/config.yaml"))?
                .account
                .ok_or_else(|| anyhow!("the address is not specified in the profile `default`"))?
                .to_hex_literal()
        } else {
            "_".to_string()
        };

        if matches!(template_type, TemplateType::OnlyMoveToml) {
            return Ok(());
        }

        TemplateType::creating_file_structure_for_empty_package(package_dir, &package_name)?;

        let move_toml_path = package_dir.join("Move.toml");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .append(true)
            .open(&move_toml_path)
            .map_err(|err| anyhow!("{move_toml_path:?} {err}"))?;
        writeln!(
            file,
            "\n\
                [addresses]\n\
                self = {profile_address_hex:?}"
        )
        .map_err(|err| anyhow!("{move_toml_path:?} {err}"))?;

        Ok(())
    }
}

impl NewPackage {
    fn package_name(&self) -> Result<String> {
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

    fn template_type(&self, package_dir: &Path, package_name: &str) -> Result<TemplateType> {
        if let Some(tp) = self.template_type {
            return Ok(tp);
        }

        print_description_of_templates(package_dir, package_name);

        println!("\nEnter the number 1-3 [Default: 1]:");
        let tp = loop {
            let number = read_line("teplate_type")?.trim().to_string();
            if number.is_empty() {
                break TemplateType::OnlyMoveToml;
            }

            match TemplateType::from_str(number.trim()) {
                Ok(tp) => break tp,
                Err(err) => {
                    println!("{}", fg_warning(&format!("{err}")));
                }
            }
        };
        println!();
        Ok(tp)
    }

    fn ask_create_package(
        &self,
        template_type: &TemplateType,
        root_path: &Path,
        package_name: &str,
    ) {
        if self.prompt_options.assume_yes {
            return;
        }

        println!("\nCreate these files and directories on your computer?");
        println!(
            "{}",
            template_type.structure_for_print(root_path, package_name)
        );
        println!("To create, please press {}", fg_bold("Enter"));
        println!("To cancel, please press {}", fg_bold("CTRL+C"));

        match read_line("Create").unwrap().trim().to_lowercase().as_str() {
            "n" | "no" => {
                println!("{}", fg_warning("Cancelled"));
                exit(1);
            }
            _ => {}
        }
        println!();
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

impl Into<PathBuf> for PackageDir {
    fn into(self) -> PathBuf {
        self.0
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

#[derive(Copy, Clone)]
pub enum TemplateType {
    OnlyMoveToml,
    EmptyPackage,
    ExampleWithCoin,
}

impl TemplateType {
    fn structure_for_print(&self, package_dir: &Path, package_name: &str) -> String {
        let package_dir_italic = fg_italic(&package_dir.to_string_lossy().to_string());
        let move_toml = fg_italic("Move.toml");

        match self {
            TemplateType::OnlyMoveToml => {
                format!(
                    "📂 {package_dir_italic}\n\
                    └─ {move_toml}"
                )
            }
            TemplateType::EmptyPackage => {
                format!(
                    "📂 {package_dir_italic}\n\
                    ├─📂 {sources}\n\
                    │ └─ {source_file}\n\
                    ├─📂 {tests}\n\
                    │ └─ {test_file}\n\
                    └─ {move_toml}",
                    sources = fg_italic("sources"),
                    source_file = fg_italic(&format!("{package_name}.move")),
                    tests = fg_italic("tests"),
                    test_file = fg_italic(&format!("{package_name}_tests.move"))
                )
            }
            TemplateType::ExampleWithCoin => format!("@todo"),
        }
    }

    fn creating_file_structure_for_empty_package(
        package_dir: &Path,
        package_name: &str,
    ) -> Result<()> {
        let move_module_path = package_dir.join(format!("sources/{package_name}.move"));
        fs::write(
            &move_module_path,
            format!("module self::{package_name} {{}}"),
        )
        .map_err(|err| {
            anyhow!("Failed to create a file with the module {move_module_path:?}.\n{err:?}")
        })?;

        let tests_dir = package_dir.join("tests");
        let move_module_path = tests_dir.join(format!("{package_name}_test.move"));
        fs::create_dir(&tests_dir).map_err(|err| {
            anyhow!("Failed to create a directory for tests {tests_dir:?}.\n{err:?}")
        })?;

        fs::write(
            &move_module_path,
            format!("#[test_only]\n\nmodule self::{package_name}_test {{}}"),
        )
        .map_err(|err| {
            anyhow!("Failed to create a file with test {move_module_path:?}.\n{err:?}")
        })?;

        Ok(())
    }
}

impl FromStr for TemplateType {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let template = match value {
            "1" => TemplateType::OnlyMoveToml,
            "2" => TemplateType::EmptyPackage,
            "3" => TemplateType::ExampleWithCoin,
            _ => bail!("Please select number 1-3"),
        };
        Ok(template)
    }
}

impl std::fmt::Display for TemplateType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let as_string = match self {
            TemplateType::OnlyMoveToml => "Only `Move.toml`",
            TemplateType::EmptyPackage => "Empty package template",
            TemplateType::ExampleWithCoin => "Example with coins",
        }
        .to_string();
        write!(f, "{as_string}")
    }
}

// ===

#[inline]
fn print_description_of_templates(package_dir: &Path, package_name: &str) {
    println!("What type of template do you want to preset?");

    for (num, tp) in [
        TemplateType::OnlyMoveToml,
        TemplateType::EmptyPackage,
        TemplateType::ExampleWithCoin,
    ]
    .iter()
    .enumerate()
    {
        println!(
            "{num} {name}\n{structure}",
            num = fg_cyan(&format!("{}.", num + 1)),
            name = fg_bold(&tp.to_string()),
            structure = tp
                .structure_for_print(package_dir, package_name)
                .lines()
                .map(|val| format!("   {val}"))
                .join("\n"),
        );
    }
}

// ===

fn fg_bold(text: &str) -> String {
    style_text(text, ColorSpec::new().set_bold(true).to_owned())
        .unwrap_or_else(|_| text.to_string())
}

fn fg_italic(text: &str) -> String {
    style_text(text, ColorSpec::new().set_italic(true).to_owned())
        .unwrap_or_else(|_| text.to_string())
}

fn fg_cyan(text: &str) -> String {
    style_text(text, ColorSpec::new().set_fg(Some(Color::Cyan)).to_owned())
        .unwrap_or_else(|_| text.to_string())
}

fn fg_warning(text: &str) -> String {
    style_text(
        text,
        ColorSpec::new().set_fg(Some(Color::Yellow)).to_owned(),
    )
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
