// main.rs — maqam-live: real-time maqam sequencer / REPL

mod app;
mod audio;
mod command;
mod sequencer;
mod tuning;
mod record;
mod synth;
mod ui;

use crossbeam_channel::bounded;

fn main() -> anyhow::Result<()> {
    let (tx, rx) = bounded::<sequencer::AudioCmd>(512);

    // Keep the stream alive for the lifetime of the app.
    let _stream = audio::start_audio(rx)?;

    let mut app = app::App::new(tx);
    ui::run(&mut app)?;

    Ok(())
}
