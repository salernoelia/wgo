//! Native MLX Whisper inference for mlx-community/whisper-large-v3-turbo-q4.
//! Architecture follows Apple's mlx-examples/whisper (MIT); see assets/whisper/README.md.
use mlx_rs::ops::indexing::IndexOp;
use mlx_rs::{fast, nn, ops, transforms::eval, Array, Dtype};
use serde::Deserialize;
use std::{collections::HashMap, io::Write, path::Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const SAMPLES_PER_WINDOW: usize = 480_000;
const EOT: u32 = 50257;
const SOT: u32 = 50258;

#[derive(Deserialize)]
struct Dimensions {
    n_mels: i32,
    n_audio_ctx: i32,
    n_audio_state: i32,
    n_audio_head: i32,
    n_audio_layer: usize,
    n_vocab: i32,
    n_text_ctx: usize,
    n_text_state: i32,
    n_text_head: i32,
    n_text_layer: usize,
    quantization: Quantization,
}
#[derive(Deserialize)]
struct Quantization {
    group_size: i32,
    bits: i32,
}
#[derive(Deserialize)]
struct Generation {
    lang_to_id: HashMap<String, u32>,
    task_to_id: HashMap<String, u32>,
    no_timestamps_token_id: u32,
    suppress_tokens: Vec<u32>,
    begin_suppress_tokens: Vec<u32>,
}
#[derive(Default)]
struct Cache {
    kv: Option<(Array, Array)>,
    cross: Option<(Array, Array)>,
}
pub(super) struct Model {
    dims: Dimensions,
    generation: Generation,
    weights: HashMap<String, Array>,
    tokenizer: tokenizers::Tokenizer,
    positions: Array,
    mel_filters: Array,
}

// MLX's C API reads NPY, so unpack one NPZ entry at a time into an RAII temporary.
// Evaluate before deleting the temporary: MLX arrays load lazily.
fn load_numpy_bytes(bytes: &[u8]) -> Result<Array> {
    let mut file = tempfile::Builder::new().suffix(".npy").tempfile()?;
    file.write_all(bytes)?;
    let array = Array::load_numpy(file.path())?;
    array.eval()?;
    Ok(array)
}

impl Model {
    pub(super) fn load(dir: &Path) -> Result<Self> {
        let dims: Dimensions = serde_json::from_slice(&std::fs::read(dir.join("config.json"))?)?;
        if dims.n_mels != 128
            || dims.n_audio_ctx != 1500
            || dims.n_audio_state != 1280
            || dims.n_audio_head != 20
            || dims.n_audio_layer != 32
            || dims.n_vocab != 51866
            || dims.n_text_ctx != 448
            || dims.n_text_state != 1280
            || dims.n_text_head != 20
            || dims.n_text_layer != 4
            || dims.quantization.bits != 4
            || dims.quantization.group_size != 64
        {
            return Err(
                "Expected mlx-community/whisper-large-v3-turbo-q4 dimensions and quantization"
                    .into(),
            );
        }
        let generation =
            serde_json::from_slice(&std::fs::read(dir.join("generation_config.json"))?)?;
        let tokenizer = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(std::fs::File::open(dir.join("weights.npz"))?)?;
        let mut weights = HashMap::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry
                .name()
                .strip_suffix(".npy")
                .ok_or("Invalid MLX NPZ entry")?
                .to_owned();
            let mut file = tempfile::Builder::new().suffix(".npy").tempfile()?;
            std::io::copy(&mut entry, &mut file)?;
            let array = Array::load_numpy(file.path())?;
            array.eval()?;
            weights.insert(name, array);
        }
        let half = dims.n_audio_state / 2;
        let mut positions = Vec::new();
        for t in 0..dims.n_audio_ctx {
            for trig in 0..2 {
                for c in 0..half {
                    let angle = t as f32 * (-10000_f32.ln() * c as f32 / (half - 1) as f32).exp();
                    positions.push(if trig == 0 { angle.sin() } else { angle.cos() });
                }
            }
        }
        let positions = Array::from_slice(&positions, &[dims.n_audio_ctx, dims.n_audio_state])
            .as_dtype(Dtype::Float16)?;
        let mel_filters = load_numpy_bytes(include_bytes!("../../assets/whisper/mel_128.npy"))?;
        Ok(Self {
            dims,
            generation,
            weights,
            tokenizer,
            positions,
            mel_filters,
        })
    }
    fn w(&self, name: &str) -> Result<&Array> {
        self.weights
            .get(name)
            .ok_or_else(|| format!("Missing MLX weight: {name}").into())
    }
    fn linear(&self, x: &Array, name: &str) -> Result<Array> {
        let w = self.w(&format!("{name}.weight"))?;
        let mut y = if let Some(scales) = self.weights.get(&format!("{name}.scales")) {
            ops::quantized_matmul(
                x,
                w,
                scales,
                self.w(&format!("{name}.biases"))?,
                true,
                self.dims.quantization.group_size,
                self.dims.quantization.bits,
            )?
        } else {
            x.matmul(&w.t())?
        };
        if let Some(bias) = self.weights.get(&format!("{name}.bias")) {
            y = y.add(bias)?;
        }
        Ok(y)
    }
    fn norm(&self, x: &Array, name: &str) -> Result<Array> {
        Ok(fast::layer_norm(
            x,
            self.w(&format!("{name}.weight"))?,
            self.w(&format!("{name}.bias"))?,
            1e-5,
        )?)
    }
    fn attention(
        &self,
        x: &Array,
        source: Option<&Array>,
        name: &str,
        heads: i32,
        cache: &mut Option<(Array, Array)>,
        causal: bool,
    ) -> Result<Array> {
        let q = self.linear(x, &format!("{name}.query"))?;
        let (k, v) = match (source, cache.as_ref()) {
            (Some(_), Some((k, v))) => (k.clone(), v.clone()),
            _ => {
                let input = source.unwrap_or(x);
                let mut k = self.linear(input, &format!("{name}.key"))?;
                let mut v = self.linear(input, &format!("{name}.value"))?;
                if source.is_none() {
                    if let Some((old_k, old_v)) = cache.as_ref() {
                        k = ops::concatenate_axis(&[old_k, &k], 1)?;
                        v = ops::concatenate_axis(&[old_v, &v], 1)?;
                    }
                }
                (k, v)
            }
        };
        let width = q.shape()[2];
        let len = q.shape()[1];
        let split = |a: &Array| {
            a.reshape(&[1, -1, heads, width / heads])?
                .transpose_axes(&[0, 2, 1, 3])
        };
        let mask = if causal && len > 1 {
            Some(fast::ScaledDotProductAttentionMask::Causal)
        } else {
            None
        };
        let y = fast::scaled_dot_product_attention(
            split(&q)?,
            split(&k)?,
            split(&v)?,
            1.0 / ((width / heads) as f32).sqrt(),
            mask,
        )?
        .transpose_axes(&[0, 2, 1, 3])?
        .reshape(&[1, len, width])?;
        *cache = Some((k, v));
        self.linear(&y, &format!("{name}.out"))
    }
    fn block(
        &self,
        x: &Array,
        source: Option<&Array>,
        name: &str,
        heads: i32,
        cache: &mut Cache,
    ) -> Result<Array> {
        let y = self.attention(
            &self.norm(x, &format!("{name}.attn_ln"))?,
            None,
            &format!("{name}.attn"),
            heads,
            &mut cache.kv,
            source.is_some(),
        )?;
        let mut x = x.add(y)?;
        if let Some(source) = source {
            x = x.add(self.attention(
                &self.norm(&x, &format!("{name}.cross_attn_ln"))?,
                Some(source),
                &format!("{name}.cross_attn"),
                heads,
                &mut cache.cross,
                false,
            )?)?;
        }
        let y = nn::gelu(self.linear(
            &self.norm(&x, &format!("{name}.mlp_ln"))?,
            &format!("{name}.mlp1"),
        )?)?;
        Ok(x.add(self.linear(&y, &format!("{name}.mlp2"))?)?)
    }
    fn mel(&self, pcm: &[f32]) -> Result<Array> {
        // Whisper: periodic Hann, centered reflect padding, 400-point FFT / 160 hop,
        // Slaney-normalized mel filters, then log10 dynamic-range compression.
        let mut padded = pcm.to_vec();
        padded.resize(SAMPLES_PER_WINDOW, 0.0);
        let mut frames = Vec::with_capacity(3000 * 400);
        for frame in 0..3000 {
            for n in 0..400 {
                let pos = frame as isize * 160 + n as isize - 200;
                let pos = if pos < 0 { -pos } else { pos } as usize;
                let sample = padded.get(pos).copied().unwrap_or(0.0);
                frames
                    .push(sample * (0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / 400.0).cos()));
            }
        }
        let spectrum = mlx_rs::fft::rfft(Array::from_slice(&frames, &[3000, 400]), 400, -1)?;
        let power = spectrum.abs()?.square()?;
        let mel = power.matmul(&self.mel_filters.t())?;
        let log = ops::maximum(mel, Array::from_f32(1e-10))?.log10()?;
        let floor = log.max(None)?.subtract(Array::from_f32(8.0))?;
        Ok(ops::maximum(&log, floor)?
            .add(Array::from_f32(4.0))?
            .divide(Array::from_f32(4.0))?
            .reshape(&[1, 3000, 128])?
            .as_dtype(Dtype::Float16)?)
    }
    fn encode(&self, pcm: &[f32]) -> Result<Array> {
        let mut x = self.mel(pcm)?;
        for (name, stride) in [("encoder.conv1", 1), ("encoder.conv2", 2)] {
            x = nn::gelu(
                ops::conv1d(&x, self.w(&format!("{name}.weight"))?, stride, 1, 1, 1)?
                    .add(self.w(&format!("{name}.bias"))?)?,
            )?;
        }
        x = x.add(&self.positions)?;
        for i in 0..self.dims.n_audio_layer {
            x = self.block(
                &x,
                None,
                &format!("encoder.blocks.{i}"),
                self.dims.n_audio_head,
                &mut Cache::default(),
            )?;
            x.eval()?;
        }
        Ok(self.norm(&x, "encoder.ln_post")?)
    }
    fn logits(
        &self,
        tokens: &[u32],
        encoded: &Array,
        cache: &mut [Cache],
        offset: usize,
    ) -> Result<Array> {
        let ids = Array::from_slice(tokens, &[tokens.len() as i32]);
        let name = "decoder.token_embedding";
        let w = self.w(&format!("{name}.weight"))?.take_axis(&ids, 0)?;
        let mut x = if let Some(scales) = self.weights.get(&format!("{name}.scales")) {
            ops::dequantize(
                w,
                scales.take_axis(&ids, 0)?,
                self.w(&format!("{name}.biases"))?.take_axis(&ids, 0)?,
                self.dims.quantization.group_size,
                self.dims.quantization.bits,
            )?
        } else {
            w
        };
        let positions = self
            .w("decoder.positional_embedding")?
            .index(offset as i32..(offset + tokens.len()) as i32);
        x = x
            .add(positions)?
            .reshape(&[1, tokens.len() as i32, self.dims.n_text_state])?;
        for (i, cache) in cache.iter_mut().enumerate() {
            x = self.block(
                &x,
                Some(encoded),
                &format!("decoder.blocks.{i}"),
                self.dims.n_text_head,
                cache,
            )?;
        }
        let x = self.norm(&x, "decoder.ln")?;
        let logits = self
            .linear(&x.index((.., -1, ..)), name)?
            .as_dtype(Dtype::Float32)?;
        logits.eval()?;
        for c in cache {
            if let Some((k, v)) = &c.kv {
                eval([k, v])?;
            }
            if let Some((k, v)) = &c.cross {
                eval([k, v])?;
            }
        }
        Ok(logits)
    }
    fn decode(&self, encoded: &Array) -> Result<String> {
        let mut cache: Vec<Cache> = (0..self.dims.n_text_layer)
            .map(|_| Cache::default())
            .collect();
        let lang_logits = self.logits(&[SOT], encoded, &mut cache, 0)?;
        let scores = lang_logits.as_slice::<f32>();
        let language = *self
            .generation
            .lang_to_id
            .values()
            .max_by(|a, b| scores[**a as usize].total_cmp(&scores[**b as usize]))
            .ok_or("No language tokens")?;
        let no_speech = scores[50363_usize];
        let peak = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let no_speech_prob =
            (no_speech - peak).exp() / scores.iter().map(|x| (x - peak).exp()).sum::<f32>();
        let mut input = vec![
            language,
            *self
                .generation
                .task_to_id
                .get("transcribe")
                .ok_or("Missing transcribe token")?,
            self.generation.no_timestamps_token_id,
        ];
        let mut offset = 1;
        let mut output = Vec::new();
        let mut sum_logprob = 0.0;
        while offset + input.len() < self.dims.n_text_ctx {
            let logits = self.logits(&input, encoded, &mut cache, offset)?;
            let mut scores = logits.as_slice::<f32>().to_vec();
            for id in &self.generation.suppress_tokens {
                scores[*id as usize] = f32::NEG_INFINITY;
            }
            for score in &mut scores[EOT as usize + 1..] {
                *score = f32::NEG_INFINITY;
            }
            if output.is_empty() {
                for id in &self.generation.begin_suppress_tokens {
                    scores[*id as usize] = f32::NEG_INFINITY;
                }
            }
            let (token, best) = scores
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .ok_or("Empty decoder logits")?;
            sum_logprob += -scores.iter().map(|s| (s - best).exp()).sum::<f32>().ln();
            if token as u32 == EOT {
                break;
            }
            output.push(token as u32);
            offset += input.len();
            input = vec![token as u32];
        }
        if no_speech_prob > 0.6 && sum_logprob / ((output.len() + 1) as f32) < -1.0 {
            return Ok(String::new());
        }
        self.tokenizer
            .decode(&output, true)
            .map_err(|e| e.to_string().into())
    }
}

impl Model {
    pub(super) fn transcribe(&self, pcm_samples: &[f32]) -> Result<String> {
        if pcm_samples.iter().any(|s| !s.is_finite()) {
            return Err("Audio contains non-finite samples".into());
        }
        if pcm_samples.is_empty() || pcm_samples.iter().all(|s| s.abs() < 1e-7) {
            return Ok(String::new());
        }
        let mut text = String::new();
        for chunk in pcm_samples.chunks(SAMPLES_PER_WINDOW) {
            let encoded = self.encode(chunk)?;
            let part = self.decode(&encoded)?;
            if !part.trim().is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(part.trim());
            }
        }
        Ok(text)
    }
}

pub fn transcribe_pcm(model_dir: &Path, pcm_samples: &[f32]) -> Result<String> {
    Model::load(model_dir)?.transcribe(pcm_samples)
}

pub(super) fn clear_memory_cache() {
    // All inference arrays have been evaluated and dropped by the owning worker.
    unsafe {
        mlx_sys::mlx_clear_cache();
    }
}

#[cfg(test)]
pub(super) fn memory_usage() -> (usize, usize) {
    let (mut active, mut cache) = (0, 0);
    unsafe {
        mlx_sys::mlx_get_active_memory(&mut active);
        mlx_sys::mlx_get_cache_memory(&mut cache);
    }
    (active, cache)
}

#[cfg(test)]
pub(super) fn peak_memory() -> usize {
    let mut peak = 0;
    unsafe {
        mlx_sys::mlx_get_peak_memory(&mut peak);
    }
    peak
}
