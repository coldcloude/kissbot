use std::{collections::{HashMap, HashSet, LinkedList, hash_map::Entry}, hash::Hash};

use crate::{Document, Error, document::Tokenizer, error::Result};

struct TokenNode<T,K>
where
    T: Eq + Hash + Clone + 'static,
    K: Eq + Hash + Clone + ToString + 'static,
{
    sub_tree_map: Option<HashMap<T,TokenNode<T,K>>>,
    leaf_set: Option<HashSet<K>>,
}

impl<T,K> TokenNode<T,K>
where
    T: Eq + Hash + Clone + 'static,
    K: Eq + Hash + Clone + ToString + 'static,
{
    pub fn new() -> Self {
        Self {
            sub_tree_map: None,
            leaf_set: None,
        }
    }

    pub fn get_sub_tree(&mut self, token: T) -> &mut Self {
        self.sub_tree_map.get_or_insert_with(|| HashMap::new()).entry(token).or_insert_with(|| Self::new())
    }

    pub fn insert_leaf(&mut self, key: K) {
        self.leaf_set.get_or_insert_with(|| HashSet::new()).insert(key);
    }

    pub fn remove(&mut self, key: &K, tokens: &mut LinkedList<T>) {
        match tokens.pop_front() {
            Some(token) => {
                //中间节点
                if let Some(sub_tree_map) = self.sub_tree_map.as_mut() {
                    if let Entry::Occupied(mut sub_tree_entry) = sub_tree_map.entry(token) {
                        //存在token对应的子节点，递归执行子节点移除
                        let sub_tree = sub_tree_entry.get_mut();
                        sub_tree.remove(key, tokens);
                        //如果子节点里没有数据了，移除子节点
                        if sub_tree.leaf_set.is_none() && sub_tree.sub_tree_map.is_none() {
                            sub_tree_entry.remove();
                        }
                    }
                    //如果没有子节点了，移除子节点表
                    if sub_tree_map.is_empty() {
                        self.sub_tree_map = None;
                    }
                }
            }
            None => {
                //叶子节点
                if let Some(leaf_set) = self.leaf_set.as_mut() {
                    //移除叶子
                    leaf_set.remove(&key);
                    //如果叶子节点为空，移除叶子表
                    if leaf_set.is_empty() {
                        self.leaf_set = None;
                    }
                }
            }
        }
    }
}
pub struct Index<T,K,TKNZ>
where
    T: Eq + Hash + Clone + 'static,
    K: Eq + Hash + Clone + ToString + 'static,
    TKNZ: Tokenizer<T>,
{
    tree: TokenNode<T,K>,
    prefix_tree: TokenNode<T,K>,
    documents: HashMap<K,Vec<Vec<T>>>,
    tokenizer: TKNZ,
    max_depth: usize,
}

impl<T,K,TKNZ> Index<T,K,TKNZ>
where
    T: Eq + Hash + Clone + 'static,
    K: Eq + Hash + Clone + ToString + 'static,
    TKNZ: Tokenizer<T>,
{
    pub fn new(tokenizer: TKNZ, max_depth: usize) -> Self {
        Self {
            tree: TokenNode::new(),
            prefix_tree: TokenNode::new(),
            documents: HashMap::new(),
            tokenizer,
            max_depth,
        }
    }
    
    pub fn insert<D: Document>(&mut self, key: &K, document: &D) -> Result<()> {
        match self.documents.entry(key.clone()) {
            Entry::Occupied(_) => {
                Err(Error::DuplicatedDocumentKey(key.to_string()))
            }
            Entry::Vacant(entry) => {
                let mut tokens_list = Vec::new();
                for content in document.contents() {
                    let tokens = self.tokenizer.tokenize(content.as_str());
                    //取所有长度不超过max_depth的子串进行索引
                    for start in 0..tokens.len() {
                        let mut valid_tokens = LinkedList::new();
                        for curr in start..std::cmp::min(start + self.max_depth, tokens.len()) {
                            valid_tokens.push_back(tokens[curr].clone());
                        }
                        //前缀子串单独存
                        let mut current_tree = if start == 0 {
                            &mut self.prefix_tree
                        } else {
                            &mut self.tree
                        };
                        //每个token一个子树
                        while let Some(token) = valid_tokens.pop_front() {
                            current_tree = current_tree.get_sub_tree(token);
                        }
                        //在叶子节点记录文档id
                        current_tree.insert_leaf(key.clone());
                    }
                    //保存文档内容用于移除
                    tokens_list.push(tokens);
                }
                entry.insert(tokens_list);
                Ok(())
            }
        }
    }

    pub fn remove(&mut self, key: &K) {
        if let Some(tokens_list) = self.documents.remove(key) {
            //找到对应的文档，并移除其中所有token序列
            for tokens in tokens_list {
                //取所有长度不超过max_depth的子串进行索引
                for start in 0..tokens.len() {
                    let mut valid_tokens = LinkedList::new();
                    for curr in start..std::cmp::min(start + self.max_depth, tokens.len()) {
                        valid_tokens.push_back(tokens[curr].clone());
                    }
                    //前缀子串单独存
                    if start == 0 {
                        self.prefix_tree.remove(key, &mut valid_tokens);
                    } else {
                        self.tree.remove(key, &mut valid_tokens);
                    };
                }
            }
        }
    }
}