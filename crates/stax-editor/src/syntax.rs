use egui::Color32;
use crate::shell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TK {
    Comment,
    Number,
    Operator,  // +  -  *  /  @  etc.  → port-signal
    Bracket,   // [ ]                  → ink-2
    Lambda,    // \  =  =>             → port-fun 500
    Str,       // "…"                  → port-stream
    Symbol,    // .name  ,name  'name  → port-fun
    Builtin,   // known words          → ink 500
    Word,      // user words           → ink 400
    Space,
}

pub struct Span {
    pub start: usize,
    pub end:   usize,
    pub kind:  TK,
}

impl Span {
    pub fn color(&self) -> Color32 {
        match self.kind {
            TK::Comment  => shell::INK_3,
            TK::Number   => shell::INK,
            TK::Operator => shell::PORT_SIGNAL,
            TK::Bracket  => shell::INK_2,
            TK::Lambda   => shell::PORT_FUN,
            TK::Str      => shell::PORT_STREAM,
            TK::Symbol   => shell::PORT_FUN,
            TK::Builtin  => shell::INK,
            TK::Word     => shell::INK,
            TK::Space    => shell::PAPER,
        }
    }

    pub fn strong(&self) -> bool {
        matches!(self.kind, TK::Builtin | TK::Lambda)
    }
}

// All M0–M3 built-in words (used to distinguish from user names)
static BUILTINS: &[&str] = &[
    // oscillators
    "sinosc","saw","lfsaw","tri","square","pulse","impulse",
    // noise
    "wnoise","white","pnoise","pink","brown","lfnoise0","lfnoise1","dust","dust2","sah",
    // filters
    "lpf1","lpf","lpf2","hpf1","hpf","hpf2","rlpf","rhpf","lag","lag2","leakdc",
    "svflp","svfhp","svfbp","svfnotch","firlp","firhp","firbp","hilbert",
    "disperser","thiran","farrow",
    // envelopes
    "ar","adsr","fadein","fadeout","hanenv","decay","decay2","line","xline",
    // dynamics / delay
    "combn","delayn","verb","compressor","limiter",
    // waveshaping
    "tanhsat","softclip","hardclip","cubicsat","atansat","chebdist",
    // spatial
    "pan2","bal2","rot2","pan3",
    // synthesis
    "pluck","grain","pvocstretch","pvocp",
    // windows
    "hann","hamming","blackman","blackmanharris","nuttall","flattop","gaussian","kaiser",
    // analysis
    "goertzel","goertzelc","cqt","mdct","imdct","lpcanalz","lpcsynth","fft","ifft",
    "normalize","peak","rms","dur",
    // attractors
    "lorenz","rossler","duffing","vanderpol","logistic","henon",
    // i/o
    "play","stop","record","p","trace","inspect","bench",
    // stack
    "dup","drop","swap","over","rot","nip","tuck",
    // control
    "if","ifelse","while","loop","do","times",
    // math
    "sqrt","abs","neg","sign","sin","cos","tan","ln","log","exp",
    "floor","ceil","round","min","max","clip","wrap","fold","hypot","sinc",
    // streams
    "nat","ord","ordz","cyc","by","nby","to","take","N","Z",
    "keep","skip","filter","keepWhile","skipWhile",
    "size","reverse","sort","grade","flatten","mirror","shift","lace","2X",
    "grow","ngrow","lindiv","expdiv","ever",
    // forms
    "has","keys","values","kv","local","dot","parent","ref","deref",
    // conversion
    "midihz","midinote","bilin","biexp","linlin","linexp","explin","dbtamp","amptodb",
    // sample-rate
    "sr","nyq","isr","inyq","rps",
    // random
    "rand","irand","rands","irands","picks","coins","seed","muss",
    // Z-list
    "natz","byz","nbyz","invz","negz","evenz","oddz",
    // misc
    "upSmp","dwnSmp",
];

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_op_char(c: char) -> bool {
    matches!(c, '+' | '-' | '*' | '/' | '!' | '%' | '@' | '&' | '|' | '^' | '~' | '#')
}

pub fn highlight(src: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0;

    macro_rules! push {
        ($s:expr, $e:expr, $k:expr) => {
            spans.push(Span { start: $s, end: $e, kind: $k })
        };
    }

    while i < len {
        let c = b[i] as char;

        // Line comment  //
        if c == '/' && i + 1 < len && b[i + 1] == b'/' {
            let s = i;
            while i < len && b[i] != b'\n' { i += 1; }
            push!(s, i, TK::Comment);
            continue;
        }

        // Whitespace
        if c.is_ascii_whitespace() {
            let s = i;
            while i < len && (b[i] as char).is_ascii_whitespace() { i += 1; }
            push!(s, i, TK::Space);
            continue;
        }

        // String literal  "…"
        if c == '"' {
            let s = i;
            i += 1;
            while i < len && b[i] != b'"' {
                if b[i] == b'\\' { i += 1; }
                i += 1;
            }
            if i < len { i += 1; }
            push!(s, i, TK::Str);
            continue;
        }

        // Brackets  [ ]
        if c == '[' || c == ']' {
            push!(i, i + 1, TK::Bracket);
            i += 1;
            continue;
        }

        // Lambda  \
        if c == '\\' {
            push!(i, i + 1, TK::Lambda);
            i += 1;
            continue;
        }

        // <= and >=  → Operator
        if (c == '<' || c == '>') && i + 1 < len && b[i + 1] == b'=' {
            push!(i, i + 2, TK::Operator);
            i += 2;
            continue;
        }

        // < >  → Operator (standalone)
        if c == '<' || c == '>' {
            push!(i, i + 1, TK::Operator);
            i += 1;
            continue;
        }

        // == → Operator,  => → Lambda,  = → Lambda (bind)
        if c == '=' {
            if i + 1 < len && b[i + 1] == b'=' {
                push!(i, i + 2, TK::Operator);
                i += 2;
            } else if i + 1 < len && b[i + 1] == b'>' {
                push!(i, i + 2, TK::Lambda);
                i += 2;
            } else {
                push!(i, i + 1, TK::Lambda);
                i += 1;
            }
            continue;
        }

        // Symbols  .name  ,name
        if (c == '.' || c == ',') && i + 1 < len && (b[i + 1] as char).is_alphabetic() {
            let s = i;
            i += 1;
            while i < len && is_word_char(b[i] as char) { i += 1; }
            push!(s, i, TK::Symbol);
            continue;
        }

        // Number (digits, optional leading -, decimal point)
        if c.is_ascii_digit() {
            let s = i;
            while i < len && (b[i] as char).is_ascii_digit() { i += 1; }
            if i < len && b[i] == b'.' && i + 1 < len && (b[i + 1] as char).is_ascii_digit() {
                i += 1;
                while i < len && (b[i] as char).is_ascii_digit() { i += 1; }
            }
            push!(s, i, TK::Number);
            continue;
        }

        // Operator characters
        if is_op_char(c) {
            let s = i;
            while i < len && is_op_char(b[i] as char) { i += 1; }
            push!(s, i, TK::Operator);
            continue;
        }

        // Word / built-in / quote-symbol 'name
        if c.is_alphabetic() || c == '_' || c == '\'' {
            let s = i;
            let is_sym = c == '\'';
            i += 1;
            while i < len && is_word_char(b[i] as char) { i += 1; }
            let word = &src[s..i];
            let kind = if is_sym {
                TK::Symbol
            } else if BUILTINS.contains(&word) {
                TK::Builtin
            } else {
                TK::Word
            };
            push!(s, i, kind);
            continue;
        }

        // Anything else
        push!(i, i + 1, TK::Operator);
        i += 1;
    }

    spans
}

/// Build an egui LayoutJob for highlighted source text at the default 13px size.
pub fn layout_job(src: &str) -> egui::text::LayoutJob {
    layout_job_sized(src, 13.0)
}

/// Build an egui LayoutJob for highlighted source text at a configurable font size.
pub fn layout_job_sized(src: &str, font_size: f32) -> egui::text::LayoutJob {
    let spans = highlight(src);
    let mut job = egui::text::LayoutJob::default();
    let mono = egui::FontId::new(font_size, egui::FontFamily::Monospace);

    for span in &spans {
        let text = &src[span.start..span.end];
        if text.is_empty() { continue; }
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: mono.clone(),
                color: span.color(),
                ..Default::default()
            },
        );
    }
    job
}
