# vcv-midi patch — apply instructions

## Step 1: copy new file
cp src/midi.rs  (already done by this tarball)

## Step 2: edit src/main.rs

Add after existing `mod` block:
    mod midi;

Add after existing statics (after jump_counters fn):
    pub static MIDI_OUT: std::sync::OnceLock<midi::MidiOutput> = std::sync::OnceLock::new();

Replace fn main() body — keep all existing content but wrap it like this:

    fn main() -> anyhow::Result<()> {
        let mut args = std::env::args().skip(1).peekable();
        let mut midi_device: Option<String> = None;
        while let Some(arg) = args.next() {
            if arg == "--midi" { midi_device = args.next(); }
        }
        if let Some(dev) = midi_device {
            match midi::MidiOutput::start(dev.clone(), 120.0) {
                Ok(out) => { let _ = MIDI_OUT.set(out); eprintln!("MIDI → {dev}"); }
                Err(e)  => { eprintln!("Warning: MIDI unavailable: {e}"); }
            }
        }

        let (tx, rx) = bounded::<sequencer::AudioCmd>(512);
        let _stream = audio::start_audio(rx)?;
        let mut app = app::App::new(tx);
        ui::run(&mut app)?;

        if let Some(out) = MIDI_OUT.get() {
            let _ = out.tx.send(midi::MidiEvent::AllOff);
        }
        Ok(())
    }

## Step 3: edit src/audio.rs

### Add import near top:
    use crate::midi::MidiEvent;

### After `spawn_voices(ev, sustain, &mut voices, milestone, &scale);` add:
                        if let Some(midi) = crate::MIDI_OUT.get() {
                            let sustain_ms = (sustain * 1000.0) as u64;
                            match ev {
                                SubdivEvent::Kick(hz) => {
                                    let _ = midi.tx.try_send(MidiEvent::Note { hz, sustain_ms });
                                    let _ = midi.tx.try_send(MidiEvent::Kick);
                                }
                                SubdivEvent::Snare(hz) => {
                                    let _ = midi.tx.try_send(MidiEvent::Note { hz, sustain_ms });
                                    let _ = midi.tx.try_send(MidiEvent::Snare);
                                }
                            }
                        }

### In AudioCmd::SetBpm handler, after `pp.rebuild(sr, bpm)` loop, add:
                        if let Some(midi) = crate::MIDI_OUT.get() {
                            let _ = midi.tx.try_send(MidiEvent::SetBpm(b));
                        }
