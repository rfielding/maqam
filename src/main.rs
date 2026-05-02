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

use crossbeam_channel::bounded;

fn main() -> anyhow::Result<()> {
    let (tx, rx) = bounded::<sequencer::AudioCmd>(512);

    // Keep the stream alive for the lifetime of the app.
    let _stream = audio::start_audio(rx)?;

    let mut app = app::App::new(tx);
    ui::run(&mut app)?;

    Ok(())
}
