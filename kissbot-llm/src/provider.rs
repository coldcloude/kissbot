use std::sync::Arc;
use std::time::Duration;

use arcstr::ArcStr;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::sleep;

use crate::config::{EffectiveLLMConfig, EffectiveModelConfig, ProviderConfig};
use crate::error::{Error, Result};
use crate::types::{Message, ModelResponse, ToolCall, ToolConfig};

/// Provider 抽象：负责向模型服务商发一次请求并解析响应
#[async_trait]
pub trait Provider: Send + Sync {
    // 可复用 request 类型
    type Request;
    fn build_request(&self, llm_cfg: &EffectiveLLMConfig, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Self::Request;
    async fn send_request(&self, request: &Self::Request, timeout: u64) -> Result<Value>;
    fn parse_response(&self, data: Value) -> ModelResponse;
    /// 从服务商 API 获取全部可用模型名（GET /models）
    async fn list_models(&self) -> Result<Vec<ArcStr>>;
}

/// 按 provider 配置构造 OpenAI 兼容实现（调用方按 provider_type 分派）
pub fn create_openai_provider(provider_config: &ProviderConfig) -> OpenAiProvider {
    OpenAiProvider {
        client: Arc::new(Client::new()),
        base_url: provider_config.base_url.clone(),
        api_key: provider_config.api_key.clone(),
    }
}

/// 按 provider 配置构造 Anthropic 实现（调用方按 provider_type 分派）
pub fn create_anthropic_provider(provider_config: &ProviderConfig) -> AnthropicProvider {
    AnthropicProvider {
        client: Arc::new(Client::new()),
        base_url: provider_config.base_url.clone(),
        api_key: provider_config.api_key.clone(),
    }
}

/// 指数退避重试（retry_count / timeout_secs 来自有效模型配置）后完成一次调用。
/// 配置类错误（如未知 provider_type）是永久性错误，重试无意义，直接返回
pub async fn call_with_retry<P: Provider>(
    provider: &P,
    llm_cfg: &EffectiveLLMConfig,
    pm_cfg: &EffectiveModelConfig,
    messages: Vec<Message>,
    tools: &Vec<Arc<ToolConfig>>,
) -> Result<ModelResponse> {
    let mut last_error = None;
    let request = provider.build_request(llm_cfg, messages, tools);
    for attempt in 0..=pm_cfg.retry_count {
        match provider.send_request(&request, pm_cfg.timeout_secs).await {
            Ok(response) => return Ok(provider.parse_response(response)),
            // 配置类错误（如未知 provider_type）是永久性错误，重试无意义，直接返回
            Err(e @ Error::ModelProviderNotSupported(_)) => return Err(e),
            Err(e) => {
                last_error = Some(e);
                if attempt < pm_cfg.retry_count {
                    sleep(Duration::from_secs(1u64 << attempt)).await; // 指数退避
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| Error::ModelApiError("模型调用失败".into())))
}

// ========== OpenAI 兼容协议（/chat/completions） ==========

pub struct OpenAiProvider {
    client: Arc<Client>,
    base_url: ArcStr,
    api_key: ArcStr,
}

/// OpenAI 兼容上下文消息：type 即枚举变体（内部标签序列化，type 与其他字段平级）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum OpenAiRequestToolCall {
    Function { function: Arc<ToolConfig> },
}

/// OpenAI 兼容上下文消息：type 即枚举变体（内部标签序列化，type 与其他字段平级）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum OpenAiRequestThinking {
    Enabled,
    Disabled,
}

/// OpenAI 兼容请求体（`Provider::Request`，字段私有：调用方不直接构造）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiRequest {
    model: ArcStr,
    messages: Vec<Message>,
    stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiRequestToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thinking: Option<OpenAiRequestThinking>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<ArcStr>,
}

/// 匹配 content 开头的 <think>...</think>（允许前导空白），剥离并返回 (剥离后内容, Option<思考内容>)
/// 标签不在开头或未闭合时原样返回
fn strip_think_tag(content: &str) -> (Option<ArcStr>, Option<ArcStr>) {
    let trimmed = content.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            let thinking = ArcStr::from(&rest[..end]);
            let stripped = ArcStr::from(&rest[end + "</think>".len()..]);
            return (Some(stripped), Some(thinking));
        }
    }
    (Some(ArcStr::from(content)), None)
}

#[async_trait]
impl Provider for OpenAiProvider {
    type Request = OpenAiRequest;

    fn build_request(&self, llm_cfg: &EffectiveLLMConfig, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> OpenAiRequest {
        // Tools 需要转化
        let result_tools = if !tools.is_empty() {
            let mut results = Vec::new();
            for t in tools.iter() {
                results.push(OpenAiRequestToolCall::Function { function: t.clone() });
            }
            Some(results)
        } else {
            None
        };
        // Thinking 需要转化
        let thinking = if let Some(t) = llm_cfg.thinking.as_ref() {
            match t.as_str() {
                "enabled" => Some(OpenAiRequestThinking::Enabled),
                "disabled" => Some(OpenAiRequestThinking::Disabled),
                _ => None,
            }
        } else {
            None
        };
        // Messages 序列化即 OpenAI 格式
        OpenAiRequest {
            model: llm_cfg.model.clone(),
            messages,
            stream: false,
            tools: result_tools,
            max_tokens: llm_cfg.max_tokens.clone(),
            temperature: llm_cfg.temperature.clone(),
            thinking,
            reasoning_effort: llm_cfg.reasoning_effort.clone(),
        }
    }

    fn parse_response(&self, mut data: Value) -> ModelResponse {
        // usage.total_tokens：本次请求 prompt+completion token 总占用（DeepSeek/Kimi 均返回）；缺字段回退 0
        let total_tokens = data["usage"]["total_tokens"].as_u64().unwrap_or(0);
        let mut choice = data["choices"][0].take();
        let mut message = choice["message"].take();
        let raw_content = message["content"].take();
        let raw_reasoning_content = message["reasoning_content"].take();
        let raw_tool_calls = message["tool_calls"].take();
        let finish_reason = if let Value::String(str) = choice["finish_reason"].take() { Some(ArcStr::from(str)) } else { None };
        // <think> 标签总剥离；thinking 独立取标签内容（无标签 → None；空标签 → Some("")）；
        // reasoning_content 独立取 API 字段（字段缺失/非字符串 → None；空串 → Some("")）
        let (content, thinking) = if let Some(c_str) = raw_content.as_str() { strip_think_tag(c_str) } else { (None, None) };
        let reasoning_content = if let Some(rc_str) = raw_reasoning_content.as_str() { Some(ArcStr::from(rc_str)) } else { None };
        // tool_calls：OpenAI function call 数组（含 thinking 模式下多轮工具调用）
        let mut tool_calls = None;
        if let Some(calls) = raw_tool_calls.as_array() {
            let mut tcs = Vec::new();
            for v in calls {
                if let Some(id) = v["id"].as_str() {
                    if let Some(name) = v["function"]["name"].as_str() {
                        tcs.push(ToolCall {
                            id: ArcStr::from(id),
                            name: ArcStr::from(name),
                            arguments: v["function"]["arguments"].clone(),
                            result_arguments: None,
                            result: None,
                        });
                    }
                }
            }
            tool_calls = Some(tcs);
        }
        ModelResponse {
            raw_content,
            content,
            thinking,
            raw_reasoning_content,
            reasoning_content,
            raw_tool_calls,
            tool_calls,
            finish_reason,
            total_tokens,
        }
    }

    async fn send_request(&self, request: &OpenAiRequest, timeout: u64) -> Result<Value> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url)
            .timeout(Duration::from_secs(timeout))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(request)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("OpenAI API {}: {}", status, text).into()));
        }
        let data: Value = resp.json().await?;
        Ok(data)
    }

    async fn list_models(&self) -> Result<Vec<ArcStr>> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let resp = self.client.get(&url)
            .timeout(Duration::from_secs(30))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("OpenAI models API {}: {}", status, text).into()));
        }
        let data: serde_json::Value = resp.json().await?;
        let models: Vec<ArcStr> = data["data"].as_array()
        .map(|arr| arr.iter().filter_map(|m| m["id"].as_str().map(ArcStr::from)).collect())
        .unwrap_or_default();
        Ok(models)
    }
}

// ========== Anthropic 协议（/v1/messages） ==========

const DEFFAULT_ANTTHROPIC_MAX_TOKENS: u32 = 65535;

pub struct AnthropicProvider {
    client: Arc<Client>,
    base_url: ArcStr,
    api_key: ArcStr,
}

#[async_trait]
impl Provider for AnthropicProvider {
    type Request = Value;

    fn build_request(&self, llm_cfg: &EffectiveLLMConfig, messages: Vec<Message>, _tools: &Vec<Arc<ToolConfig>>) -> Value {
        // 分离 system 消息
        let system_parts: Vec<ArcStr> = messages.iter()
            .filter_map(|m| match m {
                Message::System { content } => Some(content.clone()),
                _ => None,
            })
            .collect();
        let system = system_parts.join("\n");

        let msgs: Vec<serde_json::Value> = messages.iter().filter_map(|m| match m {
            Message::System { .. } => None,
            Message::User { content } => Some(json!({ "role": "user", "content": content })),
            Message::Assistant { content, .. } => Some(json!({ "role": "assistant", "content": content })),
            Message::Tool { .. } => None,  // 本轮不支持工具消息
        }).collect();

        let mut body = json!({
            "model": llm_cfg.model,
            "messages": msgs,
            "max_tokens": llm_cfg.max_tokens.unwrap_or(DEFFAULT_ANTTHROPIC_MAX_TOKENS),
        });
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        // 可选参数：有值才传（temperature / thinking / output_config.effort）
        if let Some(t) = llm_cfg.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(t) = &llm_cfg.thinking {
            body["thinking"] = json!({ "type": t });
        }
        if let Some(e) = &llm_cfg.reasoning_effort {
            body["output_config"] = json!({ "effort": e });
        }
        body
    }

    fn parse_response(&self, mut data: Value) -> ModelResponse {
        // reasoning_content：thinking block 内容（无 block → None；空串 block → Some("")）
        let mut reasoning_content = None;
        let mut content = None;
        let raw_content = data["content"].take();
        if let Some(blocks) = raw_content.as_array() {
            for block in blocks {
                match block["type"].as_str() {
                    Some("thinking") if reasoning_content.is_none() => {
                        if let Some(rc) = block["thinking"].as_str() {
                            reasoning_content = Some(ArcStr::from(rc));
                        }
                    }
                    Some("text") if content.is_none() => {
                        if let Some(c) = block["text"].as_str() {
                            content = Some(ArcStr::from(c));
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut finish_reason = None;
        if let Some(sr) = data["stop_reason"].as_str() {
            finish_reason = Some(ArcStr::from(sr));
        }
        // <think> 标签总剥离；thinking 独立取标签内容
        let (content, thinking) = if let Some(c) = content {
            strip_think_tag(c.as_str())
        } else {
            (None, None)
        };
        ModelResponse {
            raw_content,
            content,
            thinking,
            raw_reasoning_content: Value::Null,
            reasoning_content,
            raw_tool_calls: Value::Null,
            tool_calls: None,
            finish_reason,
            total_tokens: 0,   // Anthropic API 文档未找到，暂固定 0（永不触发重置）
        }
    }

    async fn send_request(&self, request: &Value, timeout: u64) -> Result<Value> {
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url)
            .timeout(Duration::from_secs(timeout))
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", "2023-06-01")
            .json(request)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("Anthropic API {}: {}", status, text).into()));
        }
        let data: Value = resp.json().await?;
        Ok(data)
    }

    async fn list_models(&self) -> Result<Vec<ArcStr>> {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        let resp = self.client.get(&url)
            .timeout(Duration::from_secs(30))
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", "2023-06-01")
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("Anthropic models API {}: {}", status, text).into()));
        }
        let data: serde_json::Value = resp.json().await?;
        let models = data["data"].as_array()
        .map(|arr| arr.iter().filter_map(|m| m["id"].as_str().map(ArcStr::from)).collect())
        .unwrap_or_default();
        Ok(models)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    // ===== 构造助手 =====

    /// OpenAI 实现实例（client/base_url/api_key 仅占位：build_request/parse_response 不触网）
    fn openai() -> OpenAiProvider {
        OpenAiProvider {
            client: Arc::new(Client::new()),
            base_url: ArcStr::from("https://api.deepseek.com"),
            api_key: ArcStr::from("sk-test"),
        }
    }

    /// Anthropic 实现实例（同 openai()，仅用于 build_request/parse_response）
    fn anthropic() -> AnthropicProvider {
        AnthropicProvider {
            client: Arc::new(Client::new()),
            base_url: ArcStr::from("https://api.anthropic.com"),
            api_key: ArcStr::from("sk-test"),
        }
    }

    /// 有效 LLM 参数：provider 与 model 为独立字段
    fn sample_llm_cfg() -> EffectiveLLMConfig {
        EffectiveLLMConfig {
            provider: ArcStr::from("deepseek"),
            model: ArcStr::from("deepseek-4-flash"),
            max_tokens: Some(2048),
            temperature: Some(0.3),
            thinking: None,
            reasoning_effort: None,
        }
    }

    /// 有效模型配置（重试/超时/上限；provider_config 仅占位）
    fn sample_pm_cfg() -> EffectiveModelConfig {
        EffectiveModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("openai"),
                base_url: ArcStr::from("https://api.deepseek.com"),
                api_key: ArcStr::from("sk-test"),
            }),
            max_tokens_usage: 128000,
            timeout_secs: 5,
            retry_count: 0,
        }
    }

    // Message 构造测试助手（字段为 ArcStr，集中构造减少噪音）
    fn sys(content: &str) -> Message {
        Message::System { content: ArcStr::from(content) }
    }

    fn usr(content: &str) -> Message {
        Message::User { content: ArcStr::from(content) }
    }

    /// Assistant 消息：content/reasoning_content/tool_calls 均为 Value（Null 由 skip_serializing_if 省略），
    /// 与 output_processor 用 raw_* 构造上下文消息的方式一致
    fn assistant(content: Value, reasoning_content: Value, tool_calls: Value) -> Message {
        Message::Assistant { content, reasoning_content, tool_calls }
    }

    /// Tool 消息（无 name 字段：工具名由 assistant.tool_calls 承载）
    fn tool_msg(id: &str, content: &str) -> Message {
        Message::Tool { tool_call_id: ArcStr::from(id), content: ArcStr::from(content) }
    }

    /// 工具定义（parameters 为 Arc<Value>）
    fn read_tool() -> Arc<ToolConfig> {
        Arc::new(ToolConfig {
            name: ArcStr::from("read"),
            description: ArcStr::from("读取文本文件"),
            parameters: Arc::new(json!({ "type": "object" })),
        })
    }

    /// tool_calls 的 wire 形状：raw_tool_calls 原样透传（与 API 返回 JSON 一致，arguments 为 JSON 字符串）
    fn wire_tool_calls() -> Value {
        json!([{ "id": "c1", "type": "function", "function": { "name": "read", "arguments": "{\"path\":\"/a\"}" } }])
    }

    /// Option<ArcStr> → Option<&str>（断言助手）
    fn text(value: &Option<ArcStr>) -> Option<&str> {
        value.as_deref()
    }

    /// strip_think_tag 结果转 String（便于断言）
    fn think(content: &str) -> (Option<String>, Option<String>) {
        let (stripped, thinking) = strip_think_tag(content);
        (
            stripped.map(|s| s.as_str().to_string()),
            thinking.map(|s| s.as_str().to_string()),
        )
    }

    #[test]
    fn create_providers_wire_config() {
        // create_* 从 provider 配置取连接信息（调用方按 provider_type 分派）
        let cfg = ProviderConfig {
            provider_type: ArcStr::from("openai"),
            base_url: ArcStr::from("https://api.example.com"),
            api_key: ArcStr::from("sk-abc"),
        };
        let oa = create_openai_provider(&cfg);
        assert_eq!(oa.base_url.as_str(), "https://api.example.com");
        assert_eq!(oa.api_key.as_str(), "sk-abc");
        let an = create_anthropic_provider(&cfg);
        assert_eq!(an.base_url.as_str(), "https://api.example.com");
        assert_eq!(an.api_key.as_str(), "sk-abc");
    }

    /// 可编排的假 Provider：前 fail_times 次返回 api_err（或 permanent 错误），之后返回固定响应
    struct FlakyProvider {
        attempts: AtomicU32,
        fail_times: u32,
        permanent: bool,
    }

    #[async_trait]
    impl Provider for FlakyProvider {
        type Request = Value;

        fn build_request(&self, _llm_cfg: &EffectiveLLMConfig, _messages: Vec<Message>, _tools: &Vec<Arc<ToolConfig>>) -> Value {
            json!({})
        }

        async fn send_request(&self, _request: &Value, _timeout: u64) -> Result<Value> {
            let attempt = self.attempts.fetch_add(1, Ordering::Relaxed);
            if attempt < self.fail_times {
                return if self.permanent {
                    Err(Error::ModelProviderNotSupported("未知 provider_type: typo".into()))
                } else {
                    Err(Error::ModelApiError("boom".into()))
                };
            }
            Ok(json!({ "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }] }))
        }

        fn parse_response(&self, data: Value) -> ModelResponse {
            openai().parse_response(data)
        }

        async fn list_models(&self) -> Result<Vec<ArcStr>> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn call_with_retry_retries_until_success() {
        // 失败一次后成功：重试 1 次后拿到响应（含 1s 指数退避）
        let provider = FlakyProvider { attempts: AtomicU32::new(0), fail_times: 1, permanent: false };
        let mut pm_cfg = sample_pm_cfg();
        pm_cfg.retry_count = 1;
        let resp = call_with_retry(&provider, &sample_llm_cfg(), &pm_cfg, vec![usr("你好")], &vec![]).await
            .expect("重试后应成功");
        assert_eq!(text(&resp.content), Some("答案"));
        assert_eq!(provider.attempts.load(Ordering::Relaxed), 2, "共发起两次请求");
    }

    #[tokio::test]
    async fn call_with_retry_returns_last_error_after_exhausting_retries() {
        // 重试次数用尽：返回最后一次错误（retry_count = 0 不重试，避免退避等待）
        let provider = FlakyProvider { attempts: AtomicU32::new(0), fail_times: u32::MAX, permanent: false };
        let err = call_with_retry(&provider, &sample_llm_cfg(), &sample_pm_cfg(), vec![usr("你好")], &vec![]).await
            .err().expect("应返回错误");
        assert!(matches!(err, Error::ModelApiError(_)));
        assert_eq!(provider.attempts.load(Ordering::Relaxed), 1, "retry_count = 0 只发一次");
    }

    #[tokio::test]
    async fn call_with_retry_does_not_retry_permanent_error() {
        // 未知 provider_type 属永久性错误：即使 retry_count > 0 也不重试
        let provider = FlakyProvider { attempts: AtomicU32::new(0), fail_times: u32::MAX, permanent: true };
        let mut pm_cfg = sample_pm_cfg();
        pm_cfg.retry_count = 3;
        let err = call_with_retry(&provider, &sample_llm_cfg(), &pm_cfg, vec![usr("你好")], &vec![]).await
            .err().expect("应返回错误");
        assert!(matches!(err, Error::ModelProviderNotSupported(_)));
        assert_eq!(provider.attempts.load(Ordering::Relaxed), 1, "永久性错误不重试");
    }

    #[test]
    fn openai_build_request_includes_params_and_messages() {
        let llm_cfg = sample_llm_cfg();
        let msgs = vec![sys("你是助手"), usr("你好")];
        let body = openai().build_request(&llm_cfg, msgs, &vec![]);
        assert_eq!(body.model.as_str(), "deepseek-4-flash");
        assert_eq!(body.max_tokens, Some(2048));
        // temperature 为 f32 字段，直接按 f32 精确值比较
        assert_eq!(body.temperature, Some(0.3_f32));
        assert!(!body.stream, "非流式请求");
        assert!(body.tools.is_none(), "无工具不构造 tools");
        assert!(body.thinking.is_none(), "未配置 thinking");
        assert!(body.reasoning_effort.is_none(), "未配置 reasoning_effort");
        assert!(matches!(&body.messages[0], Message::System { content } if content.as_str() == "你是助手"));
        assert!(matches!(&body.messages[1], Message::User { content } if content.as_str() == "你好"));
    }

    #[test]
    fn openai_build_request_omits_optional_params_when_none() {
        let mut llm_cfg = sample_llm_cfg();
        llm_cfg.max_tokens = None;
        llm_cfg.temperature = None;
        llm_cfg.thinking = None;
        llm_cfg.reasoning_effort = None;
        let body = openai().build_request(&llm_cfg, vec![usr("你好")], &vec![]);
        let body = serde_json::to_value(&body).unwrap();
        assert!(body.get("max_tokens").is_none(), "max_tokens 未配置不应传");
        assert!(body.get("temperature").is_none(), "temperature 未配置不应传");
        assert!(body.get("thinking").is_none(), "thinking 未配置不应传");
        assert!(body.get("reasoning_effort").is_none(), "reasoning_effort 未配置不应传");
        assert_eq!(body["model"], "deepseek-4-flash");
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn openai_build_request_maps_thinking_and_reasoning_effort() {
        let mut llm_cfg = sample_llm_cfg();
        llm_cfg.thinking = Some(ArcStr::from("enabled"));
        llm_cfg.reasoning_effort = Some(ArcStr::from("high"));
        let body = openai().build_request(&llm_cfg, vec![usr("你好")], &vec![]);
        let body = serde_json::to_value(&body).unwrap();
        // thinking 由字符串映射为协议枚举（内部标签 type）
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["temperature"].as_f64().unwrap() as f32, 0.3_f32);
    }

    #[test]
    fn openai_build_request_maps_disabled_thinking() {
        let mut llm_cfg = sample_llm_cfg();
        llm_cfg.thinking = Some(ArcStr::from("disabled"));
        let body = openai().build_request(&llm_cfg, vec![usr("你好")], &vec![]);
        let body = serde_json::to_value(&body).unwrap();
        assert_eq!(body["thinking"]["type"], "disabled");
    }

    #[test]
    fn openai_build_request_drops_unknown_thinking_value() {
        // 非 enabled/disabled 的取值无法映射到协议枚举 → 不发送 thinking 字段（而非原样透传）
        let mut llm_cfg = sample_llm_cfg();
        llm_cfg.thinking = Some(ArcStr::from("bogus"));
        let body = openai().build_request(&llm_cfg, vec![usr("你好")], &vec![]);
        assert!(body.thinking.is_none(), "未知取值不构造 thinking");
        let body = serde_json::to_value(&body).unwrap();
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn openai_build_request_includes_tools_when_present() {
        let llm_cfg = sample_llm_cfg();
        let tools = vec![read_tool()];
        let body = openai().build_request(&llm_cfg, vec![usr("查一下")], &tools);
        let body = serde_json::to_value(&body).unwrap();
        assert_eq!(body["tools"][0]["type"], "function", "工具以 function 类型包装");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        assert_eq!(body["tools"][0]["function"]["description"], "读取文本文件");
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn openai_build_request_omits_tools_when_empty() {
        let llm_cfg = sample_llm_cfg();
        let body = openai().build_request(&llm_cfg, vec![usr("你好")], &vec![]);
        assert!(body.tools.is_none(), "无工具不构造 tools");
        let body = serde_json::to_value(&body).unwrap();
        assert!(body.get("tools").is_none(), "无工具不应发送 tools 字段");
    }

    #[test]
    fn openai_build_request_passes_through_tool_and_result_messages() {
        // 上下文里的 assistant.tool_calls 是 raw_tool_calls（API 原始 JSON）原样透传，
        // 所以 wire 形状即协议形状：arguments 保持 JSON 字符串
        let llm_cfg = sample_llm_cfg();
        let msgs = vec![
            assistant(Value::Null, Value::Null, wire_tool_calls()),
            tool_msg("c1", "内容"),
        ];
        let body = openai().build_request(&llm_cfg, msgs, &vec![]);
        let body = serde_json::to_value(&body).unwrap();
        assert_eq!(body["messages"][0]["role"], "assistant");
        assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "c1");
        assert_eq!(body["messages"][0]["tool_calls"][0]["function"]["name"], "read");
        assert_eq!(body["messages"][0]["tool_calls"][0]["function"]["arguments"], r#"{"path":"/a"}"#, "arguments 保持 JSON 字符串");
        // content/reasoning_content 为 Null（无值）由 skip_serializing_if 省略
        assert!(body["messages"][0].get("content").is_none());
        assert!(body["messages"][0].get("reasoning_content").is_none());
        assert_eq!(body["messages"][1]["role"], "tool");
        assert_eq!(body["messages"][1]["tool_call_id"], "c1");
        assert_eq!(body["messages"][1]["content"], "内容");
        assert!(body["messages"][1].get("name").is_none(), "Message::Tool 已无 name 字段");
    }

    #[test]
    fn openai_build_request_serializes_reasoning_content_when_present() {
        // 格式能力：Message 序列化即 OpenAI 格式，reasoning_content 由格式自动序列化携带
        // （工具调用场景须回传思考内容，故上下文保留 raw_reasoning_content）
        let llm_cfg = sample_llm_cfg();
        let msgs = vec![
            sys("设定"),
            assistant(Value::String("回答".into()), Value::String("思考".into()), Value::Null),
        ];
        let body = openai().build_request(&llm_cfg, msgs, &vec![]);
        let body = serde_json::to_value(&body).unwrap();
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        assert_eq!(body["messages"][1]["role"], "assistant");
        assert_eq!(body["messages"][1]["content"], "回答");
        assert_eq!(body["messages"][1]["reasoning_content"], "思考", "格式自动序列化 reasoning_content");
        assert!(body["messages"][1].get("tool_calls").is_none(), "无 tool_calls 省略");
    }

    #[test]
    fn openai_parse_response_extracts_content_and_finish_reason() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
        });
        let resp = openai().parse_response(data);
        assert_eq!(text(&resp.content), Some("答案"));
        assert_eq!(text(&resp.finish_reason), Some("stop"));
        assert_eq!(text(&resp.reasoning_content), None, "无 reasoning_content 字段");
        assert_eq!(text(&resp.thinking), None, "无 <think> 标签");
        assert!(resp.tool_calls.is_none(), "无 tool_calls 字段");
        assert_eq!(resp.total_tokens, 0, "缺 usage 回退 0");
    }

    #[test]
    fn openai_parse_response_extracts_tool_calls() {
        let data = serde_json::json!({
            "choices": [{
                "message": { "content": null, "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "read", "arguments": "{\"path\":\"/a\"}" } }] },
                "finish_reason": "tool_calls"
            }]
        });
        let resp = openai().parse_response(data.clone());
        let tool_calls = resp.tool_calls.as_ref().expect("应解析出 tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id.as_str(), "c1");
        assert_eq!(tool_calls[0].name.as_str(), "read");
        // arguments 原样保留 API 返回的 JSON 字符串（不解析为对象）
        assert_eq!(tool_calls[0].arguments.as_str(), Some(r#"{"path":"/a"}"#));
        assert!(tool_calls[0].result.is_none(), "执行前 result 为空");
        assert_eq!(resp.raw_tool_calls, data["choices"][0]["message"]["tool_calls"], "raw_tool_calls 原样保留");
        assert!(resp.content.is_none(), "content 为 null → None");
        assert_eq!(text(&resp.finish_reason), Some("tool_calls"));
    }

    #[test]
    fn openai_parse_response_no_tool_calls_by_default() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
        });
        let resp = openai().parse_response(data);
        assert!(resp.tool_calls.is_none(), "无 tool_calls 字段时为 None");
        assert!(resp.raw_tool_calls.is_null(), "raw_tool_calls 为 Null");
    }

    #[test]
    fn openai_parse_response_skips_tool_call_missing_id_or_name() {
        // 容错：缺 id 或 function.name 的条目跳过（不整体失败）
        let data = serde_json::json!({
            "choices": [{
                "message": { "content": "答案", "tool_calls": [
                    { "function": { "name": "read", "arguments": "{}" } },
                    { "id": "c2", "function": { "arguments": "{}" } },
                    { "id": "c3", "function": { "name": "write", "arguments": "{}" } }
                ] },
                "finish_reason": "tool_calls"
            }]
        });
        let resp = openai().parse_response(data);
        let tool_calls = resp.tool_calls.as_ref().expect("应解析出 tool_calls");
        assert_eq!(tool_calls.len(), 1, "仅完整条目保留");
        assert_eq!(tool_calls[0].id.as_str(), "c3");
    }

    #[test]
    fn anthropic_build_request_separates_system_messages() {
        let llm_cfg = sample_llm_cfg();
        let msgs = vec![sys("设定"), usr("hi")];
        let body = anthropic().build_request(&llm_cfg, msgs, &vec![]);
        assert_eq!(body["system"], "设定");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1, "system 不应出现在 messages");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert_eq!(body["max_tokens"], 2048);
        assert_eq!(body["model"], "deepseek-4-flash");
    }

    #[test]
    fn anthropic_build_request_joins_multiple_system_messages() {
        let llm_cfg = sample_llm_cfg();
        let body = anthropic().build_request(&llm_cfg, vec![sys("第一段"), sys("第二段"), usr("hi")], &vec![]);
        assert_eq!(body["system"], "第一段\n第二段", "多段 system 以换行拼接");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn anthropic_build_request_defaults_max_tokens_when_unset() {
        // Anthropic API 要求 max_tokens 必填：未配置时用默认常量
        let mut llm_cfg = sample_llm_cfg();
        llm_cfg.max_tokens = None;
        let body = anthropic().build_request(&llm_cfg, vec![usr("hi")], &vec![]);
        assert_eq!(body["max_tokens"], DEFFAULT_ANTTHROPIC_MAX_TOKENS);
    }

    #[test]
    fn anthropic_build_request_omits_optional_params_when_none() {
        let mut llm_cfg = sample_llm_cfg();
        llm_cfg.temperature = None;
        llm_cfg.thinking = None;
        llm_cfg.reasoning_effort = None;
        let body = anthropic().build_request(&llm_cfg, vec![usr("hi")], &vec![]);
        assert!(body.get("temperature").is_none(), "temperature 未配置不应传");
        assert!(body.get("thinking").is_none(), "thinking 未配置不应传");
        assert!(body.get("output_config").is_none(), "reasoning_effort 未配置不应传 output_config");
    }

    #[test]
    fn anthropic_build_request_omits_system_when_absent() {
        let llm_cfg = sample_llm_cfg();
        let body = anthropic().build_request(&llm_cfg, vec![usr("hi")], &vec![]);
        assert!(body.get("system").is_none(), "无 system 消息不应发送 system 字段");
    }

    #[test]
    fn anthropic_build_request_drops_tool_messages() {
        // Anthropic 分支本轮不支持工具消息：system 归 system，Tool 消息丢弃
        let llm_cfg = sample_llm_cfg();
        let msgs = vec![
            usr("hi"),
            tool_msg("c1", "内容"),
            assistant(Value::String("回答".into()), Value::Null, wire_tool_calls()),
        ];
        let body = anthropic().build_request(&llm_cfg, msgs, &vec![]);
        assert_eq!(body["messages"].as_array().unwrap().len(), 2, "Tool 消息被丢弃");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][1]["role"], "assistant");
        assert_eq!(body["messages"][1]["content"], "回答");
    }

    #[test]
    fn anthropic_build_request_passes_thinking_and_output_config() {
        let mut llm_cfg = sample_llm_cfg();
        llm_cfg.thinking = Some(ArcStr::from("enabled"));
        llm_cfg.reasoning_effort = Some(ArcStr::from("high"));
        let body = anthropic().build_request(&llm_cfg, vec![usr("hi")], &vec![]);
        assert_eq!(body["thinking"]["type"], "enabled", "thinking 原样作为 type");
        assert_eq!(body["output_config"]["effort"], "high", "reasoning_effort 映射为 output_config.effort");
        assert_eq!(body["temperature"].as_f64().unwrap() as f32, 0.3_f32);
    }

    #[test]
    fn anthropic_parse_response_extracts_text_and_stop_reason() {
        let data = serde_json::json!({
            "content": [{ "type": "text", "text": "答复" }],
            "stop_reason": "end_turn"
        });
        let resp = anthropic().parse_response(data);
        assert_eq!(text(&resp.content), Some("答复"));
        assert_eq!(text(&resp.finish_reason), Some("end_turn"));
        assert!(resp.tool_calls.is_none(), "anthropic 分支暂不解析工具调用");
        assert!(resp.reasoning_content.is_none());
        assert!(resp.thinking.is_none());
    }


    #[test]
    fn strip_think_tag_extracts_and_removes_leading_tag() {
        assert_eq!(think("<think>让我想想</think>答案"), (Some("答案".to_string()), Some("让我想想".to_string())));
    }

    #[test]
    fn strip_think_tag_keeps_non_leading_tag() {
        // 标签不在开头（含剥标签后剩下的内容）→ 原样返回，不提取思考
        let content = "答案<think>思考</think>";
        assert_eq!(think(content), (Some(content.to_string()), None));
    }

    #[test]
    fn strip_think_tag_allows_leading_whitespace() {
        assert_eq!(think("\n<think>思考</think>答案"), (Some("答案".to_string()), Some("思考".to_string())));
    }

    #[test]
    fn strip_think_tag_returns_unchanged_when_no_tag() {
        assert_eq!(think("普通文本"), (Some("普通文本".to_string()), None));
        assert_eq!(think(""), (Some(String::new()), None), "空串有值但无思考");
    }

    #[test]
    fn strip_think_tag_keeps_unclosed_tag() {
        // 缺 </think> 闭合标签 → 原样返回（不做半截解析）
        assert_eq!(think("<think>未闭合"), (Some("<think>未闭合".to_string()), None));
    }

    #[test]
    fn strip_think_tag_extracts_empty_thinking() {
        // 空标签：剥出空思考内容，剩余内容为空串
        assert_eq!(think("<think></think>答案"), (Some("答案".to_string()), Some(String::new())));
    }

    #[test]
    fn openai_parse_response_reasoning_and_thinking_independent() {
        // API 有 reasoning_content + content 有 <think> 标签 → 两字段独立共存
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "<think>标签思考</think>答案", "reasoning_content": "API推理" }, "finish_reason": "stop" }]
        });
        let resp = openai().parse_response(data);
        assert_eq!(text(&resp.content), Some("答案"), "<think> 标签应剥离");
        assert_eq!(text(&resp.reasoning_content), Some("API推理"), "reasoning_content 独立取 API 字段");
        assert_eq!(text(&resp.thinking), Some("标签思考"), "thinking 独立取标签内容");
    }

    #[test]
    fn openai_parse_response_only_thinking_when_no_api_field() {
        // 无 API reasoning_content + <think> 标签 → reasoning_content=None，thinking=标签内容
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "<think>思考</think>答案" }, "finish_reason": "stop" }]
        });
        let resp = openai().parse_response(data);
        assert!(resp.reasoning_content.is_none(), "无 API 字段 → None");
        assert_eq!(text(&resp.thinking), Some("思考"));
    }

    #[test]
    fn openai_parse_response_extracts_reasoning_content() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案", "reasoning_content": "思考" }, "finish_reason": "stop" }]
        });
        let resp = openai().parse_response(data);
        assert_eq!(text(&resp.content), Some("答案"));
        assert_eq!(text(&resp.reasoning_content), Some("思考"));
        assert!(resp.thinking.is_none(), "仅 API 字段无标签时 thinking 为 None");
    }

    #[test]
    fn openai_parse_response_falls_back_to_think_tag() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "<think>思考</think>答案" }, "finish_reason": "stop" }]
        });
        let resp = openai().parse_response(data);
        assert_eq!(text(&resp.content), Some("答案"), "<think> 标签应剥离");
        assert!(resp.reasoning_content.is_none(), "标签内容不合并到 reasoning_content");
        assert_eq!(text(&resp.thinking), Some("思考"), "标签内容独立取 thinking");
    }

    #[test]
    fn openai_parse_response_empty_api_reasoning_is_kept_as_empty_string() {
        // 空串 reasoning_content 按「有该字段」处理：值为空串（非 None）
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "<think>思考</think>答案", "reasoning_content": "" }, "finish_reason": "stop" }]
        });
        let resp = openai().parse_response(data);
        assert_eq!(text(&resp.content), Some("答案"), "<think> 标签应剥离");
        assert_eq!(text(&resp.reasoning_content), Some(""), "空串 reasoning_content 取值为空串");
        assert_eq!(text(&resp.thinking), Some("思考"), "标签内容独立取 thinking");
    }

    #[test]
    fn openai_parse_response_no_thinking_when_both_empty() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
        });
        let resp = openai().parse_response(data);
        assert!(resp.reasoning_content.is_none());
        assert!(resp.thinking.is_none());
    }

    #[test]
    fn openai_parse_response_keeps_raw_fields() {
        // raw_* 保留 API 原始值（上下文按 raw_* 回填，保证回传格式与 API 一致）
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "<think>思考</think>答案", "reasoning_content": "API推理" }, "finish_reason": "stop" }]
        });
        let resp = openai().parse_response(data);
        assert_eq!(resp.raw_content, serde_json::json!("<think>思考</think>答案"), "raw_content 未被剥标签");
        assert_eq!(resp.raw_reasoning_content, serde_json::json!("API推理"));
    }

    #[test]
    fn anthropic_parse_response_reasoning_and_thinking_independent() {
        let data = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "API推理" },
                { "type": "text", "text": "<think>标签思考</think>答复" }
            ],
            "stop_reason": "end_turn"
        });
        let resp = anthropic().parse_response(data);
        assert_eq!(text(&resp.content), Some("答复"));
        assert_eq!(text(&resp.reasoning_content), Some("API推理"));
        assert_eq!(text(&resp.thinking), Some("标签思考"));
    }

    #[test]
    fn anthropic_parse_response_extracts_thinking_block() {
        let data = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "思考过程" },
                { "type": "text", "text": "答复" }
            ],
            "stop_reason": "end_turn"
        });
        let resp = anthropic().parse_response(data);
        assert_eq!(text(&resp.content), Some("答复"));
        assert_eq!(text(&resp.reasoning_content), Some("思考过程"), "thinking block → reasoning_content");
        assert!(resp.thinking.is_none(), "无标签时 thinking 为 None");
    }

    #[test]
    fn anthropic_parse_response_falls_back_to_think_tag() {
        let data = serde_json::json!({
            "content": [{ "type": "text", "text": "<think>思考</think>答复" }],
            "stop_reason": "end_turn"
        });
        let resp = anthropic().parse_response(data);
        assert_eq!(text(&resp.content), Some("答复"));
        assert!(resp.reasoning_content.is_none(), "无 thinking block → None");
        assert_eq!(text(&resp.thinking), Some("思考"), "标签内容独立取 thinking");
    }

    #[test]
    fn anthropic_parse_response_empty_thinking_block_is_kept_as_empty_string() {
        // 空串 thinking block 按「有该 block」处理：值为空串（非 None）
        let data = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "" },
                { "type": "text", "text": "<think>思考</think>答复" }
            ],
            "stop_reason": "end_turn"
        });
        let resp = anthropic().parse_response(data);
        assert_eq!(text(&resp.content), Some("答复"), "<think> 标签应剥离");
        assert_eq!(text(&resp.reasoning_content), Some(""), "空串 thinking block 取值为空串");
        assert_eq!(text(&resp.thinking), Some("思考"), "标签内容独立取 thinking");
    }

    #[test]
    fn anthropic_parse_response_uses_first_text_and_thinking_blocks() {
        // 多 block：text 取第一个，thinking 取第一个；其余 block 忽略
        let data = serde_json::json!({
            "content": [
                { "type": "text", "text": "第一段" },
                { "type": "thinking", "thinking": "推理一" },
                { "type": "text", "text": "第二段" },
                { "type": "thinking", "thinking": "推理二" }
            ],
            "stop_reason": "end_turn"
        });
        let resp = anthropic().parse_response(data);
        assert_eq!(text(&resp.content), Some("第一段"));
        assert_eq!(text(&resp.reasoning_content), Some("推理一"));
    }

    #[test]
    fn openai_parse_response_extracts_total_tokens() {
        // DeepSeek/Kimi 非流式响应：usage.total_tokens = prompt + completion
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        });
        let resp = openai().parse_response(data);
        assert_eq!(resp.total_tokens, 15, "应取 usage.total_tokens");
    }

    #[test]
    fn openai_parse_response_missing_usage_defaults_zero() {
        // 无 usage 字段（容错）→ 0
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
        });
        let resp = openai().parse_response(data);
        assert_eq!(resp.total_tokens, 0, "缺 usage 回退 0");
    }

    #[test]
    fn anthropic_parse_response_total_tokens_always_zero() {
        let data = serde_json::json!({
            "content": [{ "type": "text", "text": "答复" }],
            "stop_reason": "end_turn"
        });
        let resp = anthropic().parse_response(data);
        assert_eq!(resp.total_tokens, 0, "anthropic 暂固定 0");
    }
}
