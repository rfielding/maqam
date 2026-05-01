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
                        app.handle_command(&cmd);
                    }
                    KeyCode::Up    => { app.history_up(); }
                    KeyCode::Down  => { app.history_down(); }
                    KeyCode::Backspace => {
                        app.history_pos = None;
                        app.input.pop();
                    }
                    KeyCode::Char(c) => {
                        app.history_pos = None;
                        app.input.push(c);
                    }
                    KeyCode::Esc => {
                        app.input.clear();
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
    let items: Vec<ListItem> = app.phrases.iter().map(|phrase| {
        let id_str  = format!("{:>2}: ", phrase.id);
        let src_str = format!("{:<28}", phrase.src);
        let rhythm  = phrase.rhythm_display();
        let maqam_str = phrase.bar.maqam_names.join("+");

        let mut spans = vec![
            Span::styled(id_str,  Style::default().fg(DIM)),
            Span::styled(src_str, Style::default().fg(ACCENT)),
            Span::raw(" "),
        ];

        for ch in rhythm.chars() {
            let (g, col) = match ch { 'X' => ('X', KICK), _ => ('.', SNARE) };
            spans.push(Span::styled(g.to_string(), Style::default().fg(col)));
        }

        if true {
            spans.push(Span::styled(
                format!("  ×{}", phrase.repeat),
                Style::default().fg(REPEAT).add_modifier(Modifier::BOLD),
            ));
        }

        spans.push(Span::styled(
            format!("  {maqam_str}"),
            Style::default().fg(MAQAM).add_modifier(Modifier::DIM),
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
    let para = Paragraph::new(format!("> {}_", app.input))
        .style(Style::default().fg(CMD).bg(BG))
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
