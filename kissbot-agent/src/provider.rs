use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kissbot_api::AsyncCacheFactory;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::sleep;

use crate::{config_manager::ConfigManager, configs::{EffectiveLLMConfig, EffectiveModelConfig, ProviderConfig, ToolConfig}, types::{Error, Message, ModelResponse, Result, ToolCall, ToolData}};

/// Provider 抽象：负责向模型服务商发一次请求并解析响应
#[async_trait]
pub trait Provider: Send + Sync {
    // 可复用 request 类型
    type Request;
    fn build_request(&self, llm_cfg: &EffectiveLLMConfig, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Self::Request;
    async fn send_request(&self, request: &Self::Request, timeout: u64) -> Result<Value>;
    fn parse_response(&self, data: Value) -> ModelResponse;
    /// 从服务商 API 获取全部可用模型名（GET /models）
    async fn list_models(&self) -> Result<Vec<String>>;
}

/// 指数退避重试（retry_count 来自有效配置）
async fn call_with_retry<R>(
    provider: Arc<impl Provider<Request = R>>,
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

    Err(last_error.unwrap_or_else(|| Error::ModelApiError("模型调用失败".to_string())))
}

pub struct ProviderManager {
    openai_provider_factory: AsyncCacheFactory<OpenAiProvider>,
    anthropic_provider_factory: AsyncCacheFactory<AnthropicProvider>,
}

impl ProviderManager {
    pub fn new() -> Self {
        Self {
            openai_provider_factory: AsyncCacheFactory::new(),
            anthropic_provider_factory: AsyncCacheFactory::new(),
        }
    }

    async fn get_openai_provider(&self, name: &str, provider_config: &ProviderConfig) -> Arc<OpenAiProvider> {
        self.openai_provider_factory.get_or_create(name, || OpenAiProvider {
            client: Arc::new(Client::new()),
            base_url: provider_config.base_url.clone(),
            api_key: provider_config.api_key.clone(),
        }).await
    }

    async fn get_anthropic_provider(&self, name: &str, provider_config: &ProviderConfig) -> Arc<AnthropicProvider> {
        self.anthropic_provider_factory.get_or_create(name, || AnthropicProvider {
            client: Arc::new(Client::new()),
            base_url: provider_config.base_url.clone(),
            api_key: provider_config.api_key.clone(),
        }).await
    }

    pub async fn call(&self, llm_cfg: &EffectiveLLMConfig, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Result<ModelResponse> {
        let provider = llm_cfg.provider.as_str();
        let model = llm_cfg.model.as_str();
        let cm = ConfigManager::get();
        if let Some(pm_cfg) = cm.provider_model_config(provider, model).await {
            // 按 provider_type 构造 Provider 实现（"openai" | "anthropic"）
            match pm_cfg.provider_config.provider_type.as_str() {
                "openai" => {
                    let provider = self.get_openai_provider(provider, pm_cfg.provider_config.as_ref()).await;
                    call_with_retry(provider, llm_cfg, &pm_cfg, messages, tools).await
                },
                "anthropic" => {
                    let provider = self.get_anthropic_provider(provider, pm_cfg.provider_config.as_ref()).await;
                    call_with_retry(provider, llm_cfg, &pm_cfg, messages, tools).await
                },
                other => Err(Error::ModelProviderNotSupported(format!("未知 provider_type: {}", other))),
            }
        } else {
            Err(Error::ModelProviderNotFound(provider.to_string(), model.to_string()))
        }
    }

    /// 从服务商 API 获取全部模型名
    /// 返回 Err 表示 API 调用失败（网络/鉴权）
    pub async fn list_models(&self, provider_name: &str) -> Result<Vec<String>> {
        let cm = ConfigManager::get();
        if let Some(provider_config) = cm.provider_config(provider_name).await {
            // 按 provider_type 构造 Provider 实现（"openai" | "anthropic"）
            match provider_config.provider_type.as_str() {
                "openai" => {
                    let provider = self.get_openai_provider(provider_name, provider_config.as_ref()).await;
                    provider.list_models().await
                },
                "anthropic" => {
                    let provider = self.get_anthropic_provider(provider_name, provider_config.as_ref()).await;
                    provider.list_models().await
                },
                other => Err(Error::ModelProviderNotSupported(format!("未知 provider_type: {}", other))),
            }
        } else {
            Err(Error::ModelProviderNotFound(provider_name.to_string(), String::new()))
        }
    }
}

// ========== OpenAI 兼容协议（/chat/completions） ==========

struct OpenAiProvider {
    client: Arc<Client>,
    base_url: Arc<String>,
    api_key: Arc<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAiRequest {
    model: Arc<String>,
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
    reasoning_effort: Option<Arc<String>>,
}

/// 匹配 content 开头的 <think>...</think>（允许前导空白），剥离并返回 (剥离后内容, Option<思考内容>)
/// 标签不在开头或未闭合时原样返回
fn strip_think_tag(content: &str) -> (Option<Arc<String>>, Option<Arc<String>>) {
    let trimmed = content.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            let thinking = rest[..end].to_string();
            let stripped = rest[end + "</think>".len()..].to_string();
            return (Some(Arc::new(stripped)), Some(Arc::new(thinking)));
        }
    }
    (Some(Arc::new(content.to_string())), None)
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
        let finish_reason = if let Value::String(str) = choice["finish_reason"].take() { Some(Arc::new(str)) } else { None };
        // <think> 标签总剥离；thinking 独立取标签内容，空串视为 None
        let (content, thinking) = if let Some(c_str) = raw_content.as_str() { strip_think_tag(c_str) } else { (None, None) };
        let reasoning_content = if let Some(rc_str) = raw_reasoning_content.as_str() { Some(Arc::new(rc_str.to_string())) } else { None };
        // tool_calls：OpenAI function call 数组（含 thinking 模式下多轮工具调用）
        let mut tool_calls = None;
        if let Some(calls) = raw_tool_calls.as_array() {
            let mut tcs = Vec::new();
            for v in calls {
                if let Some(id) = v["id"].as_str() {
                    if let Some(name) = v["function"]["name"].as_str() {
                        tcs.push(ToolCall {
                            id: Arc::new(id.to_string()),
                            name: Arc::new(name.to_string()),
                            data: ToolData {
                                arguments: v["function"]["arguments"].clone(),
                                result: Value::Null,
                                error: Value::Null,
                            },
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
            return Err(Error::ModelApiError(format!("OpenAI API {}: {}", status, text)));
        }
        let data: Value = resp.json().await?;
        Ok(data)
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let resp = self.client.get(&url)
            .timeout(Duration::from_secs(30))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("OpenAI models API {}: {}", status, text)));
        }
        let data: serde_json::Value = resp.json().await?;
        let models: Vec<String> = data["data"].as_array()
        .map(|arr| arr.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default();
        Ok(models)
    }
}

// ========== Anthropic 协议（/v1/messages） ==========

const DEFFAULT_ANTTHROPIC_MAX_TOKENS: u32 = 65535;

pub struct AnthropicProvider {
    client: Arc<Client>,
    base_url: Arc<String>,
    api_key: Arc<String>,
}

#[async_trait]
impl Provider for AnthropicProvider {
    type Request = Value;

    fn build_request(&self, llm_cfg: &EffectiveLLMConfig, messages: Vec<Message>, _tools: &Vec<Arc<ToolConfig>>) -> Value {
        // 分离 system 消息
        let system_parts: Vec<String> = messages.iter()
            .filter_map(|m| match m {
                Message::System { content } => Some(content.to_string()),
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
        // reasoning_content：thinking block 内容（空串视为 None）
        let mut reasoning_content = None;
        let mut content = None;
        let raw_content = data["content"].take();
        if let Some(blocks) = raw_content.as_array() {
            for block in blocks {
                match block["type"].as_str() {
                    Some("thinking") if reasoning_content.is_none() => {
                        if let Some(rc) = block["thinking"].as_str() {
                            reasoning_content = Some(Arc::new(rc.to_string()));
                        }
                    }
                    Some("text") if content.is_none() => {
                        if let Some(c) = block["text"].as_str() {
                            content = Some(Arc::new(c.to_string()));
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut finish_reason = None;
        if let Some(sr) = data["stop_reason"].as_str() {
            finish_reason = Some(Arc::new(sr.to_string()));
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
            return Err(Error::ModelApiError(format!("Anthropic API {}: {}", status, text)));
        }
        let data: Value = resp.json().await?;
        Ok(data)
    }

    async fn list_models(&self) -> Result<Vec<String>> {
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
            return Err(Error::ModelApiError(format!("Anthropic models API {}: {}", status, text)));
        }
        let data: serde_json::Value = resp.json().await?;
        let models = data["data"].as_array()
        .map(|arr| arr.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default();
        Ok(models)
    }
}

// #[cfg(test)]
// mod tests {
//     use crate::configs::ProviderModel;

// use super::*;

//     fn sample_llm_cfg() -> EffectiveLLMConfig {
//         EffectiveLLMConfig {
//             model: Arc::new(ProviderModel {
//                 provider: "deepseek".to_string(),
//                 model: "deepseek-v4-flash".to_string(),
//             }),
//             max_tokens: Some(2048),
//             temperature: Some(0.3),
//             thinking: None,
//             reasoning_effort: None,
//         }
//     }

//     fn sample_model_cfg() -> EffectiveModelConfig {
//         EffectiveModelConfig {
//             provider_type: "openai".into(),
//             base_url: "https://api.deepseek.com".into(),
//             api_key: "sk-test".into(),
//             model: "deepseek-4-flash".into(),
//             max_tokens_usage: 128000,
//             timeout_secs: 30,
//             retry_count: 2,
//         }
//     }

//     // Message 构造测试助手（字段为 Arc<String>，集中构造减少噪音）
//     fn sys(content: &str) -> Message {
//         Message::System { content: Arc::new(content.into()) }
//     }

//     fn usr(content: &str) -> Message {
//         Message::User { content: Arc::new(content.into()) }
//     }

//     #[test]
//     fn openai_body_includes_params_and_messages() {
//         let llm_cfg = sample_llm_cfg();
//         let msgs = vec![sys("你是助手"), usr("你好")];
//         let body = openai_request(&llm_cfg, msgs, vec![]);
//         assert_eq!(body.model.as_str(), "deepseek-4-flash");
//         assert_eq!(body.max_tokens.unwrap(), 2048);
//         // temperature 为 f32，序列化为 f64 表示，用 f32 精确值比较
//         assert_eq!(body.temperature.unwrap(), 0.3_f32);
//         assert_eq!(body.stream, false);
//         assert!(matches!(&body.messages[0], Message::System { content } if content.as_str() == "你是助手"));
//         assert!(matches!(&body.messages[1], Message::User { content } if content.as_str() == "你好"));
//     }

//     #[test]
//     fn openai_body_omits_optional_params_when_none() {
//         let mut llm_cfg = sample_llm_cfg();
//         llm_cfg.temperature = None;
//         llm_cfg.thinking = None;
//         llm_cfg.reasoning_effort = None;
//         let msgs = vec![usr("你好")];
//         let body = openai_request(&llm_cfg, msgs, vec![]);
//         assert!(body.get("temperature").is_none(), "temperature 未配置不应传");
//         assert!(body.get("thinking").is_none(), "thinking 未配置不应传");
//         assert!(body.get("reasoning_effort").is_none(), "reasoning_effort 未配置不应传");
//         assert_eq!(body["model"], "deepseek-4-flash");
//         assert_eq!(body["stream"], false);
//     }

//     #[test]
//     fn openai_body_passes_thinking_and_reasoning_effort() {
//         let mut llm_cfg = sample_llm_cfg();
//         llm_cfg.thinking = Some(Arc::new("enabled".to_string()));
//         llm_cfg.reasoning_effort = Some(Arc::new("high".to_string()));
//         let msgs = vec![usr("你好")];
//         let body = openai_request(&llm_cfg, msgs, vec![]);
//         assert_eq!(body["thinking"]["type"], "enabled");
//         assert_eq!(body["reasoning_effort"], "high");
//         assert_eq!(body["temperature"], 0.3_f32 as f64);
//     }

//     #[test]
//     fn openai_body_includes_tools_when_present() {
//         let llm_cfg = sample_llm_cfg();
//         let msgs = vec![usr("查一下")];
//         let tools = vec![ToolConfig {
//             name: Arc::new("read".into()),
//             description: Arc::new("读取文本文件".into()),
//             parameters: json!({ "type": "object" }),
//         }];
//         let body = openai_request(&llm_cfg, msgs, tools);
//         assert_eq!(body["tools"][0]["function"]["name"], "read");
//         assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
//     }

//     #[test]
//     fn openai_body_omits_tools_when_empty() {
//         let llm_cfg = sample_llm_cfg();
//         let msgs = vec![usr("你好")];
//         let body = openai_request(&llm_cfg, &msgs, &[]);
//         assert!(body.get("tools").is_none(), "无工具不应发送 tools 字段");
//     }

//     #[test]
//     fn openai_body_maps_tool_and_assistant_tool_calls() {
//         let llm_cfg = sample_llm_cfg();
//         let msgs = vec![
//             Message::Assistant {
//                 content: Arc::new(String::new()),
//                 reasoning_content: None,
//                 tool_calls: Some(vec![Arc::new(ToolCall { id: Arc::new("c1".into()), name: Arc::new("read".into()), arguments: Arc::new(serde_json::json!({"path": "/a"})) })]),
//             },
//             Message::Tool { tool_call_id: Arc::new("c1".into()), name: Arc::new("read".into()), content: Arc::new("内容".into()) },
//         ];
//         let body = openai_request(&llm_cfg, &msgs, &[]);
//         assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "c1");
//         assert_eq!(body["messages"][0]["tool_calls"][0]["function"]["name"], "read");
//         assert_eq!(body["messages"][0]["tool_calls"][0]["function"]["arguments"], r#"{"path":"/a"}"#, "arguments 序列化为 JSON 字符串");
//         assert_eq!(body["messages"][1]["role"], "tool");
//         assert_eq!(body["messages"][1]["tool_call_id"], "c1");
//         // 输入 assistant 的 reasoning_content 为 None（上下文保留与否由 coordinator 策略决定），
//         // 此处由 skip_serializing_if 省略；若 Some（工具调用场景须回传）则自动序列化携带
//         assert!(body["messages"][0].get("reasoning_content").is_none());
//     }

//     #[test]
//     fn openai_body_serializes_reasoning_content_when_present() {
//         // 格式能力：Message 序列化即 OpenAI 格式，reasoning_content 由格式自动序列化携带；
//         // 上下文已保留 model_resp.reasoning_content（工具调用场景须回传，见 coordinator 步骤 4/6），wire 直接携带
//         let llm_cfg = sample_llm_cfg();
//         let msgs = vec![
//             Message::System { content: Arc::new("设定".into()) },
//             Message::Assistant { content: Arc::new("回答".into()), reasoning_content: Some(Arc::new("思考".into())), tool_calls: None },
//         ];
//         let body = openai_request(&llm_cfg, &msgs, &[]);
//         assert_eq!(body["messages"].as_array().unwrap().len(), 2);
//         assert_eq!(body["messages"][1]["reasoning_content"], "思考", "格式自动序列化 reasoning_content");
//         assert_eq!(body["messages"][1]["role"], "assistant");
//         assert_eq!(body["messages"][1]["content"], "回答");
//     }

//     #[test]
//     fn parse_openai_response_extracts_content_and_finish_reason() {
//         let data = serde_json::json!({
//             "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
//         });
//         let resp = parse_openai_response(data);
//         assert_eq!(resp.content.as_str(), "答案");
//         assert_eq!(resp.finish_reason.as_str(), "stop");
//     }

//     #[test]
//     fn parse_openai_response_extracts_tool_calls() {
//         let data = serde_json::json!({
//             "choices": [{
//                 "message": { "content": null, "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "read", "arguments": "{\"path\":\"/a\"}" } }] },
//                 "finish_reason": "tool_calls"
//             }]
//         });
//         let resp = parse_openai_response(&data);
//         assert_eq!(resp.tool_calls.len(), 1);
//         assert_eq!(resp.tool_calls[0].id.as_str(), "c1");
//         assert_eq!(resp.tool_calls[0].name.as_str(), "read");
//         assert_eq!(resp.tool_calls[0].arguments["path"], "/a", "arguments 解析为 JSON 对象");
//         assert_eq!(resp.finish_reason.as_str(), "tool_calls");
//     }

//     #[test]
//     fn parse_openai_response_no_tool_calls_by_default() {
//         let data = serde_json::json!({
//             "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
//         });
//         let resp = parse_openai_response(&data);
//         assert!(resp.tool_calls.is_empty(), "无 tool_calls 字段时为空");
//     }

//     #[test]
//     fn anthropic_body_separates_system_messages() {
//         let llm_cfg = sample_llm_cfg();
//         let msgs = vec![sys("设定"), usr("hi")];
//         let body = anthropic_body(&llm_cfg, &msgs, &[]);
//         assert_eq!(body["system"], "设定");
//         assert_eq!(body["messages"].as_array().unwrap().len(), 1, "system 不应出现在 messages");
//         assert_eq!(body["messages"][0]["role"], "user");
//         assert_eq!(body["max_tokens"], 2048);
//     }

//     #[test]
//     fn anthropic_body_omits_optional_params_when_none() {
//         let mut llm_cfg = sample_llm_cfg();
//         llm_cfg.temperature = None;
//         llm_cfg.thinking = None;
//         llm_cfg.reasoning_effort = None;
//         let msgs = vec![usr("hi")];
//         let body = anthropic_body(&llm_cfg, &msgs, &[]);
//         assert!(body.get("temperature").is_none(), "temperature 未配置不应传");
//         assert!(body.get("thinking").is_none(), "thinking 未配置不应传");
//         assert!(body.get("output_config").is_none(), "reasoning_effort 未配置不应传 output_config");
//     }

//     #[test]
//     fn anthropic_body_passes_thinking_and_output_config() {
//         let mut llm_cfg = sample_llm_cfg();
//         llm_cfg.thinking = Some(Arc::new("enabled".to_string()));
//         llm_cfg.reasoning_effort = Some(Arc::new("high".to_string()));
//         let msgs = vec![usr("hi")];
//         let body = anthropic_body(&llm_cfg, &msgs, &[]);
//         assert_eq!(body["thinking"]["type"], "enabled");
//         assert_eq!(body["output_config"]["effort"], "high");
//         assert_eq!(body["temperature"], 0.3_f32 as f64);
//     }

//     #[test]
//     fn parse_anthropic_response_extracts_text_and_stop_reason() {
//         let data = serde_json::json!({
//             "content": [{ "type": "text", "text": "答复" }],
//             "stop_reason": "end_turn"
//         });
//         let resp = parse_anthropic_response(&data);
//         assert_eq!(resp.content.as_str(), "答复");
//         assert_eq!(resp.finish_reason.as_str(), "end_turn");
//     }


//     #[test]
//     fn strip_think_tag_extracts_and_removes_leading_tag() {
//         assert_eq!(strip_think_tag("<think>让我想想</think>答案".to_string()), ("答案".to_string(), "让我想想".to_string()));
//     }

//     #[test]
//     fn strip_think_tag_keeps_non_leading_tag() {
//         let content = "答案<think>思考</think>".to_string();
//         assert_eq!(strip_think_tag(content.clone()), (content, String::new()));
//     }

//     #[test]
//     fn strip_think_tag_allows_leading_whitespace() {
//         assert_eq!(strip_think_tag("\n<think>思考</think>答案".to_string()), ("答案".to_string(), "思考".to_string()));
//     }

//     #[test]
//     fn strip_think_tag_returns_unchanged_when_no_tag() {
//         assert_eq!(strip_think_tag("普通文本".to_string()), ("普通文本".to_string(), "".to_string()));
//         assert_eq!(strip_think_tag("".to_string()), ("".to_string(), "".to_string()));
//     }

//     #[test]
//     fn strip_think_tag_keeps_unclosed_tag() {
//         let content = "<think>未闭合".to_string();
//         assert_eq!(strip_think_tag(content.to_string()), (content.to_string(), String::new()));
//     }

//     #[test]
//     fn parse_openai_response_reasoning_and_thinking_independent() {
//         // API 有 reasoning_content + content 有 <think> 标签 -> 两字段都 Some（独立共存）
//         let data = serde_json::json!({
//             "choices": [{ "message": { "content": "<think>标签思考</think>答案", "reasoning_content": "API推理" }, "finish_reason": "stop" }]
//         });
//         let resp = parse_openai_response(&data);
//         assert_eq!(resp.content.as_str(), "答案", "<think> 标签应剥离");
//         assert_eq!(resp.reasoning_content.as_str(), "API推理", "reasoning_content 独立取 API 字段");
//         assert_eq!(resp.thinking.as_str(), "标签思考", "thinking 独立取标签内容");
//     }

//     #[test]
//     fn parse_openai_response_only_thinking_when_no_api_field() {
//         // 无 API reasoning_content + <think> 标签 -> reasoning_content=None, thinking=Some
//         let data = serde_json::json!({
//             "choices": [{ "message": { "content": "<think>思考</think>答案" }, "finish_reason": "stop" }]
//         });
//         let resp = parse_openai_response(&data);
//         assert_eq!(resp.reasoning_content.as_str(), "");
//         assert_eq!(resp.thinking.as_str(), "思考");
//     }

//     #[test]
//     fn parse_openai_response_extracts_reasoning_content() {
//         let data = serde_json::json!({
//             "choices": [{ "message": { "content": "答案", "reasoning_content": "思考" }, "finish_reason": "stop" }]
//         });
//         let resp = parse_openai_response(&data);
//         assert_eq!(resp.content.as_str(), "答案");
//         assert_eq!(resp.reasoning_content.as_str(), "思考");
//         assert_eq!(resp.thinking.as_str(), "", "仅 API 字段无标签时 thinking 应为 None");
//     }

//     #[test]
//     fn parse_openai_response_falls_back_to_think_tag() {
//         let data = serde_json::json!({
//             "choices": [{ "message": { "content": "<think>思考</think>答案" }, "finish_reason": "stop" }]
//         });
//         let resp = parse_openai_response(&data);
//         assert_eq!(resp.content.as_str(), "答案", "<think> 标签应剥离");
//         assert_eq!(resp.reasoning_content.as_str(), "", "标签内容不再合并到 reasoning_content");
//         assert_eq!(resp.thinking.as_str(), "思考", "标签内容独立取 thinking");
//     }

//     #[test]
//     fn parse_openai_response_empty_api_reasoning_falls_back_to_think_tag() {
//         let data = serde_json::json!({
//             "choices": [{ "message": { "content": "<think>思考</think>答案", "reasoning_content": "" }, "finish_reason": "stop" }]
//         });
//         let resp = parse_openai_response(&data);
//         assert_eq!(resp.content.as_str(), "答案", "<think> 标签应剥离");
//         assert_eq!(resp.reasoning_content.as_str(), "", "空字符串 reasoning_content 应视为 None");
//         assert_eq!(resp.thinking.as_str(), "思考", "标签内容独立取 thinking");
//     }

//     #[test]
//     fn parse_response_no_thinking_when_both_empty() {
//         let data = serde_json::json!({
//             "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
//         });
//         let resp = parse_openai_response(&data);
//         assert_eq!(resp.reasoning_content.as_str(), "");
//         assert_eq!(resp.thinking.as_str(), "");
//     }

//     #[test]
//     fn parse_anthropic_response_reasoning_and_thinking_independent() {
//         let data = serde_json::json!({
//             "content": [
//                 { "type": "thinking", "thinking": "API推理" },
//                 { "type": "text", "text": "<think>标签思考</think>答复" }
//             ],
//             "stop_reason": "end_turn"
//         });
//         let resp = parse_anthropic_response(&data);
//         assert_eq!(resp.content.as_str(), "答复");
//         assert_eq!(resp.reasoning_content.as_str(), "API推理");
//         assert_eq!(resp.thinking.as_str(), "标签思考");
//     }

//     #[test]
//     fn parse_anthropic_response_extracts_thinking_block() {
//         let data = serde_json::json!({
//             "content": [
//                 { "type": "thinking", "thinking": "思考过程" },
//                 { "type": "text", "text": "答复" }
//             ],
//             "stop_reason": "end_turn"
//         });
//         let resp = parse_anthropic_response(&data);
//         assert_eq!(resp.content.as_str(), "答复");
//         assert_eq!(resp.reasoning_content.as_str(), "思考过程");
//         assert_eq!(resp.thinking.as_str(), "", "无标签时 thinking 应为 None");
//     }

//     #[test]
//     fn parse_anthropic_response_falls_back_to_think_tag() {
//         let data = serde_json::json!({
//             "content": [{ "type": "text", "text": "<think>思考</think>答复" }],
//             "stop_reason": "end_turn"
//         });
//         let resp = parse_anthropic_response(&data);
//         assert_eq!(resp.content.as_str(), "答复");
//         assert_eq!(resp.reasoning_content.as_str(), "", "标签内容不再合并到 reasoning_content");
//         assert_eq!(resp.thinking.as_str(), "思考", "标签内容独立取 thinking");
//     }

//     #[test]
//     fn parse_anthropic_response_empty_thinking_block_falls_back_to_think_tag() {
//         let data = serde_json::json!({
//             "content": [
//                 { "type": "thinking", "thinking": "" },
//                 { "type": "text", "text": "<think>思考</think>答复" }
//             ],
//             "stop_reason": "end_turn"
//         });
//         let resp = parse_anthropic_response(&data);
//         assert_eq!(resp.content.as_str(), "答复", "<think> 标签应剥离");
//         assert_eq!(resp.reasoning_content.as_str(), "", "空字符串 thinking 块应视为 None");
//         assert_eq!(resp.thinking.as_str(), "思考", "标签内容独立取 thinking");
//     }

//     #[test]
//     fn provider_for_unknown_type_returns_err() {
//         let client = Arc::new(reqwest::Client::new());
//         let err = provider_for(client, "typo", "u", "k").err().expect("未知 provider_type 应返回 Err");
//         assert!(matches!(err, Error::ModelProviderNotSupported(_)), "未知类型应返回 ModelProviderNotSupported");
//         assert!(err.to_string().contains("未知 provider_type: typo"), "错误信息应指明未知类型");
//     }

//     #[test]
//     fn parse_openai_response_extracts_total_tokens() {
//         // DeepSeek/Kimi 非流式响应：usage.total_tokens = prompt + completion
//         let data = serde_json::json!({
//             "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }],
//             "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
//         });
//         let resp = parse_openai_response(&data);
//         assert_eq!(resp.total_tokens, 15, "应取 usage.total_tokens");
//     }

//     #[test]
//     fn parse_openai_response_missing_usage_defaults_zero() {
//         // 无 usage 字段（容错）→ 0
//         let data = serde_json::json!({
//             "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
//         });
//         let resp = parse_openai_response(&data);
//         assert_eq!(resp.total_tokens, 0, "缺 usage 回退 0");
//     }

//     #[test]
//     fn parse_anthropic_response_total_tokens_always_zero() {
//         let data = serde_json::json!({
//             "content": [{ "type": "text", "text": "答复" }],
//             "stop_reason": "end_turn"
//         });
//         let resp = parse_anthropic_response(&data);
//         assert_eq!(resp.total_tokens, 0, "anthropic 暂固定 0");
//     }
// }
