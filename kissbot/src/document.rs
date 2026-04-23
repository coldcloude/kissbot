use std::collections::LinkedList;

pub trait Document {
    fn contents(&self) -> Vec<String>;
}

impl<T: ToString> Document for T {
    fn contents(&self) -> Vec<String> {
        vec![self.to_string()]
    }
}

pub trait Tokenizer<T> {
    fn tokenize(&self, document: &str) -> Vec<T>;
}

pub fn split<T>(tokens: &Vec<T>) -> Vec<Vec<&[T]>> {
    let mut results_list = Vec::new();
    let n = tokens.len();
    //空文档，直接返回空结果集
    if n == 0 {
        return results_list;
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
                    result.push(&tokens[last_pos..pos]);
                    last_pos = pos;
                }
                //最后一个分片，只能拆到结尾
                result.push(&tokens[last_pos..n]);
                //将拆分结果加入结果集
                results_list.push(result);
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
    results_list
}