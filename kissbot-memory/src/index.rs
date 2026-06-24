use dashmap::{DashMap, DashSet};
use kissbot_api::{QueryChannelRequest, QueryRequest};
use serde::{Deserialize, Serialize};
use std::collections::btree_map::Entry;
use std::hash::Hash;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use tokio::sync::{RwLock, RwLockWriteGuard};
use std::collections::{BTreeMap, LinkedList};

use crate::data::{ChannelParser, ChannelRecord, ChannelRecordKey, ChannelRecordResult, FileKey, FilePathGenerator, QueryParser, Record, RecordCombiner, RecordKey, ThinkParser, ThinkRecord, ThinkRecordResult, ToolCallParser, ToolCallRecord, ToolCallRecordResult, ToolResultParser, ToolResultRecord, ToolResultRecordResult, ensure_file_path};
use crate::error::Result;
use kai_file::ReverseLineReader;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePosition {
    pub start_pos: u64,
    pub end_pos: u64,
}

type FileIndexLock = Arc<RwLock<BTreeMap<String, FilePosition>>>;

pub(crate) struct FileIndexContext<Q,K,R,RR,P>
where
    K: Eq + Hash + Clone + FileKey + Send + Sync,
    R: Record,
    RR: Serialize,
    P: FilePathGenerator<K> + QueryParser<Q,K>,
{
    _marker: PhantomData<(Q,R,RR)>,
    pub(crate) position_map_map: DashMap<K, FileIndexLock>,
    pub(crate) obsolete_set: DashSet<K>,
    pub(crate) all_obsolete_set: DashSet<K>,
    parser: P,
}

impl<Q,K,R,RR,P> FileIndexContext<Q,K,R,RR,P>
where
    K: Eq + Hash + Clone + FileKey + Send + Sync,
    R: Record,
    RR: Serialize,
    P: FilePathGenerator<K> + QueryParser<Q,K> + RecordCombiner<K,R,RR>,
{
    pub fn new(parser: P) -> Self {
        Self {
            _marker: PhantomData,
            position_map_map: DashMap::new(),
            obsolete_set: DashSet::new(),
            all_obsolete_set: DashSet::new(),
            parser,
        }
    }

    fn get_lock(&self, key: &K) -> FileIndexLock {
        self.position_map_map.entry(key.clone()).or_insert_with(|| Arc::new(RwLock::new(BTreeMap::new()))).clone()
    }

    async fn update(mut guard: RwLockWriteGuard<'_, BTreeMap<String, FilePosition>>, file_path: impl AsRef<Path>) -> Result<()> {
        let last_key = if let Some((key, _)) = guard.last_key_value() {
            key.clone()
        } else {
            String::from("2000-01-01 00:00:00")
        };
        let mut reader = ReverseLineReader::new(file_path, None, None).await?;
        while let Some(line_with_pos) = reader.next_line().await? {
            let record = serde_json::from_str::<R>(line_with_pos.line.as_str())?;
            if record.time().as_str() < last_key.as_str() {
                break;
            }
            match guard.entry(record.time().to_string()) {
                Entry::Occupied(mut entry) => {
                    let min_start_pos = entry.get().start_pos.min(line_with_pos.start_pos);
                    let max_end_pos = entry.get().end_pos.max(line_with_pos.end_pos);
                    entry.get_mut().start_pos = min_start_pos;
                    entry.get_mut().end_pos = max_end_pos;
                }
                Entry::Vacant(entry) => {
                    entry.insert(FilePosition {
                        start_pos: line_with_pos.start_pos,
                        end_pos: line_with_pos.end_pos,
                    });
                }
            }
        }
        Ok(())
    }

    async fn update_index(&self, key: &K) -> Result<()> {
        let position_map = self.position_map_map.entry(key.clone()).or_insert_with(|| Arc::new(RwLock::new(BTreeMap::new())));
        let guard = position_map.write().await;
        let file_path = ensure_file_path(key, &self.parser).await?;
        Self::update(guard, file_path).await
    }

    async fn update_all_index(&self, key: &K) -> Result<()> {
        let position_map = self.position_map_map.entry(key.clone()).or_insert_with(|| Arc::new(RwLock::new(BTreeMap::new())));
        let mut guard = position_map.write().await;
        guard.clear();
        let file_path = ensure_file_path(key, &self.parser).await?;
        Self::update(guard, file_path).await
    }

    async fn query_reverse(&self, key: &K, start: &str, end: &str) -> Result<LinkedList<RR>> {
        if self.obsolete_set.remove(key).is_some() {
            if self.all_obsolete_set.contains(key) {
                if let Err(e) = self.update_all_index(key).await {
                    self.all_obsolete_set.insert(key.clone());
                    self.obsolete_set.insert(key.clone());
                    return Err(e);
                }
            } else {
                if let Err(e) = self.update_index(key).await {
                    self.obsolete_set.insert(key.clone());
                    return Err(e);
                }
            }
        }
        let mut results = LinkedList::new();
        let position_map = self.get_lock(key);
        let guard = position_map.read().await;
        let mut start_pos: Option<u64> = None;
        let mut end_pos: Option<u64> = None;
        if let Some((_, position)) = guard.range(start.to_string()..=end.to_string()).next() {
            start_pos = Some(position.start_pos);
        }
        if let Some((_, position)) = guard.range(start.to_string()..=end.to_string()).next_back() {
            end_pos = Some(position.end_pos);
        }
        if start_pos.is_some() && end_pos.is_some() {
            let file_path = ensure_file_path(key, &self.parser).await?;
            let mut reader = ReverseLineReader::new(file_path, start_pos, end_pos).await?;
            while let Some(line_with_pos) = reader.next_line().await? {
                let record = serde_json::from_str::<R>(line_with_pos.line.as_str())?;
                let record = self.parser.combine_record(key, &record);
                results.push_front(record);
            }
        }
        Ok(results)
    }

    pub async fn query_all(&self, query: Q) -> Result<Vec<RR>> {
        let key_with_range = self.parser.parse_query(query);
        let mut results_list = LinkedList::new();
        for (key, (start, end)) in key_with_range {
            let results = self.query_reverse(&key, start.as_str(), end.as_str()).await?;
            results_list.push_back(results);
        }
        //先计算总数
        let mut len = 0;
        for results in results_list.iter() {
            len += results.len();
        }
        //按总长建结果数组
        let mut results = Vec::with_capacity(len);
        while let Some(mut records) = results_list.pop_front() {
            while let Some(record) = records.pop_front() {
                results.push(record);
            }
        }
        Ok(results)
    }
}

pub struct MemoryIndexer {
    pub(crate) channel_indices: FileIndexContext<QueryChannelRequest, ChannelRecordKey, ChannelRecord, ChannelRecordResult, ChannelParser>,
    pub(crate) think_indices: FileIndexContext<QueryRequest, RecordKey, ThinkRecord, ThinkRecordResult, ThinkParser>,
    pub(crate) tool_call_indices: FileIndexContext<QueryRequest, RecordKey, ToolCallRecord, ToolCallRecordResult, ToolCallParser>,
    pub(crate) tool_result_indices: FileIndexContext<QueryRequest, RecordKey, ToolResultRecord, ToolResultRecordResult, ToolResultParser>,
}

static MEMORY_INDEXER: OnceLock<MemoryIndexer> = OnceLock::new();

impl MemoryIndexer {
    pub fn new() -> Self {
        Self {
            channel_indices: FileIndexContext::new(ChannelParser {}),
            think_indices: FileIndexContext::new(ThinkParser {}),
            tool_call_indices: FileIndexContext::new(ToolCallParser {}),
            tool_result_indices: FileIndexContext::new(ToolResultParser {}),
        }
    }

    pub fn get() -> &'static Self {
        MEMORY_INDEXER.get_or_init(|| MemoryIndexer::new())
    }

    pub fn mark_channel_obsolete(&self, key: &ChannelRecordKey) {
        self.channel_indices.obsolete_set.insert(key.clone());
    }

    pub fn mark_channel_all_obsolete(&self, key: &ChannelRecordKey) {
        self.channel_indices.all_obsolete_set.insert(key.clone());
    }

    pub fn mark_think_obsolete(&self, key: &RecordKey) {
        self.think_indices.obsolete_set.insert(key.clone());
    }

    pub fn mark_think_all_obsolete(&self, key: &RecordKey) {
        self.think_indices.all_obsolete_set.insert(key.clone());
    }

    pub fn mark_tool_call_obsolete(&self, key: &RecordKey) {
        self.tool_call_indices.obsolete_set.insert(key.clone());
    }

    pub fn mark_tool_call_all_obsolete(&self, key: &RecordKey) {
        self.tool_call_indices.all_obsolete_set.insert(key.clone());
    }

    pub fn mark_tool_result_obsolete(&self, key: &RecordKey) {
        self.tool_result_indices.obsolete_set.insert(key.clone());
    }

    pub fn mark_tool_result_all_obsolete(&self, key: &RecordKey) {
        self.tool_result_indices.all_obsolete_set.insert(key.clone());
    }

    pub async fn query_channel_records(&self, query: QueryChannelRequest) -> Result<Vec<ChannelRecordResult>> {
        self.channel_indices.query_all(query).await
    }

    pub async fn query_think_records(&self, query: QueryRequest) -> Result<Vec<ThinkRecordResult>> {
        self.think_indices.query_all(query).await
    }

    pub async fn query_tool_call_records(&self, query: QueryRequest) -> Result<Vec<ToolCallRecordResult>> {
        self.tool_call_indices.query_all(query).await
    }

    pub async fn query_tool_result_records(&self, query: QueryRequest) -> Result<Vec<ToolResultRecordResult>> {
        self.tool_result_indices.query_all(query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== FilePosition serde ==========

    #[test]
    fn test_serde_file_position() {
        let obj = FilePosition { start_pos: 100, end_pos: 200 };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: FilePosition = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.start_pos, 100);
        assert_eq!(deserialized.end_pos, 200);
    }

    // ========== MemoryIndexer mark methods ==========

    fn make_channel_key() -> ChannelRecordKey {
        ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        }
    }

    fn make_record_key() -> RecordKey {
        RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        }
    }

    #[test]
    fn test_mark_channel_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_channel_key();
        assert!(!indexer.channel_indices.obsolete_set.contains(&key));
        indexer.mark_channel_obsolete(&key);
        assert!(indexer.channel_indices.obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_channel_all_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_channel_key();
        assert!(!indexer.channel_indices.all_obsolete_set.contains(&key));
        indexer.mark_channel_all_obsolete(&key);
        assert!(indexer.channel_indices.all_obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_think_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_think_obsolete(&key);
        assert!(indexer.think_indices.obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_think_all_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_think_all_obsolete(&key);
        assert!(indexer.think_indices.all_obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_tool_call_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_tool_call_obsolete(&key);
        assert!(indexer.tool_call_indices.obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_tool_call_all_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_tool_call_all_obsolete(&key);
        assert!(indexer.tool_call_indices.all_obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_tool_result_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_tool_result_obsolete(&key);
        assert!(indexer.tool_result_indices.obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_tool_result_all_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_tool_result_all_obsolete(&key);
        assert!(indexer.tool_result_indices.all_obsolete_set.contains(&key));
    }

    // ========== FileIndexContext ==========

    #[test]
    fn test_file_index_context_new() {
        let ctx: FileIndexContext<QueryChannelRequest, ChannelRecordKey, ChannelRecord, ChannelRecordResult, ChannelParser> = FileIndexContext::new(ChannelParser {});
        assert!(ctx.position_map_map.is_empty());
        assert!(ctx.obsolete_set.is_empty());
        assert!(ctx.all_obsolete_set.is_empty());
    }

    #[test]
    fn test_file_index_context_get_lock() {
        let ctx: FileIndexContext<QueryChannelRequest, ChannelRecordKey, ChannelRecord, ChannelRecordResult, ChannelParser> = FileIndexContext::new(ChannelParser {});
        let key = make_channel_key();
        let lock = ctx.get_lock(&key);
        assert_eq!(ctx.position_map_map.len(), 1);
        // verify the lock is usable (non-async test)
        let _guard = lock.try_read();
        let _wguard = lock.try_write();
    }
}