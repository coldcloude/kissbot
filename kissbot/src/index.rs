use std::{collections::HashMap, hash::Hash};

use crate::{error::Result};

pub struct Split {
    pub index: usize,
    pub start: usize,
}

pub trait Index<T,K>
where
    T: Eq + Hash + Clone + 'static,
    K: Eq + Hash + Clone + ToString + 'static,
{
    fn insert(&mut self, key: &K, contents: impl IntoIterator<Item = Vec<T>>) -> Result<()>;

    fn remove(&mut self, key: &K);

    /// 查找文档
    /// 只做完全匹配，部分匹配由调用方自行处理
    fn find(&self, query: &[T], prefix_only: bool) -> HashMap<K,Vec<Split>>;

    fn retrieve(&self, key: &K, index: usize) -> Result<Vec<T>>;
}
