pub mod message_list;
pub mod text_input;

use std::sync::Arc;

use crate::app::{App, SelectedWidget};
use log::trace;
use message_list::{get_quoted_text, render_messages};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, Paragraph},
};
use ratatui_image::{Resize, ResizeEncodeRender, StatefulImage};
use tui_logger::TuiLoggerWidget;
use whatsrust as wr;

pub fn draw(frame: &mut Frame, app: &mut App) {
    if let SelectedWidget::MessageView = app.selected_widget {
        let msg_id = app.message_list_state.get_selected_message().unwrap();

        let block = Block::default()
            .title(" ✉ Mensaje Completo ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan));

        let area = block.inner(frame.area());
        frame.render_widget(block, frame.area());

        if let Some(msg) = app.messages.get(&msg_id) {
            match msg.message {
                wr::MessageContent::Text(ref text) => {
                    let paragraph = Paragraph::new(text.to_string());
                    frame.render_widget(paragraph, area);
                }
                wr::MessageContent::File(ref file) => match file.kind {
                    wr::FileKind::Image | wr::FileKind::Sticker => {
                        if let Some(image) = app.image_cache.get_mut(&file.path) {
                            trace!("Rendering image from cache: {}", file.path);
                            frame.render_stateful_widget(
                                StatefulImage::default().resize(Resize::Scale(None)),
                                area,
                                image,
                            );
                        }
                    }
                    wr::FileKind::Video => {
                        let paragraph = Paragraph::new("Video not supported yet");
                        frame.render_widget(paragraph, area);
                    }
                    wr::FileKind::Audio => {
                        let paragraph = Paragraph::new("Audio not supported yet");
                        frame.render_widget(paragraph, area);
                    }
                    wr::FileKind::Document => {
                        let paragraph = Paragraph::new("Document not supported yet");
                        frame.render_widget(paragraph, area);
                    }
                },
            }
        }

        return;
    }

    if app.show_logs {
        let [contacts_area, chat_area, logs_area] = Layout::horizontal([
            Constraint::Length(34),
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .areas(frame.area());

        render_logs(frame, logs_area);
        render_contacts(frame, app, contacts_area);
        render_chats(frame, app, chat_area);
    } else {
        let [contacts_area, chat_area] =
            Layout::horizontal([Constraint::Length(34), Constraint::Min(40)])
                .areas(frame.area());

        render_contacts(frame, app, contacts_area);
        render_chats(frame, app, chat_area);
    }

    if let SelectedWidget::StickerPicker = app.selected_widget {
        render_sticker_picker(frame, app, frame.area());
    }

    if let Some(call) = app.incoming_call.clone() {
        render_incoming_call(frame, app, &call, frame.area());
    } else if let Some(call) = &app.active_call {
        render_active_call_indicator(frame, app, call, frame.area());
    }
}

fn render_logs(frame: &mut Frame, area: Rect) {
    let log_widget = TuiLoggerWidget::default()
        .style_trace(Style::new().dark_gray())
        .style_debug(Style::new().blue())
        .style_warn(Style::new().yellow())
        .style_error(Style::new().red().bold())
        .block(
            Block::default()
                .title(" Logs ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
    frame.render_widget(log_widget, area);
}

const BADGE_COLORS: &[Color] = &[
    Color::Rgb(52, 152, 219),  // River Blue
    Color::Rgb(46, 204, 113),  // Emerald Green
    Color::Rgb(155, 89, 182),  // Amethyst Purple
    Color::Rgb(243, 156, 18),  // Amber / Warm Yellow
    Color::Rgb(230, 126, 34),  // Carrot Orange
    Color::Rgb(231, 76, 60),   // Coral Red
    Color::Rgb(26, 188, 156),  // Turquoise Teal
    Color::Rgb(253, 121, 168), // Rose Pink
    Color::Rgb(108, 92, 231),  // Indigo Periwinkle
    Color::Rgb(0, 206, 201),   // Vivid Cyan
    Color::Rgb(225, 112, 85),  // Terracotta
];

pub fn get_contact_badge(chat: &wr::JID, raw_name: &str) -> (String, Color) {
    let is_group = chat.0.ends_with("@g.us");

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    chat.0.hash(&mut hasher);
    let color_idx = (hasher.finish() as usize) % BADGE_COLORS.len();
    let color = BADGE_COLORS[color_idx];

    let clean = raw_name.trim().trim_start_matches('~').trim();
    let clean = clean.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '+').trim();

    let initials = if clean.is_empty() {
        if is_group {
            "👥".to_string()
        } else {
            "👤".to_string()
        }
    } else if clean.starts_with('+') {
        let digits: String = clean.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 2 {
            format!("#{}", &digits[digits.len() - 2..])
        } else {
            "+#".to_string()
        }
    } else {
        let words: Vec<&str> = clean.split_whitespace().collect();
        if words.len() >= 2 {
            let first = words[0].chars().find(|c| c.is_alphanumeric()).unwrap_or(' ');
            let second = words[1].chars().find(|c| c.is_alphanumeric()).unwrap_or(' ');
            format!("{}{}", first.to_uppercase(), second.to_uppercase())
        } else {
            let mut chars = clean.chars().filter(|c| c.is_alphanumeric());
            let first = chars.next().unwrap_or('?').to_uppercase().to_string();
            let second = chars
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| " ".to_string());
            format!("{}{}", first, second)
        }
    };

    (initials, color)
}

fn render_contacts(frame: &mut Frame, app: &mut App, area: Rect) {
    let chats = if app.contact_search.input.is_empty() {
        &app.sorted_chats
    } else {
        &app.filtered_chats
    };
    let items = chats
        .iter()
        .map(|chat| {
            let name = app.contact_name(chat);
            let (badge, badge_color) = get_contact_badge(chat, &name);
            let settings = app.chat_settings.get(chat);
            let is_muted = settings.map(|s| s.is_muted()).unwrap_or(false);
            let is_archived = settings.map(|s| s.is_archived()).unwrap_or(false);
            let is_pinned = settings.map(|s| s.pinned).unwrap_or(false);

            let mut spans = vec![
                Span::styled(
                    format!(" {badge} "),
                    Style::default()
                        .fg(Color::Rgb(20, 20, 25))
                        .bg(badge_color)
                        .bold(),
                ),
                Span::raw(" "),
            ];

            let name_style = if is_archived {
                Style::default().fg(Color::DarkGray)
            } else if is_muted {
                Style::default().fg(Color::Gray)
            } else {
                Style::default()
            };
            spans.push(Span::styled(name.to_string(), name_style));

            if is_pinned {
                spans.push(Span::raw(" "));
                spans.push(Span::styled("📌", Style::default().fg(Color::Cyan)));
            }
            if is_muted {
                spans.push(Span::raw(" "));
                spans.push(Span::styled("🔇", Style::default().fg(Color::DarkGray)));
            }
            if is_archived {
                spans.push(Span::raw(" "));
                spans.push(Span::styled("📦", Style::default().fg(Color::DarkGray)));
            }

            Line::from(spans)
        })
        .collect::<Vec<_>>();

    let mut list_area = area;
    if !app.contact_search.input.is_empty() || app.contact_search_active {
        let [search_area, new_list_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Percentage(100)]).areas(area);
        list_area = new_list_area;

        let text = format!(" 🔍 /{}", app.contact_search.input);
        frame.render_widget(Paragraph::new(text).cyan(), search_area);

        if app.contact_search_active {
            frame.set_cursor_position(Position::new(
                search_area.x + app.contact_search.character_index as u16 + 5,
                search_area.y,
            ));
        }
    }

    let is_focused = matches!(app.selected_widget, SelectedWidget::ChatList);
    let border_color = if is_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let title_text = if let Some(p) = app.history_sync_percent {
        format!(" 📇 Chats ({p}%) ")
    } else {
        " 📇 Chats ".to_string()
    };

    let list = List::new(items)
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(Line::from(vec![
                    Span::styled(
                        title_text,
                        if is_focused {
                            Style::default().fg(Color::Cyan).bold()
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ),
                ]))
                .border_style(Style::default().fg(border_color)),
        )
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .fg(Color::Rgb(255, 230, 100))
                .bg(Color::Rgb(30, 48, 80))
                .bold(),
        );

    frame.render_stateful_widget(list, list_area, &mut app.chat_list_state);
}

pub fn render_chats(frame: &mut Frame, app: &mut App, area: Rect) {
    let [chat_area, mut input_area] =
        Layout::vertical([Constraint::Percentage(100), Constraint::Min(10)]).areas(area);

    render_messages(frame, app, chat_area);

    if let Some(_chat_jid) = app.get_selected_chat() {
        let is_input_focused = matches!(app.selected_widget, SelectedWidget::Input);
        let border_color = if is_input_focused {
            Color::Green
        } else {
            Color::DarkGray
        };
        let input_block = app.input_border.clone().border_style(Style::default().fg(border_color));
        frame.render_widget(&input_block, input_area);

        input_area = input_block.inner(input_area);

        if let Some(msg) = &app.quoting_message {
            let [quote_area, input_areaa] =
                Layout::vertical([Constraint::Length(1), Constraint::Percentage(100)])
                    .areas(input_area);

            input_area = input_areaa;

            frame.render_widget(
                Paragraph::new(format!("> {}", get_quoted_text(msg))).dark_gray(),
                quote_area,
            );
        }

        if let Some((path, typ)) = &app.attached_file {
            let file_type_str = match typ {
                wr::FileKind::Image => "Image",
                wr::FileKind::Video => "Video",
                wr::FileKind::Audio => "Audio",
                wr::FileKind::Document => "Document",
                wr::FileKind::Sticker => "Sticker",
            };
            let [attach_area, input_areaa] =
                Layout::vertical([Constraint::Length(1), Constraint::Percentage(100)])
                    .areas(input_area);

            input_area = input_areaa;

            frame.render_widget(
                Paragraph::new(format!("🔗 {}: {}", file_type_str, path)).dark_gray(),
                attach_area,
            );
        }

        frame.render_widget(&app.input_widget, input_area);
    }
}

fn render_sticker_picker(frame: &mut Frame, app: &mut App, area: Rect) {
    let popup_width = 82.min(area.width.saturating_sub(4));
    let popup_height = 24.min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let fav_count = app.sticker_list.iter().filter(|s| s.is_favorite).count();
    let title_line = Line::from(vec![
        Span::styled(" 🎭 Galería de Stickers ", Style::default().fg(Color::Yellow).bold()),
        Span::styled(format!("({} total, {} ⭐) ", app.sticker_list.len(), fav_count), Style::default().fg(Color::LightCyan)),
    ]);

    let bottom_help = Line::from(vec![
        Span::styled(" [j/k: navegar | Enter: ENVIAR | s: Favorito ⭐ | Esc: cancelar] ", Style::default().fg(Color::DarkGray)),
    ]);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title_line)
        .title_bottom(bottom_help);

    let inner_area = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if app.sticker_list.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("  No se encontraron stickers aún en tu historial.", Style::default().fg(Color::DarkGray))),
            Line::from(""),
            Line::from(Span::styled("  Los stickers recibidos en tus chats se guardarán aquí automáticamente.", Style::default().fg(Color::White))),
            Line::from(""),
            Line::from(Span::styled("  Presiona Esc para volver al chat.", Style::default().fg(Color::Yellow))),
        ]);
        frame.render_widget(msg, inner_area);
        return;
    }

    let [list_area, preview_area] = Layout::horizontal([
        Constraint::Length(32),
        Constraint::Min(35),
    ])
    .areas(inner_area);

    let items = app
        .sticker_list
        .iter()
        .enumerate()
        .map(|(i, sticker)| {
            let star = if sticker.is_favorite { "⭐ " } else { "   " };
            let label = if sticker.is_favorite {
                format!("Favorito #{}", i + 1)
            } else {
                format!("Sticker #{:02}", i + 1)
            };
            Line::from(vec![
                Span::styled(star, Style::default().fg(Color::Yellow).bold()),
                Span::styled(format!("{:2}. ", i + 1), Style::default().fg(Color::DarkGray)),
                Span::styled(label, Style::default().fg(if sticker.is_favorite { Color::Yellow } else { Color::White })),
            ])
        })
        .collect::<Vec<_>>();

    let list = List::new(items)
        .block(Block::default().borders(Borders::RIGHT).border_style(Style::default().fg(Color::DarkGray)))
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .fg(Color::Rgb(255, 230, 100))
                .bg(Color::Rgb(30, 48, 80))
                .bold(),
        );

    frame.render_stateful_widget(list, list_area, &mut app.sticker_list_state);

    if let Some(selected_idx) = app.sticker_list_state.selected() {
        if let Some(sticker) = app.sticker_list.get(selected_idx) {
            let [image_area, info_area] = Layout::vertical([
                Constraint::Min(12),
                Constraint::Length(6),
            ])
            .areas(preview_area);

            // Load live sticker preview image
            let path_key: Arc<str> = sticker.path.to_string_lossy().into();
            if !app.image_cache.contains_key(&path_key) {
                if let Ok(reader) = image::ImageReader::open(&sticker.path) {
                    if let Ok(reader) = reader.with_guessed_format() {
                        if let Ok(decoded) = reader.decode() {
                            let mut proto = app.picker.lock().unwrap().new_resize_protocol(decoded);
                            proto.resize_encode(
                                &Resize::Scale(None),
                                Rect {
                                    x: 0,
                                    y: 0,
                                    width: image_area.width,
                                    height: image_area.height,
                                },
                            );
                            app.image_cache.insert(path_key.clone(), proto);
                        }
                    }
                }
            }

            if let Some(proto) = app.image_cache.get_mut(&path_key) {
                frame.render_stateful_widget(
                    StatefulImage::default().resize(Resize::Scale(None)),
                    image_area,
                    proto,
                );
            } else {
                frame.render_widget(
                    Paragraph::new("🖼 [Cargando vista previa...]").dark_gray(),
                    image_area,
                );
            }

            let chat_name = app.get_selected_chat()
                .map(|jid| app.contact_name(&jid).to_string())
                .unwrap_or_else(|| "Ninguno".to_string());

            let status_span = if sticker.is_favorite {
                Span::styled(" ⭐ FAVORITO ", Style::default().fg(Color::Black).bg(Color::Yellow).bold())
            } else {
                Span::styled(" RECIENTE ", Style::default().fg(Color::White).bg(Color::DarkGray))
            };

            let info_lines = vec![
                Line::from(vec![
                    Span::styled(" Destino: ", Style::default().fg(Color::Cyan).bold()),
                    Span::styled(chat_name, Style::default().fg(Color::LightGreen).bold()),
                    Span::raw(" | "),
                    status_span,
                ]),
                Line::from(Span::styled(" ──────────────────────────────────", Style::default().fg(Color::DarkGray))),
                Line::from(vec![
                    Span::styled(" Enter ", Style::default().fg(Color::Black).bg(Color::Green).bold()),
                    Span::styled(" Enviar  ", Style::default().fg(Color::White)),
                    Span::styled(" s ", Style::default().fg(Color::Black).bg(Color::Yellow).bold()),
                    Span::styled(" Fav ⭐  ", Style::default().fg(Color::White)),
                    Span::styled(" Esc ", Style::default().fg(Color::Black).bg(Color::DarkGray).bold()),
                    Span::styled(" Salir", Style::default().fg(Color::White)),
                ]),
            ];
            frame.render_widget(Paragraph::new(info_lines), info_area);
        }
    }
}

fn render_incoming_call(frame: &mut Frame, app: &App, call: &crate::app::IncomingCall, area: Rect) {
    let popup_width = 72.min(area.width.saturating_sub(4));
    let popup_height = 12.min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let caller_name = app.contact_name(&call.caller);
    let type_str = if call.is_video {
        "📹 Videollamada de WhatsApp"
    } else {
        "📞 Llamada de voz de WhatsApp"
    };

    let title_line = Line::from(vec![
        Span::styled(" 📞 LLAMADA ENTRANTE ", Style::default().fg(Color::Rgb(255, 80, 80)).bold()),
    ]);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow).bold())
        .title(title_line);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let content = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(" De: ", Style::default().fg(Color::DarkGray)),
            Span::styled(caller_name.to_string(), Style::default().fg(Color::White).bold()),
        ]),
        Line::from(vec![
            Span::styled(" Tipo: ", Style::default().fg(Color::DarkGray)),
            Span::styled(type_str, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled(" Mic:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(app.current_microphone_name(), Style::default().fg(Color::LightYellow).bold()),
            Span::styled(" [m para cambiar]", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" [a / Enter] ", Style::default().fg(Color::Green).bold()),
            Span::styled("Atender nativo  ", Style::default().fg(Color::White)),
            Span::styled("[b] ", Style::default().fg(Color::Cyan).bold()),
            Span::styled("Navegador  ", Style::default().fg(Color::White)),
            Span::styled("[m] ", Style::default().fg(Color::Yellow).bold()),
            Span::styled("Mic", Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled(" [r / Esc]   ", Style::default().fg(Color::Red).bold()),
            Span::styled("Rechazar llamada", Style::default().fg(Color::White)),
        ]),
    ]);

    frame.render_widget(content, inner);
}

fn render_active_call_indicator(frame: &mut Frame, app: &App, call: &crate::app::IncomingCall, area: Rect) {
    let caller_name = app.contact_name(&call.caller);
    let type_icon = if call.is_video { "📹" } else { "📞" };
    let rot_hint = if call.is_video { " [Ctrl+R: Rotar]" } else { "" };
    let mic_name = app.current_microphone_short();
    let mic_hint = format!(" [Ctrl+M: {}]", mic_name);
    let text = format!(" {} En llamada: {}{}{}[Ctrl+H: Cortar] ", type_icon, caller_name, rot_hint, mic_hint);
    let width = (text.chars().count() as u16 + 2).min(area.width.saturating_sub(4));
    let pill_area = Rect::new(area.width.saturating_sub(width + 2), 0, width, 3);

    frame.render_widget(Clear, pill_area);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green).bold());

    let inner = block.inner(pill_area);
    frame.render_widget(block, pill_area);

    let mut spans = vec![
        Span::styled(format!(" {} ", type_icon), Style::default().fg(Color::Green).bold()),
        Span::styled(caller_name.to_string(), Style::default().fg(Color::White).bold()),
    ];
    if call.is_video {
        spans.push(Span::styled(" [Ctrl+R: Rotar]", Style::default().fg(Color::Yellow).bold()));
    }
    spans.push(Span::styled(mic_hint, Style::default().fg(Color::Cyan).bold()));
    spans.push(Span::styled(" [Ctrl+H: Cortar]", Style::default().fg(Color::LightRed).bold()));

    let content = Paragraph::new(Line::from(spans));

    frame.render_widget(content, inner);
}
