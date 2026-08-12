use std::{
    env,
    ffi::OsStr,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::Command,
};

struct FragmentedRead<'a> {
    input: &'a [u8],
    pos: usize,
    chunk: usize,
}

impl<'a> FragmentedRead<'a> {
    const fn new(input: &'a [u8], chunk: usize) -> Self {
        Self {
            input,
            pos: 0,
            chunk,
        }
    }
}

impl Read for FragmentedRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.input.len() {
            return Ok(0);
        }
        let count = buf.len().min(self.chunk).min(self.input.len() - self.pos);
        buf[..count].copy_from_slice(&self.input[self.pos..self.pos + count]);
        self.pos += count;
        Ok(count)
    }
}

#[test]
#[ignore = "downloads or uses upstream Brotli testdata"]
fn google_testdata_compressed_streams_decode() {
    let root = ensure_google_brotli_testdata().join("tests/testdata");
    let mut cases = compressed_cases(&root);
    if let Some(case) = env::var_os("BURLI_GOOGLE_BROTLI_CASE") {
        cases.retain(|path| path.file_name() == Some(case.as_os_str()));
    }
    cases.sort();
    assert!(!cases.is_empty(), "no compressed cases under {root:?}");

    for compressed_path in cases {
        let expected_path = expected_path(&compressed_path);
        let encoded = fs::read(&compressed_path).unwrap();
        let expected = fs::read(&expected_path).unwrap();
        let label = compressed_path.file_name().unwrap().to_string_lossy();

        let decoded = burli::decompress_with_limit(&encoded, expected.len())
            .unwrap_or_else(|error| panic!("{label} failed: {error:?}"));
        assert_eq!(decoded, expected, "{label} one-shot mismatch");
    }
}

#[test]
#[ignore = "downloads or uses upstream Brotli testdata"]
fn google_testdata_representative_streams_decode_fragmented() {
    let root = ensure_google_brotli_testdata().join("tests/testdata");
    let cases = [
        "10x10y.compressed",
        "backward65536.compressed",
        "quickfox_repeated.compressed",
    ];

    for case in cases {
        let compressed_path = root.join(case);
        let expected_path = expected_path(&compressed_path);
        let encoded = fs::read(&compressed_path).unwrap();
        let expected = fs::read(&expected_path).unwrap();
        for chunk in [1, 7, 64] {
            let source = FragmentedRead::new(&encoded, chunk);
            let mut decoder = burli::StreamDecoder::with_limit(source, expected.len());
            let mut streamed = Vec::new();

            decoder
                .read_to_end(&mut streamed)
                .unwrap_or_else(|error| panic!("{case} chunk {chunk} failed: {error:?}"));

            assert_eq!(streamed, expected, "{case} chunk {chunk} mismatch");
        }
    }
}

fn ensure_google_brotli_testdata() -> PathBuf {
    if let Some(path) = env::var_os("BURLI_GOOGLE_BROTLI_ROOT") {
        return PathBuf::from(path);
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/google-brotli");
    if root.join("tests/testdata").is_dir() {
        return root;
    }

    fs::create_dir_all(root.parent().unwrap()).unwrap();
    let status = Command::new("git")
        .args([
            OsStr::new("clone"),
            OsStr::new("--depth"),
            OsStr::new("1"),
            OsStr::new("https://github.com/google/brotli.git"),
            root.as_os_str(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "failed to clone upstream Brotli testdata");
    root
}

fn compressed_cases(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.contains(".compressed"))
        })
        .filter(|path| expected_path(path).is_file())
        .collect()
}

fn expected_path(compressed_path: &Path) -> PathBuf {
    let name = compressed_path.file_name().unwrap().to_string_lossy();
    let base = name.split(".compressed").next().unwrap();
    compressed_path.with_file_name(base)
}
