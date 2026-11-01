//! The folders the user already made in COSMIC's App Library, reused as the
//! Start menu's "Folders" view. Read-only: the App Library owns this file.

use serde::Deserialize;

use crate::apps::App;

#[derive(Debug, Clone, Deserialize)]
pub struct RawFolder {
    pub name: String,
    #[serde(default)]
    pub filter: Filter,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub enum Filter {
    AppIds(Vec<String>),
    Categories {
        #[serde(default)]
        categories: Vec<String>,
        #[serde(default)]
        exclude: Vec<String>,
        #[serde(default)]
        include: Vec<String>,
    },
    #[default]
    #[serde(other)]
    Unknown,
}

/// A folder resolved against the installed apps.
#[derive(Debug, Clone)]
pub struct Folder {
    pub name: String,
    /// Indices into the app list, which is already A–Z.
    pub apps: Vec<usize>,
}

/// Tolerant: an unreadable file means no folders, never an error.
pub fn parse(ron_text: &str) -> Vec<RawFolder> {
    // One list, one `from_str`: a filter shape this build does not know takes
    // every folder with it, so say so at warn — the Folders view then looks
    // empty, which is otherwise explained by nothing at all.
    // ponytail: no per-entry tolerance; a hand-rolled RON entry splitter is
    // what it costs, and one unknown filter is a view, not a file, going away.
    ron::from_str(ron_text).unwrap_or_else(|e| {
        tracing::warn!("App Library folders unreadable: {e}");
        Vec::new()
    })
}

fn matches(filter: &Filter, app: &App) -> bool {
    match filter {
        Filter::AppIds(ids) => ids.contains(&app.id),
        Filter::Categories {
            categories,
            exclude,
            include,
        } => {
            include.contains(&app.id)
                || (!exclude.contains(&app.id)
                    && app.categories.iter().any(|c| categories.contains(c)))
        }
        Filter::Unknown => false,
    }
}

/// The folders, plus the apps that are in none of them.
pub fn resolve(raw: &[RawFolder], apps: &[App]) -> (Vec<Folder>, Vec<usize>) {
    let mut used = vec![false; apps.len()];
    let folders = raw
        .iter()
        .map(|f| {
            let idx: Vec<usize> = (0..apps.len())
                .filter(|&i| matches(&f.filter, &apps[i]))
                .collect();
            for &i in &idx {
                used[i] = true;
            }
            Folder {
                name: f.name.clone(),
                apps: idx,
            }
        })
        .collect();
    let loose = (0..apps.len()).filter(|&i| !used[i]).collect();
    (folders, loose)
}

pub fn load(apps: &[App]) -> (Vec<Folder>, Vec<usize>) {
    let text = dirs::config_dir()
        .map(|d| d.join("cosmic/com.system76.CosmicAppLibrary/v1/groups"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    resolve(&parse(&text), apps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::App;

    const GROUPS: &str = r#"[
        (name: "Games", icon: "folder-symbolic", filter: AppIds(["openttd", "0ad", "not-installed"])),
        (name: "Utilities", icon: "folder-symbolic", filter: Categories(
            categories: ["Utility"], exclude: ["calc"], include: ["btop"])),
    ]"#;

    fn app(id: &str, name: &str, cats: &[&str]) -> App {
        App {
            id: id.into(),
            name: name.into(),
            categories: cats.iter().map(|s| s.to_string()).collect(),
            ..App::default()
        }
    }

    #[test]
    fn resolves_id_lists_and_category_filters() {
        let apps = vec![
            app("0ad", "0 A.D.", &["Game"]),
            app("btop", "btop++", &["System"]),
            app("calc", "Calculator", &["Utility"]),
            app("micro", "Micro", &["Utility"]),
            app("openttd", "OpenTTD", &["Game"]),
            app("vim", "Vim", &["TextEditor"]),
        ];
        let (folders, loose) = resolve(&parse(GROUPS), &apps);
        assert_eq!(folders[0].name, "Games");
        assert_eq!(folders[0].apps, [0, 4]);
        assert_eq!(folders[1].apps, [1, 3]);
        assert_eq!(loose, [2, 5]);
    }

    #[test]
    fn a_bad_file_means_no_folders() {
        assert!(parse("not ron at all").is_empty());
        assert!(parse("").is_empty());
    }

    #[test]
    fn reads_the_real_app_library_format() {
        // Verbatim shape of ~/.config/cosmic/com.system76.CosmicAppLibrary/v1/groups.
        let real = r#"[
    (
        name: "Design",
        icon: "folder-symbolic",
        filter: AppIds([
            "blender",
        ]),
    ),
]"#;
        assert_eq!(parse(real).len(), 1);
    }
}
