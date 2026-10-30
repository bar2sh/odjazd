//! Terminal output. `anstream` drops the colours when the output is piped or `NO_COLOR` is set.

use anstream::println;
use anstyle::{Color, RgbColor, Style};
use jiff::Zoned;
use odjazd_core::{Departure, Feed, StopGroup};

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();

/// Longer headsigns are cut short with "…".
const HEADSIGN_WIDTH: usize = 28;

pub fn board(group: &StopGroup, now: &Zoned, departures: &[Departure], others: &[&StopGroup]) {
    println!(
        "{BOLD}{}{BOLD:#}  {DIM}{}{DIM:#}",
        group.name,
        now.strftime("%a %-d %b, %H:%M")
    );
    println!();
    if departures.is_empty() {
        println!("No departures found. If that's unexpected, run `odjazd update`.");
    }

    let line_width = departures
        .iter()
        .map(|d| d.route.name.chars().count())
        .max()
        .unwrap_or(0);
    let headsign_width = departures
        .iter()
        .map(|d| d.headsign.chars().count().min(HEADSIGN_WIDTH))
        .max()
        .unwrap_or(0);
    for departure in departures {
        let badge = line_style(departure);
        let line = &departure.route.name;
        let headsign = truncate(&departure.headsign, HEADSIGN_WIDTH);
        let code = departure.stop.code.as_deref().unwrap_or("");
        let approximate = if departure.approximate { "~" } else { "" };
        let time = format!("{approximate}{}", departure.time.strftime("%H:%M"));
        let countdown = countdown(now, &departure.time);
        println!(
            "{badge} {line:<line_width$} {badge:#}  {headsign:<headsign_width$}  {DIM}{code:>3}{DIM:#}  {time:>6}  {BOLD}{countdown}{BOLD:#}"
        );
    }

    if !others.is_empty() {
        let names: Vec<&str> = others.iter().map(|group| group.name.as_str()).collect();
        println!();
        println!(
            "{DIM}Not this one? Also matching: {}{DIM:#}",
            names.join(", ")
        );
    }
}

pub fn stops(feed: &Feed, groups: &[&StopGroup]) {
    let width = groups
        .iter()
        .map(|group| group.name.chars().count())
        .max()
        .unwrap_or(0);
    for group in groups {
        let codes: Vec<&str> = group
            .stops
            .iter()
            .filter_map(|&stop| feed.stops()[stop].code.as_deref())
            .collect();
        println!(
            "{BOLD}{:<width$}{BOLD:#}  {DIM}{}{DIM:#}",
            group.name,
            codes.join(" ")
        );
    }
}

/// The line's own colours (red trams, purple buses…), or inverted text without them.
fn line_style(departure: &Departure) -> Style {
    let rgb = |[r, g, b]: [u8; 3]| Some(Color::Rgb(RgbColor(r, g, b)));
    let route = departure.route;
    match route.color {
        Some(color) => Style::new()
            .bold()
            .bg_color(rgb(color))
            .fg_color(rgb(route.text_color.unwrap_or([255, 255, 255]))),
        None => Style::new().bold().invert(),
    }
}

/// "now", "5 min", or nothing for departures an hour or more away.
fn countdown(now: &Zoned, time: &Zoned) -> String {
    match time.duration_since(now).as_mins() {
        0 => "now".to_owned(),
        minutes @ 1..60 => format!("{minutes} min"),
        _ => String::new(),
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(width - 1).collect();
    cut.push('…');
    cut
}
