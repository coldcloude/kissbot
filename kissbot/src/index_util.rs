trait SearchIndex {
    
    pub fn insert(&mut self, key: &K, contents: impl IntoIterator<Item = Vec<T>>) -> Result<()>;

    pub fn remove(&mut self, key: &K);

    /// 查找文档
    /// 只做完全匹配，部分匹配由调用方自行处理
    pub fn find(&self, query: &[T], prefix_only: bool) -> HashMap<K,Vec<Split>>;
}