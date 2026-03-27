use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BcType {
    I32,
    I64,
    F64,
    Bool,
    Str,
    Unit,
    Struct(String),
    Enum(String),
    Array(Box<BcType>),
    FixedArray(Box<BcType>, i64),
    Result(Box<BcType>, Box<BcType>),
}

impl fmt::Display for BcType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BcType::I32 => write!(f, "i32"),
            BcType::I64 => write!(f, "i64"),
            BcType::F64 => write!(f, "f64"),
            BcType::Bool => write!(f, "bool"),
            BcType::Str => write!(f, "str"),
            BcType::Unit => write!(f, "unit"),
            BcType::Struct(name) => write!(f, "{name}"),
            BcType::Enum(name) => write!(f, "{name}"),
            BcType::Array(elem) => write!(f, "[{elem}]"),
            BcType::FixedArray(elem, size) => write!(f, "[{elem}; {size}]"),
            BcType::Result(ok, err) => write!(f, "Result<{ok}, {err}>"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StructInfo {
    pub fields: Vec<(String, BcType)>,
}

#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub variants: Vec<(String, Vec<BcType>)>,
}

#[derive(Debug, Clone)]
pub struct FunctionInfo {
    pub params: Vec<(String, BcType)>,
    pub return_type: BcType,
    pub is_pure: bool,
    pub is_extern: bool,
}

#[derive(Debug, Clone)]
pub struct ConstInfo {
    pub ty: BcType,
}

#[derive(Debug, Clone)]
pub struct SemanticInfo {
    pub structs: HashMap<String, StructInfo>,
    pub enums: HashMap<String, EnumInfo>,
    pub functions: HashMap<String, FunctionInfo>,
    pub constants: HashMap<String, ConstInfo>,
}
