//! The applet: a panel button and the Start menu popup it opens.

use cosmic::app::{Core, Task};
use cosmic::iced::keyboard::{key::Named, Key as KeyCode};
use cosmic::iced::window::{self, Id};
use cosmic::iced::{Length, Limits, Point, Subscription};
use cosmic::widget::{column, container, mouse_area, popover, row, text, text_input};
use cosmic::{Application, Element};

use crate::apps::App as AppEntry;
use crate::config::{Config, ListMode, RightSide, Slot, TileRef, TileSize};
use crate::fl;
use crate::folders::Folder;
use crate::keynav::{self, Zone};
use crate::session::Power;
use crate::ui::{self, Spacing};

pub const POPUP_HEIGHT: f32 = 600.0;
/// What the app list's scroll viewport is worth before the column has
/// reported its real height: the menu less the search box, the All-apps bar
/// and the card's padding. Deliberately short of the truth — guessing small
/// only scrolls to a row that was already showing, guessing large would
/// leave the highlighted row off screen.
const LIST_VIEWPORT: f32 = POPUP_HEIGHT - 140.0;
/// Between the rail, the list and the tile column.
const COLUMN_GAP: f32 = 12.0;

/// Rail + list + tile column, with padding: exactly what the columns need.
/// Summed rather than fixed, because the tile column grows with the theme's
/// density and the 2/3-across setting; a fixed width clipped the right-hand
/// tiles under Spacious. Each column scrolls inside it.
fn popup_width(spacing: Spacing, cells: u16) -> f32 {
    2.0 * f32::from(spacing.section)
        + ui::RAIL_WIDTH
        + COLUMN_GAP
        + ui::LIST_WIDTH
        + COLUMN_GAP
        + ui::tiles::column_width(spacing, cells)
}

/// The popup's size, for both the Wayland positioner and libcosmic's popup
/// frame. The frame (`popup_container`) clamps to 360 wide by default, which
/// is a Control-Center-sized popup; left alone it hides the tile column.
pub(crate) fn popup_limits(width: f32) -> Limits {
    Limits::NONE
        .min_width(width)
        .max_width(width)
        .min_height(POPUP_HEIGHT)
        .max_height(POPUP_HEIGHT)
}

/// How this process shows the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The panel button. It draws no menu of its own: it asks the menu
    /// process for one, and starts that process at login so the first press
    /// has nothing to wait for.
    Panel,
    /// The menu itself, as a layer surface of its own — the only kind of
    /// surface the compositor hands the keyboard to.
    ///
    /// `shown` is false for `--prewarm`: the process starts, claims its bus
    /// name and waits with nothing on screen until a press arrives. A cold
    /// open had to build the app index before it could draw, and showed a
    /// blank card for ~350 ms doing it.
    Shortcut { shown: bool },
}

/// Which search result the keyboard selection lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    /// Index into the ranked app hits.
    App(usize),
    /// Index into the launcher's results, which follow the apps.
    Found(usize),
}

/// The selection counts down through the apps, then the launcher's results;
/// past the end it sticks on the last row, as the arrow keys do.
fn pick(apps: usize, found: usize, selected: usize) -> Option<Pick> {
    let last = (apps + found).checked_sub(1)?;
    let n = selected.min(last);
    Some(if n < apps {
        Pick::App(n)
    } else {
        Pick::Found(n - apps)
    })
}

/// How long the menu waits after losing the keyboard before closing.
const FOCUS_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

/// How long a new surface stays hidden waiting for the fade to take hold of
/// it before the menu is shown anyway, without one. Normally the hold comes
/// within a frame or two; this only bounds the case where it never does.
const REVEAL_DEADLINE: std::time::Duration = std::time::Duration::from_millis(300);

/// How long an animation step waits for its frame callback before running
/// anyway. A surface the compositor is not repainting gets no callbacks, and
/// a fade-out that waited on one forever would never be destroyed.
const FRAME_FALLBACK: std::time::Duration = std::time::Duration::from_millis(33);

/// The fade's link to the compositor, made on the first open.
enum FaderState {
    /// No surface has been opened yet.
    Untried,
    // Boxed: one per process, and the other states carry nothing.
    Ready(Box<crate::fade::Fader>),
    /// The compositor does not offer the alpha modifier, or the surface
    /// handle could not be read: every open and close is instant, as it was
    /// before the fade existed.
    Unavailable,
}

/// The open or close animation that is running, on the open surface or on
/// one fading out.
#[derive(Debug, Clone, Copy)]
struct Fade {
    surface: Id,
    motion: crate::motion::Motion,
    /// When the last step ran. `None` until the clock starts — see
    /// `fade_step`.
    last: Option<std::time::Instant>,
    /// The offset last sent, so the margin is only touched when it moves.
    offset: i32,
    /// Whether this opening has asked for its frost yet.
    frosted: bool,
}

impl Fade {
    /// An opening that has finished, or the motion is not an opening at all.
    fn landed(fade: Option<Self>) -> bool {
        fade.is_none_or(|f| f.motion.phase() == crate::motion::Phase::Closing)
    }
}

/// How long after closing itself for losing the keyboard the menu ignores a
/// toggle.
///
/// Pressing the panel button while the menu is up does two things in order:
/// the press moves the keyboard to the panel, which closes the menu, and then
/// the button asks for a toggle. Without this guard that second half would
/// reopen the menu the same click had just dismissed.
const REOPEN_GUARD: std::time::Duration = std::time::Duration::from_millis(600);

/// The blur region behind the frosted menu, shaped to the frame's rounded
/// corners.
///
/// The compositor blurs the rectangles it is given and nothing else, so the
/// seam between blurred and unblurred shows *through* the card's translucent
/// fill. A single whole-surface rectangle put square blurred corners outside
/// the arcs; a cross of three rectangles, which simply leaves the four
/// `radius`x`radius` corner squares alone, moved the same square inside the
/// frame — the fill over an unblurred square read as a square corner sitting
/// in each rounded one.
///
/// So the corners are stepped instead: a stack of thin bands, each only as
/// wide as the arc allows at its outermost row, which keeps every rectangle
/// strictly inside the curve. The seam then follows the corner and the
/// widest step it can be wrong by is one band.
fn blur_region(width: f32, height: f32, radius: f32) -> Vec<cosmic::iced::Rectangle> {
    use cosmic::iced::Rectangle;

    /// A rectangle, unless it has no area. Every value reaching here is a
    /// whole number of pixels.
    fn push(out: &mut Vec<Rectangle>, x: f32, y: f32, width: f32, height: f32) {
        if width > 0.0 && height > 0.0 {
            out.push(Rectangle {
                x,
                y,
                width,
                height,
            });
        }
    }

    // Whole pixels throughout. libcosmic's `apply_blur` sends x, y, width and
    // height through `.round()` each on its own, so a band that starts on a
    // fraction and has a fractional height rounds away from its neighbour:
    // at radius 16 the twelve 1.33 px bands left rows 2, 6, 10 and 14 of
    // every corner uncovered — hairlines up to 15 px wide, inside the curve,
    // with sharp background showing through the frost. Snapping the band
    // *edges* first and deriving each height from them means rounding has
    // nothing left to move.
    //
    // The surface is `width as u32` wide, so the frame is the floor.
    let w = width.floor().max(0.0);
    let h = height.floor().max(0.0);
    let r = radius.clamp(0.0, w.min(h) / 2.0);
    // The corner squares in whole pixels. Past `r` the edge is straight, so
    // the extra fraction of a pixel is ordinary frame.
    let corner = r.ceil().min((w.min(h) / 2.0).floor());

    let mut out = Vec::new();
    // The middle band, full height.
    push(&mut out, corner, 0.0, w - 2.0 * corner, h);
    // The side bands, between the arcs.
    push(&mut out, 0.0, corner, corner, h - 2.0 * corner);
    push(&mut out, w - corner, corner, corner, h - 2.0 * corner);

    // One band per pixel of radius, up to a dozen: past that the steps are
    // smaller than the blur's own softness and only cost rectangles.
    let steps = (corner as usize).clamp(1, 12);
    for i in 0..steps {
        let y0 = (corner * i as f32 / steps as f32).floor();
        let y1 = (corner * (i + 1) as f32 / steps as f32).floor();
        if y1 <= y0 {
            continue;
        }
        // Measured at the band's outermost row, where the arc bites deepest,
        // and the left edge rounded *inward*, so the band can never poke out
        // past the curve.
        let inset = if y0 >= r {
            0.0
        } else {
            let dy = r - y0;
            r - (r * r - dy * dy).max(0.0).sqrt()
        };
        let left = inset.ceil();
        let band = corner - left;
        for x in [left, w - corner] {
            for y in [y0, h - y1] {
                push(&mut out, x, y, band, y1 - y0);
            }
        }
    }
    out
}

/// Whether a toggle arriving now is the tail of the click that just closed
/// the menu.
fn is_the_closing_click(closed: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    closed.is_some_and(|at| now.duration_since(at) < REOPEN_GUARD)
}

/// How long after asking for a surface a toggle still counts as the tail of
/// the press that opened it. A cold first open has to map a surface and take
/// the keyboard, which is the slow case this has to cover.
const OPEN_GUARD: std::time::Duration = std::time::Duration::from_millis(1500);

/// Whether a toggle should be ignored because the menu it would close has
/// not appeared yet.
///
/// Both ways in spawn a copy of this binary, which finds the bus name taken
/// and pokes the menu that holds it. A second press landing in the moment
/// between the first one asking for the surface and the compositor mapping
/// it therefore closed a menu the user never saw — and any even number of
/// fast presses left nothing on screen at all. A menu that has not yet held
/// the keyboard has not been seen, so there is nothing to dismiss.
///
/// Bounded in time rather than left to the focus flag alone. A surface can
/// be born without ever gaining focus — two presses coalescing into one
/// batch destroy and recreate it in the same breath, and the new one came up
/// unfocused — and an unbounded guard then ignored every press after it, so
/// a visible menu could not be closed at all. After the window the press
/// goes through, whatever the surface did.
///
/// `seen` is [`App::seen`]: the keyboard has arrived and the opening fade has
/// landed.
fn ignores_toggle(
    opened_at: Option<std::time::Instant>,
    seen: bool,
    now: std::time::Instant,
) -> bool {
    !seen && opened_at.is_some_and(|at| now.duration_since(at) < OPEN_GUARD)
}

/// What to do with a background reload when it lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnLoad {
    /// Put it on screen now.
    Apply,
    /// Hold it until the next open.
    Stash,
    /// Throw it away: something newer is already applied or held.
    Drop,
}

/// Never change what is on screen while the menu is open.
///
/// Every open re-reads the apps, launch history and config off-thread, and
/// the first frame is drawn from what the process already holds. The usage
/// file is written on every launch, so Most used and Recent are always one
/// open stale — and the letter sections are built *excluding* whatever that
/// block shows, so a different five apps re-flowed the whole list beneath it
/// and letter sections came and went. After an idle gap the page cache is
/// cold, the reload lands late, and the menu visibly jumped under the user.
///
/// So a reload landing on a populated, open menu is held back and drawn from
/// the next open's first frame. It still applies at once when there is
/// nothing on screen to jump: the menu is closed (the pre-warm, or a reload
/// that finished after close), or it is the empty first card.
///
/// The trade-off, agreed: an app installed while the menu is open during the
/// load shows up one open later than before.
///
/// "Newest" is the load *started* last, not the one that finished last. A
/// press at login while the pre-warm's load is still crawling a cold cache
/// starts a second load; that one can finish first and fill the empty card,
/// and the pre-warm's older result then landed on a populated menu, was held,
/// and replaced the newer data at the next open. `generation` is when this
/// result was started; `newest` is the latest one applied or held.
fn on_load(popup_open: bool, populated: bool, generation: u64, newest: u64) -> OnLoad {
    if generation <= newest {
        return OnLoad::Drop;
    }
    if popup_open && populated {
        OnLoad::Stash
    } else {
        OnLoad::Apply
    }
}

/// Whether a `FocusGone` check, scheduled `FOCUS_GRACE` ago, should close the
/// menu and arm the reopen guard.
///
/// It must not arm the guard when the menu has already gone by some other
/// route — a toggle, Escape, a launch. Arming it then swallowed the *next*
/// press, roughly 200-800 ms after the click that closed the menu, for a menu
/// that was no longer there to reopen.
fn focus_loss_closes(popup_open: bool, had_focus: bool, losses: u64, loss: u64) -> bool {
    popup_open && !had_focus && loss == losses
}

pub struct App {
    core: Core,
    mode: Mode,
    fader: FaderState,
    /// Whether the open surface draws its content. False from asking for the
    /// surface until the fade has hold of it, or has given up on it: the
    /// first frame of a new surface is drawn empty, so it is never seen
    /// before the fade can hide it — and never seen at all when it comes out
    /// sheared, as the first frame at a fractional scale sometimes did.
    revealed: bool,
    /// When the animation clock started for the current open: the menu's
    /// reveal moment. `None` while the menu is closed, so the state machine
    /// in `tilemotion` holds every tile on frame 0.
    anim_start: Option<std::time::Instant>,
    /// The tile the pointer is over, if any — used only by `OnHighlight` to
    /// pick its own clock. Separate from the keyboard highlight because a
    /// tile can be hovered without being focused.
    hover: Option<TileRef>,
    /// When the tile in `hover`-or-focus became highlighted, so an
    /// `OnHighlight` motion plays from frame 0 each time it comes alive.
    highlight_since: Option<std::time::Instant>,
    /// Which tile the above `highlight_since` is for: used to tell whether a
    /// Focused/Hover change needs a fresh start.
    highlight_of: Option<TileRef>,
    /// A surface fading out after a close. The menu counts as closed while it
    /// fades — a press turns the fade round instead of opening a second
    /// surface — and the surface is destroyed when the fade ends.
    closing: Option<Id>,
    fade: Option<Fade>,
    /// Bumped on every animation step: a frame callback or fallback tick
    /// carrying any other number belongs to a step already taken.
    fade_serial: u64,
    /// Whether the menu has had the keyboard since it last opened. The
    /// surface reports losing focus once before it first gains it, which is
    /// not a click away; and a menu that has never held the keyboard has not
    /// been seen, so a toggle must not close it.
    had_focus: bool,
    /// Counts `Unfocused` events, so a `FocusGone` check can tell whether
    /// focus came back (a `Focused` in between) since it was scheduled.
    focus_losses: u64,
    popup: Option<Id>,
    /// When the surface for the menu now on screen was asked for, so a
    /// toggle arriving while it is still coming up can be told from one
    /// meant to dismiss it. See [`ignores_toggle`].
    opened_at: Option<std::time::Instant>,
    /// When the menu last closed itself for losing the keyboard, so the click
    /// that did it cannot reopen it. See [`REOPEN_GUARD`].
    closed_by_focus: Option<std::time::Instant>,
    config: Config,
    apps: Vec<AppEntry>,
    usage_top: Vec<String>,
    power_open: bool,
    error: Option<String>,
    query: String,
    /// Keyboard selection among search results.
    selected: usize,
    search_id: cosmic::widget::Id,
    /// Last pointer position over the popup, where a right-click menu opens.
    pointer: Point,
    context: Option<Context>,
    edit: ui::tiles::Edit,
    letter_grid: bool,
    list_id: cosmic::widget::Id,
    folders: Vec<Folder>,
    loose: Vec<usize>,
    open_folders: std::collections::HashSet<usize>,
    mode_menu: bool,
    /// Bumped on every config edit; a save result older than this is stale.
    edit_gen: u64,
    /// A reload that landed while the menu was open and populated, held for
    /// the next open. See [`on_load`]. Only ever the newest one.
    pending: Option<Box<Loaded>>,
    /// The generation of the newest load started, and of the newest one
    /// applied or held. A result at or below the second is older than what
    /// is already on screen or in `pending`, and is dropped.
    load_started: u64,
    load_newest: u64,
    favs: Vec<String>,
    recent: Vec<String>,
    right_menu: bool,
    avatar: Avatar,
    /// A tile field mid-edit — its name or its picture path — and the text
    /// as typed so far. One at a time, like the rename it grew from.
    renaming: Option<(TileRef, TileField, String)>,
    /// The launcher's results for the current query, beyond apps.
    found: Vec<crate::launcher::Item>,
    /// The one keyboard highlight, in one of the three zones. `None` means
    /// the search box has the keyboard, which is how the menu opens.
    nav: Option<keynav::Spot>,
    /// Where the app list is scrolled to, and how tall its viewport is, so
    /// a highlighted row can be brought into view with the smallest move.
    list_offset: f32,
    list_view: f32,
}

/// The account picture, or the initial to draw when there is none.
#[derive(Debug, Clone, Default)]
pub struct Avatar {
    pub image: Option<cosmic::widget::image::Handle>,
    pub initial: String,
}

/// Decoded pixels rather than a path: COSMIC Settings saves the account
/// picture in whatever format it got (WebP here), and iced's own loader
/// does not read them all — a path handle showed the fallback initial even
/// though Settings displayed the picture fine.
fn decode_avatar(path: &std::path::Path) -> Option<cosmic::widget::image::Handle> {
    // Sniff the format from the bytes: AccountsService icons have no file
    // extension, so the extension-based `image::open` cannot name a format.
    let decoded = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(image::ImageError::IoError)
        .and_then(|r| r.decode());
    let decoded = match decoded {
        Ok(img) => img,
        Err(e) => {
            tracing::warn!("could not decode {}: {e}", path.display());
            return None;
        }
    };
    // Drawn at 28 logical pixels; 128 keeps it crisp on any scale factor
    // without holding a full-size photo in memory.
    let small = decoded.thumbnail(128, 128).into_rgba8();
    let (w, h) = small.dimensions();
    Some(cosmic::widget::image::Handle::from_rgba(
        w,
        h,
        small.into_raw(),
    ))
}

/// AccountsService keeps the picture world-readable under the user's name;
/// `~/.face` is the older convention. The initial comes from the account's
/// full name, falling back to the login name.
fn load_avatar() -> Avatar {
    let user = std::env::var("USER").unwrap_or_default();
    let image = [
        std::path::PathBuf::from("/var/lib/AccountsService/icons").join(&user),
        dirs::home_dir().unwrap_or_default().join(".face"),
    ]
    .into_iter()
    .find(|p| !user.is_empty() && p.is_file())
    .and_then(|p| decode_avatar(&p));
    let full_name = std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|passwd| {
            passwd
                .lines()
                .find(|l| l.split(':').next() == Some(user.as_str()))
                .and_then(|l| l.split(':').nth(4).map(str::to_owned))
        })
        .unwrap_or_default();
    let initial = full_name
        .chars()
        .chain(user.chars())
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    Avatar { image, initial }
}

/// Which of a tile's text fields the inline input is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileField {
    /// The tile's own name, over the app's.
    Label,
    /// The path of the picture drawn behind the tile.
    Image,
}

/// What a right-click menu is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Index into the app list.
    App(usize),
    /// A tile, by its place in the config.
    Tile(TileRef),
}

#[derive(Debug, Clone, Copy)]
pub struct Context {
    pub target: Target,
    pub at: Point,
}

/// Keys the open menu handles itself, whichever widget the press reached.
/// Enter is not one of them: it arrives either from the search input's own
/// submit or, once the keyboard has moved off the input, from the
/// uncaptured-key listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    /// Sideways, which only the tile grid has.
    Left,
    Right,
    Escape,
    /// Tab and Shift+Tab, which carry the highlight between zones.
    Tab,
    ShiftTab,
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    /// A surface got the keyboard: now the search box can take it. Focusing
    /// it at open does nothing, the surface does not exist yet.
    ///
    /// Tagged with the surface, as is `Unfocused`: a menu fading out loses
    /// the keyboard too, and reopened inside `FOCUS_GRACE` that loss used to
    /// be taken for the new surface's and closed it.
    Focused(Id),
    /// A surface lost the keyboard, e.g. to a click on a window.
    Unfocused(Id),
    /// libcosmic has made a surface: its handles can be read now.
    SurfaceOpened(Id),
    /// A surface's display and `wl_surface`, for the fade to take hold of.
    FadeHandles(Id, Option<crate::fade::Handles>),
    /// Show a surface the fade never got hold of, without a fade.
    RevealDeadline(Id),
    /// Time for the next animation step, from a frame callback or the
    /// fallback timer. The number is the step it was asked for by.
    FadeFrame(u64),
    /// `FOCUS_GRACE` after an `Unfocused`: close if the keyboard has not
    /// come back since. The number is the `focus_losses` count it belongs to.
    FocusGone(u64),
    /// Something back from COSMIC's launcher service.
    Launcher(crate::launcher::Reply),
    /// Run one of the launcher's results.
    LauncherActivate(u32),
    /// Start renaming a tile (from its right-click menu).
    RenameTile(TileRef),
    /// Start typing a picture path for a tile (from its right-click menu).
    EditTileImage(TileRef),
    RenameText(String),
    RenameDone,
    /// Set or clear a tile's own fill colour.
    TileColor(TileRef, Option<String>),
    /// Take the picture off a tile.
    ClearTileImage(TileRef),
    /// Choose how a tile's GIF plays.
    SetTileMotion(TileRef, crate::tilemotion::Motion),
    /// Whether a tile with a picture also draws its app icon.
    SetTileIconOverImage(TileRef, bool),
    /// The pointer entered or left a tile — tracked to drive OnHighlight GIFs.
    TileHover(Option<TileRef>),
    /// Advance animated-GIF tiles.
    AnimTick,
    /// Ctrl+1..9: launch the n'th pinned tile.
    TileNumber(usize),
    PopupClosed(Id),
    /// Everything read from disk when the popup opens, in one go.
    Loaded(Box<Loaded>),
    Launch(usize),
    /// A tile, which knows its app by id rather than list position.
    LaunchId(String),
    LetterGrid(bool),
    PowerMenu(bool),
    Power(Power),
    ShowError(String),
    OpenSettings,
    Query(String),
    KeyPress(Key),
    /// A plain letter with the highlight in the app list: a jump to that
    /// letter's section rather than a character for the search box.
    Letter(char),
    /// The app list was scrolled, by the pointer or by us: where to, and
    /// how tall its viewport actually is.
    ListScrolled {
        offset: f32,
        view: f32,
    },
    Submit,
    Pointer(Point),
    OpenContext(Target),
    CloseContext,
    Pin(String),
    Unpin(String),
    Resize(TileRef, TileSize),
    MoveToGroup(TileRef, usize),
    NewGroupWith(TileRef),
    RunAction(usize, usize),
    ToggleEdit,
    JumpTo(char),
    JumpToCategory(&'static str),
    ModeMenu(bool),
    SetListMode(ListMode),
    ToggleFolder(usize),
    RightMenu(bool),
    SetRightSide(RightSide),
    AddFavourite(String),
    RemoveFavourite(String),
    FavouritesSaved(Vec<String>),
    ConfigSaved(u64, Box<Config>),
    /// A tile pressed in edit mode: pick it up, drop onto it, or put it back.
    TileClicked(TileRef),
    DropEnd(usize),
    DropNew,
    AddGroup,
    RenameGroup(usize, String),
    RemoveGroup(usize),
    /// One of the rail's default-app shortcuts.
    OpenSlot(Slot),
    OpenSettingsApp,
    OpenAccount,
}

impl Message {
    /// Whether this came from someone using the menu — a key, a click, a
    /// scroll — rather than from the process's own machinery.
    ///
    /// Listed the other way round, as the machinery, so that a variant added
    /// later counts as input by default: a surface fading out ignores input
    /// (see `update`), and wrongly ignoring a new kind of click there costs
    /// nothing, where wrongly acting on one could change a menu mid-fade.
    fn is_input(&self) -> bool {
        !matches!(
            self,
            Message::TogglePopup
                | Message::Focused(_)
                | Message::Unfocused(_)
                | Message::FocusGone(_)
                | Message::SurfaceOpened(_)
                | Message::FadeHandles(..)
                | Message::RevealDeadline(_)
                | Message::FadeFrame(_)
                | Message::Launcher(_)
                | Message::PopupClosed(_)
                | Message::Loaded(_)
                | Message::ShowError(_)
                | Message::ConfigSaved(..)
                | Message::FavouritesSaved(_)
        )
    }
}

#[derive(Debug, Clone)]
pub struct Loaded {
    /// When this load was *started*, from `App::load_started`. Two loads can
    /// be in flight at once and finish in either order, so the order they
    /// land in says nothing about which is newer.
    pub generation: u64,
    pub apps: Vec<AppEntry>,
    pub most_used: Vec<String>,
    pub config: Config,
    pub folders: Vec<Folder>,
    pub loose: Vec<usize>,
    pub favs: Vec<String>,
    pub recent: Vec<String>,
    pub avatar: Avatar,
}

/// Read the app index, launch history and config. Blocking file I/O, so it
/// runs on the blocking pool, never in `update`.
fn load(generation: u64) -> Loaded {
    let apps = crate::apps::load_all();
    let ids: std::collections::HashSet<&str> = apps.iter().map(|a| a.id.as_str()).collect();
    let usage = crate::usage::Usage::path()
        .map(|p| crate::usage::Usage::load_from(&p))
        .unwrap_or_default();
    let most_used = usage.top(5, &ids);
    let recent = usage.recent(crate::usage::RECENT_CAP, &ids);
    // The index the config needs for first-run seeding is the one just
    // built: `Config::load` would otherwise read every desktop file a
    // second time, doubling the cost of every open.
    let installed: Vec<String> = apps.iter().map(|a| a.id.clone()).collect();
    let config = Config::load_with(&installed);
    // The brand colour behind each tile, derived from the app's icon. Only
    // the apps a tile points at need one — favourites and recent draw their
    // icons on no fill — so the rasterising is a dozen icons, not the whole
    // index, and `brand` caches by file across reloads.
    let mut apps = apps;
    if config.tile_colors_from_icon {
        let tiled: std::collections::HashSet<String> = config
            .groups
            .iter()
            .flat_map(|g| g.tiles.iter().map(|t| t.app.clone()))
            .collect();
        for app in apps.iter_mut().filter(|a| tiled.contains(&a.id)) {
            app.brand = crate::brand::of_icon(&app.icon);
        }
    }
    finish_load(generation, apps, most_used, config, recent)
}

fn finish_load(
    generation: u64,
    apps: Vec<AppEntry>,
    most_used: Vec<String>,
    config: Config,
    recent: Vec<String>,
) -> Loaded {
    let (folders, loose) = crate::folders::load(&apps);
    Loaded {
        generation,
        apps,
        most_used,
        config,
        folders,
        loose,
        favs: crate::favorites::read(),
        recent,
        avatar: load_avatar(),
    }
}

/// What is open on top of the popup, for deciding what Escape closes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Layers {
    /// A keyboard highlight is on something.
    highlight: bool,
    context: bool,
    renaming: bool,
    power: bool,
    mode_menu: bool,
    right_menu: bool,
    letter_grid: bool,
    picked: bool,
    query: bool,
    editing: bool,
}

/// What one press of Escape does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    DropHighlight,
    Context,
    CancelRename,
    Menus,
    LetterGrid,
    DropPick,
    ClearSearch,
    LeaveEdit,
    ClosePopup,
}

/// Escape peels one layer at a time, topmost first, and closes the whole
/// popup only when nothing else is open — so dismissing a right-click menu
/// or a search never throws the user out of the Start menu. The keyboard
/// highlight is the top rung: one press puts the keyboard back in the search
/// box, which is where the menu started.
fn escape_target(l: Layers) -> Escape {
    if l.highlight {
        Escape::DropHighlight
    } else if l.context {
        Escape::Context
    } else if l.renaming {
        Escape::CancelRename
    } else if l.power || l.mode_menu || l.right_menu {
        Escape::Menus
    } else if l.letter_grid {
        Escape::LetterGrid
    } else if l.picked {
        Escape::DropPick
    } else if l.query {
        Escape::ClearSearch
    } else if l.editing {
        Escape::LeaveEdit
    } else {
        Escape::ClosePopup
    }
}

/// What a navigation key does when something modal is on top of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavKeys {
    /// Move the highlight as asked.
    Move,
    /// Refused; the keyboard goes back to the open rename draft.
    HoldRename,
    /// Refused; the keyboard goes back to the search box.
    HoldSearch,
}

/// Whether a navigation key — Tab, Shift+Tab or an arrow onto the highlight
/// — moves it right now, and if not, where the keyboard belongs instead.
///
/// A right-click menu and an open rename draft are both modal: nothing in
/// either is focusable, so their keys arrive uncaptured and would otherwise
/// walk the highlight behind them — and the next Enter would launch the
/// highlighted row rather than acting on what was right-clicked.
///
/// Refusing is not enough on its own, because Tab has already done damage by
/// the time we see it: libcosmic's text_input marks itself read-only on Tab
/// and returns *without* consuming the press. Only a focus operation clears
/// that — a click cannot, since the mouse-press reset is gated on an
/// editable variant neither the search box nor the rename field is, and
/// neither field has an `on_tab` or `on_unfocus` of its own. So a refused key
/// hands the keyboard back to whichever input is open; left to do nothing it
/// froze the draft so hard that only Escape, which throws the draft away,
/// got out of it.
///
/// Escape is not one of these keys: its ladder already peels the top layer
/// first.
fn nav_keys(context: bool, renaming: bool) -> NavKeys {
    if renaming {
        NavKeys::HoldRename
    } else if context {
        NavKeys::HoldSearch
    } else {
        NavKeys::Move
    }
}

/// Shorthand for the keys that only need refusing, never a refocus: Enter on
/// the highlight and the first-letter jump, neither of which touches focus.
fn nav_keys_live(context: bool, renaming: bool) -> bool {
    nav_keys(context, renaming) == NavKeys::Move
}

impl App {
    /// Show the menu: a popup off the panel button, or in shortcut mode a
    /// layer surface the compositor gives the keyboard to.
    fn open(&mut self) -> Task<Message> {
        // A press while a surface is still fading out turns that fade round
        // instead (see `reverse_close`), so a new surface is only ever asked
        // for when no surface exists — which the stash below relies on.
        debug_assert!(self.closing.is_none());
        self.closed_by_focus = None;
        self.power_open = false;
        self.error = None;
        self.query.clear();
        self.selected = 0;
        self.context = None;
        self.nav = None;
        // A reload held back while the menu was last open goes in now, before
        // the surface is asked for, so this open draws it from its first
        // frame. Applied here rather than at close: a closing surface fades
        // out showing what it showed, and here there is no surface at all to
        // paint, focus or scroll — a press during the fade-out reverses it
        // without coming through here. Every close leads to this open,
        // whichever way the menu went away.
        //
        // Its config is not applied. It was read before any edit made while
        // that menu was open, and the peek just below re-reads the file — the
        // in-menu edits and anything the Settings window saved since — so the
        // stashed copy could only ever be the older one.
        if let Some(loaded) = self.pending.take() {
            self.apply_loaded(*loaded, false);
        }
        // The width and position depend on settings, and the full load
        // below only delivers after the popup is placed; edits are saved as
        // they are made, so the file is current.
        self.config = Config::peek();
        let id = window::Id::unique();
        self.popup = Some(id);
        // Every open starts not-yet-seen: `ignores_toggle` reads this, and
        // the process is resident now, so a value left over from the last
        // open would let a double press close a menu that is still mapping.
        self.had_focus = false;
        // A focus-loss check scheduled for an earlier surface is not about
        // this one.
        self.focus_losses = self.focus_losses.wrapping_add(1);
        self.opened_at = Some(std::time::Instant::now());
        tracing::debug!("menu opening");
        // Hidden until the fade has hold of the surface, then faded up from
        // nothing while rising into place. Once the fade is known not to
        // work, the menu is shown at once at rest, as it always was.
        let animated = !matches!(self.fader, FaderState::Unavailable);
        self.revealed = !animated;
        self.fade = None;
        let offset = if animated { crate::motion::SLIDE_PX } else { 0 };
        // One surface kind for both ways in. As a panel popup the menu could
        // not be closed by clicking the button again without a race, could
        // not take the keyboard unless a click handed it over, and a second
        // menu could open beside it from the shortcut. A layer surface is
        // focusable, dismissable and the only menu there is.
        let popup = crate::shortcut::surface(id, self.width(), self.config.menu_position, offset);

        let load = self.reload();

        // Focused straight away so typing searches, as in Windows.
        let focus = text_input::focus(self.search_id.clone());

        let deadline = if animated {
            Task::perform(tokio::time::sleep(REVEAL_DEADLINE), move |()| {
                cosmic::action::app(Message::RevealDeadline(id))
            })
        } else {
            Task::none()
        };

        // Without a fade the frost is asked for now, ahead of the surface, so
        // `get_layer_surface` installs it on the first commit. With one it
        // waits for the content: see `blur`.
        let frost = if self.revealed {
            self.blur()
        } else {
            Task::none()
        };
        Task::batch([frost, popup, load, focus, deadline])
    }

    /// Put a reload's data in place. Fields only: it returns no task, so it
    /// can never move focus or scroll anything by itself.
    ///
    /// `with_config` is false for a stashed reload, whose config is older
    /// than the file `open` re-reads right after.
    fn apply_loaded(&mut self, loaded: Loaded, with_config: bool) {
        let Loaded {
            generation: _,
            apps,
            most_used,
            config,
            folders,
            loose,
            favs,
            recent,
            avatar,
        } = loaded;
        self.favs = favs;
        self.recent = recent;
        self.avatar = avatar;
        self.folders = folders;
        self.loose = loose;
        self.apps = apps;
        self.usage_top = most_used;
        if with_config {
            self.config = config;
        }
    }

    /// Read the app index, launch history and config off-thread.
    ///
    /// Run on every open, so an app installed a minute ago shows up and edits
    /// made in the Settings window apply, without a file watcher — and once
    /// more at pre-warm, so the menu has real data to draw with before the
    /// first press rather than the blank card it shows with none.
    fn reload(&mut self) -> Task<Message> {
        self.load_started = self.load_started.wrapping_add(1);
        let generation = self.load_started;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || load(generation))
                    .await
                    .map_err(|e| e.to_string())
            },
            |r| match r {
                Ok(l) => cosmic::action::app(Message::Loaded(Box::new(l))),
                Err(e) => cosmic::action::app(Message::ShowError(e)),
            },
        )
    }

    /// Ask the compositor to blur what is behind the menu, so the card's
    /// translucent fill reads as COSMIC's frosted glass rather than a flat
    /// wash over whatever window it covers.
    ///
    /// libcosmic only blurs surfaces it tracks in `surface_views` — the main
    /// window and `surface-message` surfaces — and a layer surface asked for
    /// with `get_layer_surface` is in neither, so nothing requests it for us.
    ///
    /// When it is asked for has moved twice, and both moves were about what
    /// the frost sits behind.
    ///
    /// It used to wait for the keyboard, which left the card clear for the
    /// 160-500 ms until the focus event landed. So it moved into the batch
    /// that creates the surface — a request for an id with no surface yet is
    /// stashed in libcosmic's `pending_blur` and installed before the
    /// surface's first commit — and the menu was frosted from its first
    /// frame.
    ///
    /// The fade moved it again. A new surface is now drawn empty until the
    /// fade has hold of it, 70-380 ms, and frost asked for at creation showed
    /// as an empty frosted pane for all of that. The frost cannot fade with
    /// the content either: the compositor draws it at full strength as an
    /// element of its own, outside the alpha multiplier. So it is asked for on
    /// the first step that shows any content (`fade_step`), and appears with
    /// it; at close it stays at full strength and goes with the surface.
    /// Without a fade the creation-time request still applies, as before.
    ///
    /// It is asked for again when the surface takes the keyboard and when an
    /// opening fade lands, as a backstop for a cold start, where a request is
    /// lost for two reasons that do not apply to any later open:
    /// `ext_background_effect_manager` may not be bound yet on the process's
    /// very first surface, and `apply_blur` then logs "Blur effect is not
    /// supported." and drops the region; and the theme may not have loaded,
    /// so `frosted()` is still false. Setting the region twice is harmless —
    /// an already-blurred surface just has its region updated.
    ///
    /// The region is shaped to the frame's corners: the menu is exactly
    /// `width()` × `POPUP_HEIGHT`, and `card_style` rounds it by the theme's
    /// medium radius, so a whole-surface rectangle showed square blurred
    /// corners outside the arcs.
    fn blur(&self) -> Task<Message> {
        match self.popup {
            Some(id) => self.blur_for(id),
            None => Task::none(),
        }
    }

    fn blur_for(&self, id: Id) -> Task<Message> {
        let theme = self.core.system_theme();
        if !self.core.frosted(theme.cosmic()) {
            return Task::none();
        }
        let radius = theme.cosmic().corner_radii.radius_m[0];
        cosmic::iced::platform_specific::shell::commands::blur::blur(
            id,
            Some(blur_region(self.width(), POPUP_HEIGHT, radius)),
        )
        .discard()
    }

    /// Dismiss the menu: fade it out if the fade has hold of it, else take
    /// it away at once.
    ///
    /// Whatever the route — Escape, a launch, a click away, a press — the
    /// menu stops being the open menu here, and anything waiting on the close
    /// (a launch) is not held up by the fade.
    ///
    /// The keyboard is given up here too, but cosmic-comp only moves focus
    /// when the surface is destroyed: a layer surface that turns keyboard
    /// interactivity off keeps the focus it has (no leave event arrives in
    /// the 120 ms of the fade). So keys typed during the fade-out still reach
    /// the fading surface, where `update` ignores them, and the window
    /// underneath has the keyboard again when the fade ends.
    fn close_popup(&mut self) -> Task<Message> {
        let Some(id) = self.popup.take() else {
            return Task::none();
        };
        // Not "the menu now coming up" any more, so a press during the
        // fade-out turns it round instead of counting as the tail of the
        // press that opened it.
        self.opened_at = None;
        let animated =
            self.revealed && matches!(&self.fader, FaderState::Ready(fader) if fader.attached());
        if !animated {
            return self.destroy_surface(id);
        }
        tracing::debug!("menu closing");
        // From wherever it is now: a menu still fading in turns round.
        let motion = match self.fade {
            Some(fade) if fade.surface == id => fade.motion.reversed(),
            _ => crate::motion::Motion::closing(),
        };
        self.closing = Some(id);
        self.fade = Some(Fade {
            surface: id,
            motion,
            // The clock keeps running: the first step out moves by a frame.
            last: Some(std::time::Instant::now()),
            offset: motion.frame().offset,
            frosted: true,
        });
        // Its contents are left exactly as they are until it is gone — a
        // search snapping back to the full list mid-fade was the giveaway —
        // so `reset_popup_state` waits for `finish_close`.
        Task::batch([crate::shortcut::drop_keyboard(id), self.fade_step(false)])
    }

    /// Destroy a surface, and clear everything that belonged to its opening.
    ///
    /// The surface goes; the process stays. A standalone menu used to exit a
    /// moment after closing, which meant every open paid for a fresh process
    /// — a visible lag, a first frame drawn with no apps in it, and a race
    /// where a press landing on the dying copy did nothing. Resident, it
    /// keeps serving toggles over its bus name and every open after the first
    /// is instant. Nothing is left behind: the fade lets go first (its alpha
    /// object must not outlive the surface), the state of the opening is
    /// cleared, and the next open asks for a new surface id.
    fn destroy_surface(&mut self, id: Id) -> Task<Message> {
        tracing::debug!("menu closed");
        if let FaderState::Ready(fader) = &mut self.fader {
            fader.detach();
        }
        self.fade = None;
        self.revealed = false;
        self.anim_start = None;
        self.hover = None;
        self.highlight_since = None;
        self.highlight_of = None;
        self.reset_popup_state();
        cosmic::iced::platform_specific::shell::commands::layer_surface::destroy_layer_surface(id)
    }

    /// The fade-out has ended: the surface goes.
    fn finish_close(&mut self) -> Task<Message> {
        match self.closing.take() {
            Some(id) => self.destroy_surface(id),
            None => Task::none(),
        }
    }

    /// A press while the menu fades out: the same surface comes back up from
    /// where it is, with the keyboard. Destroying it and opening a new one
    /// blinked — the old card vanished a frame before the new one began.
    fn reverse_close(&mut self, id: Id) -> Task<Message> {
        tracing::debug!("menu coming back");
        self.popup = Some(id);
        // A focus-loss check from before the close is not about this return.
        self.focus_losses = self.focus_losses.wrapping_add(1);
        if let Some(fade) = &mut self.fade {
            fade.motion = fade.motion.reversed();
        }
        // It is on screen already, so a press may dismiss it again at once:
        // no opening guard (`ignores_toggle`).
        self.opened_at = None;
        // Usually the surface never lost the keyboard (see `close_popup`), and
        // goes straight back to on-demand, the open menu's steady state, so a
        // click elsewhere still closes it. If the compositor did take it, the
        // surface asks for it outright as a new one does, and the focus event
        // hands it back to on-demand. Asking outright when no focus event is
        // coming left the menu holding the keyboard for good: a click on a
        // window then never closed it.
        let keyboard = if self.had_focus {
            crate::shortcut::release_keyboard(id)
        } else {
            crate::shortcut::grab_keyboard(id)
        };
        Task::batch([keyboard, self.focus_search()])
    }

    /// Show a hidden surface at once, at rest and without a fade: the fade
    /// could not take hold of it.
    fn reveal_now(&mut self, id: Id) -> Task<Message> {
        tracing::debug!("menu shown without a fade");
        self.revealed = true;
        self.anim_start = Some(std::time::Instant::now());
        self.fade = None;
        Task::batch([
            crate::shortcut::slide(id, self.config.menu_position, 0),
            self.blur_for(id),
            self.focus_search(),
        ])
    }

    /// Whether the open menu has been seen, for `ignores_toggle`: it has the
    /// keyboard, and its opening fade has landed.
    ///
    /// The keyboard alone used to be the sign — focus took 160-500 ms to
    /// arrive, by which time the menu was plainly on screen. A resident menu
    /// takes the keyboard within a few milliseconds, while it is still
    /// invisible under the fade, so a second press of a quick pair closed it
    /// again mid-fade-in: a flash, then nothing, which is the very fault the
    /// guard exists for. Until the fade lands, the press is the tail of the
    /// one that opened it. `OPEN_GUARD` still bounds the wait.
    fn seen(&self) -> bool {
        self.had_focus && self.revealed && Fade::landed(self.fade)
    }

    /// Give the search box the keyboard, unless the highlight has it.
    ///
    /// Asked for again whenever the view switches from hidden to the real
    /// menu: `open` and the first focus event both land while the surface is
    /// still drawn empty, when there is no search box in it to focus, and the
    /// menu then came up with a box that ignored typing.
    fn focus_search(&self) -> Task<Message> {
        if self.nav.is_none() {
            text_input::focus(self.search_id.clone())
        } else {
            Task::none()
        }
    }

    /// One step of the running animation. `tick` moves it on by the time
    /// since the last step; without it the current frame is shown again,
    /// which is how a fade is started and how a close kicks off its loop.
    ///
    /// The clock starts on the first frame callback after the content is
    /// drawn, not when the fade takes hold: taking hold shows the surface at
    /// nothing and switches the view to the real menu, and the frame that
    /// draws that menu is the slow one. Each step is then capped at a frame
    /// (`motion::MAX_STEP`), so a slow frame holds the motion back instead of
    /// throwing it forward.
    ///
    /// Paced by the compositor: each step asks for a frame callback, and the
    /// next step runs when it comes — or after `FRAME_FALLBACK`, whichever is
    /// first, since a surface that is not being repainted gets no callbacks.
    fn fade_step(&mut self, tick: bool) -> Task<Message> {
        let Some(mut fade) = self.fade else {
            return Task::none();
        };
        let frame = if tick {
            let now = std::time::Instant::now();
            let elapsed = fade.last.map_or(std::time::Duration::ZERO, |t| now - t);
            fade.last = Some(now);
            fade.motion.advance(elapsed)
        } else {
            fade.motion.frame()
        };
        let closing = fade.motion.phase() == crate::motion::Phase::Closing;
        if frame.done && closing {
            return self.finish_close();
        }
        self.fade_serial = self.fade_serial.wrapping_add(1);
        let serial = self.fade_serial;
        tracing::trace!(
            serial,
            opacity = frame.opacity,
            offset = frame.offset,
            "fade step"
        );
        if let FaderState::Ready(fader) = &mut self.fader {
            fader.show(frame.opacity, serial);
        }
        let slide = if frame.offset == fade.offset {
            Task::none()
        } else {
            fade.offset = frame.offset;
            crate::shortcut::slide(fade.surface, self.config.menu_position, frame.offset)
        };
        // The frost arrives with the first content anyone can see. Asked
        // for on the step that starts the clock, which still shows nothing:
        // libcosmic installs the region without committing, so it reaches
        // the screen with the next commit — the one that makes the content
        // visible. Asked for on that step instead, it trailed by a frame and
        // the first sight of the menu was its content over sharp background.
        let frost = if tick && !fade.frosted {
            fade.frosted = true;
            self.blur_for(fade.surface)
        } else {
            Task::none()
        };
        if frame.done {
            // Landed: solid, at rest, nothing left to step. Ask for the frost
            // once more as the cold-start backstop.
            self.fade = None;
            return Task::batch([slide, frost, self.blur_for(fade.surface)]);
        }
        self.fade = Some(fade);
        let fallback = Task::perform(tokio::time::sleep(FRAME_FALLBACK), move |()| {
            cosmic::action::app(Message::FadeFrame(serial))
        });
        Task::batch([slide, frost, fallback])
    }

    /// Apply a change to the config: at once to the copy on screen, and to
    /// the file as it is on disk (off-thread), so a change made here can
    /// never undo one the Settings window saved, and an edit made before the
    /// first load cannot wipe the pins. The saved result replaces the copy on
    /// screen unless a newer edit has been made since.
    fn edit(&mut self, f: impl Fn(&mut Config) + Send + 'static) -> Task<Message> {
        f(&mut self.config);
        self.context = None;
        self.edit_gen = self.edit_gen.wrapping_add(1);
        let generation = self.edit_gen;
        let save = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || Config::update(f))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
            },
            move |r| match r {
                Ok(saved) => cosmic::action::app(Message::ConfigSaved(generation, Box::new(saved))),
                Err(e) => cosmic::action::app(Message::ShowError(e)),
            },
        );
        // The edit may have taken a tile out from under the highlight.
        Task::batch([self.renav(), save])
    }

    /// Everything that belongs to one opening of the popup. Cleared however
    /// the popup goes away — our own close, Escape, or a click outside.
    fn reset_popup_state(&mut self) {
        self.renaming = None;
        self.power_open = false;
        self.context = None;
        self.edit = ui::tiles::Edit::default();
        self.letter_grid = false;
        self.mode_menu = false;
        self.right_menu = false;
        self.query.clear();
        self.selected = 0;
        self.found.clear();
        self.nav = None;
        self.list_offset = 0.0;
        self.opened_at = None;
        // Folder indices point into `self.folders`, which is rebuilt from the
        // App Library on every load. The process used to exit between opens,
        // so this cleared itself; now it lives all session, and a retained
        // index would expand whichever folder had taken that place.
        self.open_folders.clear();
    }

    /// The apps drawn in the pinned block, which the sections below it
    /// leave out.
    fn pinned_rows(&self) -> Vec<usize> {
        ui::app_list::pinned(
            &self.apps,
            &self.usage_top,
            &self.recent,
            self.config.show_most_used,
        )
    }

    /// Height of the Most used block at the top of the list, if shown.
    fn most_used_height(&self, pinned: &[usize]) -> f32 {
        if pinned.is_empty() {
            0.0
        } else {
            ui::app_list::block_height(pinned.len())
        }
    }

    /// Height of the Folders block: its label, one row per folder, and the
    /// apps of any folder that is open.
    fn folders_height(&self) -> f32 {
        let open: usize = self
            .open_folders
            .iter()
            .filter_map(|&i| self.folders.get(i))
            .map(|f| f.apps.len())
            .sum();
        ui::app_list::block_height(self.folders.len() + open)
    }

    /// Scroll the list, and remember where to: `scroll_to` is an operation
    /// on the widget, not an event, so nothing reports back the offset we
    /// asked for and the next keyboard move would measure from the old one.
    fn scroll_list(&mut self, y: f32) -> Task<Message> {
        self.list_offset = y;
        cosmic::iced::widget::scrollable::scroll_to(
            self.list_id.clone(),
            cosmic::iced::widget::scrollable::AbsoluteOffset {
                x: None,
                y: Some(y),
            },
        )
    }

    /// Write the dock's favourites off-thread; the menu shows the new list
    /// once the write has landed, so it never shows what the dock does not.
    fn save_favourites(&mut self, list: Vec<String>) -> Task<Message> {
        self.context = None;
        Task::perform(
            async move {
                let to_write = list.clone();
                tokio::task::spawn_blocking(move || crate::favorites::write(&to_write))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r)
                    .map(|()| list)
            },
            |r| match r {
                Ok(list) => cosmic::action::app(Message::FavouritesSaved(list)),
                Err(e) => cosmic::action::app(Message::ShowError(e)),
            },
        )
    }

    fn report(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            tracing::warn!("{e}");
            self.error = Some(e);
        }
    }

    fn spacing(&self) -> Spacing {
        match self.mode {
            Mode::Panel => Spacing::from_theme(self.core.system_theme()),
            // The shortcut menu sizes its surface in `init`, before the
            // theme has loaded: the default theme's spacing made the surface
            // too narrow for Spacious gaps, squeezing the last tile column.
            // One source for both the size and the layout keeps them agreeing.
            Mode::Shortcut { .. } => Spacing::from_density(),
        }
    }

    /// Ask the launcher about the current query, or drop its old answer.
    fn ask_launcher(&mut self) {
        let q = self.query.trim();
        if q.is_empty() {
            self.found.clear();
        } else {
            crate::launcher::search(q);
        }
    }

    // --- the keyboard highlight ------------------------------------------

    /// The middle column's state, borrowed for drawing it and for walking it
    /// with the keyboard — one description, so a highlight cannot land on a
    /// row the column is not showing.
    fn list_view(&self) -> ui::app_list::ListView<'_> {
        ui::app_list::ListView {
            apps: &self.apps,
            most_used: &self.usage_top,
            recent: &self.recent,
            show_most_used: self.config.show_most_used,
            mode: self.config.list_mode,
            folders: &self.folders,
            loose: &self.loose,
            open_folders: &self.open_folders,
            list_id: self.list_id.clone(),
        }
    }

    /// Whether the browse list is on screen at all: a search replaces it
    /// with results, and the letter grid replaces it with letters.
    fn browsing(&self) -> bool {
        self.query.trim().is_empty() && !self.letter_grid
    }

    /// Every row of the app list the keyboard can land on.
    fn list_stops(&self) -> Vec<ui::app_list::Stop> {
        if !self.browsing() {
            return Vec::new();
        }
        ui::app_list::stops(&ui::app_list::plan(&self.list_view()))
    }

    /// The tiles the keyboard can land on, and the grid cells they occupy —
    /// packed by the same packer the column draws them with.
    fn tile_stops(&self) -> (Vec<TileRef>, Vec<crate::tile_layout::Placement>) {
        if !self.query.trim().is_empty() {
            return (Vec::new(), Vec::new());
        }
        let (refs, groups) = ui::tiles::keyboard_tiles(&self.config, &self.apps);
        let cells = keynav::tile_grid(&groups, self.config.tile_cells());
        (refs, cells)
    }

    /// How many stops each zone has right now.
    fn counts(&self) -> keynav::Counts {
        // Nothing is drawn at all while the first load is in flight, and the
        // results have their own arrow keys and Enter while a search is on —
        // Tab must not take the keyboard out of the box mid-query.
        if self.apps.is_empty() || !self.query.trim().is_empty() {
            return keynav::Counts::default();
        }
        keynav::Counts {
            list: self.list_stops().len(),
            tiles: self.tile_stops().0.len(),
            rail: ui::rail::items(&self.config.system_panel).len(),
        }
    }

    /// Which stop of `zone` is highlighted, if the highlight is there.
    fn focus_in(&self, zone: Zone) -> Option<usize> {
        self.nav.filter(|s| s.zone == zone).map(|s| s.index)
    }

    /// The tile currently highlighted (pointer or keyboard), if any. The
    /// keyboard wins when both land on different tiles, since arrowing
    /// through the grid should keep its own GIF playing even if the pointer
    /// is still parked somewhere else.
    fn highlighted_tile(&self) -> Option<TileRef> {
        self.focused_tile().or(self.hover)
    }

    /// Which tile the keyboard highlight is on, if the keyboard is in the
    /// tile zone.
    fn focused_tile(&self) -> Option<TileRef> {
        self.focus_in(Zone::Tiles)
            .and_then(|n| self.tile_stops().0.get(n).copied())
    }

    /// A hover/focus change updates when the OnHighlight clock started, so
    /// each highlight plays its GIF from the first frame.
    fn refresh_highlight_clock(&mut self) {
        let now = self.highlighted_tile();
        if now != self.highlight_of {
            self.highlight_of = now;
            self.highlight_since = now.map(|_| std::time::Instant::now());
        }
    }

    /// How long the animation clock for `(g, t)` has been running under
    /// `motion`, or zero if it is not animating now.
    fn tile_elapsed(&self, at: TileRef, motion: crate::tilemotion::Motion) -> std::time::Duration {
        use crate::tilemotion::Motion;
        match motion {
            Motion::Loop | Motion::Once => self.anim_start.map(|t| t.elapsed()).unwrap_or_default(),
            Motion::OnHighlight => {
                if self.highlight_of == Some(at) {
                    self.highlight_since
                        .map(|t| t.elapsed())
                        .unwrap_or_default()
                } else {
                    std::time::Duration::ZERO
                }
            }
        }
    }

    /// Take the keyboard off the search box. iced's focus operation unfocuses
    /// every focusable it is not aiming at, so aiming it at an id no widget
    /// carries is how an input is blurred — otherwise a plain letter would
    /// still type into the box while the highlight was elsewhere.
    fn blur_search() -> Task<Message> {
        text_input::focus(cosmic::widget::Id::new("start-menu-nowhere"))
    }

    /// Put the highlight somewhere, or take it away. Gaining a highlight
    /// takes the keyboard off the search box; losing one hands it straight
    /// back, so Escape always leaves the menu ready to type into.
    fn set_nav(&mut self, spot: Option<keynav::Spot>) -> Task<Message> {
        let had = self.nav.is_some();
        self.nav = spot;
        self.refresh_highlight_clock();
        match (had, self.nav.is_some()) {
            (false, true) => Task::batch([Self::blur_search(), self.reveal_nav()]),
            (true, false) => text_input::focus(self.search_id.clone()),
            (true, true) => self.reveal_nav(),
            (false, false) => Task::none(),
        }
    }

    /// Pull the highlight back into range after the rows or tiles under it
    /// have changed — a folder closed by the mouse, a tile unpinned, the
    /// background reload landing. Clamped rather than dropped, so a Tab
    /// pressed while the load was still in flight is not silently undone and
    /// the next arrow key still moves one visible step; a zone with nothing
    /// left in it hands the keyboard back to the search box.
    fn renav(&mut self) -> Task<Message> {
        let counts = self.counts();
        let to = self.nav.and_then(|s| keynav::clamped(s, counts));
        self.set_nav(to)
    }

    /// Scroll the app list so the highlighted row is showing.
    fn reveal_nav(&mut self) -> Task<Message> {
        let Some(spot) = self.nav.filter(|s| s.zone == Zone::List) else {
            return Task::none();
        };
        let Some(stop) = self.list_stops().get(spot.index).copied() else {
            return Task::none();
        };
        match keynav::reveal(stop.y, stop.h, self.list_offset, self.list_view) {
            Some(y) => self.scroll_list(y),
            None => Task::none(),
        }
    }

    /// Enter on the highlight: the very message a click on it would send, so
    /// there is one way to launch an app, open a folder or fire a rail
    /// button however the user got there.
    fn activate(&mut self) -> Task<Message> {
        let Some(spot) = self.nav else {
            return Task::none();
        };
        match spot.zone {
            Zone::List => match self.list_stops().get(spot.index).map(|s| s.act) {
                Some(ui::app_list::Act::App(i)) => self.update(Message::Launch(i)),
                Some(ui::app_list::Act::Folder(i)) => self.update(Message::ToggleFolder(i)),
                Some(ui::app_list::Act::Settings) => self.update(Message::OpenSettings),
                None => Task::none(),
            },
            Zone::Tiles => {
                let Some(at) = self.tile_stops().0.get(spot.index).copied() else {
                    return Task::none();
                };
                if self.edit.on {
                    return self.update(Message::TileClicked(at));
                }
                match self
                    .config
                    .groups
                    .get(at.0)
                    .and_then(|g| g.tiles.get(at.1))
                    .map(|t| t.app.clone())
                {
                    Some(id) => self.update(Message::LaunchId(id)),
                    None => Task::none(),
                }
            }
            Zone::Rail => {
                match ui::rail::items(&self.config.system_panel)
                    .get(spot.index)
                    .copied()
                {
                    Some(item) => {
                        let msg = ui::rail::message(item, self.power_open);
                        self.update(msg)
                    }
                    None => Task::none(),
                }
            }
        }
    }

    /// One key the whole menu listens for, wherever the keyboard is.
    fn on_key(&mut self, key: Key) -> Task<Message> {
        match key {
            Key::Escape => self.on_escape(),
            Key::Tab | Key::ShiftTab => {
                // A modal layer refuses the zone switch — and takes the
                // keyboard back, because the press has already left the
                // focused input read-only.
                match nav_keys(self.context.is_some(), self.renaming.is_some()) {
                    NavKeys::HoldRename => return text_input::focus(ui::tiles::rename_input_id()),
                    NavKeys::HoldSearch => return text_input::focus(self.search_id.clone()),
                    NavKeys::Move => {}
                }
                let to = keynav::tab(self.nav, key == Key::ShiftTab, self.counts());
                if to.is_none() && self.nav.is_none() {
                    // Nowhere to go — mid-search, every zone is empty. The
                    // keyboard stays in the search box, but libcosmic's input
                    // has already stopped taking text: with no `on_tab`
                    // handler it marks itself read-only and does not consume
                    // the press. Focusing it again is what clears that; left
                    // alone the box took no letters and no backspace, and
                    // every uncaptured letter after it was swallowed as a
                    // first-letter jump.
                    return text_input::focus(self.search_id.clone());
                }
                self.set_nav(to)
            }
            Key::Up | Key::Down | Key::Left | Key::Right => {
                let dir = match key {
                    Key::Up => keynav::Dir::Up,
                    Key::Down => keynav::Dir::Down,
                    Key::Left => keynav::Dir::Left,
                    _ => keynav::Dir::Right,
                };
                let live = nav_keys_live(self.context.is_some(), self.renaming.is_some());
                let Some(spot) = self.nav else {
                    // Down with nothing typed is the other way out of the
                    // search box, without reaching for Tab: it lands on the
                    // first row of the list. With a query in the box the
                    // arrows still walk the results, as they always have —
                    // including behind a right-click menu, which is only
                    // allowed to refuse the highlight, not the search.
                    if live && self.query.trim().is_empty() && dir == keynav::Dir::Down {
                        return self.set_nav(keynav::tab(None, false, self.counts()));
                    }
                    return self.search_arrow(dir);
                };
                // Only the highlight is gated. An arrow never trips the
                // read-only trap, so nothing needs refocusing here.
                if !live {
                    return Task::none();
                }
                let counts = self.counts();
                let (_, cells) = self.tile_stops();
                let to = keynav::moved(spot, dir, counts, &cells);
                self.set_nav(Some(to))
            }
        }
    }

    /// With no highlight the arrows belong to the search results, as they
    /// always have; sideways means nothing there.
    fn search_arrow(&mut self, dir: keynav::Dir) -> Task<Message> {
        match dir {
            keynav::Dir::Down => {
                let total = crate::search::rank(&self.apps, &self.query).len() + self.found.len();
                self.selected = (self.selected + 1).min(total.saturating_sub(1));
            }
            keynav::Dir::Up => self.selected = self.selected.saturating_sub(1),
            keynav::Dir::Left | keynav::Dir::Right => {}
        }
        Task::none()
    }

    fn on_escape(&mut self) -> Task<Message> {
        let layers = Layers {
            highlight: self.nav.is_some(),
            context: self.context.is_some(),
            renaming: self.renaming.is_some(),
            power: self.power_open,
            mode_menu: self.mode_menu,
            right_menu: self.right_menu,
            letter_grid: self.letter_grid,
            picked: self.edit.picked.is_some(),
            query: !self.query.is_empty(),
            editing: self.edit.on,
        };
        match escape_target(layers) {
            Escape::DropHighlight => return self.set_nav(None),
            Escape::Context => self.context = None,
            Escape::CancelRename => self.renaming = None,
            Escape::Menus => {
                self.power_open = false;
                self.mode_menu = false;
                self.right_menu = false;
            }
            Escape::LetterGrid => self.letter_grid = false,
            Escape::DropPick => self.edit.picked = None,
            Escape::ClearSearch => {
                self.query.clear();
                self.selected = 0;
                self.found.clear();
                return text_input::focus(self.search_id.clone());
            }
            Escape::LeaveEdit => self.edit = ui::tiles::Edit::default(),
            Escape::ClosePopup => return self.close_popup(),
        }
        Task::none()
    }

    fn width(&self) -> f32 {
        popup_width(self.spacing(), self.config.tile_cells())
    }

    /// An installed app for a rail slot nobody configured: the desktop's
    /// default browser by xdg-settings, else the first app carrying the
    /// slot's freedesktop category.
    fn slot_fallback(&self, slot: Slot) -> Option<String> {
        if slot == Slot::Browser {
            if let Some(id) = crate::session::default_browser_id()
                .filter(|id| self.apps.iter().any(|a| &a.id == id))
            {
                return Some(id);
            }
        }
        let category = match slot {
            Slot::Browser => "WebBrowser",
            Slot::Files => "FileManager",
            Slot::Terminal => "TerminalEmulator",
            Slot::TaskManager => "Monitor",
        };
        self.apps
            .iter()
            .find(|a| a.categories.iter().any(|c| c == category))
            .map(|a| a.id.clone())
    }
}

impl Application for App {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = Mode;
    type Message = Message;

    const APP_ID: &'static str = "io.github.jjnuthuagen.StartMenu";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, mode: Mode) -> (Self, Task<Message>) {
        let mut core = core;
        if matches!(mode, Mode::Shortcut { .. }) {
            // libcosmic blurs every surface it does not track, as if it were
            // the main window, the moment it opens — a whole-surface blur
            // with square corners under the menu's rounded ones, before the
            // menu's own shaped region is asked for. Popups only: the menu
            // asks for its frost itself (see `blur`), and `frosted` still
            // reads the theme because the set is not empty.
            core.set_auto_blur(cosmic::core::Auto::Popup.into());
        }
        let mut app = Self {
            core,
            mode,
            fader: FaderState::Untried,
            revealed: false,
            anim_start: None,
            hover: None,
            highlight_since: None,
            highlight_of: None,
            closing: None,
            fade: None,
            fade_serial: 0,
            had_focus: false,
            focus_losses: 0,
            popup: None,
            opened_at: None,
            closed_by_focus: None,
            // Peeked, not loaded: the panel button's icon and the shortcut
            // surface's position are needed before the first full load, and
            // peek never seeds a missing file.
            config: Config::peek(),
            apps: Vec::new(),
            usage_top: Vec::new(),
            power_open: false,
            error: None,
            query: String::new(),
            selected: 0,
            search_id: cosmic::widget::Id::new("start-menu-search"),
            pointer: Point::ORIGIN,
            context: None,
            edit: ui::tiles::Edit::default(),
            letter_grid: false,
            list_id: cosmic::widget::Id::new("start-menu-list"),
            folders: Vec::new(),
            loose: Vec::new(),
            open_folders: std::collections::HashSet::new(),
            mode_menu: false,
            edit_gen: 0,
            pending: None,
            load_started: 0,
            load_newest: 0,
            favs: Vec::new(),
            recent: Vec::new(),
            right_menu: false,
            avatar: Avatar::default(),
            renaming: None,
            found: Vec::new(),
            nav: None,
            list_offset: 0.0,
            list_view: LIST_VIEWPORT,
        };
        let open = match mode {
            Mode::Panel => {
                // Start the menu process now, closed and invisible, so the
                // user's first press finds it already loaded. The applet is
                // exec'd once per session, so this runs once.
                crate::remote::prewarm();
                Task::none()
            }
            Mode::Shortcut { shown: true } => app.open(),
            // Pre-warmed: read everything now and wait for a press, so the
            // first open draws the real menu instead of a blank card.
            Mode::Shortcut { shown: false } => app.reload(),
        };
        (app, open)
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        // A surface fading out shows what it showed and does nothing else.
        // It still gets input: cosmic-comp leaves the keyboard with a layer
        // surface that gives up keyboard interactivity until the surface is
        // destroyed, so the keys typed right after a close arrive here, and
        // typing into its search box swapped the fading list for results.
        if self.popup.is_none() && self.closing.is_some() && message.is_input() {
            return Task::none();
        }
        match message {
            Message::TogglePopup => {
                tracing::debug!(
                    open = self.popup.is_some(),
                    seen = self.seen(),
                    "toggle received"
                );
                // The panel button does not draw the menu itself: it asks the
                // menu process to appear, or to go away if it is already up.
                // That process owns a name on the bus, so however the menu is
                // asked for — this button or the keyboard shortcut — there is
                // only ever one of it.
                if self.mode == Mode::Panel {
                    crate::remote::spawn_menu();
                    return Task::none();
                }
                if ignores_toggle(self.opened_at, self.seen(), std::time::Instant::now()) {
                    return Task::none();
                }
                if self.popup.is_some() {
                    return self.close_popup();
                }
                if is_the_closing_click(self.closed_by_focus, std::time::Instant::now()) {
                    self.closed_by_focus = None;
                    return Task::none();
                }
                // After the reopen guard, so the click that dismissed the menu
                // by taking the keyboard cannot bring it straight back.
                if let Some(id) = self.closing.take() {
                    return self.reverse_close(id);
                }
                self.open()
            }
            Message::SurfaceOpened(id) => {
                if self.popup != Some(id) || self.revealed {
                    return Task::none();
                }
                crate::fade::handles(id)
                    .map(move |handles| cosmic::action::app(Message::FadeHandles(id, handles)))
            }
            Message::FadeHandles(id, handles) => {
                if self.popup != Some(id) || self.revealed {
                    return Task::none();
                }
                let Some(handles) = handles else {
                    return self.reveal_now(id);
                };
                if matches!(self.fader, FaderState::Untried) {
                    self.fader = match crate::fade::Fader::connect(handles) {
                        Some(fader) => FaderState::Ready(Box::new(fader)),
                        None => {
                            tracing::info!(
                                "no wp_alpha_modifier_v1: the menu opens without a fade"
                            );
                            FaderState::Unavailable
                        }
                    };
                }
                let attached = match &mut self.fader {
                    FaderState::Ready(fader) => fader.attach(handles),
                    _ => false,
                };
                if !attached {
                    return self.reveal_now(id);
                }
                tracing::debug!(
                    after = ?self.opened_at.map(|t| t.elapsed()),
                    "the fade has hold of the menu"
                );
                // The view switches to the real menu now, drawn under an
                // opacity of nothing. Animations start their clock from this
                // instant — a GIF on `Once` plays through once per open from
                // the moment the user can see it.
                self.revealed = true;
                self.anim_start = Some(std::time::Instant::now());
                self.fade = Some(Fade {
                    surface: id,
                    motion: crate::motion::Motion::opening(),
                    last: None,
                    offset: crate::motion::SLIDE_PX,
                    frosted: false,
                });
                Task::batch([self.fade_step(false), self.focus_search()])
            }
            Message::RevealDeadline(id) => {
                if self.popup == Some(id) && !self.revealed {
                    return self.reveal_now(id);
                }
                Task::none()
            }
            Message::FadeFrame(serial) => {
                if serial != self.fade_serial {
                    return Task::none();
                }
                self.fade_step(true)
            }
            Message::Focused(id) => {
                tracing::debug!(?id, open = ?self.popup, "keyboard entered");
                // A surface fading out keeps its focus state up to date, for a
                // press that brings it back — but nothing else happens.
                if self.closing == Some(id) {
                    self.had_focus = true;
                    return Task::none();
                }
                // A surface already gone.
                if self.popup != Some(id) {
                    return Task::none();
                }
                self.had_focus = true;
                tracing::debug!("menu has the keyboard");
                let focus = text_input::focus(self.search_id.clone());
                // The surface asks for the keyboard outright so it has it the
                // moment it maps; held that way the compositor never takes it
                // back, so a click on another window raised no `Unfocused` and
                // the menu stayed open. Once focus is actually here, hand the
                // keyboard back to on-demand so clicking away loses it.
                //
                // The frost is the cold-start backstop here, and only once the
                // content shows: focus usually lands before the fade's first
                // visible step, and frost then would sit on an empty pane.
                let frost = if self.revealed && self.fade.is_none_or(|fade| fade.frosted) {
                    self.blur()
                } else {
                    Task::none()
                };
                Task::batch([focus, crate::shortcut::release_keyboard(id), frost])
            }
            Message::Unfocused(id) => {
                tracing::debug!(?id, open = ?self.popup, "keyboard left");
                if self.closing == Some(id) {
                    self.had_focus = false;
                    return Task::none();
                }
                if self.popup != Some(id) || !self.had_focus {
                    return Task::none();
                }
                // Not closed at once: focus also leaves and comes straight
                // back when a keyboard device appears (a virtual keyboard, a
                // layout switch), and that is not a click away.
                self.had_focus = false;
                self.focus_losses = self.focus_losses.wrapping_add(1);
                let loss = self.focus_losses;
                Task::perform(tokio::time::sleep(FOCUS_GRACE), move |()| {
                    cosmic::action::app(Message::FocusGone(loss))
                })
            }
            Message::FocusGone(loss) => {
                if !focus_loss_closes(
                    self.popup.is_some(),
                    self.had_focus,
                    self.focus_losses,
                    loss,
                ) {
                    return Task::none();
                }
                self.closed_by_focus = Some(std::time::Instant::now());
                self.close_popup()
            }
            Message::PopupClosed(id) => {
                // The compositor took the surface. The fade lets go of it
                // first; if the surface is already destroyed it lets go
                // without a word (see `fade::Fader::detach`).
                if self.popup == Some(id) {
                    self.popup = None;
                } else if self.closing == Some(id) {
                    self.closing = None;
                } else {
                    return Task::none();
                }
                if let FaderState::Ready(fader) = &mut self.fader {
                    fader.detach();
                }
                self.fade = None;
                self.revealed = false;
                self.reset_popup_state();
                Task::none()
            }
            Message::Loaded(loaded) => {
                let was_empty = self.apps.is_empty();
                let generation = loaded.generation;
                // A menu fading out is still on screen, and holds whatever it
                // shows — even the empty first card — until it is gone.
                let fading_out = self.closing.is_some();
                match on_load(
                    self.popup.is_some() || fading_out,
                    !was_empty || fading_out,
                    generation,
                    self.load_newest,
                ) {
                    // Older than what is shown or held: nothing changes.
                    OnLoad::Drop => Task::none(),
                    // Nothing on screen changes, so nothing about the
                    // highlight, focus or scroll position may either.
                    OnLoad::Stash => {
                        self.load_newest = generation;
                        self.pending = Some(loaded);
                        Task::none()
                    }
                    OnLoad::Apply => {
                        self.load_newest = generation;
                        // Anything still held back is older than this.
                        self.pending = None;
                        self.apply_loaded(*loaded, true);
                        // The highlight's indices came from the old, empty
                        // lists — but the reload lands a moment after the
                        // menu opens, so dropping it outright would undo a
                        // Tab pressed in between.
                        let nav = self.renav();
                        // An empty card had no search box for `open` to focus.
                        if was_empty && self.nav.is_none() && self.popup.is_some() {
                            return Task::batch([nav, text_input::focus(self.search_id.clone())]);
                        }
                        nav
                    }
                }
            }
            Message::Launch(i) => {
                let Some(app) = self.apps.get(i).cloned() else {
                    return Task::none();
                };
                let close = self.close_popup();
                let launch = Task::perform(crate::launch::app(app), |()| cosmic::action::none());
                Task::batch([close, launch])
            }
            Message::LaunchId(id) => match self.apps.iter().position(|a| a.id == id) {
                Some(i) => self.update(Message::Launch(i)),
                None => Task::none(),
            },
            Message::LetterGrid(open) => {
                self.letter_grid = open;
                // The grid replaces the list, so a highlight in it has
                // nothing left to sit on.
                if open {
                    return self.set_nav(None);
                }
                Task::none()
            }
            Message::JumpTo(letter) => {
                self.letter_grid = false;
                let pinned = self.pinned_rows();
                let (sections, prefix) = match self.config.list_mode {
                    ListMode::Folders if !self.folders.is_empty() => (
                        crate::apps::sections_of_excluding(&self.apps, &self.loose, &pinned),
                        self.most_used_height(&pinned) + self.folders_height(),
                    ),
                    _ => (
                        crate::apps::sections_excluding(&self.apps, &pinned),
                        self.most_used_height(&pinned),
                    ),
                };
                self.scroll_list(
                    prefix
                        + ui::app_list::offset_of(
                            &sections,
                            0,
                            &letter,
                            ui::ROW_HEIGHT,
                            ui::HEADER_HEIGHT,
                        ),
                )
            }
            Message::JumpToCategory(key) => {
                self.letter_grid = false;
                let pinned = self.pinned_rows();
                let y = self.most_used_height(&pinned)
                    + ui::app_list::offset_of(
                        &crate::apps::category_sections_excluding(&self.apps, &pinned),
                        0,
                        &key,
                        ui::ROW_HEIGHT,
                        ui::HEADER_HEIGHT,
                    );
                self.scroll_list(y)
            }
            Message::ModeMenu(open) => {
                self.mode_menu = open;
                Task::none()
            }
            Message::SetListMode(mode) => {
                self.mode_menu = false;
                self.letter_grid = false;
                // A different sort is a different list of rows.
                let drop = self.set_nav(None);
                let save = self.edit(move |c| c.list_mode = mode);
                let top = self.scroll_list(0.0);
                Task::batch([drop, save, top])
            }
            Message::RightMenu(open) => {
                self.right_menu = open;
                Task::none()
            }
            Message::SetRightSide(side) => {
                self.right_menu = false;
                if side != RightSide::Tiles {
                    self.edit = ui::tiles::Edit::default();
                }
                // A different right column has different tiles, or none.
                let drop = self.set_nav(None);
                Task::batch([drop, self.edit(move |c| c.right_side = side)])
            }
            Message::AddFavourite(id) => {
                let list = crate::favorites::with_added(&self.favs, &id);
                self.save_favourites(list)
            }
            Message::RemoveFavourite(id) => {
                let list = crate::favorites::with_removed(&self.favs, &id);
                self.save_favourites(list)
            }
            Message::ConfigSaved(generation, saved) => {
                if generation == self.edit_gen {
                    self.config = *saved;
                }
                // The config just came back from disk, where the Settings
                // window may have deleted the group the highlight was in.
                self.renav()
            }
            Message::FavouritesSaved(list) => {
                self.favs = list;
                Task::none()
            }
            Message::ToggleFolder(i) => {
                if !self.open_folders.remove(&i) {
                    self.open_folders.insert(i);
                }
                // Closing a folder takes its rows out of the list under the
                // highlight.
                self.renav()
            }
            Message::PowerMenu(open) => {
                self.power_open = open;
                Task::none()
            }
            Message::Power(p) => {
                self.power_open = false;
                Task::perform(crate::session::run(p), move |r| match r {
                    Ok(()) => cosmic::action::none(),
                    Err(e) => cosmic::action::app(Message::ShowError(fl!(
                        "power-failed",
                        action = fl!(p.l10n_key()),
                        error = e
                    ))),
                })
            }
            Message::Query(q) => {
                let was_searching = !self.query.trim().is_empty();
                self.query = q;
                self.selected = 0;
                // Typing is the search box's; the highlight belongs to the
                // browse layout the results have just replaced.
                self.nav = None;
                self.ask_launcher();
                // Starting or clearing a search swaps the whole layout under
                // the box: results take the list and tile columns together,
                // so the input sits at a different place in the widget tree
                // and iced builds it fresh there — keyboard focus stays with
                // the state it left behind. That is why the box went dead
                // after the first letter, and why Enter, which only the
                // focused input reports, never launched anything.
                if was_searching != !self.query.trim().is_empty() {
                    return Task::batch([
                        text_input::focus(self.search_id.clone()),
                        text_input::move_cursor_to_end(self.search_id.clone()),
                    ]);
                }
                Task::none()
            }
            Message::Launcher(reply) => match reply {
                // A late answer to a query since cleared would bring back
                // results for text no longer in the box.
                crate::launcher::Reply::Results(mut items) => {
                    if !self.query.trim().is_empty() {
                        items.retain(|i| self.config.keeps(i.section));
                        self.found = items;
                    }
                    Task::none()
                }
                crate::launcher::Reply::Fill(text) => {
                    self.query = text;
                    self.selected = 0;
                    self.ask_launcher();
                    text_input::move_cursor_to_end(self.search_id.clone())
                }
                crate::launcher::Reply::Close => self.close_popup(),
            },
            Message::LauncherActivate(id) => {
                crate::launcher::activate(id);
                Task::none()
            }
            Message::KeyPress(key) => self.on_key(key),
            Message::Letter(c) => {
                // Only once the highlight has reached the list: while the
                // search box has the keyboard a letter is still a letter.
                if self.focus_in(Zone::List).is_none()
                    || !nav_keys_live(self.context.is_some(), self.renaming.is_some())
                {
                    return Task::none();
                }
                let letter = crate::apps::letter(&c.to_string());
                let lines = ui::app_list::plan(&self.list_view());
                let Some(stop) = ui::app_list::stop_at_letter(&lines, letter) else {
                    return Task::none();
                };
                let jump = self.update(Message::JumpTo(letter));
                // The highlight goes with the list, so the next arrow key
                // carries on from the section he asked for rather than
                // scrolling straight back to where he was.
                self.nav = Some(keynav::Spot {
                    zone: Zone::List,
                    index: stop,
                });
                jump
            }
            Message::ListScrolled { offset, view } => {
                self.list_offset = offset;
                self.list_view = view;
                Task::none()
            }
            Message::Submit => {
                // Enter on the highlight does what clicking it would —
                // unless a right-click menu or a rename draft is on top of
                // it, which owns the key.
                if self.nav.is_some() {
                    if !nav_keys_live(self.context.is_some(), self.renaming.is_some()) {
                        return Task::none();
                    }
                    return self.activate();
                }
                // Nothing to launch with an empty box, whatever has focus.
                if self.query.trim().is_empty() {
                    return Task::none();
                }
                let hits = crate::search::rank(&self.apps, &self.query);
                match pick(hits.len(), self.found.len(), self.selected) {
                    Some(Pick::App(n)) => self.update(Message::Launch(hits[n])),
                    Some(Pick::Found(n)) => {
                        self.update(Message::LauncherActivate(self.found[n].id))
                    }
                    None => Task::none(),
                }
            }
            Message::RenameTile(at) => {
                self.context = None;
                let current = self
                    .config
                    .groups
                    .get(at.0)
                    .and_then(|g| g.tiles.get(at.1))
                    .map(|t| {
                        t.label.clone().unwrap_or_else(|| {
                            self.apps
                                .iter()
                                .find(|a| a.id == t.app)
                                .map(|a| a.name.clone())
                                .unwrap_or_default()
                        })
                    })
                    .unwrap_or_default();
                self.renaming = Some((at, TileField::Label, current));
                text_input::focus(ui::tiles::rename_input_id())
            }
            Message::EditTileImage(at) => {
                self.context = None;
                let current = self
                    .config
                    .groups
                    .get(at.0)
                    .and_then(|g| g.tiles.get(at.1))
                    .and_then(|t| t.image.clone())
                    .unwrap_or_default();
                self.renaming = Some((at, TileField::Image, current));
                text_input::focus(ui::tiles::rename_input_id())
            }
            Message::RenameText(t) => {
                if let Some((_, _, draft)) = &mut self.renaming {
                    *draft = t;
                }
                Task::none()
            }
            Message::RenameDone => {
                let Some((at, field, text)) = self.renaming.take() else {
                    return Task::none();
                };
                match field {
                    TileField::Label => self.edit(move |c| c.rename(at, &text)),
                    TileField::Image => self.edit(move |c| c.set_tile_image(at, &text)),
                }
            }
            Message::TileColor(at, color) => {
                self.edit(move |c| c.set_tile_color(at, color.clone()))
            }
            Message::ClearTileImage(at) => self.edit(move |c| c.set_tile_image(at, "")),
            Message::SetTileMotion(at, motion) => self.edit(move |c| c.set_tile_motion(at, motion)),
            Message::SetTileIconOverImage(at, on) => {
                self.edit(move |c| c.set_tile_icon_over_image(at, on))
            }
            Message::TileHover(at) => {
                if self.hover != at {
                    self.hover = at;
                    self.refresh_highlight_clock();
                }
                Task::none()
            }
            Message::AnimTick => Task::none(),
            Message::TileNumber(n) => {
                let installed: std::collections::HashSet<&str> =
                    self.apps.iter().map(|a| a.id.as_str()).collect();
                match self.config.nth_tile(&installed, n) {
                    Some(id) => {
                        let id = id.to_owned();
                        self.update(Message::LaunchId(id))
                    }
                    None => Task::none(),
                }
            }
            Message::Pointer(p) => {
                self.pointer = p;
                Task::none()
            }
            Message::OpenContext(target) => {
                self.power_open = false;
                self.context = Some(Context {
                    target,
                    at: self.pointer,
                });
                Task::none()
            }
            Message::CloseContext => {
                self.context = None;
                Task::none()
            }
            Message::Pin(id) => self.edit(move |c| c.pin(&id)),
            Message::Unpin(id) => self.edit(move |c| c.unpin(&id)),
            Message::Resize(r, size) => self.edit(move |c| c.resize(r, size)),
            Message::MoveToGroup(r, g) => self.edit(move |c| c.move_tile(r, g, usize::MAX)),
            Message::NewGroupWith(r) => self.edit(move |c| {
                let g = c.add_group(fl!("new-group-name"));
                c.move_tile(r, g, 0);
            }),
            Message::RunAction(i, k) => {
                let Some(app) = self.apps.get(i).cloned() else {
                    return Task::none();
                };
                let Some(action) = app.actions.get(k).cloned() else {
                    return Task::none();
                };
                let close = self.close_popup();
                let run =
                    Task::perform(crate::launch::action(app.id, action, app.terminal), |()| {
                        cosmic::action::none()
                    });
                Task::batch([close, run])
            }
            Message::ToggleEdit => {
                // Every change in edit mode is saved as it happens, so
                // leaving it has nothing left to write.
                self.edit = ui::tiles::Edit {
                    on: !self.edit.on,
                    picked: None,
                };
                Task::none()
            }
            Message::TileClicked(at) => match self.edit.picked {
                None => {
                    self.edit.picked = Some(at);
                    Task::none()
                }
                Some(from) if from == at => {
                    self.edit.picked = None;
                    Task::none()
                }
                Some(from) => {
                    self.edit.picked = None;
                    self.edit(move |c| c.move_tile(from, at.0, at.1))
                }
            },
            Message::DropEnd(g) => match self.edit.picked.take() {
                Some(from) => self.edit(move |c| c.move_tile(from, g, usize::MAX)),
                None => Task::none(),
            },
            Message::DropNew => match self.edit.picked.take() {
                Some(from) => self.edit(move |c| {
                    let g = c.add_group(fl!("new-group-name"));
                    c.move_tile(from, g, 0);
                }),
                None => Task::none(),
            },
            Message::AddGroup => self.edit(move |c| {
                c.add_group(fl!("new-group-name"));
            }),
            Message::RenameGroup(g, name) => self.edit(move |c| c.rename_group(g, name.clone())),
            Message::RemoveGroup(g) => {
                // Group indices shift, so a picked tile's reference would
                // point at the wrong tile.
                self.edit.picked = None;
                self.edit(move |c| c.remove_group(g))
            }
            Message::OpenSettings => {
                crate::settings::open_window();
                // A window over the popup would leave it orphaned beneath.
                self.close_popup()
            }
            Message::ShowError(e) => {
                self.error = Some(e);
                Task::none()
            }
            Message::OpenSlot(slot) => {
                // The configured app when it is still installed; otherwise a
                // sensible stand-in, so the button always does something.
                let configured = self
                    .config
                    .system_panel
                    .app(slot)
                    .filter(|id| self.apps.iter().any(|a| &a.id == id))
                    .map(str::to_owned);
                if let Some(id) = configured {
                    return self.update(Message::LaunchId(id));
                }
                if let Some(id) = self.slot_fallback(slot) {
                    return self.update(Message::LaunchId(id));
                }
                match slot {
                    // The old Files behaviour: home in whatever xdg says.
                    Slot::Files => {
                        let r = crate::session::open_files();
                        self.report(r);
                        self.close_popup()
                    }
                    _ => {
                        self.error = Some(fl!("no-default-app"));
                        Task::none()
                    }
                }
            }
            Message::OpenSettingsApp => {
                let r = crate::session::open_settings_page(None);
                self.report(r);
                self.close_popup()
            }
            Message::OpenAccount => {
                let r = crate::session::open_settings_page(Some("users"));
                self.report(r);
                self.close_popup()
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        // A second shortcut press, asking this shortcut menu to close.
        let remote = Subscription::run_with((), |()| {
            futures::stream::unfold(crate::remote::requests(), |requests| async move {
                let mut receiver = requests?;
                receiver
                    .recv()
                    .await
                    .map(|()| (Message::TogglePopup, Some(receiver)))
            })
        });
        let launcher = Subscription::run_with((), |()| {
            futures::stream::unfold(crate::launcher::receiver(), |replies| async move {
                let mut receiver = replies?;
                receiver
                    .recv()
                    .await
                    .map(|reply| (Message::Launcher(reply), Some(receiver)))
            })
        });
        // The fade's frame callbacks, whenever a surface is animating —
        // including one fading out after the menu has closed.
        let frames = Subscription::run_with((), |()| {
            futures::stream::unfold(crate::fade::frame_callbacks(), |frames| async move {
                let mut receiver = frames?;
                receiver
                    .recv()
                    .await
                    .map(|serial| (Message::FadeFrame(serial), Some(receiver)))
            })
        });
        let remote = Subscription::batch([remote, launcher, frames]);
        // Focus is listened for while any surface exists, the fading one
        // included: a press during the fade-out hands that surface the
        // keyboard back, and a listener started only then could miss the
        // focus arriving.
        let focus = cosmic::iced::event::listen_with(|event, _status, id| match event {
            cosmic::iced::Event::Window(window::Event::Focused) => Some(Message::Focused(id)),
            cosmic::iced::Event::Window(window::Event::Unfocused) => Some(Message::Unfocused(id)),
            // The surface exists now, so its handles can be read.
            cosmic::iced::Event::Window(window::Event::Opened { .. }) => {
                Some(Message::SurfaceOpened(id))
            }
            // A layer surface loses the keyboard as a Wayland event, not a
            // window one: iced turns a keyboard leave into
            // `window::Event::Unfocused` for ordinary windows only. Watching
            // for the window event alone was why clicking another window
            // never closed the menu, however long you waited.
            cosmic::iced::Event::PlatformSpecific(
                cosmic::iced::event::PlatformSpecific::Wayland(
                    // The event names its own surface; trust that over the
                    // window it was routed through.
                    cosmic::iced::event::wayland::Event::Layer(layer, _, id),
                ),
            ) => match layer {
                cosmic::iced::event::wayland::LayerEvent::Focused => Some(Message::Focused(id)),
                cosmic::iced::event::wayland::LayerEvent::Unfocused => Some(Message::Unfocused(id)),
                cosmic::iced::event::wayland::LayerEvent::Done => Some(Message::Unfocused(id)),
            },
            _ => None,
        });
        if self.popup.is_none() {
            if self.closing.is_some() {
                return Subscription::batch([remote, focus]);
            }
            return remote;
        }
        // While the menu is on screen, tick at ~30Hz so animated tiles redraw.
        // Any message is enough — iced re-runs `view`, which picks the current
        // GIF frame from `tile_elapsed`. `tilemotion::is_animating` keeps still
        // tiles from redrawing beyond what iced already dedupes. We let the
        // tick run unconditionally while the popup is up: a sleeping tile
        // produces the same Handle each frame, so view diffs it to nothing.
        let anim = cosmic::iced::time::every(std::time::Duration::from_millis(33))
            .map(|_| Message::AnimTick);
        let keys = cosmic::iced::event::listen_with(|event, status, _id| match event {
            cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                key: KeyCode::Named(named),
                modifiers,
                ..
            }) => match named {
                Named::ArrowUp => Some(Message::KeyPress(Key::Up)),
                Named::ArrowDown => Some(Message::KeyPress(Key::Down)),
                // Taken whatever consumed them: with the keyboard still in
                // the search box the input moves its cursor with these, and
                // we ignore them unless a highlight is up.
                Named::ArrowLeft => Some(Message::KeyPress(Key::Left)),
                Named::ArrowRight => Some(Message::KeyPress(Key::Right)),
                Named::Escape => Some(Message::KeyPress(Key::Escape)),
                // The search input does not consume Tab — without an
                // `on_tab` handler it only stops taking text — so the zone
                // switch is ours to make either way.
                Named::Tab => Some(Message::KeyPress(if modifiers.shift() {
                    Key::ShiftTab
                } else {
                    Key::Tab
                })),
                // The search box submits for itself while it holds the
                // keyboard focus; the arrow keys are picked up here instead,
                // and iced hands a key to the focused widget only — so after
                // arrowing into the list, Enter had nowhere to go and the
                // highlighted row could not be launched. Taken here only when
                // no widget consumed the press, so a launch never fires twice.
                Named::Enter if status == cosmic::iced::event::Status::Ignored => {
                    Some(Message::Submit)
                }
                _ => None,
            },
            // Ctrl+1..9 launches a pinned tile. Ctrl, because bare digits
            // belong to the search box ("0 A.D." is a real query).
            cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                key: KeyCode::Character(c),
                modifiers,
                ..
            }) if modifiers.control() => c
                .chars()
                .next()
                .and_then(|d| d.to_digit(10))
                .filter(|d| (1..=9).contains(d))
                .map(|d| Message::TileNumber(d as usize)),
            // A plain letter once the keyboard has left the search box: a
            // jump to that letter's section. Only when nothing consumed the
            // press, so typing into the box is untouched.
            cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                key: KeyCode::Character(c),
                modifiers,
                ..
            }) if !modifiers.control()
                && !modifiers.alt()
                && !modifiers.logo()
                && status == cosmic::iced::event::Status::Ignored =>
            {
                c.chars()
                    .next()
                    .filter(|c| c.is_alphanumeric())
                    .map(Message::Letter)
            }
            _ => None,
        });
        Subscription::batch([remote, keys, focus, anim])
    }

    fn view(&self) -> Element<'_, Message> {
        // No right-click menu: a missed left-click kept opening Settings,
        // which now sits as the last row of the app list instead.
        self.core
            .applet
            .icon_button(&self.config.panel_icon)
            .on_press(Message::TogglePopup)
            .into()
    }

    fn view_window(&self, id: Id) -> Element<'_, Message> {
        // Nothing at all until the fade has hold of the surface: a frame drawn
        // before then could not be hidden, and the very first frame of a new
        // surface sometimes came out sheared besides.
        if self.popup == Some(id) && !self.revealed {
            return cosmic::widget::Space::new()
                .width(Length::Fixed(self.width()))
                .height(Length::Fixed(POPUP_HEIGHT))
                .into();
        }
        // The first open of a fresh process has no apps yet: the load runs
        // off-thread and lands a moment later. Drawing the real layout now
        // would show every tile group as an "empty group" caption over a
        // one-row list, and the whole menu would visibly jump as the data
        // arrived. An empty card simply fills in. With the process resident
        // this is seen once per session at most.
        //
        // A load that failed, though, leaves the card empty for good, so the
        // error goes on it: a featureless card with nothing to explain it
        // would be the end of the road.
        if self.apps.is_empty() {
            let body: Element<'_, Message> = match &self.error {
                Some(e) => container(text::body(e.clone()))
                    .padding(24)
                    .width(Length::Fixed(self.width()))
                    .height(Length::Fixed(POPUP_HEIGHT))
                    .into(),
                None => cosmic::widget::Space::new()
                    .width(Length::Fixed(self.width()))
                    .height(Length::Fixed(POPUP_HEIGHT))
                    .into(),
            };
            return container(body)
                .class(cosmic::theme::Container::custom(
                    crate::shortcut::card_style,
                ))
                .into();
        }
        let spacing = self.spacing();
        let search = text_input::search_input(fl!("search-placeholder"), &self.query)
            .id(self.search_id.clone())
            .style(ui::search_input_class())
            .on_input(Message::Query)
            .on_submit(|_| Message::Submit);

        // While searching, results take the list and tile columns together.
        let searching = !self.query.trim().is_empty();
        let main: Element<'_, Message> = if searching {
            let hits = crate::search::rank(&self.apps, &self.query);
            let selected = self
                .selected
                .min((hits.len() + self.found.len()).saturating_sub(1));
            column::with_children(vec![
                search.into(),
                ui::app_list::results_view(&self.apps, &hits, &self.found, selected, &self.query),
            ])
            .spacing(spacing.section)
            .width(Length::Fill)
            .into()
        } else {
            row::with_children(vec![
                column::with_children(vec![
                    search.width(Length::Fixed(ui::LIST_WIDTH)).into(),
                    ui::app_list::list_bar(
                        self.config.list_mode,
                        self.mode_menu,
                        self.config.locked,
                    ),
                    if self.letter_grid && self.config.list_mode == ListMode::Category {
                        let pinned = self.pinned_rows();
                        let present: Vec<&'static str> =
                            crate::apps::category_sections_excluding(&self.apps, &pinned)
                                .into_iter()
                                .map(|(k, _)| k)
                                .collect();
                        ui::app_list::category_grid(&present)
                    } else if self.letter_grid {
                        let pinned = self.pinned_rows();
                        let sections = match self.config.list_mode {
                            ListMode::Folders if !self.folders.is_empty() => {
                                crate::apps::sections_of_excluding(&self.apps, &self.loose, &pinned)
                            }
                            _ => crate::apps::sections_excluding(&self.apps, &pinned),
                        };
                        let present: Vec<char> = sections.into_iter().map(|(c, _)| c).collect();
                        ui::app_list::letter_grid(&present)
                    } else {
                        ui::app_list::view(self.list_view(), self.focus_in(Zone::List))
                    },
                ])
                .spacing(spacing.section)
                .into(),
                ui::tiles::view(
                    ui::tiles::RightView {
                        config: &self.config,
                        apps: &self.apps,
                        spacing,
                        edit: self.edit,
                        favs: &self.favs,
                        recent: &self.recent,
                        menu_open: self.right_menu,
                        renaming: self.renaming.as_ref(),
                        focus: self
                            .focus_in(Zone::Tiles)
                            .and_then(|n| self.tile_stops().0.get(n).copied()),
                        radius: ui::tile_radius(self.core.system_theme()),
                        menu_visible: self.revealed && self.closing.is_none(),
                        hover: self.hover,
                    },
                    |at, motion| self.tile_elapsed(at, motion),
                ),
            ])
            .spacing(COLUMN_GAP)
            .into()
        };
        let columns = row::with_children(vec![
            ui::rail::view(
                self.power_open,
                &self.avatar,
                &self.config.system_panel,
                self.focus_in(Zone::Rail),
                spacing.section,
            ),
            main,
        ])
        .spacing(COLUMN_GAP)
        .height(Length::Fill);

        let mut body = column::with_capacity(2).push(columns);
        if let Some(e) = &self.error {
            // The card's left inset now belongs to the rail, so anything
            // else in the body has to carry its own.
            body =
                body.push(container(text::caption(e.clone())).padding([0, 0, 0, spacing.section]));
        }

        // No left padding: the rail owns it, and centres its glyphs across
        // the whole strip from the card's edge to the hairline.
        let body = container(body.spacing(spacing.gap))
            .padding([spacing.section, spacing.section, spacing.section, 0])
            .width(Length::Fixed(self.width()))
            .height(Length::Fixed(POPUP_HEIGHT));

        // The pointer is tracked over the same box the menu is positioned
        // in, so the menu opens exactly where the right-click was.
        let tracked = mouse_area(body).on_move(Message::Pointer);
        let mut with_menu = popover(tracked).on_close(Message::CloseContext);
        if let Some(ctx) = &self.context {
            if let Some(menu) = ui::context::view(ctx, &self.apps, &self.config, &self.favs) {
                with_menu = with_menu
                    .popup(menu)
                    .position(popover::Position::Point(ctx.at));
            }
        }
        container(with_menu)
            .class(cosmic::theme::Container::custom(
                crate::shortcut::card_style,
            ))
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layers() -> Layers {
        Layers::default()
    }

    #[test]
    fn escape_closes_the_topmost_thing_first() {
        let all = Layers {
            highlight: false,
            context: true,
            renaming: true,
            power: true,
            mode_menu: true,
            right_menu: true,
            letter_grid: true,
            picked: true,
            query: true,
            editing: true,
        };
        assert_eq!(escape_target(all), Escape::Context);
        assert_eq!(
            escape_target(Layers {
                context: false,
                renaming: false,
                ..all
            }),
            Escape::Menus
        );
        assert_eq!(
            escape_target(Layers {
                power: true,
                ..layers()
            }),
            Escape::Menus
        );
        assert_eq!(
            escape_target(Layers {
                letter_grid: true,
                query: true,
                ..layers()
            }),
            Escape::LetterGrid
        );
        assert_eq!(
            escape_target(Layers {
                picked: true,
                query: true,
                ..layers()
            }),
            Escape::DropPick
        );
        assert_eq!(
            escape_target(Layers {
                query: true,
                ..layers()
            }),
            Escape::ClearSearch
        );
    }

    #[test]
    fn a_refused_navigation_key_hands_the_keyboard_back() {
        // Nothing on top: the key moves the highlight.
        assert_eq!(nav_keys(false, false), NavKeys::Move);
        // A rename draft is the one that must get the keyboard back, or Tab
        // leaves it read-only with no way in but Escape, which discards it.
        assert_eq!(nav_keys(false, true), NavKeys::HoldRename);
        assert_eq!(nav_keys(true, true), NavKeys::HoldRename);
        // A right-click menu with no draft open: back to the search box,
        // which Tab has just left read-only too.
        assert_eq!(nav_keys(true, false), NavKeys::HoldSearch);
    }

    #[test]
    fn a_menu_or_a_rename_on_top_owns_the_navigation_keys() {
        assert!(nav_keys_live(false, false));
        // A right-click menu: Enter must act on what was right-clicked, not
        // launch the row the highlight happens to be on behind it.
        assert!(!nav_keys_live(true, false));
        // A rename draft: Tab would blur the input and leave the draft
        // untypable.
        assert!(!nav_keys_live(false, true));
        assert!(!nav_keys_live(true, true));
    }

    #[test]
    fn escape_drops_the_highlight_before_anything_else() {
        // The top rung: one press puts the keyboard back in the search box,
        // whatever else is open behind it.
        assert_eq!(
            escape_target(Layers {
                highlight: true,
                context: true,
                renaming: true,
                power: true,
                letter_grid: true,
                picked: true,
                query: true,
                editing: true,
                ..layers()
            }),
            Escape::DropHighlight
        );
        // And with nothing else open it still only drops the highlight —
        // Escape never closes the menu while something is highlighted.
        assert_eq!(
            escape_target(Layers {
                highlight: true,
                ..layers()
            }),
            Escape::DropHighlight
        );
    }

    #[test]
    fn escape_leaves_edit_mode_after_clearing_a_search() {
        assert_eq!(
            escape_target(Layers {
                editing: true,
                ..layers()
            }),
            Escape::LeaveEdit
        );
    }

    #[test]
    fn a_fading_menu_ignores_input_but_not_its_own_machinery() {
        // What a key, a click or a scroll on the fading surface sends.
        for input in [
            Message::Query("fi".into()),
            Message::Submit,
            Message::KeyPress(Key::Down),
            Message::Letter('f'),
            Message::Launch(0),
            Message::LaunchId("firefox.desktop".into()),
            Message::ToggleFolder(0),
            Message::ListScrolled {
                offset: 0.0,
                view: 100.0,
            },
        ] {
            assert!(input.is_input(), "{input:?}");
        }
        // What drives the fade itself, and the press that turns it round.
        for machinery in [
            Message::TogglePopup,
            Message::FadeFrame(3),
            Message::RevealDeadline(Id::RESERVED),
            Message::Focused(Id::RESERVED),
            Message::Unfocused(Id::RESERVED),
            Message::PopupClosed(Id::RESERVED),
            Message::FocusGone(1),
        ] {
            assert!(!machinery.is_input(), "{machinery:?}");
        }
    }

    #[test]
    fn a_reload_never_changes_a_populated_open_menu() {
        // A fresh result (generation 2 over 1 already shown).
        // Open and already drawn from real data: held for the next open,
        // or the list jumps under the user.
        assert_eq!(on_load(true, true, 2, 1), OnLoad::Stash);
        // The empty first card: nothing to jump from, fill it in.
        assert_eq!(on_load(true, false, 2, 1), OnLoad::Apply);
        // Closed — the pre-warm, or a reload that finished after close.
        assert_eq!(on_load(false, true, 2, 1), OnLoad::Apply);
        assert_eq!(on_load(false, false, 1, 0), OnLoad::Apply);
    }

    #[test]
    fn an_older_load_never_lands_on_newer_data() {
        // The login race: the pre-warm's load (1) is still running when a
        // press starts another (2). Load 2 finishes first and fills the
        // empty card…
        assert_eq!(on_load(true, false, 2, 0), OnLoad::Apply);
        // …so load 1, landing late on the populated menu, is dropped rather
        // than held and applied over newer data at the next open.
        assert_eq!(on_load(true, true, 1, 2), OnLoad::Drop);
        // Whatever the menu is doing, and even for a repeat of the same one.
        assert_eq!(on_load(false, true, 1, 2), OnLoad::Drop);
        assert_eq!(on_load(false, false, 2, 2), OnLoad::Drop);
        // A newer one still goes through as usual.
        assert_eq!(on_load(true, true, 3, 2), OnLoad::Stash);
    }

    #[test]
    fn a_toggle_cannot_close_a_menu_that_has_not_appeared_yet() {
        let now = std::time::Instant::now();
        let just_now = Some(now - std::time::Duration::from_millis(50));
        // Both triggers spawn a copy that pokes the running menu, so a
        // second press landing while the surface is still mapping used to
        // close a menu nobody had seen — any even number of fast presses
        // left nothing on screen.
        assert!(ignores_toggle(just_now, false, now));
        // Once it has the keyboard it has been seen, so a press dismisses it.
        assert!(!ignores_toggle(just_now, true, now));
        // Nothing open: this is a plain open, whatever focus has done.
        assert!(!ignores_toggle(None, false, now));
        assert!(!ignores_toggle(None, true, now));
    }

    #[test]
    fn a_surface_that_never_appears_can_still_be_dismissed() {
        let now = std::time::Instant::now();
        // A surface destroyed and recreated in one batch came up without
        // focus; an unguarded wait on the focus flag then ignored every
        // press and the menu could not be closed at all.
        let long_ago = Some(now - std::time::Duration::from_secs(5));
        assert!(!ignores_toggle(long_ago, false, now));
        // Just inside the window it is still the tail of the opening press.
        let edge = Some(now - (OPEN_GUARD - std::time::Duration::from_millis(1)));
        assert!(ignores_toggle(edge, false, now));
    }

    #[test]
    fn only_a_focus_loss_that_still_has_a_menu_arms_the_reopen_guard() {
        // The ordinary case: the menu is up, focus has not come back, and
        // this is the newest loss.
        assert!(focus_loss_closes(true, false, 3, 3));
        // The menu went by some other route — a toggle, Escape, a launch —
        // while the check was in flight. Arming the guard here swallowed the
        // press *after* the one that closed the menu.
        assert!(!focus_loss_closes(false, false, 3, 3));
        // Focus came back: a keyboard device appearing, not a click away.
        assert!(!focus_loss_closes(true, true, 3, 3));
        // A stale check from an earlier loss.
        assert!(!focus_loss_closes(true, false, 4, 3));
    }

    #[test]
    fn the_click_that_closed_the_menu_cannot_reopen_it() {
        let now = std::time::Instant::now();
        // The panel button's press closes the menu by moving the keyboard,
        // and the toggle it sends lands a moment later.
        assert!(is_the_closing_click(
            Some(now - std::time::Duration::from_millis(50)),
            now
        ));
        // A press a second later is someone asking for the menu again.
        assert!(!is_the_closing_click(
            Some(now - std::time::Duration::from_secs(2)),
            now
        ));
        // Nothing closed it: this is a plain open.
        assert!(!is_the_closing_click(None, now));
    }

    /// The region as the compositor receives it: libcosmic's `apply_blur`
    /// sends each field through `.round() as i32` on its own. The old test
    /// checked the exact geometry, which was right, and so could not see the
    /// rows that rounding opened up.
    fn as_sent(rects: &[cosmic::iced::Rectangle]) -> Vec<(i32, i32, i32, i32)> {
        rects
            .iter()
            .map(|r| {
                (
                    r.x.round() as i32,
                    r.y.round() as i32,
                    r.width.round() as i32,
                    r.height.round() as i32,
                )
            })
            .collect()
    }

    fn covered(rects: &[(i32, i32, i32, i32)], px: i32, py: i32) -> bool {
        rects
            .iter()
            .any(|&(x, y, w, h)| px >= x && px < x + w && py >= y && py < y + h)
    }

    /// Whether a pixel's centre is inside a `w`x`h` frame rounded by `r`.
    fn inside_frame(px: i32, py: i32, w: f32, h: f32, r: f32) -> bool {
        let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
        // Fold every corner onto the top-left one.
        let fx = cx.min(w - cx);
        let fy = cy.min(h - cy);
        if fx >= r || fy >= r {
            return true;
        }
        let (dx, dy) = (r - fx, r - fy);
        dx * dx + dy * dy <= r * r
    }

    #[test]
    fn blur_stays_inside_the_rounded_corners_once_rounded() {
        // Every built-in roundness, and a fractional radius and width the
        // theme or the density setting could produce.
        for (w, h, radius) in [
            (700.0, 600.0, 0.0),
            (700.0, 600.0, 2.0),
            (700.0, 600.0, 4.0),
            (700.0, 600.0, 8.0),
            (700.0, 600.0, 12.0),
            (700.0, 600.0, 16.0),
            (700.4, 600.0, 10.5),
        ] {
            let sent = as_sent(&blur_region(w, h, radius));
            let (wi, hi) = (w as i32, h as i32);
            let r = radius;
            let corner = r.ceil() as i32;
            for py in 0..hi {
                for px in 0..wi {
                    let on = covered(&sent, px, py);
                    // Never outside the curve, where sharp background would
                    // show beyond the card's rounded edge.
                    if on {
                        assert!(
                            inside_frame(px, py, w.floor(), h, r),
                            "r={radius}: ({px},{py}) is blurred outside the arc"
                        );
                    }
                }
            }
            // Never a row left out inside the curve: the corner's innermost
            // column is inside the arc on every row below the outermost band
            // (which the arc makes zero-width by design), so it must be
            // blurred — this is where rows 2, 6, 10 and 14 went missing.
            let steps = corner.clamp(1, 12);
            let first_band = (corner / steps).max(1);
            for y in first_band..corner {
                for (px, py) in [
                    (corner - 1, y),
                    (wi - corner, y),
                    (corner - 1, hi - 1 - y),
                    (wi - corner, hi - 1 - y),
                ] {
                    assert!(
                        covered(&sent, px, py),
                        "r={radius}: ({px},{py}) inside the curve is not blurred"
                    );
                }
            }
            // Everything outside the corner squares is blurred.
            for (px, py) in [(wi / 2, 0), (wi / 2, hi - 1), (0, hi / 2), (wi - 1, hi / 2)] {
                assert!(
                    covered(&sent, px, py),
                    "r={radius}: ({px},{py}) not blurred"
                );
            }
            // And no rectangle is empty or inside out.
            assert!(sent.iter().all(|&(_, _, w, h)| w > 0 && h > 0));
        }
    }

    #[test]
    fn a_square_frame_is_one_rectangle() {
        let sent = as_sent(&blur_region(700.0, 600.0, 0.0));
        assert_eq!(sent, vec![(0, 0, 700, 600)]);
    }

    #[test]
    fn a_radius_bigger_than_the_menu_cannot_produce_negative_sizes() {
        for r in blur_region(20.0, 10.0, 50.0) {
            assert!(r.width > 0.0 && r.height > 0.0);
        }
    }

    #[test]
    fn popup_frame_is_as_wide_as_the_menu() {
        // libcosmic's popup_container clamps every applet popup to 360 wide
        // unless told otherwise, which cut the tile column off entirely.
        let max = popup_limits(700.0).max();
        assert_eq!(max.width, 700.0);
        assert_eq!(popup_limits(700.0).min().width, 700.0);
        assert!(max.height >= POPUP_HEIGHT);
    }

    #[test]
    fn popup_fits_the_tile_column_at_every_density() {
        // Spacious gaps once pushed the third tile column past a fixed
        // 680, and the popup edge clipped it narrower than the other two.
        for gap in [4, 8, 12] {
            let spacing = Spacing {
                gap,
                pad_y: gap,
                section: gap + 4,
            };
            for cells in [4, 6] {
                let used = 2.0 * f32::from(spacing.section)
                    + ui::RAIL_WIDTH
                    + ui::LIST_WIDTH
                    + 2.0 * COLUMN_GAP
                    + ui::tiles::grid_width(spacing, cells);
                assert!(popup_width(spacing, cells) >= used);
            }
        }
    }

    #[test]
    fn escape_cancels_a_rename_before_anything_below_it() {
        assert_eq!(
            escape_target(Layers {
                renaming: true,
                query: true,
                editing: true,
                ..layers()
            }),
            Escape::CancelRename
        );
        assert_eq!(
            escape_target(Layers {
                context: true,
                renaming: true,
                ..layers()
            }),
            Escape::Context
        );
    }

    #[test]
    fn enter_picks_apps_first_then_launcher_results() {
        assert_eq!(pick(0, 0, 0), None);
        assert_eq!(pick(2, 3, 0), Some(Pick::App(0)));
        assert_eq!(pick(2, 3, 1), Some(Pick::App(1)));
        assert_eq!(pick(2, 3, 2), Some(Pick::Found(0)));
        assert_eq!(pick(2, 3, 4), Some(Pick::Found(2)));
        // A selection left over from a longer list clamps to the last row.
        assert_eq!(pick(2, 3, 9), Some(Pick::Found(2)));
        assert_eq!(pick(0, 1, 5), Some(Pick::Found(0)));
        assert_eq!(pick(3, 0, 5), Some(Pick::App(2)));
    }

    #[test]
    fn escape_closes_the_popup_only_when_nothing_else_is_open() {
        assert_eq!(escape_target(layers()), Escape::ClosePopup);
    }
}
