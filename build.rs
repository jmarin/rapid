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
    configure_macos_libmagic();

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

/// On macOS, libmagic is typically installed via Homebrew into a non-standard prefix
/// that the linker does not search by default. This function emits the correct
/// `rustc-link-search` directive so that `-lmagic` resolves at link time.
#[cfg(target_os = "macos")]
fn configure_macos_libmagic() {
    // Prefer the exact prefix reported by Homebrew for libmagic.
    let brew_lib = Command::new("brew")
        .args(["--prefix", "libmagic"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| format!("{}/lib", s.trim()));

    if let Some(lib_dir) = brew_lib {
        println!("cargo:rustc-link-search=native={lib_dir}");
        return;
    }

    // Fallback: common Homebrew prefixes (Apple Silicon then Intel).
    for dir in ["/opt/homebrew/lib", "/usr/local/lib"] {
        if Path::new(dir).exists() {
            println!("cargo:rustc-link-search=native={dir}");
        }
    }
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
}

/// Resolves the path to the `file` command. On Unix it is expected to be on
/// `PATH`. On Windows it may live inside an MSYS2 prefix that is not on `PATH`,
/// so we probe well-known locations.
fn resolve_file_command() -> PathBuf {
    #[cfg(not(target_os = "windows"))]
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
