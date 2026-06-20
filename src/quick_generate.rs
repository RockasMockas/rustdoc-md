use std::path::Path;

// ---------------------------------------------------------------------------
// Quick-generate API – full pipeline from Cargo project to multi-file Markdown
// ---------------------------------------------------------------------------

/// Finds the project root by walking up from `start_dir` looking for a `Cargo.toml`.
///
/// Returns the path to the directory containing `Cargo.toml`, or an error if none is found.
///
/// # Examples
///
/// ```no_run
/// use rustdoc_md::find_project_root;
///
/// let root = find_project_root(std::env::current_dir().unwrap()).unwrap();
/// println!("Project root: {}", root.display());
/// ```
pub fn find_project_root(start_dir: &Path) -> eyre::Result<std::path::PathBuf> {
    let mut current = start_dir.to_path_buf();

    loop {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists() {
            return Ok(current);
        }
        if !current.pop() {
            return Err(eyre::eyre!(
                "No Cargo.toml found in '{}' or any parent directory.",
                start_dir.display()
            ));
        }
    }
}

/// The output directory name used by the quick-generate flow for multi-file docs.
pub const QUICK_OUTPUT_FOLDER: &str = ".crates-docs";

/// Ensures that the quick-generate output folder is in `.gitignore` at `project_root`.
///
/// If `.gitignore` does not exist it is created. If the entry already exists nothing
/// is changed.
pub fn ensure_gitignore(project_root: &Path, entry: &str) -> eyre::Result<()> {
    let gitignore = project_root.join(".gitignore");

    if !gitignore.exists() {
        std::fs::write(&gitignore, format!("{}\n", entry))?;
        return Ok(());
    }

    let contents = std::fs::read_to_string(&gitignore)?;
    if contents.lines().any(|line| line.trim() == entry) {
        return Ok(());
    }

    std::fs::write(&gitignore, format!("{}\n{}", contents.trim_end(), entry))?;
    Ok(())
}

/// Configuration for the quick-generate pipeline.
///
/// Fields let you control which crates are included, the output folder name,
/// and whether `.gitignore` is updated.
#[derive(Debug, Clone)]
pub struct QuickGenerateConfig {
    /// Include dependency crates in the rustdoc JSON output (default: `true`).
    /// When `false`, `--no-deps` is passed to `cargo doc`.
    pub include_deps: bool,
    /// The folder name inside the project root where multi-file Markdown output
    /// will be written (default: `".crates-docs"`).
    pub output_folder: String,
    /// Whether to add the output folder to `.gitignore` (default: `true`).
    pub update_gitignore: bool,
}

impl Default for QuickGenerateConfig {
    fn default() -> Self {
        Self {
            include_deps: true,
            output_folder: QUICK_OUTPUT_FOLDER.to_string(),
            update_gitignore: true,
        }
    }
}

/// Generates rustdoc JSON for a Cargo project.
///
/// This function attempts nightly rustdoc first, then falls back to stable
/// (with `RUSTC_BOOTSTRAP=1`). The JSON files are written to
/// `<project_root>/target/doc/`.
///
/// Returns the path to the `target/doc` directory on success.
///
/// # Errors
///
/// Returns an error if neither nightly nor stable rustdoc can generate JSON.
pub fn generate_rustdoc_json(project_root: &Path, include_deps: bool) -> eyre::Result<std::path::PathBuf> {
    let json_output_dir = project_root.join("target").join("doc");
    let deps_flag = if include_deps { "" } else { " --no-deps" };

    // Suppress all cargo build output (stdout+stderr -> /dev/null)
    let nightly_result = std::process::Command::new("sh")
        .args([
            "-c",
            &format!("RUSTDOCFLAGS=\"-Z unstable-options --output-format json\" cargo doc{} 2>&1 >/dev/null", deps_flag),
        ])
        .current_dir(project_root)
        .output();

    if let Ok(output) = &nightly_result {
        if output.status.success() {
            return find_json_output(&json_output_dir);
        }
    }

    // If nightly isn't available, fall back to RUSTC_BOOTSTRAP=1 on stable.
    let fallback_result = std::process::Command::new("sh")
        .args([
            "-c",
            &format!("RUSTC_BOOTSTRAP=1 cargo doc{} 2>&1 >/dev/null", deps_flag),
        ])
        .current_dir(project_root)
        .output()?;

    if !fallback_result.status.success() {
        let stderr = String::from_utf8_lossy(&fallback_result.stderr);
        return Err(eyre::eyre!(
            "rustdoc JSON generation failed (even with RUSTC_BOOTSTRAP=1):\n{}",
            stderr
        ));
    }

    find_json_output(&json_output_dir)
}

/// Locates the single `.json` file inside a rustdoc output directory.
///
/// Returns an error if zero or multiple JSON files are found.
pub fn find_json_output(dir: &Path) -> eyre::Result<std::path::PathBuf> {
    let mut json_files: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "json"))
        .collect();

    match json_files.len() {
        0 => Err(eyre::eyre!(
            "No .json file found in {}. Ensure cargo doc ran successfully.",
            dir.display()
        )),
        1 => {
            let json_path = json_files.pop().unwrap().path();
            Ok(json_path)
        }
        _ => {
            let names: Vec<_> = json_files.iter().map(|e| e.file_name()).collect();
            Err(eyre::eyre!(
                "Multiple .json files found in {}. Please specify which one to use:\n  {}",
                dir.display(),
                names.iter()
                    .map(|n| n.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }
}

/// The full quick-generate pipeline.
///
/// This is the library equivalent of running `rustdoc-md --quick`. It:
/// 1. Finds the project root from `start_dir`.
/// 2. Generates rustdoc JSON (nightly first, stable fallback).
/// 3. Converts all JSON files to multi-file Markdown.
/// 4. Optionally updates `.gitignore`.
///
/// # Examples
///
/// ```no_run
/// use rustdoc_md::quick_generate;
///
/// quick_generate(std::env::current_dir().unwrap()).unwrap();
/// ```
pub fn quick_generate(start_dir: &Path) -> eyre::Result<()> {
    quick_generate_with_config(start_dir, QuickGenerateConfig::default())
}

/// The full quick-generate pipeline with custom configuration.
pub fn quick_generate_with_config(start_dir: &Path, config: QuickGenerateConfig) -> eyre::Result<()> {
    // 1. Find project root
    let project_root = find_project_root(start_dir)?;

    // 2. Generate JSON
    let json_output_dir = generate_rustdoc_json(&project_root, config.include_deps)?;

    // 3. Convert all JSON files to multi-file Markdown
    let output_dir = project_root.join(&config.output_folder);
    crate::generate_markdown_for_all_json_in_dir(&json_output_dir, &output_dir)?;

    // 4. Update .gitignore
    if config.update_gitignore {
        ensure_gitignore(&project_root, &config.output_folder)?;
    }

    Ok(())
}
