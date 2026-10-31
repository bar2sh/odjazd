use std::collections::HashMap;

use crate::{Feed, LatLon, Stop, StopIdx, Timetable};

/// Stops with the same name further apart than this are different places: there is a
/// "Centrum" in Warsaw and another one in Łomianki.
const SAME_PLACE_M: f64 = 2_000.0;

/// All the poles and platforms riders think of as one stop, e.g. the twelve poles of
/// "Pl. Narutowicza".
#[derive(Debug, Clone)]
pub struct StopGroup {
    /// The name to show. Includes the town when another group has the same name.
    pub name: String,
    pub stops: Vec<StopIdx>,
    words: Vec<String>,
    /// Where the first pole stands.
    position: LatLon,
}

impl Feed {
    /// Groups stops by name. The "Metro Politechnika" bus and tram stops and the
    /// "Politechnika" metro platforms below them end up in one group.
    pub fn stop_groups(&self) -> Vec<StopGroup> {
        group_stops(self.stops())
    }
}

impl Timetable {
    /// Groups stops by name, as [`Feed::stop_groups`] does.
    pub fn stop_groups(&self) -> Vec<StopGroup> {
        group_stops(self.stops())
    }
}

fn group_stops(stops: &[Stop]) -> Vec<StopGroup> {
    let mut groups: Vec<StopGroup> = Vec::new();
    let mut by_name: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
    for (i, stop) in stops.iter().enumerate() {
        let words = place_words(&stop.name);
        let same_name = by_name.entry(words.clone()).or_default();
        let nearby = same_name
            .iter()
            .find(|&&g| groups[g].position.distance(stop.position()) < SAME_PLACE_M);
        match nearby {
            Some(&g) => groups[g].stops.push(i),
            None => {
                same_name.push(groups.len());
                groups.push(StopGroup {
                    name: stop.name.clone(),
                    stops: vec![i],
                    words,
                    position: stop.position(),
                });
            }
        }
    }

    let mut name_count: HashMap<String, usize> = HashMap::new();
    for group in &groups {
        *name_count.entry(group.name.clone()).or_default() += 1;
    }
    for group in &mut groups {
        if name_count[&group.name] > 1
            && let Some(town) = &stops[group.stops[0]].town
        {
            group.name = format!("{} ({town})", group.name);
        }
    }
    groups
}

/// Stop groups matching `query`, best first. Typos, missing Polish letters and spelled-out
/// abbreviations ("plac" for "Pl.") are fine.
pub fn search<'g>(groups: &'g [StopGroup], query: &str) -> Vec<&'g StopGroup> {
    let query = place_words(query);
    if query.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<_> = groups
        .iter()
        .filter_map(|group| Some((score(&query, &group.words)?, group)))
        .collect();
    found.sort_by(|(a_score, a), (b_score, b)| {
        b_score.total_cmp(a_score).then_with(|| a.name.cmp(&b.name))
    });
    found.into_iter().map(|(_, group)| group).collect()
}

/// A stop name reduced to the words that matter for comparing it: lowercase, without
/// Polish letters or punctuation, with common words shortened the way Warsaw stop names
/// abbreviate them. Both "Plac Narutowicza" and "Pl. Narutowicza" become `pl narutowicza`.
pub fn normalize(name: &str) -> String {
    words(name).collect::<Vec<_>>().join(" ")
}

fn words(name: &str) -> impl Iterator<Item = String> + '_ {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| {
            let word: String = word
                .chars()
                .flat_map(char::to_lowercase)
                .map(without_diacritics)
                .collect();
            abbreviation(&word).map_or(word, str::to_owned)
        })
}

/// [`words`] minus a leading "metro", so that the "Metro Politechnika" stops and the
/// "Politechnika" metro station count as the same place.
fn place_words(name: &str) -> Vec<String> {
    let mut words: Vec<String> = words(name).collect();
    if words.len() > 1 && words[0] == "metro" {
        words.remove(0);
    }
    words
}

fn without_diacritics(c: char) -> char {
    match c {
        'ą' => 'a',
        'ć' => 'c',
        'ę' => 'e',
        'ł' => 'l',
        'ń' => 'n',
        'ó' => 'o',
        'ś' => 's',
        'ź' | 'ż' => 'z',
        c => c,
    }
}

/// The abbreviation Warsaw stop names use for a spelled-out word.
fn abbreviation(word: &str) -> Option<&'static str> {
    Some(match word {
        "plac" | "placu" => "pl",
        "aleja" | "aleje" | "alei" | "alej" => "al",
        "osiedle" | "osiedla" => "os",
        "dworzec" | "dworca" => "dw",
        "cmentarz" | "cmentarza" => "cm",
        "ksiadz" | "ksiedza" => "ks",
        "swiety" | "swieta" | "swiete" | "swietego" | "swietej" | "swietych" => "sw",
        "general" | "generala" => "gen",
        "polnoc" | "polnocny" | "polnocna" | "polnocne" => "pln",
        "poludnie" | "poludniowy" | "poludniowa" | "poludniowe" => "pld",
        "zachodni" | "zachodnia" | "zachodnie" => "zach",
        "wschodni" | "wschodnia" | "wschodnie" => "wsch",
        "glowny" | "glowna" | "glowne" => "gl",
        _ => return None,
    })
}

/// How well `query` matches a name, or `None` when one of its words matches nothing.
fn score(query: &[String], name: &[String]) -> Option<f64> {
    let mut total = 0.0;
    for q in query {
        let best = name
            .iter()
            .map(|word| similarity(q, word))
            .fold(0.0, f64::max);
        if best < 0.8 {
            return None;
        }
        total += best;
    }
    // Prefer names without extra words: "Centrum" over "Centrum Handlowe".
    let extra_words = name.len().saturating_sub(query.len()) as f64;
    Some(total / query.len() as f64 - 0.05 * extra_words)
}

fn similarity(query: &str, word: &str) -> f64 {
    if query == word {
        1.0
    } else if word.starts_with(query) {
        // Still typing: "narut".
        0.9
    } else {
        // A typo: "narutowica".
        0.9 * strsim::jaro_winkler(query, word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(name: &str) -> StopGroup {
        StopGroup {
            name: name.to_owned(),
            stops: Vec::new(),
            words: place_words(name),
            position: LatLon::new(0.0, 0.0),
        }
    }

    fn search_names<'g>(groups: &'g [StopGroup], query: &str) -> Vec<&'g str> {
        search(groups, query)
            .into_iter()
            .map(|group| group.name.as_str())
            .collect()
    }

    #[test]
    fn normalizes_letters_punctuation_and_abbreviations() {
        assert_eq!(normalize("Pl. Narutowicza"), "pl narutowicza");
        assert_eq!(normalize("Plac Narutowicza"), "pl narutowicza");
        assert_eq!(normalize("Dworzec Centralny"), "dw centralny");
        assert_eq!(normalize("Świętokrzyska"), "swietokrzyska");
        assert_eq!(
            normalize("Os. Górczewska – Al. Lotników"),
            "os gorczewska al lotnikow"
        );
    }

    #[test]
    fn finds_stops_despite_typos_and_missing_letters() {
        let groups = [
            "Pl. Narutowicza",
            "Narutowicza",
            "Dw. Centralny",
            "Świętokrzyska",
            "Centrum",
        ]
        .map(group);
        assert_eq!(
            search_names(&groups, "plac narutowicza"),
            ["Pl. Narutowicza"]
        );
        assert_eq!(search_names(&groups, "swietokrzyska"), ["Świętokrzyska"]);
        assert_eq!(
            search_names(&groups, "narutowica"),
            ["Narutowicza", "Pl. Narutowicza"]
        );
        assert_eq!(search_names(&groups, "dworzec centr"), ["Dw. Centralny"]);
        assert!(search_names(&groups, "zoo").is_empty());
    }

    #[test]
    fn metro_station_and_metro_stops_are_one_place() {
        assert_eq!(
            place_words("Metro Politechnika"),
            place_words("Politechnika")
        );
        assert_eq!(place_words("Metro"), ["metro"]);
    }
}
