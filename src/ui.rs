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

fn render_contacts(frame: &mut Frame, app: &mut App, area: Rect) {
    let chats = if app.contact_search.input.is_empty() {
        &app.sorted_chats
    } else {
        &app.filtered_chats
    };
    let items = chats
        .iter()
        .map(|chat| {
            let is_group = chat.0.ends_with("@g.us");
            let icon = if is_group { "👥 " } else { "👤 " };
            let icon_color = if is_group { Color::LightCyan } else { Color::Green };
            let name = app.contact_name(chat);
            Line::from(vec![
                Span::styled(icon, Style::default().fg(icon_color)),
                Span::raw(name.to_string()),
            ])
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
