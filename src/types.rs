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
    Handle,
    Map,
    MapStrI32,
    MapStrI64,
    MapStrF64,
    MapI32Str,
    MapI32I32,
    Struct(String),
    Enum(String),
    Array(Box<BcType>),
    FixedArray(Box<BcType>, i64),
    Result(Box<BcType>, Box<BcType>),
    FnPtr(Vec<BcType>, Box<BcType>),
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
            BcType::Handle => write!(f, "handle"),
            BcType::Map => write!(f, "map"),
            BcType::MapStrI32 => write!(f, "map_str_i32"),
            BcType::MapStrI64 => write!(f, "map_str_i64"),
            BcType::MapStrF64 => write!(f, "map_str_f64"),
            BcType::MapI32Str => write!(f, "map_i32_str"),
            BcType::MapI32I32 => write!(f, "map_i32_i32"),
            BcType::Struct(name) => write!(f, "{name}"),
            BcType::Enum(name) => write!(f, "{name}"),
            BcType::Array(elem) => write!(f, "[{elem}]"),
            BcType::FixedArray(elem, size) => write!(f, "[{elem}; {size}]"),
            BcType::Result(ok, err) => write!(f, "Result<{ok}, {err}>"),
            BcType::FnPtr(params, ret) => {
                write!(f, "fn(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {ret}")
            }
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
