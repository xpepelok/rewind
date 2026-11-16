use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::cdp::{Connection, Key, POLL_INTERVAL, Page, js_string};
use crate::failure::Failure;
use crate::publish::{MAX_DESCRIPTION_CHARS, MAX_TITLE_CHARS, Privacy};
use crate::studio::edit_url;

const LOAD_TIMEOUT: Duration = Duration::from_secs(60);
const STEP_TIMEOUT: Duration = Duration::from_secs(20);
const SAVE_TIMEOUT: Duration = Duration::from_secs(60);
const THUMBNAIL_TIMEOUT: Duration = Duration::from_secs(60);
const DELETE_TIMEOUT: Duration = Duration::from_secs(60);
const PRESS_TIMEOUT: Duration = Duration::from_secs(5);
const TOAST_GRACE: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_millis(600);

pub const MAX_THUMBNAIL_BYTES: u64 = 2 * 1024 * 1024;

mod css {
    pub const TITLE: &str = "ytcp-video-title #textbox";
    pub const DESCRIPTION: &str = "ytcp-video-description #textbox";
    pub const SAVE: &str = "ytcp-video-details-section ytcp-button#save";
    pub const VISIBILITY_OPEN: &str = "ytcp-video-metadata-visibility #select-button";
    pub const VISIBILITY_LABEL: &str = "ytcp-video-metadata-visibility #content";
    pub const VISIBILITY_DONE: &str = "ytcp-video-visibility-edit-popup #save-button";
    pub const THUMBNAIL_INPUT: &str = "ytcp-thumbnail-uploader input#file-loader";
    pub const THUMBNAIL_BUTTON: &str = "ytcp-thumbnail-uploader #select-button";
    pub const OVERFLOW: &str = "ytcp-video-overflow-menu #overflow-menu-button";
    pub const DELETE_ITEM: &str = "tp-yt-paper-item[test-id=VIDEO_DELETE]";
    pub const DELETE_UNDERSTOOD: &str = "#dialog-content-confirm-checkboxes label.checkbox-wrapper";
    pub const DIALOG: &str = "ytcp-dialog, tp-yt-paper-dialog, ytcp-confirmation-dialog";
    pub const DIALOG_CONFIRM: &str = "ytcp-button#confirm-button";
    pub const DIALOG_CANCEL: &str = "ytcp-button#cancel-button";
    pub const FIELDS: &str = "ytcp-video-title, ytcp-video-description";
    pub const INVALID: &str = "[invalid], [aria-invalid=true]";
    pub const TOAST: &str = "tp-yt-paper-toast";
    pub const COVERING: &str = "tp-yt-paper-dialog, [role=dialog], [role=alertdialog]";

    pub fn privacy(name: &str) -> String {
        format!("ytcp-video-visibility-edit-popup tp-yt-paper-radio-button[name={name}]")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Details {
    pub title: String,
    pub description: String,
    pub privacy: Option<Privacy>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub title: Option<String>,
    pub description: Option<String>,
    pub privacy: Option<Privacy>,
    pub thumbnail: Option<PathBuf>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.privacy.is_none()
            && self.thumbnail.is_none()
    }

    pub fn problem(&self) -> Option<String> {
        if let Some(title) = &self.title {
            let title = title.trim();
            if title.is_empty() {
                return Some("the title is empty".to_string());
            }
            if title.chars().count() > MAX_TITLE_CHARS {
                return Some(format!("the title is over {MAX_TITLE_CHARS} characters"));
            }
            if title.contains(['<', '>']) {
                return Some("the title contains < or >".to_string());
            }
        }
        if let Some(description) = &self.description {
            if description.chars().count() > MAX_DESCRIPTION_CHARS {
                return Some(format!(
                    "the description is over {MAX_DESCRIPTION_CHARS} characters"
                ));
            }
            if description.contains(['<', '>']) {
                return Some("the description contains < or >".to_string());
            }
        }
        let image = self.thumbnail.as_deref()?;
        let size = std::fs::metadata(image)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|metadata| metadata.len());
        thumbnail_problem(image, size)
    }
}

pub fn thumbnail_problem(image: &Path, size: Option<u64>) -> Option<String> {
    let Some(size) = size else {
        return Some(format!("{} is not a file", image.display()));
    };
    let extension = image
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !matches!(extension.as_str(), "jpg" | "jpeg" | "png") {
        return Some(format!("{} is not a JPEG or PNG image", image.display()));
    }
    if size > MAX_THUMBNAIL_BYTES {
        return Some(format!("{} is larger than 2 MB", image.display()));
    }
    None
}

const fn radio_name(privacy: Privacy) -> &'static str {
    match privacy {
        Privacy::Public => "PUBLIC",
        Privacy::Unlisted => "UNLISTED",
        Privacy::Private => "PRIVATE",
    }
}

fn privacy_from_label(label: &str) -> Option<Privacy> {
    let lower = label.to_lowercase();
    if ["запланир", "schedul", "премьер", "premiere"]
        .iter()
        .any(|word| lower.contains(word))
    {
        None
    } else if lower.contains("ссылк") || lower.contains("unlisted") {
        Some(Privacy::Unlisted)
    } else if lower.contains("открыт") || lower.contains("public") {
        Some(Privacy::Public)
    } else if lower.contains("ограничен") || lower.contains("приват") || lower.contains("private")
    {
        Some(Privacy::Private)
    } else {
        None
    }
}

fn same_text(seen: &str, wanted: &str) -> bool {
    seen.split_whitespace().eq(wanted.split_whitespace())
}

fn on_video_page(url: &str, video_id: &str) -> bool {
    let marker = format!("/video/{video_id}");
    url.split(['?', '#'])
        .next()
        .and_then(|path| path.split_once(&marker))
        .is_some_and(|(_, rest)| rest.is_empty() || rest.starts_with('/'))
}

#[derive(Clone, Debug, PartialEq)]
enum Aim {
    At(f64, f64),
    Missing,
    Covered(String),
}

fn aim(answer: &str) -> Aim {
    if let Some(cover) = answer.strip_prefix("covered:") {
        return Aim::Covered(cover.to_string());
    }
    answer
        .split_once(',')
        .and_then(|(x, y)| Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?)))
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .map_or(Aim::Missing, |(x, y)| Aim::At(x, y))
}

const DEEP: &str = "const deep = (root, wanted) => { const found = []; const seen = new Set(); \
     const walk = (at) => { if (!at || seen.has(at)) return; seen.add(at); \
     if (at.shadowRoot) walk(at.shadowRoot); \
     for (const hit of at.querySelectorAll(wanted)) found.push(hit); \
     for (const node of at.querySelectorAll('*')) if (node.shadowRoot) walk(node.shadowRoot); }; \
     walk(root); return found; }; \
     const shown = (node) => { if (!node || !node.isConnected) return false; \
     const box = node.getBoundingClientRect(); if (!(box.width > 0 && box.height > 0)) return false; \
     return getComputedStyle(node).visibility !== 'hidden'; };";

fn first_shown(selector: &str) -> String {
    format!(
        "(() => {{ {DEEP} return deep(document, {}).find(shown) || null; }})()",
        js_string(selector)
    )
}

fn in_delete_dialog(selector: &str) -> String {
    format!(
        "(() => {{ {DEEP} const box = deep(document, {}).find(shown); if (!box) return null; \
         let dialog = box; \
         while (dialog && !(dialog.matches && dialog.matches({}))) dialog = dialog.parentNode || dialog.host; \
         if (!dialog) return null; \
         return deep(dialog, {}).find(shown) || null; }})()",
        js_string(css::DELETE_UNDERSTOOD),
        js_string(css::DIALOG),
        js_string(selector)
    )
}

fn present(finder: &str) -> String {
    format!("(() => !!{finder})()")
}

fn absent(finder: &str) -> String {
    format!("(() => !{finder})()")
}

fn enabled(finder: &str) -> String {
    format!(
        "(() => {{ const node = {finder}; return !!node && !node.hasAttribute('disabled') && \
         node.getAttribute('aria-disabled') !== 'true'; }})()"
    )
}

fn checked(finder: &str) -> String {
    format!(
        "(() => {{ const node = {finder}; return !!node && node.getAttribute('aria-checked') === 'true'; }})()"
    )
}

fn aim_script(finder: &str) -> String {
    format!(
        "(() => {{ const node = {finder}; if (!node) return 'missing'; \
         node.scrollIntoView({{ block: 'center', inline: 'nearest' }}); \
         const box = node.getBoundingClientRect(); if (!box.width || !box.height) return 'missing'; \
         const x = box.left + box.width / 2; const y = box.top + box.height / 2; \
         let hit = document.elementFromPoint(x, y); \
         while (hit && hit.shadowRoot) {{ const inner = hit.shadowRoot.elementFromPoint(x, y); \
           if (!inner || inner === hit) break; hit = inner; }} \
         for (let at = hit; at; at = at.parentNode || at.host) if (at === node) return x + ',' + y; \
         return 'covered:' + (hit ? hit.tagName.toLowerCase() + (hit.id ? '#' + hit.id : '') : 'nothing'); }})()"
    )
}

fn press(
    connection: &mut Connection,
    page: &Page,
    finder: &str,
    what: &str,
) -> Result<(), Failure> {
    let script = aim_script(finder);
    let deadline = Instant::now() + PRESS_TIMEOUT;
    loop {
        let answer = page.eval_string(connection, &script).unwrap_or_default();
        let last = match aim(&answer) {
            Aim::At(x, y) => return page.click_point(connection, x, y),
            other => other,
        };
        if Instant::now() >= deadline {
            return Err(Failure::PageChanged(match last {
                Aim::Covered(cover) => format!("{what} is covered by {cover}"),
                _ => what.to_string(),
            }));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn press_css(connection: &mut Connection, page: &Page, selector: &str) -> Result<(), Failure> {
    press(connection, page, &first_shown(selector), selector)
}

fn text_of(connection: &mut Connection, page: &Page, selector: &str) -> String {
    page.eval_string(
        connection,
        &format!(
            "(() => {{ const node = {}; return node ? (node.innerText || node.textContent || '') : ''; }})()",
            first_shown(selector)
        ),
    )
    .unwrap_or_default()
    .trim()
    .to_owned()
}

fn open_editor(connection: &mut Connection, page: &Page, video_id: &str) -> Result<(), Failure> {
    page.navigate(connection, &edit_url(video_id))?;
    let signed_out = "(() => location.host === 'accounts.google.com')()";
    let title = present(&first_shown(css::TITLE));
    let found = page.race(
        connection,
        &[title.as_str(), signed_out],
        LOAD_TIMEOUT,
        "video details",
    )?;
    if found == 1 {
        return Err(Failure::SignedOut);
    }
    let url = page.url(connection)?;
    if !on_video_page(&url, video_id) {
        return Err(Failure::PageChanged(format!(
            "Studio opened {url} instead of video {video_id}"
        )));
    }
    page.pretend_to_be_focused(connection);
    std::thread::sleep(SETTLE);
    Ok(())
}

fn set_text(
    connection: &mut Connection,
    page: &Page,
    selector: &str,
    text: &str,
) -> Result<bool, Failure> {
    if text_of(connection, page, selector) == text.trim() {
        return Ok(false);
    }
    let node = first_shown(selector);
    press(connection, page, &node, selector)?;
    let cleared = page.eval_bool(
        connection,
        &format!(
            "(() => {{ const node = {node}; if (!node) return false; node.focus(); \
             document.execCommand('selectAll', false, null); \
             document.execCommand('delete', false, null); return true; }})()"
        ),
    )?;
    if !cleared {
        return Err(Failure::PageChanged(selector.to_string()));
    }
    if !text.is_empty() {
        page.insert_text(connection, text)?;
    }
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        if same_text(&text_of(connection, page, selector), text) {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Err(Failure::PageChanged(format!(
                "{selector} would not take the text"
            )));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn close_visibility(connection: &mut Connection, page: &Page) {
    let popup = absent(&first_shown(css::VISIBILITY_DONE));
    if page.eval_bool(connection, &popup).unwrap_or(false) {
        return;
    }
    let _ = page.press(connection, Key::Escape);
    let _ = page.wait_until(connection, &popup, STEP_TIMEOUT, "visibility closed");
}

fn set_privacy(
    connection: &mut Connection,
    page: &Page,
    privacy: Privacy,
) -> Result<bool, Failure> {
    let label = text_of(connection, page, css::VISIBILITY_LABEL);
    if privacy_from_label(&label) == Some(privacy) {
        return Ok(false);
    }
    press_css(connection, page, css::VISIBILITY_OPEN)?;
    let outcome = choose_privacy(connection, page, privacy);
    if !matches!(outcome, Ok(true)) {
        close_visibility(connection, page);
    }
    outcome
}

fn choose_privacy(
    connection: &mut Connection,
    page: &Page,
    privacy: Privacy,
) -> Result<bool, Failure> {
    let radio = first_shown(&css::privacy(radio_name(privacy)));
    page.wait_until(connection, &present(&radio), STEP_TIMEOUT, "visibility")
        .map_err(|_| {
            Failure::PageChanged(format!(
                "Studio does not offer {} for this video",
                radio_name(privacy)
            ))
        })?;
    if page
        .eval_bool(connection, &checked(&radio))
        .unwrap_or(false)
    {
        return Ok(false);
    }
    press(connection, page, &radio, radio_name(privacy))?;
    page.wait_until(
        connection,
        &checked(&radio),
        STEP_TIMEOUT,
        "visibility choice",
    )?;
    let done = first_shown(css::VISIBILITY_DONE);
    page.wait_until(connection, &enabled(&done), STEP_TIMEOUT, "visibility done")?;
    press(connection, page, &done, css::VISIBILITY_DONE)?;
    page.wait_until(
        connection,
        &absent(&done),
        STEP_TIMEOUT,
        "visibility closed",
    )?;
    Ok(true)
}

fn set_thumbnail(connection: &mut Connection, page: &Page, image: &Path) -> Result<bool, Failure> {
    if !page
        .eval_bool(connection, &enabled(&first_shown(css::THUMBNAIL_BUTTON)))
        .unwrap_or(false)
    {
        return Err(Failure::Rejected(
            "Studio does not offer custom thumbnails for this channel".to_string(),
        ));
    }
    page.set_file(connection, css::THUMBNAIL_INPUT, image)?;
    page.wait_until(
        connection,
        &enabled(&first_shown(css::SAVE)),
        THUMBNAIL_TIMEOUT,
        "thumbnail",
    )?;
    Ok(true)
}

fn refused_fields(connection: &mut Connection, page: &Page) -> Option<String> {
    let names = page
        .eval_string(
            connection,
            &format!(
                "(() => {{ {DEEP} return deep(document, {}).filter(host => \
                 (host.matches({}) && shown(host)) || deep(host, {}).some(shown)) \
                 .map(host => host.tagName.toLowerCase().replace('ytcp-video-', '')).join(', '); }})()",
                js_string(css::FIELDS),
                js_string(css::INVALID),
                js_string(css::INVALID)
            ),
        )
        .unwrap_or_default();
    (!names.trim().is_empty()).then(|| format!("Studio refused the {}", names.trim()))
}

fn covering_dialog(connection: &mut Connection, page: &Page) -> String {
    page.eval_string(
        connection,
        &format!(
            "(() => {{ {DEEP} return deep(document, {}).filter(shown) \
             .map(node => (node.innerText || '').trim().split(String.fromCharCode(10)).join(' ').slice(0, 200)) \
             .filter(text => text).join(' | '); }})()",
            js_string(css::COVERING)
        ),
    )
    .unwrap_or_default()
}

fn toast(connection: &mut Connection, page: &Page) -> String {
    text_of(connection, page, css::TOAST)
}

fn save(connection: &mut Connection, page: &Page) -> Result<(), Failure> {
    let button = first_shown(css::SAVE);
    wait_for_save_button(connection, page, &button)?;
    press(connection, page, &button, css::SAVE)?;
    let still_enabled = enabled(&button);
    let deadline = Instant::now() + SAVE_TIMEOUT;
    let mut toasted: Option<Instant> = None;
    loop {
        if !page.eval_bool(connection, &still_enabled).unwrap_or(true) {
            std::thread::sleep(SETTLE);
            return Ok(());
        }
        if let Some(refused) = refused_fields(connection, page) {
            return Err(Failure::Rejected(refused));
        }
        let message = toast(connection, page);
        if message.is_empty() {
            toasted = None;
        } else {
            let since = *toasted.get_or_insert_with(Instant::now);
            if since.elapsed() >= TOAST_GRACE {
                return Err(Failure::Rejected(message));
            }
        }
        if Instant::now() >= deadline {
            let covering = covering_dialog(connection, page);
            return Err(Failure::Timeout(if covering.is_empty() {
                "saved".to_string()
            } else {
                format!("saved ({covering})")
            }));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_save_button(
    connection: &mut Connection,
    page: &Page,
    button: &str,
) -> Result<(), Failure> {
    let ready = enabled(button);
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        if page.eval_bool(connection, &ready).unwrap_or(false) {
            return Ok(());
        }
        if let Some(refused) = refused_fields(connection, page) {
            return Err(Failure::Rejected(refused));
        }
        if Instant::now() >= deadline {
            return Err(Failure::Timeout("save".to_string()));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn forget_changes(connection: &mut Connection, page: &Page) {
    let _ = page.eval(
        connection,
        "(() => { window.onbeforeunload = null; return true; })()",
    );
}

pub fn read(connection: &mut Connection, page: &Page, video_id: &str) -> Result<Details, Failure> {
    open_editor(connection, page, video_id)?;
    let label = text_of(connection, page, css::VISIBILITY_LABEL);
    Ok(Details {
        title: text_of(connection, page, css::TITLE),
        description: text_of(connection, page, css::DESCRIPTION),
        privacy: privacy_from_label(&label),
    })
}

pub fn apply(
    connection: &mut Connection,
    page: &Page,
    video_id: &str,
    changes: &Changes,
) -> Result<(), Failure> {
    if changes.is_empty() {
        return Ok(());
    }
    if let Some(problem) = changes.problem() {
        return Err(Failure::Rejected(problem));
    }
    open_editor(connection, page, video_id)?;
    let outcome = edit(connection, page, changes);
    if outcome.is_err() {
        forget_changes(connection, page);
    }
    outcome
}

fn edit(connection: &mut Connection, page: &Page, changes: &Changes) -> Result<(), Failure> {
    let mut changed = false;
    if let Some(image) = &changes.thumbnail {
        changed |= set_thumbnail(connection, page, image)?;
    }
    if let Some(title) = &changes.title {
        changed |= set_text(connection, page, css::TITLE, title.trim())?;
    }
    if let Some(description) = &changes.description {
        changed |= set_text(connection, page, css::DESCRIPTION, description)?;
    }
    if let Some(privacy) = changes.privacy {
        changed |= set_privacy(connection, page, privacy)?;
    }
    if changed {
        save(connection, page)
    } else {
        Ok(())
    }
}

pub fn delete(connection: &mut Connection, page: &Page, video_id: &str) -> Result<(), Failure> {
    let confirm = open_delete_dialog(connection, page, video_id)?;
    let outcome = confirm_delete(connection, page, &confirm);
    if outcome.is_err() {
        let _ = cancel_delete(connection, page);
        return outcome;
    }
    wait_until_gone(connection, page, video_id)
}

fn open_delete_dialog(
    connection: &mut Connection,
    page: &Page,
    video_id: &str,
) -> Result<String, Failure> {
    open_editor(connection, page, video_id)?;
    press_css(connection, page, css::OVERFLOW)?;
    let item = first_shown(css::DELETE_ITEM);
    page.wait_until(connection, &present(&item), STEP_TIMEOUT, "delete item")?;
    press(connection, page, &item, css::DELETE_ITEM)?;

    let confirm = in_delete_dialog(css::DIALOG_CONFIRM);
    page.wait_until(
        connection,
        &present(&confirm),
        STEP_TIMEOUT,
        "delete confirmation",
    )?;
    Ok(confirm)
}

fn cancel_delete(connection: &mut Connection, page: &Page) -> Result<(), Failure> {
    let cancel = in_delete_dialog(css::DIALOG_CANCEL);
    press(connection, page, &cancel, css::DIALOG_CANCEL)?;
    page.wait_until(
        connection,
        &absent(&in_delete_dialog(css::DIALOG_CONFIRM)),
        STEP_TIMEOUT,
        "delete dialog closed",
    )
}

fn confirm_delete(connection: &mut Connection, page: &Page, confirm: &str) -> Result<(), Failure> {
    press(
        connection,
        page,
        &in_delete_dialog(css::DELETE_UNDERSTOOD),
        css::DELETE_UNDERSTOOD,
    )?;
    page.wait_until(connection, &enabled(confirm), STEP_TIMEOUT, "delete button")?;
    press(connection, page, confirm, css::DIALOG_CONFIRM)
}

fn wait_until_gone(
    connection: &mut Connection,
    page: &Page,
    video_id: &str,
) -> Result<(), Failure> {
    let deadline = Instant::now() + DELETE_TIMEOUT;
    loop {
        if let Ok(url) = page.url(connection)
            && !url.is_empty()
            && !on_video_page(&url, video_id)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let dialog_open = page
                .eval_bool(connection, &present(&in_delete_dialog(css::DIALOG_CONFIRM)))
                .unwrap_or(false);
            return Err(if dialog_open {
                Failure::Timeout("deleted".to_string())
            } else {
                Failure::PageChanged(format!("video {video_id} is still there after deleting"))
            });
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_names_round_trip() {
        assert_eq!(radio_name(Privacy::Unlisted), "UNLISTED");
        assert!(css::privacy("PUBLIC").ends_with("[name=PUBLIC]"));
        assert_eq!(privacy_from_label("По ссылке"), Some(Privacy::Unlisted));
        assert_eq!(privacy_from_label("Открытый доступ"), Some(Privacy::Public));
        assert_eq!(
            privacy_from_label("Ограниченный доступ"),
            Some(Privacy::Private)
        );
        assert_eq!(privacy_from_label("Unlisted"), Some(Privacy::Unlisted));
        assert_eq!(privacy_from_label("Запланировано"), None);
    }

    #[test]
    fn a_scheduled_or_premiere_label_is_never_read_as_the_privacy_it_mentions() {
        assert_eq!(privacy_from_label("Запланировано (открытый доступ)"), None);
        assert_eq!(privacy_from_label("Scheduled · Public"), None);
        assert_eq!(privacy_from_label("Premiere"), None);
        assert_eq!(privacy_from_label("Премьера"), None);
        assert_eq!(privacy_from_label(""), None);
    }

    #[test]
    fn empty_changes_touch_nothing() {
        assert!(Changes::default().is_empty());
        assert!(
            !Changes {
                privacy: Some(Privacy::Private),
                ..Changes::default()
            }
            .is_empty()
        );
        assert_eq!(Changes::default().problem(), None);
    }

    #[test]
    fn a_title_studio_would_refuse_is_caught_before_the_page_opens() {
        let titled = |title: &str| Changes {
            title: Some(title.to_string()),
            ..Changes::default()
        };
        assert_eq!(titled("Fine title").problem(), None);
        assert_eq!(titled(&"я".repeat(MAX_TITLE_CHARS)).problem(), None);
        assert!(titled("   ").problem().is_some());
        assert!(titled(&"a".repeat(MAX_TITLE_CHARS + 1)).problem().is_some());
        assert!(titled("a <b> c").problem().is_some());
    }

    #[test]
    fn a_description_studio_would_refuse_is_caught_too_but_an_empty_one_is_allowed() {
        let described = |description: &str| Changes {
            description: Some(description.to_string()),
            ..Changes::default()
        };
        assert_eq!(described("").problem(), None);
        assert_eq!(described("line one\nline two").problem(), None);
        assert!(
            described(&"d".repeat(MAX_DESCRIPTION_CHARS + 1))
                .problem()
                .is_some()
        );
        assert!(described("1 > 0").problem().is_some());
    }

    #[test]
    fn a_thumbnail_must_be_a_small_jpeg_or_png_that_exists() {
        let picture = Path::new("cover.JPG");
        assert_eq!(thumbnail_problem(picture, Some(1_000)), None);
        assert_eq!(
            thumbnail_problem(Path::new("a.png"), Some(MAX_THUMBNAIL_BYTES)),
            None
        );
        assert!(thumbnail_problem(picture, None).is_some());
        assert!(thumbnail_problem(picture, Some(MAX_THUMBNAIL_BYTES + 1)).is_some());
        assert!(thumbnail_problem(Path::new("a.webp"), Some(10)).is_some());
        assert!(thumbnail_problem(Path::new("noextension"), Some(10)).is_some());

        let missing = Changes {
            thumbnail: Some(PathBuf::from("definitely/not/here.png")),
            ..Changes::default()
        };
        assert!(missing.problem().is_some());
    }

    #[test]
    fn text_read_back_matches_whatever_line_breaks_studio_chose() {
        assert!(same_text("one\n\ntwo ", "one\ntwo"));
        assert!(same_text("", "  "));
        assert!(!same_text("one two", "onetwo"));
        assert!(!same_text("one", "one two"));
    }

    #[test]
    fn only_the_page_of_this_very_video_counts_as_its_editor() {
        let id = "dQw4w9WgXcQ";
        assert!(on_video_page(
            "https://studio.youtube.com/video/dQw4w9WgXcQ/edit",
            id
        ));
        assert!(on_video_page(
            "https://studio.youtube.com/video/dQw4w9WgXcQ/edit?c=UC1",
            id
        ));
        assert!(on_video_page(
            "https://studio.youtube.com/video/dQw4w9WgXcQ",
            id
        ));
        assert!(!on_video_page(
            "https://studio.youtube.com/channel/UC1/videos/upload",
            id
        ));
        assert!(!on_video_page(
            "https://studio.youtube.com/video/dQw4w9WgXcQ2/edit",
            id
        ));
        assert!(!on_video_page(
            "https://studio.youtube.com/channel/UC1?next=/video/dQw4w9WgXcQ/edit",
            id
        ));
        assert!(!on_video_page("", id));
    }

    #[test]
    fn a_press_lands_only_where_the_element_itself_is_hit() {
        assert_eq!(aim("10.5,20"), Aim::At(10.5, 20.0));
        assert_eq!(aim("missing"), Aim::Missing);
        assert_eq!(aim(""), Aim::Missing);
        assert_eq!(aim("NaN,NaN"), Aim::Missing);
        assert_eq!(
            aim("covered:tp-yt-iron-overlay-backdrop"),
            Aim::Covered("tp-yt-iron-overlay-backdrop".to_string())
        );
    }

    #[test]
    fn the_scripts_never_fall_back_to_a_synthetic_click() {
        for script in [
            aim_script(&first_shown(css::SAVE)),
            aim_script(&in_delete_dialog(css::DIALOG_CONFIRM)),
        ] {
            assert!(!script.contains(".click()"), "{script}");
            assert!(script.contains("elementFromPoint"), "{script}");
        }
    }

    #[test]
    fn the_delete_button_is_looked_for_only_inside_the_dialog_holding_the_checkbox() {
        let script = in_delete_dialog(css::DIALOG_CONFIRM);
        let checkbox = script
            .find("dialog-content-confirm-checkboxes")
            .expect("checkbox");
        let button = script.find("confirm-button").expect("button");
        assert!(
            checkbox < button,
            "the dialog is found from the checkbox first"
        );
        assert!(script.contains("deep(dialog,"), "{script}");
    }

    #[test]
    #[ignore = "reads a real video through a Session; set REWIND_YOUTUBE_DATA, REWIND_ACCOUNT and REWIND_VIDEO"]
    fn a_real_video_reads_back() {
        let directory = std::path::PathBuf::from(std::env::var("REWIND_YOUTUBE_DATA").unwrap());
        let account = std::env::var("REWIND_ACCOUNT").unwrap();
        let video = std::env::var("REWIND_VIDEO").unwrap();
        let headless = std::env::var("REWIND_VISIBLE").is_err();
        let mut session = crate::Session::open(&directory, &account, headless).unwrap();
        let mut tab = session.tab().unwrap();
        let details = tab.read_video(&video).unwrap();
        tab.close();
        session.close();
        assert!(!details.title.is_empty(), "{details:?}");
        assert!(details.privacy.is_some(), "{details:?}");
        println!("{details:?}");
    }

    struct Live {
        browser: crate::chrome::Browser,
        connection: Connection,
        page: Page,
        video: String,
        shots: PathBuf,
    }

    impl Live {
        fn open() -> Self {
            let directory = PathBuf::from(std::env::var("REWIND_YOUTUBE_DATA").unwrap());
            let account = std::env::var("REWIND_ACCOUNT").unwrap();
            let video = std::env::var("REWIND_VIDEO").unwrap();
            let shots =
                std::env::var("REWIND_SHOTS").map_or_else(|_| std::env::temp_dir(), PathBuf::from);
            let headless = std::env::var("REWIND_VISIBLE").is_err();
            let profile = crate::chrome::profile_directory(&directory, &account);
            let browser = crate::chrome::Browser::launch(&profile, headless).unwrap();
            let mut connection = Connection::connect(&browser.websocket_url).unwrap();
            let page = connection.open("about:blank").unwrap();
            let agent = connection.user_agent().unwrap();
            if crate::cdp::is_headless_agent(&agent) {
                page.set_user_agent(&mut connection, &crate::cdp::visible_user_agent(&agent))
                    .unwrap();
            }
            page.block(&mut connection, &crate::session::UNNEEDED)
                .unwrap();
            Self {
                browser,
                connection,
                page,
                video,
                shots,
            }
        }

        fn timed<T: std::fmt::Debug>(
            &mut self,
            step: &str,
            run: impl FnOnce(&mut Connection, &Page, &str) -> Result<T, Failure>,
        ) -> T {
            let started = Instant::now();
            let video = self.video.clone();
            let outcome = run(&mut self.connection, &self.page, &video);
            println!("{step}: {outcome:?} in {:?}", started.elapsed());
            match outcome {
                Ok(value) => value,
                Err(failure) => {
                    let shot = self
                        .shots
                        .join(format!("manage-{}.png", step.replace(' ', "-")));
                    self.page.screenshot(&mut self.connection, &shot);
                    let open = self.page.eval_string(
                        &mut self.connection,
                        &format!(
                            "(() => {{ {DEEP} return deep(document, 'tp-yt-paper-dialog, ytcp-dialog, [role=dialog], [role=alertdialog], tp-yt-iron-overlay-backdrop, tp-yt-paper-toast, ytcp-button#save').filter(shown).map(node => node.tagName.toLowerCase() + '#' + node.id + ' ' + [...node.attributes].map(a => a.name + '=' + a.value.slice(0, 40)).join(',') + ' :: ' + (node.innerText || '').trim().slice(0, 300)).join('\\n'); }})()"
                        ),
                    );
                    println!("shown on failure:\n{}", open.unwrap_or_default());
                    forget_changes(&mut self.connection, &self.page);
                    self.close();
                    panic!("{step}: {failure:?} (screenshot {})", shot.display());
                }
            }
        }

        fn close(&mut self) {
            self.connection.close_browser();
            self.browser.wait_for_exit(crate::session::SHUTDOWN_TIMEOUT);
        }
    }

    const ORIGINAL_TITLE: &str = "тест заливка";
    const ORIGINAL_DESCRIPTION: &str = "uploaded via github.com/xpepelok/nvideva";

    fn original() -> Details {
        Details {
            title: ORIGINAL_TITLE.to_string(),
            description: ORIGINAL_DESCRIPTION.to_string(),
            privacy: Some(Privacy::Unlisted),
        }
    }

    #[test]
    #[ignore = "edits a real video and puts it back; set REWIND_YOUTUBE_DATA, REWIND_ACCOUNT and REWIND_VIDEO"]
    fn a_real_video_takes_an_edit_and_gives_it_back() {
        let mut live = Live::open();
        let before = live.timed("read before", read);
        assert_eq!(before, original());

        let description = format!("{ORIGINAL_DESCRIPTION}\nвторая строка\n\nчетвёртая строка");
        let changed = Changes {
            title: Some(format!("{ORIGINAL_TITLE} (правка)")),
            description: Some(description.clone()),
            privacy: Some(Privacy::Private),
            thumbnail: None,
        };
        assert_eq!(changed.problem(), None);
        live.timed("open", open_editor);
        live.timed("title", |connection, page, _| {
            set_text(
                connection,
                page,
                css::TITLE,
                changed.title.as_deref().unwrap_or_default(),
            )
        });
        live.timed("description", |connection, page, _| {
            set_text(connection, page, css::DESCRIPTION, &description)
        });
        live.timed("privacy", |connection, page, _| {
            set_privacy(connection, page, Privacy::Private)
        });
        live.timed("save", |connection, page, _| save(connection, page));
        let saved = live.timed("save button after saving", |connection, page, _| {
            page.eval_bool(connection, &enabled(&first_shown(css::SAVE)))
        });
        let edited = live.timed("read edited", read);

        let restore = Changes {
            title: Some(ORIGINAL_TITLE.to_string()),
            description: Some(ORIGINAL_DESCRIPTION.to_string()),
            privacy: Some(Privacy::Unlisted),
            thumbnail: None,
        };
        live.timed("restore", |connection, page, video| {
            apply(connection, page, video, &restore)
        });
        let after = live.timed("read restored", read);
        live.close();

        assert!(!saved, "Save stays enabled after saving");
        assert_eq!(edited.title, format!("{ORIGINAL_TITLE} (правка)"));
        assert!(same_text(&edited.description, &description), "{edited:?}");
        assert_eq!(edited.description.lines().count(), 4, "{edited:?}");
        assert_eq!(edited.privacy, Some(Privacy::Private));
        assert_eq!(after, original());
    }

    #[test]
    #[ignore = "opens a real video's visibility and title without saving; set REWIND_YOUTUBE_DATA, REWIND_ACCOUNT and REWIND_VIDEO"]
    fn a_real_editor_reports_what_it_will_not_save() {
        let mut live = Live::open();
        live.timed("open", open_editor);
        live.timed("open visibility", |connection, page, _| {
            press_css(connection, page, css::VISIBILITY_OPEN)
        });
        let picked = live.timed("pick the current visibility", |connection, page, _| {
            choose_privacy(connection, page, Privacy::Unlisted)
        });
        let closed = live.timed("close visibility", |connection, page, _| {
            close_visibility(connection, page);
            page.eval_bool(connection, &absent(&first_shown(css::VISIBILITY_DONE)))
        });
        let unchanged = live.timed("save button untouched", |connection, page, _| {
            page.eval_bool(connection, &enabled(&first_shown(css::SAVE)))
        });
        let emptied = live.timed("empty the title", |connection, page, _| {
            set_text(connection, page, css::TITLE, "")
        });
        let started = Instant::now();
        let refused = save(&mut live.connection, &live.page);
        let waited = started.elapsed();
        println!("save an empty title: {refused:?} in {waited:?}");
        forget_changes(&mut live.connection, &live.page);
        let after = live.timed("read untouched", read);
        live.close();

        assert!(!picked);
        assert!(closed);
        assert!(!unchanged);
        assert!(emptied);
        assert_eq!(
            refused,
            Err(Failure::Rejected("Studio refused the title".to_string()))
        );
        assert!(waited < STEP_TIMEOUT);
        assert_eq!(after, original());
    }

    #[test]
    #[ignore = "opens the delete dialog of a real video and cancels it; set REWIND_YOUTUBE_DATA, REWIND_ACCOUNT and REWIND_VIDEO"]
    fn a_real_delete_dialog_opens_and_is_cancelled() {
        let mut live = Live::open();
        let confirm = live.timed("open delete dialog", open_delete_dialog);
        let armed = live.timed("delete stays disarmed", |connection, page, _| {
            page.eval_bool(connection, &enabled(&confirm))
        });
        live.timed("cancel", |connection, page, _| {
            cancel_delete(connection, page)
        });
        let after = live.timed("read after cancel", read);
        live.close();
        assert!(!armed);
        assert_eq!(after, original());
    }

    #[test]
    #[ignore = "sets a custom thumbnail on a real video; set REWIND_YOUTUBE_DATA, REWIND_ACCOUNT, REWIND_VIDEO and REWIND_THUMBNAIL"]
    fn a_real_video_takes_a_custom_thumbnail() {
        let image = PathBuf::from(std::env::var("REWIND_THUMBNAIL").unwrap());
        let mut live = Live::open();
        let changes = Changes {
            thumbnail: Some(image),
            ..Changes::default()
        };
        live.timed("thumbnail", |connection, page, video| {
            apply(connection, page, video, &changes)
        });
        let saved = live.timed("save button after thumbnail", |connection, page, _| {
            page.eval_bool(connection, &enabled(&first_shown(css::SAVE)))
        });
        let after = live.timed("read after thumbnail", read);
        live.close();
        assert!(!saved);
        assert_eq!(after, original());
    }
}
