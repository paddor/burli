use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};

const QUALITIES: &[u8] = &[0, 1, 2, 3, 4, 5];

#[derive(Clone, Deserialize)]
struct BenchRow {
    codec: String,
    encoded_by: Option<String>,
    decoded_by: Option<String>,
    input: String,
    quality: u8,
    input_size: usize,
    compressed_size: usize,
    compress_ns: f64,
    decompress_ns: f64,
    is_small: bool,
    timestamp_secs: u64,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
struct Key {
    input: String,
    quality: u8,
    is_small: bool,
    input_size: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Scope {
    All,
    Full,
    Small,
}

impl Scope {
    fn includes(self, is_small: bool) -> bool {
        match self {
            Scope::All => true,
            Scope::Full => !is_small,
            Scope::Small => is_small,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CorpusFilter {
    Web,
    Silesia,
    All,
}

impl CorpusFilter {
    fn includes(self, input: &str) -> bool {
        let is_silesia = input.starts_with("silesia-");
        match self {
            CorpusFilter::Web => !is_silesia,
            CorpusFilter::Silesia => is_silesia,
            CorpusFilter::All => true,
        }
    }
}

#[derive(Clone)]
struct Args {
    scope: Scope,
    corpus: CorpusFilter,
    top: usize,
    cache: Option<PathBuf>,
}

#[derive(Clone)]
struct GateRow {
    input: String,
    quality: u8,
    is_small: bool,
    input_size: usize,
    ratio: f64,
    burli_mbs: Option<f64>,
    google_mbs: Option<f64>,
    burli_size: Option<usize>,
    google_size: Option<usize>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    let cache = args.cache.unwrap_or_else(default_cache_dir);
    let google = load_latest(&cache, "google-brotli")?;
    let burli = load_latest(&cache, "burli")?;
    let burli_on_google = load_latest(&cache, "google-brotli-burli")?;

    let mut encode = Vec::new();
    let mut decode_c = Vec::new();
    let mut decode_self = Vec::new();
    let mut size = Vec::new();
    let mut missing_burli = 0usize;
    let mut missing_decode_c = 0usize;

    for (key, g) in &google {
        if !args.scope.includes(key.is_small)
            || !args.corpus.includes(&key.input)
            || !QUALITIES.contains(&key.quality)
        {
            continue;
        }
        if let Some(b) = burli.get(key) {
            encode.push(speed_gate(key, b, g, true));
            decode_self.push(speed_gate(key, b, g, false));
            size.push(size_gate(key, b, g));
        } else {
            missing_burli += 1;
        }
        if let Some(bg) = burli_on_google.get(key) {
            decode_c.push(speed_gate(key, bg, g, false));
        } else {
            missing_decode_c += 1;
        }
    }

    print_worst("encode speed: Burli / C", encode, args.top, true);
    print_worst("decode speed on C streams: Burli / C", decode_c, args.top, true);
    print_worst("decode speed on own streams: Burli / C", decode_self, args.top, true);
    print_worst("compressed size: Burli / C", size, args.top, false);
    if missing_burli != 0 || missing_decode_c != 0 {
        println!(
            "missing cells: burli={missing_burli}, decode-c={missing_decode_c}"
        );
    }

    Ok(())
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = Args {
        scope: Scope::All,
        corpus: CorpusFilter::Web,
        top: 12,
        cache: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--full" => args.scope = Scope::Full,
            "--small" => args.scope = Scope::Small,
            "--all" => args.scope = Scope::All,
            "--corpus" => {
                args.corpus = parse_corpus_filter(&iter.next().ok_or("--corpus needs value")?)?;
            }
            "--top" => args.top = parse_non_zero_usize(&iter.next().ok_or("--top needs value")?)?,
            "--cache" => args.cache = Some(PathBuf::from(iter.next().ok_or("--cache needs value")?)),
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}").into()),
        }
    }
    Ok(args)
}

fn print_help() {
    println!(
        "Usage: burli_gates [--all|--full|--small] [--corpus web|silesia|all] [--top N] [--cache PATH]"
    );
}

fn parse_corpus_filter(value: &str) -> Result<CorpusFilter, Box<dyn Error>> {
    match value {
        "web" => Ok(CorpusFilter::Web),
        "silesia" => Ok(CorpusFilter::Silesia),
        "all" => Ok(CorpusFilter::All),
        _ => Err(format!("unknown corpus: {value}").into()),
    }
}

fn parse_non_zero_usize(value: &str) -> Result<usize, Box<dyn Error>> {
    let parsed = value.parse()?;
    if parsed == 0 {
        return Err("value must be non-zero".into());
    }
    Ok(parsed)
}

fn default_cache_dir() -> PathBuf {
    std::env::var_os("BURLI_CACHE_DIR").map_or_else(
        || {
            let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
            PathBuf::from(home).join(".cache").join("burli")
        },
        PathBuf::from,
    )
}

fn load_latest(cache: &Path, codec: &str) -> Result<BTreeMap<Key, BenchRow>, Box<dyn Error>> {
    let path = cache.join(format!("{}.jsonl", codec.replace([' ', '-'], "_")));
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.into()),
    };
    let mut rows: BTreeMap<Key, BenchRow> = BTreeMap::new();
    for line in content.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(row) = serde_json::from_str::<BenchRow>(line) else {
            continue;
        };
        if row.codec != codec {
            continue;
        }
        if !row_matches_labels(&row, codec) {
            continue;
        }
        let key = Key {
            input: row.input.clone(),
            quality: row.quality,
            is_small: row.is_small,
            input_size: row.input_size,
        };
        match rows.get(&key) {
            Some(old) if old.timestamp_secs >= row.timestamp_secs => {}
            _ => {
                rows.insert(key, row);
            }
        }
    }
    Ok(rows)
}

fn row_matches_labels(row: &BenchRow, codec: &str) -> bool {
    let (encoded_by, decoded_by) = match codec {
        "google-brotli-burli" => ("google-brotli", "burli"),
        _ => (codec, codec),
    };
    row.encoded_by
        .as_deref()
        .is_none_or(|value| value == encoded_by)
        && row
            .decoded_by
            .as_deref()
            .is_none_or(|value| value == decoded_by)
}

fn speed_gate(key: &Key, burli: &BenchRow, google: &BenchRow, encode: bool) -> GateRow {
    let burli_ns = if encode {
        burli.compress_ns
    } else {
        burli.decompress_ns
    };
    let google_ns = if encode {
        google.compress_ns
    } else {
        google.decompress_ns
    };
    let burli_mbs = mb_per_sec(burli.input_size, burli_ns);
    let google_mbs = mb_per_sec(google.input_size, google_ns);
    GateRow {
        input: key.input.clone(),
        quality: key.quality,
        is_small: key.is_small,
        input_size: burli.input_size,
        ratio: burli_mbs / google_mbs,
        burli_mbs: Some(burli_mbs),
        google_mbs: Some(google_mbs),
        burli_size: Some(burli.compressed_size),
        google_size: Some(google.compressed_size),
    }
}

fn size_gate(key: &Key, burli: &BenchRow, google: &BenchRow) -> GateRow {
    GateRow {
        input: key.input.clone(),
        quality: key.quality,
        is_small: key.is_small,
        input_size: burli.input_size,
        ratio: burli.compressed_size as f64 / google.compressed_size as f64,
        burli_mbs: None,
        google_mbs: None,
        burli_size: Some(burli.compressed_size),
        google_size: Some(google.compressed_size),
    }
}

fn mb_per_sec(bytes: usize, ns: f64) -> f64 {
    bytes as f64 / ns * 1000.0
}

fn print_worst(title: &str, mut rows: Vec<GateRow>, top: usize, higher_is_better: bool) {
    if rows.is_empty() {
        return;
    }
    if higher_is_better {
        rows.sort_by(|a, b| a.ratio.partial_cmp(&b.ratio).unwrap_or(Ordering::Equal));
    } else {
        rows.sort_by(|a, b| b.ratio.partial_cmp(&a.ratio).unwrap_or(Ordering::Equal));
    }
    println!();
    println!("{title}");
    println!("ratio  q  scope  size     input                         burli        C            bytes");
    for row in rows.into_iter().take(top) {
        let scope = if row.is_small { "small" } else { "full " };
        let burli_value = row.burli_mbs.map_or_else(
            || format_bytes(row.burli_size),
            |mbs| format!("{mbs:8.1} MB/s"),
        );
        let google_value = row.google_mbs.map_or_else(
            || format_bytes(row.google_size),
            |mbs| format!("{mbs:8.1} MB/s"),
        );
        let bytes = match (row.burli_size, row.google_size) {
            (Some(b), Some(g)) => format!("{b}/{g}"),
            _ => String::new(),
        };
        println!(
            "{:5.2} q{} {scope} {:7} {:29} {:>14} {:>12} {:>12}",
            row.ratio,
            row.quality,
            row.input_size,
            row.input,
            burli_value,
            google_value,
            bytes
        );
    }
}

fn format_bytes(value: Option<usize>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| format!("{value:8} B"))
}
