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

    let status = Command::new("file")
        .arg("-C")
        .arg("-m")
        .arg(merged_source_name)
        .current_dir(compiled_dir)
        .status()
        .expect("failed to execute file -C -m for merged custom magic source");

    if !status.success() {
        panic!(
            "file -C -m failed for merged custom magic source {}",
            merged_source_path.display()
        );
    }

    fs::remove_file(merged_source_path).expect("failed to remove merged custom magic source file");
}
