//! Local OPUS-MT models (Helsinki-NLP, ONNX build by Xenova). CC-BY-4.0.
//! The first offline translation downloads the models. Later runs stay on disk.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use futures_util::StreamExt;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use tokenizers::Tokenizer;

const MAX_SOURCE_TOKENS: usize = 128;
const MAX_DECODE_STEPS: usize = 96;

static RUNTIME: OnceLock<ort::environment::Environment> = OnceLock::new();
static ENGINES: Mutex<Vec<Loaded>> = Mutex::new(Vec::new());

struct Loaded {
    key: String,
    encoder: Session,
    decoder: Session,
    tokenizer: Tokenizer,
    start: i64,
    eos: i64,
    pad: i64,
}

pub async fn translate(
    model_dir: &Path,
    text: &str,
    target: &str,
    on_status: &(dyn Fn(String) + Send + Sync),
) -> Result<String, String> {
    let key = if target == "en" { "zh-en" } else { "en-zh" };
    ensure_models(model_dir, key, on_status).await?;
    let model_dir = model_dir.to_path_buf();
    let text = text.to_string();
    on_status("正在加载离线模型…".into());
    tokio::task::spawn_blocking(move || infer(&model_dir, key, &text))
        .await
        .map_err(|err| err.to_string())?
}

async fn ensure_models(
    model_dir: &Path,
    key: &str,
    on_status: &(dyn Fn(String) + Send + Sync),
) -> Result<(), String> {
    let dir = model_dir.join(key);
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let files = [
        ("config.json", format!("https://huggingface.co/Xenova/opus-mt-{key}/resolve/main/config.json"), 100),
        ("tokenizer.json", format!("https://huggingface.co/Xenova/opus-mt-{key}/resolve/main/tokenizer.json"), 100_000),
        ("encoder.onnx", format!("https://huggingface.co/Xenova/opus-mt-{key}/resolve/main/onnx/encoder_model_quantized.onnx"), 1_000_000),
        ("decoder.onnx", format!("https://huggingface.co/Xenova/opus-mt-{key}/resolve/main/onnx/decoder_model_quantized.onnx"), 1_000_000),
    ];
    let label = if key == "zh-en" { "中文→英文" } else { "英文→中文" };
    for (name, url, min_size) in files {
        let dest = dir.join(name);
        if dest.is_file() && dest.metadata().map(|m| m.len()).unwrap_or(0) >= min_size {
            continue;
        }
        on_status(format!("正在下载离线模型（{label}，{name}）…"));
        download(&url, &dest, min_size).await?;
    }
    Ok(())
}

async fn download(url: &str, dest: &Path, min_size: u64) -> Result<(), String> {
    let mirrors = [
        url.replace("https://huggingface.co/", "https://hf-mirror.com/"),
        url.to_string(),
    ];
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 Vela")
        .build()
        .map_err(|err| err.to_string())?;
    let mut last = String::from("下载失败。");
    for mirror in mirrors {
        match download_one(&client, &mirror, dest, min_size).await {
            Ok(()) => return Ok(()),
            Err(err) => last = err,
        }
    }
    Err(format!("离线模型下载失败。请联网后重试。{last}"))
}

async fn download_one(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    min_size: u64,
) -> Result<(), String> {
    let partial = dest.with_extension("partial");
    let response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(300))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let mut file = File::create(&partial).map_err(|err| err.to_string())?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| err.to_string())?;
        file.write_all(&chunk).map_err(|err| err.to_string())?;
    }
    file.flush().map_err(|err| err.to_string())?;
    drop(file);
    let size = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
    if size < min_size {
        let _ = fs::remove_file(&partial);
        return Err(format!("文件不完整（{size} 字节）。"));
    }
    fs::rename(&partial, dest).map_err(|err| err.to_string())?;
    Ok(())
}

fn infer(model_dir: &Path, key: &str, text: &str) -> Result<String, String> {
    prepare_runtime()?;
    let mut engines = ENGINES.lock().map_err(|err| err.to_string())?;
    if !engines.iter().any(|engine| engine.key == key) {
        engines.push(load_engine(model_dir, key)?);
    }
    let engine = engines
        .iter_mut()
        .find(|engine| engine.key == key)
        .ok_or("离线模型没有载入。")?;
    let mut ids = encode(&engine.tokenizer, text)?;
    if ids.is_empty() {
        return Err("没有可翻译的文字。".into());
    }
    if ids.len() > MAX_SOURCE_TOKENS {
        ids.truncate(MAX_SOURCE_TOKENS);
    }
    let mask = vec![1i64; ids.len()];
    let hidden = encoder_hidden(&mut engine.encoder, &ids, &mask)?;
    let mut decoded = vec![engine.start];
    for _ in 0..MAX_DECODE_STEPS {
        let next = decoder_next(
            &mut engine.decoder,
            &decoded,
            &hidden.0,
            &hidden.1,
            &mask,
            engine.pad,
        )?;
        if next == engine.eos || (decoded.len() > 1 && next == *decoded.last().unwrap_or(&next)) {
            break;
        }
        decoded.push(next);
    }
    let output_ids: Vec<u32> = decoded
        .into_iter()
        .skip(1)
        .filter(|id| *id != engine.eos && *id != engine.pad)
        .map(|id| id as u32)
        .collect();
    let text = engine
        .tokenizer
        .decode(&output_ids, true)
        .map_err(|err| err.to_string())?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("离线模型没有返回译文。".into());
    }
    Ok(text)
}

fn encode(tokenizer: &Tokenizer, text: &str) -> Result<Vec<i64>, String> {
    let encoding = tokenizer
        .encode(text, true)
        .map_err(|err| err.to_string())?;
    Ok(encoding.get_ids().iter().map(|id| i64::from(*id)).collect())
}

fn encoder_hidden(
    session: &mut Session,
    ids: &[i64],
    mask: &[i64],
) -> Result<(Vec<usize>, Vec<f32>), String> {
    let ids_tensor = tensor_i64(&[1, ids.len()], ids)?;
    let mask_tensor = tensor_i64(&[1, mask.len()], mask)?;
    let outputs = session
        .run(ort::inputs![
            "input_ids" => ids_tensor,
            "attention_mask" => mask_tensor
        ])
        .map_err(|err| format!("离线编码器失败：{err}"))?;
    take_array(&outputs, "last_hidden_state")
}

fn decoder_next(
    session: &mut Session,
    ids: &[i64],
    hidden_shape: &[usize],
    hidden_data: &[f32],
    mask: &[i64],
    pad: i64,
) -> Result<i64, String> {
    let ids_tensor = tensor_i64(&[1, ids.len()], ids)?;
    let mask_tensor = tensor_i64(&[1, mask.len()], mask)?;
    let hidden_tensor = Tensor::from_array((hidden_shape.to_vec(), hidden_data.to_vec()))
        .map_err(|err| err.to_string())?;
    let outputs = session
        .run(ort::inputs![
            "input_ids" => ids_tensor,
            "encoder_attention_mask" => mask_tensor,
            "encoder_hidden_states" => hidden_tensor
        ])
        .map_err(|err| format!("离线解码器失败：{err}"))?;
    let (shape, data) = take_array(&outputs, "logits")?;
    if shape.len() != 3 {
        return Err("离线模型输出形状无法识别。".into());
    }
    let vocab = shape[2];
    let row = (shape[1] - 1) * vocab;
    let mut best = 0usize;
    let mut best_value = f32::NEG_INFINITY;
    for (index, value) in data[row..row + vocab].iter().enumerate() {
        if index as i64 == pad {
            continue;
        }
        if *value > best_value {
            best = index;
            best_value = *value;
        }
    }
    Ok(best as i64)
}

fn take_array(
    outputs: &ort::session::SessionOutputs,
    preferred: &str,
) -> Result<(Vec<usize>, Vec<f32>), String> {
    if let Ok(view) = outputs[preferred].try_extract_array::<f32>() {
        return Ok((view.shape().to_vec(), view.iter().copied().collect()));
    }
    for output in outputs.iter() {
        if let Ok(view) = output.1.try_extract_array::<f32>() {
            return Ok((view.shape().to_vec(), view.iter().copied().collect()));
        }
    }
    Err("离线模型没有返回数值。".into())
}

fn tensor_i64(shape: &[usize], data: &[i64]) -> Result<Tensor<i64>, String> {
    Tensor::from_array((shape.to_vec(), data.to_vec())).map_err(|err| err.to_string())
}

fn load_engine(model_dir: &Path, key: &str) -> Result<Loaded, String> {
    let env = RUNTIME.get().ok_or("离线运行库还没有初始化。")?;
    let dir = model_dir.join(key);
    let config = fs::read_to_string(dir.join("config.json")).map_err(|err| err.to_string())?;
    let config: serde_json::Value = serde_json::from_str(&config).map_err(|err| err.to_string())?;
    let tokenizer = load_tokenizer(&dir.join("tokenizer.json"))?;
    let encoder = Session::builder(env)
        .map_err(|err| err.to_string())?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|err| err.to_string())?
        .with_intra_threads(2)
        .map_err(|err| err.to_string())?
        .commit_from_file(dir.join("encoder.onnx"))
        .map_err(|err| format!("无法载入离线编码器：{err}"))?;
    let decoder = Session::builder(env)
        .map_err(|err| err.to_string())?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|err| err.to_string())?
        .with_intra_threads(2)
        .map_err(|err| err.to_string())?
        .commit_from_file(dir.join("decoder.onnx"))
        .map_err(|err| format!("无法载入离线解码器：{err}"))?;
    Ok(Loaded {
        key: key.to_string(),
        encoder,
        decoder,
        tokenizer,
        start: json_i64(&config, "decoder_start_token_id", 65000),
        eos: json_i64(&config, "eos_token_id", 0),
        pad: json_i64(&config, "pad_token_id", 65000),
    })
}

fn load_tokenizer(path: &Path) -> Result<Tokenizer, String> {
    let raw = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let mut value: serde_json::Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
    let broken = value
        .pointer("/normalizer/precompiled_charsmap")
        .is_some_and(|item| item.is_null());
    if broken {
        if let Some(object) = value.as_object_mut() {
            object.remove("normalizer");
        }
    }
    Tokenizer::from_bytes(value.to_string().into_bytes()).map_err(|err| err.to_string())
}

fn json_i64(value: &serde_json::Value, key: &str, fallback: i64) -> i64 {
    value.get(key).and_then(|item| item.as_i64()).unwrap_or(fallback)
}

fn prepare_runtime() -> Result<(), String> {
    if RUNTIME.get().is_some() {
        return Ok(());
    }
    let env = ort::init().build().map_err(|err| err.to_string())?;
    let _ = RUNTIME.set(env);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_model_translates_a_short_greeting() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.models");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let text = runtime
            .block_on(translate(&dir, "你好", "en", &|status| eprintln!("{status}")))
            .unwrap_or_else(|err| panic!("{err}"));
        eprintln!("zh-en greeting: {text}");
        let lower = text.to_lowercase();
        assert!(
            lower.contains("hello") || lower.contains("hi"),
            "unexpected translation: {text}"
        );
        let sentence = runtime
            .block_on(translate(&dir, "今天天气很好", "en", &|_| {}))
            .unwrap_or_else(|err| panic!("{err}"));
        eprintln!("zh-en sentence: {sentence}");
        let sentence_lower = sentence.to_lowercase();
        assert!(
            sentence_lower.contains("day") || sentence_lower.contains("weather"),
            "unexpected sentence: {sentence}"
        );
        let back = runtime
            .block_on(translate(&dir, "hello", "zh-CN", &|_| {}))
            .unwrap_or_else(|err| panic!("{err}"));
        assert!(
            back.contains('你'),
            "unexpected reverse translation: {back}"
        );
    }
}
