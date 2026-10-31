//! Build tasks for this repository, run as `cargo xtask <task>` (the
//! [xtask](https://github.com/matklad/cargo-xtask) convention).

use std::{
    env, fs,
    io::BufRead,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use jiff::{Zoned, civil::Date};
use odjazd_core::{Feed, Timetable};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Builds the map page into target/site: odjazd-wasm compiled to WebAssembly, the
    /// files in site/, and the timetable compressed for the browser.
    ///
    /// Needs the wasm32-unknown-unknown target and the wasm-bindgen CLI of the version in
    /// Cargo.lock. Serve the result with e.g. `python3 -m http.server -d target/site`.
    Site {
        /// An unpacked GTFS feed or its zip. By default the one `odjazd update` downloaded.
        #[arg(long)]
        feed: Option<PathBuf>,
        /// The day to load, with the day before and after it. By default today, in the
        /// feed's time zone.
        #[arg(long)]
        day: Option<Date>,
        /// Where to put the page. By default target/site.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().task {
        Task::Site { feed, day, out } => site(feed, day, out),
    }
}

fn site(feed: Option<PathBuf>, day: Option<Date>, out: Option<PathBuf>) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .to_owned();
    let out = out.unwrap_or_else(|| root.join("target/site"));

    let wasm = build_wasm(&root)?;
    let feed_dir = feed_dir(feed, &wasm)?;
    let data = timetable(&feed_dir, day)?;

    if out.exists() {
        fs::remove_dir_all(&out).with_context(|| format!("cannot clear {}", out.display()))?;
    }
    let pkg = out.join("pkg");
    fs::create_dir_all(&pkg).with_context(|| format!("cannot create {}", pkg.display()))?;
    wasm_bindgen(&root, &wasm, &pkg)?;
    fs::write(out.join("timetable.br"), &data)?;

    // Pages are cached for a while, so every link carries a hash of what it points to:
    // after a deploy, a cached page never mixes old code with new.
    let mut code = Fnv::default();
    for file in [pkg.join("odjazd_wasm.js"), pkg.join("odjazd_wasm_bg.wasm")] {
        code.write(&fs::read(file)?);
    }
    let site = root.join("site");
    let mut files: Vec<PathBuf> = fs::read_dir(&site)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    files.sort();
    for file in &files {
        code.write(&fs::read(file)?);
    }
    let (code, data_hash) = (code.short(), Fnv::of(&data).short());
    for file in &files {
        let to = out.join(file.file_name().expect("a file name"));
        if matches!(extension(file), "html" | "js" | "css") {
            let text = fs::read_to_string(file)?
                .replace("__CODE__", &code)
                .replace("__DATA__", &data_hash);
            fs::write(to, text)?;
        } else {
            fs::copy(file, to)?;
        }
    }

    let size = |path: PathBuf| fs::metadata(path).map(|m| m.len() as f64 / 1e6);
    eprintln!(
        "Built {}: odjazd_wasm_bg.wasm {:.2} MB, timetable.br {:.2} MB",
        out.display(),
        size(pkg.join("odjazd_wasm_bg.wasm"))?,
        size(out.join("timetable.br"))?,
    );
    Ok(())
}

/// Builds odjazd-wasm and returns the `.wasm` file cargo wrote.
fn build_wasm(root: &Path) -> Result<PathBuf> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut child = Command::new(cargo)
        .current_dir(root)
        .args(["build", "--locked", "--package", "odjazd-wasm", "--lib"])
        .args(["--target", "wasm32-unknown-unknown", "--profile", "wasm"])
        .arg("--message-format=json-render-diagnostics")
        .stdout(Stdio::piped())
        .spawn()
        .context("cannot run cargo")?;
    let mut wasm = None;
    let messages = std::io::BufReader::new(child.stdout.take().expect("piped stdout"));
    for line in messages.lines() {
        let message: serde_json::Value = serde_json::from_str(&line?)?;
        if message["reason"] == "compiler-artifact"
            && message["target"]["name"] == "odjazd_wasm"
            && let Some(files) = message["filenames"].as_array()
        {
            wasm = files
                .iter()
                .filter_map(|file| file.as_str())
                .find(|file| file.ends_with(".wasm"))
                .map(PathBuf::from);
        }
    }
    ensure!(
        child.wait()?.success(),
        "cannot build odjazd-wasm; is the target installed? \
         rustup target add wasm32-unknown-unknown"
    );
    wasm.context("cargo built no .wasm file")
}

/// Generates the JavaScript that loads `wasm` into `pkg`, with the wasm-bindgen CLI of
/// the same version as the wasm-bindgen crate.
fn wasm_bindgen(root: &Path, wasm: &Path, pkg: &Path) -> Result<()> {
    let lock = fs::read_to_string(root.join("Cargo.lock"))?;
    let wanted = lock
        .split("[[package]]")
        .find(|package| package.contains("\nname = \"wasm-bindgen\"\n"))
        .and_then(|package| package.split("\nversion = \"").nth(1))
        .and_then(|rest| rest.split('"').next())
        .context("no wasm-bindgen in Cargo.lock")?;
    let found = Command::new("wasm-bindgen").arg("--version").output();
    let found = match &found {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout),
        _ => "no wasm-bindgen".into(),
    };
    if found.trim() != format!("wasm-bindgen {wanted}") {
        bail!(
            "need wasm-bindgen {wanted}, found {}: \
             cargo install wasm-bindgen-cli --version {wanted} --locked",
            found.trim()
        );
    }
    let status = Command::new("wasm-bindgen")
        .args(["--target", "web", "--no-typescript", "--out-dir"])
        .args([pkg, wasm])
        .status()?;
    ensure!(status.success(), "wasm-bindgen failed");
    Ok(())
}

/// The unpacked feed to build from: `feed` itself, `feed` unpacked when it is a zip, or
/// the one the odjazd CLI keeps.
fn feed_dir(feed: Option<PathBuf>, wasm: &Path) -> Result<PathBuf> {
    match feed {
        Some(zip) if extension(&zip) == "zip" => {
            // Beside the build, e.g. target/wasm32-unknown-unknown/wasm/site-feed.
            let dir = wasm.with_file_name("site-feed");
            odjazd_core::install(&zip, &dir)
                .with_context(|| format!("cannot unpack {}", zip.display()))?;
            Ok(dir)
        }
        Some(dir) => Ok(dir),
        None => {
            let data = match env::var_os("ODJAZD_DATA_DIR") {
                Some(dir) => PathBuf::from(dir),
                None => dirs::cache_dir()
                    .context("no cache directory")?
                    .join("odjazd"),
            };
            Ok(data.join("feed"))
        }
    }
}

/// The timetable of `day` (or today) from the feed in `dir`, saved and compressed.
fn timetable(dir: &Path, day: Option<Date>) -> Result<Vec<u8>> {
    let feed = Feed::open(dir).with_context(|| {
        format!(
            "cannot read the timetable in {}; download one with `odjazd update` or pass --feed",
            dir.display()
        )
    })?;
    let day = day.unwrap_or_else(|| Zoned::now().with_time_zone(feed.timezone().clone()).date());
    let start = Instant::now();
    let bytes = Timetable::build(&feed, day)?.to_bytes()?;
    let built = start.elapsed();

    let start = Instant::now();
    let mut compressed = Vec::new();
    let params = brotli::enc::BrotliEncoderParams {
        quality: 11,
        lgwin: 24,
        size_hint: bytes.len(),
        ..Default::default()
    };
    brotli::BrotliCompress(&mut &bytes[..], &mut compressed, &params)?;
    eprintln!(
        "Timetable around {day} (feed {}): {:.1} MB built in {built:.1?}, \
         {:.2} MB compressed in {:.1?}",
        feed.version().unwrap_or("without a version"),
        bytes.len() as f64 / 1e6,
        compressed.len() as f64 / 1e6,
        start.elapsed()
    );
    Ok(compressed)
}

fn extension(path: &Path) -> &str {
    path.extension().and_then(|ext| ext.to_str()).unwrap_or("")
}

/// FNV-1a, for short content hashes.
#[derive(Clone, Copy)]
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn of(bytes: &[u8]) -> Self {
        let mut hash = Self::default();
        hash.write(bytes);
        hash
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn short(self) -> String {
        format!("{:08x}", self.0 >> 32)
    }
}
