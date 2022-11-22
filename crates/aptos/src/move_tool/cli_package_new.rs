use std::collections::{BTreeMap, HashMap};
use std::fmt::Formatter;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::str::FromStr;

use anyhow::{anyhow, bail, ensure, Result};
use async_trait::async_trait;
use clap::Parser;
use convert_case::{Case, Casing};
use itertools::Itertools;
use path_absolutize::Absolutize;
use termcolor::{BufferWriter, Color, ColorChoice, ColorSpec, WriteColor};
use walkdir::WalkDir;

use crate::common::init::Network;

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
/// $ aptos new /tmp/my_package --name ExampleProject --template-type 2 --assume-yes --skip-profile-creation
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
    /// 2. Empty package template: https://github.com/mkurnikov/aptos-templates.git
    /// 3. Example with coins: @todo
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
        let package_lowercase_name = package_name.to_case(Case::Snake);

        let template_type = self.template_type(package_dir, &package_lowercase_name)?;
        println!("Template type: {}", fg_cyan(&template_type.to_string()));

        self.ask_create_package(&template_type, package_dir, &package_lowercase_name);

        if matches!(template_type, TemplateType::ExampleWithCoin) {
            todo!("download from github")
        }

        fs::create_dir_all(package_dir)
            .map_err(|err| anyhow!("Failed to create a directory {package_dir:?}.\n{err}"))?;

        println!(
            "Creating a {profile_name} profile for a package",
            profile_name = fg_cyan("`default`")
        );

        let profile_address_hex = if !self.skip_profile_creation {
            self.aptos_init_profile_default(package_dir).await?
        } else {
            "_".to_string()
        };

        if matches!(template_type, TemplateType::OnlyMoveToml) {
            return self.aptos_move_init(&package_name, package_dir).await;
        }

        template_type.git_download()?;
        template_type.copy_template_to(&package_dir, self.ask_type_language())?;
        TemplateType::replace_values_in_the_template(
            package_dir,
            &[
                ("package_name", &package_name),
                ("package_lowercase_name", &package_lowercase_name),
                ("default_address", &profile_address_hex),
            ],
        )?;

        Ok(())
    }
}

impl NewPackage {
    #[inline]
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

    #[inline]
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

    #[inline]
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
        if template_type.url().is_some() {
            println!("{} The template will be downloaded", fg_warning("*"));
        }
        println!();
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

    #[inline]
    fn ask_type_language(&self) -> TemplateTypeVariants {
        if self.prompt_options.assume_yes {
            TemplateTypeVariants::MoveTs
        } else {
            println!(
                "Want to add a `typescript aptos template`?\n\
                Enter yes or no [Default: yes]:"
            );

            loop {
                let result = match read_line("typescript") {
                    Ok(s) => s,
                    Err(err) => {
                        println!("{err}");
                        continue;
                    }
                };
                match result.to_lowercase().trim() {
                    "" | "y" | "yes" => break TemplateTypeVariants::MoveTs,
                    "n" | "no" => break TemplateTypeVariants::Move,
                    _ => {
                        println!(
                            "Please enter {yes} or {no}",
                            yes = fg_bold("yes"),
                            no = fg_bold("no")
                        )
                    }
                }
            }
        }
    }

    /// $ aptos move init
    #[inline]
    async fn aptos_move_init(
        &self,
        package_name: &String,
        package_dir: &Path,
    ) -> Result<(), CliError> {
        InitPackage {
            name: package_name.clone(),
            package_dir: Some(package_dir.to_path_buf()),
            named_addresses: self.named_addresses.clone(),
            prompt_options: self.prompt_options,
            framework_package_args: self.framework_package_args.clone(),
        }
        .execute()
        .await
    }

    /// $ aptos init
    #[inline]
    async fn aptos_init_profile_default(&self, package_dir: &Path) -> Result<String, CliError> {
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
    OnlyMoveToml = 1,
    EmptyPackage = 2,
    ExampleWithCoin = 3,
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
                    └─ {move_toml}\n\
                    \n\
                    GitHub: {url}",
                    sources = fg_italic("sources"),
                    source_file = fg_italic(&format!("{package_name}.move")),
                    tests = fg_italic("tests"),
                    test_file = fg_italic(&format!("{package_name}_tests.move")),
                    url = TemplateType::EmptyPackage.url().unwrap_or_default()
                )
            }
            TemplateType::ExampleWithCoin => format!(
                "@todo\n\
                    \n\
                GitHub: @todo"
            ),
        }
    }

    fn git_template_path(&self) -> PathBuf {
        std::env::temp_dir().join(format!("aptos_template_{}", *self as u8))
    }

    fn git_download(&self) -> anyhow::Result<PathBuf> {
        let url = self.url().unwrap_or_else(|| unreachable!());

        let tmp_dir = self.git_template_path();
        if !tmp_dir.exists() {
            println!("Download: {url}");
            git2::Repository::clone(&url, &tmp_dir)?;
        }

        Ok(tmp_dir)
    }

    fn copy_template_to(&self, to: &Path, ctp: TemplateTypeVariants) -> Result<()> {
        let template_path = self.git_template_path();
        let mut from = match ctp {
            TemplateTypeVariants::Move => template_path.join("_default"),
            TemplateTypeVariants::MoveTs => template_path.join("_typescript"),
        };

        if !from.exists() {
            from = template_path;
        }

        cp_r(&from, to)
    }

    fn replace_values_in_the_template(
        package_dir: &Path,
        values: &[(&str, &String)],
    ) -> Result<()> {
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

    fn url(&self) -> Option<String> {
        match self {
            TemplateType::OnlyMoveToml => None,
            TemplateType::EmptyPackage => {
                Some("https://github.com/mkurnikov/aptos-templates.git".to_string())
            }
            TemplateType::ExampleWithCoin => todo!(),
        }
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

enum TemplateTypeVariants {
    Move,
    MoveTs,
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
            "{num} {name}\n{structure}\n",
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
                .trim_start_matches("/"),
        );

        if copy_from.is_file() {
            fs::copy(copy_from, copy_to)?;
        } else if copy_from.is_dir() {
            fs::create_dir(copy_to)?;
        }
    }
    println!("\rCopying completed{}", " ".repeat(30));
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
