//! Burli concat fragments.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use burli_core::{
    BurliError, Mode, Options as BrotliOptions, Quality, Result,
    bits::{BitReader, BitWriter},
    format::{MAX_BLOCK_BITS, MAX_WINDOW_BITS, MIN_BLOCK_BITS, MIN_WINDOW_BITS},
};

const VERSION_MAJOR: u8 = 1;
const VERSION_MINOR: u8 = 0;
const MAGIC: &[u8; 8] = b"BURLICAT";
const HEADER_LEN: usize = 72;
const FLAG_NO_BACKWARD_REFERENCES: u32 = 1 << 0;
const FLAG_DICTIONARY_DISABLED: u32 = 1 << 1;
const FLAG_LOCAL_BACKWARD_REFERENCES: u32 = 1 << 2;
const FLAG_PRIOR_STATE_INDEPENDENT: u32 = 1 << 3;
const PAYLOAD_KIND_FLAGS: u32 = FLAG_NO_BACKWARD_REFERENCES | FLAG_LOCAL_BACKWARD_REFERENCES;
const REQUIRED_FLAGS: u32 = FLAG_DICTIONARY_DISABLED | FLAG_PRIOR_STATE_INDEPENDENT;
const ALLOWED_FLAGS: u32 = REQUIRED_FLAGS | PAYLOAD_KIND_FLAGS;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

const OFF_VERSION_MAJOR: usize = 8;
const OFF_VERSION_MINOR: usize = 9;
const OFF_HEADER_LEN: usize = 10;
const OFF_FLAGS: usize = 12;
const OFF_QUALITY: usize = 16;
const OFF_MODE: usize = 17;
const OFF_WINDOW_BITS: usize = 18;
const OFF_BLOCK_BITS: usize = 19;
const OFF_LARGE_WINDOW: usize = 20;
const OFF_DICTIONARY_POLICY: usize = 21;
const OFF_RESERVED_22: usize = 22;
const OFF_INPUT_LEN: usize = 24;
const OFF_PAYLOAD_LEN: usize = 32;
const OFF_PAYLOAD_BIT_LEN: usize = 40;
const OFF_FIRST_LEN: usize = 48;
const OFF_FIRST_BYTES: usize = 49;
const OFF_LAST_LEN: usize = 51;
const OFF_LAST_BYTES: usize = 52;
const OFF_RESERVED_54: usize = 54;
const OFF_CHECKSUM: usize = 56;
const OFF_HEADER_CHECKSUM: usize = 64;

/// Limits and validation behavior for concat APIs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Options {
    input_ceiling: usize,
    payload_budget: usize,
    assembled_cap: usize,
}

impl Options {
    /// Build options with no explicit limits.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            input_ceiling: usize::MAX,
            payload_budget: usize::MAX,
            assembled_cap: usize::MAX,
        }
    }

    /// Set maximum decoded bytes allowed for one fragment.
    #[must_use]
    pub const fn with_max_fragment_input_len(mut self, limit: usize) -> Self {
        self.input_ceiling = limit;
        self
    }

    /// Set maximum encoded payload bytes allowed for one fragment.
    #[must_use]
    pub const fn with_max_fragment_payload_len(mut self, limit: usize) -> Self {
        self.payload_budget = limit;
        self
    }

    /// Set maximum decoded bytes allowed across an assembled stream.
    #[must_use]
    pub const fn with_max_assembled_input_len(mut self, limit: usize) -> Self {
        self.assembled_cap = limit;
        self
    }

    /// Return maximum decoded bytes allowed for one fragment.
    pub const fn max_fragment_input_len(&self) -> usize {
        self.input_ceiling
    }

    /// Return maximum encoded payload bytes allowed for one fragment.
    pub const fn max_fragment_payload_len(&self) -> usize {
        self.payload_budget
    }

    /// Return maximum decoded bytes allowed across an assembled stream.
    pub const fn max_assembled_input_len(&self) -> usize {
        self.assembled_cap
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

/// Dictionary policy for concat fragments.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum DictionaryPolicy {
    /// Version 1 fragments do not use any dictionary.
    #[default]
    Disabled,
}

/// Parameters shared by all fragments in one assembled Brotli stream.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ConcatSpec {
    quality: Quality,
    mode: Mode,
    window_bits: u8,
    block_bits: Option<u8>,
    large_window: bool,
    dictionary_policy: DictionaryPolicy,
}

impl ConcatSpec {
    /// Build a concat spec using standard Brotli window bits.
    ///
    /// # Errors
    ///
    /// Returns an error when `window_bits` is outside 10 through 24.
    pub fn new(quality: Quality, window_bits: u8) -> Result<Self> {
        if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&window_bits) {
            return Err(BurliError::InvalidWindowBits(window_bits));
        }
        Ok(Self {
            quality,
            mode: Mode::Generic,
            window_bits,
            block_bits: None,
            large_window: false,
            dictionary_policy: DictionaryPolicy::Disabled,
        })
    }

    /// Set input mode hint.
    #[must_use]
    pub const fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    /// Set meta-block size override.
    ///
    /// # Errors
    ///
    /// Returns an error when `block_bits` is outside the Brotli range.
    pub fn with_block_bits(mut self, block_bits: Option<u8>) -> Result<Self> {
        self.options()?.with_block_bits(block_bits)?;
        self.block_bits = block_bits;
        Ok(self)
    }

    /// Return configured quality.
    pub const fn quality(&self) -> Quality {
        self.quality
    }

    /// Return configured input mode.
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// Return configured Brotli window bits.
    pub const fn window_bits(&self) -> u8 {
        self.window_bits
    }

    /// Return configured meta-block bits.
    pub const fn block_bits(&self) -> Option<u8> {
        self.block_bits
    }

    /// Return whether large-window output is enabled.
    pub const fn large_window(&self) -> bool {
        self.large_window
    }

    /// Return dictionary policy.
    pub const fn dictionary_policy(&self) -> DictionaryPolicy {
        self.dictionary_policy
    }

    fn options(&self) -> Result<BrotliOptions> {
        if self.large_window {
            return Err(BurliError::Unsupported(
                "large-window concat fragments are not implemented",
            ));
        }
        let options = BrotliOptions::default()
            .with_quality(self.quality.get())?
            .with_window_bits(self.window_bits)?
            .with_block_bits(self.block_bits)?
            .with_mode(self.mode);
        Ok(options)
    }
}

const fn mode_wire_value(mode: Mode) -> u8 {
    match mode {
        Mode::Text => 1,
        Mode::Font => 2,
        _ => 0,
    }
}

fn mode_from_wire(value: u8) -> Result<Mode> {
    match value {
        0 => Ok(Mode::Generic),
        1 => Ok(Mode::Text),
        2 => Ok(Mode::Font),
        _ => Err(BurliError::Format("invalid concat fragment mode")),
    }
}

const fn dictionary_policy_wire_value(policy: DictionaryPolicy) -> u8 {
    match policy {
        DictionaryPolicy::Disabled => 0,
    }
}

fn dictionary_policy_from_wire(value: u8) -> Result<DictionaryPolicy> {
    match value {
        0 => Ok(DictionaryPolicy::Disabled),
        _ => Err(BurliError::Format(
            "invalid concat fragment dictionary policy",
        )),
    }
}

/// Sidecar metadata for one concat fragment.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FragmentMetadata {
    version_major: u8,
    version_minor: u8,
    spec: ConcatSpec,
    input_len: usize,
    payload_len: usize,
    payload_bit_len: usize,
    first_bytes: [u8; 2],
    first_len: u8,
    last_bytes: [u8; 2],
    last_len: u8,
    checksum: u64,
    flags: u32,
}

impl FragmentMetadata {
    /// Return major format version.
    pub const fn version_major(&self) -> u8 {
        self.version_major
    }

    /// Return minor format version.
    pub const fn version_minor(&self) -> u8 {
        self.version_minor
    }

    /// Return fragment spec.
    pub const fn spec(&self) -> &ConcatSpec {
        &self.spec
    }

    /// Return decoded byte length.
    pub const fn input_len(&self) -> usize {
        self.input_len
    }

    /// Return payload byte length.
    pub const fn payload_len(&self) -> usize {
        self.payload_len
    }

    /// Return valid payload bit length.
    pub const fn payload_bit_len(&self) -> usize {
        self.payload_bit_len
    }

    /// Return first decoded bytes, up to two.
    pub fn first_bytes(&self) -> &[u8] {
        &self.first_bytes[..usize::from(self.first_len)]
    }

    /// Return last decoded bytes, up to two.
    pub fn last_bytes(&self) -> &[u8] {
        &self.last_bytes[..usize::from(self.last_len)]
    }

    /// Return payload checksum.
    pub const fn checksum(&self) -> u64 {
        self.checksum
    }

    /// Return metadata flags.
    pub const fn flags(&self) -> u32 {
        self.flags
    }
}

/// Headerless concat fragment plus sidecar metadata.
#[cfg(feature = "alloc")]
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ConcatFragment {
    metadata: FragmentMetadata,
    payload: Vec<u8>,
}

#[cfg(feature = "alloc")]
impl ConcatFragment {
    /// Return sidecar metadata.
    pub const fn metadata(&self) -> &FragmentMetadata {
        &self.metadata
    }

    /// Return payload bytes.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Split into metadata and payload.
    #[must_use]
    pub fn into_parts(self) -> (FragmentMetadata, Vec<u8>) {
        (self.metadata, self.payload)
    }

    /// Build a fragment from metadata and payload, validating both.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata and payload disagree or configured limits
    /// are exceeded.
    pub fn from_parts(
        metadata: FragmentMetadata,
        payload: Vec<u8>,
        options: &Options,
    ) -> Result<Self> {
        validate_fragment_parts(&metadata, &payload, options)?;
        Ok(Self { metadata, payload })
    }

    /// Serialize this fragment into the binary format described in `FORMAT.md`.
    ///
    /// # Errors
    ///
    /// Returns an error if the fragment is internally invalid or configured
    /// limits are exceeded.
    pub fn to_bytes(&self, options: &Options) -> Result<Vec<u8>> {
        validate_fragment_parts(&self.metadata, &self.payload, options)?;
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        let mut header = encode_header(&self.metadata)?;
        out.append(&mut header);
        out.extend_from_slice(&self.payload);
        Ok(out)
    }

    /// Parse a serialized concat fragment.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed headers, checksum mismatch, invalid
    /// fragment payload, or configured limits.
    pub fn from_bytes(input: &[u8], options: &Options) -> Result<Self> {
        if input.len() < HEADER_LEN {
            return Err(BurliError::Format("concat fragment header truncated"));
        }
        let metadata = decode_header(&input[..HEADER_LEN])?;
        let expected_len = HEADER_LEN
            .checked_add(metadata.payload_len)
            .ok_or(BurliError::Format("concat fragment length overflow"))?;
        if input.len() != expected_len {
            return Err(BurliError::Format(
                "serialized concat fragment length mismatch",
            ));
        }
        validate_fragment_metadata_limits(&metadata, options)?;
        Self::from_parts(metadata, input[HEADER_LEN..].to_vec(), options)
    }
}

/// Encode one headerless concat fragment.
///
/// # Errors
///
/// Returns an error for unsupported encoder options or configured limits.
#[cfg(feature = "alloc")]
pub fn encode_fragment(
    input: &[u8],
    spec: &ConcatSpec,
    options: &Options,
) -> Result<ConcatFragment> {
    validate_input_len_limit(input.len(), options.input_ceiling)?;
    let brotli_options = spec.options()?;
    let (payload, payload_bit_len, has_copy) =
        burli_encode::encode_concat_fragment_with_options(input, &brotli_options)?;
    validate_payload_len_limit(payload.len(), options.payload_budget)?;
    let payload_kind = if has_copy {
        FLAG_LOCAL_BACKWARD_REFERENCES
    } else {
        FLAG_NO_BACKWARD_REFERENCES
    };
    let metadata = FragmentMetadata {
        version_major: VERSION_MAJOR,
        version_minor: VERSION_MINOR,
        spec: spec.clone(),
        input_len: input.len(),
        payload_len: payload.len(),
        payload_bit_len,
        first_bytes: two_prefix(input),
        first_len: input.len().min(2) as u8,
        last_bytes: two_suffix(input),
        last_len: input.len().min(2) as u8,
        checksum: checksum64(&payload),
        flags: REQUIRED_FLAGS | payload_kind,
    };
    validate_fragment_parts(&metadata, &payload, options)?;
    Ok(ConcatFragment { metadata, payload })
}

/// Assemble fragments into one normal Brotli stream.
#[cfg(feature = "alloc")]
#[derive(Clone, Debug)]
pub struct ConcatAssembler {
    spec: ConcatSpec,
    options: Options,
    fragments: Vec<ConcatFragment>,
}

#[cfg(feature = "alloc")]
impl ConcatAssembler {
    /// Create an assembler for a spec.
    #[must_use]
    pub fn new(spec: &ConcatSpec, options: &Options) -> Self {
        Self {
            spec: spec.clone(),
            options: *options,
            fragments: Vec::new(),
        }
    }

    /// Validate and stage one fragment.
    ///
    /// # Errors
    ///
    /// Returns an error if the fragment does not match this assembler.
    pub fn push(&mut self, fragment: ConcatFragment) -> Result<&mut Self> {
        validate_fragment_for_spec(&self.spec, &fragment, &self.options)?;
        validate_assembled_input_len(
            self.fragments
                .iter()
                .map(|fragment| fragment.metadata.input_len)
                .chain(core::iter::once(fragment.metadata.input_len)),
            self.options.assembled_cap,
        )?;
        self.fragments.push(fragment);
        Ok(self)
    }

    /// Validate and stage several fragments.
    ///
    /// # Errors
    ///
    /// Returns an error if any fragment does not match this assembler.
    pub fn push_all<I>(&mut self, fragments: I) -> Result<&mut Self>
    where
        I: IntoIterator<Item = ConcatFragment>,
    {
        for fragment in fragments {
            self.push(fragment)?;
        }
        Ok(self)
    }

    /// Return staged fragment count.
    pub fn len(&self) -> usize {
        self.fragments.len()
    }

    /// Return true when no fragments are staged.
    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }

    /// Finish into `output`, appending bytes only after validation succeeds.
    ///
    /// # Errors
    ///
    /// Returns an error if validation or bit assembly fails.
    pub fn finish(&self, output: &mut Vec<u8>) -> Result<usize> {
        assemble_fragments(&self.spec, &self.fragments, output, &self.options)
    }
}

/// Assemble fragments into `output`.
///
/// # Errors
///
/// Returns an error if any fragment is invalid, mismatched, or over a
/// configured limit.
#[cfg(feature = "alloc")]
pub fn assemble_fragments(
    spec: &ConcatSpec,
    fragments: &[ConcatFragment],
    output: &mut Vec<u8>,
    options: &Options,
) -> Result<usize> {
    for fragment in fragments {
        validate_fragment_for_spec(spec, fragment, options)?;
    }
    validate_assembled_input_len(
        fragments.iter().map(|fragment| fragment.metadata.input_len),
        options.assembled_cap,
    )?;

    let brotli_options = spec.options()?;
    let mut writer = BitWriter::new();
    burli_encode::write_concat_stream_header(&mut writer, &brotli_options)?;
    for fragment in fragments {
        write_payload_bits(
            &mut writer,
            &fragment.payload,
            fragment.metadata.payload_bit_len,
        )?;
    }
    burli_encode::write_concat_stream_trailer(&mut writer)?;
    Ok(writer.finish_into(output))
}

#[cfg(feature = "alloc")]
fn validate_fragment_for_spec(
    spec: &ConcatSpec,
    fragment: &ConcatFragment,
    concat_options: &Options,
) -> Result<()> {
    if fragment.metadata.spec != *spec {
        return Err(BurliError::Format("concat fragment spec mismatch"));
    }
    validate_fragment_parts(&fragment.metadata, &fragment.payload, concat_options)
}

#[cfg(feature = "alloc")]
fn validate_fragment_parts(
    metadata: &FragmentMetadata,
    payload: &[u8],
    concat_options: &Options,
) -> Result<()> {
    if metadata.version_major != VERSION_MAJOR {
        return Err(BurliError::Format("unsupported concat fragment version"));
    }
    if metadata.spec.large_window {
        return Err(BurliError::Unsupported(
            "large-window concat fragments are not implemented",
        ));
    }
    if metadata.payload_len != payload.len() {
        return Err(BurliError::Format(
            "concat fragment payload length mismatch",
        ));
    }
    validate_fragment_metadata_limits(metadata, concat_options)?;
    if metadata.flags & !ALLOWED_FLAGS != 0 {
        return Err(BurliError::Format("concat fragment unknown flags set"));
    }
    let payload_kind = metadata.flags & PAYLOAD_KIND_FLAGS;
    if payload_kind != FLAG_NO_BACKWARD_REFERENCES && payload_kind != FLAG_LOCAL_BACKWARD_REFERENCES
    {
        return Err(BurliError::Format(
            "concat fragment payload kind is invalid",
        ));
    }
    if metadata.payload_bit_len > payload.len().saturating_mul(8) {
        return Err(BurliError::Format(
            "concat fragment bit length exceeds payload",
        ));
    }
    if metadata.payload_bit_len == 0 && !payload.is_empty() {
        return Err(BurliError::Format(
            "concat fragment has unused payload bytes",
        ));
    }
    if metadata.payload_bit_len != 0 && metadata.payload_bit_len.div_ceil(8) != payload.len() {
        return Err(BurliError::Format("concat fragment payload is not minimal"));
    }
    if metadata.first_len > 2 || metadata.last_len > 2 {
        return Err(BurliError::Format(
            "concat fragment byte summary is invalid",
        ));
    }
    if metadata.first_bytes[usize::from(metadata.first_len)..]
        .iter()
        .any(|&byte| byte != 0)
        || metadata.last_bytes[usize::from(metadata.last_len)..]
            .iter()
            .any(|&byte| byte != 0)
    {
        return Err(BurliError::Format(
            "concat fragment unused byte summary data is non-zero",
        ));
    }
    if usize::from(metadata.first_len) != metadata.input_len.min(2)
        || usize::from(metadata.last_len) != metadata.input_len.min(2)
    {
        return Err(BurliError::Format(
            "concat fragment byte summary length mismatch",
        ));
    }
    if metadata.checksum != checksum64(payload) {
        return Err(BurliError::Format("concat fragment checksum mismatch"));
    }
    if metadata.flags & REQUIRED_FLAGS != REQUIRED_FLAGS {
        return Err(BurliError::Format("concat fragment required flags missing"));
    }
    validate_decoded_summary(metadata, payload, concat_options)?;
    Ok(())
}

/// Encode independent input parts into one RFC 7932 self-contained stream.
///
/// Each part starts from local Brotli history, cannot refer to an earlier
/// part, and ends on a byte boundary. The returned stream is standard Brotli;
/// no Burli-specific wrapper is added.
///
/// This function does not create or manage threads.
#[cfg(feature = "alloc")]
pub fn assemble_rfc7932_parts(
    spec: &ConcatSpec,
    inputs: &[&[u8]],
    output: &mut Vec<u8>,
) -> Result<()> {
    let brotli_options = spec.options()?;
    let mut writer = BitWriter::new();
    burli_encode::write_concat_stream_header(&mut writer, &brotli_options)?;
    for input in inputs {
        burli_encode::encode_concat_fragment_into_writer(
            input,
            &brotli_options,
            &mut writer,
            true,
        )?;
    }
    burli_encode::write_concat_stream_trailer(&mut writer)?;
    output.extend(writer.into_bytes());
    Ok(())
}

#[cfg(feature = "alloc")]
fn validate_decoded_summary(
    metadata: &FragmentMetadata,
    payload: &[u8],
    concat_options: &Options,
) -> Result<()> {
    let (decoded, has_copy) = burli_decode::decompress_concat_payload_with_limit(
        payload,
        metadata.payload_bit_len,
        metadata.spec.window_bits,
        concat_options.input_ceiling,
    )?;
    let expected_kind = if has_copy {
        FLAG_LOCAL_BACKWARD_REFERENCES
    } else {
        FLAG_NO_BACKWARD_REFERENCES
    };
    if metadata.flags & PAYLOAD_KIND_FLAGS != expected_kind {
        return Err(BurliError::Format("concat fragment payload kind mismatch"));
    }
    if decoded.len() != metadata.input_len {
        return Err(BurliError::Format(
            "concat fragment decoded length mismatch",
        ));
    }
    if two_prefix(&decoded) != metadata.first_bytes || two_suffix(&decoded) != metadata.last_bytes {
        return Err(BurliError::Format("concat fragment byte summary mismatch"));
    }
    Ok(())
}

fn validate_fragment_metadata_limits(metadata: &FragmentMetadata, options: &Options) -> Result<()> {
    validate_input_len_limit(metadata.input_len, options.input_ceiling)?;
    validate_payload_len_limit(metadata.payload_len, options.payload_budget)
}

fn validate_input_len_limit(needed: usize, limit: usize) -> Result<()> {
    if needed > limit {
        return Err(BurliError::OutputLimitExceeded { limit, needed });
    }
    Ok(())
}

fn validate_payload_len_limit(needed: usize, limit: usize) -> Result<()> {
    if needed > limit {
        return Err(BurliError::Format(
            "concat fragment payload length exceeds configured limit",
        ));
    }
    Ok(())
}

fn validate_assembled_input_len<I>(lengths: I, limit: usize) -> Result<()>
where
    I: IntoIterator<Item = usize>,
{
    let mut needed = 0_usize;
    for len in lengths {
        needed = needed
            .checked_add(len)
            .ok_or(BurliError::Format("concat assembled input length overflow"))?;
        if needed > limit {
            return Err(BurliError::OutputLimitExceeded { limit, needed });
        }
    }
    Ok(())
}

#[cfg(feature = "alloc")]
fn encode_header(metadata: &FragmentMetadata) -> Result<Vec<u8>> {
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.push(metadata.version_major);
    header.push(metadata.version_minor);
    put_u16(&mut header, HEADER_LEN as u16);
    put_u32(&mut header, metadata.flags);
    header.push(metadata.spec.quality.get());
    header.push(mode_wire_value(metadata.spec.mode));
    header.push(metadata.spec.window_bits);
    header.push(metadata.spec.block_bits.unwrap_or(0));
    header.push(u8::from(metadata.spec.large_window));
    header.push(dictionary_policy_wire_value(
        metadata.spec.dictionary_policy,
    ));
    put_u16(&mut header, 0);
    put_usize_as_u64(&mut header, metadata.input_len)?;
    put_usize_as_u64(&mut header, metadata.payload_len)?;
    put_usize_as_u64(&mut header, metadata.payload_bit_len)?;
    header.push(metadata.first_len);
    header.extend_from_slice(&metadata.first_bytes);
    header.push(metadata.last_len);
    header.extend_from_slice(&metadata.last_bytes);
    put_u16(&mut header, 0);
    put_u64(&mut header, metadata.checksum);
    let header_checksum = checksum64(&header);
    header.extend_from_slice(&header_checksum.to_le_bytes());
    debug_assert_eq!(header.len(), HEADER_LEN);
    Ok(header)
}

#[cfg(feature = "alloc")]
fn decode_header(header: &[u8]) -> Result<FragmentMetadata> {
    if header.len() != HEADER_LEN {
        return Err(BurliError::Format("invalid concat fragment header length"));
    }
    if &header[..8] != MAGIC {
        return Err(BurliError::Format("invalid concat fragment magic"));
    }
    if checksum64(&header[..OFF_HEADER_CHECKSUM]) != read_u64(header, OFF_HEADER_CHECKSUM)? {
        return Err(BurliError::Format(
            "concat fragment header checksum mismatch",
        ));
    }
    let version_major = header[OFF_VERSION_MAJOR];
    let version_minor = header[OFF_VERSION_MINOR];
    if version_major != VERSION_MAJOR {
        return Err(BurliError::Format("unsupported concat fragment version"));
    }
    if read_u16(header, OFF_HEADER_LEN)? != HEADER_LEN as u16 {
        return Err(BurliError::Format(
            "unsupported concat fragment header length",
        ));
    }
    if read_u16(header, OFF_RESERVED_22)? != 0 || read_u16(header, OFF_RESERVED_54)? != 0 {
        return Err(BurliError::Format(
            "concat fragment reserved bytes are non-zero",
        ));
    }

    let block_bits = match header[OFF_BLOCK_BITS] {
        0 => None,
        bits if (MIN_BLOCK_BITS..=MAX_BLOCK_BITS).contains(&bits) => Some(bits),
        _ => return Err(BurliError::Format("invalid concat fragment block bits")),
    };
    let large_window = match header[OFF_LARGE_WINDOW] {
        0 => false,
        1 => true,
        _ => {
            return Err(BurliError::Format(
                "invalid concat fragment large-window flag",
            ));
        }
    };
    let window_bits = header[OFF_WINDOW_BITS];
    if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&window_bits) {
        return Err(BurliError::InvalidWindowBits(window_bits));
    }

    let mut first_bytes = [0; 2];
    first_bytes.copy_from_slice(&header[OFF_FIRST_BYTES..OFF_LAST_LEN]);
    let mut last_bytes = [0; 2];
    last_bytes.copy_from_slice(&header[OFF_LAST_BYTES..OFF_RESERVED_54]);

    Ok(FragmentMetadata {
        version_major,
        version_minor,
        spec: ConcatSpec {
            quality: Quality::new(header[OFF_QUALITY])?,
            mode: mode_from_wire(header[OFF_MODE])?,
            window_bits,
            block_bits,
            large_window,
            dictionary_policy: dictionary_policy_from_wire(header[OFF_DICTIONARY_POLICY])?,
        },
        input_len: read_usize(
            header,
            OFF_INPUT_LEN,
            "concat fragment input length exceeds usize",
        )?,
        payload_len: read_usize(
            header,
            OFF_PAYLOAD_LEN,
            "concat fragment payload length exceeds usize",
        )?,
        payload_bit_len: read_usize(
            header,
            OFF_PAYLOAD_BIT_LEN,
            "concat fragment bit length exceeds usize",
        )?,
        first_bytes,
        first_len: header[OFF_FIRST_LEN],
        last_bytes,
        last_len: header[OFF_LAST_LEN],
        checksum: read_u64(header, OFF_CHECKSUM)?,
        flags: read_u32(header, OFF_FLAGS)?,
    })
}

#[cfg(feature = "alloc")]
fn put_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

#[cfg(feature = "alloc")]
fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

#[cfg(feature = "alloc")]
fn put_usize_as_u64(output: &mut Vec<u8>, value: usize) -> Result<()> {
    let value = u64::try_from(value)
        .map_err(|_| BurliError::Format("concat fragment integer exceeds u64"))?;
    output.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

#[cfg(feature = "alloc")]
fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn read_u16(input: &[u8], offset: usize) -> Result<u16> {
    let bytes = input
        .get(offset..offset + 2)
        .ok_or(BurliError::Format("concat fragment header truncated"))?;
    Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u32(input: &[u8], offset: usize) -> Result<u32> {
    let bytes = input
        .get(offset..offset + 4)
        .ok_or(BurliError::Format("concat fragment header truncated"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u64(input: &[u8], offset: usize) -> Result<u64> {
    let bytes = input
        .get(offset..offset + 8)
        .ok_or(BurliError::Format("concat fragment header truncated"))?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_usize(input: &[u8], offset: usize, error: &'static str) -> Result<usize> {
    usize::try_from(read_u64(input, offset)?).map_err(|_| BurliError::Format(error))
}

fn write_payload_bits(writer: &mut BitWriter, payload: &[u8], bit_len: usize) -> Result<()> {
    let mut reader = BitReader::with_bit_pos(payload, 0)?;
    let mut remaining = bit_len;
    while remaining != 0 {
        let width = remaining.min(usize::from(burli_core::bits::MAX_BITS_PER_OP)) as u8;
        let bits = reader.read_bits(width)?;
        writer.write_bits(width, bits)?;
        remaining -= usize::from(width);
    }
    Ok(())
}

fn two_prefix(input: &[u8]) -> [u8; 2] {
    let mut out = [0; 2];
    for (slot, &byte) in out.iter_mut().zip(input.iter()) {
        *slot = byte;
    }
    out
}

fn two_suffix(input: &[u8]) -> [u8; 2] {
    let mut out = [0; 2];
    let len = input.len().min(2);
    if len != 0 {
        out[..len].copy_from_slice(&input[input.len() - len..]);
    }
    out
}

fn checksum64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    #[cfg(feature = "std")]
    use std::io::Read;

    fn spec(quality: u8) -> ConcatSpec {
        ConcatSpec::new(Quality::new(quality).unwrap(), 22).unwrap()
    }

    #[cfg(feature = "std")]
    fn decode_with_rust_brotli(encoded: &[u8]) -> Vec<u8> {
        let mut decoder = rust_brotli::Decompressor::new(encoded, 4096);
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded).unwrap();
        decoded
    }

    fn metadata_for_payload(
        payload: &[u8],
        payload_bit_len: usize,
        input_len: usize,
        payload_kind: u32,
    ) -> FragmentMetadata {
        let (mut metadata, _) = encode_fragment(b"", &spec(1), &Options::new())
            .unwrap()
            .into_parts();
        metadata.input_len = input_len;
        metadata.payload_len = payload.len();
        metadata.payload_bit_len = payload_bit_len;
        metadata.first_bytes = [0; 2];
        metadata.first_len = 0;
        metadata.last_bytes = [0; 2];
        metadata.last_len = 0;
        metadata.checksum = checksum64(payload);
        metadata.flags = REQUIRED_FLAGS | payload_kind;
        metadata
    }

    fn serialized_with_header(metadata: &FragmentMetadata, payload: &[u8]) -> Vec<u8> {
        let mut bytes = encode_header(metadata).unwrap();
        bytes.extend_from_slice(payload);
        bytes
    }

    fn rewrite_header_checksum(bytes: &mut [u8]) {
        let checksum = checksum64(&bytes[..64]).to_le_bytes();
        bytes[64..72].copy_from_slice(&checksum);
    }

    #[test]
    fn assembles_no_fragments_as_empty_brotli_stream() {
        let mut output = Vec::new();
        assemble_fragments(&spec(0), &[], &mut output, &Options::new()).unwrap();

        assert_eq!(burli_decode::decompress(&output).unwrap(), b"");
        #[cfg(feature = "std")]
        assert_eq!(decode_with_rust_brotli(&output), b"");
    }

    #[test]
    fn assembles_literal_fragments_in_order() {
        let spec = spec(5);
        let inputs = [
            b"alpha alpha alpha ".as_slice(),
            b"beta beta beta ".as_slice(),
            b"gamma gamma gamma".as_slice(),
        ];
        let fragments = inputs
            .iter()
            .map(|input| encode_fragment(input, &spec, &Options::new()).unwrap())
            .collect::<Vec<_>>();

        let mut encoded = Vec::new();
        ConcatAssembler::new(&spec, &Options::new())
            .push_all(fragments)
            .unwrap()
            .finish(&mut encoded)
            .unwrap();

        let expected = inputs.concat();
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), expected);
        #[cfg(feature = "std")]
        assert_eq!(decode_with_rust_brotli(&encoded), expected);
    }

    #[test]
    #[cfg(feature = "std")]
    fn assembles_rfc7932_self_contained_parts() {
        let inputs = [
            b"first part ".repeat(4096),
            b"second part ".repeat(4096),
            b"third part".repeat(4096),
        ];
        let views = inputs.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let mut encoded = Vec::new();

        assemble_rfc7932_parts(&spec(5), &views, &mut encoded).unwrap();

        let expected = inputs.concat();
        assert_eq!(decode_with_rust_brotli(&encoded), expected);
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), expected);
    }

    #[test]
    fn rfc7932_parts_do_not_leak_prior_history() {
        let spec = spec(5);
        let options = spec.options().unwrap();
        let first_a = b"part-a-prefix-".repeat(4096);
        let first_b = b"part-b-prefix-".repeat(4096);
        let second = first_a.clone();

        let mut expected_second = BitWriter::new();
        burli_encode::encode_concat_fragment_into_writer(
            &second,
            &options,
            &mut expected_second,
            true,
        )
        .unwrap();
        burli_encode::write_concat_stream_trailer(&mut expected_second).unwrap();
        let expected_second = expected_second.into_bytes();

        for first in [&first_a[..], &first_b[..]] {
            let mut prefix = BitWriter::new();
            burli_encode::write_concat_stream_header(&mut prefix, &options).unwrap();
            burli_encode::encode_concat_fragment_into_writer(first, &options, &mut prefix, true)
                .unwrap();
            assert_eq!(prefix.written_bits() % 8, 0);
            let prefix = prefix.into_bytes();

            let mut assembled = Vec::new();
            assemble_rfc7932_parts(&spec, &[first, &second], &mut assembled).unwrap();
            assert_eq!(&assembled[prefix.len()..], expected_second.as_slice());
        }
    }

    #[test]
    fn repeated_fragments_use_local_copy_compression() {
        let input = b"abcdefghabcdefghabcdefghabcdefgh".repeat(1024);

        for quality in 0..=5 {
            let spec = spec(quality);
            let fragment = encode_fragment(&input, &spec, &Options::new()).unwrap();

            assert_eq!(
                fragment.metadata().flags() & PAYLOAD_KIND_FLAGS,
                FLAG_LOCAL_BACKWARD_REFERENCES
            );
            assert!(fragment.payload().len() < input.len() / 4);

            let mut encoded = Vec::new();
            assemble_fragments(&spec, &[fragment], &mut encoded, &Options::new()).unwrap();
            assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
            #[cfg(feature = "std")]
            assert_eq!(decode_with_rust_brotli(&encoded), input);
        }
    }

    #[test]
    fn fragment_payload_is_not_standalone_brotli() {
        let fragment = encode_fragment(b"not a stream", &spec(1), &Options::new()).unwrap();

        assert!(burli_decode::decompress(fragment.payload()).is_err());
    }

    #[test]
    fn empty_and_tiny_fragments_round_trip() {
        let spec = spec(3);
        let inputs = [
            b"".as_slice(),
            b"a".as_slice(),
            b"bc".as_slice(),
            b"def".as_slice(),
        ];
        let fragments = inputs
            .iter()
            .map(|input| encode_fragment(input, &spec, &Options::new()).unwrap())
            .collect::<Vec<_>>();

        let mut encoded = Vec::new();
        assemble_fragments(&spec, &fragments, &mut encoded, &Options::new()).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), inputs.concat());
    }

    #[test]
    fn serialized_fragment_has_documented_wire_header() {
        let fragment = encode_fragment(b"wire", &spec(2), &Options::new()).unwrap();
        let bytes = fragment.to_bytes(&Options::new()).unwrap();
        let metadata = fragment.metadata();

        assert_eq!(&bytes[..8], b"BURLICAT");
        assert_eq!(bytes[8], 1);
        assert_eq!(bytes[9], 0);
        assert_eq!(u16::from_le_bytes(bytes[10..12].try_into().unwrap()), 72);
        assert_eq!(
            u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            REQUIRED_FLAGS | FLAG_NO_BACKWARD_REFERENCES
        );
        assert_eq!(bytes[16], 2);
        assert_eq!(bytes[17], 0);
        assert_eq!(bytes[18], 22);
        assert_eq!(bytes[19], 0);
        assert_eq!(bytes[20], 0);
        assert_eq!(bytes[21], 0);
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 0);
        assert_eq!(u64::from_le_bytes(bytes[24..32].try_into().unwrap()), 4);
        assert_eq!(
            u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
            metadata.payload_len() as u64
        );
        assert_eq!(
            u64::from_le_bytes(bytes[40..48].try_into().unwrap()),
            metadata.payload_bit_len() as u64
        );
        assert_eq!(bytes[48], 2);
        assert_eq!(&bytes[49..51], b"wi");
        assert_eq!(bytes[51], 2);
        assert_eq!(&bytes[52..54], b"re");
        assert_eq!(u16::from_le_bytes(bytes[54..56].try_into().unwrap()), 0);
        assert_eq!(
            u64::from_le_bytes(bytes[56..64].try_into().unwrap()),
            metadata.checksum()
        );
        assert_eq!(bytes.len(), 72 + metadata.payload_len());
    }

    #[test]
    fn serialized_fragment_round_trips() {
        let fragment = encode_fragment(b"serialized fragment", &spec(4), &Options::new()).unwrap();
        let parsed = ConcatFragment::from_bytes(
            &fragment.to_bytes(&Options::new()).unwrap(),
            &Options::new(),
        )
        .unwrap();

        assert_eq!(parsed, fragment);
    }

    #[test]
    fn options_limit_encoded_fragment_input() {
        let options = Options::new().with_max_fragment_input_len(3);

        assert!(matches!(
            encode_fragment(b"four", &spec(1), &options),
            Err(BurliError::OutputLimitExceeded {
                limit: 3,
                needed: 4
            })
        ));
    }

    #[test]
    fn options_limit_parsed_fragment_payload_before_copy() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let bytes = fragment.to_bytes(&Options::new()).unwrap();
        let options =
            Options::new().with_max_fragment_payload_len(fragment.metadata().payload_len() - 1);

        assert!(matches!(
            ConcatFragment::from_bytes(&bytes, &options),
            Err(BurliError::Format(
                "concat fragment payload length exceeds configured limit"
            ))
        ));
    }

    #[test]
    fn options_limit_parsed_fragment_decoded_len_before_decode() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let bytes = fragment.to_bytes(&Options::new()).unwrap();
        let options =
            Options::new().with_max_fragment_input_len(fragment.metadata().input_len() - 1);

        assert!(matches!(
            ConcatFragment::from_bytes(&bytes, &options),
            Err(BurliError::OutputLimitExceeded {
                limit: 6,
                needed: 7
            })
        ));
    }

    #[test]
    fn options_limit_assembled_input_before_output_mutation() {
        let spec = spec(2);
        let first = encode_fragment(b"first", &spec, &Options::new()).unwrap();
        let second = encode_fragment(b"second", &spec, &Options::new()).unwrap();
        let options = Options::new().with_max_assembled_input_len(10);
        let mut output = b"prefix".to_vec();

        assert!(matches!(
            assemble_fragments(&spec, &[first, second], &mut output, &options),
            Err(BurliError::OutputLimitExceeded {
                limit: 10,
                needed: 11
            })
        ));
        assert_eq!(output, b"prefix");
    }

    #[test]
    fn rejects_corrupt_serialized_header_checksum() {
        let fragment = encode_fragment(b"serialized fragment", &spec(4), &Options::new()).unwrap();
        let mut bytes = fragment.to_bytes(&Options::new()).unwrap();
        bytes[16] ^= 1;

        assert!(matches!(
            ConcatFragment::from_bytes(&bytes, &Options::new()),
            Err(BurliError::Format(
                "concat fragment header checksum mismatch"
            ))
        ));
    }

    #[test]
    fn rejects_corrupt_serialized_payload_checksum() {
        let fragment = encode_fragment(b"serialized fragment", &spec(4), &Options::new()).unwrap();
        let mut bytes = fragment.to_bytes(&Options::new()).unwrap();
        let last = bytes.last_mut().unwrap();
        *last ^= 1;

        assert!(matches!(
            ConcatFragment::from_bytes(&bytes, &Options::new()),
            Err(BurliError::Format("concat fragment checksum mismatch"))
        ));
    }

    #[test]
    fn rejects_serialized_reserved_bytes() {
        let fragment = encode_fragment(b"serialized fragment", &spec(4), &Options::new()).unwrap();
        let mut bytes = fragment.to_bytes(&Options::new()).unwrap();
        bytes[22] = 1;
        rewrite_header_checksum(&mut bytes);

        assert!(matches!(
            ConcatFragment::from_bytes(&bytes, &Options::new()),
            Err(BurliError::Format(
                "concat fragment reserved bytes are non-zero"
            ))
        ));
    }

    #[test]
    fn rejects_serialized_length_mismatch() {
        let fragment = encode_fragment(b"serialized fragment", &spec(4), &Options::new()).unwrap();
        let mut bytes = fragment.to_bytes(&Options::new()).unwrap();
        bytes.push(0);

        assert!(matches!(
            ConcatFragment::from_bytes(&bytes, &Options::new()),
            Err(BurliError::Format(
                "serialized concat fragment length mismatch"
            ))
        ));
    }

    #[test]
    fn staged_fragment_validation_happens_before_output_mutation() {
        let spec = spec(2);
        let mut fragment = encode_fragment(b"payload", &spec, &Options::new()).unwrap();
        fragment.metadata.checksum ^= 1;
        let mut output = b"prefix".to_vec();

        assert!(assemble_fragments(&spec, &[fragment], &mut output, &Options::new()).is_err());
        assert_eq!(output, b"prefix");
    }

    #[test]
    fn rejects_spec_mismatch() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let mut output = Vec::new();

        assert!(matches!(
            assemble_fragments(&spec(2), &[fragment], &mut output, &Options::new()),
            Err(BurliError::Format("concat fragment spec mismatch"))
        ));
    }

    #[test]
    fn rejects_payload_length_mismatch() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let (metadata, mut payload) = fragment.into_parts();
        payload.push(0);

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format(
                "concat fragment payload length mismatch"
            ))
        ));
    }

    #[test]
    fn rejects_non_minimal_payload_bits() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let (mut metadata, payload) = fragment.into_parts();
        metadata.payload_bit_len = metadata.payload_bit_len.saturating_sub(8);

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format("concat fragment payload is not minimal"))
        ));
    }

    #[test]
    fn rejects_missing_required_flags() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let (mut metadata, payload) = fragment.into_parts();
        metadata.flags = FLAG_NO_BACKWARD_REFERENCES;

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format("concat fragment required flags missing"))
        ));
    }

    #[test]
    fn rejects_unknown_flags() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let (mut metadata, payload) = fragment.into_parts();
        metadata.flags |= 1 << 31;

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format("concat fragment unknown flags set"))
        ));
    }

    #[test]
    fn rejects_invalid_payload_kind_flags() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let (mut metadata, payload) = fragment.into_parts();
        metadata.flags = REQUIRED_FLAGS;

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format(
                "concat fragment payload kind is invalid"
            ))
        ));
    }

    #[test]
    fn rejects_conflicting_payload_kind_flags() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let (mut metadata, payload) = fragment.into_parts();
        metadata.flags =
            REQUIRED_FLAGS | FLAG_NO_BACKWARD_REFERENCES | FLAG_LOCAL_BACKWARD_REFERENCES;

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format(
                "concat fragment payload kind is invalid"
            ))
        ));
    }

    #[test]
    fn rejects_payload_kind_mismatch() {
        let input = b"abcdefghabcdefghabcdefghabcdefgh".repeat(128);
        let fragment = encode_fragment(&input, &spec(3), &Options::new()).unwrap();
        let (mut metadata, payload) = fragment.into_parts();
        metadata.flags = (metadata.flags & !PAYLOAD_KIND_FLAGS) | FLAG_NO_BACKWARD_REFERENCES;

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format("concat fragment payload kind mismatch"))
        ));
    }

    #[test]
    fn rejects_non_zero_payload_padding() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let (mut metadata, mut payload) = fragment.into_parts();
        let padding_bits = payload
            .len()
            .saturating_mul(8)
            .saturating_sub(metadata.payload_bit_len);
        assert!(padding_bits != 0);
        let mask = 1_u8 << (metadata.payload_bit_len % 8);
        *payload.last_mut().unwrap() |= mask;
        metadata.checksum = checksum64(&payload);

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format("non-zero concat fragment padding"))
        ));
    }

    #[test]
    fn rejects_metadata_block_payload() {
        let payload = vec![0b0000_0110];
        let metadata = metadata_for_payload(&payload, 6, 0, FLAG_NO_BACKWARD_REFERENCES);

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format(
                "concat fragment contains metadata block"
            ))
        ));
    }

    #[test]
    fn rejects_stream_trailer_payload() {
        let mut writer = BitWriter::new();
        burli_encode::write_concat_stream_trailer(&mut writer).unwrap();
        let payload_bit_len = writer.written_bits();
        let payload = writer.into_bytes();
        let metadata =
            metadata_for_payload(&payload, payload_bit_len, 0, FLAG_NO_BACKWARD_REFERENCES);

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format(
                "concat fragment contains stream trailer"
            ))
        ));
    }

    #[test]
    fn rejects_serialized_metadata_block_payload() {
        let payload = vec![0b0000_0110];
        let metadata = metadata_for_payload(&payload, 6, 0, FLAG_NO_BACKWARD_REFERENCES);
        let bytes = serialized_with_header(&metadata, &payload);

        assert!(matches!(
            ConcatFragment::from_bytes(&bytes, &Options::new()),
            Err(BurliError::Format(
                "concat fragment contains metadata block"
            ))
        ));
    }

    #[test]
    fn rejects_decoded_length_mismatch() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let (mut metadata, payload) = fragment.into_parts();
        metadata.input_len += 1;

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format(
                "concat fragment decoded length mismatch"
            ))
        ));
    }

    #[test]
    fn rejects_byte_summary_mismatch() {
        let fragment = encode_fragment(b"payload", &spec(1), &Options::new()).unwrap();
        let (mut metadata, payload) = fragment.into_parts();
        metadata.first_bytes[0] ^= 1;

        assert!(matches!(
            ConcatFragment::from_parts(metadata, payload, &Options::new()),
            Err(BurliError::Format("concat fragment byte summary mismatch"))
        ));
    }

    #[test]
    fn q6_fragments_are_unsupported_until_encoder_supports_them() {
        assert!(matches!(
            encode_fragment(b"payload", &spec(6), &Options::new()),
            Err(BurliError::Unsupported(_))
        ));
    }

    #[test]
    fn types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<ConcatSpec>();
        assert_send_sync::<FragmentMetadata>();
        assert_send_sync::<ConcatFragment>();
        assert_send_sync::<ConcatAssembler>();
        assert_send_sync::<Options>();
    }

    #[cfg(feature = "std")]
    #[test]
    fn fragments_can_be_encoded_in_parallel_by_callers() {
        let spec = spec(4);
        let inputs = [
            b"thread one thread one".to_vec(),
            b"thread two thread two".to_vec(),
            b"thread three thread three".to_vec(),
        ];
        let handles = inputs
            .iter()
            .cloned()
            .map(|input| {
                let spec = spec.clone();
                std::thread::spawn(move || encode_fragment(&input, &spec, &Options::new()).unwrap())
            })
            .collect::<Vec<_>>();
        let fragments = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        let mut encoded = Vec::new();
        assemble_fragments(&spec, &fragments, &mut encoded, &Options::new()).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), inputs.concat());
    }
}
