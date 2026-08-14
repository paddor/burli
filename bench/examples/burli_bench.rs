extern crate libc;

use libc::c_int;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_QUALITY: u8 = 5;
const MAX_BENCH_QUALITY: u8 = 5;
const DEFAULT_WINDOW_BITS: u8 = 22;
const BROTLI_MODE_GENERIC: c_int = 0;
const BROTLI_DECODER_RESULT_SUCCESS: c_int = 1;

unsafe extern "C" {
    fn BrotliEncoderMaxCompressedSize(input_size: usize) -> usize;
    fn BrotliEncoderCompress(
        quality: c_int,
        lgwin: c_int,
        mode: c_int,
        input_size: usize,
        input_buffer: *const u8,
        encoded_size: *mut usize,
        encoded_buffer: *mut u8,
    ) -> c_int;
    fn BrotliDecoderDecompress(
        encoded_size: usize,
        encoded_buffer: *const u8,
        decoded_size: *mut usize,
        decoded_buffer: *mut u8,
    ) -> c_int;
}

#[derive(Clone, Copy)]
enum CorpusSet {
    Web,
    Silesia,
}

#[derive(Clone, Copy)]
enum CorpusSelection {
    Web,
    Silesia,
    All,
}

impl CorpusSelection {
    fn includes(self, set: CorpusSet) -> bool {
        matches!(
            (self, set),
            (Self::All, _) | (Self::Web, CorpusSet::Web) | (Self::Silesia, CorpusSet::Silesia)
        )
    }
}

#[derive(Clone, Copy)]
enum CorpusSource {
    Direct {
        url: &'static str,
    },
    Zip {
        url: &'static str,
        archive_sha256: &'static str,
        member: &'static str,
    },
}

#[derive(Clone, Copy)]
struct CorpusEntry {
    set: CorpusSet,
    rel: &'static str,
    label: &'static str,
    source: CorpusSource,
    size: usize,
    sha256: &'static str,
}

#[derive(Clone)]
struct BenchInput {
    name: String,
    data: Vec<u8>,
    sha256: String,
    is_small: bool,
}

#[derive(Serialize)]
struct BenchResult {
    codec: String,
    encoded_by: String,
    decoded_by: String,
    input: String,
    quality: u8,
    input_size: usize,
    compressed_size: usize,
    compress_ns: f64,
    decompress_ns: f64,
    input_sha256: String,
    is_small: bool,
    timestamp_secs: u64,
}

macro_rules! silesia_source {
    ($member:literal, $archive_sha256:literal) => {
        CorpusSource::Zip {
            url: concat!(
                "https://raw.githubusercontent.com/MiloszKrajewski/SilesiaCorpus/",
                "3f3fa2cdbbb3795c903b74e774acb309e1360337/",
                $member,
                ".zip"
            ),
            archive_sha256: $archive_sha256,
            member: $member,
        }
    };
}

const CORPUS: &[CorpusEntry] = &[
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/jquery-3.7.1.js",
        label: "jquery",
        source: CorpusSource::Direct {
            url: "https://raw.githubusercontent.com/jquery/jquery/3.7.1/dist/jquery.js",
        },
        size: 285_314,
        sha256: "78a85aca2f0b110c29e0d2b137e09f0a1fb7a8e554b499f740d6744dc8962cfe",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/lodash-4.17.21.js",
        label: "lodash",
        source: CorpusSource::Direct {
            url: "https://raw.githubusercontent.com/lodash/lodash/4.17.21/lodash.js",
        },
        size: 544_098,
        sha256: "4c04561befdf653aef017a42ac5addf68ea943cdfca6bdee5ce04e04e8139f54",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/bootstrap-5.3.3.bundle.js",
        label: "bootstrap-js",
        source: CorpusSource::Direct {
            url: "https://raw.githubusercontent.com/twbs/bootstrap/v5.3.3/dist/js/bootstrap.bundle.js",
        },
        size: 207_819,
        sha256: "9a4a11a15db88d5fab08f59c1c34796b03f1f15bb3cc928dd226e1c59f7f59a3",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/bootstrap-5.3.3.css",
        label: "bootstrap-css",
        source: CorpusSource::Direct {
            url: "https://raw.githubusercontent.com/twbs/bootstrap/v5.3.3/dist/css/bootstrap.css",
        },
        size: 281_046,
        sha256: "18a105d7cb38e01e5ed0ca255c092992a2e211b39594a7fa57262bfc6fc4ea9c",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/github-markdown-5.5.1.css",
        label: "github-css",
        source: CorpusSource::Direct {
            url: "https://raw.githubusercontent.com/sindresorhus/github-markdown-css/v5.5.1/github-markdown.css",
        },
        size: 28_316,
        sha256: "f8748ea5a2067260000e2fe0bb4810893220720cd8132f9bd38e16d437943d91",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/normalize-8.0.1.css",
        label: "normalize-css",
        source: CorpusSource::Direct {
            url: "https://raw.githubusercontent.com/necolas/normalize.css/8.0.1/normalize.css",
        },
        size: 6_138,
        sha256: "580818700724d42d7fcc4979b0197971fca1c6d2e0286769237a0ac897df5512",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/react-18.2.0.production.min.js",
        label: "react-min",
        source: CorpusSource::Direct {
            url: "https://unpkg.com/react@18.2.0/umd/react.production.min.js",
        },
        size: 10_737,
        sha256: "4b4969fa4ef3594324da2c6d78ce8766fbbc2fd121fff395aedf997db0a99a06",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/preact-10.19.6.module.js",
        label: "preact",
        source: CorpusSource::Direct {
            url: "https://unpkg.com/preact@10.19.6/dist/preact.module.js",
        },
        size: 11_269,
        sha256: "73367fb7f14686b0e57c6eee59079799e3eae71f0382e5cdf8214edc789512b0",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/vue-3.4.21.global.prod.js",
        label: "vue",
        source: CorpusSource::Direct {
            url: "https://unpkg.com/vue@3.4.21/dist/vue.global.prod.js",
        },
        size: 147_534,
        sha256: "4963101441ded7e420c05665e7c616b2f2e3851c99e1cf8af84d29d6f10e77da",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/citm-catalog.json",
        label: "json-citm",
        source: CorpusSource::Direct {
            url: "https://raw.githubusercontent.com/RichardHightower/json-parsers-benchmark/e6d09a817eafc50a5cad821e0743d565899639d9/data/citm_catalog.json",
        },
        size: 1_727_204,
        sha256: "a73e7a883f6ea8de113dff59702975e60119b4b58d451d518a929f31c92e2059",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/mdn-getting-started.html",
        label: "html-mdn-getting-started",
        source: CorpusSource::Direct {
            url: "https://raw.githubusercontent.com/mdn/learning-area/180371ffc567f0f2ef359bf408b976667839e764/html/introduction-to-html/getting-started/index.html",
        },
        size: 224,
        sha256: "fd7f1f3a8acffe21225a31986b1af90a267ce5cb1a827ad1e69ab3e961e1ea3c",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/mdn-debug-example.html",
        label: "html-mdn-debug",
        source: CorpusSource::Direct {
            url: "https://raw.githubusercontent.com/mdn/learning-area/180371ffc567f0f2ef359bf408b976667839e764/html/introduction-to-html/debugging-html/debug-example.html",
        },
        size: 717,
        sha256: "bc8d3e7497a514f8d95d2109eda10f293c108d809db8c015dbee9bbb465816e2",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/mdn-document-structure.html",
        label: "html-mdn-document",
        source: CorpusSource::Direct {
            url: "https://raw.githubusercontent.com/mdn/learning-area/180371ffc567f0f2ef359bf408b976667839e764/html/introduction-to-html/document_and_website_structure/index.html",
        },
        size: 3_525,
        sha256: "8623d0ff8d0696128844a2315d8c23840cf839d52fab09fcd06eabb3e5e966ec",
    },
    CorpusEntry {
        set: CorpusSet::Web,
        rel: "corpus/web/whatwg-html-source",
        label: "html-whatwg-source",
        source: CorpusSource::Direct {
            url: "https://raw.githubusercontent.com/whatwg/html/ac0389a3aca0331055bf4bf23f509c2913e3f795/source",
        },
        size: 7_891_621,
        sha256: "d7c570a3f5a29e559da5a9f57d57aabac787f31b7e42a8565dd5e8d1e7bece66",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/dickens",
        label: "silesia-dickens",
        source: silesia_source!(
            "dickens",
            "b0fcae3adb0334b5b3b73b1d1d06edfc5839c0bb7561255e0c490ab4682b46cc"
        ),
        size: 10_192_446,
        sha256: "b24c37886142e11d0ee687db6ab06f936207aa7f2ea1fd1d9a36763c7a507e6a",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/mozilla",
        label: "silesia-mozilla",
        source: silesia_source!(
            "mozilla",
            "3abdbd504073eda475f5d3d3ee7a69460db465065c329c73dd37ba3a082b8088"
        ),
        size: 51_220_480,
        sha256: "657fc3764b0c75ac9de9623125705831ebbfbe08fed248df73bc2dc66e2a963b",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/mr",
        label: "silesia-mr",
        source: silesia_source!(
            "mr",
            "bfb3e0735c7d275d22b3bc5d142e3f5431aacb7d3f7d329c6c9fe51dc1dfea2e"
        ),
        size: 9_970_564,
        sha256: "68637ed52e3e4860174ed2dc0840ac77d5f1a60abbcb13770d5754e3774d53e6",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/nci",
        label: "silesia-nci",
        source: silesia_source!(
            "nci",
            "2982cb2a3fd9360735c74997b2e60f63b2f0a6a3941167cb0021f45dc0225a02"
        ),
        size: 33_553_445,
        sha256: "fc63a31770947b8c2062d3b19ca94c00485a232bb91b502021948fee983e1635",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/ooffice",
        label: "silesia-ooffice",
        source: silesia_source!(
            "ooffice",
            "909880ebf9fc5702036b921935345450c43f9e352a6acb32100babafcf8f1d30"
        ),
        size: 6_152_192,
        sha256: "e7ee013880d34dd5208283d0d3d91b07f442e067454276095ded14f322a656eb",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/osdb",
        label: "silesia-osdb",
        source: silesia_source!(
            "osdb",
            "a1955a73be3ef1b1b14ab73c75e45e2c5c013c9bbbcaec277e58a94f732eeb1b"
        ),
        size: 10_085_684,
        sha256: "60f027179302ca3ad87c58ac90b6be72ec23588aaa7a3b7fe8ecc0f11def3fa3",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/reymont",
        label: "silesia-reymont",
        source: silesia_source!(
            "reymont",
            "691069ebbcf881d2e5177c0ff81711008209e6bd824e07a90e703451fb96d9c2"
        ),
        size: 6_627_202,
        sha256: "0eac0114a3dfe6e2ee1f345a0f79d653cb26c3bc9f0ed79238af4933422b7578",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/samba",
        label: "silesia-samba",
        source: silesia_source!(
            "samba",
            "285c06096c0e24b71e28705f489932482b23d823b306dddc1cd8d0a8145121a1"
        ),
        size: 21_606_400,
        sha256: "93ba07bc44d8267789c1d911992f40b089ffa2140b4a160fac11ccae9a40e7b2",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/sao",
        label: "silesia-sao",
        source: silesia_source!(
            "sao",
            "eeb657d7511dbdff833853157249506b61cde55a3223f2013e88cbbdb934c36f"
        ),
        size: 7_251_944,
        sha256: "c2d0ea2cc59d4c21b7fe43a71499342a00cbe530a1d5548770e91ecd6214adcc",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/webster",
        label: "silesia-webster",
        source: silesia_source!(
            "webster",
            "6495af470253ced7d60e616a2b2f2f2841a88ea55bfd23cf0f1d46daa808f937"
        ),
        size: 41_458_703,
        sha256: "6a68f69b26daf09f9dd84f7470368553194a0b294fcfa80f1604efb11143a383",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/x-ray",
        label: "silesia-x-ray",
        source: silesia_source!(
            "x-ray",
            "f3d111158444a6cb42e7e60a46582755083c58f1657a55613f0edd64c5626ec6"
        ),
        size: 8_474_240,
        sha256: "7de9fce1405dc44ae5e6813ed21cd5751e761bd4265655a005d39b9685d1c9ad",
    },
    CorpusEntry {
        set: CorpusSet::Silesia,
        rel: "corpus/silesia/xml",
        label: "silesia-xml",
        source: silesia_source!(
            "xml",
            "feeac237babe74e77ca1b7cd72d651ab0a722218ee3d2c07d519625b1a60fe50"
        ),
        size: 5_345_280,
        sha256: "0e82e54e695c1938e4193448022543845b33020c8be6bf3bf3ead2224903e08c",
    },
];

const SMALL_SIZES: &[usize] = &[
    512, 1024, 2048, 4096, 8192, 16_384, 32_768, 65_536, 131_072, 262_144,
];
const CHART_SMALL_FILES: &[&str] = &["bootstrap-js", "bootstrap-css", "json-citm"];
const CHART_SMALL_SIZES: &[usize] = &[512, 1024, 2048, 4096, 8192, 16_384, 32_768, 65_536, 131_072];
const DEFAULT_TARGET_NS: u64 = 30_000_000;
const DEFAULT_ROUNDS: usize = 3;
const DEFAULT_WARMUP: usize = 1;
const QUICK_TARGET_NS: u64 = 30_000_000;
const QUICK_ROUNDS: usize = 1;

struct Args {
    impls: Vec<String>,
    qualities: Vec<u8>,
    corpus: CorpusSelection,
    files: Option<HashSet<String>>,
    small_sizes: Vec<usize>,
    small_only: bool,
    profile_encode_only: bool,
    profile_decode_only: bool,
    bench: BenchConfig,
}

#[derive(Clone, Copy)]
struct BenchConfig {
    target_ns: u64,
    rounds: usize,
    warmup: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    let inputs = load_inputs(&args)?;
    let cache = if args.profile_encode_only {
        None
    } else {
        let cache = cache_root()?;
        fs::create_dir_all(&cache)?;
        Some(cache)
    };

    for codec in args.impls {
        for quality in &args.qualities {
            for input in &inputs {
                if args.profile_encode_only {
                    profile_encode_only(&codec, input, *quality, args.bench)?;
                    continue;
                }
                if args.profile_decode_only {
                    profile_decode_only(&codec, input, *quality, args.bench)?;
                    continue;
                }
                match bench_codec(&codec, input, *quality, args.bench) {
                    Ok(Some(result)) => {
                        append_result(cache.as_deref().unwrap(), &result)?;
                        println!(
                            "{} q{} {}: {} -> {} bytes",
                            result.codec,
                            result.quality,
                            result.input,
                            result.input_size,
                            result.compressed_size
                        );
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("skip {codec} q{quality} {}: {error}", input.name),
                }
            }
        }
    }

    Ok(())
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut args = Args {
        impls: vec!["burli".to_owned(), "rust-brotli".to_owned()],
        qualities: vec![DEFAULT_QUALITY],
        corpus: CorpusSelection::Web,
        files: None,
        small_sizes: SMALL_SIZES.to_vec(),
        small_only: false,
        profile_encode_only: false,
        profile_decode_only: false,
        bench: BenchConfig::from_env(),
    };

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "--impl" => {
                let value = iter.next().ok_or("--impl needs value")?;
                args.impls = if value == "all" {
                    vec![
                        "google-brotli".to_owned(),
                        "burli".to_owned(),
                        "rust-brotli".to_owned(),
                    ]
                } else {
                    value.split(',').map(str::to_owned).collect()
                };
            }
            "--qualities" | "--levels" => {
                let value = iter.next().ok_or("--qualities needs value")?;
                args.qualities = parse_quality_list(&value)?;
            }
            "--corpus" => {
                let value = iter.next().ok_or("--corpus needs value")?;
                args.corpus = parse_corpus_selection(&value)?;
            }
            "--files" => {
                let value = iter.next().ok_or("--files needs value")?;
                args.files = Some(value.split(',').map(str::to_owned).collect());
            }
            "--small-sizes" => {
                let value = iter.next().ok_or("--small-sizes needs value")?;
                args.small_sizes = parse_size_list(&value)?;
            }
            "--small-only" => args.small_only = true,
            "--chart-small-only" | "--small-chart-only" => {
                args.small_only = true;
                args.files = Some(
                    CHART_SMALL_FILES
                        .iter()
                        .map(|name| (*name).to_owned())
                        .collect(),
                );
                args.small_sizes = CHART_SMALL_SIZES.to_vec();
            }
            "--quick" => {
                args.bench = BenchConfig {
                    target_ns: QUICK_TARGET_NS,
                    rounds: QUICK_ROUNDS,
                    warmup: 0,
                };
            }
            "--target-ns" => {
                args.bench.target_ns = parse_non_zero_u64(
                    &iter.next().ok_or("--target-ns needs value")?,
                    "--target-ns",
                )?;
            }
            "--target-ms" => {
                let millis = parse_non_zero_u64(
                    &iter.next().ok_or("--target-ms needs value")?,
                    "--target-ms",
                )?;
                args.bench.target_ns = millis
                    .checked_mul(1_000_000)
                    .ok_or("--target-ms value is too large")?;
            }
            "--rounds" => {
                args.bench.rounds =
                    parse_non_zero_usize(&iter.next().ok_or("--rounds needs value")?, "--rounds")?;
            }
            "--warmup" => {
                args.bench.warmup = iter.next().ok_or("--warmup needs value")?.parse()?;
            }
            "--profile-encode-only" => args.profile_encode_only = true,
            "--profile-decode-only" => args.profile_decode_only = true,
            other => return Err(format!("unknown arg: {other}").into()),
        }
    }

    Ok(args)
}

fn print_help() {
    println!(
        "Usage: burli_bench [--impl burli|rust-brotli|google-brotli|all] \
         [--qualities LIST] [--corpus web|silesia|all] [--files LIST] \
         [--small-only] [--chart-small-only] [--small-sizes LIST] [--quick] \
         [--target-ms N] [--target-ns N] [--rounds N] [--warmup N]"
    );
}

impl BenchConfig {
    fn from_env() -> Self {
        Self {
            target_ns: env_non_zero_u64("BURLI_BENCH_TARGET_NS").unwrap_or(DEFAULT_TARGET_NS),
            rounds: env_non_zero_usize("BURLI_BENCH_ROUNDS").unwrap_or(DEFAULT_ROUNDS),
            warmup: env_usize("BURLI_BENCH_WARMUP").unwrap_or(DEFAULT_WARMUP),
        }
    }
}

fn env_non_zero_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|&value| value != 0)
}

fn env_non_zero_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|&value| value != 0)
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}

fn parse_non_zero_u64(value: &str, name: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let parsed: u64 = value.parse()?;
    if parsed == 0 {
        return Err(format!("{name} must be non-zero").into());
    }
    Ok(parsed)
}

fn parse_non_zero_usize(value: &str, name: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let parsed: usize = value.parse()?;
    if parsed == 0 {
        return Err(format!("{name} must be non-zero").into());
    }
    Ok(parsed)
}

fn parse_quality_list(value: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut qualities = Vec::new();
    for item in value.split(',') {
        let quality: u8 = item.parse()?;
        if quality > MAX_BENCH_QUALITY {
            return Err(
                format!("benchmark qualities are limited to q0..q{MAX_BENCH_QUALITY}").into(),
            );
        }
        burli::Quality::new(quality)?;
        qualities.push(quality);
    }
    Ok(qualities)
}

fn parse_corpus_selection(value: &str) -> Result<CorpusSelection, Box<dyn std::error::Error>> {
    match value {
        "web" => Ok(CorpusSelection::Web),
        "silesia" => Ok(CorpusSelection::Silesia),
        "all" => Ok(CorpusSelection::All),
        _ => Err(format!("unknown corpus: {value}").into()),
    }
}

fn parse_size_list(value: &str) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
    let mut sizes = Vec::new();
    for item in value.split(',') {
        let size: usize = item.parse()?;
        if size == 0 {
            return Err("small sizes must be non-zero".into());
        }
        sizes.push(size);
    }
    sizes.sort_unstable();
    sizes.dedup();
    if sizes.is_empty() {
        return Err("--small-sizes needs at least one size".into());
    }
    Ok(sizes)
}

fn load_inputs(args: &Args) -> Result<Vec<BenchInput>, Box<dyn std::error::Error>> {
    let mut inputs = Vec::new();
    for entry in CORPUS {
        if !args.corpus.includes(entry.set) {
            continue;
        }
        if args
            .files
            .as_ref()
            .is_some_and(|files| !files.contains(entry.label))
        {
            continue;
        }
        let data = read_corpus_entry(entry)?;
        let sha256 = sha256_hex(&data);
        if args.small_only {
            for &size in &args.small_sizes {
                if size <= data.len() {
                    inputs.push(BenchInput {
                        name: format!("{}_{}", entry.label, size_label(size)),
                        data: data[..size].to_vec(),
                        sha256: sha256_hex(&data[..size]),
                        is_small: true,
                    });
                }
            }
        } else {
            inputs.push(BenchInput {
                name: entry.label.to_owned(),
                data,
                sha256,
                is_small: false,
            });
        }
    }
    Ok(inputs)
}

fn read_corpus_entry(entry: &CorpusEntry) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = Path::new("bench").join(entry.rel);
    if !path.exists() {
        match entry.source {
            CorpusSource::Direct { url } => download_file(url, &path)?,
            CorpusSource::Zip {
                url,
                archive_sha256,
                member,
            } => extract_zip_corpus_file(url, archive_sha256, member, &path)?,
        }
    }
    let data = fs::read(&path)?;
    if data.len() != entry.size {
        return Err(format!(
            "{} size mismatch: got {}, expected {}",
            entry.label,
            data.len(),
            entry.size
        )
        .into());
    }
    let actual = sha256_hex(&data);
    if actual != entry.sha256 {
        return Err(format!("{} sha256 mismatch: {actual}", entry.label).into());
    }
    Ok(data)
}

fn extract_zip_corpus_file(
    url: &str,
    archive_sha256: &str,
    member: &str,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let zip_path = Path::new("bench")
        .join("corpus")
        .join("silesia")
        .join(".downloads")
        .join(format!("{member}.zip"));
    ensure_downloaded_file(url, &zip_path, archive_sha256)?;

    let parent = path.parent().ok_or("corpus path missing parent")?;
    fs::create_dir_all(parent)?;
    let tmp = temp_path(path)?;
    let _ = fs::remove_file(&tmp);
    let output = File::create(&tmp)?;
    let status = Command::new("unzip")
        .arg("-p")
        .arg(&zip_path)
        .arg(member)
        .stdout(Stdio::from(output))
        .status()?;
    if !status.success() {
        let _ = fs::remove_file(&tmp);
        return Err(format!("unzip failed for {member}").into());
    }
    fs::rename(tmp, path)?;
    Ok(())
}

fn ensure_downloaded_file(
    url: &str,
    path: &Path,
    sha256: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        if file_sha256_hex(path)? == sha256 {
            return Ok(());
        }
        fs::remove_file(path)?;
    }
    download_file(url, path)?;
    let actual = file_sha256_hex(path)?;
    if actual != sha256 {
        let _ = fs::remove_file(path);
        return Err(format!("{} sha256 mismatch: {actual}", path.display()).into());
    }
    Ok(())
}

fn download_file(url: &str, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().ok_or("corpus path missing parent")?;
    fs::create_dir_all(parent)?;
    let tmp = temp_path(path)?;
    let _ = fs::remove_file(&tmp);
    let status = Command::new("curl")
        .arg("-fL")
        .arg("--retry")
        .arg("3")
        .arg("-o")
        .arg(&tmp)
        .arg(url)
        .status()?;
    if !status.success() {
        let _ = fs::remove_file(&tmp);
        return Err(format!("curl failed for {url}").into());
    }
    fs::rename(tmp, path)?;
    Ok(())
}

fn temp_path(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let file_name = path
        .file_name()
        .ok_or("corpus path missing file name")?
        .to_string_lossy();
    Ok(path.with_file_name(format!("{file_name}.tmp")))
}

fn file_sha256_hex(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(sha256_hex(&fs::read(path)?))
}

fn profile_encode_only(
    codec: &str,
    input: &BenchInput,
    quality: u8,
    bench: BenchConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let compressed = compress_codec(codec, &input.data, quality)?;
    verify_decodes(codec, &compressed, input)?;
    let compress_ns = bench_compress(codec, &input.data, quality, bench)?;
    let mbs = input.data.len() as f64 / compress_ns * 1000.0;
    println!(
        "{} q{} {}: {} -> {} bytes, encode {:.1} MB/s",
        codec_label(codec),
        quality,
        input.name,
        input.data.len(),
        compressed.len(),
        mbs
    );
    Ok(())
}

fn profile_decode_only(
    codec: &str,
    input: &BenchInput,
    quality: u8,
    bench: BenchConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let compressed = compress_codec(codec, &input.data, quality)?;
    verify_decodes(codec, &compressed, input)?;
    let decompress_ns = bench_decompress(codec, &compressed, input.data.len(), bench)?;
    let mbs = input.data.len() as f64 / decompress_ns * 1000.0;
    println!(
        "{} q{} {}: {} <- {} bytes, decode {:.1} MB/s",
        codec_label(codec),
        quality,
        input.name,
        input.data.len(),
        compressed.len(),
        mbs
    );
    Ok(())
}

fn bench_codec(
    codec: &str,
    input: &BenchInput,
    quality: u8,
    bench: BenchConfig,
) -> Result<Option<BenchResult>, Box<dyn std::error::Error>> {
    let compressed = match compress_codec(codec, &input.data, quality) {
        Ok(output) => output,
        Err(error) if is_unsupported_burli(error.as_ref()) => return Ok(None),
        Err(error) => return Err(error),
    };
    verify_decodes(codec, &compressed, input)?;

    let compress_ns = bench_compress(codec, &input.data, quality, bench)?;

    let decompress_ns = bench_decompress(codec, &compressed, input.data.len(), bench)?;

    let encoded_by = encoded_by_label(codec);
    let decoded_by = decoded_by_label(codec);
    let codec = codec_label(codec);
    Ok(Some(BenchResult {
        codec: codec.clone(),
        encoded_by,
        decoded_by,
        input: input.name.clone(),
        quality,
        input_size: input.data.len(),
        compressed_size: compressed.len(),
        compress_ns,
        decompress_ns,
        input_sha256: input.sha256.clone(),
        is_small: input.is_small,
        timestamp_secs: now_secs(),
    }))
}

fn bench_decompress(
    codec: &str,
    compressed: &[u8],
    decoded_len: usize,
    bench: BenchConfig,
) -> Result<f64, Box<dyn std::error::Error>> {
    match codec {
        "burli" => Ok(bench_loop(bench, || {
            let _ = burli_decompress_with_limit(compressed, decoded_len);
        })),
        "google-brotli" => Ok(bench_loop(bench, || {
            let _ = google_brotli_decompress(compressed, decoded_len);
        })),
        "google-brotli-burli" => Ok(bench_loop(bench, || {
            let _ = burli_decompress_with_limit(compressed, decoded_len);
        })),
        "rust-brotli" => Ok(bench_loop(bench, || {
            let _ = rust_brotli_decompress(compressed);
        })),
        other => Err(format!("unknown impl: {other}").into()),
    }
}

fn compress_codec(
    codec: &str,
    input: &[u8],
    quality: u8,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    match codec {
        "burli" => burli_compress(input, quality).map_err(Into::into),
        "google-brotli" => google_brotli_compress(input, quality),
        "google-brotli-burli" => google_brotli_compress(input, quality),
        "rust-brotli" => rust_brotli_compress(input, quality),
        other => Err(format!("unknown impl: {other}").into()),
    }
}

fn bench_compress(
    codec: &str,
    input: &[u8],
    quality: u8,
    bench: BenchConfig,
) -> Result<f64, Box<dyn std::error::Error>> {
    match codec {
        "burli" => Ok(bench_loop(bench, || {
            let _ = burli_compress(input, quality);
        })),
        "google-brotli" => Ok(bench_loop(bench, || {
            let _ = google_brotli_compress(input, quality);
        })),
        "google-brotli-burli" => Ok(bench_loop(bench, || {
            let _ = google_brotli_compress(input, quality);
        })),
        "rust-brotli" => Ok(bench_loop(bench, || {
            let _ = rust_brotli_compress(input, quality);
        })),
        other => Err(format!("unknown impl: {other}").into()),
    }
}

fn verify_decodes(
    codec: &str,
    compressed: &[u8],
    input: &BenchInput,
) -> Result<(), Box<dyn std::error::Error>> {
    let decoded = match codec {
        "burli" => burli_decompress(compressed)?,
        "google-brotli" => google_brotli_decompress(compressed, input.data.len())?,
        "google-brotli-burli" => burli_decompress(compressed)?,
        "rust-brotli" => rust_brotli_decompress(compressed)?,
        _ => unreachable!(),
    };
    if decoded != input.data {
        return Err(format!("{codec} roundtrip mismatch on {}", input.name).into());
    }
    Ok(())
}

fn is_unsupported_burli(error: &(dyn std::error::Error + 'static)) -> bool {
    error
        .downcast_ref::<burli::BurliError>()
        .is_some_and(|error| matches!(error, burli::BurliError::Unsupported(_)))
}

fn burli_compress(input: &[u8], quality: u8) -> Result<Vec<u8>, burli::BurliError> {
    burli::compress(input, quality)
}

fn burli_decompress(input: &[u8]) -> Result<Vec<u8>, burli::BurliError> {
    burli::decompress(input)
}

fn burli_decompress_with_limit(
    input: &[u8],
    decoded_len: usize,
) -> Result<Vec<u8>, burli::BurliError> {
    burli::decompress_with_limit(input, decoded_len)
}

fn google_brotli_compress(
    input: &[u8],
    quality: u8,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut output_size = unsafe { BrotliEncoderMaxCompressedSize(input.len()) };
    let mut output = vec![0; output_size];
    let ok = unsafe {
        BrotliEncoderCompress(
            i32::from(quality),
            i32::from(DEFAULT_WINDOW_BITS),
            BROTLI_MODE_GENERIC,
            input.len(),
            input.as_ptr(),
            &mut output_size,
            output.as_mut_ptr(),
        )
    };
    if ok != 1 {
        return Err("google-brotli compression failed".into());
    }
    output.truncate(output_size);
    Ok(output)
}

fn google_brotli_decompress(
    input: &[u8],
    decoded_size: usize,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut output_size = decoded_size;
    let mut output = vec![0; output_size];
    let result = unsafe {
        BrotliDecoderDecompress(
            input.len(),
            input.as_ptr(),
            &mut output_size,
            output.as_mut_ptr(),
        )
    };
    if result != BROTLI_DECODER_RESULT_SUCCESS {
        return Err(format!("google-brotli decompression failed: {result}").into());
    }
    output.truncate(output_size);
    Ok(output)
}

fn rust_brotli_compress(input: &[u8], quality: u8) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut reader = rust_brotli::CompressorReader::new(
        input,
        4096,
        u32::from(quality),
        u32::from(DEFAULT_WINDOW_BITS),
    );
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    Ok(output)
}

fn rust_brotli_decompress(input: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut reader = rust_brotli::Decompressor::new(input, 4096);
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    Ok(output)
}

fn bench_loop<F: FnMut()>(bench: BenchConfig, mut f: F) -> f64 {
    for _ in 0..bench.warmup {
        f();
    }

    let mut best = f64::MAX;
    for _ in 0..bench.rounds {
        let mut iters = 0u64;
        let start = cpu_nanos();
        loop {
            std::hint::black_box(&mut f)();
            iters += 1;
            if cpu_nanos() - start >= bench.target_ns {
                break;
            }
        }
        let elapsed = cpu_nanos() - start;
        let ns_per_op = elapsed as f64 / iters as f64;
        best = best.min(ns_per_op);
    }
    best
}

fn cpu_nanos() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

fn append_result(cache: &Path, result: &BenchResult) -> Result<(), Box<dyn std::error::Error>> {
    let path = cache.join(format!("{}.jsonl", result.codec.replace([' ', '-'], "_")));
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, result)?;
    writeln!(file)?;
    Ok(())
}

fn cache_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(value) = std::env::var_os("BURLI_CACHE_DIR") {
        return Ok(PathBuf::from(value));
    }
    let home = std::env::var_os("HOME").ok_or("HOME not set")?;
    Ok(PathBuf::from(home).join(".cache").join("burli"))
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn size_label(size: usize) -> String {
    match size {
        512 => "512".to_owned(),
        1024 => "1k".to_owned(),
        2048 => "2k".to_owned(),
        4096 => "4k".to_owned(),
        8192 => "8k".to_owned(),
        16_384 => "16k".to_owned(),
        32_768 => "32k".to_owned(),
        65_536 => "64k".to_owned(),
        131_072 => "128k".to_owned(),
        262_144 => "256k".to_owned(),
        _ => format!("{size}b"),
    }
}

fn codec_label(codec: &str) -> String {
    if codec == "burli" && cfg!(feature = "paranoid") {
        "burli paranoid".to_owned()
    } else {
        codec.to_owned()
    }
}

fn encoded_by_label(codec: &str) -> String {
    match codec {
        "google-brotli-burli" => "google-brotli".to_owned(),
        _ => codec_label(codec),
    }
}

fn decoded_by_label(codec: &str) -> String {
    match codec {
        "google-brotli-burli" => "burli".to_owned(),
        _ => codec_label(codec),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
