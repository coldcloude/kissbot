use std::{collections::{HashMap, HashSet, LinkedList, hash_map::Entry}, hash::Hash, marker::PhantomData, ops::Range};

use crate::{Document, error::Result, index::{Index, Split}, tokenizer::Tokenizer};

pub fn split<T>(tokens: &Vec<T>) -> Vec<Vec<Range<usize>>> {
    let mut result_splits: Vec<Vec<Range<usize>>> = Vec::new();
    let n = tokens.len();
    //空文档，直接返回空结果集
    if n == 0 {
        return Vec::new();
    }
    //按分片数由小到大
    for split_num in 1..=n {
        //栈中每个元素为一种拆分，包括当前已经使用的token数，和已固定的分片
        let mut stack = LinkedList::new();
        //初始还未使用token，也无固定分片
        stack.push_back((0, Vec::new()));
        //取一个没有分完的拆分
        while let Some((current_pos, splits)) = stack.pop_back() {
            //确认已经固定的分片数
            let splits_used = splits.len();
            //只剩最后一片要拆，只能固定拆到结尾；否则就再拆分一片出来
            if splits_used == split_num - 1 {
                let mut result = Vec::with_capacity(split_num);
                let mut last_pos = 0;
                //输出所有已经固定的分片
                for &pos in &splits {
                    result.push(last_pos..pos);
                    last_pos = pos;
                }
                //最后一个分片，只能拆到结尾
                result.push(last_pos..n);
                //将拆分结果加入结果集
                result_splits.push(result);
            }
            else {
                //确认还要再拆几片（不算当前这片）
                let remaining_splits = split_num - splits_used - 1;
                //分片最少有1个token，最大要给每个剩余分片留1个token
                let min_pos = current_pos + 1;
                let max_pos = n - remaining_splits;
                //遍历所有可能的分片位置
                for split_pos in (min_pos..=max_pos).rev() {
                    let mut new_splits = splits.clone();
                    new_splits.push(split_pos);
                    stack.push_back((split_pos, new_splits));
                }
            }
        }
    }
    result_splits
}

type RawResult<K> = HashMap<K,Vec<Split>>;
type ResultIndexMap = HashMap<usize,HashMap<usize,usize>>;
type CombinedResult<K> = HashMap<K,ResultIndexMap>;
type CombinedPriorityResult<K> = HashMap<K,(usize,ResultIndexMap)>;

fn combine_raw_result<K>(result: &mut CombinedResult<K>, raw_result: &mut RawResult<K>, token_len: usize, intersect: bool)
where
    K: Eq + Hash + Clone + ToString + 'static,
{
    //交集模式，先清理不匹配的
    if intersect {
        result.retain(|key, _| raw_result.contains_key(key));
    }
    
    for (key, splits) in raw_result.drain() {
        //交集模式只取已有的
        let index_map_or_none = if intersect {
            result.get_mut(&key)
        } else {
            let index_map = result.entry(key).or_insert(HashMap::new());
            Some(index_map)
        };
        if let Some(index_map) = index_map_or_none {
            for split in splits {
                let start_map = index_map.entry(split.index).or_insert(HashMap::new());
                match start_map.entry(split.start) {
                    Entry::Occupied(mut entry) => {
                        let old_len = *entry.get();
                        *entry.get_mut() = std::cmp::max(old_len, token_len);
                    },
                    Entry::Vacant(entry) => {
                        entry.insert(token_len);
                    },
                }
            }
        }
    }
}

fn combine_priority_result<K>(priority_result: &mut CombinedPriorityResult<K>, result: &mut CombinedResult<K>, priority: usize)
where
    K: Eq + Hash + Clone + ToString + 'static,
{
    for (key, index_map) in result.drain() {
        match priority_result.entry(key) {
            Entry::Occupied(mut entry) => {
                if priority < entry.get().0 {
                    *entry.get_mut() = (priority, index_map);
                }
            },
            Entry::Vacant(entry) => {
                entry.insert((priority, index_map));
            },
        }
    }
}

pub enum SearchMode {
    Prefix,
    Full,
    Split,
}

type TokenSplitMap<T> = HashMap<Vec<T>,HashMap<Range<usize>,usize>>;

pub struct IndexSearchContext<T>
where
    T: Eq + Hash + Clone + 'static,
{
    current_split_or_none: Option<TokenSplitMap<T>>,
    split_tokens_list: Vec<TokenSplitMap<T>>,
}

impl<T> IndexSearchContext<T>
where
    T: Eq + Hash + Clone + 'static,
{
    pub fn new() -> Self {
        Self {
            current_split_or_none: None,
            split_tokens_list: Vec::new(),
        }
    }
    
    pub fn add_to_split(&mut self, tokens: &Vec<T>, range: Range<usize>) {
        let current_split = self.current_split_or_none.get_or_insert(HashMap::new());
        match current_split.get_mut(tokens) {
            Some(map) => {
                match map.entry(range) {
                    Entry::Occupied(mut entry) => {
                        *entry.get_mut() += 1;
                    },
                    Entry::Vacant(entry) => {
                        entry.insert(1);
                    },
                };
            },
            None => {
                let mut new_map = HashMap::new();
                new_map.insert(range, 1);
                current_split.insert(tokens.clone(), new_map);
            },
        }
    }

    pub fn end_split(&mut self) {
        let current_split_or_none = self.current_split_or_none.take();
        self.current_split_or_none = None;
        if let Some(current_split) = current_split_or_none {
            self.split_tokens_list.push(current_split);
        }
    }

    pub fn find<K,IDX>(&self, priority_result: &mut CombinedPriorityResult<K>, index: &IDX)
    where
        K: Eq + Hash + Clone + ToString + 'static,
        IDX: Index<T,K>,
    {
        //对tokens去重
        let mut tokens_set: HashSet<&[T]> = HashSet::new();
        for split in &self.split_tokens_list {
            for (tokens, map) in split {
                for range in map.keys() {
                    tokens_set.insert(&tokens[range.start..range.end]);
                }
            }
        }
        //匹配去重后的每组tokens
        let mut raw_result_map: HashMap<&[T],RawResult<K>> = HashMap::new();
        for tokens in tokens_set {
            let raw_result = index.find(tokens, false);
            raw_result_map.insert(tokens, raw_result);
        }
        //合并结果
        for split_tokens in &self.split_tokens_list {
            //对每一种拆分方式提取结果，要求拆分出的每个tokens都能匹配到
            let mut priority: usize = 0;
            let mut combine_result = HashMap::new();
            for (tokens, range_map) in split_tokens {
                for (range, count) in range_map {
                    priority += count;
                    match raw_result_map.get_mut(&tokens[range.start..range.end]) {
                        None => {
                            //有一个tokens没有找到，查询失败
                            combine_result.clear();
                            break;
                        },
                        Some(raw_result) => {
                            //合并结果，取交集
                            combine_raw_result(&mut combine_result, raw_result, tokens.len(), true);
                        },
                    }
                }
            }
            //保存到最终结果
            if !combine_result.is_empty() {
                combine_priority_result(priority_result, &mut combine_result, priority);
            }
        }
    }
}

pub struct IndexSearch<T,K,TKNZ,IDX>
where
    T: Eq + Hash + Clone + 'static,
    K: Eq + Hash + Clone + ToString + 'static,
    TKNZ: Tokenizer<T>,
    IDX: Index<T,K>,
{
    _marker: PhantomData<(T, K)>,
    index: IDX,
    tokenizer: TKNZ,
}

impl<T,K,TKNZ,IDX> IndexSearch<T,K,TKNZ,IDX>
where
    T: Eq + Hash + Clone + 'static,
    K: Eq + Hash + Clone + ToString + 'static,
    TKNZ: Tokenizer<T>,
    IDX: Index<T,K>,
{
    pub fn new(index: IDX, tokenizer: TKNZ) -> Self {
        Self {
            _marker: PhantomData,
            index,
            tokenizer,
        }
    }

    pub fn insert<D>(&mut self, key: &K, document: D) -> Result<()>
    where
        D: Document,
    {
        let mut tokens_list: Vec<Vec<T>> = Vec::new();
        for content in document.contents() {
            let splits = content.split_whitespace();
            let mut tokens: Vec<T> = Vec::new();
            for split in splits {
                tokens.extend(self.tokenizer.tokenize(split));
            }
            tokens_list.push(tokens);
        }
        self.index.insert(key, tokens_list.into_iter())
    }

    pub fn remove(&mut self, key: &K) {
        self.index.remove(key);
    }

    fn find_index_maps(&self, query: &str, mode: SearchMode) -> CombinedPriorityResult<K> {
        let parts = query.split_whitespace();
        let mut priority_result: CombinedPriorityResult<K> = HashMap::new();
        match mode {
            SearchMode::Prefix => {
                let mut tokens: Vec<T> = Vec::new();
                for part in parts {
                    tokens.extend(self.tokenizer.tokenize(part));
                }
                let mut raw_results = self.index.find(&tokens, true);
                let mut results = HashMap::new();
                combine_raw_result(&mut results, &mut raw_results, tokens.len(), false);
                combine_priority_result(&mut priority_result, &mut results, 0);
            },
            SearchMode::Full => {
                let mut context = IndexSearchContext::new();
                for part in parts {
                    let tokens = self.tokenizer.tokenize(part);
                    context.add_to_split(&tokens,0..tokens.len());
                }
                context.end_split();
                context.find(&mut priority_result, &self.index);
            },
            SearchMode::Split => {
                let mut tokens_list: Vec<Vec<T>> = Vec::new();
                let mut part_splits_list: Vec<Vec<Vec<Range<usize>>>> = Vec::new();
                for part in parts {
                    let tokens = self.tokenizer.tokenize(part);
                    let splits = split(&tokens);
                    tokens_list.push(tokens);
                    part_splits_list.push(splits);
                }
                let part_num = part_splits_list.len();
                let mut pis: Vec<usize> = Vec::with_capacity(part_num);
                let mut context: IndexSearchContext<T> = IndexSearchContext::new();
                pis.push(0);
                while pis.len() > 0 {
                    //前一part变更，下一part从0开始
                    while pis.len() < part_num {
                        pis.push(0);
                    }
                    //遍历所有part当前索引，添加到context，作为一个split
                    for p in 0..part_num {
                        let tokens = &tokens_list[p];
                        let splits_list = &part_splits_list[p];
                        let i = pis[p];
                        if i < splits_list.len() {
                            let splits = &splits_list[i];
                            for split in splits {
                                context.add_to_split(&tokens, split.clone());
                            }
                        }
                    }
                    context.end_split();
                    //递归去除已遍历结束的part
                    let mut p = (pis.len() - 1) as i32;
                    while p >= 0 && pis[p as usize] >= part_splits_list[p as usize].len() {
                        pis.pop();
                        p -= 1;
                        if p >= 0 {
                            pis[p as usize] += 1;
                        }
                    }
                }
                //查找所有split组合
                context.find(&mut priority_result, &self.index);
            },
        }
        priority_result
    }

    pub fn find_all_keys(&self, query: &str, split: bool) -> Vec<K> {
        //找到所有结果
        let mut priority_result = self.find_index_maps(query, if split { SearchMode::Split } else { SearchMode::Full });
        let mut result = Vec::new();
        for (key, (priority,_)) in priority_result.drain() {
            result.push((key,priority));
        }
        //按优先级排序
        result.sort_by(|(_, p1), (_, p2)| p1.cmp(&p2));
        //去掉priority字段，只按顺序返回key
        result
            .into_iter()
            .map(|(key, _)| key)
            .collect::<Vec<K>>()
    }

    pub fn find_prefix(&self, query: &str) -> Vec<String> {
        let mut priority_result = self.find_index_maps(query, SearchMode::Prefix);
        let mut result = Vec::new();
        for (key, (_,mut index_map)) in priority_result.drain() {
            for (index, _) in index_map.drain() {
                if let Ok(tokens) = self.index.retrieve(&key, index) {
                    let content = self.tokenizer.untokenize(tokens.as_slice());
                    result.push(content);
                }
            }
        }
        result
    }
}
