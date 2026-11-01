//! Middle column: an optional "Most used" block, then every app under its
//! letter header.

use cosmic::desktop::IconSourceExt;
use std::collections::HashSet;

use cosmic::iced::{Alignment, Length, Point};
use cosmic::widget::{
    button, column, container, icon, mouse_area, popover, row, scrollable, text, Space,
};
use cosmic::Element;

use crate::app::{Message, Target};
use crate::apps::{self, App};
use crate::config::ListMode;
use crate::fl;
use crate::folders::Folder;
use crate::launcher::{Item, Section};
use crate::ui::{
    header_text, menu_card, muted_text, quiet_button, row_radius, selected_button, thin_scroll,
    HEADER_BAND, HEADER_HEIGHT, ICON, LIST_WIDTH, ROW_GUTTER, ROW_HEIGHT, SCROLL_GUTTER,
    ZONE_LABEL_HEIGHT,
};

/// A fixed-height button lays its content out from the top; this centres it
/// vertically so icon and text sit on the row's midline.
fn centred<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .height(Length::Fill)
        .align_y(Alignment::Center)
        .into()
}

/// The icon column: a fixed `ICON`-square box every row's glyph is drawn
/// into, so a 16 px source and a 512 px one take exactly the same width and
/// every label in the column starts on the same x.
fn icon_box<'a>(glyph: Element<'a, Message>) -> Element<'a, Message> {
    container(glyph)
        .center(Length::Fixed(f32::from(ICON)))
        .clip(true)
        .into()
}

pub fn app_row<'a>(app: &'a App, index: usize, selected: bool) -> Element<'a, Message> {
    let body = row::with_children(vec![
        icon_box(icon(app.icon.as_cosmic_icon()).size(ICON).into()),
        text::body(&app.name)
            .wrapping(cosmic::iced::widget::text::Wrapping::None)
            .into(),
    ])
    .spacing(12)
    .align_y(Alignment::Center);
    let body = button::custom(centred(body))
        .class(if selected {
            selected_button()
        } else {
            quiet_button()
        })
        .padding([0, ROW_GUTTER])
        .width(Length::Fill)
        .height(Length::Fixed(ROW_HEIGHT))
        .on_press(Message::Launch(index));
    mouse_area(body)
        .on_right_press(Message::OpenContext(Target::App(index)))
        .into()
}

/// A label over a pinned block ("Most used", "Recent"): quieter and
/// smaller than a letter header, so a semantic group and an alphabet marker
/// never read the same. 8 px of air above it, 4 below.
pub fn section_label<'a>(label: String) -> Element<'a, Message> {
    container(header_text(label).class(cosmic::theme::Text::Custom(muted_text)))
        .padding([6, ROW_GUTTER, 4, ROW_GUTTER])
        .height(Length::Fixed(ZONE_LABEL_HEIGHT))
        .align_y(Alignment::Center)
        .into()
}

/// The band a section or letter header's text sits in: 26 px tall with 16
/// above and 4 below, which is what makes it read as the start of a group.
fn header_band<'a>(label: Element<'a, Message>, msg: Message) -> Element<'a, Message> {
    container(
        button::custom(
            container(label)
                .height(Length::Fill)
                .align_y(Alignment::Center),
        )
        .class(quiet_button())
        .padding([0, ROW_GUTTER])
        .width(Length::Fill)
        .height(Length::Fixed(HEADER_BAND))
        .on_press(msg),
    )
    .padding([8, 0, 2, 0])
    .height(Length::Fixed(HEADER_HEIGHT))
    .into()
}

/// A letter header: the uppercase letter at 13 px semibold and 60 % of the
/// foreground, in the same left gutter as the icon column, so the alphabet
/// marks the column without competing with the app names.
pub fn letter_header<'a>(letter: char) -> Element<'a, Message> {
    header_band(
        header_text(letter.to_uppercase().to_string())
            .class(cosmic::theme::Text::Custom(muted_text))
            .into(),
        Message::LetterGrid(true),
    )
}

/// How far down the list `target`'s header sits. Rows and headers have fixed
/// heights, so this is exact rather than estimated.
pub fn offset_of<L: PartialEq>(
    sections: &[(L, Vec<usize>)],
    most_used_rows: usize,
    target: &L,
    row_h: f32,
    header_h: f32,
) -> f32 {
    let mut y = if most_used_rows > 0 {
        header_h + most_used_rows as f32 * row_h
    } else {
        0.0
    };
    for (label, rows) in sections {
        if label == target {
            return y;
        }
        y += header_h + rows.len() as f32 * row_h;
    }
    y
}

/// Tapping a letter header swaps the list for this grid of letters; tapping
/// one jumps there. Letters with no apps are shown but disabled, as Windows
/// does, so the grid keeps its shape.
pub fn letter_grid<'a>(present: &[char]) -> Element<'a, Message> {
    let mut letters: Vec<char> = std::iter::once('#').chain('A'..='Z').collect();
    let extra: Vec<char> = present
        .iter()
        .copied()
        .filter(|c| !letters.contains(c))
        .collect();
    letters.extend(extra);
    let rows = letters.chunks(4).map(|chunk| {
        let mut r = row::with_capacity(4).spacing(6);
        for &c in chunk {
            let b = button::custom(container(text::title4(c.to_string())).center(Length::Fill))
                .class(quiet_button())
                .padding(0)
                .width(Length::Fill)
                .height(Length::Fixed(48.0));
            r = r.push(if present.contains(&c) {
                b.on_press(Message::JumpTo(c))
            } else {
                b
            });
        }
        for _ in chunk.len()..4 {
            r = r.push(cosmic::widget::Space::new().width(Length::Fill));
        }
        r.into()
    });
    thin_scroll(scrollable(
        column::with_children(rows.collect::<Vec<_>>()).spacing(6),
    ))
    .width(Length::Fixed(LIST_WIDTH))
    .height(Length::Fill)
    .into()
}

/// Everything the middle column needs, borrowed from the app state.
pub struct ListView<'a> {
    pub apps: &'a [App],
    pub most_used: &'a [String],
    /// Recently launched, newest first: the second pinned block.
    pub recent: &'a [String],
    pub show_most_used: bool,
    pub mode: ListMode,
    pub folders: &'a [Folder],
    /// Apps in no folder, for the Folders view.
    pub loose: &'a [usize],
    pub open_folders: &'a HashSet<usize>,
    pub list_id: cosmic::widget::Id,
}

fn mode_key(mode: ListMode) -> &'static str {
    match mode {
        ListMode::Az => "mode-az",
        ListMode::Category => "mode-category",
        ListMode::Folders => "mode-folders",
    }
}

/// "All apps" and the switch between A–Z, Category and Folders. Locked, the
/// switch is not drawn at all: the sort only changes from Settings.
pub fn list_bar<'a>(mode: ListMode, menu_open: bool, locked: bool) -> Element<'a, Message> {
    if locked {
        return row::with_children(vec![container(text::caption(fl!("all-apps")))
            .padding([0, 10])
            .into()])
        .align_y(Alignment::Center)
        .width(Length::Fixed(LIST_WIDTH))
        .into();
    }
    let label = row::with_children(vec![
        text::body(fl!(mode_key(mode))).into(),
        icon::from_name("pan-down-symbolic").size(12).into(),
    ])
    .spacing(4)
    .align_y(Alignment::Center);
    let switch = button::custom(label)
        .class(quiet_button())
        .padding([3, 8])
        .on_press(Message::ModeMenu(!menu_open));
    let mut switch = popover(switch)
        .position(popover::Position::Point(Point::new(0.0, 30.0)))
        .on_close(Message::ModeMenu(false));
    if menu_open {
        let items = [ListMode::Az, ListMode::Category, ListMode::Folders].map(|m| {
            button::custom(
                row::with_children(vec![
                    text::body(fl!(mode_key(m))).into(),
                    Space::new().width(Length::Fill).into(),
                    if m == mode {
                        icon::from_name("object-select-symbolic").size(14).into()
                    } else {
                        Space::new().width(14).into()
                    },
                ])
                .align_y(Alignment::Center),
            )
            .class(quiet_button())
            .padding([7, 10])
            .width(Length::Fill)
            .on_press(Message::SetListMode(m))
            .into()
        });
        switch = switch.popup(
            container(column::with_children(items.into_iter().collect::<Vec<_>>()).spacing(1))
                .padding(6)
                .width(Length::Fixed(170.0))
                .class(menu_card()),
        );
    }
    row::with_children(vec![
        container(text::caption(fl!("all-apps")))
            .padding([0, 10])
            .into(),
        Space::new().width(Length::Fill).into(),
        switch.into(),
    ])
    .align_y(Alignment::Center)
    .width(Length::Fixed(LIST_WIDTH))
    .into()
}

fn category_header<'a>(key: &'static str) -> Element<'a, Message> {
    header_band(
        header_text(fl!(key))
            .class(cosmic::theme::Text::Custom(muted_text))
            .into(),
        Message::LetterGrid(true),
    )
}

/// A folder row: folder glyph on a tinted base, name, count, chevron.
fn folder_row<'a>(
    index: usize,
    folder: &'a Folder,
    open: bool,
    selected: bool,
) -> Element<'a, Message> {
    let base = container(icon::from_name("folder-symbolic").size(14))
        .center(Length::Fixed(f32::from(ICON)))
        .class(cosmic::theme::Container::Custom(Box::new(|theme| {
            let mut tint = theme.cosmic().accent_color();
            tint.alpha = 0.22;
            container::Style {
                background: Some(cosmic::iced::Background::Color(tint.into())),
                border: cosmic::iced::Border {
                    radius: row_radius(theme).into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })));
    let body = row::with_children(vec![
        base.into(),
        text::body(&folder.name).width(Length::Fill).into(),
        text::caption(folder.apps.len().to_string()).into(),
        icon::from_name(if open {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        })
        .size(12)
        .into(),
    ])
    .spacing(12)
    .align_y(Alignment::Center);
    button::custom(centred(body))
        .class(if selected {
            selected_button()
        } else {
            quiet_button()
        })
        .padding([0, ROW_GUTTER])
        .width(Length::Fill)
        .height(Length::Fixed(ROW_HEIGHT))
        .on_press(Message::ToggleFolder(index))
        .into()
}

/// One pinned block of at most five rows. Two stacked blocks pushed the
/// first letter header nearly 500 px down the column, which is most of the
/// visible list: the alphabet has to start on the first screen.
pub const MOST_USED_CAP: usize = 5;

/// The apps the pinned block shows, in the order they are drawn: what he
/// opens most, topped up with what he opened last when there are not five
/// of them yet. Shared with `app.rs`, which measures the block to keep
/// letter jumps exact.
pub fn pinned(
    apps: &[App],
    most_used: &[String],
    recent: &[String],
    show_most_used: bool,
) -> Vec<usize> {
    if !show_most_used {
        return Vec::new();
    }
    let find = |id: &String| apps.iter().position(|a| &a.id == id);
    let mut top: Vec<usize> = most_used
        .iter()
        .filter_map(find)
        .take(MOST_USED_CAP)
        .collect();
    for i in recent.iter().filter_map(find) {
        if top.len() >= MOST_USED_CAP {
            break;
        }
        if !top.contains(&i) {
            top.push(i);
        }
    }
    top
}

/// One line of the middle column, in the order it is drawn. The plan is the
/// single description of the column: the renderer walks it, and so does the
/// keyboard — which is the only way a highlight and a scroll offset can be
/// sure they mean the same row as the one on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A quiet block label ("Most used", "Folders").
    Label(String),
    /// A letter header, which also opens the jump grid.
    Letter(char),
    /// A category header, by l10n key.
    Category(&'static str),
    /// An installed app, by its index in `apps`.
    App(usize),
    Folder {
        index: usize,
        open: bool,
    },
    /// An app inside an open folder: the same row, indented.
    FolderApp(usize),
    /// The Start Menu Settings entry, the last line of the column.
    Settings,
}

/// What Enter does on a line the keyboard can land on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Launch the app at this index.
    App(usize),
    /// Open or close this folder.
    Folder(usize),
    /// Open the Start Menu's own settings.
    Settings,
}

/// A line the keyboard can land on: what it does, and where it sits in the
/// column, so the list can be scrolled to it. Headers and block labels are
/// not stops — the highlight walks rows, not signposts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub act: Act,
    pub y: f32,
    pub h: f32,
}

/// What a line does, or `None` for a header or label.
pub fn act(line: &Line) -> Option<Act> {
    match *line {
        Line::App(i) | Line::FolderApp(i) => Some(Act::App(i)),
        Line::Folder { index, .. } => Some(Act::Folder(index)),
        Line::Settings => Some(Act::Settings),
        Line::Label(_) | Line::Letter(_) | Line::Category(_) => None,
    }
}

/// Every line's drawn height, from the same constants the rows are built
/// with — which is what keeps a letter jump and a keyboard scroll exact.
pub fn line_height(line: &Line) -> f32 {
    match line {
        Line::Label(_) => ZONE_LABEL_HEIGHT,
        Line::Letter(_) | Line::Category(_) => HEADER_HEIGHT,
        _ => ROW_HEIGHT,
    }
}

/// Height of a labelled block at the top of the column: a `Line::Label` and
/// `rows` rows under it.
///
/// `app.rs` measures the Most used and Folders blocks with this, and a label
/// costs `ZONE_LABEL_HEIGHT` both here and in `line_height`, so the offset a
/// letter jump scrolls to is the height the plan actually draws. Recomputing
/// it in `app.rs` with a header's height scrolled the Folders jump 6 px past
/// its header.
pub fn block_height(rows: usize) -> f32 {
    ZONE_LABEL_HEIGHT + rows as f32 * ROW_HEIGHT
}

/// The stops of a plan, in order, each with how far down the column it is.
pub fn stops(lines: &[Line]) -> Vec<Stop> {
    let mut out = Vec::new();
    let mut y = 0.0;
    for line in lines {
        let h = line_height(line);
        if let Some(act) = act(line) {
            out.push(Stop { act, y, h });
        }
        y += h;
    }
    out
}

/// The first stop under `letter`'s header, so a first-letter jump takes the
/// highlight with it rather than leaving it behind off-screen.
pub fn stop_at_letter(lines: &[Line], letter: char) -> Option<usize> {
    let mut n = 0;
    let mut arrived = false;
    for line in lines {
        if arrived && act(line).is_some() {
            return Some(n);
        }
        if *line == Line::Letter(letter) {
            arrived = true;
        }
        if act(line).is_some() {
            n += 1;
        }
    }
    None
}

/// Which lines the column draws, for this view's mode and state.
pub fn plan(v: &ListView<'_>) -> Vec<Line> {
    fn az(out: &mut Vec<Line>, sections: Vec<(char, Vec<usize>)>) {
        for (letter, idxs) in sections {
            out.push(Line::Letter(letter));
            out.extend(idxs.into_iter().map(Line::App));
        }
    }
    let list = v.apps;
    let mut out: Vec<Line> = Vec::with_capacity(list.len() + 40);
    // The pinned zone: the apps he actually opens, above the first letter
    // or category header in every mode.
    let top = pinned(list, v.most_used, v.recent, v.show_most_used);
    if !top.is_empty() {
        out.push(Line::Label(fl!("most-used")));
        out.extend(top.iter().copied().map(Line::App));
    }
    // Every section is drawn without the apps the pinned block already
    // shows, so no app appears twice on the same screen.
    match v.mode {
        ListMode::Az => az(&mut out, apps::sections_excluding(list, &top)),
        ListMode::Category => {
            for (key, idxs) in apps::category_sections_excluding(list, &top) {
                out.push(Line::Category(key));
                out.extend(idxs.into_iter().map(Line::App));
            }
        }
        // No folders found (no App Library file, or an unreadable one) means
        // the plain A–Z list rather than an empty section.
        ListMode::Folders if v.folders.is_empty() => {
            az(&mut out, apps::sections_excluding(list, &top));
        }
        ListMode::Folders => {
            out.push(Line::Label(fl!("folders")));
            for (fi, folder) in v.folders.iter().enumerate() {
                let open = v.open_folders.contains(&fi);
                out.push(Line::Folder { index: fi, open });
                if open {
                    // Not the pinned block's apps either: an app can be both
                    // pinned and filed in a folder, and drawing it twice would
                    // break the one-row-per-app rule every other section keeps.
                    out.extend(
                        folder
                            .apps
                            .iter()
                            .copied()
                            .filter(|i| !top.contains(i))
                            .map(Line::FolderApp),
                    );
                }
            }
            az(&mut out, apps::sections_of_excluding(list, v.loose, &top));
        }
    }
    // Last line, under every section: Settings belongs with the apps rather
    // than pinned beneath them.
    out.push(Line::Settings);
    out
}

/// `focus` is the n'th stop of the plan, highlighted in the same accent
/// tint a keyboard-selected search result wears.
pub fn view<'a>(v: ListView<'a>, focus: Option<usize>) -> Element<'a, Message> {
    let list = v.apps;
    let lines = plan(&v);
    let mut col = column::with_capacity(lines.len()).spacing(0);
    let mut n = 0usize;
    for line in lines {
        let on = act(&line).is_some() && Some(n) == focus;
        if act(&line).is_some() {
            n += 1;
        }
        col = col.push(match line {
            Line::Label(label) => section_label(label),
            Line::Letter(letter) => letter_header(letter),
            Line::Category(key) => category_header(key),
            Line::App(i) => app_row(&list[i], i, on),
            Line::FolderApp(i) => container(app_row(&list[i], i, on))
                .padding([0, 0, 0, 16])
                .into(),
            Line::Folder { index, open } => folder_row(index, &v.folders[index], open, on),
            Line::Settings => settings_row(on),
        });
    }
    thin_scroll(scrollable(container(col).padding([
        0,
        SCROLL_GUTTER,
        12,
        0,
    ])))
    .id(v.list_id)
    .on_scroll(|vp| Message::ListScrolled {
        offset: vp.absolute_offset().y,
        view: vp.bounds().height,
    })
    .width(Length::Fixed(LIST_WIDTH))
    .height(Length::Fill)
    .into()
}

/// The "Start Menu Settings" entry: the last row of the app list, drawn like
/// any installed app so it reads as one more item rather than a fixture.
pub fn settings_row<'a>(selected: bool) -> Element<'a, Message> {
    let body = row::with_children(vec![
        icon_box(icon::from_name("preferences-system").size(ICON).into()),
        text::body(fl!("menu-settings"))
            .wrapping(cosmic::iced::widget::text::Wrapping::None)
            .into(),
    ])
    .spacing(12)
    .align_y(Alignment::Center);
    button::custom(centred(body))
        .class(if selected {
            selected_button()
        } else {
            quiet_button()
        })
        .padding([0, ROW_GUTTER])
        .width(Length::Fill)
        .height(Length::Fixed(ROW_HEIGHT))
        .on_press(Message::OpenSettings)
        .into()
}

/// The Category view's jump grid: two columns of category names.
pub fn category_grid<'a>(present: &[&'static str]) -> Element<'a, Message> {
    let rows = present.chunks(2).map(|chunk| {
        let mut r = row::with_capacity(2).spacing(6);
        for &key in chunk {
            r = r.push(
                button::custom(container(text::body(fl!(key))).center(Length::Fill))
                    .class(quiet_button())
                    .padding(0)
                    .width(Length::Fill)
                    .height(Length::Fixed(44.0))
                    .on_press(Message::JumpToCategory(key)),
            );
        }
        if chunk.len() < 2 {
            r = r.push(Space::new().width(Length::Fill));
        }
        r.into()
    });
    thin_scroll(scrollable(
        column::with_children(rows.collect::<Vec<_>>()).spacing(6),
    ))
    .width(Length::Fixed(LIST_WIDTH))
    .height(Length::Fill)
    .into()
}

/// One of the launcher's results: icon, name, and its detail underneath.
fn found_row<'a>(item: &'a Item, selected: bool) -> Element<'a, Message> {
    let glyph: Element<'a, Message> = match &item.icon {
        Some(name) => icon_box(icon::from_name(name.as_str()).size(ICON).into()),
        None => Space::new().width(ICON).into(),
    };
    let mut words = column::with_capacity(2)
        .push(text::body(&item.name).wrapping(cosmic::iced::widget::text::Wrapping::None));
    if !item.description.is_empty() {
        words = words.push(
            text::caption(&item.description).wrapping(cosmic::iced::widget::text::Wrapping::None),
        );
    }
    let body = row::with_children(vec![glyph, words.into()])
        .spacing(12)
        .align_y(Alignment::Center);
    // Same height as an app row unless there is a second line, so the
    // hover and selection fills match the rows they sit on.
    let height = if item.description.is_empty() {
        ROW_HEIGHT
    } else {
        ROW_HEIGHT + 12.0
    };
    button::custom(centred(body))
        .class(if selected {
            selected_button()
        } else {
            quiet_button()
        })
        .padding([0, ROW_GUTTER])
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .on_press(Message::LauncherActivate(item.id))
        .into()
}

/// Search results: matching apps first, then what the launcher found, one
/// headed section per kind. `selected` counts down through both.
pub fn results_view<'a>(
    list: &'a [App],
    hits: &[usize],
    found: &'a [Item],
    selected: usize,
    query: &str,
) -> Element<'a, Message> {
    if hits.is_empty() && found.is_empty() {
        return container(
            column::with_children(vec![
                container(icon::from_name("system-search-symbolic").size(32))
                    .class(cosmic::theme::Container::Custom(Box::new(|theme| {
                        cosmic::widget::container::Style {
                            icon_color: muted_text(theme).color,
                            ..Default::default()
                        }
                    })))
                    .into(),
                text::body(fl!("no-results", query = query.trim())).into(),
                text::caption(fl!("no-results-hint"))
                    .class(cosmic::theme::Text::Custom(muted_text))
                    .into(),
            ])
            .spacing(8)
            .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .padding([48, 16])
        .into();
    }
    let mut rows: Vec<Element<'a, Message>> = Vec::with_capacity(hits.len() + found.len() + 8);
    if !hits.is_empty() && !found.is_empty() {
        rows.push(section_label(fl!("search-apps")));
    }
    rows.extend(
        hits.iter()
            .enumerate()
            .map(|(n, &i)| app_row(&list[i], i, n == selected)),
    );
    let mut last: Option<Section> = None;
    for (n, item) in found.iter().enumerate() {
        if last != Some(item.section) {
            rows.push(section_label(fl!(item.section.title_key())));
            last = Some(item.section);
        }
        rows.push(found_row(item, hits.len() + n == selected));
    }
    // The same right inset the browse list has, so the hover and selection
    // fills stop short of the popup edge and the scrollbar instead of
    // running underneath them; a hair of spacing keeps adjacent fills from
    // welding into one block. (No jump-scrolling here, so the spacing does
    // not upset any offset arithmetic.)
    thin_scroll(scrollable(
        container(column::with_children(rows).spacing(2)).padding([0, SCROLL_GUTTER, 12, 0]),
    ))
    .height(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str) -> App {
        App {
            id: id.into(),
            name: name.into(),
            ..App::default()
        }
    }

    #[test]
    fn the_pinned_block_is_one_capped_deduped_list() {
        let apps: Vec<App> = (0..8)
            .map(|n| app(&format!("a{n}"), &format!("App {n}")))
            .collect();
        let most: Vec<String> = ["a0", "a1", "a2"].iter().map(|s| s.to_string()).collect();
        // Recent tops the block up to the cap and never repeats a most-used.
        let recent: Vec<String> = ["a1", "a5", "a6", "a7"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let top = pinned(&apps, &most, &recent, true);
        assert_eq!(top, vec![0, 1, 2, 5, 6]);
        assert_eq!(top.len(), MOST_USED_CAP);
        assert!(pinned(&apps, &most, &recent, false).is_empty());
    }

    #[test]
    fn sections_leave_out_whatever_the_pinned_block_shows() {
        let apps = vec![app("a", "Alpha"), app("b", "Beta"), app("c", "Alfa")];
        let pinned = vec![0usize];
        let out = crate::apps::sections_excluding(&apps, &pinned);
        let flat: Vec<usize> = out.iter().flat_map(|(_, v)| v.clone()).collect();
        assert_eq!(flat, vec![2, 1]);
        assert!(!flat.contains(&0));
    }

    fn view_of<'a>(
        apps: &'a [App],
        folders: &'a [Folder],
        open: &'a HashSet<usize>,
    ) -> ListView<'a> {
        ListView {
            apps,
            most_used: &[],
            recent: &[],
            show_most_used: false,
            mode: ListMode::Az,
            folders,
            loose: &[],
            open_folders: open,
            list_id: cosmic::widget::Id::new("test-list"),
        }
    }

    #[test]
    fn the_plan_is_headers_rows_and_settings_last() {
        let apps = vec![app("a", "Alpha"), app("b", "Beta"), app("c", "Alfa")];
        let open = HashSet::new();
        let lines = plan(&view_of(&apps, &[], &open));
        assert_eq!(
            lines,
            vec![
                Line::Letter('A'),
                Line::App(0),
                Line::App(2),
                Line::Letter('B'),
                Line::App(1),
                Line::Settings,
            ]
        );
    }

    #[test]
    fn only_rows_are_stops_and_each_knows_how_far_down_it_is() {
        let apps = vec![app("a", "Alpha"), app("b", "Beta")];
        let open = HashSet::new();
        let lines = plan(&view_of(&apps, &[], &open));
        let stops = stops(&lines);
        // Two apps and the Settings row; neither letter header is a stop.
        assert_eq!(
            stops.iter().map(|s| s.act).collect::<Vec<_>>(),
            vec![Act::App(0), Act::App(1), Act::Settings]
        );
        assert_eq!(stops[0].y, HEADER_HEIGHT);
        assert_eq!(stops[0].h, ROW_HEIGHT);
        // The second app sits under its own header as well as the first row.
        assert_eq!(stops[1].y, 2.0 * HEADER_HEIGHT + ROW_HEIGHT);
        assert_eq!(stops[2].y, 2.0 * HEADER_HEIGHT + 2.0 * ROW_HEIGHT);
    }

    #[test]
    fn a_folder_row_is_a_stop_that_opens_it() {
        let apps = vec![app("a", "Alpha"), app("b", "Beta")];
        let folders = vec![Folder {
            name: "Office".into(),
            apps: vec![1],
        }];
        let mut open = HashSet::new();
        open.insert(0usize);
        let mut v = view_of(&apps, &folders, &open);
        v.mode = ListMode::Folders;
        let lines = plan(&v);
        assert!(lines.contains(&Line::Folder {
            index: 0,
            open: true
        }));
        // The open folder's app is drawn indented, and is a stop like any
        // other row.
        assert!(lines.contains(&Line::FolderApp(1)));
        let acts: Vec<Act> = stops(&lines).into_iter().map(|s| s.act).collect();
        assert_eq!(acts[0], Act::Folder(0));
        assert_eq!(acts[1], Act::App(1));
    }

    #[test]
    fn a_letter_jump_finds_the_first_row_of_that_section() {
        let apps = vec![app("a", "Alpha"), app("b", "Beta"), app("c", "Bravo")];
        let open = HashSet::new();
        let lines = plan(&view_of(&apps, &[], &open));
        assert_eq!(stop_at_letter(&lines, 'A'), Some(0));
        assert_eq!(stop_at_letter(&lines, 'B'), Some(1));
        // A letter with no section of its own is no jump at all.
        assert_eq!(stop_at_letter(&lines, 'Z'), None);
    }

    #[test]
    fn offset_counts_headers_and_rows_before_the_letter() {
        let s = vec![('#', vec![0]), ('A', vec![1, 2]), ('B', vec![3])];
        // Most used: heading + 2 rows. Then '#': header+1 row, 'A': header+2 rows.
        assert_eq!(
            offset_of(&s, 2, &'B', 10.0, 20.0),
            20.0 + 2.0 * 10.0 + 20.0 + 10.0 + 20.0 + 2.0 * 10.0
        );
        assert_eq!(offset_of(&s, 0, &'#', 10.0, 20.0), 0.0);
    }

    #[test]
    fn an_open_folder_does_not_redraw_a_pinned_app() {
        let apps = vec![app("a", "Alpha"), app("b", "Beta")];
        let folders = vec![Folder {
            name: "Office".into(),
            apps: vec![0, 1],
        }];
        let open: HashSet<usize> = [0].into();
        let most = vec!["b".to_string()];
        let lines = plan(&ListView {
            apps: &apps,
            most_used: most.as_slice(),
            recent: &[],
            show_most_used: true,
            mode: ListMode::Folders,
            folders: &folders,
            loose: &[],
            open_folders: &open,
            list_id: cosmic::widget::Id::new("test"),
        });
        // "Beta" is pinned in the Most used block, so the open folder that
        // also holds it draws only "Alpha" — one row per app, as every other
        // section already does.
        assert!(lines.contains(&Line::FolderApp(0)));
        assert!(!lines.contains(&Line::FolderApp(1)));
        assert!(lines.contains(&Line::App(1)));
    }

    #[test]
    fn the_folders_block_height_matches_the_lines_it_draws() {
        let apps = vec![app("a", "Alpha"), app("b", "Beta")];
        let folders = vec![
            Folder {
                name: "Office".into(),
                apps: vec![1],
            },
            Folder {
                name: "Tools".into(),
                apps: vec![0],
            },
        ];
        let open: HashSet<usize> = [0].into();
        let mut v = view_of(&apps, &folders, &open);
        v.mode = ListMode::Folders;
        let lines = plan(&v);
        // Everything but the trailing Settings row is the Folders block: its
        // label, two folder rows, and the open folder's app. `app.rs` places a
        // letter jump with `block_height`; if that disagrees with the height
        // the plan draws (as a header height did), the jump lands off by the
        // difference.
        let block: Vec<&Line> = lines.iter().filter(|l| **l != Line::Settings).collect();
        let drawn: f32 = block.iter().copied().map(line_height).sum();
        assert_eq!(block.len(), 4);
        assert_eq!(drawn, block_height(folders.len() + 1));
    }
}
