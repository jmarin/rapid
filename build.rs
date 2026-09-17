use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

const CUSTOM_MAGIC_SOURCE_DIR: &str = "magic-files";
const CUSTOM_MAGIC_COMPILED_SUBDIR: &str = "custom-magic-compiled";
const CUSTOM_MAGIC_ENV: &str = "CUSTOM_MAGIC_DATABASE_DIR";
const MERGED_MAGIC_FILENAME: &str = "custom-merged.magic";

fn main() {
    println!("cargo:rerun-if-changed={CUSTOM_MAGIC_SOURCE_DIR}");

    #[cfg(target_os = "macos")]
    configure_macos_native_libs();

    #[cfg(target_os = "windows")]
    configure_windows_libmagic();

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set"));
    let source_dir = manifest_dir.join(CUSTOM_MAGIC_SOURCE_DIR);

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is not set"));
    let compiled_dir = out_dir.join(CUSTOM_MAGIC_COMPILED_SUBDIR);

    fs::create_dir_all(&compiled_dir).expect("failed to create compiled magic output directory");
    clean_directory(&compiled_dir);
    compile_merged_magic_file(&source_dir, &compiled_dir);

    println!(
        "cargo:rustc-env={CUSTOM_MAGIC_ENV}={}",
        compiled_dir.display()
    );
}

/// The Homebrew formulas providing the native libraries this crate links
/// against, each paired with a dylib it is expected to supply.
///
/// `libvips` (via the `libvips` crate) and `libmagic` (via `magic-sys`) emit
/// bare `-lvips`, `-lglib-2.0`, `-lgobject-2.0` and `-lmagic` flags without any
/// search paths of their own. Homebrew installs these under a prefix
/// (`/opt/homebrew` on Apple Silicon, `/usr/local` on Intel) that the macOS
/// linker does not search by default, so each needs an explicit
/// `rustc-link-search` directive to resolve at link time.
///
/// `glib` covers both `-lglib-2.0` and `-lgobject-2.0`, which share a prefix.
#[cfg(target_os = "macos")]
const MACOS_BREW_LIBS: &[(&str, &str)] = &[
    ("libmagic", "libmagic.dylib"),
    ("vips", "libvips.dylib"),
    ("glib", "libglib-2.0.dylib"),
];

/// Emits a `rustc-link-search` directive for each Homebrew-provided native
/// library, warning with an actionable `brew install` hint for any that is
/// missing. Without the hint the failure surfaces only at link time as an
/// opaque `ld: library 'vips' not found`.
#[cfg(target_os = "macos")]
fn configure_macos_native_libs() {
    for (formula, dylib) in MACOS_BREW_LIBS {
        match macos_brew_path(formula, &format!("lib/{dylib}")) {
            Some(dylib_path) => println!(
                "cargo:rustc-link-search=native={}",
                dylib_path
                    .parent()
                    .expect("resolved dylib path always has a parent directory")
                    .display()
            ),
            None => println!(
                "cargo:warning={dylib} not found. Install it with `brew install {formula}`."
            ),
        }
    }
}

/// Resolves `relative` (e.g. `lib/libvips.dylib`) inside the Homebrew keg for
/// `formula`, preferring the prefix Homebrew reports and falling back to the
/// standard Homebrew prefixes (Apple Silicon, then Intel).
///
/// `brew --prefix <formula>` succeeds and prints a path even when the formula
/// is not installed, so the reported prefix is only trusted once `relative` is
/// confirmed to exist inside it.
#[cfg(target_os = "macos")]
fn macos_brew_path(formula: &str, relative: &str) -> Option<PathBuf> {
    let reported_prefix = Command::new("brew")
        .args(["--prefix", formula])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|prefix| PathBuf::from(prefix.trim()));

    reported_prefix
        .into_iter()
        .chain([
            PathBuf::from("/opt/homebrew/opt").join(formula),
            PathBuf::from("/usr/local/opt").join(formula),
        ])
        .map(|prefix| prefix.join(relative))
        .find(|path| path.exists())
}

/// On Windows, libmagic must be installed manually via MSYS2 or vcpkg.
/// This function emits the correct `rustc-link-search` directive so that
/// `-lmagic` resolves at link time.
#[cfg(target_os = "windows")]
fn configure_windows_libmagic() {
    // Prefer vcpkg if VCPKG_ROOT is set.
    if let Ok(vcpkg_root) = env::var("VCPKG_ROOT") {
        let vcpkg_lib = PathBuf::from(&vcpkg_root).join("installed/x64-windows/lib");
        if vcpkg_lib.exists() {
            println!("cargo:rustc-link-search=native={}", vcpkg_lib.display());
            return;
        }
    }

    // Check MSYS2 via MSYSTEM_PREFIX (set inside MSYS2 shells).
    if let Ok(prefix) = env::var("MSYSTEM_PREFIX") {
        let lib_dir = PathBuf::from(&prefix).join("lib");
        if lib_dir.exists() {
            println!("cargo:rustc-link-search=native={}", lib_dir.display());
            return;
        }
    }

    // Fallback: common MSYS2 installation paths.
    for prefix in ["C:/msys64/mingw64", "C:/msys64/ucrt64", "C:/msys64/clang64"] {
        let lib_dir = Path::new(prefix).join("lib");
        if lib_dir.exists() {
            println!("cargo:rustc-link-search=native={}", lib_dir.display());
            return;
        }
    }

    println!(
        "cargo:warning=libmagic not found. Install via MSYS2 (pacman -S mingw-w64-x86_64-file) or vcpkg."
    );
}

fn clean_directory(dir: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let _ = fs::remove_file(path);
        }
    }
}

fn compile_merged_magic_file(source_dir: &Path, compiled_dir: &Path) {
    let mut source_files = fs::read_dir(source_dir)
        .expect("failed to read custom magic source directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && !path.extension().is_some_and(|extension| extension == "mgc")
        })
        .collect::<Vec<_>>();

    source_files.sort();

    let merged_source_path = compiled_dir.join(MERGED_MAGIC_FILENAME);
    let mut merged_content = Vec::new();

    for source_path in &source_files {
        println!("cargo:rerun-if-changed={}", source_path.display());

        let source_content = fs::read(source_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", source_path.display()));
        merged_content.extend_from_slice(&source_content);
        merged_content.extend_from_slice(b"\n\n");
    }

    fs::write(&merged_source_path, merged_content).unwrap_or_else(|e| {
        panic!(
            "failed to write merged custom magic source {}: {e}",
            merged_source_path.display()
        )
    });

    let merged_source_name = merged_source_path
        .file_name()
        .expect("merged custom magic source file name missing");

    let file_cmd = resolve_file_command();
    let status = Command::new(&file_cmd)
        .arg("-C")
        .arg("-m")
        .arg(merged_source_name)
        .current_dir(compiled_dir)
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "failed to execute '{}' -C -m: {e}. \
                 On Windows, install libmagic via MSYS2 and ensure its bin directory is on PATH.",
                file_cmd.display()
            )
        });

    if !status.success() {
        panic!(
            "file -C -m failed for merged custom magic source {}",
            merged_source_path.display()
        );
    }

    fs::remove_file(merged_source_path).expect("failed to remove merged custom magic source file");

    prune_unexpected_outputs(compiled_dir, &format!("{MERGED_MAGIC_FILENAME}.mgc"));
}

/// Removes every file in the compiled output directory except `expected`.
///
/// Apple's `file` additionally compiles its own default system magic database
/// into the working directory as a stray 7 MB `magic.mgc`. `magic.rs` loads
/// every `.mgc` it finds in this directory, so the stray would be picked up and
/// applied alongside the custom rules.
fn prune_unexpected_outputs(compiled_dir: &Path, expected: &str) {
    let Ok(entries) = fs::read_dir(compiled_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .is_some_and(|name| name.to_str() != Some(expected))
        {
            let _ = fs::remove_file(path);
        }
    }
}

/// Resolves the path to the `file` command used to compile the custom magic
/// database.
///
/// The compiled database must come from the same `file` release as the libmagic
/// that gets linked into the binary, because a compiled `.mgc` carries a format
/// version that libmagic refuses to load if it does not match its own. On macOS
/// the two disagree by default: Apple's `/usr/bin/file` (5.41) emits format
/// version 16, while Homebrew's libmagic (5.46) only loads version 20. When
/// that happens libmagic does not fail loudly — it falls back to parsing the
/// binary `.mgc` as magic *source text*, which yields a stream of
/// `offset ... invalid` warnings and a database that matches nothing. So prefer
/// Homebrew's `file`, which is built against the same libmagic.
///
/// On Windows `file` may live inside an MSYS2 prefix that is not on `PATH`, so
/// we probe well-known locations.
fn resolve_file_command() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        macos_brew_path("file-formula", "bin/file").unwrap_or_else(|| {
            println!(
                "cargo:warning=Homebrew `file` not found, falling back to the system `file`. \
                 Apple's `file` compiles magic databases in a format Homebrew's libmagic \
                 cannot load, which silently disables custom MIME detection. \
                 Install it with `brew install file-formula`."
            );
            PathBuf::from("file")
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        PathBuf::from("file")
    }

    #[cfg(target_os = "windows")]
    {
        // Inside an MSYS2 shell the prefix is available as an env var.
        if let Ok(prefix) = env::var("MSYSTEM_PREFIX") {
            let file_exe = PathBuf::from(&prefix).join("bin/file.exe");
            if file_exe.exists() {
                return file_exe;
            }
        }

        // Probe common MSYS2 paths.
        for prefix in ["C:/msys64/mingw64", "C:/msys64/ucrt64", "C:/msys64/clang64"] {
            let file_exe = Path::new(prefix).join("bin/file.exe");
            if file_exe.exists() {
                return file_exe;
            }
        }

        // Last resort: assume it is on PATH.
        PathBuf::from("file")
    }
}
