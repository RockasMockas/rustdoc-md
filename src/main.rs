use std::path::PathBuf;
use std::process::Command;

use clap::Parser;
use rustdoc_md::rustdoc_json_types::ParsedCrateDoc;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Quick mode: find project root, generate JSON, output multi-file to `.crates-docs`
    #[arg(long)]
    quick: bool,

    /// The path to a rust docs json file
    #[arg(short, long, value_name = "JSON_PATH")]
    input_json: Option<PathBuf>,

    /// The output path.
    /// If not specified, Markdown is printed to stdout (single-document mode).
    /// If specified and --multi-file is not used, output is a single Markdown file.
    /// If specified and --multi-file is used, output is a directory for multiple Markdown files.
    #[arg(short, long, value_name = "OUTPUT_PATH")]
    output: Option<PathBuf>,

    /// Generate a directory of markdown files instead of a single document.
    /// Requires --output to be specified.
    #[arg(long)]
    multi_file: bool,

    /// Generate the rustdoc JSON from the current project (nightly first, falls back to stable),
    /// then use the generated JSON. Overrides --input-json if provided.
    /// If --output is not specified, uses the current project's directory as the output folder.
    #[arg(long, short = 'g', help = "Generate rustdoc JSON first (nightly first, stable fallback), then convert it")]
    generate_json: bool,
}

fn main() -> eyre::Result<()> {
    let cli = Cli::parse();

    if cli.quick {
        return run_quick();
    }

    run_std(&cli)
}

fn run_quick() -> eyre::Result<()> {
    // Find project root (first Cargo.toml going upwards)
    let project_root = find_project_root()?;

    // Generate JSON for all crates (including dependencies)
    let json_output_dir = project_root.join("target").join("doc");

    // Suppress all cargo build output (stdout+stderr -> /dev/null)
    let nightly_result = std::process::Command::new("sh")
        .args([
            "-c",
            "RUSTDOCFLAGS=\"-Z unstable-options --output-format json\" cargo doc 2>&1 >/dev/null",
        ])
        .current_dir(&project_root)
        .output();

    if let Ok(output) = &nightly_result {
        if output.status.success() {
            return generate_all(&json_output_dir);
        }
    }

    // If nightly isn't available, fall back to RUSTC_BOOTSTRAP=1 on stable.
    let fallback_result = std::process::Command::new("sh")
        .args([
            "-c",
            // On stable, use RUSTC_BOOTSTRAP=1 without -Z flags since -Z is nightly-only.
            "RUSTC_BOOTSTRAP=1 cargo doc 2>&1 >/dev/null",
        ])
        .current_dir(&project_root)
        .output()?;

    if !fallback_result.status.success() {
        let stderr = String::from_utf8_lossy(&fallback_result.stderr);
        return Err(eyre::eyre!(
            "rustdoc JSON generation failed (even with RUSTC_BOOTSTRAP=1):\n{}",
            stderr
        ));
    }

    generate_all(&json_output_dir)
}

/// Generate markdown for all JSON files in target/doc/
fn generate_all(json_output_dir: &std::path::Path) -> eyre::Result<()> {
    let project_root = find_project_root()?;
    let output_dir = project_root.join(".crates-docs");

    // Use the built-in multi-crate generator
    rustdoc_md::generate_markdown_for_all_json_in_dir(json_output_dir, &output_dir)?;

    // Ensure .crates-docs is in .gitignore
    ensure_gitignore(&project_root)?;

    Ok(())
}

/// Ensure `.crates-docs` is in .gitignore at the project root.
fn ensure_gitignore(project_root: &PathBuf) -> eyre::Result<()> {
    let gitignore = project_root.join(".gitignore");
    let entry = ".crates-docs";

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

fn find_project_root() -> eyre::Result<PathBuf> {
    let mut current = std::env::current_dir()?;

    loop {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists() {
            return Ok(current);
        }
        if !current.pop() {
            return Err(eyre::eyre!(
                "No Cargo.toml found in current directory or any parent directory."
            ));
        }
    }
}

fn generate_json_at(project_root: &PathBuf) -> eyre::Result<PathBuf> {
    let json_output_dir = project_root.join("target").join("doc");

    // Suppress all cargo output: redirect stdout+stderr to /dev/null
    let nightly_result = Command::new("sh")
        .args([
            "-c",
            "RUSTDOCFLAGS=\"-Z unstable-options --output-format json\" cargo doc --no-deps 2>&1 >/dev/null",
        ])
        .current_dir(project_root)
        .output();

    if let Ok(output) = &nightly_result {
        if output.status.success() {
            return find_json_output(&json_output_dir);
        }
    }

    // If nightly isn't available, fall back to RUSTC_BOOTSTRAP=1 on stable.
    let fallback_result = Command::new("sh")
        .args([
            "-c",
            // On stable, we use RUSTC_BOOTSTRAP=1 without -Z flags since -Z is nightly-only.
            // json output-format is available on stable when RUSTC_BOOTSTRAP=1 is set.
            "RUSTC_BOOTSTRAP=1 cargo doc --no-deps 2>&1 >/dev/null",
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

fn run_std(cli: &Cli) -> eyre::Result<()> {
    let input_path = if cli.generate_json {
        generate_json_at(&std::env::current_dir()?)?
    } else {
        cli.input_json
            .as_ref()
            .ok_or_else(|| eyre::eyre!("--input-json (-i) is required unless --generate-json (-g) or --quick is used"))?
            .clone()
    };

    let krate = ParsedCrateDoc::from_file(&input_path)?;

    let output_path = if let Some(p) = &cli.output {
        p.clone()
    } else if cli.generate_json {
        std::env::current_dir()?
    } else if cli.quick {
        find_project_root()?
    } else {
        println!("{}", krate.to_string());
        return Ok(());
    };

    if cli.multi_file {
        if output_path.exists() && !output_path.is_dir() {
            return Err(eyre::eyre!(
                "For multi-file output, the output path '{}' must be a directory, but it's a file.",
                output_path.display()
            ));
        }
        krate.to_multi_file(&output_path, None)?;
    } else {
        if output_path.is_dir() {
            return Err(eyre::eyre!(
                "For single-file output, the output path '{}' must be a file, but it's a directory.",
                output_path.display()
            ));
        }
        krate.to_single_file(&output_path, None)?;
    }

    Ok(())
}

fn find_json_output(dir: &std::path::Path) -> eyre::Result<PathBuf> {
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
