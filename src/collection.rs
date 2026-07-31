// @builtin category="Array" name="array_clone" sig="fn! array_clone(array: [T]) -> [T]" desc="Allocate a shallow clone, preserving fixed or dynamic shape"
// @builtin category="Array" name="array_repeat" sig="fn! array_repeat(value: T, count: i32) -> [T]" desc="Allocate a dynamic array with count shallow copies"
// @builtin category="Array" name="array_reverse" sig="fn! array_reverse(array: [T])" desc="Reverse a mutable fixed or dynamic array in place"
// @builtin category="Array" name="array_fill" sig="fn! array_fill(array: [T], value: T)" desc="Fill a mutable fixed or dynamic array in place"
// @builtin category="Array" name="array_swap" sig="fn! array_swap(array: [T], left: i32, right: i32)" desc="Swap two checked indices in a mutable array"
// @builtin category="Array" name="array_clear" sig="fn! array_clear(array: [T])" desc="Clear a mutable dynamic array"
// @builtin category="Array" name="array_extend" sig="fn! array_extend(destination: [T], source: [T])" desc="Append an array to a mutable dynamic destination"
// @builtin category="Array" name="array_insert" sig="fn! array_insert(array: [T], index: i32, value: T)" desc="Insert a value at a checked index in a mutable dynamic array"
// @builtin category="Array" name="array_remove_at" sig="fn! array_remove_at(array: [T], index: i32) -> T" desc="Remove and return a value from a mutable dynamic array"
// @builtin category="Array" name="array_slice" sig="fn! array_slice(array: [T], start: i32, end: i32) -> [T]" desc="Allocate a dynamic half-open slice [start, end)"
// @builtin-family category="Array" name="array_contains_{type}" sig="fn array_contains_{type}(array: [{type}], value: {type}) -> bool" desc="Test whether a {type} array contains a value" types="bool,i32,i64,f64,str"
// @builtin-family category="Array" name="array_index_of_{type}" sig="fn array_index_of_{type}(array: [{type}], value: {type}) -> i32" desc="Find the first matching {type} value, or -1" types="bool,i32,i64,f64,str"
// @builtin-family category="Array" name="array_last_index_of_{type}" sig="fn array_last_index_of_{type}(array: [{type}], value: {type}) -> i32" desc="Find the last matching {type} value, or -1" types="bool,i32,i64,f64,str"
// @builtin-family category="Array" name="array_count_{type}" sig="fn array_count_{type}(array: [{type}], value: {type}) -> i32" desc="Count matching {type} values" types="bool,i32,i64,f64,str"
// @builtin-family category="Array" name="array_compare_{type}" sig="fn array_compare_{type}(left: [{type}], right: [{type}]) -> i32" desc="Lexicographically compare two {type} arrays" types="bool,i32,i64,f64,str"
// @builtin-family category="Array" name="array_sort_{type}" sig="fn! array_sort_{type}(array: [{type}])" desc="Sort a mutable {type} array in place" types="bool,i32,i64,f64,str"
// @builtin-family category="Array" name="array_any_{type}" sig="fn array_any_{type}(array: [{type}], predicate: fn({type}) -> bool) -> bool" desc="Test whether any {type} element matches a pure predicate" types="bool,i32,i64,f64,str"
// @builtin-family category="Array" name="array_all_{type}" sig="fn array_all_{type}(array: [{type}], predicate: fn({type}) -> bool) -> bool" desc="Test whether all {type} elements match a pure predicate" types="bool,i32,i64,f64,str"
// @builtin-family category="Array" name="array_filter_{type}" sig="fn! array_filter_{type}(array: [{type}], predicate: fn({type}) -> bool) -> [{type}]" desc="Allocate an ordered array of matching {type} elements" types="bool,i32,i64,f64,str"
// @builtin-family category="Array" name="array_fold_{type}" sig="fn array_fold_{type}(array: [{type}], initial: {type}, step: fn({type}, {type}) -> {type}) -> {type}" desc="Left-fold a {type} array with a pure step function" types="bool,i32,i64,f64,str"
// @builtin-family category="Array" name="array_for_each_{type}" sig="fn! array_for_each_{type}(array: [{type}], visit: fn!({type}) -> unit)" desc="Visit each {type} element in order" types="bool,i32,i64,f64,str"
// @builtin-map-family category="Array" name="array_map_{source}_to_{target}" sig="fn! array_map_{source}_to_{target}(array: [{source}], transform: fn({source}) -> {target}) -> [{target}]" desc="Transform an ordered {source} array into a {target} array" sources="bool,i32,i64,f64,str" targets="bool,i32,i64,f64,str"

use crate::types::BcType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreType {
    Bool,
    I32,
    I64,
    F64,
    Str,
}

impl CoreType {
    pub const ALL: [Self; 5] = [Self::Bool, Self::I32, Self::I64, Self::F64, Self::Str];

    pub fn from_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "bool" => Some(Self::Bool),
            "i32" => Some(Self::I32),
            "i64" => Some(Self::I64),
            "f64" => Some(Self::F64),
            "str" => Some(Self::Str),
            _ => None,
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F64 => "f64",
            Self::Str => "str",
        }
    }

    pub fn bc_type(self) -> BcType {
        match self {
            Self::Bool => BcType::Bool,
            Self::I32 => BcType::I32,
            Self::I64 => BcType::I64,
            Self::F64 => BcType::F64,
            Self::Str => BcType::Str,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollectionOp {
    Clone,
    Repeat,
    Reverse,
    Fill,
    Swap,
    Clear,
    Extend,
    Insert,
    RemoveAt,
    Slice,
    Contains,
    IndexOf,
    LastIndexOf,
    Count,
    Compare,
    Sort,
    Any,
    All,
    Map,
    Filter,
    Fold,
    ForEach,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollectionIntrinsic {
    pub op: CollectionOp,
    pub source: Option<CoreType>,
    pub target: Option<CoreType>,
}

impl CollectionIntrinsic {
    pub fn resolve(name: &str) -> Option<Self> {
        let generic = match name {
            "array_clone" => Some(CollectionOp::Clone),
            "array_repeat" => Some(CollectionOp::Repeat),
            "array_reverse" => Some(CollectionOp::Reverse),
            "array_fill" => Some(CollectionOp::Fill),
            "array_swap" => Some(CollectionOp::Swap),
            "array_clear" => Some(CollectionOp::Clear),
            "array_extend" => Some(CollectionOp::Extend),
            "array_insert" => Some(CollectionOp::Insert),
            "array_remove_at" => Some(CollectionOp::RemoveAt),
            "array_slice" => Some(CollectionOp::Slice),
            _ => None,
        };
        if let Some(op) = generic {
            return Some(Self {
                op,
                source: None,
                target: None,
            });
        }

        if let Some(rest) = name.strip_prefix("array_map_") {
            let (source, target) = rest.split_once("_to_")?;
            return Some(Self {
                op: CollectionOp::Map,
                source: Some(CoreType::from_suffix(source)?),
                target: Some(CoreType::from_suffix(target)?),
            });
        }

        let (op, suffix) = [
            ("array_last_index_of_", CollectionOp::LastIndexOf),
            ("array_index_of_", CollectionOp::IndexOf),
            ("array_contains_", CollectionOp::Contains),
            ("array_compare_", CollectionOp::Compare),
            ("array_for_each_", CollectionOp::ForEach),
            ("array_filter_", CollectionOp::Filter),
            ("array_count_", CollectionOp::Count),
            ("array_sort_", CollectionOp::Sort),
            ("array_fold_", CollectionOp::Fold),
            ("array_any_", CollectionOp::Any),
            ("array_all_", CollectionOp::All),
        ]
        .into_iter()
        .find_map(|(prefix, op)| name.strip_prefix(prefix).map(|suffix| (op, suffix)))?;

        Some(Self {
            op,
            source: Some(CoreType::from_suffix(suffix)?),
            target: None,
        })
    }

    pub fn resolve_with_alias(name: &str) -> Option<Self> {
        let canonical = match name {
            "sort_i32" => "array_sort_i32",
            "sort_i64" => "array_sort_i64",
            "sort_f64" => "array_sort_f64",
            "sort_str" => "array_sort_str",
            _ => name,
        };
        Self::resolve(canonical)
    }

    pub fn canonical_name(self) -> String {
        let typed = |prefix: &str, ty: CoreType| format!("{prefix}{}", ty.suffix());
        match self.op {
            CollectionOp::Clone => "array_clone".into(),
            CollectionOp::Repeat => "array_repeat".into(),
            CollectionOp::Reverse => "array_reverse".into(),
            CollectionOp::Fill => "array_fill".into(),
            CollectionOp::Swap => "array_swap".into(),
            CollectionOp::Clear => "array_clear".into(),
            CollectionOp::Extend => "array_extend".into(),
            CollectionOp::Insert => "array_insert".into(),
            CollectionOp::RemoveAt => "array_remove_at".into(),
            CollectionOp::Slice => "array_slice".into(),
            CollectionOp::Contains => typed("array_contains_", self.source.unwrap()),
            CollectionOp::IndexOf => typed("array_index_of_", self.source.unwrap()),
            CollectionOp::LastIndexOf => typed("array_last_index_of_", self.source.unwrap()),
            CollectionOp::Count => typed("array_count_", self.source.unwrap()),
            CollectionOp::Compare => typed("array_compare_", self.source.unwrap()),
            CollectionOp::Sort => typed("array_sort_", self.source.unwrap()),
            CollectionOp::Any => typed("array_any_", self.source.unwrap()),
            CollectionOp::All => typed("array_all_", self.source.unwrap()),
            CollectionOp::Map => format!(
                "array_map_{}_to_{}",
                self.source.unwrap().suffix(),
                self.target.unwrap().suffix()
            ),
            CollectionOp::Filter => typed("array_filter_", self.source.unwrap()),
            CollectionOp::Fold => typed("array_fold_", self.source.unwrap()),
            CollectionOp::ForEach => typed("array_for_each_", self.source.unwrap()),
        }
    }

    pub fn arity(self) -> usize {
        match self.op {
            CollectionOp::Clone
            | CollectionOp::Reverse
            | CollectionOp::Clear
            | CollectionOp::Sort => 1,
            CollectionOp::Repeat
            | CollectionOp::Fill
            | CollectionOp::Extend
            | CollectionOp::RemoveAt
            | CollectionOp::Contains
            | CollectionOp::IndexOf
            | CollectionOp::LastIndexOf
            | CollectionOp::Count
            | CollectionOp::Compare
            | CollectionOp::Any
            | CollectionOp::All
            | CollectionOp::Map
            | CollectionOp::Filter
            | CollectionOp::ForEach => 2,
            CollectionOp::Swap
            | CollectionOp::Insert
            | CollectionOp::Slice
            | CollectionOp::Fold => 3,
        }
    }

    pub fn is_mutating(self) -> bool {
        matches!(
            self.op,
            CollectionOp::Reverse
                | CollectionOp::Fill
                | CollectionOp::Swap
                | CollectionOp::Clear
                | CollectionOp::Extend
                | CollectionOp::Insert
                | CollectionOp::RemoveAt
                | CollectionOp::Sort
        )
    }

    pub fn requires_dynamic_array(self) -> bool {
        matches!(
            self.op,
            CollectionOp::Clear
                | CollectionOp::Extend
                | CollectionOp::Insert
                | CollectionOp::RemoveAt
        )
    }

    pub fn is_pure(self) -> bool {
        matches!(
            self.op,
            CollectionOp::Contains
                | CollectionOp::IndexOf
                | CollectionOp::LastIndexOf
                | CollectionOp::Count
                | CollectionOp::Compare
                | CollectionOp::Any
                | CollectionOp::All
                | CollectionOp::Fold
        )
    }

    pub fn is_higher_order(self) -> bool {
        matches!(
            self.op,
            CollectionOp::Any
                | CollectionOp::All
                | CollectionOp::Map
                | CollectionOp::Filter
                | CollectionOp::Fold
                | CollectionOp::ForEach
        )
    }

    pub fn callback_type(self) -> Option<BcType> {
        let source = self.source?.bc_type();
        let (params, ret, is_pure) = match self.op {
            CollectionOp::Any | CollectionOp::All | CollectionOp::Filter => {
                (vec![source], BcType::Bool, true)
            }
            CollectionOp::Map => (
                vec![source],
                self.target.expect("map target").bc_type(),
                true,
            ),
            CollectionOp::Fold => (vec![source.clone(), source.clone()], source, true),
            CollectionOp::ForEach => (vec![source], BcType::Unit, false),
            _ => return None,
        };
        Some(BcType::FnPtr(params, Box::new(ret), is_pure))
    }

    pub fn callback_index(self) -> Option<usize> {
        match self.op {
            CollectionOp::Any
            | CollectionOp::All
            | CollectionOp::Map
            | CollectionOp::Filter
            | CollectionOp::ForEach => Some(1),
            CollectionOp::Fold => Some(2),
            _ => None,
        }
    }

    pub fn runtime_symbol(self) -> Option<String> {
        let symbol = match self.op {
            CollectionOp::Clone => "osc_array_clone".into(),
            CollectionOp::Repeat => "osc_array_repeat".into(),
            CollectionOp::Reverse => "osc_array_reverse".into(),
            CollectionOp::Fill => "osc_array_fill".into(),
            CollectionOp::Swap => "osc_array_swap".into(),
            CollectionOp::Clear => "osc_array_clear".into(),
            CollectionOp::Extend => "osc_array_extend".into(),
            CollectionOp::Insert => "osc_array_insert".into(),
            CollectionOp::RemoveAt => "osc_array_remove_at".into(),
            CollectionOp::Slice => "osc_array_slice".into(),
            CollectionOp::Contains
            | CollectionOp::IndexOf
            | CollectionOp::LastIndexOf
            | CollectionOp::Count
            | CollectionOp::Compare
            | CollectionOp::Sort => format!(
                "osc_array_{}_{}",
                match self.op {
                    CollectionOp::Contains => "contains",
                    CollectionOp::IndexOf => "index_of",
                    CollectionOp::LastIndexOf => "last_index_of",
                    CollectionOp::Count => "count",
                    CollectionOp::Compare => "compare",
                    CollectionOp::Sort => "sort",
                    _ => unreachable!(),
                },
                self.source.unwrap().suffix()
            ),
            _ => return None,
        };
        Some(symbol)
    }
}

pub fn array_element_type(ty: &BcType) -> Option<BcType> {
    match ty {
        BcType::Array(elem) | BcType::FixedArray(elem, _) => Some((**elem).clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_all_map_combinations() {
        for source in CoreType::ALL {
            for target in CoreType::ALL {
                let name = format!("array_map_{}_to_{}", source.suffix(), target.suffix());
                let intrinsic = CollectionIntrinsic::resolve(&name).unwrap();
                assert_eq!(intrinsic.source, Some(source));
                assert_eq!(intrinsic.target, Some(target));
                assert_eq!(intrinsic.canonical_name(), name);
            }
        }
    }

    #[test]
    fn resolves_sort_aliases_to_canonical_intrinsics() {
        assert_eq!(
            CollectionIntrinsic::resolve_with_alias("sort_i32"),
            CollectionIntrinsic::resolve("array_sort_i32")
        );
        assert_eq!(
            CollectionIntrinsic::resolve_with_alias("sort_str"),
            CollectionIntrinsic::resolve("array_sort_str")
        );
    }
}
