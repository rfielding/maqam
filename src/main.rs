// main.rs — maqam-live: real-time maqam sequencer / REPL

mod app;
mod audio;
mod command;
mod sequencer;
mod tuning;
mod record;
mod synth;
mod ui;

/// Shared atomic: audio thread writes current phrase index, TUI reads it.
pub static CUR_PHRASE: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub static CUR_SUBDIV: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub static CUR_PLAYS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub static CUR_JUMP_REM: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Jump counters visible to TUI: phrase_id → remaining jumps.
/// Written by audio thread on every jump state change.
pub static JUMP_COUNTERS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, usize>>
> = std::sync::OnceLock::new();

pub fn jump_counters() -> &'static std::sync::Mutex<std::collections::HashMap<usize, usize>> {
    JUMP_COUNTERS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

use crossbeam_channel::bounded;

fn main() -> anyhow::Result<()> {
    let (tx, rx) = bounded::<sequencer::AudioCmd>(512);

    // Keep the stream alive for the lifetime of the app.
    let _stream = audio::start_audio(rx)?;

    let mut app = app::App::new(tx);
    ui::run(&mut app)?;

    Ok(())
}
