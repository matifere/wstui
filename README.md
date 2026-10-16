# wstui (Enhanced Fork)

A modern, keyboard-driven **WhatsApp client for the terminal**, built in Rust with [ratatui](https://github.com/ratatui-org/ratatui) and the [whatsmeow](https://github.com/tulir/whatsmeow) Go backend.

This fork introduces visual improvements, Neovim ergonomics, native sticker sending, a WhatsApp Web-style sticker gallery with live terminal previews, and smart content-addressed storage deduplication.

---

## ✨ Features Added in this Fork

### 🎨 Visual & UI Enhancements
- **Rounded Modern Borders:** Polished `BorderType::Rounded` applied across all panes, popups, and dialogs.
- **Group & Contact Icons:** Clear visual distinction with `👥` for groups and `👤` for direct contacts.
- **Readable Contact Names:** Automatic cleanup of raw JIDs into readable formatted names and phone numbers.
- **Active Chat Header:** Prominent active conversation banner styled in bold yellow with underline to prevent sending messages to the wrong chat.
- **Neovim Mode Badges:** Dynamic, high-visibility status indicators for `[ NORMAL ]` (Cyan) and `[ INSERTAR ]` (Green/Bold).
- **Refined Selection Highlighting:** Clear contrast for selected chats and messages.

### 🎭 WhatsApp Web-Style Sticker Gallery (`Ctrl+S` / `Space s`)
- **Sticker Picker Modal:** Open your stickers gallery instantly with `Ctrl+S` (or `Space s` in Normal mode).
- **Live Terminal Preview:** Real-time graphics rendering of the selected sticker using `ratatui-image` and `chafa` before sending.
- **Favorites System:** Toggle any sticker as a favorite with `s`. Favorites are pinned at the top (`⭐ Favorito #1`) and persisted across restarts.
- **Instant Send:** Press `Enter` on any sticker to send it immediately to the active chat.

### 🚀 Native Sticker Sending
- Added `FileTypeSticker` support to the Go `whatsrust` bridge, uploading webp media via `whatsmeow.MediaImage` and encoding into `waE2E.StickerMessage`.

### ♻️ Smart Deduplication (Content Addressing via SHA-256)
- **Zero Disk Waste:** Stickers are named and indexed by their SHA-256 content hash (`stickers/<sha256>.webp`) instead of unique message IDs.
- **Bypass Redundant Downloads:** If a sticker was already received or sent anywhere, wstui detects the local file and avoids re-downloading it.
- **Clean Sticker List:** The sticker picker automatically filters out duplicates so your gallery stays clean and organized.

---

## 📋 Requirements

- **Rust** (2024 edition; recent stable toolchain)
- **Go** (tested with 1.22+)
- **Chafa** (`libchafa-dev` or `chafa` on Arch Linux) for terminal image rendering

---

## 🛠️ Installation

### Installing from this repository

```bash
git clone https://github.com/matifere/wstui.git
cd wstui
PKG_CONFIG_PATH=$HOME/.local/lib/pkgconfig cargo install --path .
```

Or build a release binary directly:

```bash
PKG_CONFIG_PATH=$HOME/.local/lib/pkgconfig RUSTFLAGS="-C link-arg=-Wl,-rpath,$HOME/.local/lib" cargo build --release
cp target/release/wstui ~/.local/bin/wstui
```

---

## 📱 Usage

```bash
# Start and link with QR code (displayed in terminal)
wstui

# Link with phone number (pairing code will be printed)
wstui -- -p +1234567890
```

On first run, the client creates `whatsmeow_store.db` (session) and `media/` for downloaded files. The local message cache is in `whatsapp.db`.

---

## ⌨️ Keybindings

### General

| Action | Shortcut |
|---|---|
| Quit | `Ctrl+Q` |
| Toggle logs | `Ctrl+Shift+L` |
| Cycle image protocol | `Ctrl+P` |
| **Open Sticker Gallery** | **`Ctrl+S`** or **`Space s`** |

### 🎭 Sticker Picker Modal

| Action | Shortcut |
|---|---|
| Navigate up / down | `j` / `k` or `↑` / `↓` |
| **Send selected sticker** | **`Enter`** |
| **Toggle Favorite ⭐** | **`s`** |
| Close / Cancel | `Esc` |

### Focus / Navigation

| Action | Shortcut |
|---|---|
| Chat list → Message list | `Ctrl+L` |
| Message list → Chat list | `Ctrl+H` |
| Message list → Input | `Ctrl+J` |
| Input → Message list | `Ctrl+K` |
| Input → Chat list | `Ctrl+H` |
| Message view → Message list | `Esc` |

### Chat List

| Action | Shortcut |
|---|---|
| Next / previous chat | `j` / `k` |
| Open chat | `Enter` or `l` |
| Search contacts | `/` |
| Move cursor left/right | `←` / `→` |

### Message List

| Action | Shortcut |
|---|---|
| Next / previous message | `j` / `k` |
| First message | `g g` |
| Last message | `G` |
| Scroll up | `Ctrl+E` |
| Scroll down | `Ctrl+Y` |
| Open (external) | `o` |
| Reply to message | `r` |
| Copy to clipboard | `y` |
| View full content | `Enter` |
| Go to quoted message | `g q` |
| Reset selection | `Esc` |

### Input (Normal & Vim Mode)

| Action | Shortcut |
|---|---|
| Send message | `Ctrl+X` |
| Edit with external editor | `Ctrl+E` |
| Open Sticker Gallery | `Space s` |
| Clear quote | `Space r` |
| Attach image | `Space a i` |
| Attach document | `Space a d` |
| Clear attachment | `Space a r` |
| Paste from clipboard | `Space p` |
| Enter insert mode | `i` |
| Enter normal mode | `Esc` |
| Append / Append at EOL | `a` / `A` |
| New line below / above | `o` / `O` |
| Move cursor | `h` / `j` / `k` / `l` |
| Word movement | `w` / `b` / `e` |
| Line start / end | `^` / `$` |
| Yank / Delete / Change line | `yy` / `dd` / `cc` |
| Paste | `p` |
| Undo / Redo | `u` / `Ctrl+R` |
| Visual modes | `v` / `V` |
| Half-page scroll | `Ctrl+D` / `Ctrl+U` |

---

## 📄 License

GPL-3.0 (same as upstream [NullSeile/wstui](https://github.com/NullSeile/wstui)).
