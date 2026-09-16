use crate::{pipeline::AgentToolCaller, station::Station, types::{ToolCall}};

use async_trait::async_trait;

pub struct StationToolCaller;

#[async_trait]
impl AgentToolCaller for StationToolCaller {
    async fn call_tool(&self, tool_call: ToolCall) -> ToolCall {
        let station = Station::get();
        station.call_tool(tool_call, &[]).await
    }
}