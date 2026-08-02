// command.rs — mini-language parser

use crate::fx::FxSettings;
use crate::tuning::{Maqam, Pitch};
use crate::vcf::{VcfBank, VcfSettings, VcfTarget, VcoWave};

pub const START_REF: isize = isize::MIN;

pub struct JinsSpec {
    pub src: String,
    pub root: Pitch,
    pub maqam: Maqam,
    pub groups: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VcfChange {
    pub enabled: Option<bool>,
    pub target: Option<VcfTarget>,
    pub cutoff_hz: Option<ValueChange>,
    pub resonance: Option<ValueChange>,
    pub drive: Option<ValueChange>,
    pub wave: Option<VcoWave>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FxChange {
    pub reverb_enabled: Option<bool>,
    pub reverb_mix: Option<ValueChange>,
    pub reverb_decay: Option<ValueChange>,
    pub delay_enabled: Option<bool>,
    pub delay_time_secs: Option<ValueChange>,
    pub delay_feedback: Option<ValueChange>,
    pub delay_mix: Option<ValueChange>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SympatheticChange {
    pub target: Option<SympatheticTarget>,
    pub enabled: Option<bool>,
    pub decay: Option<f32>,
    pub gain: Option<f32>,
    pub interval_ratio: Option<f64>,
    pub harmony: Option<SympatheticHarmony>,
    pub amount: Option<f32>,
    pub mic: Option<f32>,
    pub kanun: Option<f32>,
    pub bass: Option<f32>,
    pub drums: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NamCommand {
    Login,
    Logout,
    Load { path: String },
    Import { path: String, name: Option<String> },
    Pin { url: String, name: String },
    Tone3000 { tone_id: u64, name: String },
    Search { query: String },
    List,
    Off,
    Gain(f32),
    Input(NamInput),
    Latency(NamInput),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NamInput {
    Left,
    Right,
    Stereo,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SympatheticHarmonyComponent {
    pub ratio: f64,
    pub weight: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SympatheticHarmony {
    pub components: [SympatheticHarmonyComponent; 8],
    pub len: usize,
}

impl SympatheticHarmony {
    #[contracts::debug_requires(
        self.len <= self.components.len(),
        "harmony length stays within storage"
    )]
    #[contracts::debug_ensures(
        self.len <= self.components.len(),
        "harmony length stays within storage"
    )]
    pub(crate) fn push(&mut self, component: SympatheticHarmonyComponent) -> Result<(), String> {
        if self.len >= self.components.len() {
            return Err("sym harmony can contain at most 8 intervals".into());
        }
        self.components[self.len] = component;
        self.len += 1;
        Ok(())
    }

    #[contracts::debug_requires(
        self.len <= self.components.len(),
        "harmony length stays within storage"
    )]
    pub fn iter(&self) -> impl Iterator<Item = SympatheticHarmonyComponent> + '_ {
        self.components[..self.len].iter().copied()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SympatheticTarget {
    All,
    Mic,
    Kanun,
    Bass,
    Drums,
}

impl SympatheticTarget {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "all" | "mix" | "master" => Some(Self::All),
            "mic" | "input" | "live" => Some(Self::Mic),
            "kanun" | "qanun" | "melody" => Some(Self::Kanun),
            "bass" | "sub" | "subbass" => Some(Self::Bass),
            "drums" | "drum" | "kick" | "kicks" => Some(Self::Drums),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Mic => "mic",
            Self::Kanun => "kanun",
            Self::Bass => "bass",
            Self::Drums => "drums",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CommandMetadata {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub targets: &'static [CommandTokenMetadata],
    pub parameters: &'static [CommandParameterMetadata],
    pub first_parameter: &'static str,
    pub notes: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub struct CommandTokenMetadata {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub struct CommandParameterMetadata {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub values: &'static [&'static str],
    pub units: &'static str,
    pub lower: Option<f64>,
    pub upper: Option<f64>,
    pub typical: &'static str,
    pub notes: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub struct LanguagePatternMetadata {
    pub syntax: &'static str,
    pub description: &'static str,
    pub notes: &'static [&'static str],
}

const VCF_TARGETS: &[CommandTokenMetadata] = &[
    CommandTokenMetadata {
        name: "all",
        aliases: &["mix", "master"],
    },
    CommandTokenMetadata {
        name: "mic",
        aliases: &["input", "live"],
    },
    CommandTokenMetadata {
        name: "bass",
        aliases: &["sub", "subbass"],
    },
    CommandTokenMetadata {
        name: "kanun",
        aliases: &["qanun", "melody"],
    },
    CommandTokenMetadata {
        name: "drums",
        aliases: &["drum", "kick", "kicks"],
    },
    CommandTokenMetadata {
        name: "sym",
        aliases: &["tanbura", "tambura", "sympathetics"],
    },
];

const VCF_PARAMETERS: &[CommandParameterMetadata] = &[
    CommandParameterMetadata {
        name: "cut",
        aliases: &["cutoff", "freq", "frequency"],
        description: "filter cutoff frequency",
        values: &[],
        units: "Hz",
        lower: Some(10.0),
        upper: Some(22000.0),
        typical: "700..4000 for audible sweeps",
        notes: &[],
    },
    CommandParameterMetadata {
        name: "res",
        aliases: &["q", "reso", "resonance"],
        description: "filter resonance",
        values: &[],
        units: "",
        lower: Some(0.0),
        upper: Some(0.98),
        typical: "0.2..0.75",
        notes: &["values near 0.98 can ring sharply"],
    },
    CommandParameterMetadata {
        name: "drive",
        aliases: &["drv"],
        description: "filter input drive",
        values: &[],
        units: "",
        lower: Some(0.1),
        upper: Some(12.0),
        typical: "1..4",
        notes: &[],
    },
    CommandParameterMetadata {
        name: "wave",
        aliases: &["wav", "shape"],
        description: "VCO/source waveform used by instrument VCFs",
        values: &["sin", "tri", "squ", "saw", "mic"],
        units: "",
        lower: None,
        upper: None,
        typical: "target instrument defaults to its own source",
        notes: &["vcf all ignores wave because it filters the final outgoing mix"],
    },
    CommandParameterMetadata {
        name: "off",
        aliases: &[],
        description: "turn the selected VCF off",
        values: &[],
        units: "",
        lower: None,
        upper: None,
        typical: "",
        notes: &[],
    },
];

const SYM_TARGETS: &[CommandTokenMetadata] = &[
    CommandTokenMetadata {
        name: "all",
        aliases: &["mix", "master"],
    },
    CommandTokenMetadata {
        name: "mic",
        aliases: &["input", "live"],
    },
    CommandTokenMetadata {
        name: "kanun",
        aliases: &["qanun", "melody"],
    },
    CommandTokenMetadata {
        name: "bass",
        aliases: &["sub", "subbass"],
    },
    CommandTokenMetadata {
        name: "drums",
        aliases: &["drum", "kick", "kicks"],
    },
];

const SYM_PARAMETERS: &[CommandParameterMetadata] = &[
    CommandParameterMetadata {
        name: "decay",
        aliases: &[],
        description: "sympathetic string decay time",
        values: &[],
        units: "",
        lower: Some(0.9),
        upper: Some(0.99999),
        typical: "0.999",
        notes: &["higher values ring longer"],
    },
    CommandParameterMetadata {
        name: "drive",
        aliases: &["gain"],
        description: "sympathetic resonator excitation drive",
        values: &[],
        units: "",
        lower: Some(0.0),
        upper: Some(512.0),
        typical: "0.5..8",
        notes: &[
            "default is 2",
            "values above 16 are extreme unless the user explicitly asks",
        ],
    },
    CommandParameterMetadata {
        name: "amount",
        aliases: &["amt", "level", "send"],
        description: "source send amount into a target sympathetic partition",
        values: &[],
        units: "",
        lower: Some(0.0),
        upper: Some(512.0),
        typical: "0..4",
        notes: &["hard range is wide for sound design; ordinary edits should stay small"],
    },
    CommandParameterMetadata {
        name: "up",
        aliases: &["interval", "transpose"],
        description: "multiply sympathetic resonator target strings upward by an ideal interval ratio",
        values: &[
            "unison", "second", "third", "fourth", "fifth", "sixth", "seventh", "octave",
        ],
        units: "ratio",
        lower: Some(0.25),
        upper: Some(4.0),
        typical: "third, fourth, fifth, octave",
        notes: &[
            "`sym down <interval>` uses the reciprocal interval",
            "`sym harmony root third fourth octave` combines weighted target intervals",
            "explicit ratios are accepted, e.g. `sym interval 3/2`",
            "third and sixth quality matters; use minor-third/major-third or minor-sixth/major-sixth",
        ],
    },
    CommandParameterMetadata {
        name: "mic",
        aliases: &["input", "live"],
        description: "mic source send amount",
        values: &[],
        units: "",
        lower: Some(0.0),
        upper: Some(512.0),
        typical: "0..4",
        notes: &[],
    },
    CommandParameterMetadata {
        name: "kanun",
        aliases: &["qanun", "melody"],
        description: "kanun source send amount",
        values: &[],
        units: "",
        lower: Some(0.0),
        upper: Some(512.0),
        typical: "0..4",
        notes: &[],
    },
    CommandParameterMetadata {
        name: "bass",
        aliases: &["sub", "subbass"],
        description: "bass source send amount",
        values: &[],
        units: "",
        lower: Some(0.0),
        upper: Some(512.0),
        typical: "0..4",
        notes: &[],
    },
    CommandParameterMetadata {
        name: "drums",
        aliases: &["drum", "kick", "kicks"],
        description: "drum source send amount",
        values: &[],
        units: "",
        lower: Some(0.0),
        upper: Some(512.0),
        typical: "0..4",
        notes: &[],
    },
    CommandParameterMetadata {
        name: "on",
        aliases: &[],
        description: "turn sympathetics on",
        values: &[],
        units: "",
        lower: None,
        upper: None,
        typical: "",
        notes: &[],
    },
    CommandParameterMetadata {
        name: "off",
        aliases: &[],
        description: "turn sympathetics off",
        values: &[],
        units: "",
        lower: None,
        upper: None,
        typical: "",
        notes: &[],
    },
];

pub const VCF_METADATA: CommandMetadata = CommandMetadata {
    name: "vcf",
    aliases: &[
        "filter", "filt", "cut", "cutoff", "res", "q", "drive", "drv",
    ],
    description: "voltage-controlled filter lines; each target can have its own partition, and vcf all filters the final outgoing mix",
    targets: VCF_TARGETS,
    parameters: VCF_PARAMETERS,
    first_parameter: "cut",
    notes: &[
        "named parameters may be combined on one line; omitted values keep their current value",
        "instrument targets default wave to their own source, so `vcf mic ... wave mic` is redundant",
        "relative values like cut=+100, res=-0.1, drive=*1.5, and tick values like cut=+2t are accepted",
    ],
};

pub const SYM_METADATA: CommandMetadata = CommandMetadata {
    name: "sym",
    aliases: &["sympathetics", "tanbura", "tambura"],
    description: "sympathetic resonator lines partitioned by all, mic, kanun, bass, and drums",
    targets: SYM_TARGETS,
    parameters: SYM_PARAMETERS,
    first_parameter: "decay",
    notes: &[
        "named parameters may be combined on one line; omitted values keep their current value",
        "partitioned lines let mic have different decay, drive, and amount from kanun, bass, or drums",
        "`gain` is accepted as an alias for `drive`, but prefer `drive` when generating commands",
    ],
};

const NAM_PARAMETERS: &[CommandParameterMetadata] = &[
    CommandParameterMetadata {
        name: "load",
        aliases: &[],
        description: "load a cached NAM capture name, .nam file path, or URL for live mic input",
        values: &[],
        units: "name/path/url",
        lower: None,
        upper: None,
        typical: "nam metallica",
        notes: &[
            "the `load` word is optional; `nam metallica` and `nam load metallica` are accepted",
            "use `nam import FILENAME.nam as name` before loading by cached name",
            "URL loads download into the local cache first and show progress",
        ],
    },
    CommandParameterMetadata {
        name: "import",
        aliases: &["pull"],
        description: "copy or download a .nam file into the local NAM capture cache",
        values: &[],
        units: "path/url/name",
        lower: None,
        upper: None,
        typical: "nam import https://example.com/amp.nam as amp",
        notes: &[
            "without `as name`, the cache name is derived from the file name or URL",
            "by default captures are cached in ./.nam, which is created automatically",
            "URL imports show a progress meter while downloading",
        ],
    },
    CommandParameterMetadata {
        name: "ls",
        aliases: &["list"],
        description: "list cached NAM captures and .nam files in the current directory",
        values: &[],
        units: "",
        lower: None,
        upper: None,
        typical: "",
        notes: &[],
    },
    CommandParameterMetadata {
        name: "gain",
        aliases: &["drive"],
        description: "input gain before the NAM model",
        values: &[],
        units: "",
        lower: Some(0.0),
        upper: Some(8.0),
        typical: "0.5..2",
        notes: &["use this to control how hard the amp model is driven"],
    },
    CommandParameterMetadata {
        name: "off",
        aliases: &[],
        description: "bypass the live input NAM model",
        values: &[],
        units: "",
        lower: None,
        upper: None,
        typical: "",
        notes: &[],
    },
];

pub const NAM_METADATA: CommandMetadata = CommandMetadata {
    name: "nam",
    aliases: &[],
    description: "live mic-input Neural Amp Modeler A1/A2 amp stage",
    targets: &[],
    parameters: NAM_PARAMETERS,
    first_parameter: "load",
    notes: &[
        "NAM is live input state and is not saved in .mq files",
        "the chain is mic input -> NAM -> vcf mic or vcf all -> output",
        "cached captures live under ./.nam unless MAQAM_NAM_CACHE_DIR is set",
        "NAM models have an expected sample rate; set the audio device to that rate if the model sounds wrong",
    ],
};

pub const LANGUAGE_PATTERNS: &[LanguagePatternMetadata] = &[
    LanguagePatternMetadata {
        syntax: "<root> <jins> [rhythm]",
        description: "add a phrase using one jins or Western mode",
        notes: &[
            "roots are c d e f g a b with optional + or -, e.g. b-",
            "jins/modes include bayati, hijaz, rast, kurd, saba, ajam, nahawand, major, minor, dorian, phrygian, lydian, mixolydian, aeolian, locrian, diminished",
            "rhythm is a run of group lengths such as 4444 or 332332",
            "there is no time signature setting; express time signatures and meter by grouping rhythm chunks",
            "for 7/8 use 43, 34, 223, or similar groupings; 4433 is two 7/8 bars",
        ],
    },
    LanguagePatternMetadata {
        syntax: "<root> <jins>, <root> <jins> [rhythm]",
        description: "add a phrase that changes jins inside the phrase",
        notes: &["use this for compact turnarounds and mixed modal phrases"],
    },
    LanguagePatternMetadata {
        syntax: "<phrase> r<N>",
        description: "repeat a phrase locally before moving to the next timeline row",
        notes: &[
            "prefer repeats over many duplicated phrase rows",
            "when the user asks for N bars of one phrase, use r<N>",
            "16 bars in 7/8 is usually `<root> <jins> 43 r16`; if using a two-bar grouping like 4433, use r8",
        ],
    },
    LanguagePatternMetadata {
        syntax: "j <id> <times>",
        description: "jump back or forward to an existing timeline id",
        notes: &[
            "use jumps only to restart a multi-row section after its intended duration",
            "do not use a jump where one phrase repeat like r16 expresses the requested bars",
            "jump times count passes through the jump row, not phrase rows and not bars",
            "for a restart after 16 bars, count 16 steps in time, not 16 phrase rows; count time bars from rhythm group totals and phrase repeats, then place one jump at that point",
            "a jump with times 1 is a no-op; do not generate `j <id> 1`",
            "keep generated scores readable in about a dozen timeline rows",
        ],
    },
    LanguagePatternMetadata {
        syntax: "i <id> <command>",
        description: "insert a phrase or control command before an existing id",
        notes: &["the current timeline id may be used as the insertion point"],
    },
    LanguagePatternMetadata {
        syntax: "edit <id> <command>",
        description: "replace an existing timeline row",
        notes: &["tab completion after edit id should offer the row's current command"],
    },
    LanguagePatternMetadata {
        syntax: "x <id> [id ...]",
        description: "delete one or more timeline rows",
        notes: &[],
    },
    LanguagePatternMetadata {
        syntax: "up <id> | down <id> | rot | stop",
        description: "move rows, rotate the timeline, or insert a stop line",
        notes: &[],
    },
    LanguagePatternMetadata {
        syntax: "bpm <20..400>",
        description: "add a tempo control line",
        notes: &["write `bpm 180`, never `set tempo 180`"],
    },
    LanguagePatternMetadata {
        syntax: "s <0.05..10> | sus <0.05..10>",
        description: "add a sustain control line in seconds",
        notes: &[],
    },
    LanguagePatternMetadata {
        syntax: "sym on | sym off",
        description: "turn sympathetic resonators on or off",
        notes: &[
            "if the user says `add in sympathetics`, make the edit; do not merely explain it",
            "a useful default is `sym on` followed by `sym decay 0.999 drive 2 kanun 0.5 bass 0.5`",
        ],
    },
    LanguagePatternMetadata {
        syntax: "sym decay <0.9..0.99999> drive <0..512>",
        description: "set global sympathetic decay and drive on one control line",
        notes: &[
            "`sym gain <0..512>` is accepted as an alias for drive",
            "decay 0.999 is typical",
            "drive practical edit values are usually 0.5..8",
            "drive values above 16 are extreme unless the user explicitly asks",
        ],
    },
    LanguagePatternMetadata {
        syntax: "sym decay <n> drive <n> kanun <n> bass <n>",
        description: "set multiple sympathetic parameters on a compressed global line",
        notes: &["only mentioned values change; omitted values keep their current value"],
    },
    LanguagePatternMetadata {
        syntax: "sym up <interval> | sym down <interval> | sym interval <ratio>",
        description: "transpose sympathetic resonator target strings",
        notes: &[
            "named intervals include second, third, fourth, fifth, sixth, seventh, and octave",
            "named intervals use ideal ratios: minor-third=6/5, major-third=5/4, fourth=4/3, fifth=3/2, octave=2/1",
            "generic third means minor-third; use major-third when the harmony needs 5/4",
            "explicit ratios like `sym interval 3/2` are accepted",
        ],
    },
    LanguagePatternMetadata {
        syntax: "sym harmony <interval> [weight] [interval weight ...]",
        description: "combine multiple sympathetic target harmonies with weighted energy",
        notes: &[
            "example: `sym harmony root third fourth octave`",
            "example with explicit split: `sym harmony root 0.50 third 0.25 fifth 0.25`",
            "default behavior is equivalent to `sym harmony root 1.0`",
            "weights are normalized to a total energy of 1.0 before retuning",
        ],
    },
    LanguagePatternMetadata {
        syntax: "sym <all|mic|kanun|bass|drums> decay <n> drive <n> amount <n>",
        description: "set one sympathetic partition independently",
        notes: &[
            "amount/source sends have hard range 0..512",
            "practical amount/source send values are usually 0..4",
        ],
    },
    LanguagePatternMetadata {
        syntax: "vcf off | vcf <all|mic|bass|kanun|drums|sym> off",
        description: "turn all VCFs off or turn one VCF partition off",
        notes: &[],
    },
    LanguagePatternMetadata {
        syntax: "vcf <target> cut <10..22000> res <0..0.98> drive <0.1..12> wave <sin|tri|squ|saw|mic>",
        description: "set one VCF partition with named parameters",
        notes: &[
            "only mentioned values change; omitted values keep their current value",
            "instrument targets default wave to their own source",
            "vcf all ignores wave and filters the final outgoing mix",
        ],
    },
    LanguagePatternMetadata {
        syntax: "reverb on/off | reverb mix <0..1> decay <0..0.98>",
        description: "turn reverb on or off and edit reverb parameters",
        notes: &["named parameters may be combined on one line"],
    },
    LanguagePatternMetadata {
        syntax: "delay on/off | delay time <0.01..2> feedback <0..0.95> mix <0..1>",
        description: "turn ping-pong delay on or off and edit delay parameters",
        notes: &["`pingpong` is accepted as an alias for delay"],
    },
    LanguagePatternMetadata {
        syntax: "fx off",
        description: "turn reverb and delay off",
        notes: &[],
    },
    LanguagePatternMetadata {
        syntax: "nam login | nam logout | nam tone3000 ID as name | nam pin URL as name | nam import FILENAME.nam [as name] | nam input left|right|stereo | nam latency left|right | nam search <query> | nam <name|FILENAME.nam|URL> | nam load <name|FILENAME.nam|URL> | nam ls | nam off | nam gain <0..8>",
        description: "cache, load, list, or bypass Neural Amp Modeler A1/A2 captures on live mic input",
        notes: &[
            "`nam pin URL as name` writes an unambiguous downloadable dependency into the loaded .mq file",
            "`nam login` opens TONE3000 OAuth in a browser while the TUI keeps running; `nam logout` forgets it",
            "do not invent fake NAM paths; use a real local .nam file or URL",
            "import a capture once, then load it later by cached name",
            "`nam ls` browses cached captures plus .nam files in the current directory",
            "`nam search Fender clean A2` searches for real capture pages and direct .nam links",
            "use `vcf mic` to filter the modeled input bus or `vcf all` to filter the final mix",
        ],
    },
    LanguagePatternMetadata {
        syntax: "create <Name> <ratios...> | delete <Name>",
        description: "create or delete a custom jins",
        notes: &["ratios use forms like 1/1 9/8 5/4"],
    },
];

pub fn language_reference() -> String {
    let mut out = String::new();
    out.push_str("maqam-live command language reference\n\n");
    out.push_str("Core rule: use only this command language; do not invent English aliases.\n\n");
    out.push_str("Patterns:\n");
    for pattern in LANGUAGE_PATTERNS {
        out.push_str("- `");
        out.push_str(pattern.syntax);
        out.push_str("`: ");
        out.push_str(pattern.description);
        append_notes(&mut out, pattern.notes);
        out.push('\n');
    }
    out.push('\n');
    out.push_str("Nouns:\n");
    for meta in [&VCF_METADATA, &SYM_METADATA, &NAM_METADATA] {
        out.push_str("- `");
        out.push_str(meta.name);
        out.push_str("`: ");
        out.push_str(meta.description);
        append_aliases(&mut out, meta.aliases);
        out.push('\n');
        if !meta.targets.is_empty() {
            out.push_str("  targets: ");
            append_tokens(&mut out, meta.targets);
            out.push('\n');
        }
        out.push_str("  first parameter for completion: `");
        out.push_str(meta.first_parameter);
        out.push_str("`\n");
        out.push_str("  parameters:\n");
        for param in meta.parameters {
            out.push_str("  - `");
            out.push_str(param.name);
            out.push('`');
            append_aliases(&mut out, param.aliases);
            out.push_str(": ");
            out.push_str(param.description);
            append_limits(&mut out, param);
            if !param.typical.is_empty() {
                out.push_str("; typical ");
                out.push_str(param.typical);
            }
            append_notes(&mut out, param.notes);
            out.push('\n');
        }
        for note in meta.notes {
            out.push_str("  note: ");
            out.push_str(note);
            out.push('\n');
        }
    }
    out
}

fn append_aliases(out: &mut String, aliases: &[&str]) {
    if aliases.is_empty() {
        return;
    }
    out.push_str(" (aliases: ");
    for (i, alias) in aliases.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(alias);
    }
    out.push(')');
}

fn append_tokens(out: &mut String, tokens: &[CommandTokenMetadata]) {
    for (i, token) in tokens.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(token.name);
        append_aliases(out, token.aliases);
    }
}

fn append_limits(out: &mut String, param: &CommandParameterMetadata) {
    if !param.values.is_empty() {
        out.push_str("; values ");
        for (i, value) in param.values.iter().enumerate() {
            if i > 0 {
                out.push('|');
            }
            out.push_str(value);
        }
    }
    match (param.lower, param.upper) {
        (Some(lower), Some(upper)) => {
            out.push_str("; range ");
            out.push_str(&format_number(lower));
            out.push_str("..");
            out.push_str(&format_number(upper));
        }
        (Some(lower), None) => {
            out.push_str("; minimum ");
            out.push_str(&format_number(lower));
        }
        (None, Some(upper)) => {
            out.push_str("; maximum ");
            out.push_str(&format_number(upper));
        }
        (None, None) => {}
    }
    if !param.units.is_empty() {
        out.push(' ');
        out.push_str(param.units);
    }
}

fn append_notes(out: &mut String, notes: &[&str]) {
    if notes.is_empty() {
        return;
    }
    out.push_str("; ");
    out.push_str(&notes.join("; "));
}

fn format_number(n: f64) -> String {
    let mut s = format!("{n:.5}");
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

pub fn command_metadata(head: &str) -> Option<&'static CommandMetadata> {
    let head = head.to_ascii_lowercase();
    [&VCF_METADATA, &SYM_METADATA, &NAM_METADATA]
        .into_iter()
        .find(|meta| meta.name == head || meta.aliases.contains(&head.as_str()))
}

pub fn command_token_name(tokens: &[CommandTokenMetadata], token: &str) -> Option<&'static str> {
    let token = token.to_ascii_lowercase();
    tokens
        .iter()
        .find(|item| item.name == token || item.aliases.contains(&token.as_str()))
        .map(|item| item.name)
}

pub fn command_parameter(
    meta: &CommandMetadata,
    token: &str,
) -> Option<&'static CommandParameterMetadata> {
    let token = token.to_ascii_lowercase();
    meta.parameters
        .iter()
        .find(|param| param.name == token || param.aliases.contains(&token.as_str()))
}

#[derive(Clone, Copy, Debug)]
pub enum ValueChange {
    Set(f64),
    Add(f64),
    Mul(f64),
    Div(f64),
    Tick(f64),
}

impl ValueChange {
    fn parse(token: &str, usage: &str) -> Result<Self, String> {
        let t = token.trim();
        if let Some(step) = t.strip_suffix('t') {
            let n = step.parse::<f64>().map_err(|_| usage.to_string())?;
            return Ok(ValueChange::Tick(n));
        }
        if t.len() >= 2 {
            let (op, rest) = t.split_at(1);
            let n = rest.parse::<f64>().map_err(|_| usage.to_string())?;
            return match op {
                "+" => Ok(ValueChange::Add(n)),
                "-" => Ok(ValueChange::Add(-n)),
                "*" => Ok(ValueChange::Mul(n)),
                "/" => Ok(ValueChange::Div(n)),
                _ => Ok(ValueChange::Set(
                    t.parse::<f64>().map_err(|_| usage.to_string())?,
                )),
            };
        }
        Ok(ValueChange::Set(
            t.parse::<f64>().map_err(|_| usage.to_string())?,
        ))
    }

    pub fn apply(self, current: f64) -> Result<f64, String> {
        match self {
            ValueChange::Set(n) => Ok(n),
            ValueChange::Add(n) => Ok(current + n),
            ValueChange::Mul(n) => Ok(current * n),
            ValueChange::Div(n) => {
                if n == 0.0 {
                    return Err("division by zero".into());
                }
                Ok(current / n)
            }
            ValueChange::Tick(_) => Ok(current),
        }
    }
}

#[allow(dead_code)]
pub enum Cmd {
    AddPhrase {
        source: String,
        specs: Vec<JinsSpec>,
        repeat: usize,
    },
    Jump {
        to: isize,
        times: usize,
    },
    Insert {
        before: isize,
        source: String,
        specs: Vec<JinsSpec>,
        repeat: usize,
    },
    InsertBpm {
        before: isize,
        change: ValueChange,
    },
    InsertSustain {
        before: isize,
        change: ValueChange,
    },
    InsertVcf {
        before: isize,
        change: VcfChange,
    },
    InsertFx {
        before: isize,
        change: FxChange,
    },
    InsertNam {
        before: isize,
        command: NamCommand,
    },
    InsertSympathetics {
        before: isize,
        enabled: bool,
    },
    InsertSympatheticDecay {
        before: isize,
        decay: f32,
    },
    InsertSympatheticGain {
        before: isize,
        gain: f32,
    },
    InsertSympathetic {
        before: isize,
        change: SympatheticChange,
    },
    MoveUp(isize),
    MoveDown(isize),
    Edit {
        id: isize,
        source: String,
        specs: Vec<JinsSpec>,
        repeat: usize,
    },
    EditJump {
        id: isize,
        to: isize,
        times: usize,
    },
    EditBpm {
        id: isize,
        change: ValueChange,
    },
    EditSustain {
        id: isize,
        change: ValueChange,
    },
    EditVcf {
        id: isize,
        change: VcfChange,
    },
    EditFx {
        id: isize,
        change: FxChange,
    },
    EditNam {
        id: isize,
        command: NamCommand,
    },
    EditSympathetics {
        id: isize,
        enabled: bool,
    },
    EditSympatheticDecay {
        id: isize,
        decay: f32,
    },
    EditSympatheticGain {
        id: isize,
        gain: f32,
    },
    EditSympathetic {
        id: isize,
        change: SympatheticChange,
    },
    InsertJump {
        before: isize,
        to: isize,
        times: usize,
    },
    DeleteBars(Vec<isize>),
    Rotate,
    Stop,
    Sympathetics(bool),
    SympatheticDecay(f32),
    SympatheticGain(f32),
    Sympathetic(SympatheticChange),
    SetBpm(ValueChange),
    TuneTo(Pitch),
    SetSustain(ValueChange),
    SetVcf(VcfChange),
    SetFx(FxChange),
    SetNam(NamCommand),
    SetVol(f32),
    Record(usize),
    TogglePause {
        start_id: Option<isize>,
    },
    ListJins,
    AuditionJins {
        specs: Vec<JinsSpec>,
    },
    CreateJins {
        name: String,
        ratios: Vec<(u32, u32)>,
    },
    DeleteJins {
        name: String,
    },
    Save {
        path: Option<String>,
    },
    Load {
        path: String,
    },
    AskLlm {
        provider: LlmProvider,
        prompt: String,
    },
    Clear,
    Help,
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LlmProvider {
    ChatGpt,
    Claude,
}

pub fn parse(raw: &str) -> Result<Cmd, String> {
    let input = raw.trim();
    if input.is_empty() {
        return Err("empty".into());
    }

    if let Some((provider, prompt)) = parse_llm_prompt(input) {
        if prompt.is_empty() {
            return Err(
                "type a question after the colon, like chatgpt: how do i set a vcf filter?".into(),
            );
        }
        return Ok(Cmd::AskLlm { provider, prompt });
    }

    // ── Exact keyword matches ─────────────────────────────────────────────
    match input {
        "q" | "quit" => return Ok(Cmd::Quit),
        "?" | "help" => return Ok(Cmd::Help),
        "clear" => return Ok(Cmd::Clear),
        "rot" => return Ok(Cmd::Rotate),
        "stop" => return Ok(Cmd::Stop),
        "sym" | "sympathetics" | "tanbura" | "tambura" => return Ok(Cmd::Sympathetics(true)),
        "sym on" | "sympathetics on" | "tanbura on" | "tambura on" => {
            return Ok(Cmd::Sympathetics(true));
        }
        "sym off" | "sympathetics off" | "tanbura off" | "tambura off" => {
            return Ok(Cmd::Sympathetics(false));
        }
        "start" => {
            return Ok(Cmd::TogglePause {
                start_id: Some(START_REF),
            });
        }
        "pause" => return Ok(Cmd::TogglePause { start_id: None }),
        "m" => return Ok(Cmd::Record(1)),
        _ => {}
    }

    // ── m<N> / m <N> ─────────────────────────────────────────────────────
    {
        let mut it = input.split_whitespace();
        if let Some(tok) = it.next() {
            let tl = tok.to_ascii_lowercase();
            if tl.starts_with('m') && !tl.starts_with("ma") {
                let d = &tl[1..];
                let repeat: usize = if !d.is_empty() {
                    d.parse().unwrap_or(1)
                } else {
                    it.next().and_then(|s| s.parse().ok()).unwrap_or(1)
                };
                return Ok(Cmd::Record(repeat.max(1)));
            }
        }
    }

    let first = input.split_whitespace().next().unwrap_or("");
    let alpha: String = first
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let digits: String = first
        .chars()
        .skip_while(|c| c.is_ascii_alphabetic())
        .collect();
    let al = alpha.to_ascii_lowercase();

    if matches!(al.as_str(), "sym" | "sympathetics" | "tanbura" | "tambura") {
        let change = parse_sympathetic_change(input)?;
        return if change.enabled == Some(true)
            && change.target.is_none()
            && change.decay.is_none()
            && change.gain.is_none()
            && change.amount.is_none()
            && change.mic.is_none()
            && change.kanun.is_none()
            && change.bass.is_none()
            && change.drums.is_none()
        {
            Ok(Cmd::Sympathetics(true))
        } else if change.enabled == Some(false)
            && change.target.is_none()
            && change.decay.is_none()
            && change.gain.is_none()
            && change.amount.is_none()
            && change.mic.is_none()
            && change.kanun.is_none()
            && change.bass.is_none()
            && change.drums.is_none()
        {
            Ok(Cmd::Sympathetics(false))
        } else if let Some(decay) = change.decay {
            if change.target.is_none()
                && change.enabled.is_none()
                && change.gain.is_none()
                && change.amount.is_none()
                && change.mic.is_none()
                && change.kanun.is_none()
                && change.bass.is_none()
                && change.drums.is_none()
            {
                Ok(Cmd::SympatheticDecay(decay))
            } else {
                Ok(Cmd::Sympathetic(change))
            }
        } else if let Some(gain) = change.gain {
            if change.target.is_none()
                && change.enabled.is_none()
                && change.decay.is_none()
                && change.amount.is_none()
                && change.mic.is_none()
                && change.kanun.is_none()
                && change.bass.is_none()
                && change.drums.is_none()
            {
                Ok(Cmd::SympatheticGain(gain))
            } else {
                Ok(Cmd::Sympathetic(change))
            }
        } else {
            Ok(Cmd::Sympathetic(change))
        };
    }

    // ── SOUND TOGGLE / TRANSPORT: z [phrase-id] ──────────────────────────
    if al == "z" {
        let start_id: Option<isize> = if !digits.is_empty() {
            Some(parse_id_ref(&digits, "usage: z [phrase-id]")?)
        } else {
            input
                .split_whitespace()
                .nth(1)
                .map(|value| parse_id_ref(value, "usage: z [phrase-id]"))
                .transpose()?
        };
        return Ok(Cmd::TogglePause { start_id });
    }

    // ── JUMP: j <pos> [<times>] ───────────────────────────────────────────
    if al == "j" {
        let to: isize = if !digits.is_empty() {
            parse_id_ref(&digits, "usage: j <pos> [times]")?
        } else {
            input
                .split_whitespace()
                .nth(1)
                .map(|value| parse_id_ref(value, "usage: j <pos> [times]"))
                .transpose()?
                .ok_or("usage: j <pos> [times]")?
        };
        let times_idx = if digits.is_empty() { 2 } else { 1 };
        let times: usize = input
            .split_whitespace()
            .nth(times_idx)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        return Ok(Cmd::Jump {
            to,
            times: times.max(1),
        });
    }

    // ── EDIT: edit <id> <cmd> ─────────────────────────────────────────────
    if al == "edit" {
        let mut toks = input.splitn(3, char::is_whitespace);
        toks.next(); // skip "edit"
        let id: isize = toks
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or("usage: edit <id> <phrase|j target [times]|bpm n|s n>")?;
        let rest = toks.next().unwrap_or("").trim();
        if rest.is_empty() {
            return Err("usage: edit <id> <phrase|j target [times]|bpm n|s n>".into());
        }

        return match parse(rest)? {
            Cmd::AddPhrase {
                source,
                specs,
                repeat,
            } => Ok(Cmd::Edit {
                id,
                source,
                specs,
                repeat,
            }),
            Cmd::Jump { to, times } => Ok(Cmd::EditJump { id, to, times }),
            Cmd::SetBpm(change) => Ok(Cmd::EditBpm { id, change }),
            Cmd::SetSustain(change) => Ok(Cmd::EditSustain { id, change }),
            Cmd::SetVcf(change) => Ok(Cmd::EditVcf { id, change }),
            Cmd::SetFx(change) => Ok(Cmd::EditFx { id, change }),
            Cmd::SetNam(command) => Ok(Cmd::EditNam { id, command }),
            Cmd::Sympathetics(enabled) => Ok(Cmd::EditSympathetics { id, enabled }),
            Cmd::SympatheticDecay(decay) => Ok(Cmd::EditSympatheticDecay { id, decay }),
            Cmd::SympatheticGain(gain) => Ok(Cmd::EditSympatheticGain { id, gain }),
            Cmd::Sympathetic(change) => Ok(Cmd::EditSympathetic { id, change }),
            _ => Err("unsupported command after edit".into()),
        };
    }

    // ── REORDER: up/down <id> ─────────────────────────────────────────────
    if al == "up" {
        let id = input
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<isize>().ok())
            .ok_or("usage: up <id>")?;
        return Ok(Cmd::MoveUp(id));
    }
    if al == "down" {
        let id = input
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<isize>().ok())
            .ok_or("usage: down <id>")?;
        return Ok(Cmd::MoveDown(id));
    }

    // ── INSERT: i<pos> <cmd> ──────────────────────────────────────────────
    if al == "i" {
        let before: isize;
        let rest: &str;
        if !digits.is_empty() {
            before = parse_id_ref(&digits, "usage: i<pos> <phrase|j target times|bpm n|s n>")?;
            rest = input[first.len()..].trim();
        } else {
            let mut toks = input.splitn(3, char::is_whitespace);
            toks.next();
            before = toks
                .next()
                .and_then(|s| s.parse::<isize>().ok())
                .ok_or("usage: i <pos> <phrase|j target times|bpm n|s n>")?;
            rest = toks.next().unwrap_or("").trim();
        }
        if rest.is_empty() {
            return Err("usage: i <pos> <phrase|j target times|bpm n|s n>".into());
        }

        return match parse(rest)? {
            Cmd::AddPhrase {
                source,
                specs,
                repeat,
            } => Ok(Cmd::Insert {
                before,
                source,
                specs,
                repeat,
            }),
            Cmd::Jump { to, times } => Ok(Cmd::InsertJump { before, to, times }),
            Cmd::SetBpm(change) => Ok(Cmd::InsertBpm { before, change }),
            Cmd::SetSustain(change) => Ok(Cmd::InsertSustain { before, change }),
            Cmd::SetVcf(change) => Ok(Cmd::InsertVcf { before, change }),
            Cmd::SetFx(change) => Ok(Cmd::InsertFx { before, change }),
            Cmd::SetNam(command) => Ok(Cmd::InsertNam { before, command }),
            Cmd::Sympathetics(enabled) => Ok(Cmd::InsertSympathetics { before, enabled }),
            Cmd::SympatheticDecay(decay) => Ok(Cmd::InsertSympatheticDecay { before, decay }),
            Cmd::SympatheticGain(gain) => Ok(Cmd::InsertSympatheticGain { before, gain }),
            Cmd::Sympathetic(change) => Ok(Cmd::InsertSympathetic { before, change }),
            _ => Err("unsupported command after insert".into()),
        };
    }

    // ── BPM ───────────────────────────────────────────────────────────────
    if al == "bpm" {
        let tok = input
            .split_whitespace()
            .nth(1)
            .ok_or("usage: bpm <tempo|*k|/k|+n|-n>")?;
        let change = ValueChange::parse(tok, "usage: bpm <tempo|*k|/k|+n|-n>")?;
        return Ok(Cmd::SetBpm(change));
    }

    // ── TUNE REFERENCE ───────────────────────────────────────────────────
    if al == "tuneto" {
        let tok = input
            .split_whitespace()
            .nth(1)
            .ok_or("type a pitch after tuneto, like tuneto c or tuneto b-")?;
        let pitch = Pitch::parse(tok).ok_or_else(|| {
            format!("unknown pitch '{tok}'; type a pitch like c, d, a, or b- after tuneto")
        })?;
        return Ok(Cmd::TuneTo(pitch));
    }

    // ── SUSTAIN ───────────────────────────────────────────────────────────
    if (al == "s" || al == "sus") && digits.is_empty() {
        let tok = input
            .split_whitespace()
            .nth(1)
            .ok_or("usage: s <secs|*k|/k|+n|-n>")?;
        let change = ValueChange::parse(tok, "usage: s <secs|*k|/k|+n|-n>")?;
        return Ok(Cmd::SetSustain(change));
    }

    // ── VCF ──────────────────────────────────────────────────────────────
    if matches!(
        al.as_str(),
        "vcf" | "filter" | "filt" | "cut" | "cutoff" | "res" | "q" | "drive" | "drv"
    ) && digits.is_empty()
    {
        return Ok(Cmd::SetVcf(parse_vcf_change(input)?));
    }

    // ── FX ───────────────────────────────────────────────────────────────
    if matches!(al.as_str(), "fx" | "reverb" | "rev" | "delay" | "pingpong") && digits.is_empty() {
        return Ok(Cmd::SetFx(parse_fx_change(input)?));
    }

    // ── NAM input amp model ──────────────────────────────────────────────
    if al == "nam" {
        let rest = input
            .split_once(char::is_whitespace)
            .map(|(_, rest)| rest.trim())
            .filter(|s| !s.is_empty())
            .ok_or("usage: nam <cached-name|FILENAME.nam|URL> | nam import <FILENAME.nam|URL> [as name] | nam ls | nam off | nam gain <0..8>")?;
        let mut toks = rest.split_whitespace();
        match toks.next().unwrap_or("").to_ascii_lowercase().as_str() {
            "login" => return Ok(Cmd::SetNam(NamCommand::Login)),
            "logout" => return Ok(Cmd::SetNam(NamCommand::Logout)),
            "off" => return Ok(Cmd::SetNam(NamCommand::Off)),
            "ls" | "list" => return Ok(Cmd::SetNam(NamCommand::List)),
            "search" | "find" => {
                let query = rest
                    .split_once(char::is_whitespace)
                    .map(|(_, value)| value.trim())
                    .filter(|value| !value.is_empty())
                    .ok_or(
                        "usage: nam search <amp, artist, or tone>; describe the capture to find",
                    )?;
                return Ok(Cmd::SetNam(NamCommand::Search {
                    query: query.to_string(),
                }));
            }
            "import" | "pull" => {
                let import_rest = rest
                    .split_once(char::is_whitespace)
                    .map(|(_, value)| value.trim())
                    .filter(|s| !s.is_empty())
                    .ok_or("usage: nam import <FILENAME.nam|URL> [as name]; type a real .nam file name or URL after nam import")?;
                let (path, name) = if let Some((path, name)) = import_rest.rsplit_once(" as ") {
                    let name = name.trim();
                    if name.is_empty() {
                        return Err(
                            "usage: nam import <FILENAME.nam|URL> as name; type a cache name after as"
                                .into(),
                        );
                    }
                    (path.trim().to_string(), Some(name.to_string()))
                } else {
                    (import_rest.to_string(), None)
                };
                if path.is_empty() {
                    return Err(
                        "usage: nam import <FILENAME.nam|URL> [as name]; type a real .nam file name or URL after nam import"
                            .into(),
                    );
                }
                return Ok(Cmd::SetNam(NamCommand::Import { path, name }));
            }
            "pin" | "require" => {
                let pin_rest = rest
                    .split_once(char::is_whitespace)
                    .map(|(_, value)| value.trim())
                    .filter(|value| !value.is_empty())
                    .ok_or("usage: nam pin <direct-.nam-URL> as <name>")?;
                let (url, name) = pin_rest
                    .rsplit_once(" as ")
                    .ok_or("usage: nam pin <direct-.nam-URL> as <name>")?;
                if !url.starts_with("https://") && !url.starts_with("http://") {
                    return Err("nam pin requires a direct http(s) model URL".into());
                }
                let name = name.trim();
                if name.is_empty() {
                    return Err("nam pin needs a stable name after `as`".into());
                }
                return Ok(Cmd::SetNam(NamCommand::Pin {
                    url: url.trim().to_string(),
                    name: name.to_string(),
                }));
            }
            "tone3000" | "t3k" => {
                let values = rest.split_whitespace().collect::<Vec<_>>();
                if values.len() != 4 || !values[2].eq_ignore_ascii_case("as") {
                    return Err("usage: nam tone3000 <tone-id> as <name>".into());
                }
                let tone_id = values[1]
                    .parse::<u64>()
                    .map_err(|_| "TONE3000 tone ID must be a number")?;
                return Ok(Cmd::SetNam(NamCommand::Tone3000 {
                    tone_id,
                    name: values[3].to_string(),
                }));
            }
            "gain" | "drive" => {
                let value = toks
                    .next()
                    .ok_or("usage: nam gain <0..8>; type a number after nam gain")?;
                let gain = value
                    .parse::<f32>()
                    .map_err(|_| format!("bad NAM gain '{value}'; type a number from 0 to 8"))?;
                if !(0.0..=8.0).contains(&gain) {
                    return Err(format!(
                        "NAM gain {gain} out of range 0..8; use nam gain <0..8>"
                    ));
                }
                return Ok(Cmd::SetNam(NamCommand::Gain(gain)));
            }
            "input" | "in" => {
                let route = match toks.next().unwrap_or("").to_ascii_lowercase().as_str() {
                    "left" | "l" | "1" => NamInput::Left,
                    "right" | "r" | "2" => NamInput::Right,
                    "stereo" | "both" | "lr" => NamInput::Stereo,
                    _ => return Err("usage: nam input <left|right|stereo>".into()),
                };
                return Ok(Cmd::SetNam(NamCommand::Input(route)));
            }
            "latency" => {
                let route = match toks.next().unwrap_or("left").to_ascii_lowercase().as_str() {
                    "left" | "l" | "1" => NamInput::Left,
                    "right" | "r" | "2" => NamInput::Right,
                    _ => return Err("usage: nam latency <left|right>".into()),
                };
                return Ok(Cmd::SetNam(NamCommand::Latency(route)));
            }
            "load" => {
                let path = rest
                    .split_once(char::is_whitespace)
                    .map(|(_, path)| path.trim())
                    .filter(|s| !s.is_empty())
                    .ok_or("usage: nam load <model.nam|cached-name|URL>; type a .nam path, cached name, or URL after nam load")?;
                return Ok(Cmd::SetNam(NamCommand::Load {
                    path: path.to_string(),
                }));
            }
            _ => {
                return Ok(Cmd::SetNam(NamCommand::Load {
                    path: rest.to_string(),
                }));
            }
        }
    }

    // ── VOL ───────────────────────────────────────────────────────────────
    if al == "vol" {
        let n: f32 = if !digits.is_empty() {
            digits.parse().unwrap_or(1.0)
        } else {
            input
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0)
        };
        if !(0.0..=2.0).contains(&n) {
            return Err(format!("vol {n} out of range 0–2"));
        }
        return Ok(Cmd::SetVol(n));
    }

    // ── DELETE: x<N> [N …] ───────────────────────────────────────────────
    if al == "x" {
        let mut ids: Vec<isize> = Vec::new();
        if !digits.is_empty() {
            ids.push(parse_id_ref(&digits, "usage: x<N> [N …]")?);
        }
        let mut toks = input.split_whitespace();
        toks.next();
        for tok in toks {
            ids.push(tok.parse().map_err(|_| format!("bad id '{tok}'"))?);
        }
        if ids.is_empty() {
            return Err("usage: x<N>".into());
        }
        return Ok(Cmd::DeleteBars(ids));
    }

    // ── LS: list all jins ─────────────────────────────────────────────────
    if input == "ls" {
        return Ok(Cmd::ListJins);
    }

    // ── AUDITION: audition <phrase-spec> ─────────────────────────────────
    if al == "audition" {
        let rest = input
            .split_once(char::is_whitespace)
            .map(|(_, rest)| rest)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("usage: audition <Name> | audition <root> <Name> [, <root> <Name> ...]")?;

        let specs: Result<Vec<JinsSpec>, String> =
            if Pitch::parse(rest.split_whitespace().next().unwrap_or("")).is_some() {
                rest.split(',').map(|p| parse_jins_spec(p.trim())).collect()
            } else {
                let maqam = Maqam::parse(rest).ok_or_else(|| format!("unknown maqam '{rest}'"))?;
                Ok(vec![JinsSpec {
                    src: format!("d {}", maqam.name()),
                    root: Pitch {
                        letter: 'd',
                        accidental: 0,
                        octave: 4,
                    },
                    maqam,
                    groups: None,
                }])
            };
        return Ok(Cmd::AuditionJins { specs: specs? });
    }

    // ── CREATE: create <Name> <p/q> <p/q> … ──────────────────────────────
    if al == "create" {
        let mut toks = input.split_whitespace();
        toks.next(); // skip "create"
        let name = toks
            .next()
            .ok_or("usage: create <Name> <ratios…>")?
            .to_string();
        let ratios: Result<Vec<(u32, u32)>, String> = toks
            .map(|t| parse_ratio(t).ok_or_else(|| format!("bad ratio '{t}'")))
            .collect();
        let ratios = ratios?;
        if ratios.is_empty() {
            return Err("need at least one ratio".into());
        }
        return Ok(Cmd::CreateJins { name, ratios });
    }

    // ── DELETE: delete <Name> ─────────────────────────────────────────────
    if al == "delete" {
        let name = input
            .split_whitespace()
            .nth(1)
            .ok_or("usage: delete <Name>")?
            .to_string();
        return Ok(Cmd::DeleteJins { name });
    }

    // ── SAVE / LOAD ───────────────────────────────────────────────────────
    if al == "save" {
        let path = input
            .split_once(char::is_whitespace)
            .map(|(_, path)| path)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        return Ok(Cmd::Save { path });
    }
    if al == "load" {
        let path = input
            .split_once(char::is_whitespace)
            .map(|(_, path)| path)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or("usage: load <path>")?
            .to_string();
        return Ok(Cmd::Load { path });
    }

    // ── ADD PHRASE ────────────────────────────────────────────────────────
    let (phrase_part, repeat) = strip_repeat(input);
    if phrase_part.is_empty() {
        return Err("empty phrase".into());
    }
    let specs: Result<Vec<JinsSpec>, String> = phrase_part
        .split(',')
        .map(|p| parse_jins_spec(p.trim()))
        .collect();
    Ok(Cmd::AddPhrase {
        source: input.trim().to_string(),
        specs: specs?,
        repeat,
    })
}

fn parse_llm_prompt(input: &str) -> Option<(LlmProvider, String)> {
    let (head, rest) = input.split_once(':')?;
    let provider = match head.trim().to_ascii_lowercase().as_str() {
        "chatgpt" | "gpt" | "openai" => LlmProvider::ChatGpt,
        "claude" | "anthropic" => LlmProvider::Claude,
        _ => return None,
    };
    Some((provider, rest.trim().to_string()))
}

fn parse_id_ref(token: &str, usage: &str) -> Result<isize, String> {
    if token.eq_ignore_ascii_case("start") {
        return Ok(START_REF);
    }
    token.parse::<isize>().map_err(|_| usage.to_string())
}

fn strip_repeat(input: &str) -> (&str, usize) {
    let toks: Vec<&str> = input.split_whitespace().collect();
    if toks.is_empty() {
        return (input, 1);
    }
    let last = *toks.last().unwrap();
    let la = last.to_ascii_lowercase();
    let la_a: String = la.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let la_d: String = la.chars().skip_while(|c| c.is_ascii_alphabetic()).collect();
    let (is_r, num_s): (bool, &str) = if la_a == "r" && !la_d.is_empty() {
        (true, &la_d)
    } else if la_a.is_empty() && !la_d.is_empty() {
        (false, &la_d)
    } else {
        return (input, 1);
    };
    let Ok(n) = num_s.parse::<usize>() else {
        return (input, 1);
    };
    if !is_r && n > 20 {
        return (input, 1);
    }
    let trimmed = input.trim_end();
    let pos = trimmed.rfind(last).unwrap_or(trimmed.len());
    let remaining = trimmed[..pos].trim_end();
    (remaining, n.max(1))
}

fn parse_jins_spec(part: &str) -> Result<JinsSpec, String> {
    let mut toks = part.split_whitespace();
    let root_tok = toks.next().ok_or("missing pitch")?;
    let root = Pitch::parse(root_tok).ok_or_else(|| format!("unknown pitch '{root_tok}'"))?;
    let maq_tok = toks.next().ok_or("missing maqam")?;
    let maqam = Maqam::parse(maq_tok).ok_or_else(|| {
        format!("unknown maqam '{maq_tok}'  (nah bay hij rast kurd saba ajam major minor modes)")
    })?;
    let groups = match toks.next() {
        None => None,
        Some(tok) => {
            let g: Vec<u8> = tok
                .chars()
                .filter(|c| c.is_ascii_digit() && *c != '0')
                .map(|c| c as u8 - b'0')
                .collect();
            if g.is_empty() {
                return Err(format!("rhythm '{tok}' must be non-zero digits"));
            }
            Some(g)
        }
    };
    Ok(JinsSpec {
        src: part.trim().to_string(),
        root,
        maqam,
        groups,
    })
}

fn parse_ratio(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.splitn(2, '/');
    let p = parts.next()?.parse::<u32>().ok()?;
    let q = parts
        .next()
        .and_then(|q| q.parse::<u32>().ok())
        .unwrap_or(1);
    if q == 0 {
        return None;
    }
    Some((p, q))
}

fn parse_sympathetic_change(input: &str) -> Result<SympatheticChange, String> {
    let usage = "usage: sym [all|mic|kanun|bass|drums] [on|off] [decay <0.9..0.99999>] [drive <0..512>] [up|down <interval>] [interval <ratio>] [amount <0..512>] [mic <0..512>] [kanun <0..512>] [bass <0..512>] [drums <0..512>]";
    let mut tokens = input.split_whitespace();
    tokens.next();
    let mut rest: Vec<&str> = tokens.collect();
    if rest.is_empty() {
        return Ok(SympatheticChange {
            enabled: Some(true),
            ..SympatheticChange::default()
        });
    }

    let mut change = SympatheticChange::default();
    if rest.len() >= 2 && is_sym_setting_token(rest[1]) {
        if let Some(target) = SympatheticTarget::parse(rest[0]) {
            change.target = Some(target);
            rest.remove(0);
        }
    }
    let mut i = 0usize;
    while i < rest.len() {
        let key = rest[i].to_ascii_lowercase();
        match key.as_str() {
            "on" => {
                change.enabled = Some(true);
                i += 1;
            }
            "off" => {
                change.enabled = Some(false);
                i += 1;
            }
            "decay" => {
                i += 1;
                let decay = rest
                    .get(i)
                    .ok_or(usage)?
                    .parse::<f32>()
                    .map_err(|_| usage.to_string())?;
                if !(0.9..=0.999_99).contains(&decay) {
                    return Err("sym decay out of range 0.9..0.99999".into());
                }
                change.decay = Some(decay);
                i += 1;
            }
            "gain" | "drive" => {
                i += 1;
                change.gain = Some(parse_sym_gain(rest.get(i).copied(), usage)?);
                i += 1;
            }
            "amount" | "amt" | "level" | "send" => {
                i += 1;
                change.amount = Some(parse_sym_gain(rest.get(i).copied(), usage)?);
                i += 1;
            }
            "up" => {
                i += 1;
                let ratio = parse_sym_interval_ratio(rest.get(i).copied(), usage)?;
                change.interval_ratio = Some(ratio);
                i += 1;
            }
            "down" => {
                i += 1;
                let ratio = parse_sym_interval_ratio(rest.get(i).copied(), usage)?;
                change.interval_ratio = Some(1.0 / ratio);
                i += 1;
            }
            "interval" | "transpose" => {
                i += 1;
                change.interval_ratio =
                    Some(parse_sym_interval_ratio(rest.get(i).copied(), usage)?);
                i += 1;
            }
            "harmony" | "harm" | "chord" => {
                i += 1;
                let (harmony, consumed) = parse_sym_harmony(&rest[i..], usage)?;
                change.harmony = Some(harmony);
                i += consumed;
            }
            "mic" | "input" | "live" => {
                i += 1;
                change.mic = Some(parse_sym_gain(rest.get(i).copied(), usage)?);
                i += 1;
            }
            "kanun" | "qanun" | "melody" => {
                i += 1;
                change.kanun = Some(parse_sym_gain(rest.get(i).copied(), usage)?);
                i += 1;
            }
            "bass" | "sub" | "subbass" => {
                i += 1;
                change.bass = Some(parse_sym_gain(rest.get(i).copied(), usage)?);
                i += 1;
            }
            "drums" | "drum" | "kick" | "kicks" => {
                i += 1;
                change.drums = Some(parse_sym_gain(rest.get(i).copied(), usage)?);
                i += 1;
            }
            "sym" | "sympathetics" | "tanbura" | "tambura" => {
                i += 1;
            }
            _ => return Err(usage.into()),
        }
    }
    Ok(change)
}

fn is_sym_setting_token(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "on" | "off"
            | "decay"
            | "gain"
            | "drive"
            | "amount"
            | "amt"
            | "level"
            | "send"
            | "up"
            | "down"
            | "interval"
            | "transpose"
            | "harmony"
            | "harm"
            | "chord"
    )
}

fn parse_sym_harmony(tokens: &[&str], usage: &str) -> Result<(SympatheticHarmony, usize), String> {
    let mut harmony = SympatheticHarmony::default();
    let mut i = 0usize;
    while i < tokens.len() {
        let token = tokens[i].to_ascii_lowercase();
        if is_sym_setting_token(&token) && harmony.len > 0 {
            break;
        }
        let ratio = parse_sym_interval_ratio(Some(tokens[i]), usage)?;
        i += 1;
        let mut weight = 1.0f32;
        if let Some(next) = tokens.get(i) {
            if !is_sym_setting_token(next) {
                if let Ok(parsed) = next.parse::<f32>() {
                    if !(0.0..=512.0).contains(&parsed) {
                        return Err("sym harmony weight out of range 0..512".into());
                    }
                    weight = parsed;
                    i += 1;
                }
            }
        }
        harmony.push(SympatheticHarmonyComponent { ratio, weight })?;
    }
    if harmony.len == 0 {
        return Err(usage.to_string());
    }
    Ok((harmony, i))
}

fn parse_sym_interval_ratio(value: Option<&str>, usage: &str) -> Result<f64, String> {
    let token = value.ok_or(usage)?.to_ascii_lowercase();
    if let Some((num, den)) = token.split_once('/') {
        let num = num.parse::<f64>().map_err(|_| usage.to_string())?;
        let den = den.parse::<f64>().map_err(|_| usage.to_string())?;
        if den == 0.0 {
            return Err("sym interval ratio cannot divide by zero".into());
        }
        return validate_sym_interval_ratio(num / den);
    }
    if let Ok(ratio) = token.parse::<f64>() {
        return validate_sym_interval_ratio(ratio);
    }
    let normalized = token.replace(['-', '_', ' '], "");
    let ratio = match normalized.as_str() {
        "unison" | "root" | "fundamental" | "f0" => 1.0,
        "minorsecond" | "minor2" | "m2" | "b2" | "semitone" | "halfstep" => 16.0 / 15.0,
        "second" | "majorsecond" | "major2" | "wholestep" | "whole" => 9.0 / 8.0,
        "third" | "minorthird" | "minor3" | "min3" | "m3" | "b3" => 6.0 / 5.0,
        "majorthird" | "major3" | "maj3" => 5.0 / 4.0,
        "fourth" | "perfectfourth" | "p4" => 4.0 / 3.0,
        "tritone" | "flatfifth" | "sharpfourth" | "b5" | "#4" => 45.0 / 32.0,
        "fifth" | "perfectfifth" | "p5" => 3.0 / 2.0,
        "sixth" | "minorsixth" | "minor6" | "min6" | "m6" | "b6" => 8.0 / 5.0,
        "majorsixth" | "major6" | "maj6" => 5.0 / 3.0,
        "minorseventh" | "m7" | "b7" => 9.0 / 5.0,
        "seventh" | "majorseventh" | "major7" => 15.0 / 8.0,
        "octave" | "oct" | "8ve" => 2.0,
        _ => return Err(usage.to_string()),
    };
    validate_sym_interval_ratio(ratio)
}

fn validate_sym_interval_ratio(ratio: f64) -> Result<f64, String> {
    if !(0.25..=4.0).contains(&ratio) {
        return Err("sym interval ratio out of range 1/4..4/1".into());
    }
    Ok(ratio)
}

fn parse_sym_gain(value: Option<&str>, usage: &str) -> Result<f32, String> {
    let gain = value
        .ok_or(usage)?
        .parse::<f32>()
        .map_err(|_| usage.to_string())?;
    if !(0.0..=512.0).contains(&gain) {
        return Err("sym gain out of range 0..512".into());
    }
    Ok(gain)
}

fn parse_vcf_change(input: &str) -> Result<VcfChange, String> {
    let usage = "usage: vcf [all|mic|bass|kanun|drums|kick|sym] <cutoff> [res] [drive] | vcf [target] off | vcf <target> cut=<hz|+n|-n|+nt> res=<0..1|+n|-n|+nt> drive=<n|+n|-n|+nt> wave=<sin|tri|squ|saw|mic> | cut <hz> | res <0..1> | drive <n>";
    let mut toks = input.split_whitespace();
    let head = toks.next().unwrap_or("").to_ascii_lowercase();
    let mut out = VcfChange::default();

    if matches!(head.as_str(), "cut" | "cutoff" | "filt" | "filter") {
        let tok = toks.next().ok_or(usage)?;
        out.enabled = Some(true);
        out.cutoff_hz = Some(ValueChange::parse(tok, usage)?);
        return Ok(out);
    }
    if matches!(head.as_str(), "res" | "q") {
        let tok = toks.next().ok_or(usage)?;
        out.enabled = Some(true);
        out.resonance = Some(ValueChange::parse(tok, usage)?);
        return Ok(out);
    }
    if matches!(head.as_str(), "drive" | "drv") {
        let tok = toks.next().ok_or(usage)?;
        out.enabled = Some(true);
        out.drive = Some(ValueChange::parse(tok, usage)?);
        return Ok(out);
    }

    let mut rest: Vec<&str> = toks.collect();
    if rest.is_empty() {
        return Ok(VcfChange {
            enabled: Some(true),
            ..VcfChange::default()
        });
    }
    if let Some(target) = rest.first().and_then(|tok| VcfTarget::parse(tok)) {
        out.target = Some(target);
        rest.remove(0);
        if rest.is_empty() {
            return Err(usage.into());
        }
    }
    if rest.len() == 1 && rest[0].eq_ignore_ascii_case("off") {
        out.enabled = Some(false);
        if out.target.is_none() {
            out.target = Some(VcfTarget::All);
        }
        return Ok(out);
    }

    let has_named = rest.iter().any(|tok| {
        tok.contains('=')
            || matches!(
                tok.to_ascii_lowercase().as_str(),
                "cut"
                    | "cutoff"
                    | "freq"
                    | "frequency"
                    | "res"
                    | "q"
                    | "reso"
                    | "resonance"
                    | "drive"
                    | "drv"
                    | "wave"
                    | "wav"
                    | "shape"
            )
    });

    if !has_named {
        out.enabled = Some(true);
        if rest.is_empty() {
            return Err(usage.into());
        }
        out.cutoff_hz = Some(ValueChange::parse(rest[0], usage)?);
        if let Some(tok) = rest.get(1) {
            out.resonance = Some(ValueChange::parse(tok, usage)?);
        }
        if let Some(tok) = rest.get(2) {
            out.drive = Some(ValueChange::parse(tok, usage)?);
        }
        if rest.len() > 3 {
            return Err(usage.into());
        }
        return Ok(out);
    }

    let mut positional = Vec::new();
    while let Some(tok) = rest.first() {
        let lower = tok.to_ascii_lowercase();
        let is_named_token = tok.contains('=')
            || matches!(
                lower.as_str(),
                "cut"
                    | "cutoff"
                    | "freq"
                    | "frequency"
                    | "res"
                    | "q"
                    | "reso"
                    | "resonance"
                    | "drive"
                    | "drv"
                    | "wave"
                    | "wav"
                    | "shape"
            );
        if is_named_token {
            break;
        }
        positional.push(rest.remove(0));
        if positional.len() > 3 {
            return Err(usage.into());
        }
    }
    if !positional.is_empty() {
        out.enabled = Some(true);
        out.cutoff_hz = Some(ValueChange::parse(positional[0], usage)?);
        if let Some(tok) = positional.get(1) {
            out.resonance = Some(ValueChange::parse(tok, usage)?);
        }
        if let Some(tok) = positional.get(2) {
            out.drive = Some(ValueChange::parse(tok, usage)?);
        }
    }

    let mut i = 0usize;
    while i < rest.len() {
        out.enabled = Some(true);
        let tok = rest[i];
        let (name, value) = if let Some((name, value)) = tok.split_once('=') {
            (name.to_ascii_lowercase(), value)
        } else {
            let name = tok.to_ascii_lowercase();
            i += 1;
            let Some(value) = rest.get(i) else {
                if matches!(name.as_str(), "drive" | "drv") {
                    break;
                }
                return Err(usage.into());
            };
            if matches!(name.as_str(), "drive" | "drv") && is_vcf_named_token(value) {
                continue;
            }
            (name, *value)
        };
        match name.as_str() {
            "cut" | "cutoff" | "freq" | "frequency" => {
                out.cutoff_hz = Some(ValueChange::parse(value, usage)?)
            }
            "res" | "q" | "reso" | "resonance" => {
                out.resonance = Some(ValueChange::parse(value, usage)?)
            }
            "drive" | "drv" => out.drive = Some(ValueChange::parse(value, usage)?),
            "wave" | "wav" | "shape" => {
                out.wave = Some(VcoWave::parse(value).ok_or(usage)?);
            }
            _ => return Err(format!("unknown vcf parameter '{name}'")),
        }
        i += 1;
    }

    Ok(out)
}

fn is_vcf_named_token(token: &str) -> bool {
    token.contains('=')
        || matches!(
            token.to_ascii_lowercase().as_str(),
            "cut"
                | "cutoff"
                | "freq"
                | "frequency"
                | "res"
                | "q"
                | "reso"
                | "resonance"
                | "drive"
                | "drv"
                | "wave"
                | "wav"
                | "shape"
        )
}

pub fn apply_vcf_change(current: VcfBank, change: VcfChange) -> Result<VcfSettings, String> {
    let target = change.target.unwrap_or(current.focus);
    let current = current.get(target);
    let mut cutoff_step_per_tick = current.cutoff_step_per_tick;
    let cutoff_hz = match change.cutoff_hz {
        Some(ValueChange::Tick(step)) => {
            cutoff_step_per_tick = step as f32;
            current.cutoff_hz
        }
        Some(ValueChange::Add(0.0)) => {
            cutoff_step_per_tick = 0.0;
            current.cutoff_hz
        }
        Some(change) => change.apply(current.cutoff_hz as f64)? as f32,
        None => current.cutoff_hz,
    };
    if !(10.0..=22_000.0).contains(&cutoff_hz) {
        return Err(format!("vcf cutoff {cutoff_hz} Hz out of range 10..22000"));
    }

    let mut resonance_step_per_tick = current.resonance_step_per_tick;
    let resonance = match change.resonance {
        Some(ValueChange::Tick(step)) => {
            resonance_step_per_tick = step as f32;
            current.resonance
        }
        Some(ValueChange::Add(0.0)) => {
            resonance_step_per_tick = 0.0;
            current.resonance
        }
        Some(change) => change.apply(current.resonance as f64)? as f32,
        None => current.resonance,
    };
    if !(0.0..=0.98).contains(&resonance) {
        return Err(format!("vcf resonance {resonance} out of range 0..0.98"));
    }

    let mut drive_step_per_tick = current.drive_step_per_tick;
    let drive = match change.drive {
        Some(ValueChange::Tick(step)) => {
            drive_step_per_tick = step as f32;
            current.drive
        }
        Some(ValueChange::Add(0.0)) => {
            drive_step_per_tick = 0.0;
            current.drive
        }
        Some(change) => change.apply(current.drive as f64)? as f32,
        None => current.drive,
    };
    if !(0.1..=12.0).contains(&drive) {
        return Err(format!("vcf drive {drive} out of range 0.1..12"));
    }

    Ok(VcfSettings {
        enabled: change.enabled.unwrap_or(current.enabled),
        target,
        cutoff_hz,
        resonance,
        drive,
        cutoff_step_per_tick,
        resonance_step_per_tick,
        drive_step_per_tick,
        wave: if target == VcfTarget::All {
            current.wave
        } else if target == VcfTarget::Mic {
            VcoWave::Mic
        } else {
            change.wave.unwrap_or(current.wave)
        },
    })
}

fn parse_fx_change(input: &str) -> Result<FxChange, String> {
    let usage = "usage: reverb mix=<0..1> decay=<0..0.98> | delay time=<secs> feedback=<0..0.95> mix=<0..1> | pingpong ... | reverb off | delay off | fx off";
    let mut toks = input.split_whitespace();
    let head = toks.next().unwrap_or("").to_ascii_lowercase();
    let rest: Vec<&str> = toks.collect();
    let mut out = FxChange::default();

    if head == "fx" {
        if rest.len() == 1 && rest[0].eq_ignore_ascii_case("off") {
            out.reverb_enabled = Some(false);
            out.delay_enabled = Some(false);
            return Ok(out);
        }
        return Err(usage.into());
    }

    let is_reverb = matches!(head.as_str(), "reverb" | "rev");
    let is_delay = matches!(head.as_str(), "delay" | "pingpong");
    if !is_reverb && !is_delay {
        return Err(usage.into());
    }
    if rest.len() == 1 && rest[0].eq_ignore_ascii_case("off") {
        if is_reverb {
            out.reverb_enabled = Some(false);
        } else {
            out.delay_enabled = Some(false);
        }
        return Ok(out);
    }
    if rest.is_empty() || (rest.len() == 1 && rest[0].eq_ignore_ascii_case("on")) {
        if is_reverb {
            out.reverb_enabled = Some(true);
        } else {
            out.delay_enabled = Some(true);
        }
        return Ok(out);
    }

    if is_reverb {
        out.reverb_enabled = Some(true);
    } else {
        out.delay_enabled = Some(true);
    }

    let mut i = 0usize;
    while i < rest.len() {
        let tok = rest[i];
        let (name, value) = if let Some((name, value)) = tok.split_once('=') {
            (name.to_ascii_lowercase(), value)
        } else {
            let name = tok.to_ascii_lowercase();
            i += 1;
            let value = rest.get(i).ok_or(usage)?;
            (name, *value)
        };
        let change = ValueChange::parse(value, usage)?;
        match name.as_str() {
            "mix" if is_reverb => out.reverb_mix = Some(change),
            "decay" | "room" | "feedback" if is_reverb => out.reverb_decay = Some(change),
            "time" | "t" | "secs" | "seconds" if is_delay => out.delay_time_secs = Some(change),
            "feedback" | "fb" if is_delay => out.delay_feedback = Some(change),
            "mix" if is_delay => out.delay_mix = Some(change),
            _ => return Err(format!("unknown fx parameter '{name}'")),
        }
        i += 1;
    }
    Ok(out)
}

pub fn apply_fx_change(current: FxSettings, change: FxChange) -> Result<FxSettings, String> {
    let mut next = current;
    if let Some(enabled) = change.reverb_enabled {
        next.reverb_enabled = enabled;
    }
    if let Some(enabled) = change.delay_enabled {
        next.delay_enabled = enabled;
    }
    apply_fx_value(
        &mut next.reverb_mix,
        &mut next.reverb_mix_step_per_tick,
        change.reverb_mix,
    )?;
    apply_fx_value(
        &mut next.reverb_decay,
        &mut next.reverb_decay_step_per_tick,
        change.reverb_decay,
    )?;
    apply_fx_value(
        &mut next.delay_time_secs,
        &mut next.delay_time_step_per_tick,
        change.delay_time_secs,
    )?;
    apply_fx_value(
        &mut next.delay_feedback,
        &mut next.delay_feedback_step_per_tick,
        change.delay_feedback,
    )?;
    apply_fx_value(
        &mut next.delay_mix,
        &mut next.delay_mix_step_per_tick,
        change.delay_mix,
    )?;
    validate_fx(next)
}

fn apply_fx_value(
    value: &mut f32,
    step: &mut f32,
    change: Option<ValueChange>,
) -> Result<(), String> {
    match change {
        Some(ValueChange::Tick(n)) => *step = n as f32,
        Some(ValueChange::Add(0.0)) => *step = 0.0,
        Some(change) => *value = change.apply(*value as f64)? as f32,
        None => {}
    }
    Ok(())
}

fn validate_fx(next: FxSettings) -> Result<FxSettings, String> {
    if !(0.0..=1.0).contains(&next.reverb_mix) {
        return Err(format!("reverb mix {} out of range 0..1", next.reverb_mix));
    }
    if !(0.0..=0.98).contains(&next.reverb_decay) {
        return Err(format!(
            "reverb decay {} out of range 0..0.98",
            next.reverb_decay
        ));
    }
    if !(0.01..=2.0).contains(&next.delay_time_secs) {
        return Err(format!(
            "delay time {}s out of range 0.01..2",
            next.delay_time_secs
        ));
    }
    if !(0.0..=0.95).contains(&next.delay_feedback) {
        return Err(format!(
            "delay feedback {} out of range 0..0.95",
            next.delay_feedback
        ));
    }
    if !(0.0..=1.0).contains(&next.delay_mix) {
        return Err(format!("delay mix {} out of range 0..1", next.delay_mix));
    }
    Ok(next)
}
