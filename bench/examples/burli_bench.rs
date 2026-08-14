extern crate libc;

use libc::c_int;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
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
struct CorpusEntry {
    rel: &'static str,
    label: &'static str,
    url: &'static str,
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

const CORPUS: &[CorpusEntry] = &[
    CorpusEntry {
        rel: "corpus/web/jquery-3.7.1.js",
        label: "jquery",
        url: "https://raw.githubusercontent.com/jquery/jquery/3.7.1/dist/jquery.js",
        size: 285_314,
        sha256: "78a85aca2f0b110c29e0d2b137e09f0a1fb7a8e554b499f740d6744dc8962cfe",
    },
    CorpusEntry {
        rel: "corpus/web/lodash-4.17.21.js",
        label: "lodash",
        url: "https://raw.githubusercontent.com/lodash/lodash/4.17.21/lodash.js",
        size: 544_098,
        sha256: "4c04561befdf653aef017a42ac5addf68ea943cdfca6bdee5ce04e04e8139f54",
    },
    CorpusEntry {
        rel: "corpus/web/bootstrap-5.3.3.bundle.js",
        label: "bootstrap-js",
        url: "https://raw.githubusercontent.com/twbs/bootstrap/v5.3.3/dist/js/bootstrap.bundle.js",
        size: 207_819,
        sha256: "9a4a11a15db88d5fab08f59c1c34796b03f1f15bb3cc928dd226e1c59f7f59a3",
    },
    CorpusEntry {
        rel: "corpus/web/bootstrap-5.3.3.css",
        label: "bootstrap-css",
        url: "https://raw.githubusercontent.com/twbs/bootstrap/v5.3.3/dist/css/bootstrap.css",
        size: 281_046,
        sha256: "18a105d7cb38e01e5ed0ca255c092992a2e211b39594a7fa57262bfc6fc4ea9c",
    },
    CorpusEntry {
        rel: "corpus/web/github-markdown-5.5.1.css",
        label: "github-css",
        url: "https://raw.githubusercontent.com/sindresorhus/github-markdown-css/v5.5.1/github-markdown.css",
        size: 28_316,
        sha256: "f8748ea5a2067260000e2fe0bb4810893220720cd8132f9bd38e16d437943d91",
    },
    CorpusEntry {
        rel: "corpus/web/normalize-8.0.1.css",
        label: "normalize-css",
        url: "https://raw.githubusercontent.com/necolas/normalize.css/8.0.1/normalize.css",
        size: 6_138,
        sha256: "580818700724d42d7fcc4979b0197971fca1c6d2e0286769237a0ac897df5512",
    },
    CorpusEntry {
        rel: "corpus/web/react-18.2.0.production.min.js",
        label: "react-min",
        url: "https://unpkg.com/react@18.2.0/umd/react.production.min.js",
        size: 10_737,
        sha256: "4b4969fa4ef3594324da2c6d78ce8766fbbc2fd121fff395aedf997db0a99a06",
    },
    CorpusEntry {
        rel: "corpus/web/preact-10.19.6.module.js",
        label: "preact",
        url: "https://unpkg.com/preact@10.19.6/dist/preact.module.js",
        size: 11_269,
        sha256: "73367fb7f14686b0e57c6eee59079799e3eae71f0382e5cdf8214edc789512b0",
    },
    CorpusEntry {
        rel: "corpus/web/vue-3.4.21.global.prod.js",
        label: "vue",
        url: "https://unpkg.com/vue@3.4.21/dist/vue.global.prod.js",
        size: 147_534,
        sha256: "4963101441ded7e420c05665e7c616b2f2e3851c99e1cf8af84d29d6f10e77da",
    },
    CorpusEntry {
        rel: "corpus/web/citm-catalog.json",
        label: "json-citm",
        url: "https://raw.githubusercontent.com/RichardHightower/json-parsers-benchmark/e6d09a817eafc50a5cad821e0743d565899639d9/data/citm_catalog.json",
        size: 1_727_204,
        sha256: "a73e7a883f6ea8de113dff59702975e60119b4b58d451d518a929f31c92e2059",
    },
    CorpusEntry {
        rel: "corpus/web/mdn-getting-started.html",
        label: "html-mdn-getting-started",
        url: "https://raw.githubusercontent.com/mdn/learning-area/180371ffc567f0f2ef359bf408b976667839e764/html/introduction-to-html/getting-started/index.html",
        size: 224,
        sha256: "fd7f1f3a8acffe21225a31986b1af90a267ce5cb1a827ad1e69ab3e961e1ea3c",
    },
    CorpusEntry {
        rel: "corpus/web/mdn-debug-example.html",
        label: "html-mdn-debug",
        url: "https://raw.githubusercontent.com/mdn/learning-area/180371ffc567f0f2ef359bf408b976667839e764/html/introduction-to-html/debugging-html/debug-example.html",
        size: 717,
        sha256: "bc8d3e7497a514f8d95d2109eda10f293c108d809db8c015dbee9bbb465816e2",
    },
    CorpusEntry {
        rel: "corpus/web/mdn-document-structure.html",
        label: "html-mdn-document",
        url: "https://raw.githubusercontent.com/mdn/learning-area/180371ffc567f0f2ef359bf408b976667839e764/html/introduction-to-html/document_and_website_structure/index.html",
        size: 3_525,
        sha256: "8623d0ff8d0696128844a2315d8c23840cf839d52fab09fcd06eabb3e5e966ec",
    },
    CorpusEntry {
        rel: "corpus/web/whatwg-html-source",
        label: "html-whatwg-source",
        url: "https://raw.githubusercontent.com/whatwg/html/ac0389a3aca0331055bf4bf23f509c2913e3f795/source",
        size: 7_891_621,
        sha256: "d7c570a3f5a29e559da5a9f57d57aabac787f31b7e42a8565dd5e8d1e7bece66",
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
         [--qualities LIST] [--files LIST] [--small-only] [--chart-small-only] \
         [--small-sizes LIST] [--quick] [--target-ms N] [--target-ns N] \
         [--rounds N] [--warmup N]"
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
        let parent = path.parent().ok_or("corpus path missing parent")?;
        fs::create_dir_all(parent)?;
        let status = Command::new("curl")
            .arg("-fL")
            .arg("--retry")
            .arg("3")
            .arg("-o")
            .arg(&path)
            .arg(entry.url)
            .status()?;
        if !status.success() {
            return Err(format!("curl failed for {}", entry.url).into());
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
