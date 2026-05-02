// ui.rs — one row per phrase

use std::io;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame, Terminal,
};
use crate::app::App;

const BG:       Color = Color::Rgb(18, 18, 22);
const BORDER:   Color = Color::Rgb(55, 55, 70);
const ACCENT:   Color = Color::Rgb(100, 200, 170);
const DIM:      Color = Color::Rgb(90, 90, 110);
const KICK:     Color = Color::Rgb(220, 160, 80);
const SNARE:    Color = Color::Rgb(100, 140, 200);
const CMD:      Color = Color::Rgb(200, 200, 100);
const ERR:      Color = Color::Rgb(200, 80, 80);
const MAQAM:    Color = Color::Rgb(150, 220, 150);
const REPEAT:   Color = Color::Rgb(180, 120, 220);

pub fn run(app: &mut App) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend  = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    loop {
        term.draw(|f| draw(f, app))?;

        if event::poll(std::time::Duration::from_millis(40))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('c') | KeyCode::Char('q')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    { app.should_quit = true; }

                    KeyCode::Enter => {
                        let cmd = app.input.clone();
                        app.history_push(&cmd);
                        app.input.clear();
                        app.cursor_pos = 0;
                        app.handle_command(&cmd);
                    }
                    KeyCode::Up    => { app.history_up(); }
                    KeyCode::Down  => { app.history_down(); }
                    KeyCode::Left  => { app.cursor_left(); }
                    KeyCode::Right => { app.cursor_right(); }
                    KeyCode::Home  => { app.cursor_home(); }
                    KeyCode::End   => { app.cursor_end(); }
                    KeyCode::Delete => {
                        app.history_pos = None;
                        app.delete_char();
                    }
                    KeyCode::Backspace => {
                        app.history_pos = None;
                        app.backspace();
                    }
                    KeyCode::Char(c) => {
                        app.history_pos = None;
                        app.insert_char(c);
                    }
                    KeyCode::Esc => {
                        app.input.clear();
                        app.cursor_pos  = 0;
                        app.message     = None;
                        app.history_pos = None;
                    }
                    _ => {}
                }
            }
        }
        if app.should_quit { break; }
    }

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

fn draw(f: &mut Frame, app: &App) {
    let area   = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
.constraints([Constraint::Min(3), Constraint::Length(3), Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    draw_phrases(f, app, chunks[0]);
    draw_input(f, app, chunks[1]);
    draw_status(f, app, chunks[2]);
    draw_recording(f, app, chunks[3]);
}

fn draw_phrases(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let cur = if app.paused { usize::MAX } else {
        crate::CUR_PHRASE.load(std::sync::atomic::Ordering::Relaxed)
    };
    let cur_sub   = crate::CUR_SUBDIV.load(std::sync::atomic::Ordering::Relaxed);
    let cur_plays = crate::CUR_PLAYS.load(std::sync::atomic::Ordering::Relaxed);
    let n = app.phrases.len().max(1);

    let items: Vec<ListItem> = app.phrases.iter().enumerate().map(|(idx, phrase)| {
        let playing = !app.paused && idx == cur % n;
        let id_str  = format!("{:>2}: ", idx);
        let marker  = if playing { "▶ " } else { "  " };

        // Jump entries render as control-flow markers
        if phrase.jump.is_some() {
            let col = if playing { Color::Rgb(220, 190, 80) } else { Color::Rgb(110, 95, 40) };
            return ListItem::new(Line::from(vec![
                Span::styled(marker,             Style::default().fg(ACCENT)),
                Span::styled(id_str,             Style::default().fg(DIM)),
                Span::styled(phrase.src.clone(), Style::default().fg(col).add_modifier(Modifier::BOLD)),
            ]));
        }

        let src_str   = format!("{:<28}", phrase.src);
        let rhythm    = phrase.rhythm_display();
        let maqam_str = phrase.bar.maqam_names.join("+");

        let (fg_id, fg_src) = if playing {
            (ACCENT, Color::Rgb(255, 255, 180))
        } else {
            (DIM, ACCENT)
        };

        let mut spans = vec![
            Span::styled(marker,  Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(id_str,  Style::default().fg(fg_id)),
            Span::styled(src_str, Style::default().fg(fg_src)),
            Span::raw(" "),
        ];

        for (si, ch) in rhythm.chars().enumerate() {
            let is_now = playing && si == cur_sub;
            let col = if is_now { Color::Rgb(255,255,255) }
                      else if playing { match ch { 'X' => KICK, _ => SNARE } }
                      else { match ch { 'X' => Color::Rgb(140,100,50), _ => Color::Rgb(60,80,110) } };
            let sty = if is_now {
                Style::default().fg(col).add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else { Style::default().fg(col) };
            spans.push(Span::styled(ch.to_string(), sty));
        }

        if playing && phrase.repeat > 1 {
            spans.push(Span::styled(
                format!(" {}/{}", cur_plays + 1, phrase.repeat),
                Style::default().fg(Color::Rgb(180,180,100)).add_modifier(Modifier::BOLD),
            ));
        } else if !playing && phrase.repeat > 1 {
            spans.push(Span::styled(
                format!("  ×{}", phrase.repeat),
                Style::default().fg(REPEAT),
            ));
        }

        spans.push(Span::styled(
            format!("  {maqam_str}"),
            Style::default().fg(MAQAM).add_modifier(if playing { Modifier::empty() } else { Modifier::DIM }),
        ));

        ListItem::new(Line::from(spans))
    }).collect();

    let list = List::new(items)
        .block(Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" maqam-live ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)))
            .border_style(Style::default().fg(BORDER))
            .style(Style::default().bg(BG)));
    f.render_widget(list, area);
}

fn draw_input(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    // Render with cursor block at cursor_pos
    let chars: Vec<char> = app.input.chars().collect();
    let mut spans = vec![Span::styled("> ", Style::default().fg(DIM))];
    for (i, &ch) in chars.iter().enumerate() {
        if i == app.cursor_pos {
            spans.push(Span::styled(
                ch.to_string(),
                Style::default().fg(BG).bg(CMD).add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(ch.to_string(), Style::default().fg(CMD)));
        }
    }
    // Cursor at end of input
    if app.cursor_pos >= chars.len() {
        spans.push(Span::styled(
            " ",
            Style::default().fg(BG).bg(CMD),
        ));
    }
    let para = Paragraph::new(Line::from(spans))
        .style(Style::default().bg(BG))
        .block(Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" cmd ", Style::default().fg(DIM)))
            .border_style(Style::default().fg(BORDER)));
    f.render_widget(para, area);
}

fn draw_status(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let text = if let Some(msg) = &app.message {
        let col = if msg.starts_with('✗') { ERR } else { DIM };
        Line::from(vec![Span::styled(format!("  {msg}"), Style::default().fg(col))])
    } else {
        Line::from(vec![Span::styled(
            format!("  {}BPM:{} sus:{:.1}s vol:{:.2} phrases:{}  [?] help  [z] pause",
                if app.paused { "⏸ PAUSED  " } else { "" },
                app.bpm, app.sustain, app.vol, app.phrases.len()),
            Style::default().fg(DIM),
        )])
    };
    f.render_widget(Paragraph::new(text).style(Style::default().bg(BG)), area);
}

fn draw_recording(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let text = match &app.last_recording {
        Some(path) => Line::from(vec![
            Span::styled("  ◉ ", Style::default().fg(Color::Rgb(200, 80, 80))),
            Span::styled(path.as_str(), Style::default().fg(Color::Rgb(160, 160, 180))),
        ]),
        None => Line::from(vec![
            Span::styled("  m → record cycle to $HOME/maqam-<ts>.mp4",
                Style::default().fg(Color::Rgb(55, 55, 70))),
        ]),
    };
    f.render_widget(Paragraph::new(text).style(Style::default().bg(BG)), area);
}
