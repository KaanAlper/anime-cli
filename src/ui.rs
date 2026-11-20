use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph},
};
use std::io;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

const PINK: Color = Color::Rgb(0xe4, 0x5c, 0xc0);
const NORMAL: Color = Color::Rgb(0xaa, 0xbb, 0xcc);
const GREEN: Color = Color::Rgb(0x8b, 0xb2, 0x7f);
const GOLD: Color = Color::Rgb(0xc4, 0xb4, 0x8b);
const MUTED: Color = Color::Rgb(0x66, 0x66, 0x66);
const ERROR_RED: Color = Color::Rgb(0xff, 0x5f, 0x5f);

static TERMINAL: Mutex<Option<Terminal<CrosstermBackend<io::Stdout>>>> = Mutex::new(None);

pub struct UiSession;

impl UiSession {
    pub fn start() -> Result<Self> {
        resume()?;
        Ok(Self)
    }
}

impl Drop for UiSession {
    fn drop(&mut self) {
        suspend();
    }
}

pub fn suspend() {
    if let Ok(mut guard) = TERMINAL.lock()
        && guard.take().is_some()
    {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

pub fn resume() -> Result<()> {
    let mut guard = TERMINAL.lock().map_err(|_| anyhow::anyhow!("UI lock poisoned"))?;
    if guard.is_none() {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut term = Terminal::new(backend)?;
        term.clear()?;
        *guard = Some(term);
    }
    Ok(())
}

fn with_terminal<T, F>(f: F) -> Result<T>
where
    F: FnOnce(&mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<T>,
{
    resume()?;
    let mut guard = TERMINAL.lock().map_err(|_| anyhow::anyhow!("UI lock poisoned"))?;
    let term = guard.as_mut().expect("terminal initialized");
    f(term)
}

pub fn with_spinner<T, F>(label: &str, f: F) -> T
where
    F: FnOnce() -> T,
{
    let done = Arc::new(AtomicBool::new(false));
    let done_clone = Arc::clone(&done);
    let msg = label.to_string();

    let handle = thread::spawn(move || {
        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let mut idx = 0;
        while !done_clone.load(Ordering::Relaxed) {
            let frame = frames[idx];
            let _ = with_terminal(|term| {
                term.draw(|f| {
                    let area = f.area();
                    let p = Paragraph::new(Line::from(vec![
                        Span::styled(
                            format!(" {frame} "),
                            Style::default().fg(PINK).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(&msg, Style::default().fg(NORMAL)),
                    ]));
                    f.render_widget(p, area);
                })?;
                Ok(())
            });
            idx = (idx + 1) % frames.len();
            thread::sleep(Duration::from_millis(80));
        }
    });

    let result = f();
    done.store(true, Ordering::Relaxed);
    let _ = handle.join();
    result
}

pub fn show_error(msg: &str) {
    let _ = with_terminal(|term| {
        term.draw(|f| {
            let area = f.area();
            let p = Paragraph::new(vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        " [!] ",
                        Style::default().fg(ERROR_RED).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(msg, Style::default().fg(ERROR_RED)),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    " Devam etmek için bir tuşa basın...",
                    Style::default().fg(MUTED),
                )),
            ]);
            f.render_widget(p, area);
        })?;
        loop {
            if let Event::Key(k) = event::read()?
                && k.kind == KeyEventKind::Press
            {
                break;
            }
        }
        Ok(())
    });
}

pub fn select(title: &str, subtitle: &str, items: &[String], initial: usize) -> Result<Option<usize>> {
    if items.is_empty() {
        return Ok(None);
    }

    with_terminal(|terminal| {
        let mut filtering = false;
        let mut filter_query = String::new();
        let mut filtered_indices: Vec<usize> = (0..items.len()).collect();
        let mut state = ListState::default();
        state.select(Some(initial.min(items.len().saturating_sub(1))));

        loop {
            terminal.draw(|f| {
                let area = f.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .horizontal_margin(2)
                    .vertical_margin(1)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(3),
                        Constraint::Length(1),
                    ])
                    .split(area);

                let header_line = if filtering || !filter_query.is_empty() {
                    Line::from(vec![
                        Span::styled(
                            "Filtre: ",
                            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(&filter_query, Style::default().fg(NORMAL)),
                        Span::styled("▎", Style::default().fg(GOLD)),
                    ])
                } else {
                    Line::from(Span::styled(
                        subtitle,
                        Style::default().fg(MUTED).add_modifier(Modifier::ITALIC),
                    ))
                };

                let header = Paragraph::new(vec![
                    Line::from(Span::styled(
                        title,
                        Style::default().fg(PINK).add_modifier(Modifier::BOLD),
                    )),
                    header_line,
                ]);
                f.render_widget(header, chunks[0]);

                let list_items: Vec<ListItem> = filtered_indices
                    .iter()
                    .map(|&orig_idx| {
                        let text = &items[orig_idx];
                        if text.is_empty() {
                            ListItem::new(Line::from(""))
                        } else {
                            ListItem::new(Line::from(Span::styled(
                                text.as_str(),
                                Style::default().fg(NORMAL),
                            )))
                        }
                    })
                    .collect();

                let list = List::new(list_items)
                    .highlight_style(Style::default().fg(PINK).add_modifier(Modifier::BOLD))
                    .highlight_symbol("» ");

                f.render_stateful_widget(list, chunks[1], &mut state);

                let help_line = if filtering {
                    Line::from(vec![
                        Span::styled("yaz", Style::default().fg(GREEN)),
                        Span::styled(" filtrele • ", Style::default().fg(MUTED)),
                        Span::styled("enter", Style::default().fg(GOLD)),
                        Span::styled(" onayla • ", Style::default().fg(MUTED)),
                        Span::styled("esc", Style::default().fg(GOLD)),
                        Span::styled(" temizle", Style::default().fg(MUTED)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("↑/↓", Style::default().fg(GOLD)),
                        Span::styled(" gezin • ", Style::default().fg(MUTED)),
                        Span::styled("/", Style::default().fg(GREEN)),
                        Span::styled(" filtrele • ", Style::default().fg(MUTED)),
                        Span::styled("enter", Style::default().fg(GOLD)),
                        Span::styled(" seç • ", Style::default().fg(MUTED)),
                        Span::styled("esc/q", Style::default().fg(GOLD)),
                        Span::styled(" geri", Style::default().fg(MUTED)),
                    ])
                };
                f.render_widget(Paragraph::new(help_line), chunks[2]);
            })?;

            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
                {
                    return Ok(None);
                }

                if filtering {
                    match key.code {
                        KeyCode::Esc => {
                            filtering = false;
                            filter_query.clear();
                            filtered_indices = (0..items.len()).collect();
                            state.select(Some(0));
                        }
                        KeyCode::Enter => {
                            filtering = false;
                        }
                        KeyCode::Backspace => {
                            filter_query.pop();
                            let q = filter_query.to_lowercase();
                            filtered_indices = items
                                .iter()
                                .enumerate()
                                .filter(|(_, s)| !s.is_empty() && s.to_lowercase().contains(&q))
                                .map(|(i, _)| i)
                                .collect();
                            state.select(if filtered_indices.is_empty() {
                                None
                            } else {
                                Some(0)
                            });
                        }
                        KeyCode::Char(c) => {
                            filter_query.push(c);
                            let q = filter_query.to_lowercase();
                            filtered_indices = items
                                .iter()
                                .enumerate()
                                .filter(|(_, s)| !s.is_empty() && s.to_lowercase().contains(&q))
                                .map(|(i, _)| i)
                                .collect();
                            state.select(if filtered_indices.is_empty() {
                                None
                            } else {
                                Some(0)
                            });
                        }
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Left => return Ok(None),
                    KeyCode::Char('/') => {
                        filtering = true;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if !filtered_indices.is_empty() {
                            let cur = state.selected().unwrap_or(0);
                            let mut next = (cur + 1) % filtered_indices.len();
                            if items[filtered_indices[next]].is_empty() && filtered_indices.len() > 1
                            {
                                next = (next + 1) % filtered_indices.len();
                            }
                            state.select(Some(next));
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if !filtered_indices.is_empty() {
                            let cur = state.selected().unwrap_or(0);
                            let mut prev = if cur == 0 {
                                filtered_indices.len() - 1
                            } else {
                                cur - 1
                            };
                            if items[filtered_indices[prev]].is_empty() && filtered_indices.len() > 1
                            {
                                prev = if prev == 0 {
                                    filtered_indices.len() - 1
                                } else {
                                    prev - 1
                                };
                            }
                            state.select(Some(prev));
                        }
                    }
                    KeyCode::Enter | KeyCode::Right => {
                        if let Some(sel) = state.selected()
                            && let Some(&orig_idx) = filtered_indices.get(sel)
                            && !items[orig_idx].is_empty()
                        {
                            return Ok(Some(orig_idx));
                        }
                    }
                    _ => {}
                }
            }
        }
    })
}

pub fn input(title: &str, placeholder: &str) -> Result<Option<String>> {
    with_terminal(|terminal| {
        let mut text = String::new();

        loop {
            terminal.draw(|f| {
                let area = f.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .horizontal_margin(2)
                    .vertical_margin(1)
                    .constraints([
                        Constraint::Length(2),
                        Constraint::Length(2),
                        Constraint::Length(1),
                        Constraint::Min(0),
                    ])
                    .split(area);

                let header = Paragraph::new(Line::from(Span::styled(
                    title,
                    Style::default().fg(PINK).add_modifier(Modifier::BOLD),
                )));
                f.render_widget(header, chunks[0]);

                let display = if text.is_empty() {
                    Line::from(vec![
                        Span::styled(
                            "» ",
                            Style::default().fg(GOLD).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(placeholder, Style::default().fg(MUTED)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(
                            "» ",
                            Style::default().fg(GOLD).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(&text, Style::default().fg(NORMAL)),
                        Span::styled("▎", Style::default().fg(GOLD)),
                    ])
                };
                f.render_widget(Paragraph::new(display), chunks[1]);

                let help = Paragraph::new(Line::from(vec![
                    Span::styled("enter", Style::default().fg(GOLD)),
                    Span::styled(" ara • ", Style::default().fg(MUTED)),
                    Span::styled("esc", Style::default().fg(GOLD)),
                    Span::styled(" geri", Style::default().fg(MUTED)),
                ]));
                f.render_widget(help, chunks[2]);
            })?;

            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
                {
                    return Ok(None);
                }
                match key.code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Enter => {
                        let trimmed = text.trim().to_string();
                        if trimmed.is_empty() {
                            return Ok(None);
                        }
                        return Ok(Some(trimmed));
                    }
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char(c) => {
                        text.push(c);
                    }
                    _ => {}
                }
            }
        }
    })
}
