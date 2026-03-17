use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "osi-engine",
    about = "OSI mapping reference engine — renders mappings to PostgreSQL views"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render a mapping file to PostgreSQL SQL
    Render {
        /// Path to mapping.yaml
        mapping: PathBuf,
        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Emit CREATE TABLE statements for input tables
        #[arg(long)]
        create_tables: bool,
        /// Add comments showing where user-defined expressions, filters, and strategies appear
        #[arg(long)]
        annotate: bool,
        /// Emit materialized views with unique indexes instead of plain views
        #[arg(long)]
        materialize: bool,
    },
    /// Validate mapping file(s)
    Validate {
        /// Path to a mapping.yaml or directory
        path: Option<PathBuf>,
        /// Recursively find mapping.yaml in directory
        #[arg(long)]
        all: bool,
        /// Show warnings
        #[arg(short, long)]
        verbose: bool,
        /// Only show failures
        #[arg(short, long)]
        quiet: bool,
    },
    /// Emit a GraphViz DOT representation of the view DAG
    Dot {
        /// Path to mapping.yaml
        mapping: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Render {
            mapping,
            output,
            create_tables,
            annotate,
            materialize,
        } => {
            let doc = osi_engine::parser::parse_file(&mapping)?;
            let dag = osi_engine::dag::build_dag(&doc);
            let sql =
                osi_engine::render::render_sql(&doc, &dag, create_tables, annotate, materialize)?;

            match output {
                Some(path) => std::fs::write(&path, &sql)?,
                None => print!("{sql}"),
            }
        }
        Command::Validate {
            path,
            all: _,
            verbose,
            quiet,
        } => {
            let repo_root = std::env::current_dir()?;
            let files = collect_mapping_files(path, &repo_root)?;

            let mut total_errors = 0;
            let mut total_warnings = 0;
            let mut checked = 0;

            for filepath in &files {
                checked += 1;
                let label = filepath
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| filepath.display().to_string());

                let (errors, warnings) = match osi_engine::parser::parse_file(filepath) {
                    Ok(doc) => {
                        let result = osi_engine::validate::validate(&doc);
                        let errors: Vec<String> = result.errors().map(|d| d.to_string()).collect();
                        let warnings: Vec<String> =
                            result.warnings().map(|d| d.to_string()).collect();
                        (errors, warnings)
                    }
                    Err(e) => (vec![format!("[Parse] {e:#}")], vec![]),
                };

                total_errors += errors.len();
                total_warnings += warnings.len();

                if !errors.is_empty() {
                    println!(
                        "  FAIL {label}: {} error(s), {} warning(s)",
                        errors.len(),
                        warnings.len()
                    );
                    for e in &errors {
                        println!("       {e}");
                    }
                    if verbose {
                        for w in &warnings {
                            println!("       {w}");
                        }
                    }
                } else if !warnings.is_empty() && verbose {
                    println!("  WARN {label}: {} warning(s)", warnings.len());
                    for w in &warnings {
                        println!("       {w}");
                    }
                } else if !quiet {
                    let status = if warnings.is_empty() {
                        "OK  ".to_string()
                    } else {
                        let s = if warnings.len() == 1 { "" } else { "s" };
                        format!("OK   ({} warning{s})", warnings.len())
                    };
                    println!("  {status} {label}");
                }
            }

            println!("\n{checked} checked, {total_errors} error(s), {total_warnings} warning(s)");
            if total_errors > 0 {
                std::process::exit(1);
            }
        }
        Command::Dot { mapping } => {
            let doc = osi_engine::parser::parse_file(&mapping)?;
            let dag = osi_engine::dag::build_dag(&doc);
            let dot = osi_engine::dag::to_dot(&dag);
            print!("{dot}");
        }
    }

    Ok(())
}

/// Collect mapping files from a path argument.
fn collect_mapping_files(
    path: Option<PathBuf>,
    repo_root: &std::path::Path,
) -> Result<Vec<PathBuf>> {
    match path {
        Some(p) => {
            if p.is_file() {
                Ok(vec![p])
            } else if p.is_dir() {
                let mut files: Vec<PathBuf> = Vec::new();
                collect_yamls_recursive(&p, &mut files);
                files.sort();
                Ok(files)
            } else {
                anyhow::bail!("Path not found: {}", p.display());
            }
        }
        None => {
            // Default: look for examples/ relative to current dir or parent
            let examples = if repo_root.join("examples").is_dir() {
                repo_root.join("examples")
            } else if repo_root
                .parent()
                .is_some_and(|p| p.join("examples").is_dir())
            {
                repo_root.parent().unwrap().join("examples")
            } else {
                anyhow::bail!("No examples/ directory found; provide a path explicitly");
            };
            let mut files: Vec<PathBuf> = Vec::new();
            collect_yamls_recursive(&examples, &mut files);
            files.sort();
            Ok(files)
        }
    }
}

fn collect_yamls_recursive(dir: &std::path::Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_yamls_recursive(&path, files);
            } else if path.file_name().is_some_and(|n| n == "mapping.yaml") {
                files.push(path);
            }
        }
    }
}
