use crate::cdp::{Connection, Key, Page, query, query_all};
use crate::clock::Zone;
use crate::failure::Failure;
use crate::publish::{Privacy, PublishSettings};
use crate::selectors::studio as css;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

pub const UPLOAD_URL: &str = "https://www.youtube.com/upload?hl=en&persist_hl=1";

pub const DIALOG_TIMEOUT: Duration = Duration::from_secs(60);

pub const SETTLE: Duration = Duration::from_millis(1200);

pub const SAVE_TIMEOUT: Duration = Duration::from_secs(60);

pub const SAVE_QUIET: Duration = Duration::from_secs(4);

const CHOICE_TIMEOUT: Duration = Duration::from_secs(5);

const DIALOG_SETTLE: Duration = Duration::from_secs(25);
const DIALOG_ATTEMPTS: usize = 2;
pub const STEP_TIMEOUT: Duration = Duration::from_secs(45);

pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(6 * 3_600);
const TICK: Duration = Duration::from_millis(500);
const TRANSFER_TICK: Duration = Duration::from_secs(2);
const TIDY_INTERVAL: Duration = Duration::from_secs(60);

pub const PUBLISH_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    #[default]
    Opening,

    Uploading,

    Processing,

    Publishing,
    Done,
}

impl Stage {
    pub fn message_key(&self) -> &'static str {
        match self {
            Self::Opening => "youtube.stage.opening",
            Self::Uploading => "youtube.stage.uploading",
            Self::Processing => "youtube.stage.processing",
            Self::Publishing => "youtube.stage.publishing",
            Self::Done => "youtube.stage.done",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Continue,
    Cancel,
}

pub fn parse_percent(label: &str) -> Option<u32> {
    let bytes = label.as_bytes();
    let percent = label.find('%')?;
    let mut start = percent;
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    if start == percent {
        return None;
    }
    label[start..percent]
        .parse::<u32>()
        .ok()
        .map(|value| value.min(100))
}

pub fn is_transfer_done(label: &str) -> bool {
    let lower = label.to_lowercase();
    !lower.contains('%')
        && (lower.contains("process")
            || lower.contains("check")
            || lower.contains("upload complete")
            || lower.contains("обработ")
            || lower.contains("оброб"))
}

pub fn video_id_from_link(link: &str) -> Option<String> {
    let candidate = if let Some(rest) = link.split("youtu.be/").nth(1) {
        rest
    } else if let Some(rest) = link.split("watch?v=").nth(1) {
        rest
    } else {
        link.split("/video/").nth(1)?
    };
    let id: String = candidate
        .chars()
        .take_while(|character| {
            character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
        })
        .collect();
    (id.len() >= 8).then_some(id)
}

fn text_of(selector: &str) -> String {
    format!(
        "(() => {{ const node = {}; return node ? (node.innerText || node.textContent || '').trim() : ''; }})()",
        query(selector)
    )
}

fn exists(selector: &str) -> String {
    format!("(() => !!{})()", query(selector))
}

fn visible(selector: &str) -> String {
    format!(
        "(() => {{ const node = {}; return !!node && !!(node.offsetWidth || node.offsetHeight || \
         node.getClientRects().length); }})()",
        query(selector)
    )
}

fn usable(selector: &str) -> String {
    format!(
        "(() => {{ const node = {}; if (!node) return false; \
         if (!(node.offsetWidth || node.offsetHeight || node.getClientRects().length)) return false; \
         return !(node.hasAttribute('disabled') || node.getAttribute('aria-disabled') === 'true'); }})()",
        query(selector)
    )
}

fn chosen(selector: &str) -> String {
    format!(
        "(() => {{ const node = {}; return !!node && (node.getAttribute('aria-checked') === 'true' \
         || node.hasAttribute('checked')); }})()",
        query(selector)
    )
}

fn point(centre: &str) -> Option<(f64, f64)> {
    let (x, y) = centre.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

fn click_if_present(connection: &mut Connection, page: &Page, selector: &str) -> bool {
    page.eval_bool(
        connection,
        &format!(
            "(() => {{ const node = {}; if (!node) return false; node.click(); return true; }})()",
            query(selector)
        ),
    )
    .unwrap_or(false)
}

struct Progress<'a> {
    report: &'a mut dyn FnMut(Stage, u32) -> Control,
    last: (Stage, u32),
}

impl Progress<'_> {
    fn tell(&mut self, stage: Stage, percent: u32) -> Result<(), Failure> {
        self.last = (stage, percent);
        match (self.report)(stage, percent) {
            Control::Continue => Ok(()),
            Control::Cancel => Err(Failure::Cancelled),
        }
    }

    fn still_wanted(&mut self) -> Result<(), Failure> {
        let (stage, percent) = self.last;
        self.tell(stage, percent)
    }
}

pub fn upload(
    connection: &mut Connection,
    page: &Page,
    settings: &PublishSettings,
    source: &Path,
    report: &mut dyn FnMut(Stage, u32) -> Control,
) -> Result<String, Failure> {
    if !source.is_file() {
        return Err(Failure::Io(format!("{} is not a file", source.display())));
    }
    let mut progress = Progress {
        report,
        last: (Stage::Opening, 0),
    };
    let outcome = drive(connection, page, settings, source, &mut progress);
    if outcome.as_ref().is_err_and(Failure::is_cancelled) {
        let _ = click_if_present(connection, page, css::CLOSE_DIALOG);
    }
    outcome
}

fn drive(
    connection: &mut Connection,
    page: &Page,
    settings: &PublishSettings,
    source: &Path,
    progress: &mut Progress,
) -> Result<String, Failure> {
    progress.tell(Stage::Opening, 0)?;

    open_dialog(connection, page, progress)?;
    progress.still_wanted()?;
    page.choose_file(connection, css::SELECT_FILES, source)?;

    wait_or_cancel(
        connection,
        page,
        &format!("(() => {}.length > 1)()", query_all(css::TEXT_BOXES)),
        DIALOG_TIMEOUT,
        "details form",
        progress,
    )?;
    progress.tell(Stage::Uploading, 0)?;

    fill_details(connection, page, settings)?;
    progress.still_wanted()?;

    let video_id = wait_for_video_id(connection, page, progress)?;

    watch_transfer(connection, page, progress)?;

    restore_details(connection, page, settings);

    progress.tell(Stage::Publishing, 0)?;
    finish(connection, page, settings, progress)?;
    let _ = progress.tell(Stage::Done, 100);
    Ok(video_id)
}

fn wait_or_cancel(
    connection: &mut Connection,
    page: &Page,
    script: &str,
    timeout: Duration,
    what: &str,
    progress: &mut Progress,
) -> Result<(), Failure> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if page.eval_bool(connection, script).unwrap_or(false) {
            return Ok(());
        }
        progress.still_wanted()?;
        if std::time::Instant::now() >= deadline {
            return Err(Failure::Timeout(what.to_string()));
        }
        std::thread::sleep(crate::cdp::POLL_INTERVAL);
    }
}

fn restore_details(connection: &mut Connection, page: &Page, settings: &PublishSettings) {
    let title = settings.title.trim();
    if title.is_empty() {
        return;
    }

    let mut refilled = false;
    if !text_box_holds(connection, page, 0, title) {
        let _ = fill_text_box(connection, page, 0, title);
        refilled = true;
    }
    if !settings.description.is_empty()
        && !text_box_holds(connection, page, 1, &settings.description)
    {
        let _ = fill_text_box(connection, page, 1, &settings.description);
        refilled = true;
    }
    if refilled {
        std::thread::sleep(SETTLE);
    }
}

fn fill_details(
    connection: &mut Connection,
    page: &Page,
    settings: &PublishSettings,
) -> Result<(), Failure> {
    page.pretend_to_be_focused(connection);
    fill_text_box(connection, page, 0, settings.title.trim())?;
    if !settings.description.is_empty() {
        fill_text_box(connection, page, 1, &settings.description)?;
    }
    std::thread::sleep(SETTLE);

    let kids = if settings.made_for_kids {
        css::MADE_FOR_KIDS
    } else {
        css::NOT_FOR_KIDS
    };
    if !click_if_present(connection, page, kids) {
        return Err(Failure::PageChanged(kids.to_string()));
    }

    open_advanced(connection, page)?;

    if !settings.tags.is_empty() {
        page.fill(
            connection,
            css::TAGS,
            &format!("{},", settings.tags.join(",")),
        )?;
    }
    if settings.age_restricted {
        click_if_present(connection, page, css::AGE_RESTRICTED);
    }

    click_if_present(
        connection,
        page,
        if settings.altered_content {
            css::ALTERED_CONTENT_YES
        } else {
            css::ALTERED_CONTENT_NO
        },
    );

    set_category(connection, page, settings)?;
    fill_extras(connection, page, settings);
    Ok(())
}

pub fn fill_extras(connection: &mut Connection, page: &Page, settings: &PublishSettings) {
    if let Some(thumbnail) = settings.thumbnail.as_deref().filter(|path| path.is_file()) {
        let _ = page.set_file(connection, css::THUMBNAIL_INPUT, thumbnail);
    }

    set_playlists(connection, page, &settings.playlists);

    if !settings.video_language.trim().is_empty() {
        pick_by_text(
            connection,
            page,
            css::VIDEO_LANGUAGE,
            &settings.video_language,
        );
    }

    if settings.license != crate::publish::License::default() {
        pick_option(
            connection,
            page,
            css::LICENSE_SELECT,
            settings.license.option_index(),
        );
    }
    set_checkbox(connection, page, css::ALLOW_EMBED, settings.allow_embedding);
    set_checkbox(
        connection,
        page,
        css::PAID_PROMOTION,
        settings.paid_promotion,
    );

    if let Some(moderation) = settings.comments.moderation_index()
        && moderation > 0
    {
        pick_option(connection, page, css::MODERATION_SELECT, moderation);
    }
    if !settings.comments.enabled() {
        pick_last_option(connection, page, css::COMMENTS_SELECT);
    }

    set_labelled_checkbox(connection, page, "like", settings.show_like_count);

    if settings.remix != crate::publish::Remix::default() {
        click_if_present(connection, page, settings.remix.selector());
    }
}

fn open_items() -> String {
    format!(
        "{}.filter(node => !!(node.offsetWidth || node.offsetHeight || \
         node.getClientRects().length))",
        query_all(css::DROPDOWN_ITEMS)
    )
}

fn pick_option(connection: &mut Connection, page: &Page, trigger: &str, index: usize) -> bool {
    if !click_if_present(connection, page, trigger) {
        return false;
    }
    if page
        .wait_until(
            connection,
            &format!("(() => {}.length > {index})()", open_items()),
            STEP_TIMEOUT,
            "dropdown",
        )
        .is_err()
    {
        return false;
    }
    page.eval_bool(
        connection,
        &format!(
            "(() => {{ const node = {}[{index}]; if (!node) return false; node.click(); \
             return true; }})()",
            open_items()
        ),
    )
    .unwrap_or(false)
}

fn pick_last_option(connection: &mut Connection, page: &Page, trigger: &str) -> bool {
    if !click_if_present(connection, page, trigger) {
        return false;
    }
    if page
        .wait_until(
            connection,
            &format!("(() => {}.length > 0)()", open_items()),
            STEP_TIMEOUT,
            "dropdown",
        )
        .is_err()
    {
        return false;
    }
    page.eval_bool(
        connection,
        &format!(
            "(() => {{ const items = {}; const node = items[items.length - 1]; \
             if (!node) return false; node.click(); return true; }})()",
            open_items()
        ),
    )
    .unwrap_or(false)
}

fn pick_by_text(connection: &mut Connection, page: &Page, trigger: &str, text: &str) -> bool {
    if !click_if_present(connection, page, trigger) {
        return false;
    }
    if page
        .wait_until(
            connection,
            &format!("(() => {}.length > 0)()", open_items()),
            STEP_TIMEOUT,
            "dropdown",
        )
        .is_err()
    {
        return false;
    }
    let wanted = crate::publish::normalise_label(text);
    page.eval_bool(
        connection,
        &format!(
            "(() => {{ const wanted = {}; const node = {}.find(item => \
             (item.innerText || item.textContent || '').trim().toLowerCase() \
             .split(/\\s+/).join(' ') === wanted); if (!node) return false; node.click(); \
             return true; }})()",
            crate::cdp::js_string(&wanted),
            open_items()
        ),
    )
    .unwrap_or(false)
}

fn set_checkbox(connection: &mut Connection, page: &Page, selector: &str, wanted: bool) {
    let _ = page.eval(
        connection,
        &format!(
            "(() => {{ const outer = {}; if (!outer) return false; \
             const lit = outer.matches('ytcp-checkbox-lit') ? outer \
               : outer.querySelector('ytcp-checkbox-lit') || outer; \
             const on = lit.hasAttribute('checked') || \
               lit.getAttribute('aria-checked') === 'true'; \
             if (on === {wanted}) return true; \
             const target = lit.querySelector('#checkbox-container') || \
               lit.querySelector('#checkbox') || lit; \
             target.click(); return true; }})()",
            query(selector)
        ),
    );
}

fn set_labelled_checkbox(connection: &mut Connection, page: &Page, word: &str, wanted: bool) {
    let _ = page.eval(
        connection,
        &format!(
            "(() => {{ const word = {}; \
             const section = document.querySelector('ytcp-video-metadata-editor-advanced'); \
             if (!section) return false; \
             const node = Array.from(section.querySelectorAll('ytcp-form-checkbox')) \
             .find(item => (item.innerText || '').toLowerCase().includes(word)); \
             if (!node) return false; \
             const lit = node.querySelector('ytcp-checkbox-lit') || node; \
             const on = lit.hasAttribute('checked') || \
               lit.getAttribute('aria-checked') === 'true'; \
             if (on === {wanted}) return true; \
             const target = lit.querySelector('#checkbox-container') || \
               lit.querySelector('#checkbox') || lit; \
             target.click(); return true; }})()",
            crate::cdp::js_string(&word.to_lowercase())
        ),
    );
}

fn set_playlists(connection: &mut Connection, page: &Page, wanted: &[String]) {
    if wanted.is_empty() {
        return;
    }
    if !click_if_present(connection, page, css::PLAYLIST_SELECT) {
        return;
    }
    if page
        .wait_until(
            connection,
            &format!("(() => {}.length > 0)()", query_all(css::PLAYLIST_ITEMS)),
            STEP_TIMEOUT,
            "playlists",
        )
        .is_err()
    {
        return;
    }
    let names: Vec<String> = wanted
        .iter()
        .map(|name| crate::publish::normalise_label(name))
        .collect();
    let _ = page.eval(
        connection,
        &format!(
            "(() => {{ const wanted = {}; \
             for (const item of {}) {{ \
               const text = (item.innerText || item.textContent || '').trim().toLowerCase() \
                 .split(/\\s+/).join(' '); \
               if (!wanted.includes(text)) continue; \
               if (item.hasAttribute('checked') || \
                   item.getAttribute('aria-checked') === 'true') continue; \
               const target = item.querySelector('#checkbox-container') || \
                 item.querySelector('#checkbox') || item; \
               target.click(); \
             }} return true; }})()",
            serde_json::to_string(&names).unwrap_or_else(|_| "[]".to_string()),
            query_all(css::PLAYLIST_ITEMS)
        ),
    );
    click_if_present(connection, page, css::PLAYLIST_DONE);
}

fn open_dialog(
    connection: &mut Connection,
    page: &Page,
    progress: &mut Progress,
) -> Result<(), Failure> {
    let appeared = |connection: &mut Connection, progress: &mut Progress| {
        let waited = wait_or_cancel(
            connection,
            page,
            &exists(css::SELECT_FILES),
            DIALOG_SETTLE,
            "upload dialog",
            progress,
        );
        match waited {
            Ok(()) => Ok(true),
            Err(Failure::Cancelled) => Err(Failure::Cancelled),
            Err(_) => Ok(false),
        }
    };

    for attempt in 0..DIALOG_ATTEMPTS {
        if attempt > 0 {
            let _ = page.eval(
                connection,
                "(() => { window.onbeforeunload = null; return true; })()",
            );
        }
        page.navigate(connection, UPLOAD_URL)?;

        if appeared(connection, progress)? {
            return Ok(());
        }
        if click_if_present(connection, page, css::CREATE_BUTTON) {
            std::thread::sleep(TICK);
            click_if_present(connection, page, css::UPLOAD_MENU_ITEM);
            if appeared(connection, progress)? {
                return Ok(());
            }
        }
    }
    Err(Failure::Timeout("upload dialog".to_string()))
}

fn fill_text_box(
    connection: &mut Connection,
    page: &Page,
    index: usize,
    text: &str,
) -> Result<(), Failure> {
    let centre = page
        .eval_string(
            connection,
            &format!(
                "(() => {{ const node = {}[{index}]; if (!node) return ''; \
                 node.scrollIntoView({{ block: 'center' }}); \
                 const box = node.getBoundingClientRect(); \
                 if (!box.width || !box.height) return ''; \
                 return (box.left + box.width / 2) + ',' + (box.top + box.height / 2); }})()",
                query_all(css::TEXT_BOXES)
            ),
        )
        .unwrap_or_default();

    if let Some((x, y)) = point(&centre) {
        let _ = page.click_point(connection, x, y);
        std::thread::sleep(TICK);
    }

    let focused = page.eval_bool(
        connection,
        &format!(
            "(() => {{ const node = {}[{index}]; if (!node) return false; node.focus(); \
             document.execCommand('selectAll', false, null); \
             document.execCommand('delete', false, null); return true; }})()",
            query_all(css::TEXT_BOXES)
        ),
    )?;
    if !focused {
        return Err(Failure::PageChanged(format!(
            "{} [{index}]",
            css::TEXT_BOXES
        )));
    }
    page.insert_text(connection, text)?;
    commit_text_box(connection, page, index);

    if text_box_holds(connection, page, index, text) {
        return Ok(());
    }

    let written = page.eval_bool(
        connection,
        &format!(
            "(() => {{ const node = {}[{index}]; if (!node) return false; \
             node.textContent = {}; \
             node.dispatchEvent(new InputEvent('input', {{ bubbles: true, composed: true }})); \
             node.dispatchEvent(new Event('change', {{ bubbles: true, composed: true }})); \
             return true; }})()",
            query_all(css::TEXT_BOXES),
            crate::cdp::js_string(text)
        ),
    )?;
    if !written {
        return Err(Failure::PageChanged(format!(
            "{} [{index}]",
            css::TEXT_BOXES
        )));
    }

    if text_box_holds(connection, page, index, text) {
        return Ok(());
    }
    Err(Failure::PageChanged(format!(
        "{} [{index}] would not take the text",
        css::TEXT_BOXES
    )))
}

fn real_click(connection: &mut Connection, page: &Page, selector: &str) -> bool {
    let centre = page
        .eval_string(
            connection,
            &format!(
                "(() => {{ const node = {}; if (!node) return '';                  node.scrollIntoView({{ block: 'center' }});                  const box = node.getBoundingClientRect();                  if (!box.width || !box.height) return '';                  return (box.left + box.width / 2) + ',' + (box.top + box.height / 2); }})()",
                query(selector)
            ),
        )
        .unwrap_or_default();

    let Some((x, y)) = point(&centre) else {
        return page.click(connection, selector).is_ok();
    };

    if page.click_point(connection, x, y).is_err() {
        return page.click(connection, selector).is_ok();
    }
    std::thread::sleep(TICK);
    true
}

fn commit_text_box(connection: &mut Connection, page: &Page, index: usize) {
    let _ = page.eval_bool(
        connection,
        &format!(
            "(() => {{ const node = {}[{index}]; if (!node) return false;              node.dispatchEvent(new InputEvent('input', {{ bubbles: true, composed: true }}));              node.dispatchEvent(new Event('change', {{ bubbles: true, composed: true }}));              node.blur();              node.dispatchEvent(new FocusEvent('blur', {{ bubbles: false }}));              node.dispatchEvent(new FocusEvent('focusout', {{ bubbles: true, composed: true }}));              return true; }})()",
            query_all(css::TEXT_BOXES)
        ),
    );
}

fn text_box_holds(connection: &mut Connection, page: &Page, index: usize, text: &str) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let wanted = text.trim();
    let held = format!(
        "(() => {{ const node = {}[{index}]; \
         return node ? (node.innerText || node.textContent || '') : ''; }})()",
        query_all(css::TEXT_BOXES)
    );
    loop {
        let seen = page.eval_string(connection, &held).unwrap_or_default();
        if seen.trim() == wanted {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(crate::cdp::POLL_INTERVAL);
    }
}

fn open_advanced(connection: &mut Connection, page: &Page) -> Result<(), Failure> {
    let deadline = std::time::Instant::now() + STEP_TIMEOUT;
    loop {
        if page
            .eval_bool(connection, &exists(css::ADVANCED))
            .unwrap_or(false)
        {
            return Ok(());
        }
        if !click_if_present(connection, page, css::SHOW_MORE) {
            return Err(Failure::PageChanged(css::SHOW_MORE.to_string()));
        }
        if std::time::Instant::now() >= deadline {
            return Err(Failure::Timeout("show more".to_string()));
        }
        std::thread::sleep(TICK);
    }
}

fn set_category(
    connection: &mut Connection,
    page: &Page,
    settings: &PublishSettings,
) -> Result<(), Failure> {
    if settings.category_id.trim().is_empty() {
        return Ok(());
    }

    if !click_if_present(connection, page, css::CATEGORY_SELECT) {
        return Err(Failure::PageChanged(css::CATEGORY_SELECT.to_string()));
    }

    page.wait_until(
        connection,
        &format!("(() => {}.length > 0)()", query_all(css::CATEGORY_ITEMS)),
        STEP_TIMEOUT,
        "category list",
    )?;

    let labels: Vec<String> = page
        .eval(
            connection,
            &format!(
                "(() => {}.map(node => (node.innerText || node.textContent || '').trim()))()",
                query_all(css::CATEGORY_ITEMS)
            ),
        )?
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();

    let Some(index) = labels.iter().position(|text| {
        crate::publish::is_category(text, &settings.category_id, &settings.category_label)
    }) else {
        let _ = page.eval_bool(
            connection,
            "(() => { document.body.click(); return true; })()",
        );
        return Ok(());
    };

    let clicked = page.eval_bool(
        connection,
        &format!(
            "(() => {{ const node = {}[{index}]; if (!node) return false; node.click(); return true; }})()",
            query_all(css::CATEGORY_ITEMS)
        ),
    )?;
    if clicked {
        Ok(())
    } else {
        Err(Failure::PageChanged(css::CATEGORY_ITEMS.to_string()))
    }
}

fn set_notify_subscribers(connection: &mut Connection, page: &Page, wanted: bool) {
    set_checkbox(connection, page, css::NOTIFY_SUBSCRIBERS, wanted);
}

fn wait_for_video_id(
    connection: &mut Connection,
    page: &Page,
    progress: &mut Progress,
) -> Result<String, Failure> {
    let deadline = std::time::Instant::now() + DIALOG_TIMEOUT;
    let shared = format!(
        "(() => {{ const node = {}; if (!node) return ''; return node.href || node.value || \
         node.textContent || ''; }})()",
        query(css::SHARE_URL)
    );
    loop {
        progress.still_wanted()?;
        let link = page.eval_string(connection, &shared).unwrap_or_default();
        if let Some(id) = video_id_from_link(&link) {
            return Ok(id);
        }
        if let Some(failure) = studio_error(connection, page) {
            return Err(failure);
        }
        if std::time::Instant::now() >= deadline {
            return Err(Failure::Timeout("video link".to_string()));
        }
        std::thread::sleep(crate::cdp::POLL_INTERVAL);
    }
}

fn watch_transfer(
    connection: &mut Connection,
    page: &Page,
    progress: &mut Progress,
) -> Result<(), Failure> {
    let deadline = std::time::Instant::now() + TRANSFER_TIMEOUT;
    let mut last = (Stage::Uploading, u32::MAX);
    let mut tidied: Option<std::time::Instant> = None;
    let script = transfer_script();
    loop {
        if tidied.is_none_or(|at| at.elapsed() >= TIDY_INTERVAL) {
            let _ = page.collect_garbage(connection);
            crate::chrome::trim_memory();
            tidied = Some(std::time::Instant::now());
        }
        let reading = TransferReading::of(&page.eval(connection, &script)?);

        if let Some(failure) = reading.failure() {
            return Err(failure);
        }

        let (stage, percent) = reading.progress(last);

        if (stage, percent) != last && percent != u32::MAX {
            last = (stage, percent);
            progress.tell(stage, percent)?;
        } else {
            progress.still_wanted()?;
        }

        if stage == Stage::Processing {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(Failure::Timeout("upload".to_string()));
        }
        std::thread::sleep(TRANSFER_TICK);
    }
}

fn finish(
    connection: &mut Connection,
    page: &Page,
    settings: &PublishSettings,
    progress: &mut Progress,
) -> Result<(), Failure> {
    walk_to_last_step(connection, page, progress)?;

    set_notify_subscribers(connection, page, settings.notify_subscribers);
    apply_visibility(connection, page, settings)?;

    wait_or_cancel(
        connection,
        page,
        &usable(css::DONE),
        STEP_TIMEOUT,
        "publish button",
        progress,
    )?;
    progress.still_wanted()?;
    if !real_click(connection, page, css::DONE) {
        return Err(Failure::PageChanged(css::DONE.to_string()));
    }
    std::thread::sleep(SETTLE);
    wait_until_published(connection, page)
}

fn walk_to_last_step(
    connection: &mut Connection,
    page: &Page,
    progress: &mut Progress,
) -> Result<(), Failure> {
    let deadline = std::time::Instant::now() + STEP_TIMEOUT * 4;
    let last = visible(css::DONE);
    let next = usable(css::NEXT);
    while !page.eval_bool(connection, &last).unwrap_or(false) {
        progress.still_wanted()?;
        if let Some(failure) = studio_error(connection, page) {
            return Err(failure);
        }
        if std::time::Instant::now() >= deadline {
            return Err(Failure::Timeout("upload steps".to_string()));
        }

        if page.eval_bool(connection, &next).unwrap_or(false) {
            real_click(connection, page, css::NEXT);
        }
        std::thread::sleep(crate::cdp::POLL_INTERVAL);
    }
    Ok(())
}

fn wait_until_published(connection: &mut Connection, page: &Page) -> Result<(), Failure> {
    let state = format!(
        "(() => {{ const onScreen = (node) => {{ if (!node) return false; \
         const box = node.getBoundingClientRect(); return box.width > 0 && box.height > 0; }}; \
         if (onScreen(document.querySelector({}))) return 'settled'; \
         const dialog = document.querySelector({}); \
         if (!onScreen(dialog)) return 'settled'; \
         if (!onScreen(dialog.querySelector({}))) return 'settled'; \
         return 'waiting'; }})()",
        crate::cdp::js_string(css::PUBLISHED_DIALOG),
        crate::cdp::js_string(css::DIALOG),
        crate::cdp::js_string(css::DONE)
    );

    let deadline = std::time::Instant::now() + PUBLISH_TIMEOUT;
    loop {
        if let Some(failure) = studio_error(connection, page) {
            return Err(failure);
        }
        answer_the_checks_prompt(connection, page);
        if page
            .eval_string(connection, &state)
            .map(|value| value.trim() == "settled")
            .unwrap_or(false)
        {
            wait_until_saved(connection, page);
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(Failure::Timeout("publish".to_string()));
        }
        std::thread::sleep(TICK);
    }
}

fn answer_the_checks_prompt(connection: &mut Connection, page: &Page) -> bool {
    let centre = page
        .eval_string(
            connection,
            &format!(
                "(() => {{ const wanted = {}; \
                 const nodes = Array.from(document.querySelectorAll('ytcp-button, tp-yt-paper-button, button')); \
                 const hit = nodes.find(node => (node.innerText || '').trim().toLowerCase() === wanted \
                 && node.getBoundingClientRect().width > 0 && node.getBoundingClientRect().height > 0); \
                 if (!hit) return ''; \
                 const box = hit.getBoundingClientRect(); \
                 return (box.left + box.width / 2) + ',' + (box.top + box.height / 2); }})()",
                crate::cdp::js_string(css::CONFIRM_LABEL)
            ),
        )
        .unwrap_or_default();

    let Some((x, y)) = point(&centre) else {
        return false;
    };

    if page.click_point(connection, x, y).is_err() {
        return false;
    }
    std::thread::sleep(SETTLE);
    true
}

fn wait_until_saved(connection: &mut Connection, page: &Page) {
    let saving = "(() => { const marks = Array.from(document.querySelectorAll('*'))          .filter(node => node.children.length === 0)          .map(node => (node.innerText || '').trim().toLowerCase());          return marks.some(text => text.startsWith('saving') || text.startsWith('сохранение'))          ? 'saving' : 'saved'; })()";

    let deadline = std::time::Instant::now() + SAVE_TIMEOUT;
    let mut quiet = Quiet::default();
    loop {
        let busy = page
            .eval_string(connection, saving)
            .map(|value| value.trim() != "saved")
            .unwrap_or(false);
        let now = std::time::Instant::now();
        if quiet.observe(busy, now) || now >= deadline {
            break;
        }
        std::thread::sleep(TICK);
    }
}

#[derive(Default)]
struct Quiet {
    since: Option<std::time::Instant>,
}

impl Quiet {
    fn observe(&mut self, busy: bool, now: std::time::Instant) -> bool {
        if busy {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        now.duration_since(since) >= SAVE_QUIET
    }
}

fn apply_visibility(
    connection: &mut Connection,
    page: &Page,
    settings: &PublishSettings,
) -> Result<(), Failure> {
    if let Some(publish_at) = settings
        .publish_at
        .as_deref()
        .filter(|at| !at.trim().is_empty())
    {
        return schedule(connection, page, publish_at);
    }

    let state = match settings.effective_privacy() {
        Privacy::Public => "public",
        Privacy::Unlisted => "unlisted",
        Privacy::Private => "private",
    };
    let selector = css::visibility(state);
    page.wait_until(
        connection,
        &visible(&selector),
        STEP_TIMEOUT,
        "visibility options",
    )?;
    if !real_click(connection, page, &selector) {
        click_if_present(connection, page, &selector);
    }
    page.wait_until(
        connection,
        &chosen(&selector),
        CHOICE_TIMEOUT,
        "visibility choice",
    )
    .map_err(|_| Failure::PageChanged(selector))
}

fn schedule(connection: &mut Connection, page: &Page, publish_at: &str) -> Result<(), Failure> {
    let wanted = WallClock::of_stamp(publish_at, Zone::Local)
        .ok_or_else(|| Failure::PageChanged(format!("schedule time {publish_at:?}")))?;
    if !click_if_present(connection, page, css::SCHEDULE_TOGGLE) {
        return Err(Failure::PageChanged(css::SCHEDULE_TOGGLE.to_string()));
    }
    page.wait_until(
        connection,
        &visible(css::SCHEDULE_DATE_TRIGGER),
        STEP_TIMEOUT,
        "schedule date",
    )
    .map_err(|_| Failure::PageChanged(css::SCHEDULE_DATE_TRIGGER.to_string()))?;

    let date = wanted.studio_date();
    type_date(connection, page, &date)?;
    let wanted_date = Some((wanted.year, wanted.month, wanted.day));
    let shown = settle_on(connection, page, &shown_date(), |shown| {
        read_studio_date(shown) == wanted_date
    });
    if read_studio_date(&shown) != wanted_date {
        return Err(Failure::PageChanged(format!(
            "schedule date: typed {date:?}, Studio shows {shown:?}"
        )));
    }

    let time = wanted.studio_time();
    page.fill(connection, css::SCHEDULE_TIME, &time)?;
    page.press(connection, Key::Enter)?;
    let wanted_time = Some((wanted.hour, wanted.minute));
    let shown = settle_on(connection, page, &value_of(css::SCHEDULE_TIME), |shown| {
        read_studio_time(shown) == wanted_time
    });
    if read_studio_time(&shown) != wanted_time {
        return Err(Failure::PageChanged(format!(
            "schedule time: typed {time:?}, Studio shows {shown:?}"
        )));
    }
    Ok(())
}

fn type_date(connection: &mut Connection, page: &Page, date: &str) -> Result<(), Failure> {
    let dialog = click_if_present(connection, page, css::SCHEDULE_DATE_TRIGGER)
        && page
            .wait_until(
                connection,
                &visible(css::SCHEDULE_DATE_INPUT),
                DATE_DIALOG_TIMEOUT,
                "date picker",
            )
            .is_ok();
    let field = if dialog {
        css::SCHEDULE_DATE_INPUT
    } else {
        css::SCHEDULE_DATE
    };
    page.fill(connection, field, date)?;
    page.press(connection, Key::Enter)?;
    Ok(())
}

fn settle_on(
    connection: &mut Connection,
    page: &Page,
    script: &str,
    accept: impl Fn(&str) -> bool,
) -> String {
    let deadline = std::time::Instant::now() + CHOICE_TIMEOUT;
    loop {
        let shown = page.eval_string(connection, script).unwrap_or_default();
        if accept(&shown) || std::time::Instant::now() >= deadline {
            return shown;
        }
        std::thread::sleep(crate::cdp::POLL_INTERVAL);
    }
}

const DATE_DIALOG_TIMEOUT: Duration = Duration::from_secs(5);

fn shown_date() -> String {
    format!(
        "(() => {{ const node = {}; if (!node) return ''; const input = node.querySelector('input'); \
         return ((input && input.value) || node.innerText || node.textContent || '').trim(); }})()",
        query(css::SCHEDULE_DATE_TRIGGER)
    )
}

fn value_of(selector: &str) -> String {
    format!(
        "(() => {{ const node = {}; return node ? String(node.value || '').trim() : ''; }})()",
        query(selector)
    )
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WallClock {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl WallClock {
    pub fn of_stamp(publish_at: &str, zone: Zone) -> Option<Self> {
        let wall = zone.wall_clock(crate::unix_from_iso(publish_at.trim())?);
        let (year, month, day) = crate::civil_from_unix(wall);
        let seconds = wall.rem_euclid(86_400);
        Some(Self {
            year,
            month,
            day,
            hour: (seconds / 3_600) as u32,
            minute: (seconds % 3_600 / 60) as u32,
        })
    }

    pub fn studio_date(&self) -> String {
        let month = MONTHS[(self.month.clamp(1, 12) - 1) as usize];
        format!("{month} {}, {}", self.day, self.year)
    }

    pub fn studio_time(&self) -> String {
        let (hour, half) = match self.hour {
            0 => (12, "AM"),
            1..=11 => (self.hour, "AM"),
            12 => (12, "PM"),
            later => (later - 12, "PM"),
        };
        format!("{hour}:{:02} {half}", self.minute)
    }
}

pub fn read_studio_date(text: &str) -> Option<(i64, u32, u32)> {
    let words: Vec<&str> = text
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();

    if let [year, month, day] = words.as_slice()
        && year.len() == 4
        && let (Ok(year), Ok(month), Ok(day)) = (year.parse(), month.parse(), day.parse())
    {
        return Some((year, month, day));
    }

    let month = words.iter().find_map(|word| month_number(word))?;
    let numbers: Vec<i64> = words.iter().filter_map(|word| word.parse().ok()).collect();
    let year = *numbers.iter().find(|number| **number >= 1_000)?;
    let day = *numbers.iter().find(|number| (1..=31).contains(*number))?;
    Some((year, month, day as u32))
}

fn month_number(word: &str) -> Option<u32> {
    if word.len() < 3
        || !word
            .chars()
            .all(|character| character.is_ascii_alphabetic())
    {
        return None;
    }
    let word = word.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|month| word.starts_with(&month.to_ascii_lowercase()))
        .map(|index| index as u32 + 1)
}

pub fn read_studio_time(text: &str) -> Option<(u32, u32)> {
    let lower = text.trim().to_ascii_lowercase();
    let (hour, rest) = lower.split_once(':')?;
    let hour: u32 = hour.trim().parse().ok()?;
    let minute: u32 = rest
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    let hour = match (lower.contains("am"), lower.contains("pm"), hour) {
        (true, false, 12) => 0,
        (false, true, 1..=11) => hour + 12,
        _ => hour,
    };
    (hour < 24 && minute < 60).then_some((hour, minute))
}

const DAILY_LIMIT: &str = "(() => (document.body.innerText || '').toLowerCase() \
                           .includes('daily upload limit'))()";

fn progress_label() -> String {
    format!(
        "(() => {{ const all = {}.map(node => (node.innerText || node.textContent || '') \
         .trim()); return all.find(text => text.includes('%')) || all[0] || ''; }})()",
        query_all(css::PROGRESS_LABEL)
    )
}

fn transfer_script() -> String {
    format!(
        "(() => {{ const attempt = (read, fallback) => {{ try {{ return read(); }} \
         catch (error) {{ return fallback; }} }}; \
         return {{ label: {}, limit: attempt(() => {DAILY_LIMIT}, false), \
         error: attempt(() => {}, '') }}; }})()",
        progress_label(),
        text_of(css::ERROR_SHORT)
    )
}

#[derive(Debug, Default, PartialEq, Eq)]
struct TransferReading {
    label: String,
    limit_hit: bool,
    error: String,
}

impl TransferReading {
    fn of(answer: &Value) -> Self {
        let text = |key: &str| {
            answer
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Self {
            label: text("label"),
            limit_hit: answer.get("limit") == Some(&Value::Bool(true)),
            error: text("error"),
        }
    }

    fn failure(&self) -> Option<Failure> {
        if self.limit_hit {
            return Some(Failure::Rejected("daily upload limit reached".to_string()));
        }
        (!self.error.is_empty()).then(|| Failure::Rejected(self.error.clone()))
    }

    fn progress(&self, last: (Stage, u32)) -> (Stage, u32) {
        match parse_percent(&self.label) {
            Some(percent) => (Stage::Uploading, percent),
            None if is_transfer_done(&self.label) => (Stage::Processing, 100),
            None => last,
        }
    }
}

fn studio_error(connection: &mut Connection, page: &Page) -> Option<Failure> {
    static SCRIPT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let script = SCRIPT.get_or_init(|| text_of(css::ERROR_SHORT));
    let message = page.eval_string(connection, script).unwrap_or_default();
    (!message.is_empty()).then_some(Failure::Rejected(message))
}

pub fn edit_url(video_id: &str) -> String {
    format!("https://studio.youtube.com/video/{video_id}/edit")
}

#[cfg(test)]
mod publish_tests {
    use super::*;
    #[test]
    fn the_still_checking_prompt_is_answered_by_publishing_anyway() {
        assert_eq!(css::CONFIRM_LABEL, "publish anyway");
    }

    #[test]
    fn the_follow_up_dialog_studio_shows_after_saving_is_recognised() {
        assert!(css::PUBLISHED_DIALOG.contains("ytcp-uploads-still-processing-dialog"));
        assert!(css::PUBLISHED_DIALOG.contains("ytcp-video-published-dialog"));
    }
}

#[cfg(test)]
mod language_tests {
    use super::*;

    #[test]
    fn studio_is_asked_for_english_so_labels_match_the_table() {
        assert!(UPLOAD_URL.contains("hl=en"));
        assert!(UPLOAD_URL.contains("persist_hl=1"));
        assert!(crate::login::STUDIO_URL.contains("hl=en"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_percentage_is_read_out_of_studios_own_progress_line() {
        assert_eq!(parse_percent("Uploading 43% ... 2 minutes left"), Some(43));
        assert_eq!(parse_percent("Загрузка 7 % завершена"), None);
        assert_eq!(parse_percent("Загружено 7%"), Some(7));
        assert_eq!(parse_percent("100% uploaded"), Some(100));
        assert_eq!(parse_percent("0%"), Some(0));
    }

    #[test]
    fn a_line_with_no_number_is_not_mistaken_for_zero_percent() {
        assert_eq!(parse_percent(""), None);
        assert_eq!(parse_percent("Upload complete"), None);
        assert_eq!(parse_percent("%"), None);
    }

    #[test]
    fn a_percentage_beyond_a_hundred_is_clamped_rather_than_shown() {
        assert_eq!(parse_percent("999%"), Some(100));
    }

    #[test]
    fn the_end_of_the_transfer_is_recognised_in_more_than_one_language() {
        assert!(is_transfer_done(
            "Upload complete ... processing will begin shortly"
        ));
        assert!(is_transfer_done("Обработка видео"));
        assert!(is_transfer_done("Checks complete. No issues found."));
        assert!(!is_transfer_done("Uploading 43% ..."));
        assert!(!is_transfer_done(""));
    }

    #[test]
    fn one_answer_from_the_page_carries_the_label_the_limit_and_the_error() {
        let answer = serde_json::json!({
            "label": "Uploading 43% ... 2 minutes left",
            "limit": false,
            "error": "",
        });
        let reading = TransferReading::of(&answer);
        assert_eq!(
            reading,
            TransferReading {
                label: "Uploading 43% ... 2 minutes left".to_string(),
                limit_hit: false,
                error: String::new(),
            }
        );
        assert_eq!(reading.failure(), None);
        assert_eq!(
            reading.progress((Stage::Uploading, u32::MAX)),
            (Stage::Uploading, 43)
        );
    }

    #[test]
    fn an_answer_with_missing_or_misshapen_values_reads_as_nothing_seen() {
        for answer in [
            Value::Null,
            serde_json::json!("Uploading 43%"),
            serde_json::json!({}),
            serde_json::json!({ "label": 43, "limit": "true", "error": null }),
            serde_json::json!({ "label": null, "limit": 1, "error": ["x"] }),
        ] {
            let reading = TransferReading::of(&answer);
            assert_eq!(reading, TransferReading::default(), "{answer}");
            assert_eq!(reading.failure(), None, "{answer}");
            assert_eq!(
                reading.progress((Stage::Uploading, 12)),
                (Stage::Uploading, 12),
                "{answer}"
            );
        }
    }

    #[test]
    fn the_daily_limit_is_reported_before_any_error_studio_shows() {
        let both = TransferReading::of(&serde_json::json!({
            "label": "Uploading 5%",
            "limit": true,
            "error": "Processing abandoned",
        }));
        assert_eq!(
            both.failure(),
            Some(Failure::Rejected("daily upload limit reached".to_string()))
        );

        let error_only = TransferReading::of(&serde_json::json!({
            "label": "Uploading 5%",
            "limit": false,
            "error": "Processing abandoned",
        }));
        assert_eq!(
            error_only.failure(),
            Some(Failure::Rejected("Processing abandoned".to_string()))
        );
    }

    #[test]
    fn the_label_decides_the_stage_and_an_unreadable_one_keeps_the_last() {
        let labelled = |label: &str| TransferReading {
            label: label.to_string(),
            ..Default::default()
        };
        let last = (Stage::Uploading, 60);
        assert_eq!(
            labelled("Uploading 61%").progress(last),
            (Stage::Uploading, 61)
        );
        assert_eq!(
            labelled("Upload complete ... processing will begin shortly").progress(last),
            (Stage::Processing, 100)
        );
        assert_eq!(labelled("").progress(last), last);
        assert_eq!(labelled("Preparing").progress(last), last);
    }

    #[test]
    fn the_transfer_is_read_with_one_script_that_holds_all_three_lookups() {
        let script = transfer_script();
        assert!(script.contains(&progress_label()));
        assert!(script.contains(DAILY_LIMIT));
        assert!(script.contains(&text_of(css::ERROR_SHORT)));
        let label = script.find("label:").expect("label");
        let limit = script.find("limit:").expect("limit");
        let error = script.find("error:").expect("error");
        assert!(label < limit && limit < error);
    }

    #[test]
    fn only_the_limit_and_error_lookups_are_allowed_to_fail_quietly() {
        let script = transfer_script();
        assert!(script.contains(&format!("label: {}", progress_label())));
        assert!(script.contains(&format!("limit: attempt(() => {DAILY_LIMIT}, false)")));
        assert!(script.contains(&format!(
            "error: attempt(() => {}, '')",
            text_of(css::ERROR_SHORT)
        )));
    }

    #[test]
    fn the_video_id_comes_out_of_whichever_link_studio_is_showing() {
        assert_eq!(
            video_id_from_link("https://youtu.be/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            video_id_from_link("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            video_id_from_link("https://studio.youtube.com/video/dQw4w9WgXcQ/edit"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn an_empty_or_unfinished_link_yields_no_id_so_the_wait_carries_on() {
        assert_eq!(video_id_from_link(""), None);
        assert_eq!(video_id_from_link("https://youtu.be/"), None);
        assert_eq!(video_id_from_link("https://youtu.be/short"), None);
        assert_eq!(video_id_from_link("https://www.youtube.com/"), None);
    }

    #[test]
    fn the_stages_are_ordered_the_way_an_upload_runs() {
        assert!(Stage::Opening < Stage::Uploading);
        assert!(Stage::Uploading < Stage::Processing);
        assert!(Stage::Processing < Stage::Publishing);
        assert!(Stage::Publishing < Stage::Done);
        assert_eq!(Stage::Uploading.message_key(), "youtube.stage.uploading");
    }

    #[test]
    fn a_cancel_answered_to_any_report_stops_the_upload_and_a_poll_repeats_the_last_stage() {
        let mut seen = Vec::new();
        {
            let mut calls = 0;
            let mut report = |stage: Stage, percent: u32| {
                seen.push((stage, percent));
                calls += 1;
                if calls >= 3 {
                    Control::Cancel
                } else {
                    Control::Continue
                }
            };
            let mut progress = Progress {
                report: &mut report,
                last: (Stage::Opening, 0),
            };

            assert_eq!(progress.tell(Stage::Opening, 0), Ok(()));
            assert_eq!(progress.tell(Stage::Uploading, 40), Ok(()));
            assert_eq!(
                progress.still_wanted(),
                Err(Failure::Cancelled),
                "a poll during a long wait is where the cancel is noticed"
            );
        }
        assert_eq!(
            seen,
            [
                (Stage::Opening, 0),
                (Stage::Uploading, 40),
                (Stage::Uploading, 40)
            ],
            "polling never moves the progress bar"
        );
    }

    #[test]
    fn a_utc_schedule_is_typed_as_the_local_clock_reads_it() {
        let moscow = Zone::Fixed(3 * 3_600);
        let wall = WallClock::of_stamp("2026-09-23T13:05:00Z", moscow).expect("stamp");
        assert_eq!(
            wall,
            WallClock {
                year: 2026,
                month: 9,
                day: 23,
                hour: 16,
                minute: 5
            }
        );
        assert_eq!(wall.studio_date(), "Sep 23, 2026");
        assert_eq!(wall.studio_time(), "4:05 PM");

        let late_evening_in_new_york =
            WallClock::of_stamp("2026-09-24T02:30:00Z", Zone::Fixed(-4 * 3_600)).expect("stamp");
        assert_eq!(
            late_evening_in_new_york.studio_date(),
            "Sep 23, 2026",
            "the date is the local one, not the UTC one"
        );
        assert_eq!(late_evening_in_new_york.studio_time(), "10:30 PM");

        assert_eq!(WallClock::of_stamp("2026-09-23 13:05", moscow), None);
    }

    #[test]
    fn midnight_and_noon_are_written_the_way_a_twelve_hour_clock_writes_them() {
        let at = |hour| WallClock {
            year: 2026,
            month: 1,
            day: 5,
            hour,
            minute: 0,
        };
        assert_eq!(at(0).studio_time(), "12:00 AM");
        assert_eq!(at(11).studio_time(), "11:00 AM");
        assert_eq!(at(12).studio_time(), "12:00 PM");
        assert_eq!(at(23).studio_time(), "11:00 PM");
        assert_eq!(at(0).studio_date(), "Jan 5, 2026");
    }

    #[test]
    fn the_date_studio_shows_back_is_understood_in_the_shapes_it_comes_in() {
        for shown in [
            "Sep 23, 2026",
            "September 23, 2026",
            "23 Sep 2026",
            "2026-09-23",
            "  Sep 23, 2026 \n",
        ] {
            assert_eq!(read_studio_date(shown), Some((2026, 9, 23)), "{shown:?}");
        }
        assert_eq!(read_studio_date(""), None, "nothing shown is not a match");
        assert_eq!(read_studio_date("Schedule"), None);
        assert_ne!(
            read_studio_date("Sep 24, 2026"),
            Some((2026, 9, 23)),
            "a picker that kept its own default is caught"
        );
    }

    #[test]
    fn the_time_studio_shows_back_is_understood_on_either_clock() {
        assert_eq!(read_studio_time("4:05 PM"), Some((16, 5)));
        assert_eq!(read_studio_time("4:05\u{202f}pm"), Some((16, 5)));
        assert_eq!(read_studio_time("16:05"), Some((16, 5)));
        assert_eq!(read_studio_time("12:00 AM"), Some((0, 0)));
        assert_eq!(read_studio_time("12:30 PM"), Some((12, 30)));
        assert_eq!(read_studio_time("9:15 AM"), Some((9, 15)));
        assert_eq!(read_studio_time(""), None);
        assert_eq!(read_studio_time("25:00"), None);
    }

    #[test]
    fn studio_counts_as_saved_only_once_it_has_stayed_quiet_for_the_whole_window() {
        let start = std::time::Instant::now();
        let mut quiet = Quiet::default();
        assert!(
            !quiet.observe(false, start),
            "the window has only just begun"
        );
        assert!(!quiet.observe(false, start + SAVE_QUIET / 2));
        assert!(
            !quiet.observe(true, start + SAVE_QUIET),
            "a fresh save restarts the wait"
        );
        let resumed = start + SAVE_QUIET + Duration::from_millis(1);
        assert!(!quiet.observe(false, resumed));
        assert!(!quiet.observe(false, resumed + SAVE_QUIET / 2));
        assert!(quiet.observe(false, resumed + SAVE_QUIET));
    }

    #[test]
    fn a_page_that_never_shows_a_saving_mark_waits_no_longer_than_the_quiet_window() {
        let start = std::time::Instant::now();
        let mut quiet = Quiet::default();
        assert!(!quiet.observe(false, start));
        assert!(quiet.observe(false, start + SAVE_QUIET));
        assert!(SAVE_QUIET < Duration::from_secs(6));
    }

    #[test]
    fn a_point_is_read_from_the_pair_the_page_answers_with() {
        assert_eq!(point("12.5,40"), Some((12.5, 40.0)));
        assert_eq!(point(" 3 , 4 "), Some((3.0, 4.0)));
        assert_eq!(point(""), None);
        assert_eq!(point("12.5"), None);
        assert_eq!(point("left,4"), None);
    }

    #[test]
    fn the_link_points_at_the_editor() {
        assert_eq!(
            edit_url("abc12345"),
            "https://studio.youtube.com/video/abc12345/edit"
        );
    }
}
