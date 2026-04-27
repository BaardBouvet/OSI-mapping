//! Minimal v2 CLI.

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "osi-engine",
    about = "OSI mapping reference engine — v2 (PostgreSQL views + SPARQL)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Backend {
    /// PostgreSQL view DDL.
    Pg,
    /// SPARQL plan summary (no executor in CLI; see conformance tests).
    Sparql,
}

#[derive(Subcommand)]
enum Command {
    /// Parse a v2 mapping file and print a debug dump of the model.
    Parse {
        /// Path to mapping.yaml (v2 schema).
        mapping: PathBuf,
    },
    /// Validate a v2 mapping file: JSON Schema (Pass 0) + serde parse.
    ///
    /// Reports all structural errors at once instead of failing on the
    /// first one. Exits 0 if the file passes both passes; non-zero
    /// otherwise.
    Validate {
        /// Path to mapping.yaml (v2 schema).
        mapping: PathBuf,
    },
    /// Render a v2 mapping for the chosen backend.
    ///
    /// Without --out-dir: prints a human-readable pipeline summary to stdout
    /// (or --output file).
    ///
    /// With --out-dir (SPARQL only): writes one CONSTRUCT artifact file per
    /// named graph.  Register each `*.sparql` file with the triplestore's
    /// rule API; only LIFT is then needed — downstream graphs update
    /// automatically.
    Render {
        /// Path to mapping.yaml (v2 schema).
        mapping: PathBuf,
        /// Output file for the text summary (default: stdout). Ignored when
        /// --out-dir is set.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Which backend to emit.
        #[arg(short, long, value_enum, default_value_t = Backend::Pg)]
        backend: Backend,
        /// (SPARQL only) Write individual artifact files to this directory
        /// instead of printing a text summary. The directory is created if
        /// it does not exist.
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// (SPARQL only) Base IRI to root all generated graph and property
        /// IRIs at.  Must end with `/`.  Defaults to `https://osi.test/`.
        #[arg(long, default_value = "https://osi.test/")]
        base_iri: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Parse { mapping } => {
            let doc = osi_engine::parser::parse_file(&mapping)?;
            println!("{doc:#?}");
        }
        Command::Validate { mapping } => {
            let yaml = std::fs::read_to_string(&mapping)
                .map_err(|e| anyhow::anyhow!("reading {}: {e}", mapping.display()))?;

            // Pass 0: JSON Schema. Collect all structural errors and print
            // them; do NOT short-circuit on serde even if schema fails —
            // showing both helps users.
            let mut had_errors = false;
            let value: serde_json::Value = match serde_yaml::from_str(&yaml) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{}: YAML parse failed: {e}", mapping.display());
                    std::process::exit(1);
                }
            };
            if let Err(errs) = osi_engine::validate::validate_schema(&value) {
                had_errors = true;
                eprintln!(
                    "{}: schema validation failed ({} error{}):",
                    mapping.display(),
                    errs.len(),
                    if errs.len() == 1 { "" } else { "s" }
                );
                for e in &errs {
                    eprintln!("  - {e}");
                }
            }

            // Pass 1: typed deserialization (serde) — catches things the
            // schema doesn't (custom version check, etc.).
            if let Err(e) = osi_engine::parser::parse_str(&yaml) {
                had_errors = true;
                eprintln!("{}: parse failed: {e:#}", mapping.display());
            }

            if had_errors {
                std::process::exit(1);
            }
            println!("{}: ok", mapping.display());
        }
        Command::Render {
            mapping,
            output,
            backend,
            out_dir,
            base_iri,
        } => {
            let doc = osi_engine::parser::parse_file(&mapping)?;

            // --out-dir is SPARQL-only: write individual artifact files.
            if let (Some(dir), Backend::Sparql) = (&out_dir, backend) {
                let plan = osi_engine::render::render_sparql_with_base(&doc, &base_iri)?;
                plan.write_artifacts(dir)?;
                eprintln!("Wrote SPARQL artifacts to {}", dir.display());
                return Ok(());
            }

            let text = match backend {
                Backend::Pg => osi_engine::render::render_pg(&doc)?,
                Backend::Sparql => {
                    let plan = osi_engine::render::render_sparql_with_base(&doc, &base_iri)?;
                    format!("{plan}")
                }
            };
            match output {
                Some(path) => std::fs::write(&path, &text)?,
                None => print!("{text}"),
            }
        }
    }
    Ok(())
}
