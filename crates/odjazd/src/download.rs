use std::{
    fs::{self, File},
    io::{self, IsTerminal, Read},
    path::Path,
};

use anyhow::{Context, Result};

/// Downloads the GTFS archive at `url` and unpacks it into `feed_dir`. The previous
/// timetable is replaced only once the new one is unpacked and opens fine.
pub fn update(url: &str, feed_dir: &Path) -> Result<()> {
    let data_dir = feed_dir
        .parent()
        .context("the timetable directory has no parent")?;
    fs::create_dir_all(data_dir)
        .with_context(|| format!("cannot create {}", data_dir.display()))?;

    let archive = data_dir.join("feed.zip.part");
    download(url, &archive)?;
    eprintln!("Unpacking…");
    let installed =
        odjazd_core::install(&archive, feed_dir).context("the download is not a usable GTFS feed");
    fs::remove_file(&archive)?;
    installed
}

fn download(url: &str, to: &Path) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(crate::USER_AGENT)
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("cannot download {url}"))?;
    let total = response
        .body()
        .content_length()
        .filter(|_| io::stderr().is_terminal());
    let mut body = Progress {
        inner: response.body_mut().as_reader(),
        read: 0,
        total,
        percent: None,
    };
    let mut file = File::create(to).with_context(|| format!("cannot create {}", to.display()))?;
    eprint!("Downloading timetable…");
    io::copy(&mut body, &mut file).with_context(|| format!("download from {url} broke off"))?;
    eprintln!(" done ({:.0} MB)", body.read as f64 / 1e6);
    Ok(())
}

/// Counts bytes as they are read and, when `total` is known, shows the percentage done.
struct Progress<R> {
    inner: R,
    read: u64,
    total: Option<u64>,
    percent: Option<u64>,
}

impl<R: Read> Read for Progress<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        if let Some(total) = self.total.filter(|&total| total > 0) {
            let percent = self.read * 100 / total;
            if self.percent != Some(percent) {
                self.percent = Some(percent);
                eprint!("\rDownloading timetable… {percent}%");
            }
        }
        Ok(n)
    }
}
