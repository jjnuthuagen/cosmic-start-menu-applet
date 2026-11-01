//! Type-to-search ranking. Pure: the same query over the same apps always
//! gives the same order, which is what makes Enter-launches-top-hit safe.

use crate::apps::App;

/// Lower is better. `None` means no match.
///
/// Every whitespace-separated word in `q` must match somewhere and the app
/// scores its worst-matching word, so "office libre" finds LibreOffice and
/// "fire fox" finds Firefox — neither is a prefix, substring or keyword as a
/// whole — while a query with a word an app has nowhere still excludes it.
fn score(app: &App, q: &str) -> Option<u8> {
    q.split_whitespace()
        .map(|word| score_word(app, word))
        .try_fold(0u8, |worst, hit| hit.map(|h| worst.max(h)))
}

/// One word: 0 starts the name, 1 starts one of its words, 2 is anywhere in
/// it, 3 is in the generic name or the keywords.
fn score_word(app: &App, q: &str) -> Option<u8> {
    let name = app.name.to_lowercase();
    if name.starts_with(q) {
        return Some(0);
    }
    if name
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w.starts_with(q))
    {
        return Some(1);
    }
    if name.contains(q) {
        return Some(2);
    }
    app.generic_name
        .iter()
        .chain(app.keywords.iter())
        .any(|s| s.to_lowercase().contains(q))
        .then_some(3)
}

/// Indices into `apps`, best first. A blank query matches nothing.
pub fn rank(apps: &[App], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u8, usize)> = apps
        .iter()
        .enumerate()
        .filter_map(|(i, a)| score(a, &q).map(|s| (s, i)))
        .collect();
    // `apps` is already A–Z, so a stable sort on score keeps ties alphabetical.
    hits.sort_by_key(|(s, _)| *s);
    hits.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::App;

    fn app(name: &str, generic: Option<&str>, kw: &[&str]) -> App {
        App {
            name: name.into(),
            generic_name: generic.map(Into::into),
            keywords: kw.iter().map(|s| s.to_string()).collect(),
            ..App::default()
        }
    }

    fn names(apps: &[App], q: &str) -> Vec<String> {
        rank(apps, q)
            .into_iter()
            .map(|i| apps[i].name.clone())
            .collect()
    }

    #[test]
    fn prefix_beats_word_start_beats_substring_beats_keyword() {
        let apps = vec![
            app("Terminal", None, &[]),
            app("GNOME Terminal", None, &[]),
            app("Sterling", None, &[]),
            app("Console", None, &["terminal"]),
        ];
        assert_eq!(
            names(&apps, "ter"),
            ["Terminal", "GNOME Terminal", "Sterling", "Console"]
        );
    }

    #[test]
    fn case_is_ignored() {
        let apps = vec![app("Firefox", None, &[])];
        assert_eq!(names(&apps, "FIREfox"), ["Firefox"]);
    }

    #[test]
    fn blank_query_matches_nothing() {
        let apps = vec![app("Firefox", None, &[])];
        assert!(rank(&apps, "   ").is_empty());
        assert!(rank(&apps, "").is_empty());
    }

    #[test]
    fn generic_name_matches_like_a_keyword() {
        let apps = vec![app("Firefox", Some("Web Browser"), &[])];
        assert_eq!(names(&apps, "browser"), ["Firefox"]);
    }

    #[test]
    fn ties_keep_alphabetical_order() {
        let apps = vec![app("Files", None, &[]), app("Firefox", None, &[])];
        assert_eq!(names(&apps, "fi"), ["Files", "Firefox"]);
    }

    #[test]
    fn no_match_is_empty() {
        assert!(rank(&[app("Files", None, &[])], "zzz").is_empty());
    }

    #[test]
    fn words_match_in_any_order_and_anywhere_in_the_name() {
        let apps = vec![app("LibreOffice", None, &[]), app("Firefox", None, &[])];
        // Neither whole query is a prefix, substring or keyword of the name,
        // but every word is present, so the app is found.
        assert_eq!(names(&apps, "office libre"), ["LibreOffice"]);
        assert_eq!(names(&apps, "fire fox"), ["Firefox"]);
        assert_eq!(names(&apps, "fox fire"), ["Firefox"]);
    }

    #[test]
    fn every_word_must_match() {
        let apps = vec![app("Firefox", None, &[])];
        assert!(rank(&apps, "fire zzz").is_empty());
        assert!(rank(&apps, "zzz fire").is_empty());
    }
}
