#[allow(dead_code)]
pub enum InputColumn {
    Int(Vec<i64>),
    Float(Vec<f64>),
    Str(Vec<String>),
    Null(usize),
}

