use std::{
    env::{self, current_dir},
    fs::{remove_dir_all, File},
    io::prelude::*,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use antidote::Mutex;

use ptx_builder::{error::*, prelude::*};

static ENV_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[test]
fn should_provide_output_path() {
    let _lock = ENV_MUTEX.lock();

    cleanup_temp_location();

    let builder = Builder::new("tests/fixtures/sample-crate").unwrap();

    match builder.disable_colors().build().unwrap() {
        BuildStatus::Success(output) => {
            assert!(output
                .get_assembly_path()
                .starts_with(Path::new(env!("OUT_DIR")).join("sample_ptx_crate"),));
        }

        BuildStatus::NotNeeded => unreachable!(),
    }
}

#[test]
fn should_write_assembly() {
    let _lock = ENV_MUTEX.lock();

    cleanup_temp_location();

    let builder = Builder::new("tests/fixtures/sample-crate").unwrap();

    match builder.disable_colors().build().unwrap() {
        BuildStatus::Success(output) => {
            let mut assembly_contents = String::new();

            File::open(output.get_assembly_path())
                .unwrap()
                .read_to_string(&mut assembly_contents)
                .unwrap();

            assert!(output
                .get_assembly_path()
                .to_string_lossy()
                .contains("release"));

            assert!(assembly_contents.contains(".visible .entry the_kernel("));
        }

        BuildStatus::NotNeeded => unreachable!(),
    }
}

#[test]
fn should_build_mixed_crate_lib() {
    let _lock = ENV_MUTEX.lock();

    cleanup_temp_location();

    let builder = Builder::new("tests/fixtures/mixed-crate").unwrap();

    match builder
        .set_crate_type(CrateType::Library)
        .disable_colors()
        .build()
        .unwrap()
    {
        BuildStatus::Success(output) => {
            let mut assembly_contents = String::new();

            println!("{}", output.get_assembly_path().display());

            File::open(output.get_assembly_path())
                .unwrap()
                .read_to_string(&mut assembly_contents)
                .unwrap();

            assert!(output
                .get_assembly_path()
                .to_string_lossy()
                .contains("release"));

            assert!(assembly_contents.contains(".visible .entry the_kernel("));
        }

        BuildStatus::NotNeeded => unreachable!(),
    }
}

#[test]
fn should_handle_rebuild_without_changes() {
    let _lock = ENV_MUTEX.lock();

    cleanup_temp_location();

    let builder = {
        Builder::new("tests/fixtures/sample-crate")
            .unwrap()
            .disable_colors()
    };

    builder.build().unwrap();

    match builder.build().unwrap() {
        BuildStatus::Success(output) => {
            let mut assembly_contents = String::new();

            File::open(output.get_assembly_path())
                .unwrap()
                .read_to_string(&mut assembly_contents)
                .unwrap();

            assert!(output
                .get_assembly_path()
                .to_string_lossy()
                .contains("release"));

            assert!(assembly_contents.contains(".visible .entry the_kernel("));
        }

        BuildStatus::NotNeeded => unreachable!(),
    }
}

#[test]
fn should_write_assembly_in_debug_mode() {
    let _lock = ENV_MUTEX.lock();

    cleanup_temp_location();

    let builder = Builder::new("tests/fixtures/sample-crate").unwrap();

    match builder
        .set_profile(Profile::Debug)
        .disable_colors()
        .build()
        .unwrap()
    {
        BuildStatus::Success(output) => {
            let mut assembly_contents = String::new();

            File::open(output.get_assembly_path())
                .unwrap()
                .read_to_string(&mut assembly_contents)
                .unwrap();

            assert!(output
                .get_assembly_path()
                .to_string_lossy()
                .contains("debug"));

            assert!(assembly_contents.contains(".visible .entry the_kernel("));
        }

        BuildStatus::NotNeeded => unreachable!(),
    }
}

#[test]
fn should_report_about_build_failure() {
    let _lock = ENV_MUTEX.lock();

    cleanup_temp_location();

    let builder = Builder::new("tests/fixtures/faulty-crate")
        .unwrap()
        .disable_colors();

    let output = builder.build();
    let crate_absoulte_path = current_dir()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("faulty-crate");

    let lib_path = PathBuf::from("src").join("lib.rs");

    let crate_absoulte_path_str = crate_absoulte_path.display().to_string();

    match output.unwrap_err().kind() {
        BuildErrorKind::BuildFailed(diagnostics) => {
            assert_eq!(
                diagnostics
                    .iter()
                    .filter(|item| !item.contains("Blocking waiting")
                        && !item.contains("Compiling core")
                        && !item.contains("Compiling compiler_builtins")
                        && !item.contains("Finished release [optimized] target(s)"))
                    .collect::<Vec<_>>(),
                &[
                    &format!(
                        "   Compiling faulty-ptx_crate v0.1.0 ({})",
                        crate_absoulte_path_str
                    ),
                    "error[E0425]: cannot find function `external_fn` in this scope",
                    &format!(" --> {}:7:20", lib_path.display()),
                    "  |",
                    "7 |     *y.offset(0) = external_fn(*x.offset(0)) * a;",
                    "  |                    ^^^^^^^^^^^ not found in this scope",
                    "",
                    "For more information about this error, try `rustc --explain E0425`.",
                    "error: could not compile `faulty-ptx_crate` (lib) due to 1 previous error",
                    "",
                ]
            );
        }

        _ => unreachable!("it should fail with proper error"),
    }
}

#[test]
fn should_provide_crate_source_files() {
    let _lock = ENV_MUTEX.lock();

    let crate_path = {
        current_dir()
            .unwrap()
            .join("tests")
            .join("fixtures")
            .join("sample-crate")
    };

    let builder = Builder::new(crate_path.display().to_string()).unwrap();

    match builder.disable_colors().build().unwrap() {
        BuildStatus::Success(output) => {
            let mut sources = output.dependencies().unwrap();
            let mut expectations = vec![
                crate_path.join("src").join("lib.rs"),
                crate_path.join("src").join("mod1.rs"),
                crate_path.join("src").join("mod2.rs"),
                crate_path.join("Cargo.toml"),
                crate_path.join("Cargo.lock"),
            ];

            sources.sort();
            expectations.sort();

            assert_eq!(sources, expectations);
        }

        BuildStatus::NotNeeded => unreachable!(),
    }
}

#[test]
fn should_not_get_built_recursively() {
    let _lock = ENV_MUTEX.lock();

    env::set_var("PTX_CRATE_BUILDING", "1");

    assert!(!Builder::is_build_needed());
    let builder = Builder::new("tests/fixtures/sample-crate").unwrap();

    match builder.disable_colors().build().unwrap() {
        BuildStatus::NotNeeded => {}
        BuildStatus::Success(_) => unreachable!(),
    }

    env::set_var("PTX_CRATE_BUILDING", "");
}

fn cleanup_temp_location() {
    let crate_names = &[
        "faulty_ptx_crate",
        "sample_app_ptx_crate",
        "sample_ptx_crate",
        "mixed_crate",
    ];

    for name in crate_names {
        remove_dir_all(Path::new(env!("OUT_DIR")).join(name)).unwrap_or_default();
    }
}
