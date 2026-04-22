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
