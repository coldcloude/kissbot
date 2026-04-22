use std::{collections::{HashMap, HashSet, LinkedList, hash_map::Entry}, hash::Hash, sync::Arc};

use parking_lot::RwLock;

use crate::{Document, Error, document::Tokenizer, error::Result};

type TokenNodeRef<T,K> = Arc<RwLock<TokenNode<T,K>>>;

struct TokenNode<T,K>
where
    T: Eq + Hash + Clone + Send + Sync + 'static,
    K: Eq + Hash + Clone + ToString + Send + Sync + 'static,
{
    sub_tree_map: Option<Arc<RwLock<HashMap<T,TokenNodeRef<T,K>>>>>,
    leaf_set: Option<Arc<RwLock<HashSet<K>>>>,
}

impl<T,K> TokenNode<T,K>
where
    T: Eq + Hash + Clone + Send + Sync + 'static,
    K: Eq + Hash + Clone + ToString + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            sub_tree_map: None,
            leaf_set: None,
        }
    }
}

pub struct Index<T,K,TKNZ>
where
    T: Eq + Hash + Clone + Send + Sync + 'static,
    K: Eq + Hash + Clone + ToString + Send + Sync + 'static,
    TKNZ: Tokenizer<T>,
{
    tree: TokenNodeRef<T,K>,
    prefix_tree: TokenNodeRef<T,K>,
    documents: Arc<RwLock<HashMap<K,Vec<Vec<T>>>>>,
    tokenizer: TKNZ,
    max_depth: usize,
}

impl<T,K,TKNZ> Index<T,K,TKNZ>
where
    T: Eq + Hash + Clone + Send + Sync + 'static,
    K: Eq + Hash + Clone + ToString + Send + Sync + 'static,
    TKNZ: Tokenizer<T>,
{
    pub fn new(tokenizer: TKNZ, max_depth: usize) -> Self {
        Self {
            tree: Arc::new(RwLock::new(TokenNode::new())),
            prefix_tree: Arc::new(RwLock::new(TokenNode::new())),
            documents: Arc::new(RwLock::new(HashMap::new())),
            tokenizer,
            max_depth,
        }
    }
    
    pub fn insert<D: Document>(&mut self, key: &K, document: &D) -> Result<()> {
        let mut documents_guard = self.documents.write();
        match documents_guard.entry(key.clone()) {
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
                            self.prefix_tree.clone()
                        } else {
                            self.tree.clone()
                        };
                        //每个token一个子树
                        while let Some(token) = valid_tokens.pop_front() {
                            let current_tree_ref = current_tree.clone();
                            let mut tree_guard = current_tree_ref.write();
                            let sub_tree_map = tree_guard.sub_tree_map.get_or_insert_with(|| Arc::new(RwLock::new(HashMap::new())));
                            let mut sub_tree_map_guard = sub_tree_map.write();
                            current_tree = sub_tree_map_guard.entry(token).or_insert_with(|| Arc::new(RwLock::new(TokenNode::new()))).clone();
                        }
                        //在叶子节点记录文档id
                        let mut tree_guard = current_tree.write();
                        let leaf_set = tree_guard.leaf_set.get_or_insert_with(|| Arc::new(RwLock::new(HashSet::new())));
                        let mut leaf_set_guard = leaf_set.write();
                        leaf_set_guard.insert(key.clone());
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
        let mut documents_guard = self.documents.write();
        let tokens_list = documents_guard.remove(key);
        if let Some(tokens_list) = tokens_list {
            //找到对应的文档，并移除其中所有token序列
            for tokens in tokens_list {
                //取所有长度不超过max_depth的子串进行索引
                for start in 0..tokens.len() {
                    let mut valid_tokens = LinkedList::new();
                    for curr in start..std::cmp::min(start + self.max_depth, tokens.len()) {
                        valid_tokens.push_back(tokens[curr].clone());
                    }
                    //前缀子串单独存
                    let mut current_tree = if start == 0 {
                        self.prefix_tree.clone()
                    } else {
                        self.tree.clone()
                    };
                    //搜索路径，记录每个token对应的子树
                    let mut tree_list: LinkedList<(T,TokenNodeRef<T,K>)> = LinkedList::new();
                    while let Some(token) = valid_tokens.pop_front() {
                        //保存路径
                        tree_list.push_back((token.clone(), current_tree.clone()));
                        //搜索下一个子树
                        let current_tree_ref = current_tree.clone();
                        let tree_guard = current_tree_ref.read();
                        if let Some(sub_tree_map) = tree_guard.sub_tree_map.as_ref() {
                            let sub_tree_map_guard = sub_tree_map.read();
                            if let Some(sub_tree) = sub_tree_map_guard.get(&token) {
                                current_tree = sub_tree.clone();
                            } else {
                                break;
                            }
                        }
                    }
                    //用于标记上一步是否已空
                    let mut empty = false;
                    //在叶子节点移除文档id
                    {
                        let mut tree_guard = current_tree.write();
                        if let Some(leaf_set) = tree_guard.leaf_set.as_mut() {
                            //是叶子节点
                            let mut leaf_set_guard = leaf_set.write();
                            leaf_set_guard.remove(&key);
                            empty = leaf_set_guard.is_empty();
                        }
                        //移除空叶子节点
                        if empty {
                            tree_guard.leaf_set = None;
                        }
                        //保险起见，也检查子树是否为空
                        empty = false;
                        if let Some(sub_tree_map) = tree_guard.sub_tree_map.as_ref() {
                            let sub_tree_map_guard = sub_tree_map.read();
                            empty = sub_tree_map_guard.is_empty();
                        }
                        //移除空子树
                        if empty {
                            tree_guard.sub_tree_map = None;
                        }
                        //标记当前节点是否为空，避免回溯时再锁一次
                        empty = tree_guard.sub_tree_map.is_none() && tree_guard.leaf_set.is_none();
                    }
                    //回溯，从父节点移除
                    while empty && let Some((token, current_tree)) = tree_list.pop_back() {
                        let mut tree_guard = current_tree.write();
                        if let Some(sub_tree_map) = tree_guard.sub_tree_map.as_mut() {
                            let mut sub_tree_map_guard = sub_tree_map.write();
                            //进入这里时已经判断过空了，直接删除
                            sub_tree_map_guard.remove(&token);
                            empty = sub_tree_map_guard.is_empty();
                        }
                        //移除空子树
                        if empty {
                            tree_guard.sub_tree_map = None;
                        }
                        //保险起见，也检查叶子是否为空
                        empty = false;
                        if let Some(leaf_set) = tree_guard.leaf_set.as_ref() {
                            let leaf_set_guard = leaf_set.read();
                            empty = leaf_set_guard.is_empty();
                        }
                        //移除空叶子节点
                        if empty {
                            tree_guard.leaf_set = None;
                        }
                        //标记当前节点是否为空，避免回溯时再锁一次
                        empty = tree_guard.sub_tree_map.is_none() && tree_guard.leaf_set.is_none();
                    }
                }
            }
        }
    }
}