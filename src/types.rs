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
    FnPtr(Vec<BcType>, Box<BcType>, bool),
}

impl BcType {
    pub fn is_compatible_with(&self, expected: &Self) -> bool {
        if self == expected {
            return true;
        }
        matches!(
            (self, expected),
            (
                Self::FnPtr(actual_params, actual_ret, true),
                Self::FnPtr(expected_params, expected_ret, false)
            ) if actual_params == expected_params && actual_ret == expected_ret
        )
    }

    pub fn compatible_join(&self, other: &Self, expected: Option<&Self>) -> Option<Self> {
        if self == other {
            return Some(self.clone());
        }
        if let Some(expected) = expected {
            if self.is_compatible_with(expected) && other.is_compatible_with(expected) {
                return Some(expected.clone());
            }
        }
        if self.is_compatible_with(other) {
            return Some(other.clone());
        }
        if other.is_compatible_with(self) {
            return Some(self.clone());
        }
        None
    }

    pub fn is_arena_safe(&self) -> bool {
        matches!(
            self,
            Self::I32
                | Self::I64
                | Self::F64
                | Self::Bool
                | Self::Unit
                | Self::Handle
                | Self::FnPtr(_, _, _)
        )
    }
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
            BcType::FnPtr(params, ret, is_pure) => {
                write!(f, "{}(", if *is_pure { "fn" } else { "fn!" })?;
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
