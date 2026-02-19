use std::env;
use std::fs;
use std::process;

use rshcl::api::validate_bytes;
use rshcl::diagnostics::{Diagnostic, Severity};

fn main() {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "rshcl".to_owned());
    let Some(config_path) = args.next() else {
        eprintln!("usage: {program} <path> [schema-path]");
        process::exit(2);
    };
    let schema_path = args.next();

    if args.next().is_some() {
        eprintln!("usage: {program} <path> [schema-path]");
        process::exit(2);
    }

    let source = match fs::read(&config_path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("error reading `{config_path}`: {error}");
            process::exit(1);
        }
    };

    let schema_source = schema_path.as_deref().map(|schema_path| {
        fs::read(schema_path).unwrap_or_else(|error| {
            eprintln!("error reading schema `{schema_path}`: {error}");
            process::exit(1);
        })
    });

    let validated = validate_bytes(&source, schema_source.as_deref());
    let diagnostics = validated.diagnostics;

    for diagnostic in &diagnostics {
        eprintln!("{}", format_diagnostic(diagnostic));
    }

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        process::exit(1);
    }
}

fn format_diagnostic(diagnostic: &Diagnostic) -> String {
    let severity = match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };

    format!(
        "{severity}: {} at bytes {}..{}",
        diagnostic.message, diagnostic.span.start, diagnostic.span.end
    )
}
