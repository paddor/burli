use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn compress(input: &[u8], quality: u8) -> Result<Vec<u8>, JsError> {
    burli::compress(input, quality).map_err(|error| JsError::new(&error.to_string()))
}

#[wasm_bindgen]
pub fn decompress(input: &[u8], max_output_size: Option<usize>) -> Result<Vec<u8>, JsError> {
    let result = match max_output_size {
        Some(limit) => burli::decompress_with_limit(input, limit),
        None => burli::decompress(input),
    };
    result.map_err(|error| JsError::new(&error.to_string()))
}

#[wasm_bindgen]
pub struct Compressor {
    inner: burli::Compressor,
}

#[wasm_bindgen]
impl Compressor {
    #[wasm_bindgen(constructor)]
    pub fn new(quality: u8) -> Result<Compressor, JsError> {
        burli::Compressor::new(quality)
            .map(|inner| Compressor { inner })
            .map_err(|error| JsError::new(&error.to_string()))
    }

    pub fn compress(&mut self, input: &[u8]) -> Result<Vec<u8>, JsError> {
        self.inner
            .compress(input)
            .map_err(|error| JsError::new(&error.to_string()))
    }
}

#[wasm_bindgen]
pub struct Decompressor {
    inner: burli::Decompressor,
}

#[wasm_bindgen]
impl Decompressor {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Decompressor {
        Decompressor {
            inner: burli::Decompressor::new(),
        }
    }

    pub fn decompress(
        &mut self,
        input: &[u8],
        max_output_size: Option<usize>,
    ) -> Result<Vec<u8>, JsError> {
        let previous_limit = self.inner.max_output_size();
        if let Some(limit) = max_output_size {
            self.inner.set_limit(limit);
        }
        let result = self
            .inner
            .decompress(input)
            .map_err(|error| JsError::new(&error.to_string()));
        self.inner.set_limit(previous_limit);
        result
    }
}
