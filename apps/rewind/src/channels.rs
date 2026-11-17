use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use gpui::Context;
use parking_lot::Mutex;
use youtube::{
    Account, Accounts, Control, Failure, History, HistoryEntry, Privacy, PublishSettings, Queue,
    Session, Stage, Tab, TaskState, UploadStatus,
};

use crate::assets::Icon;
use crate::platform::http;
use crate::state::{Rewind, UiEvent};

const POLL: Duration = Duration::from_millis(250);
const PROFILE_MOVE_ATTEMPTS: u32 = 6;
const PROFILE_MOVE_BACKOFF: Duration = Duration::from_millis(250);
const AVATARS_DIR: &str = "avatars";
const PARALLEL_UPLOADS: usize = 10;
const REMOVE_ATTEMPTS: usize = 6;
const REMOVE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);
const PLAYER_RECHECK: std::time::Duration = std::time::Duration::from_secs(3);
const VIDEOS_DIR: &str = "videos";
const THUMBS_DIR: &str = "thumbs";
const THUMB_AT: Duration = Duration::from_secs(1);
const THUMB_WIDTH: u32 = 320;
const THUMB_HOST: &str = "i.ytimg.com";
const HTTP_OK: u32 = 200;

pub mod text {
    use youtube::{Failure, Privacy, Stage, TaskState, UploadStatus};

    pub const ACCOUNT_ADDED: &str = "Аккаунт YouTube добавлен";
    pub const QUEUED: &str = "Видео добавлено в очередь";
    pub const QUEUED_DETAIL: &str = "Загрузка идёт в фоне";
    pub const PUBLISHED: &str = "Видео опубликовано на YouTube";
    pub const UPLOAD_FAILED: &str = "Загрузка остановлена";
    pub const SIGN_IN_FAILED: &str = "Не удалось войти в YouTube";
    pub const FETCH_FAILED: &str = "Не удалось скачать видео с YouTube";
    pub fn queued_many(count: usize) -> String {
        format!("На YouTube отправляется видео: {count}")
    }

    pub const READ_FAILED: &str = "Не удалось прочитать данные видео из YouTube";
    pub const EDIT_SAVED: &str = "Изменения сохранены на YouTube";
    pub const EDIT_FAILED: &str = "Не удалось сохранить изменения на YouTube";
    pub const VIDEO_DELETED: &str = "Видео удалено с YouTube";
    pub const VIDEO_DELETE_FAILED: &str = "Не удалось удалить видео с YouTube";
    pub const SOURCE_REMOVED: &str = "Исходник удалён с компьютера";
    pub const SOURCE_KEPT: &str = "Не удалось удалить исходник";
    pub const PLAYER_IN_WAY: &str =
        "Закройте окно YouTube — загрузка начнётся сама, когда оно закроется";
    pub const PLAYER_BUSY: &str = "Дождитесь окончания загрузки на YouTube, потом откройте окно";
    pub const PLAYER_FAILED: &str = "Не удалось открыть плеер YouTube";

    pub fn failure(failure: &Failure) -> String {
        let detail = failure.note().detail;
        let template = match failure.message_key() {
            "youtube.error.browserLaunch" => "Браузер не запустился: {detail}",
            "youtube.error.cancelled" => "Отменено.",
            "youtube.error.interrupted" => {
                "Программа закрылась во время загрузки. Проверьте YouTube Studio, прежде чем повторять."
            }
            "youtube.error.io" => "Не удалось прочитать файл: {detail}",
            "youtube.error.noBrowser" => {
                "Браузер не найден. Для публикации нужен Chrome, Chromium или Edge."
            }
            "youtube.error.noChannel" => {
                "У этого аккаунта Google нет канала YouTube. Создайте канал и войдите снова."
            }
            "youtube.error.pageChanged" => {
                "На странице YouTube Studio не нашёлся нужный элемент: {detail}"
            }
            "youtube.error.protocol" => "Потеряна связь с браузером: {detail}",
            "youtube.error.rejected" => "YouTube отклонил видео: {detail}",
            "youtube.error.signInAbandoned" => {
                "Вход так и не завершён. Попробуйте снова, когда будете готовы."
            }
            "youtube.error.signedOut" => {
                "Сохранённая сессия завершилась. Войдите в этот аккаунт заново."
            }
            "youtube.error.timeout" => "YouTube Studio не ответил вовремя: {detail}",
            "youtube.error.wrongChannel" => {
                "Это другой канал ({detail}). Войдите в канал именно этого аккаунта."
            }
            _ => "Ошибка: {detail}",
        };
        template.replace("{detail}", &detail)
    }

    pub fn stage(stage: Stage, percent: u32) -> String {
        match stage {
            Stage::Opening => "Открываем YouTube Studio".to_owned(),
            Stage::Uploading => format!("Загрузка {percent}%"),
            Stage::Processing => "YouTube обрабатывает видео".to_owned(),
            Stage::Publishing => "Публикуем".to_owned(),
            Stage::Done => "Готово".to_owned(),
        }
    }

    pub const fn privacy(privacy: Privacy) -> &'static str {
        match privacy {
            Privacy::Public => "Открытый",
            Privacy::Unlisted => "По ссылке",
            Privacy::Private => "Приватный",
        }
    }

    pub fn status(status: &UploadStatus) -> &'static str {
        match status {
            UploadStatus::Uploaded => "Загружено",
            UploadStatus::Processing => "Обрабатывается на YouTube",
            UploadStatus::Processed => "Готово",
            UploadStatus::Rejected { .. } => "Отклонено YouTube",
            UploadStatus::Failed { .. } => "Ошибка",
            UploadStatus::Deleted => "Больше нет на YouTube",
        }
    }

    pub fn task(state: &TaskState) -> &'static str {
        match state {
            TaskState::Waiting => "Ждёт своей очереди",
            TaskState::Running => "Загружается",
            TaskState::Done { .. } => "Загружено",
            TaskState::Failed { .. } => "Загрузка остановлена",
            TaskState::Cancelled => "Отменено",
        }
    }
}

pub struct YouTube {
    pub directory: PathBuf,
    pub accounts: Accounts,
    pub history: History,
    pub queue: Queue,
    pub signing_in: bool,
    pub sign_in: Option<youtube::window::SignInWindow>,
    pub supported: bool,
    pub generation: u64,
    uploads: HashMap<String, Live>,
    workers: HashMap<String, usize>,
    browsers: HashMap<String, Browser>,
    watching: bool,
    fetching: HashMap<String, Fetch>,
    fetchers: HashMap<String, usize>,
    thumbnailing: bool,
    thumbs_tried: HashSet<String>,
    player: Option<(String, youtube::chrome::Browser)>,
    player_waiting: bool,
    player_noticed: bool,
    pub managing: HashMap<String, Managing>,
    pub details: HashMap<String, youtube::manage::Details>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Managing {
    Reading,
    Saving,
    Deleting,
}

type Browser = Arc<Mutex<Option<Session>>>;

#[derive(Clone, Default)]
struct Fetch {
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone)]
struct Live {
    progress: Arc<Mutex<(Stage, u32)>>,
    cancel: Arc<AtomicBool>,
}

impl Live {
    fn new() -> Self {
        Self {
            progress: Arc::new(Mutex::new((Stage::Opening, 0))),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl YouTube {
    pub fn load(directory: PathBuf) -> Self {
        let mut queue = Queue::load(&directory);
        queue.reclaim_interrupted();
        discard_partial_videos(&directory.join(VIDEOS_DIR));
        Self {
            accounts: Accounts::load(&directory),
            history: History::load(&directory),
            queue,
            directory,
            signing_in: false,
            sign_in: None,
            supported: youtube::session::is_supported(),
            generation: 0,
            uploads: HashMap::new(),
            workers: HashMap::new(),
            browsers: HashMap::new(),
            watching: false,
            fetching: HashMap::new(),
            fetchers: HashMap::new(),
            thumbnailing: false,
            thumbs_tried: HashSet::new(),
            player: None,
            player_waiting: false,
            player_noticed: false,
            managing: HashMap::new(),
            details: HashMap::new(),
        }
    }

    pub fn cached_video(&self, video_id: &str) -> PathBuf {
        self.directory
            .join(VIDEOS_DIR)
            .join(format!("{video_id}.mp4"))
    }

    pub fn thumbnail(&self, video_id: &str) -> Option<PathBuf> {
        Some(self.thumbnail_path(video_id)).filter(|path| path.exists())
    }

    pub fn thumbnail_path(&self, video_id: &str) -> PathBuf {
        self.directory
            .join(THUMBS_DIR)
            .join(format!("{video_id}.png"))
    }

    pub fn local_video(&self, entry: &HistoryEntry) -> Option<PathBuf> {
        entry
            .source_file
            .as_ref()
            .map(PathBuf::from)
            .filter(|path| path.exists())
            .or_else(|| Some(self.cached_video(&entry.video_id)).filter(|path| path.exists()))
    }

    pub fn fetch_progress(&self, video_id: &str) -> Option<u32> {
        self.fetching
            .get(video_id)
            .map(|fetch| fetch.progress.load(Ordering::Relaxed))
    }

    fn fetch_progresses(&self) -> Vec<(String, u32)> {
        let mut progress: Vec<(String, u32)> = self
            .fetching
            .iter()
            .map(|(id, fetch)| (id.clone(), fetch.progress.load(Ordering::Relaxed)))
            .collect();
        progress.sort_unstable();
        progress
    }

    fn save(&self) {
        self.accounts.save(&self.directory).ok();
        self.history.save(&self.directory).ok();
        self.queue.save(&self.directory).ok();
    }
}

fn adopt_profile(directory: &Path, from: &Path, account_id: &str) -> std::io::Result<()> {
    let to = youtube::chrome::profile_directory(directory, account_id);
    if from == to {
        return Ok(());
    }
    std::fs::remove_dir_all(&to).ok();
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, &to) {
            Ok(()) => return Ok(()),
            Err(error) if attempt + 1 >= PROFILE_MOVE_ATTEMPTS => return Err(error),
            Err(_) => {
                attempt += 1;
                std::thread::sleep(PROFILE_MOVE_BACKOFF * attempt);
            }
        }
    }
}

fn remembered(tasks: &[youtube::Task], entry: &HistoryEntry) -> youtube::manage::Details {
    let sent = tasks.iter().rev().find(|task| {
        matches!(&task.state, youtube::TaskState::Done { video_id } if *video_id == entry.video_id)
    });
    youtube::manage::Details {
        title: entry.title.clone(),
        description: sent.map_or_else(String::new, |task| task.settings.description.clone()),
        privacy: Some(entry.privacy),
    }
}

fn sign_in(
    directory: &Path,
    now: i64,
    window: &youtube::window::SignInWindow,
) -> Result<(Account, Option<Vec<u8>>), Failure> {
    let scratch = format!("pending-{now:x}");
    let (account, avatar) =
        youtube::session::sign_in_within(directory, &scratch, now, window, &mut || true)?;
    let from = youtube::chrome::profile_directory(directory, &scratch);
    adopt_profile(directory, &from, &account.id).map_err(|error| Failure::Io(error.to_string()))?;
    if account.title.trim().is_empty() || avatar.is_none() {
        return Ok(decorate(directory, account, avatar));
    }
    Ok((account, avatar))
}

fn decorate(
    directory: &Path,
    mut account: Account,
    avatar: Option<Vec<u8>>,
) -> (Account, Option<Vec<u8>>) {
    let Ok(shown) = youtube::session::refresh_decorations(directory, &account.id) else {
        return (account, avatar);
    };
    if !shown.title.trim().is_empty() {
        account.title = shown.title;
    }
    if !shown.handle.trim().is_empty() {
        account.handle = shown.handle;
    }
    if !shown.avatar_url.trim().is_empty() {
        account.avatar_url = shown.avatar_url;
    }
    (account, shown.avatar.or(avatar))
}

fn store_avatar(directory: &Path, account: &mut Account, avatar: Option<Vec<u8>>) {
    let Some(bytes) = avatar else {
        return;
    };
    let file = directory
        .join(AVATARS_DIR)
        .join(format!("{}.img", account.id));
    if file
        .parent()
        .is_some_and(|parent| std::fs::create_dir_all(parent).is_ok())
        && std::fs::write(&file, bytes).is_ok()
    {
        account.avatar_file = Some(file);
    }
}

fn is_undecorated(account: &Account) -> bool {
    account.title.trim().is_empty()
        || account
            .avatar_file
            .as_ref()
            .is_none_or(|file| !file.exists())
}

fn open_tab(directory: &Path, browser: &Browser, account_id: &str) -> Result<Tab, Failure> {
    let mut shared = browser.lock();
    let session = match shared.as_mut() {
        Some(session) => session,
        None => {
            let mut open = Session::open(directory, account_id, true)?;
            if !open.is_signed_in()? {
                open.close();
                return Err(Failure::SignedOut);
            }
            shared.insert(open)
        }
    };
    match session.tab() {
        Ok(tab) => Ok(tab),
        Err(failure) => {
            if let Some(mut dead) = shared.take() {
                dead.close();
            }
            Err(failure)
        }
    }
}

fn fetch_video(
    directory: &Path,
    browser: &Browser,
    account_id: &str,
    video_id: &str,
    destination: &Path,
    fetch: &Fetch,
) -> Result<(), Failure> {
    if fetch.cancel.load(Ordering::Relaxed) {
        return Err(Failure::Cancelled);
    }
    let mut tab = open_tab(directory, browser, account_id)?;
    let outcome = tab.download(video_id, destination, &mut |percent| {
        fetch.progress.store(percent, Ordering::Relaxed);
        if fetch.cancel.load(Ordering::Relaxed) {
            Control::Cancel
        } else {
            Control::Continue
        }
    });
    tab.close();
    outcome
}

fn discard_partial_videos(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let finished = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"));
        if !finished && path.is_file() {
            std::fs::remove_file(&path).ok();
        }
    }
}

fn make_thumbnail(video_id: &str, source: Option<&Path>, destination: &Path) -> bool {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    if let Some(source) = source
        && rewind_media::Runtime::enter().is_ok()
        && rewind_media::thumbnail(source, THUMB_AT, THUMB_WIDTH, destination).is_ok()
    {
        return true;
    }
    http::Session::new(http::BROWSER_AGENT)
        .and_then(|session| session.get(THUMB_HOST, &format!("/vi/{video_id}/mqdefault.jpg")))
        .ok()
        .filter(|response| response.status == HTTP_OK && !response.bytes.is_empty())
        .is_some_and(|response| std::fs::write(destination, &response.bytes).is_ok())
}

fn upload(
    directory: &Path,
    browser: &Browser,
    task: &youtube::Task,
    live: &Live,
) -> Result<String, Failure> {
    let mut tab = open_tab(directory, browser, &task.account_id)?;
    let outcome = tab.upload(&task.settings, &task.source, &mut |stage, percent| {
        *live.progress.lock() = (stage, percent);
        if live.cancel.load(Ordering::Relaxed) {
            Control::Cancel
        } else {
            Control::Continue
        }
    });
    tab.close();
    outcome
}

pub fn default_settings(source: &Path) -> PublishSettings {
    let title = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().trim_end_matches(".DVR").to_owned())
        .unwrap_or_default();
    PublishSettings {
        title,
        privacy: Privacy::Unlisted,
        ..PublishSettings::default()
    }
}

impl Rewind {
    pub fn decorate_youtube_accounts(&mut self, cx: &mut Context<Self>) {
        let pending: Vec<Account> = self
            .youtube
            .accounts
            .accounts
            .iter()
            .filter(|account| is_undecorated(account))
            .cloned()
            .collect();
        if pending.is_empty() || !self.youtube.supported {
            return;
        }
        let directory = self.youtube.directory.clone();
        cx.spawn(async move |this, cx| {
            let decorated = cx
                .background_executor()
                .spawn(async move {
                    pending
                        .into_iter()
                        .map(|account| {
                            let (mut account, avatar) = decorate(&directory, account, None);
                            store_avatar(&directory, &mut account, avatar);
                            account
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |rewind, cx| {
                for account in decorated {
                    if rewind.youtube.accounts.get(&account.id).is_some() {
                        rewind.youtube.accounts.upsert(account);
                    }
                }
                rewind.youtube.save();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn add_youtube_account(
        &mut self,
        placement: Option<youtube::window::Placement>,
        cx: &mut Context<Self>,
    ) {
        if self.youtube.signing_in {
            return;
        }
        let window = youtube::window::SignInWindow::new(placement);
        self.youtube.signing_in = true;
        self.youtube.sign_in = placement.map(|_| window.clone());
        cx.notify();
        let directory = self.youtube.directory.clone();
        let now = youtube::now_unix();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { sign_in(&directory, now, &window) })
                .await;
            this.update(cx, |rewind, cx| rewind.signed_in(outcome, cx))
                .ok();
        })
        .detach();
    }

    pub fn cancel_sign_in(&mut self) {
        if let Some(window) = &self.youtube.sign_in {
            window.cancel();
        }
    }

    fn signed_in(
        &mut self,
        outcome: Result<(Account, Option<Vec<u8>>), Failure>,
        cx: &mut Context<Self>,
    ) {
        self.youtube.signing_in = false;
        self.youtube.sign_in = None;
        match outcome {
            Ok((mut account, avatar)) => {
                store_avatar(&self.youtube.directory, &mut account, avatar);
                let title = account.display_name();
                let id = account.id.clone();
                self.youtube.accounts.upsert(account);
                self.youtube.accounts.select(&id);
                self.youtube.save();
                self.toast(Icon::Check, text::ACCOUNT_ADDED, Some(title), false, cx);
            }
            Err(Failure::Cancelled | Failure::SignInAbandoned) => {}
            Err(failure) => {
                self.toast(
                    Icon::Close,
                    text::SIGN_IN_FAILED,
                    Some(text::failure(&failure)),
                    true,
                    cx,
                );
            }
        }
        cx.notify();
    }

    pub fn remove_youtube_account(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.youtube.accounts.remove(id) {
            self.stop_uploads_for(id);
            if !self.youtube.workers.contains_key(id) {
                youtube::session::forget_profile(&self.youtube.directory, id).ok();
            }
            self.youtube.save();
            cx.notify();
        }
    }

    fn stop_uploads_for(&mut self, account_id: &str) {
        let tasks: Vec<String> = self
            .youtube
            .queue
            .tasks
            .iter()
            .filter(|task| task.account_id == account_id && task.state.is_active())
            .map(|task| task.id.clone())
            .collect();
        for id in tasks {
            match self.youtube.uploads.get(&id) {
                Some(live) => live.cancel.store(true, Ordering::Relaxed),
                None => {
                    self.youtube.queue.cancel(&id);
                }
            }
        }
    }

    pub fn select_youtube_account(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.youtube.accounts.select(id) {
            self.youtube.save();
            cx.notify();
        }
    }

    pub fn publish(
        &mut self,
        account_id: String,
        settings: PublishSettings,
        source: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let total = std::fs::metadata(&source).map_or(0, |meta| meta.len());
        self.youtube
            .queue
            .push(&account_id, settings, source, total, youtube::now_unix());
        self.youtube.save();
        self.toast(
            Icon::Replay,
            text::QUEUED,
            Some(text::QUEUED_DETAIL.to_owned()),
            false,
            cx,
        );
        self.pump_uploads(cx);
        cx.notify();
    }

    pub fn publish_many(
        &mut self,
        account_id: String,
        items: Vec<(PublishSettings, PathBuf)>,
        cx: &mut Context<Self>,
    ) {
        let count = items.len();
        for (settings, source) in items {
            let total = std::fs::metadata(&source).map_or(0, |meta| meta.len());
            self.youtube
                .queue
                .push(&account_id, settings, source, total, youtube::now_unix());
        }
        self.youtube.save();
        self.toast(
            Icon::Upload,
            text::queued_many(count),
            Some(text::QUEUED_DETAIL.to_owned()),
            false,
            cx,
        );
        self.pump_uploads(cx);
        cx.notify();
    }

    pub fn cancel_upload(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(live) = self.youtube.uploads.get(id) {
            live.cancel.store(true, Ordering::Relaxed);
        } else if self.youtube.queue.cancel(id) {
            self.youtube.save();
        }
        cx.notify();
    }

    pub fn retry_upload(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.youtube.queue.retry(id) {
            self.youtube.save();
            self.pump_uploads(cx);
            cx.notify();
        }
    }

    fn player_open(&mut self, account: &str) -> bool {
        match self.youtube.player.as_mut() {
            Some((owner, browser)) if owner == account => {
                if browser.is_running() {
                    true
                } else {
                    self.youtube.player = None;
                    false
                }
            }
            _ => false,
        }
    }

    fn wait_for_player(&mut self, cx: &mut Context<Self>) {
        if self.youtube.player_waiting {
            return;
        }
        self.youtube.player_waiting = true;
        if !std::mem::replace(&mut self.youtube.player_noticed, true) {
            self.toast(Icon::YouTube, text::PLAYER_IN_WAY, None, false, cx);
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PLAYER_RECHECK).await;
            this.update(cx, |rewind, cx| {
                rewind.youtube.player_waiting = false;
                rewind.pump_uploads(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn pump_uploads(&mut self, cx: &mut Context<Self>) {
        for account in self.youtube.queue.waiting_accounts() {
            if self.player_open(&account) {
                self.wait_for_player(cx);
                continue;
            }
            self.youtube.player_noticed = false;
            let waiting = self
                .youtube
                .queue
                .tasks
                .iter()
                .filter(|task| task.state == TaskState::Waiting && task.account_id == account)
                .count();
            let busy = self.youtube.workers.get(&account).copied().unwrap_or(0);
            for _ in busy..(busy + waiting).min(PARALLEL_UPLOADS) {
                *self.youtube.workers.entry(account.clone()).or_default() += 1;
                self.spawn_upload_worker(account.clone(), cx);
            }
        }
        self.watch_uploads(cx);
    }

    fn next_upload(&mut self, account: &str) -> Option<(youtube::Task, Live)> {
        let task = self.youtube.queue.next_waiting_for(account).cloned()?;
        self.youtube.queue.start(&task.id);
        self.youtube.save();
        let live = Live::new();
        self.youtube.uploads.insert(task.id.clone(), live.clone());
        Some((task, live))
    }

    fn spawn_upload_worker(&mut self, account: String, cx: &mut Context<Self>) {
        let directory = self.youtube.directory.clone();
        let browser = self
            .youtube
            .browsers
            .entry(account.clone())
            .or_default()
            .clone();
        cx.spawn(async move |this, cx| {
            while let Ok(Some((task, live))) =
                this.update(cx, |rewind, _| rewind.next_upload(&account))
            {
                let directory = directory.clone();
                let job = task.clone();
                let shared = browser.clone();
                let outcome = cx
                    .background_executor()
                    .spawn(async move { upload(&directory, &shared, &job, &live) })
                    .await;
                let alive = this.update(cx, |rewind, cx| rewind.upload_finished(task, outcome, cx));
                if alive.is_err() {
                    break;
                }
            }
            this.update(cx, |rewind, cx| {
                let left = rewind.youtube.workers.get_mut(&account).map(|count| {
                    *count = count.saturating_sub(1);
                    *count
                });
                if left.unwrap_or(0) > 0 {
                    return;
                }
                rewind.youtube.workers.remove(&account);
                rewind.release_browser(&account, cx);
                if rewind.youtube.accounts.get(&account).is_none() {
                    youtube::session::forget_profile(&rewind.youtube.directory, &account).ok();
                }
                rewind.pump_uploads(cx);
            })
            .ok();
        })
        .detach();
    }

    fn watch_uploads(&mut self, cx: &mut Context<Self>) {
        if self.youtube.watching || self.youtube.workers.is_empty() {
            return;
        }
        self.youtube.watching = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let busy = this.update(cx, |rewind, cx| {
                    rewind.sync_upload_progress(cx);
                    let busy = !rewind.youtube.workers.is_empty();
                    rewind.youtube.watching = busy;
                    busy
                });
                if !matches!(busy, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn sync_upload_progress(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut advanced = false;
        for (id, live) in &self.youtube.uploads {
            let (stage, percent) = *live.progress.lock();
            let before = self.youtube.queue.get(id).map(|task| task.stage);
            changed |= self.youtube.queue.progress(id, stage, percent);
            advanced |= before != self.youtube.queue.get(id).map(|task| task.stage);
        }
        if advanced {
            self.youtube.save();
        }
        if changed {
            cx.notify();
        }
    }

    fn upload_finished(
        &mut self,
        task: youtube::Task,
        outcome: Result<String, Failure>,
        cx: &mut Context<Self>,
    ) {
        self.youtube.uploads.remove(&task.id);
        match outcome {
            Ok(video_id) => {
                self.youtube.queue.finish(&task.id, &video_id);
                if let Some(original) = task.settings.delete_source.clone() {
                    self.discard_uploaded(original, video_id.clone(), cx);
                }
                let kept = self.keep_uploaded(&task.source, &video_id);
                self.youtube.history.upsert(HistoryEntry {
                    video_id,
                    account_id: task.account_id.clone(),
                    title: task.settings.title.clone(),
                    uploaded_at: youtube::now_unix(),
                    privacy: task.settings.effective_privacy(),
                    status: UploadStatus::Uploaded,
                    source_file: Some(kept.display().to_string()),
                    scheduled_for: task.settings.publish_at.clone(),
                    bytes: task.total,
                });
                self.youtube.generation += 1;
                self.ensure_youtube_thumbnails(cx);
                self.toast(
                    Icon::Check,
                    text::PUBLISHED,
                    Some(task.settings.title),
                    false,
                    cx,
                );
            }
            Err(failure) => {
                self.youtube.queue.fail(&task.id, &failure);
                if !matches!(failure, Failure::Cancelled) {
                    self.toast(
                        Icon::Close,
                        text::UPLOAD_FAILED,
                        Some(text::failure(&failure)),
                        true,
                        cx,
                    );
                }
            }
        }
        self.youtube.save();
        cx.notify();
    }

    fn discard_uploaded(&mut self, original: PathBuf, video_id: String, cx: &mut Context<Self>) {
        if self.uploading(&original) {
            return;
        }
        let thumbnail = self.youtube.thumbnail_path(&video_id);
        self.youtube.thumbs_tried.insert(video_id.clone());
        cx.spawn(async move |this, cx| {
            let target = original.clone();
            let removed = cx
                .background_executor()
                .spawn(async move {
                    if !thumbnail.exists() {
                        make_thumbnail(&video_id, Some(&target), &thumbnail);
                    }
                    (0..REMOVE_ATTEMPTS).any(|attempt| {
                        if attempt > 0 {
                            std::thread::sleep(REMOVE_BACKOFF);
                        }
                        std::fs::remove_file(&target).is_ok() || !target.exists()
                    })
                })
                .await;
            this.update(cx, |rewind, cx| {
                rewind.youtube.generation += 1;
                if !removed {
                    rewind.toast(
                        Icon::Close,
                        text::SOURCE_KEPT,
                        Some(original.display().to_string()),
                        true,
                        cx,
                    );
                    return;
                }
                rewind.forget_thumbnail(&original);
                rewind.gallery.retain(|item| item.path != original);
                rewind.gallery_generation += 1;
                rewind.youtube.generation += 1;
                rewind.toast(
                    Icon::Delete,
                    text::SOURCE_REMOVED,
                    original
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned()),
                    false,
                    cx,
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn keep_uploaded(&self, source: &Path, video_id: &str) -> PathBuf {
        if !crate::editor_files::is_scratch(source) {
            return source.to_path_buf();
        }
        let kept = self.youtube.cached_video(video_id);
        let moved = kept
            .parent()
            .is_some_and(|parent| std::fs::create_dir_all(parent).is_ok())
            && std::fs::rename(source, &kept).is_ok();
        if moved {
            kept
        } else {
            std::fs::remove_file(source).ok();
            source.to_path_buf()
        }
    }

    fn run_managed<T: Send + 'static>(
        &mut self,
        video_id: &str,
        kind: Managing,
        job: impl FnOnce(&mut youtube::Tab, &str) -> Result<T, Failure> + Send + 'static,
        done: impl FnOnce(&mut Self, Result<T, Failure>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.published(video_id) else {
            return;
        };
        if self.youtube.managing.contains_key(video_id) {
            return;
        }
        if self.player_open(&entry.account_id) {
            self.toast(Icon::YouTube, text::PLAYER_IN_WAY, None, false, cx);
            return;
        }
        self.youtube.managing.insert(video_id.to_owned(), kind);
        *self
            .youtube
            .fetchers
            .entry(entry.account_id.clone())
            .or_default() += 1;
        let directory = self.youtube.directory.clone();
        let browser = self
            .youtube
            .browsers
            .entry(entry.account_id.clone())
            .or_default()
            .clone();
        let (account, video) = (entry.account_id, video_id.to_owned());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (from, id) = (account.clone(), video.clone());
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let mut tab = open_tab(&directory, &browser, &from)?;
                    let outcome = job(&mut tab, &id);
                    tab.close();
                    outcome
                })
                .await;
            this.update(cx, |rewind, cx| {
                rewind.youtube.managing.remove(&video);
                let left = rewind.youtube.fetchers.get_mut(&account).map(|count| {
                    *count = count.saturating_sub(1);
                    *count
                });
                if left == Some(0) {
                    rewind.youtube.fetchers.remove(&account);
                }
                rewind.release_browser(&account, cx);
                done(rewind, outcome, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn load_published(&mut self, video_id: &str, cx: &mut Context<Self>) {
        if self.youtube.details.contains_key(video_id) {
            return;
        }
        let id = video_id.to_owned();
        self.run_managed(
            video_id,
            Managing::Reading,
            |tab, video| tab.read_video(video),
            move |rewind, outcome, cx| match outcome {
                Ok(details) => {
                    rewind.youtube.details.insert(id, details);
                }
                Err(failure) => rewind.toast(
                    Icon::Close,
                    text::READ_FAILED,
                    Some(text::failure(&failure)),
                    true,
                    cx,
                ),
            },
            cx,
        );
    }

    pub fn save_published(
        &mut self,
        video_id: &str,
        changes: youtube::manage::Changes,
        cx: &mut Context<Self>,
    ) {
        if changes.is_empty() {
            return;
        }
        let id = video_id.to_owned();
        let sent = changes.clone();
        self.run_managed(
            video_id,
            Managing::Saving,
            move |tab, video| tab.edit_video(video, &sent),
            move |rewind, outcome, cx| match outcome {
                Ok(()) => rewind.apply_saved(&id, &changes, cx),
                Err(failure) => rewind.toast(
                    Icon::Close,
                    text::EDIT_FAILED,
                    Some(text::failure(&failure)),
                    true,
                    cx,
                ),
            },
            cx,
        );
    }

    fn apply_saved(
        &mut self,
        id: &str,
        changes: &youtube::manage::Changes,
        cx: &mut Context<Self>,
    ) {
        if let Some(entry) = self
            .youtube
            .history
            .entries
            .iter_mut()
            .find(|entry| entry.video_id == id)
        {
            if let Some(title) = &changes.title {
                entry.title = title.clone();
            }
            if let Some(privacy) = changes.privacy {
                entry.privacy = privacy;
            }
        }
        if let Some(details) = self.youtube.details.get_mut(id) {
            if let Some(title) = &changes.title {
                details.title = title.clone();
            }
            if let Some(description) = &changes.description {
                details.description = description.clone();
            }
            if let Some(privacy) = changes.privacy {
                details.privacy = Some(privacy);
            }
        }
        if let Some(picture) = &changes.thumbnail {
            let cached = self.youtube.thumbnail_path(id);
            if let Some(parent) = cached.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            if std::fs::copy(picture, &cached).is_err() {
                std::fs::remove_file(&cached).ok();
                self.youtube.thumbs_tried.remove(id);
                self.ensure_youtube_thumbnails(cx);
            }
        }
        self.youtube.save();
        self.youtube.generation += 1;
        self.toast(Icon::Check, text::EDIT_SAVED, None, false, cx);
    }

    pub fn delete_published(&mut self, video_id: &str, cx: &mut Context<Self>) {
        let id = video_id.to_owned();
        self.run_managed(
            video_id,
            Managing::Deleting,
            |tab, video| tab.delete_video(video),
            move |rewind, outcome, cx| match outcome {
                Ok(()) => {
                    rewind.youtube.history.remove(&id);
                    rewind.youtube.details.remove(&id);
                    rewind.youtube.save();
                    rewind.youtube.generation += 1;
                    rewind.toast(Icon::Delete, text::VIDEO_DELETED, None, false, cx);
                }
                Err(failure) => rewind.toast(
                    Icon::Close,
                    text::VIDEO_DELETE_FAILED,
                    Some(text::failure(&failure)),
                    true,
                    cx,
                ),
            },
            cx,
        );
    }

    fn release_browser(&mut self, account: &str, cx: &mut Context<Self>) {
        if self.youtube.workers.contains_key(account) || self.youtube.fetchers.contains_key(account)
        {
            return;
        }
        if let Some(browser) = self.youtube.browsers.remove(account)
            && let Some(mut session) = browser.lock().take()
        {
            cx.background_executor()
                .spawn(async move { session.close() })
                .detach();
        }
    }

    pub fn has_local_video(&self, video_id: &str) -> bool {
        self.youtube
            .history
            .entries
            .iter()
            .find(|entry| entry.video_id == video_id)
            .is_some_and(|entry| self.youtube.local_video(entry).is_some())
    }

    pub fn play_published(&mut self, video_id: &str, cx: &mut Context<Self>) {
        if let Some(entry) = self.published(video_id) {
            self.launch_window(&entry.account_id, entry.url(), cx);
        }
    }

    pub fn forget_published(&mut self, video_id: &str, cx: &mut Context<Self>) {
        if self.youtube.history.remove(video_id) {
            self.youtube.save();
            cx.notify();
        }
    }

    pub fn remembered(&self, video_id: &str) -> Option<youtube::manage::Details> {
        if let Some(details) = self.youtube.details.get(video_id) {
            return Some(details.clone());
        }
        let entry = self.published(video_id)?;
        Some(remembered(&self.youtube.queue.tasks, &entry))
    }

    pub fn published(&self, video_id: &str) -> Option<HistoryEntry> {
        self.youtube
            .history
            .entries
            .iter()
            .find(|entry| entry.video_id == video_id)
            .cloned()
    }

    fn launch_window(&mut self, account_id: &str, url: String, cx: &mut Context<Self>) {
        if self.youtube.browsers.contains_key(account_id) {
            self.toast(Icon::YouTube, text::PLAYER_BUSY, None, false, cx);
            return;
        }
        self.youtube.player = None;
        let profile = youtube::chrome::profile_directory(&self.youtube.directory, account_id);
        let page = format!("--app={url}");
        let account = account_id.to_owned();
        cx.spawn(async move |this, cx| {
            let launched = cx
                .background_executor()
                .spawn(async move { youtube::chrome::Browser::launch_at(&profile, false, &page) })
                .await;
            this.update(cx, |rewind, cx| match launched {
                Ok(browser) => rewind.youtube.player = Some((account, browser)),
                Err(failure) => rewind.toast(
                    Icon::Close,
                    text::PLAYER_FAILED,
                    Some(text::failure(&failure)),
                    true,
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    pub fn watch_video(&mut self, video_id: &str, cx: &mut Context<Self>) -> Option<PathBuf> {
        let entry = self
            .youtube
            .history
            .entries
            .iter()
            .find(|entry| entry.video_id == video_id)?
            .clone();
        if let Some(local) = self.youtube.local_video(&entry) {
            return Some(local);
        }
        if self.youtube.fetching.contains_key(video_id) {
            return None;
        }
        if self.player_open(&entry.account_id) {
            self.toast(Icon::YouTube, text::PLAYER_IN_WAY, None, false, cx);
            return None;
        }
        let fetch = Fetch::default();
        let idle = self.youtube.fetching.is_empty();
        self.youtube
            .fetching
            .insert(video_id.to_owned(), fetch.clone());
        *self
            .youtube
            .fetchers
            .entry(entry.account_id.clone())
            .or_default() += 1;
        let directory = self.youtube.directory.clone();
        let destination = self.youtube.cached_video(video_id);
        let browser = self
            .youtube
            .browsers
            .entry(entry.account_id.clone())
            .or_default()
            .clone();
        let (account, video) = (entry.account_id, video_id.to_owned());
        cx.spawn(async move |this, cx| {
            let target = destination.clone();
            let (from, id) = (account.clone(), video.clone());
            let outcome = cx
                .background_executor()
                .spawn(
                    async move { fetch_video(&directory, &browser, &from, &id, &target, &fetch) },
                )
                .await;
            this.update(cx, |rewind, cx| {
                rewind.youtube.fetching.remove(&video);
                let left = rewind.youtube.fetchers.get_mut(&account).map(|count| {
                    *count = count.saturating_sub(1);
                    *count
                });
                if left == Some(0) {
                    rewind.youtube.fetchers.remove(&account);
                }
                rewind.release_browser(&account, cx);
                rewind.youtube.generation += 1;
                match outcome {
                    Ok(()) => cx.emit(UiEvent::OpenVideo(destination)),
                    Err(Failure::Cancelled) => {}
                    Err(failure) => rewind.toast(
                        Icon::Close,
                        text::FETCH_FAILED,
                        Some(text::failure(&failure)),
                        true,
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        if idle {
            cx.spawn(async move |this, cx| {
                let mut shown = Vec::new();
                loop {
                    cx.background_executor().timer(POLL).await;
                    let busy = this.update(cx, |rewind, cx| {
                        let progress = rewind.youtube.fetch_progresses();
                        if progress != shown {
                            shown = progress;
                            cx.notify();
                        }
                        !rewind.youtube.fetching.is_empty()
                    });
                    if !busy.unwrap_or(false) {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
        None
    }

    pub fn cancel_fetch(&mut self, video_id: &str, cx: &mut Context<Self>) {
        if let Some(fetch) = self.youtube.fetching.get(video_id) {
            fetch.cancel.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    pub fn ensure_youtube_thumbnails(&mut self, cx: &mut Context<Self>) {
        if self.youtube.thumbnailing {
            return;
        }
        let missing: Vec<(String, Option<PathBuf>, PathBuf)> = self
            .youtube
            .history
            .entries
            .iter()
            .filter(|entry| {
                self.youtube.thumbnail(&entry.video_id).is_none()
                    && !self.youtube.thumbs_tried.contains(&entry.video_id)
            })
            .map(|entry| {
                (
                    entry.video_id.clone(),
                    self.youtube.local_video(entry),
                    self.youtube.thumbnail_path(&entry.video_id),
                )
            })
            .collect();
        if missing.is_empty() {
            return;
        }
        self.youtube.thumbnailing = true;
        self.youtube
            .thumbs_tried
            .extend(missing.iter().map(|(id, _, _)| id.clone()));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    for (id, source, destination) in missing {
                        make_thumbnail(&id, source.as_deref(), &destination);
                    }
                })
                .await;
            this.update(cx, |rewind, cx| {
                rewind.youtube.thumbnailing = false;
                rewind.youtube.generation += 1;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(video_id: &str, title: &str) -> HistoryEntry {
        HistoryEntry {
            video_id: video_id.to_owned(),
            account_id: "channel".to_owned(),
            title: title.to_owned(),
            uploaded_at: 0,
            privacy: youtube::Privacy::Unlisted,
            status: youtube::UploadStatus::default(),
            source_file: None,
            scheduled_for: None,
            bytes: 0,
        }
    }

    fn sent(video_id: &str, description: &str) -> youtube::Task {
        let settings = youtube::PublishSettings {
            description: description.to_owned(),
            ..youtube::PublishSettings::default()
        };
        let mut task = youtube::Task::new(
            "task".to_owned(),
            "channel".to_owned(),
            settings,
            PathBuf::from("clip.mp4"),
            0,
            0,
        );
        task.state = youtube::TaskState::Done {
            video_id: video_id.to_owned(),
        };
        task
    }

    #[test]
    fn a_published_video_is_remembered_with_what_it_was_uploaded_with() {
        let tasks = [sent("other", "чужое"), sent("video", "описание")];
        let known = remembered(&tasks, &entry("video", "название"));
        assert_eq!(known.title, "название");
        assert_eq!(known.description, "описание");
        assert_eq!(known.privacy, Some(youtube::Privacy::Unlisted));
    }

    #[test]
    fn a_video_without_a_queue_task_keeps_its_title_and_an_empty_description() {
        let known = remembered(&[sent("other", "чужое")], &entry("video", "название"));
        assert_eq!(known.title, "название");
        assert_eq!(known.description, "");
    }
}
