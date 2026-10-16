use std::io::stdout;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{collections::HashMap, sync::Arc, sync::Condvar, sync::Mutex};
use std::{fs, thread};

pub mod events;
pub mod inputs;
pub mod vim_input;

pub use crate::app;
use crate::app::events::{AppEvent, AppInput};
use crate::db;
use crate::key_handler::KeybindHandler;
use crate::ui;
use crate::vim;
// use crate::key_handler;

use arboard::Clipboard;
use db::DatabaseHandler;
use directories::ProjectDirs;
use log::{debug, error, info, trace};
use notify_rust::Notification;
use ratatui::crossterm::ExecutableCommand;
use ratatui::crossterm::event;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::Rect;
use ratatui::widgets::{Block, ListState};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, ResizeEncodeRender};
use ratatui_textarea::TextArea;
use ui::message_list::MessageListState;
use ui::message_list::{IMAGE_HEIGHT, IMAGE_WIDTH};
use vim::Vim;
use whatsrust as wr;

use crate::ui::text_input::TextInput;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputReaderState {
    Running,
    Pausing,
    Paused,
    Stopped,
}

pub enum SelectedWidget {
    ChatList,
    Input,
    MessageList,
    MessageView,
    StickerPicker,
}

#[derive(Clone, Debug)]
pub struct StickerItem {
    pub path: PathBuf,
    pub filename: String,
    pub is_favorite: bool,
}

#[derive(Clone, Debug)]
pub struct Chat {
    pub jid: wr::JID,
    pub last_message_time: Option<i64>,
}

#[derive(Debug)]
pub enum FileMeta {
    Loaded,
    Loading,
    LoadFailed,
    Downloaded,
    Downloading,
    DownloadFailed,
}

pub enum Metadata {
    File(FileMeta),
}

pub struct App<'a> {
    pub db_handler: DatabaseHandler,
    pub media_path: PathBuf,
    pub whatsmeow_db: PathBuf,

    pub messages: HashMap<wr::MessageId, wr::Message>,
    pub chats: HashMap<wr::JID, Chat>,

    // Maps JID to display name
    pub contacts: HashMap<wr::JID, Arc<str>>,

    pub clipboard: Clipboard,

    pub chat_messages: HashMap<wr::JID, Vec<wr::MessageId>>,

    pub sorted_chats: Vec<wr::JID>,
    pub chat_list_state: ListState,

    pub history_sync_percent: Option<u8>,

    pub quoting_message: Option<wr::Message>,
    pub attached_file: Option<(Arc<str>, wr::FileKind)>,
    pub message_list_state: MessageListState,
    pub metadata: HashMap<wr::MessageId, Metadata>,
    pub image_cache: HashMap<Arc<str>, StatefulProtocol>,
    pub default_protocol_type: ProtocolType,
    pub picker: Arc<Mutex<Picker>>,

    pub selected_widget: SelectedWidget,

    pub kh: KeybindHandler,

    pub show_logs: bool,

    pub vim: Vim,
    pub input_widget: TextArea<'a>,
    pub input_border: Block<'a>,
    pub visual_line_anchor: Option<usize>,

    pub contact_search_active: bool,
    pub contact_search: TextInput,
    pub filtered_chats: Vec<wr::JID>,

    pub sticker_list: Vec<StickerItem>,
    pub sticker_list_state: ListState,

    pub should_quit: bool,

    pub tx: mpsc::Sender<AppInput>,
    pub rx: mpsc::Receiver<AppInput>,
    input_reader_control: Arc<(Mutex<InputReaderState>, Condvar)>,
}

impl Default for App<'_> {
    fn default() -> Self {
        let mut input_widget = TextArea::default();
        // input_widget.set_cursor_line_style(vim::Mode::Nor::default());
        input_widget.set_cursor_style(vim::Mode::Insert.cursor_style());
        // input_widget.set_block(vim::Mode::Normal.block());
        input_widget.set_placeholder_text("Type a message...");

        let picker = Picker::from_query_stdio().unwrap_or_else(|err| {
            // Fallback for non-interactive environments (e.g. CI, piped stdio).
            log::warn!(
                "Failed to query terminal image capabilities; falling back to halfblocks: {err}"
            );
            Picker::halfblocks()
        });
        let default_protocol_type = picker.protocol_type();

        let project_dirs = ProjectDirs::from("com", "nullptr", "wstui").unwrap();
        let data_dir = project_dirs.data_dir();
        fs::create_dir_all(data_dir).unwrap();

        let (tx, rx) = mpsc::channel::<AppInput>();

        Self {
            db_handler: DatabaseHandler::new(&data_dir.join("whatsapp.db")),
            media_path: data_dir.join("media"),
            whatsmeow_db: data_dir.join("whatsmeow.db"),

            clipboard: Clipboard::new().unwrap(),

            messages: HashMap::new(),
            chats: HashMap::new(),
            contacts: HashMap::new(),
            chat_messages: HashMap::new(),

            sorted_chats: Vec::new(),
            chat_list_state: ListState::default(),

            message_list_state: MessageListState::default(),
            metadata: HashMap::new(),
            history_sync_percent: None,
            image_cache: HashMap::new(),
            default_protocol_type,
            quoting_message: None,
            attached_file: None,
            picker: Arc::new(Mutex::new(picker)),
            selected_widget: SelectedWidget::ChatList,

            kh: KeybindHandler::default(),

            contact_search_active: false,
            contact_search: TextInput::new(),
            filtered_chats: Vec::new(),

            sticker_list: Vec::new(),
            sticker_list_state: ListState::default(),

            show_logs: false,
            vim: Vim::new(vim::Mode::Insert),
            input_border: vim::Mode::Insert.block(),
            input_widget,
            visual_line_anchor: None,
            should_quit: false,
            tx,
            rx,
            input_reader_control: Arc::new((Mutex::new(InputReaderState::Running), Condvar::new())),
        }
    }
}

impl App<'_> {
    pub fn run(&mut self, phone: Option<String>) {
        self.db_handler.init();
        self.load_data_from_db();
        self.sort_chats();

        wr::new_client(self.whatsmeow_db.to_str().unwrap());

        {
            let tx = self.tx.clone();
            wr::set_log_handler(move |msg, level| {
                let level = match level {
                    0 => log::Level::Error,
                    1 => log::Level::Warn,
                    2 => log::Level::Info,
                    3 => log::Level::Debug,
                    _ => log::Level::Trace,
                };
                log::log!(level, "{msg}");
                tx.send(AppInput::Draw).unwrap();
            });
        }
        {
            let tx = self.tx.clone();
            wr::set_event_handler(move |event| {
                tx.send(AppInput::WhatsApp(event)).unwrap();
            })
        }
        {
            let tx = self.tx.clone();
            wr::set_message_handler(move |message, is_sync| {
                tx.send(AppInput::Message { message, is_sync }).unwrap();
            });
        }

        // Single dedicated thread for all CGo downloads. Calling Go from many Rust-spawned
        // threads can crash even with a mutex; one long-lived worker avoids that.
        let (download_tx, download_rx) = mpsc::channel::<(wr::MessageId, wr::FileId)>();
        let media_path = self.media_path.to_owned();
        let app_tx = self.tx.clone();
        thread::spawn(move || {
            for (message_id, file_id) in download_rx {
                let result = wr::download_file(&file_id, &media_path);
                let state = if result.is_err() {
                    FileMeta::DownloadFailed
                } else {
                    FileMeta::Downloaded
                };
                app_tx
                    .send(AppInput::App(AppEvent::SetFileState(message_id, state)))
                    .unwrap();
            }
        });

        info!("Connecting to WhatsApp Web");
        // thread::spawn(|| {
        wr::connect(move |data| {
            qr2term::print_qr(data).unwrap();
            if let Some(phone) = phone.as_ref() {
                let code = wr::pair_phone(phone);
                println!("Pairing code: {}", code);
            }
        });
        // });
        info!("Connected, initializing terminal UI");

        let mut terminal = match ratatui::try_init() {
            Ok(terminal) => terminal,
            Err(e) => {
                error!("Failed to initialize terminal UI: {e}");
                eprintln!("Failed to initialize terminal UI: {e}");
                return;
            }
        };

        {
            let tx = self.tx.clone();
            let input_reader_control = Arc::clone(&self.input_reader_control);
            thread::spawn(move || {
                loop {
                    {
                        let (state_lock, state_changed) = &*input_reader_control;
                        let mut state = state_lock.lock().unwrap();
                        loop {
                            match *state {
                                InputReaderState::Running => break,
                                InputReaderState::Pausing => {
                                    *state = InputReaderState::Paused;
                                    state_changed.notify_all();
                                }
                                InputReaderState::Paused => {
                                    state = state_changed.wait(state).unwrap();
                                }
                                InputReaderState::Stopped => return,
                            }
                        }
                    }

                    match event::poll(Duration::from_millis(50)) {
                        Ok(true) => match event::read() {
                            Ok(event) => {
                                if let Err(e) = tx.send(AppInput::Terminal(event)) {
                                    error!("Failed to send terminal event: {:?}", e);
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("Failed to read terminal event: {e}");
                                thread::sleep(Duration::from_millis(50));
                            }
                        },
                        Ok(false) => {}
                        Err(e) => {
                            error!("Failed to poll terminal events: {e}");
                            thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
            });
        }

        terminal.draw(|frame| ui::draw(frame, self)).unwrap();

        loop {
            let msg = self.rx.recv();
            // info!("Received message: {:?}", &msg);
            let should_draw = match msg {
                Ok(AppInput::App(event)) => match event {
                    AppEvent::EditWithExternalEditor => {
                        self.suspend_input_reader();
                        stdout().execute(LeaveAlternateScreen).unwrap();
                        disable_raw_mode().unwrap();
                        let edit_result = edit::edit(self.input_widget.lines().join("\n"));
                        stdout().execute(EnterAlternateScreen).unwrap();
                        enable_raw_mode().unwrap();
                        terminal.clear().unwrap();
                        self.resume_input_reader();

                        if let Ok(text) = edit_result {
                            self.input_widget.select_all();
                            self.input_widget.delete_next_char();
                            self.input_widget.insert_str(&text);
                        } else {
                            error!("Failed to launch external editor");
                        }
                        true
                    }
                    AppEvent::SetFilePreview(message_id, file_path, img) => {
                        self.image_cache.insert(file_path.clone(), img);
                        self.metadata
                            .insert(message_id.clone(), Metadata::File(FileMeta::Loaded));

                        trace!("Set file preview for message: {:?}", message_id);

                        true
                    }
                    AppEvent::LoadFilePreview(message_id) => {
                        if !matches!(
                            self.metadata.get(&message_id),
                            Some(Metadata::File(FileMeta::Loading))
                        ) {
                            self.metadata
                                .insert(message_id.clone(), Metadata::File(FileMeta::Loading));

                            let tx = self.tx.clone();
                            let media_path = self.media_path.to_owned();
                            let picker = Arc::clone(&self.picker);

                            let file = match &self.messages.get(&message_id).unwrap().message {
                                wr::MessageContent::File(f) => Some(f.clone()),
                                _ => None,
                            };
                            if let Some(file) = file {
                                thread::spawn(move || {
                                    let binding = file.path.to_string();
                                    let path = std::path::Path::new(&binding);
                                    let image_res = image::ImageReader::open(media_path.join(path))
                                        .unwrap()
                                        .decode();

                                    if let Ok(image_src) = image_res {
                                        let mut img =
                                            picker.lock().unwrap().new_resize_protocol(image_src);
                                        img.resize_encode(
                                            &Resize::Scale(None),
                                            Rect {
                                                x: 0,
                                                y: 0,
                                                width: IMAGE_WIDTH as u16,
                                                height: IMAGE_HEIGHT as u16,
                                            },
                                        );

                                        tx.send(AppInput::App(AppEvent::SetFilePreview(
                                            message_id.clone(),
                                            file.path.clone(),
                                            img,
                                        )))
                                        .unwrap();
                                    } else {
                                        tx.send(AppInput::App(AppEvent::SetFileState(
                                            message_id.clone(),
                                            FileMeta::LoadFailed,
                                        )))
                                        .unwrap();
                                    }
                                });
                            } else {
                                error!("Expected a file message for preview");
                            }
                        }
                        false // We will redraw after the preview is loaded
                    }
                    AppEvent::SetFileState(message_id, state) => {
                        self.metadata
                            .insert(message_id.clone(), Metadata::File(state));

                        true
                    }
                    AppEvent::DownloadFile(message_id, file_id) => {
                        if matches!(
                            self.metadata.get(&message_id),
                            Some(Metadata::File(FileMeta::Downloading))
                        ) {
                            false
                        } else {
                            self.metadata
                                .insert(message_id.clone(), Metadata::File(FileMeta::Downloading));
                            download_tx.send((message_id, file_id)).unwrap();
                            false
                        }
                    }
                    AppEvent::DownloadFileDone(message_id, state) => {
                        self.metadata
                            .insert(message_id.clone(), Metadata::File(state));
                        true
                    }
                },
                Ok(AppInput::WhatsApp(event)) => match event {
                    wr::Event::AppStateSyncComplete => {
                        self.get_contacts();
                        self.sort_chats();

                        true
                    }
                    wr::Event::SyncProgress(percent) => {
                        self.history_sync_percent = Some(percent);
                        true
                    }
                    wr::Event::Receipt {
                        kind,
                        chat,
                        message_ids,
                    } => {
                        debug!(
                            "Received receipt: {:?} for chat: {:?} with messages: {:?}",
                            kind, chat, message_ids
                        );
                        for msg_id in message_ids {
                            if let Some(message) = self.messages.get_mut(&msg_id) {
                                message.info.read_by += 1;
                                self.db_handler.add_message(message);
                            }
                        }
                        true
                    }
                },
                Ok(AppInput::Message {
                    message: msg,
                    is_sync,
                }) => {
                    if !is_sync {
                        self.handle_notification(&msg);
                    }

                    self.db_handler.add_message(&msg);
                    self.add_message(msg);

                    let chat_jid = self.get_selected_chat();

                    self.sort_chats();

                    self.select_chat(chat_jid);
                    !is_sync
                }
                Ok(AppInput::Terminal(event)) => {
                    self.on_terminal_event(event);
                    true
                }
                Ok(AppInput::Draw) => true,
                Err(_) => {
                    error!("Failed to receive input from channel");
                    true
                }
            };

            if should_draw {
                terminal.draw(|frame| ui::draw(frame, self)).unwrap();
            }

            if self.should_quit {
                break;
            }
        }

        self.stop_input_reader();
        ratatui::restore();
        wr::disconnect();
    }

    fn load_data_from_db(&mut self) {
        info!("Reading database");
        for chat in self.db_handler.get_chats() {
            self.chats.insert(chat.jid.clone(), chat);
        }
        for (jid, name) in self.db_handler.get_contacts() {
            self.contacts.insert(jid, name);
        }

        for message in self.db_handler.get_messages() {
            self.add_message(message);
        }
        info!(
            "Finished reading database with {} chats and {} messages",
            self.chats.len(),
            self.messages.len()
        );
    }

    /// Display name for a JID (chat or sender). Falls back to the JID string if not in contacts.
    pub fn contact_name(&self, jid: &wr::JID) -> Arc<str> {
        if let Some(name) = self.contacts.get(jid) {
            return name.clone();
        }
        if let Some(phone) = jid.0.strip_suffix("@s.whatsapp.net") {
            return format!("+{phone}").into();
        }
        if let Some(group_id) = jid.0.strip_suffix("@g.us") {
            return format!("Grupo {group_id}").into();
        }
        jid.0.clone()
    }

    pub fn load_favorite_stickers(&self) -> std::collections::HashSet<String> {
        let file = self.media_path.join("favorite_stickers.txt");
        if let Ok(content) = std::fs::read_to_string(&file) {
            content
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            std::collections::HashSet::new()
        }
    }

    pub fn load_stickers(&mut self) {
        let stickers_dir = self.media_path.join("stickers");
        let favs = self.load_favorite_stickers();

        let mut items = Vec::new();
        let mut seen_canonical = std::collections::HashSet::new();
        let mut seen_hashes = std::collections::HashSet::new();

        if let Ok(entries) = std::fs::read_dir(&stickers_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                // Skip symlinks to avoid duplicate entries from backward-compatibility aliases
                if path.is_symlink() {
                    continue;
                }
                if path.extension().and_then(|s| s.to_str()) == Some("webp") {
                    if let Ok(canonical) = std::fs::canonicalize(&path) {
                        if !seen_canonical.insert(canonical) {
                            continue;
                        }
                    }

                    let filename = path.file_name().unwrap().to_string_lossy().to_string();
                    let stem = path.file_stem().unwrap().to_string_lossy().to_string();

                    // Calculate hash or use stem if already sha256 hex
                    let hash = if stem.len() == 64 && stem.chars().all(|c| c.is_ascii_hexdigit()) {
                        stem.to_lowercase()
                    } else if let Ok(bytes) = std::fs::read(&path) {
                        use sha2::{Digest, Sha256};
                        let mut hasher = Sha256::new();
                        hasher.update(&bytes);
                        format!("{:x}", hasher.finalize())
                    } else {
                        filename.clone()
                    };

                    if !seen_hashes.insert(hash.clone()) {
                        continue;
                    }

                    let is_favorite = favs.contains(&filename) || favs.contains(&format!("{}.webp", hash));
                    items.push(StickerItem {
                        path,
                        filename,
                        is_favorite,
                    });
                }
            }
        }

        // Sort: favorites first, then by file modification time (newest first)
        items.sort_by(|a, b| {
            if a.is_favorite != b.is_favorite {
                return b.is_favorite.cmp(&a.is_favorite);
            }
            let mtime_a = a.path.metadata().and_then(|m| m.modified()).ok();
            let mtime_b = b.path.metadata().and_then(|m| m.modified()).ok();
            mtime_b.cmp(&mtime_a)
        });

        self.sticker_list = items;
        if !self.sticker_list.is_empty() && self.sticker_list_state.selected().is_none() {
            self.sticker_list_state.select(Some(0));
        }
    }

    pub fn open_sticker_picker(&mut self) {
        self.load_stickers();
        self.selected_widget = SelectedWidget::StickerPicker;
    }

    pub fn toggle_favorite_sticker(&mut self, rel_path: &str) {
        let filename = std::path::Path::new(rel_path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| rel_path.to_string());

        let file = self.media_path.join("favorite_stickers.txt");
        let mut favs = self.load_favorite_stickers();
        if favs.contains(&filename) {
            favs.remove(&filename);
            log::info!("Sticker removido de favoritos: {}", filename);
        } else {
            favs.insert(filename.clone());
            log::info!("Sticker añadido a favoritos: {}", filename);
        }

        let serialized = favs.into_iter().collect::<Vec<_>>().join("\n");
        let _ = std::fs::write(file, serialized);
        self.load_stickers();
    }

    pub fn send_selected_sticker(&mut self) {
        if let Some(idx) = self.sticker_list_state.selected() {
            if let Some(sticker) = self.sticker_list.get(idx) {
                if let Some(c) = self.get_selected_chat() {
                    let msg = wr::MessageContent::File(wr::FileContent {
                        kind: wr::FileKind::Sticker,
                        path: sticker.path.to_str().unwrap().into(),
                        file_id: "".into(),
                        caption: None,
                    });
                    wr::send_message(&c, &msg, self.quoting_message.as_ref());
                    self.quoting_message = None;
                    log::info!("Sticker enviado: {:?}", sticker.filename);
                }
            }
        }
        self.selected_widget = SelectedWidget::Input;
    }

    fn handle_notification(&self, message: &wr::Message) {
        if message.info.is_from_me {
            return;
        }

        let chat_settings = wr::get_chat_settings(&message.info.chat);
        info!(
            "Chat settings for {:?}: {:?}",
            message.info.chat, chat_settings
        );
        if chat_settings.found {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or_default();
            if chat_settings.muted_until > now {
                return;
            }
        }

        let summary = self.contact_name(&message.info.sender);
        let body = match &message.message {
            wr::MessageContent::Text(text) => text.to_string(),
            wr::MessageContent::File(file) => {
                if let Some(caption) = &file.caption {
                    caption.to_string()
                } else {
                    match file.kind {
                        wr::FileKind::Image => "Sent an image".to_string(),
                        wr::FileKind::Video => "Sent a video".to_string(),
                        wr::FileKind::Audio => "Sent an audio message".to_string(),
                        wr::FileKind::Document => "Sent a document".to_string(),
                        wr::FileKind::Sticker => "Sent a sticker".to_string(),
                    }
                }
            }
        };

        if let Err(err) = Notification::new().summary(&summary).body(&body).show() {
            error!("Failed to show desktop notification: {err}");
        }
    }

    pub fn mark_chat_as_read(&mut self, chat_jid: &wr::JID) {
        if let Some(msgs) = self.chat_messages.get(chat_jid) {
            let msgs_per_sender: HashMap<wr::JID, Vec<wr::MessageId>> = msgs
                .iter()
                .filter_map(|msg_id| self.messages.get(msg_id))
                .filter(|msg| !msg.info.is_from_me && msg.info.read_by == 0)
                .fold(HashMap::new(), |mut acc, msg| {
                    acc.entry(msg.info.sender.clone())
                        .or_insert_with(Vec::new)
                        .push(msg.info.id.clone());
                    acc
                });

            for (sender, msg_ids) in msgs_per_sender {
                wr::mark_as_read(&msg_ids, chat_jid, &sender);
            }
        }
    }

    fn suspend_input_reader(&self) {
        let (state_lock, state_changed) = &*self.input_reader_control;
        let mut state = state_lock.lock().unwrap();

        match *state {
            InputReaderState::Stopped | InputReaderState::Paused => return,
            InputReaderState::Running => {
                *state = InputReaderState::Pausing;
                state_changed.notify_all();
            }
            InputReaderState::Pausing => {}
        }

        while *state != InputReaderState::Paused && *state != InputReaderState::Stopped {
            state = state_changed.wait(state).unwrap();
        }
    }

    fn resume_input_reader(&self) {
        let (state_lock, state_changed) = &*self.input_reader_control;
        let mut state = state_lock.lock().unwrap();

        if *state != InputReaderState::Stopped {
            *state = InputReaderState::Running;
            state_changed.notify_all();
        }
    }

    fn stop_input_reader(&self) {
        let (state_lock, state_changed) = &*self.input_reader_control;
        let mut state = state_lock.lock().unwrap();
        *state = InputReaderState::Stopped;
        state_changed.notify_all();
    }

    pub fn get_selected_chat(&self) -> Option<wr::JID> {
        self.chat_list_state.selected().map(|index| {
            if self.contact_search.input.is_empty() {
                self.sorted_chats[index].clone()
            } else {
                self.filtered_chats[index].clone()
            }
        })
    }

    pub fn select_chat(&mut self, jid: Option<wr::JID>) {
        let target_list = if self.contact_search.input.is_empty() {
            &self.sorted_chats
        } else {
            &self.filtered_chats
        };

        if let Some(jid) = jid
            && let Some(index) = target_list.iter().position(|chat_jid| chat_jid == &jid)
        {
            self.chat_list_state.select(Some(index));
        } else if self.sorted_chats.len() > 0 {
            self.chat_list_state.select(Some(0));
        } else {
            self.chat_list_state.select(None);
        }
    }

    fn update_filtered_chats(&mut self) {
        let query = self.contact_search.input.to_lowercase();
        self.filtered_chats = self
            .sorted_chats
            .iter()
            .filter(|chat| {
                let name = self.contact_name(&chat).to_lowercase();
                name.contains(&query)
            })
            .map(|chat| chat.clone())
            .collect();

        if self.filtered_chats.len() > 0 {
            self.chat_list_state.select(Some(0));
        } else {
            self.chat_list_state.select(None);
        }
    }

    fn add_message(&mut self, message: wr::Message) {
        let chat_jid = message.info.chat.clone();
        self.add_or_update_chat(
            Chat {
                jid: chat_jid.clone(),
                last_message_time: Some(message.info.timestamp),
            },
            |chat| {
                if Some(message.info.timestamp) > chat.last_message_time {
                    chat.last_message_time = Some(message.info.timestamp);
                }
            },
        );

        let id = message.info.id.clone();

        // Insert the message into the messages map, if it's aleardy present,
        // updated if the message is newer.
        if let Some(existing_message) = self.messages.get_mut(&id) {
            if existing_message.info.timestamp < message.info.timestamp {
                *existing_message = message;
                self.sort_chat_messages(chat_jid.clone());
            }
        } else {
            self.messages.insert(id.clone(), message);
            self.chat_messages
                .entry(chat_jid.clone())
                .or_default()
                .push(id);
        }
    }

    fn add_or_update_chat<F: FnOnce(&mut Chat)>(&mut self, chat: Chat, callback: F) {
        if let Some(existing_chat) = self.chats.get_mut(&chat.jid) {
            callback(existing_chat);
            self.db_handler.add_chat(existing_chat);
        } else {
            self.db_handler.add_chat(&chat);
            self.chats.insert(chat.jid.clone(), chat);
        }
    }

    fn get_contacts(&mut self) {
        for (jid, name) in wr::get_contacts() {
            self.contacts.insert(jid.clone(), name.clone());
            self.db_handler.add_contact(&jid, name.as_ref());
        }
    }

    fn sort_chats(&mut self) {
        let mut entries: Vec<_> = self.chats.values().cloned().collect();
        entries.sort_by(|a, b| {
            let a_time = a.last_message_time.unwrap_or_default();
            let b_time = b.last_message_time.unwrap_or_default();
            b_time.cmp(&a_time)
        });

        self.sorted_chats = entries.iter().map(|chat| chat.jid.clone()).collect();
    }

    fn sort_chat_messages(&mut self, chat_jid: wr::JID) {
        if let Some(messages) = self.chat_messages.get_mut(&chat_jid) {
            messages.sort_by_cached_key(|msg_id| {
                self.messages
                    .get(msg_id)
                    .map(|m| m.info.timestamp)
                    .unwrap_or(i64::MIN)
            });
        }
    }
}
