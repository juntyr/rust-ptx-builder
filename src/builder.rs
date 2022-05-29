use std::{
    env, fmt,
    fs::File,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
};

use crate::{
    error::{BuildErrorKind, Error, Result, ResultExt},
    executable::{Cargo, ExecutableRunner, Linker},
    source::Crate,
};

const TARGET_NAME: &str = "nvptx64-nvidia-cuda";

/// Core of the crate - PTX assembly build controller.
#[derive(Debug)]
pub struct Builder {
    source_crate: Crate,

    profile: Profile,
    colors: bool,
    crate_type: Option<CrateType>,
    message_format: MessageFormat,
    prefix: String,
}

/// Successful build output.
#[derive(Debug)]
pub struct BuildOutput<'a> {
    builder: &'a Builder,
    output_path: PathBuf,
    crate_type: CrateType,
}

/// Non-failed build status.
#[derive(Debug)]
pub enum BuildStatus<'a> {
    /// The CUDA crate building was performed without errors.
    Success(BuildOutput<'a>),

    /// The CUDA crate building is not needed. Can happend in several cases:
    /// - `build.rs` script was called by **RLS**,
    /// - `build.rs` was called **recursively** (e.g. `build.rs` call for device
    ///   crate in single-source setup)
    NotNeeded,
}

/// Debug / Release profile.
///
/// # Usage
/// ``` no_run
/// use ptx_builder::prelude::*;
/// # use ptx_builder::error::Result;
///
/// # fn main() -> Result<()> {
/// Builder::new(".")?
///     .set_profile(Profile::Debug)
///     .build()?;
/// # Ok(())
/// # }
/// ```
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum Profile {
    /// Equivalent for `cargo-build` **without** `--release` flag.
    Debug,

    /// Equivalent for `cargo-build` **with** `--release` flag.
    Release,
}

/// Message format.
///
/// # Usage
/// ``` no_run
/// use ptx_builder::prelude::*;
/// # use ptx_builder::error::Result;
///
/// # fn main() -> Result<()> {
/// Builder::new(".")?
///     .set_message_format(MessageFormat::Short)
///     .build()?;
/// # Ok(())
/// # }
/// ```
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum MessageFormat {
    /// Equivalent for `cargo-build` with `--message-format=human` flag
    /// (default).
    Human,

    /// Equivalent for `cargo-build` with `--message-format=json` flag
    Json {
        /// Whether rustc diagnostics are rendered by cargo or included into the
        /// output stream.
        render_diagnostics: bool,
        /// Whether the `rendered` field of rustc diagnostics are using the
        /// "short" rendering.
        short: bool,
        /// Whether the `rendered` field of rustc diagnostics embed ansi color
        /// codes.
        ansi: bool,
    },

    /// Equivalent for `cargo-build` with `--message-format=short` flag
    Short,
}

/// Build specified crate type.
///
/// Mandatory for mixed crates - that have both `lib.rs` and `main.rs`,
/// otherwise Cargo won't know which to build:
/// ```text
/// error: extra arguments to `rustc` can only be passed to one target, consider filtering
/// the package by passing e.g. `--lib` or `--bin NAME` to specify a single target
/// ```
///
/// # Usage
/// ``` no_run
/// use ptx_builder::prelude::*;
/// # use ptx_builder::error::Result;
///
/// # fn main() -> Result<()> {
/// Builder::new(".")?
///     .set_crate_type(CrateType::Library)
///     .build()?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub enum CrateType {
    Library,
    Binary,
}

impl Builder {
    /// Construct a builder for device crate at `path`.
    ///
    /// Can also be the same crate, for single-source mode:
    /// ``` no_run
    /// use ptx_builder::prelude::*;
    /// # use ptx_builder::error::Result;
    ///
    /// # fn main() -> Result<()> {
    /// match Builder::new(".")?.build()? {
    ///     BuildStatus::Success(output) => {
    ///         // do something with the output...
    ///     }
    ///
    ///     BuildStatus::NotNeeded => {
    ///         // ...
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        Ok(Builder {
            source_crate: Crate::analyse(path).context("Unable to analyse source crate")?,
            // TODO: choose automatically, e.g.:
            // `env::var("PROFILE").unwrap_or("release".to_string())`
            profile: Profile::Release,
            colors: true,
            crate_type: None,
            message_format: MessageFormat::Human,
            prefix: String::new(),
        })
    }

    /// Returns bool indicating whether the actual build is needed.
    ///
    /// Behavior is consistent with
    /// [`BuildStatus::NotNeeded`](enum.BuildStatus.html#variant.NotNeeded).
    #[must_use]
    pub fn is_build_needed() -> bool {
        let recursive_env = env::var("PTX_CRATE_BUILDING");

        let is_recursive_build = recursive_env.map_or(false, |recursive_env| recursive_env == "1");

        !is_recursive_build
    }

    /// Returns the name of the source crate at the construction `path`.
    #[must_use]
    pub fn get_crate_name(&self) -> &str {
        self.source_crate.get_name()
    }

    /// Disable colors for internal calls to `cargo`.
    #[must_use]
    pub fn disable_colors(mut self) -> Self {
        self.colors = false;
        self
    }

    /// Set build profile.
    #[must_use]
    pub fn set_profile(mut self, profile: Profile) -> Self {
        self.profile = profile;
        self
    }

    /// Set crate type that needs to be built.
    ///
    /// Mandatory for mixed crates - that have both `lib.rs` and `main.rs`,
    /// otherwise Cargo won't know which to build:
    /// ```text
    /// error: extra arguments to `rustc` can only be passed to one target, consider filtering
    /// the package by passing e.g. `--lib` or `--bin NAME` to specify a single target
    /// ```
    #[must_use]
    pub fn set_crate_type(mut self, crate_type: CrateType) -> Self {
        self.crate_type = Some(crate_type);
        self
    }

    /// Set the message format.
    #[must_use]
    pub fn set_message_format(mut self, message_format: MessageFormat) -> Self {
        self.message_format = message_format;
        self
    }

    /// Set the build command prefix.
    #[must_use]
    pub fn set_prefix(mut self, prefix: String) -> Self {
        self.prefix = prefix;
        self
    }

    /// Performs an actual build: runs `cargo` with proper flags and
    /// environment.
    pub fn build(&self) -> Result<BuildStatus> {
        self.build_live(|_line| (), |_line| ())
    }

    #[allow(clippy::too_many_lines)]
    /// Performs an actual build: runs `cargo` with proper flags and
    /// environment.
    pub fn build_live<O: FnMut(&str), E: FnMut(&str)>(
        &self,
        on_stdout_line: O,
        mut on_stderr_line: E,
    ) -> Result<BuildStatus> {
        if !Self::is_build_needed() {
            return Ok(BuildStatus::NotNeeded);
        }

        // Verify `ptx-linker` version.
        ExecutableRunner::new(Linker).with_args(vec!["-V"]).run()?;

        let mut cargo = ExecutableRunner::new(Cargo);
        let mut args = vec!["rustc"];

        if self.profile == Profile::Release {
            args.push("--release");
        }

        args.push("--color");
        args.push(if self.colors { "always" } else { "never" });

        let mut json_format = String::from("--message-format=json");
        args.push(match self.message_format {
            MessageFormat::Human => "--message-format=human",
            MessageFormat::Json {
                render_diagnostics,
                short,
                ansi,
            } => {
                if render_diagnostics {
                    json_format.push_str(",json-render-diagnostics");
                }

                if short {
                    json_format.push_str(",json-diagnostic-short");
                }

                if ansi {
                    json_format.push_str(",json-diagnostic-rendered-ansi");
                }

                &json_format
            }
            MessageFormat::Short => "--message-format=short",
        });

        args.push("--target");
        args.push(TARGET_NAME);

        args.push("--example");
        let example_name = format!("{}-{}", self.source_crate.get_name(), self.prefix);
        args.push(&example_name);

        let output_path = {
            self.source_crate
                .get_output_path()
                .context("Unable to create output path")?
        };

        let mut lock_file = fslock::LockFile::open(&output_path.join(".ptx-builder.lock"))
            .context("Unable to create the lockfile for the ptx-builder")?;
        lock_file
            .lock()
            .context("Unable to lock the lockfile for the ptx-builder")?;

        let mut lock_file_inner = std::fs::File::options()
            .read(true)
            .open(output_path.join(".ptx-builder.lock"))
            .context("Unable to open the lockfile for the ptx-builder")?;
        let mut prior_example_name = String::new();
        lock_file_inner
            .read_to_string(&mut prior_example_name)
            .context("Unable to read from the lockfile for the ptx-builder")?;
        std::mem::drop(lock_file_inner);

        if prior_example_name.is_empty() {
            prior_example_name.push_str(self.source_crate.get_name());
            prior_example_name.push_str("-ptx-builder");
        }

        let mut lock_file_inner = std::fs::File::options()
            .write(true)
            .truncate(true)
            .open(output_path.join(".ptx-builder.lock"))
            .context("Unable to open the lockfile for the ptx-builder")?;
        lock_file_inner
            .write_all(example_name.as_bytes())
            .context("Unable to write to the lockfile for the ptx-builder")?;
        lock_file_inner
            .flush()
            .context("Unable to close the lockfile for the ptx-builder")?;
        std::mem::drop(lock_file_inner);

        let mut reader = BufReader::new(
            std::fs::File::open(self.source_crate.get_path().join("Cargo.toml"))
                .context(BuildErrorKind::OtherError)?,
        );
        let mut old_cargo_toml = String::new();
        reader
            .read_to_string(&mut old_cargo_toml)
            .context(BuildErrorKind::OtherError)?;

        let new_cargo_toml = old_cargo_toml.replace(&prior_example_name, &example_name);
        let old_cargo_toml = old_cargo_toml.replace(
            &prior_example_name,
            &format!("{}-ptx-builder", self.source_crate.get_name()),
        );

        let mut writer = std::io::BufWriter::new(
            std::fs::File::options()
                .write(true)
                .truncate(true)
                .open(self.source_crate.get_path().join("Cargo.toml"))
                .context(BuildErrorKind::OtherError)?,
        );
        writer
            .write_all(new_cargo_toml.as_bytes())
            .context(BuildErrorKind::OtherError)?;
        writer.flush().context(BuildErrorKind::OtherError)?;
        std::mem::drop(writer);

        args.push("-v");

        args.push("--");

        args.push("--crate-type");
        let crate_type = self.source_crate.get_crate_type(self.crate_type)?;
        args.push(match crate_type {
            CrateType::Binary => "bin",
            CrateType::Library => "cdylib",
        });

        cargo
            .with_args(&args)
            .with_cwd(self.source_crate.get_path())
            .with_env("PTX_CRATE_BUILDING", "1")
            .with_env("CARGO_TARGET_DIR", output_path.clone());

        let cargo_output = cargo
            .run_live(on_stdout_line, |line| {
                if Self::output_is_not_verbose(line) {
                    on_stderr_line(line);
                }
            })
            .map_err(|error| match error.kind() {
                BuildErrorKind::CommandFailed { stderr, .. } => {
                    #[allow(clippy::manual_filter_map)]
                    let lines = stderr
                        .trim_matches('\n')
                        .split('\n')
                        .filter(|s| Self::output_is_not_verbose(*s))
                        .map(String::from)
                        .collect();

                    Error::from(BuildErrorKind::BuildFailed(lines))
                }
                _ => error,
            });

        let mut writer = std::io::BufWriter::new(
            std::fs::File::options()
                .write(true)
                .truncate(true)
                .open(self.source_crate.get_path().join("Cargo.toml"))
                .context(BuildErrorKind::OtherError)?,
        );
        writer
            .write_all(old_cargo_toml.as_bytes())
            .context(BuildErrorKind::OtherError)?;
        writer.flush().context(BuildErrorKind::OtherError)?;
        std::mem::drop(writer);

        lock_file
            .unlock()
            .context("Unable to unlock 'ptx-builder.lock'")?;

        let _cargo_output = cargo_output?;

        let output = BuildOutput::new(self, output_path, crate_type);

        if output.get_assembly_path().exists() {
            Ok(BuildStatus::Success(output))
        } else {
            Err(
                BuildErrorKind::InternalError(String::from("Unable to find PTX assembly output"))
                    .into(),
            )
        }
    }

    fn output_is_not_verbose(line: &str) -> bool {
        !line.starts_with("+ ")
            && !line.contains("Running")
            && !line.contains("Fresh")
            && !line.starts_with("Caused by:")
            && !line.starts_with("  process didn\'t exit successfully: ")
    }
}

impl<'a> BuildOutput<'a> {
    fn new(builder: &'a Builder, output_path: PathBuf, crate_type: CrateType) -> Self {
        BuildOutput {
            builder,
            output_path,
            crate_type,
        }
    }

    /// Returns path to PTX assembly file.
    ///
    /// # Usage
    /// Can be used from `build.rs` script to provide Rust with the path
    /// via environment variable:
    /// ```no_run
    /// use ptx_builder::prelude::*;
    /// # use ptx_builder::error::Result;
    ///
    /// # fn main() -> Result<()> {
    /// if let BuildStatus::Success(output) = Builder::new(".")?.build()? {
    ///     println!(
    ///         "cargo:rustc-env=KERNEL_PTX_PATH={}",
    ///         output.get_assembly_path().display()
    ///     );
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn get_assembly_path(&self) -> PathBuf {
        self.output_path
            .join(TARGET_NAME)
            .join(self.builder.profile.to_string())
            .join("examples")
            .join(format!(
                "{}{}{}.ptx",
                match self.crate_type {
                    CrateType::Binary => self.builder.source_crate.get_name(),
                    CrateType::Library => self.builder.source_crate.get_output_file_prefix(),
                },
                match self.crate_type {
                    CrateType::Binary => '-',
                    CrateType::Library => '_',
                },
                self.builder.prefix,
            ))
    }

    /// Returns a list of crate dependencies.
    ///
    /// # Usage
    /// Can be used from `build.rs` script to notify Cargo the dependencies,
    /// so it can automatically rebuild on changes:
    /// ```no_run
    /// use ptx_builder::prelude::*;
    /// # use ptx_builder::error::Result;
    ///
    /// # fn main() -> Result<()> {
    /// if let BuildStatus::Success(output) = Builder::new(".")?.build()? {
    ///     for path in output.dependencies()? {
    ///         println!("cargo:rerun-if-changed={}", path.display());
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn dependencies(&self) -> Result<Vec<PathBuf>> {
        let mut deps_contents = {
            self.get_deps_file_contents()
                .context("Unable to get crate deps")?
        };

        if deps_contents.is_empty() {
            bail!(BuildErrorKind::InternalError(String::from(
                "Empty deps file",
            )));
        }

        deps_contents = deps_contents
            .chars()
            .skip(3) // workaround for Windows paths starts wuth "[A-Z]:\"
            .skip_while(|c| *c != ':')
            .skip(1)
            .collect::<String>();

        let mut cargo_lock_dir = self.builder.source_crate.get_path();

        // Traverse the workspace directory structure towards the root
        while !cargo_lock_dir.join("Cargo.lock").is_file() {
            cargo_lock_dir = match cargo_lock_dir.parent() {
                Some(parent) => parent,
                None => bail!(BuildErrorKind::InternalError(String::from(
                    "Unable to find Cargo.lock file",
                ))),
            }
        }

        let cargo_deps = vec![
            self.builder.source_crate.get_path().join("Cargo.toml"),
            cargo_lock_dir.join("Cargo.lock"),
        ];

        Ok(deps_contents
            .trim()
            .split(' ')
            .map(|item| PathBuf::from(item.trim()))
            .chain(cargo_deps.into_iter())
            .collect())
    }

    fn get_deps_file_contents(&self) -> Result<String> {
        let crate_deps_path = self
            .output_path
            .join(TARGET_NAME)
            .join(self.builder.profile.to_string())
            .join("examples")
            .join(format!(
                "{}{}{}.d",
                match self.crate_type {
                    CrateType::Binary => self.builder.source_crate.get_name(),
                    CrateType::Library => self.builder.source_crate.get_output_file_prefix(),
                },
                match self.crate_type {
                    CrateType::Binary => '-',
                    CrateType::Library => '_',
                },
                self.builder.prefix,
            ));

        let mut crate_deps_reader =
            BufReader::new(File::open(crate_deps_path).context(BuildErrorKind::OtherError)?);

        let mut crate_deps_contents = String::new();

        crate_deps_reader
            .read_to_string(&mut crate_deps_contents)
            .context(BuildErrorKind::OtherError)?;

        Ok(crate_deps_contents)
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Profile::Debug => write!(fmt, "debug"),
            Profile::Release => write!(fmt, "release"),
        }
    }
}
