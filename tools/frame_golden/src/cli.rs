//! Command-line surface: hand-rolled parsing (no argument crate exists in
//! the workspace) and the thin wiring from parsed commands onto the
//! library API.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::adapter::{RunOutput, RunProducts, RunRequest};
use crate::capture;
use crate::compare::{self, CompareReport, DeltaReport, Difference};
use crate::error::{FrameGoldenError, FrameGoldenResult};
use crate::golden::GoldenDoc;
use crate::manifest::{self, GoldenManifest};

/// The default committed-fixture directory, relative to the repo root.
pub const GOLDEN_DIR: &str = "tests/fixtures/golden";

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Capture {
        name: String,
    },
    Materialize {
        name: String,
    },
    MaterializeAll,
    Compare {
        name: String,
        columns: Option<Vec<String>>,
    },
    CompareAll {
        dir: Option<PathBuf>,
    },
    Delta {
        name: String,
        columns: Option<Vec<String>>,
    },
    Diff {
        dir_a: PathBuf,
        dir_b: PathBuf,
    },
    Run {
        manifest: PathBuf,
        repeat: usize,
        duration_s: Option<i64>,
        output: RunTarget,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunTarget {
    /// Keep frames in memory; write no Parquet.
    InMemory,
    /// Write full products as Parquet into the given directory.
    WriteTo(PathBuf),
    /// Keep nothing: no engine file output, no retained batches.
    Discard,
}

const USAGE: &str = "\
frame_golden capture <name>
frame_golden materialize <name> | materialize-all
frame_golden compare <name> [--columns <c1,c2,...>]
frame_golden compare-all [dir]
frame_golden delta <name> [--columns <c1,c2,...>]
frame_golden diff <dir-a> <dir-b>
frame_golden run <manifest-path> [--repeat <k>] [--duration-s <s>]
                 [--output-dir <dir> | --discard-output]";

/// Parses argv (without the program name).
pub fn parse(args: &[String]) -> FrameGoldenResult<Command> {
    let Some(command) = args.first() else {
        return Err(FrameGoldenError::Usage(USAGE.to_string()));
    };
    let rest = &args[1..];
    match command.as_str() {
        "capture" => {
            let name = one_positional(rest, "capture")?;
            Ok(Command::Capture { name })
        }
        "materialize" => {
            let name = one_positional(rest, "materialize")?;
            Ok(Command::Materialize { name })
        }
        "materialize-all" => {
            no_arguments(rest, "materialize-all")?;
            Ok(Command::MaterializeAll)
        }
        "compare" => {
            let (name, flags) = one_positional_with_flags(rest, "compare")?;
            let columns = parse_columns(&flags)?;
            Ok(Command::Compare { name, columns })
        }
        "compare-all" => {
            let mut dir = None;
            let mut positionals = Vec::new();
            for arg in rest.iter() {
                if arg == "--columns" {
                    // compare-all takes no column selection; rejected below.
                    return Err(FrameGoldenError::Usage(
                        "compare-all takes no --columns".to_string(),
                    ));
                }
                positionals.push(arg.clone());
            }
            if positionals.len() > 1 {
                return Err(FrameGoldenError::Usage(format!(
                    "compare-all takes at most one directory argument\n{USAGE}"
                )));
            }
            if let Some(single) = positionals.pop() {
                dir = Some(PathBuf::from(single));
            }
            Ok(Command::CompareAll { dir })
        }
        "delta" => {
            let (name, flags) = one_positional_with_flags(rest, "delta")?;
            let columns = parse_columns(&flags)?;
            Ok(Command::Delta { name, columns })
        }
        "diff" => {
            let mut positionals = Vec::new();
            for arg in rest {
                if arg.starts_with('-') {
                    return Err(FrameGoldenError::Usage(format!(
                        "diff takes two directories\n{USAGE}"
                    )));
                }
                positionals.push(PathBuf::from(arg));
            }
            if positionals.len() != 2 {
                return Err(FrameGoldenError::Usage(format!(
                    "diff takes exactly two directories\n{USAGE}"
                )));
            }
            let dir_b = positionals.pop().expect("len checked");
            let dir_a = positionals.pop().expect("len checked");
            Ok(Command::Diff { dir_a, dir_b })
        }
        "run" => {
            let mut positionals = Vec::new();
            let mut repeat = 1usize;
            let mut duration_s = None;
            let mut output = RunTarget::InMemory;
            let mut iter = rest.iter();
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "--repeat" => {
                        repeat = iter
                            .next()
                            .ok_or_else(|| missing_value("--repeat"))?
                            .parse()
                            .map_err(|_| {
                                FrameGoldenError::Usage("--repeat expects an integer".to_string())
                            })?;
                        if repeat == 0 {
                            return Err(FrameGoldenError::Usage(
                                "--repeat expects a positive integer".to_string(),
                            ));
                        }
                    }
                    "--duration-s" => {
                        duration_s = Some(
                            iter.next()
                                .ok_or_else(|| missing_value("--duration-s"))?
                                .parse()
                                .map_err(|_| {
                                    FrameGoldenError::Usage(
                                        "--duration-s expects an integer".to_string(),
                                    )
                                })?,
                        );
                    }
                    "--output-dir" => {
                        if output != RunTarget::InMemory {
                            return Err(FrameGoldenError::Usage(
                                "--output-dir and --discard-output are exclusive".to_string(),
                            ));
                        }
                        let dir = PathBuf::from(
                            iter.next().ok_or_else(|| missing_value("--output-dir"))?,
                        );
                        output = RunTarget::WriteTo(dir);
                    }
                    "--discard-output" => {
                        if output != RunTarget::InMemory {
                            return Err(FrameGoldenError::Usage(
                                "--output-dir and --discard-output are exclusive".to_string(),
                            ));
                        }
                        output = RunTarget::Discard;
                    }
                    other if other.starts_with('-') => {
                        return Err(FrameGoldenError::Usage(format!(
                            "unknown flag {other:?}\n{USAGE}"
                        )));
                    }
                    _ => positionals.push(PathBuf::from(arg)),
                }
            }
            if positionals.len() != 1 {
                return Err(FrameGoldenError::Usage(format!(
                    "run takes exactly one manifest path\n{USAGE}"
                )));
            }
            Ok(Command::Run {
                manifest: positionals.pop().expect("len checked"),
                repeat,
                duration_s,
                output,
            })
        }
        other => Err(FrameGoldenError::Usage(format!(
            "unknown command {other:?}\n{USAGE}"
        ))),
    }
}

fn one_positional(args: &[String], command: &str) -> FrameGoldenResult<String> {
    let (positional, flags) = one_positional_with_flags(args, command)?;
    if !flags.is_empty() {
        return Err(FrameGoldenError::Usage(format!(
            "{command} takes no flags\n{USAGE}"
        )));
    }
    Ok(positional)
}

fn one_positional_with_flags(
    args: &[String],
    command: &str,
) -> FrameGoldenResult<(String, Vec<String>)> {
    let mut positional = None;
    let mut flags = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--columns" {
            flags.push("--columns".to_string());
            flags.push(
                iter.next()
                    .ok_or_else(|| missing_value("--columns"))?
                    .clone(),
            );
        } else if arg.starts_with('-') {
            return Err(FrameGoldenError::Usage(format!(
                "unknown flag {arg:?} for {command}\n{USAGE}"
            )));
        } else if positional.is_none() {
            positional = Some(arg.clone());
        } else {
            return Err(FrameGoldenError::Usage(format!(
                "{command} takes exactly one name\n{USAGE}"
            )));
        }
    }
    let positional = positional
        .ok_or_else(|| FrameGoldenError::Usage(format!("{command} requires a name\n{USAGE}")))?;
    Ok((positional, flags))
}

fn no_arguments(args: &[String], command: &str) -> FrameGoldenResult<()> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(FrameGoldenError::Usage(format!(
            "{command} takes no arguments\n{USAGE}"
        )))
    }
}

fn parse_columns(flags: &[String]) -> FrameGoldenResult<Option<Vec<String>>> {
    let mut columns = None;
    let mut iter = flags.iter();
    while let Some(flag) = iter.next() {
        if flag == "--columns" {
            let value = iter.next().ok_or_else(|| missing_value("--columns"))?;
            let selected: Vec<String> = value
                .split(',')
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_string)
                .collect();
            // An empty selection would compare nothing and pass.
            if selected.is_empty() {
                return Err(FrameGoldenError::Usage(format!(
                    "--columns expects at least one column name\n{USAGE}"
                )));
            }
            columns = Some(selected);
        }
    }
    Ok(columns)
}

fn missing_value(flag: &str) -> FrameGoldenError {
    FrameGoldenError::Usage(format!("{flag} expects a value\n{USAGE}"))
}

/// The repository root for CLI invocations: the nearest ancestor of the
/// working directory holding a `.git` entry, with the same compile-time
/// fallback the manifest loader uses.
pub fn discover_repo_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut dir = cwd;
    loop {
        if dir.join(".git").exists() {
            return dir;
        }
        if !dir.pop() {
            return manifest::repo_root(Path::new("/"));
        }
    }
}

fn load_manifest_for(root: &Path, name: &str) -> FrameGoldenResult<(GoldenManifest, PathBuf)> {
    let path = manifest::resolve_manifest(root, name);
    let loaded = GoldenManifest::load(&path)?;
    // Compare and delta run the same manifest the capture did: under a
    // different feature set they would compare unrelated behaviour, so
    // the capture/materialize rule applies identically here.
    capture::check_features(&loaded, &path)?;
    Ok((loaded, path))
}

/// Loads the committed golden document for a manifest, pointing at
/// `capture` when it does not exist yet.
fn load_golden_doc(golden_path: &Path) -> FrameGoldenResult<GoldenDoc> {
    let bytes = std::fs::read(golden_path).map_err(|err| FrameGoldenError::Manifest {
        path: golden_path.to_path_buf(),
        detail: format!("no committed golden ({err}): run `frame_golden capture` first"),
    })?;
    GoldenDoc::from_bytes(&bytes)
}

/// Executes a parsed command and returns the process exit code.
pub fn execute(command: Command) -> FrameGoldenResult<i32> {
    let root = discover_repo_root();
    match command {
        Command::Capture { name } => {
            let (_manifest, manifest_path) = load_manifest_for(&root, &name)?;
            let frames_dir = capture::frames_dir_for(
                &root,
                &manifest_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            let captured = capture::capture(&manifest_path, &root, Some(&frames_dir))?;
            println!("captured {}", captured.golden_path.display());
            for path in list_frames(&frames_dir) {
                println!("wrote {}", path.display());
            }
            Ok(0)
        }
        Command::Materialize { name } => {
            let (_manifest, manifest_path) = load_manifest_for(&root, &name)?;
            let frames_dir = capture::frames_dir_for(
                &root,
                &manifest_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            let captured = capture::materialize(&manifest_path, &root, &frames_dir)?;
            println!("materialized {}", captured.golden_path.display());
            for path in list_frames(&frames_dir) {
                println!("wrote {}", path.display());
            }
            Ok(0)
        }
        Command::MaterializeAll => {
            let dir = root.join(GOLDEN_DIR);
            let mut exit = 0;
            for path in manifest::list_manifests(&dir)? {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let frames_dir = capture::frames_dir_for(&root, &name);
                match capture::materialize(&path, &root, &frames_dir) {
                    Ok(_) => println!("materialized {name}: ok"),
                    Err(err) => {
                        println!("materialized {name}: FAILED: {err}");
                        exit = 1;
                    }
                }
            }
            Ok(exit)
        }
        Command::Compare { name, columns } => {
            let (loaded, manifest_path) = load_manifest_for(&root, &name)?;
            let fixture_name = manifest_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let frames_dir = capture::frames_dir_for(&root, &fixture_name);
            let golden = load_golden_doc(&capture::committed_golden_path(&manifest_path))?;
            let products = run_products(&loaded, &root, RunOutput::Full, None)?;
            let report = compare::compare_products(
                &fixture_name,
                &golden,
                &products.frames,
                &products.metrics_rows,
                products.health.as_ref(),
                &columns,
                Some(&frames_dir),
            )?;
            print_compare_report(&report);
            Ok(if report.is_identical() { 0 } else { 1 })
        }
        Command::CompareAll { dir } => {
            let fixture_dir = dir.unwrap_or_else(|| root.join(GOLDEN_DIR));
            let running = manifest::running_features();
            let selected = manifest::select_manifests(&fixture_dir, &running)?;
            if selected.is_empty() {
                println!(
                    "no manifests matched the running feature set {running:?} in {}",
                    fixture_dir.display()
                );
                return Ok(1);
            }
            let mut exit = 0;
            for path in &selected {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                // Every failure below names its manifest before the loop
                // continues: one broken fixture must not hide the others.
                match compare_all_one(path, &name, &root) {
                    Ok(report) if report.is_identical() => println!("compared {name}: identical"),
                    Ok(report) => {
                        println!("compared {name}: FAILED");
                        print_compare_report(&report);
                        exit = 1;
                    }
                    Err(err) => {
                        println!("compared {name}: ERROR: {err}");
                        exit = 1;
                    }
                }
            }
            Ok(exit)
        }
        Command::Delta { name, columns } => {
            let (loaded, manifest_path) = load_manifest_for(&root, &name)?;
            let fixture_name = manifest_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let frames_dir = capture::frames_dir_for(&root, &fixture_name);
            let golden = load_golden_doc(&capture::committed_golden_path(&manifest_path))?;
            let products = run_products(&loaded, &root, RunOutput::Full, None)?;
            let report = compare::delta_products(
                &fixture_name,
                &golden,
                &frames_dir,
                &products.frames,
                &products.metrics_rows,
                &columns,
            )?;
            print_delta_report(&report);
            Ok(0)
        }
        Command::Diff { dir_a, dir_b } => {
            let report = compare::diff_dirs(&dir_a, &dir_b)?;
            print_delta_report(&report);
            Ok(0)
        }
        Command::Run {
            manifest: manifest_path,
            repeat,
            duration_s,
            output,
        } => {
            let loaded = GoldenManifest::load(&manifest_path)?;
            capture::check_features(&loaded, &manifest_path)?;
            let run_output = match output {
                RunTarget::Discard => RunOutput::Discard,
                RunTarget::InMemory | RunTarget::WriteTo(_) => RunOutput::Full,
            };
            for repetition in 1..=repeat {
                let started = Instant::now();
                let products = run_products(&loaded, &root, run_output, duration_s)?;
                let wall = started.elapsed();
                print_run_repetition(repetition, &products, wall.as_secs_f64(), output.clone())?;
            }
            Ok(0)
        }
    }
}

/// Runs and compares one manifest for compare-all: setup failures (manifest
/// load, golden read, run) surface through the same report path as
/// comparison failures.
fn compare_all_one(path: &Path, name: &str, root: &Path) -> FrameGoldenResult<CompareReport> {
    let loaded = GoldenManifest::load(path)?;
    let golden = load_golden_doc(&capture::committed_golden_path(path))?;
    let products = run_products(&loaded, root, RunOutput::Full, None)?;
    let frames_dir = capture::frames_dir_for(root, name);
    compare::compare_products(
        name,
        &golden,
        &products.frames,
        &products.metrics_rows,
        products.health.as_ref(),
        &None,
        Some(&frames_dir),
    )
}

fn run_products(
    loaded: &GoldenManifest,
    root: &Path,
    output: RunOutput,
    duration_s: Option<i64>,
) -> FrameGoldenResult<RunProducts> {
    crate::adapter::run(RunRequest {
        manifest: loaded,
        repo_root: root,
        output,
        duration_override_s: duration_s,
    })
}

fn list_frames(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return paths;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            paths.extend(list_frames(&path));
        } else {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

fn print_compare_report(report: &CompareReport) {
    if report.is_identical() {
        println!("{}: identical", report.name);
        return;
    }
    println!(
        "{}: {} difference(s)",
        report.name,
        report.differences.len()
    );
    for difference in &report.differences {
        match difference {
            Difference::MissingProduct { product } => {
                println!("  product {product}: missing from the fresh run");
            }
            Difference::ExtraProduct { product } => {
                println!("  product {product}: absent from the committed golden");
            }
            Difference::ProductSchema { product, detail } => {
                println!("  product {product}: {detail}");
            }
            Difference::RowCount {
                product,
                expected,
                actual,
            } => {
                println!("  product {product}: row count {actual}, expected {expected}");
            }
            Difference::ColumnDigest {
                product,
                column,
                block,
            } => {
                println!(
                    "  product {product}: column {column:?}: digest differs, first differing block {block} (rows {}..{})",
                    block * crate::digest::BLOCK_ROWS,
                    (block + 1) * crate::digest::BLOCK_ROWS
                );
            }
            Difference::MetricsRowCount { expected, actual } => {
                println!("  metrics: row count {actual}, expected {expected}");
            }
            Difference::MetricsField {
                field,
                expected,
                actual,
            } => {
                println!("  metrics: {field}: expected {expected}, actual {actual}");
            }
            Difference::Health { expected, actual } => {
                println!("  health: expected {expected}, actual {actual}");
            }
        }
    }
    if let Some(mismatch) = &report.row_mismatch {
        println!(
            "  first differing row: {} row {}: expected {}, actual {}",
            mismatch.column, mismatch.row, mismatch.expected, mismatch.actual
        );
        for (column, cells) in &mismatch.per_column_cells {
            if *cells > 0 {
                println!("  column {column}: {cells} differing cell(s)");
            }
        }
    }
    for note in &report.notes {
        println!("  note: {note}");
    }
}

fn print_delta_report(report: &DeltaReport) {
    println!("products compared: {:?}", report.products_compared);
    if report.columns.is_empty() && report.metrics_changes.is_empty() {
        println!("no differences");
    }
    for column in &report.columns {
        println!(
            "  column {:?}: {} differing cell(s), {} validity change(s), max abs diff {:?}, max rel diff {:?}",
            column.column,
            column.differing_cells,
            column.validity_changes,
            column.max_abs_difference,
            column.max_relative_difference
        );
    }
    for (field, expected, actual) in &report.metrics_changes {
        println!("  metrics {field}: {expected} -> {actual}");
    }
    for note in &report.notes {
        println!("  note: {note}");
    }
}

/// Reads VmRSS in kB from /proc/self/status; unavailable off Linux.
fn vm_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let value = rest.trim().trim_end_matches(" kB").trim();
            return value.parse().ok();
        }
    }
    None
}

fn frame_memory_bytes(products: &RunProducts) -> u64 {
    let mut total = 0u64;
    for frame in products.frames.values() {
        for batch in &frame.batches {
            for column in batch.columns() {
                total += column.get_array_memory_size() as u64;
            }
        }
    }
    total
}

fn print_run_repetition(
    repetition: usize,
    products: &RunProducts,
    wall_seconds: f64,
    target: RunTarget,
) -> FrameGoldenResult<()> {
    let rss = match vm_rss_kb() {
        Some(kb) => format!("{kb} kB"),
        None => "unavailable".to_string(),
    };
    println!(
        "rep {repetition}: construct+simulate {wall_seconds:.3} s (construct {:.3} s, simulate {:.3} s), vm_rss {rss}, frame_bytes {}",
        products.timing.construct.as_secs_f64(),
        products.timing.simulate.as_secs_f64(),
        frame_memory_bytes(products)
    );
    #[cfg(feature = "profiling")]
    if let Some(summary) = &products.profiling {
        let total_secs = summary.step_total.as_secs_f64();
        if total_secs > 0.0 {
            let pct = |d: std::time::Duration| d.as_secs_f64() * 100.0 / total_secs;
            println!(
                "rep {repetition} profiling: step_total {:.3} s, environment {:.0}%, control {:.0}%, ideal_capacity {:.0}%, actors {:.0}%, dispatch {:.0}%, equipment {:.0}%, envelope {:.0}%, invariants {:.0}%, state_snapshot {:.0}%, output {:.0}%, accounting {:.0}%, memory_high_water_kb {}, hot_path_alloc_violations {}",
                total_secs,
                pct(summary.environment),
                pct(summary.control),
                pct(summary.ideal_capacity),
                pct(summary.actors),
                pct(summary.dispatch),
                pct(summary.equipment),
                pct(summary.envelope),
                pct(summary.invariants),
                pct(summary.state_snapshot),
                pct(summary.output),
                pct(summary.accounting),
                summary.memory_high_water_kb,
                summary.hot_path_alloc_violations
            );
        }
    }
    let _ = std::io::stdout().flush();
    match target {
        RunTarget::WriteTo(dir) => {
            for (name, frame) in &products.frames {
                let path = dir.join(format!("{name}.parquet"));
                crate::frames::write_frame(&path, &frame.batches)?;
                println!("wrote {}", path.display());
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
