//! case 名称与路径规则。
//!
//! case 的唯一标识是相对森林的完整路径，段之间用 `/` 连接：首段是模型名表示角色 case，
//! 首段是 `[event]{event_id}` 表示事件树；隐藏角色根固定为 `[role]`。模型只看到路径的最后一段。

use arcstr::ArcStr;

/// 隐藏角色根：只用于组织角色树，不是业务 case。
pub const ROLE_ROOT_CASE_NAME_STR: &str = "[role]";
pub const ROLE_ROOT_CASE_NAME: ArcStr = arcstr::literal!("[role]");

/// 事件根标记：事件根路径为 `[event]{event_id}`。
pub const EVENT_ROOT_PREFIX: &str = "[event]";

/// 单段名称 / 事件标识的长度上限。
pub const MAX_CASE_NAME_LEN: usize = 64;

/// 完整 case 路径的长度上限。
pub const MAX_CASE_PATH_LEN: usize = 200;

pub fn is_role_root(case_name: &str) -> bool {
    case_name == ROLE_ROOT_CASE_NAME_STR
}

/// 是否是事件树里的路径（首段是 `[event]{event_id}`）。
pub fn is_event_case(case_name: &str) -> bool {
    case_name.split('/').next().is_some_and(|first| first.starts_with(EVENT_ROOT_PREFIX))
}

/// 是否是事件根（顶层、单段、`[event]{event_id}`）。
pub fn is_event_root(case_name: &str) -> bool {
    !case_name.contains('/') && is_event_case(case_name)
}

/// 是否是角色 case（排除隐藏角色根与事件树）。
pub fn is_role_case(case_name: &str) -> bool {
    !is_role_root(case_name) && !is_event_case(case_name)
}

pub fn leaf_name(case_name: &str) -> &str {
    case_name.rsplit('/').next().unwrap_or(case_name)
}

pub fn event_root_name(event_id: &str) -> ArcStr {
    ArcStr::from(format!("{}{}", EVENT_ROOT_PREFIX, event_id))
}

/// 模型给出的单段名称：只能含英文字母、数字与下划线。
pub fn valid_case_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CASE_NAME_LEN
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// 事件标识：英文字母、数字与 `-`（记忆后缀里允许 UUID 这类标识）。
pub fn valid_event_id(event_id: &str) -> bool {
    !event_id.is_empty()
        && event_id.len() <= MAX_CASE_NAME_LEN
        && event_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// 完整路径：首段可以是事件根，其余段必须是模型名；`[role]` 不是业务 case，不接受。
pub fn valid_case_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_CASE_PATH_LEN {
        return false;
    }
    let mut segments = path.split('/');
    let Some(first) = segments.next() else {
        return false;
    };
    let first_ok = match first.strip_prefix(EVENT_ROOT_PREFIX) {
        Some(event_id) => valid_event_id(event_id),
        None => valid_case_name(first),
    };
    first_ok && segments.all(valid_case_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_case_name_only_letters_digits_underscore() {
        assert!(valid_case_name("weather_chat"));
        assert!(valid_case_name("a1_B2"));
        assert!(!valid_case_name(""));
        assert!(!valid_case_name("weather-chat"));
        assert!(!valid_case_name("天气"));
        assert!(!valid_case_name("weather chat"));
        assert!(!valid_case_name("weather.chat"));
        assert!(!valid_case_name("[role]"));
        assert!(!valid_case_name(&"a".repeat(MAX_CASE_NAME_LEN + 1)));
    }

    #[test]
    fn valid_case_path_accepts_role_and_event_paths() {
        assert!(valid_case_path("weather_chat"));
        assert!(valid_case_path("weather_chat/rain"));
        assert!(valid_case_path("[event]e1"));
        assert!(valid_case_path("[event]e1/topic"));
        assert!(!valid_case_path(""));
        assert!(!valid_case_path("[role]"));
        assert!(!valid_case_path("a//b"));
        assert!(!valid_case_path("/a"));
        assert!(!valid_case_path("a/"));
        assert!(!valid_case_path("a/../b"));
        assert!(!valid_case_path("[event]/topic"));
        assert!(!valid_case_path("[event]e1/topic-bad"));
    }

    #[test]
    fn classification_and_leaf_name() {
        assert_eq!(leaf_name("weather_chat/rain"), "rain");
        assert_eq!(leaf_name("[event]e1/topic"), "topic");
        assert!(is_role_root("[role]"));
        assert!(!is_role_root("role"));
        assert!(is_event_case("[event]e1/topic"));
        assert!(is_event_root("[event]e1"));
        assert!(!is_event_root("[event]e1/topic"));
        assert!(is_role_case("weather_chat/rain"));
        assert!(!is_role_case("[event]e1"));
        assert!(!is_role_case("[role]"));
    }
}
