//! Configurable OCR / translation providers.
//!
//! Nothing is bundled and no model is downloaded by the app. The settings
//! centre writes `[ocr]` and `[translate]` in `snip.toml`; this module turns
//! those values into HTTP requests. Supported shapes:
//!
//! * `openai` — any OpenAI-compatible chat-completions endpoint (OpenAI,
//!   DeepSeek, Qwen, vLLM, Ollama `/v1`, …). For OCR the model must accept
//!   image content parts.
//! * `libretranslate` — a LibreTranslate `/translate` endpoint.
//! * `custom` — a URL + headers + body template + JSON response path, so a
//!   service this app does not know can still be plugged in.
//!
//! Secrets are stored in `snip.toml`; the settings UI masks them, but they are
//! not encrypted at rest. Requests are never logged.

use base64::Engine;
use serde_json::{json, Value};

use crate::config;
use crate::net;
use crate::translate::Direction;

const DEFAULT_MODEL: &str = "gpt-4o-mini";
const DEFAULT_OCR_PROMPT: &str = "Extract all text from this image. Preserve the original line breaks and reading order. Return only the text, without explanations or Markdown fences.";
/// Online vision APIs are slow and expensive on huge images; 2048 px keeps a
/// 4K crop under control while remaining readable.
const MAX_API_SIDE: u32 = 2048;

/// Returns `(text, badge)`, where `badge` is shown next to the card title.
pub fn ocr(bgra: Vec<u8>, w: u32, h: u32) -> Result<(String, String), String> {
    ocr_with(&config::load().ocr, bgra, w, h)
}

fn ocr_with(cfg: &config::Ocr, bgra: Vec<u8>, w: u32, h: u32) -> Result<(String, String), String> {
    match cfg.provider.trim() {
        "windows" | "" => Ok((
            crate::ocr::windows_recognize(bgra, w, h)?,
            "离线 OCR".to_string(),
        )),
        "openai" => Ok((ocr_openai(cfg, bgra, w, h)?, "在线 OCR".to_string())),
        "custom" => Ok((ocr_custom(cfg, bgra, w, h)?, "在线 OCR".to_string())),
        other => Err(format!("未知的 OCR 提供方：{other}")),
    }
}

pub fn translate(text: &str, dir: Direction) -> Result<String, String> {
    translate_with(&config::load().translate, text, dir)
}

fn translate_with(cfg: &config::Translate, text: &str, dir: Direction) -> Result<String, String> {
    match cfg.provider.trim() {
        "none" | "" => Err(
            "未配置翻译服务。打开设置中心 →「截图与标注」→「翻译」，选择 OpenAI 兼容 API、LibreTranslate 或自定义 HTTP"
                .to_string(),
        ),
        "openai" => translate_openai(cfg, text, dir),
        "libretranslate" => translate_libre(cfg, text, dir),
        "custom" => translate_custom(cfg, text, dir),
        other => Err(format!("未知的翻译提供方：{other}")),
    }
}

// -- OpenAI-compatible chat completions ------------------------------------

fn ocr_openai(cfg: &config::Ocr, bgra: Vec<u8>, w: u32, h: u32) -> Result<String, String> {
    if openai_key_missing(&cfg.openai_base_url, &cfg.openai_key) {
        return Err(
            "在线 OCR 未配置完整：请填写 OpenAI API Key，或在设置中切回「系统 OCR」".to_string(),
        );
    }
    let (bgra, w, h) = fit_for_api(bgra, w, h);
    let png = crate::output::png_bytes(&bgra, w, h)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let prompt = if cfg.openai_prompt.trim().is_empty() {
        DEFAULT_OCR_PROMPT.to_string()
    } else {
        cfg.openai_prompt.clone()
    };
    let body = json!({
        "model": model_name(&cfg.openai_model),
        "messages": [
            {"role": "system", "content": prompt},
            {"role": "user", "content": [
                {"type": "text", "text": "Recognize the text in this screenshot."},
                {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{b64}")}}
            ]}
        ],
        "temperature": 0
    });
    let url = endpoint(&cfg.openai_base_url, "chat/completions");
    let response = post(&url, &bearer(&cfg.openai_key), &body, "OCR API")?;
    let text = openai_text(&response)
        .ok_or_else(|| format!("OCR 响应中没有文字：{}", snippet(&response.to_string())))?;
    let text = strip_fences(&text);
    if text.trim().is_empty() {
        return Err("OCR 服务返回了空文本".to_string());
    }
    Ok(text.trim().to_string())
}

fn ocr_custom(cfg: &config::Ocr, bgra: Vec<u8>, w: u32, h: u32) -> Result<String, String> {
    if cfg.custom_url.trim().is_empty() {
        return Err("自定义 OCR 地址为空".to_string());
    }
    if cfg.custom_body.trim().is_empty() {
        return Err("自定义 OCR 请求体为空（可用 {{image_b64}} / {{mime}} 占位）".to_string());
    }
    let (bgra, w, h) = fit_for_api(bgra, w, h);
    let png = crate::output::png_bytes(&bgra, w, h)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let body = fill_template(
        &cfg.custom_body,
        &[
            ("image_b64", b64.as_str()),
            ("mime", "image/png"),
            ("width", &w.to_string()),
            ("height", &h.to_string()),
        ],
    );
    let headers = parse_headers(&cfg.custom_headers);
    let (status, text) = net::post_json(&cfg.custom_url, &headers, &body)?;
    if !(200..300).contains(&status) {
        return Err(net::http_error("OCR API", status, &text));
    }
    let out = extract_response(&text, &cfg.custom_path, "OCR")?;
    let out = strip_fences(&out);
    if out.trim().is_empty() {
        return Err("自定义 OCR 返回了空文本".to_string());
    }
    Ok(out.trim().to_string())
}

fn translate_openai(cfg: &config::Translate, text: &str, dir: Direction) -> Result<String, String> {
    if openai_key_missing(&cfg.openai_base_url, &cfg.openai_key) {
        return Err("翻译服务未配置完整：请填写 OpenAI API Key".to_string());
    }
    let target = match dir {
        Direction::EnZh => "Simplified Chinese (zh-CN)",
        Direction::ZhEn => "English",
    };
    let mut system = if cfg.openai_prompt.trim().is_empty() {
        format!(
            "You are a professional translator. Translate the user's text into {target}. \
             Preserve paragraph breaks. Return only the translation, without explanations or Markdown fences."
        )
    } else {
        cfg.openai_prompt.replace("{{target}}", target).replace(
            "{{source}}",
            if dir == Direction::EnZh {
                "English"
            } else {
                "Chinese"
            },
        )
    };
    if !system.contains(target) {
        system.push_str(&format!("\nTarget language: {target}."));
    }
    let body = json!({
        "model": model_name(&cfg.openai_model),
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": text}
        ],
        "temperature": 0.2
    });
    let url = endpoint(&cfg.openai_base_url, "chat/completions");
    let response = post(&url, &bearer(&cfg.openai_key), &body, "翻译 API")?;
    let out = openai_text(&response)
        .ok_or_else(|| format!("翻译响应中没有文字：{}", snippet(&response.to_string())))?;
    Ok(strip_fences(&out).trim().to_string())
}

// -- LibreTranslate --------------------------------------------------------

fn translate_libre(cfg: &config::Translate, text: &str, dir: Direction) -> Result<String, String> {
    let raw = if cfg.libre_url.trim().is_empty() {
        "http://127.0.0.1:5000/translate"
    } else {
        cfg.libre_url.trim()
    };
    let raw = raw.trim_end_matches('/');
    let url = if raw.ends_with("/translate") {
        raw.to_string()
    } else {
        format!("{raw}/translate")
    };
    let (source, target) = match dir {
        Direction::EnZh => ("en", "zh"),
        Direction::ZhEn => ("zh", "en"),
    };
    let mut body = json!({
        "q": text,
        "source": source,
        "target": target,
        "format": "text"
    });
    if !cfg.libre_key.trim().is_empty() {
        body["api_key"] = Value::String(cfg.libre_key.trim().to_string());
    }
    let (status, response) = net::post_json(&url, &[], &body.to_string())?;
    if !(200..300).contains(&status) {
        return Err(net::http_error("LibreTranslate", status, &response));
    }
    let value: Value = serde_json::from_str(&response)
        .map_err(|e| format!("LibreTranslate 响应不是 JSON：{e}"))?;
    let out = value
        .get("translatedText")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "LibreTranslate 响应缺少 translatedText：{}",
                snippet(&response)
            )
        })?;
    Ok(out.trim().to_string())
}

fn translate_custom(cfg: &config::Translate, text: &str, dir: Direction) -> Result<String, String> {
    if cfg.custom_url.trim().is_empty() {
        return Err("自定义翻译地址为空".to_string());
    }
    if cfg.custom_body.trim().is_empty() {
        return Err(
            "自定义翻译请求体为空（可用 {{text}} / {{source}} / {{target}} 占位）".to_string(),
        );
    }
    let (source, target) = match dir {
        Direction::EnZh => ("en", "zh"),
        Direction::ZhEn => ("zh", "en"),
    };
    let escaped = json_escape(text);
    let body = fill_template(
        &cfg.custom_body,
        &[("text", &escaped), ("source", source), ("target", target)],
    );
    let headers = parse_headers(&cfg.custom_headers);
    let (status, response) = net::post_json(&cfg.custom_url, &headers, &body)?;
    if !(200..300).contains(&status) {
        return Err(net::http_error("翻译 API", status, &response));
    }
    let out = extract_response(&response, &cfg.custom_path, "翻译")?;
    Ok(strip_fences(&out).trim().to_string())
}

// -- helpers ---------------------------------------------------------------

fn post(
    url: &str,
    headers: &[(String, String)],
    body: &Value,
    provider: &str,
) -> Result<Value, String> {
    let (status, response) = net::post_json(url, headers, &body.to_string())?;
    if !(200..300).contains(&status) {
        return Err(net::http_error(provider, status, &response));
    }
    serde_json::from_str(&response)
        .map_err(|e| format!("{provider} 响应不是 JSON：{e}\n{}", snippet(&response)))
}

fn bearer(key: &str) -> Vec<(String, String)> {
    if key.trim().is_empty() {
        Vec::new()
    } else {
        vec![(
            "Authorization".to_string(),
            format!("Bearer {}", key.trim()),
        )]
    }
}

fn model_name(model: &str) -> &str {
    let model = model.trim();
    if model.is_empty() {
        DEFAULT_MODEL
    } else {
        model
    }
}

fn openai_key_missing(base: &str, key: &str) -> bool {
    key.trim().is_empty() && (base.trim().is_empty() || base.trim().contains("api.openai.com"))
}

fn endpoint(base: &str, suffix: &str) -> String {
    let base = if base.trim().is_empty() {
        "https://api.openai.com/v1"
    } else {
        base.trim()
    };
    let base = base.trim_end_matches('/');
    if base.contains(suffix) {
        base.to_string()
    } else {
        format!("{base}/{suffix}")
    }
}

fn openai_text(value: &Value) -> Option<String> {
    let choice = value.get("choices")?.get(0)?;
    if let Some(text) = choice.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let content = choice.get("message")?.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let parts = content.as_array()?;
    let mut out = String::new();
    for part in parts {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            out.push_str(text);
        }
    }
    (!out.is_empty()).then_some(out)
}

fn extract_response(body: &str, path: &str, what: &str) -> Result<String, String> {
    let path = path.trim();
    if path.is_empty() {
        if let Ok(value) = serde_json::from_str::<Value>(body) {
            if let Some(text) = value.as_str() {
                return Ok(text.to_string());
            }
            return Err(format!(
                "{what} 响应是 JSON，请在设置里填写响应路径（例如 data.text）"
            ));
        }
        return Ok(body.to_string());
    }
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("{what} 响应不是 JSON：{e}"))?;
    let found = value_at_path(&value, path)
        .ok_or_else(|| format!("{what} 响应中找不到路径 {path}：{}", snippet(body)))?;
    match found {
        Value::String(text) => Ok(text.clone()),
        Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item
                    .as_str()
                    .or_else(|| item.get("text").and_then(Value::as_str))
                {
                    out.push_str(text);
                }
            }
            if out.is_empty() {
                Err(format!("{what} 响应路径 {path} 不是文本"))
            } else {
                Ok(out)
            }
        }
        Value::Object(map) => map
            .get("text")
            .or_else(|| map.get("content"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("{what} 响应路径 {path} 不是文本")),
        _ => Err(format!("{what} 响应路径 {path} 不是文本")),
    }
}

fn value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        current = match segment.parse::<usize>() {
            Ok(index) => current.get(index)?,
            Err(_) => current.get(segment)?,
        };
    }
    Some(current)
}

fn parse_headers(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            if !name.is_empty() {
                out.push((name.to_string(), value.trim().to_string()));
            }
        }
    }
    out
}

/// Replace `{{name}}` placeholders. Text placeholders are JSON-escaped by the
/// caller, so a body template can safely put `{{text}}` inside quotes.
fn fill_template(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (key, value) in pairs {
        out = out.replace(&format!("{{{{{key}}}}}"), value);
    }
    out
}

fn json_escape(text: &str) -> String {
    let quoted = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
    quoted[1..quoted.len().saturating_sub(1)].to_string()
}

fn strip_fences(text: &str) -> String {
    let text = text.trim();
    if !text.starts_with("```") {
        return text.to_string();
    }
    let inner = text
        .strip_prefix("```")
        .and_then(|s| s.split_once('\n').map(|(_, rest)| rest))
        .unwrap_or(text);
    let inner = inner.strip_suffix("```").unwrap_or(inner);
    inner.trim().to_string()
}

fn snippet(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(180)
        .collect()
}

fn fit_for_api(bgra: Vec<u8>, w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    if w <= MAX_API_SIDE && h <= MAX_API_SIDE {
        return (bgra, w, h);
    }
    let scale = MAX_API_SIDE as f32 / w.max(h) as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    let src_stride = w as usize * 4;
    let dst_stride = nw as usize * 4;
    let mut out = vec![0u8; dst_stride * nh as usize];
    for y in 0..nh as usize {
        let sy = (y as u64 * h as u64 / nh as u64) as usize;
        for x in 0..nw as usize {
            let sx = (x as u64 * w as u64 / nw as u64) as usize;
            let so = sy * src_stride + sx * 4;
            let do_ = y * dst_stride + x * 4;
            out[do_..do_ + 4].copy_from_slice(&bgra[so..so + 4]);
        }
    }
    (out, nw, nh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    #[test]
    fn endpoint_accepts_base_or_full_path() {
        assert_eq!(
            endpoint("https://api.openai.com/v1/", "chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint(
                "https://api.openai.com/v1/chat/completions",
                "chat/completions"
            ),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint(
                "https://api.example.com/openai/deployments/x/chat/completions?api-version=1",
                "chat/completions"
            ),
            "https://api.example.com/openai/deployments/x/chat/completions?api-version=1"
        );
        assert_eq!(
            endpoint("", "chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn json_path_reads_objects_and_arrays() {
        let value: Value = serde_json::json!({
            "choices": [{"message": {"content": "hello"}}],
            "data": {"text": "世界"}
        });
        assert_eq!(
            value_at_path(&value, "choices.0.message.content").and_then(Value::as_str),
            Some("hello")
        );
        assert_eq!(
            value_at_path(&value, "data.text").and_then(Value::as_str),
            Some("世界")
        );
    }

    #[test]
    fn custom_template_escapes_quotes_and_newlines() {
        let escaped = json_escape("a \"quote\"\nline");
        assert!(!escaped.contains('\n'));
        assert!(escaped.contains("\\\""));
        let body = fill_template("{\"q\":\"{{text}}\"}", &[("text", &escaped)]);
        assert!(serde_json::from_str::<Value>(&body).is_ok());
    }

    #[test]
    fn fences_are_removed() {
        assert_eq!(strip_fences("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_fences("plain"), "plain");
    }

    #[test]
    fn headers_parse_one_per_line() {
        let headers = parse_headers("Authorization: Bearer x\nX-Test: 1\n# ignore\n");
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].0, "Authorization");
    }

    #[test]
    fn openai_content_array_is_joined() {
        let value: Value = serde_json::json!({
            "choices": [{"message": {"content": [
                {"type": "text", "text": "你"},
                {"type": "text", "text": "好"}
            ]}}]
        });
        assert_eq!(openai_text(&value).as_deref(), Some("你好"));
    }

    /// One-shot HTTP server used by the provider round-trip tests. It returns
    /// the captured request so header/body assembly can be asserted.
    fn mock_once(response_body: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).expect("read");
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&data[..pos]).to_ascii_lowercase();
                    let need = head
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if data.len() >= pos + 4 + need {
                        break;
                    }
                }
            }
            let request = String::from_utf8_lossy(&data).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).expect("write");
            request
        });
        (port, server)
    }

    /// Exercises the whole custom-provider path against a local HTTP server:
    /// template expansion, WinHTTP POST, JSON path and response extraction.
    #[test]
    #[ignore]
    fn custom_translate_round_trip_through_winhttp() {
        let (port, server) = mock_once("{\"data\":{\"text\":\"你好，世界\"}}");
        let mut cfg = config::Translate::default();
        cfg.provider = "custom".to_string();
        cfg.custom_url = format!("http://127.0.0.1:{port}/x");
        cfg.custom_body = "{\"q\":\"{{text}}\"}".to_string();
        cfg.custom_path = "data.text".to_string();
        let out = translate_with(&cfg, "hello world", Direction::EnZh).expect("translate");
        assert_eq!(out, "你好，世界");
        let request = server.join().unwrap();
        assert!(request.contains("\"q\":\"hello world\""), "{request}");
    }

    #[test]
    #[ignore]
    fn openai_translate_round_trip() {
        let (port, server) = mock_once("{\"choices\":[{\"message\":{\"content\":\"你好\"}}]}");
        let mut cfg = config::Translate::default();
        cfg.provider = "openai".to_string();
        cfg.openai_base_url = format!("http://127.0.0.1:{port}/v1");
        cfg.openai_model = "mock-model".to_string();
        let out = translate_with(&cfg, "hello", Direction::EnZh).expect("translate");
        assert_eq!(out, "你好");
        let request = server.join().unwrap();
        assert!(request.contains("POST /v1/chat/completions"), "{request}");
        assert!(request.contains("\"model\":\"mock-model\""), "{request}");
    }

    #[test]
    #[ignore]
    fn libre_translate_round_trip() {
        let (port, server) = mock_once("{\"translatedText\":\"你好\"}");
        let mut cfg = config::Translate::default();
        cfg.provider = "libretranslate".to_string();
        cfg.libre_url = format!("http://127.0.0.1:{port}/translate");
        let out = translate_with(&cfg, "hello", Direction::EnZh).expect("translate");
        assert_eq!(out, "你好");
        let request = server.join().unwrap();
        assert!(request.contains("\"source\":\"en\""), "{request}");
        assert!(request.contains("\"target\":\"zh\""), "{request}");
    }

    #[test]
    #[ignore]
    fn openai_ocr_round_trip() {
        let (port, server) = mock_once("{\"choices\":[{\"message\":{\"content\":\"识别结果\"}}]}");
        let mut cfg = config::Ocr::default();
        cfg.provider = "openai".to_string();
        cfg.openai_base_url = format!("http://127.0.0.1:{port}/v1");
        cfg.openai_model = "mock-vision".to_string();
        let bgra = vec![255u8; 4 * 4 * 4];
        let (text, badge) = ocr_with(&cfg, bgra, 4, 4).expect("ocr");
        assert_eq!(text, "识别结果");
        assert_eq!(badge, "在线 OCR");
        let request = server.join().unwrap();
        assert!(request.contains("data:image/png;base64,"), "{request}");
    }
}
