//! Memory layout and value-representation rules shared by every part of
//! the Cranelift backend.
//!
//! Two orthogonal questions are answered here:
//!
//! * [`layout_of`] — "how many bytes does a value of this type occupy in
//!   memory, and what is its alignment?" Used for `osc_array` element
//!   sizes, arena allocation sizes, and struct/enum/result field offsets.
//!   The algorithm is the ordinary C structure-layout algorithm (fields in
//!   declaration order, each at the next offset satisfying its own natural
//!   alignment, total size rounded up to the maximum member alignment) —
//!   deliberately, since it must agree byte-for-byte with the real C
//!   compiler that built the runtime archive for every type that crosses
//!   the shim boundary (`super::runtime_abi`), and reusing the same
//!   algorithm for user structs/enums keeps one mental model everywhere.
//! * [`Repr::of`] — "what Cranelift SSA shape does a value of this type
//!   have?" Every `BcType` is either a direct scalar (`I32`/`I64`/`F64`/
//!   `I8` for `bool`, or `I32` for a payload-less enum) or a single
//!   pointer-sized value (everything else). Pointer-repr aggregates
//!   (`str`, payload-bearing enums, structs, `Result`) always point at
//!   arena-allocated memory — never a raw Cranelift stack slot — because
//!   Oscan values routinely outlive the expression that produced them
//!   (returned, stored in a struct field, pushed into an array), and only
//!   the arena gives that lifetime. This trades a small amount of
//!   performance (every aggregate temporary is a real allocation) for a
//!   dramatically simpler and more obviously-correct implementation.
//!   Stack-slot escape analysis remains a possible future optimization.

use std::collections::HashMap;

use cranelift_codegen::ir::types;
use cranelift_codegen::ir::Type;

use crate::ir;
use crate::types::BcType;

/// Size/alignment of a value in memory, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub size: u32,
    pub align: u32,
}

impl Layout {
    const fn scalar(size: u32) -> Self {
        Layout { size, align: size }
    }

    /// Grow `self` to include a field of `field` size/align placed at the
    /// next correctly-aligned offset. Returns that offset.
    fn place(&mut self, field: Layout) -> u32 {
        let offset = round_up(self.size, field.align.max(1));
        self.size = offset + field.size;
        self.align = self.align.max(field.align);
        offset
    }

    /// Pad `self.size` up to `self.align` (the final step of C struct
    /// layout: total size is a multiple of the maximum member alignment).
    fn finish(mut self) -> Self {
        self.size = round_up(self.size, self.align.max(1));
        self
    }
}

fn round_up(value: u32, align: u32) -> u32 {
    if align <= 1 {
        return value;
    }
    (value + align - 1) / align * align
}

/// The Cranelift-level pointer size for every target this backend
/// supports (Windows/Linux x86-64, AArch64, RISC-V64 are all LP64/64-bit
/// pointer targets); kept as a named constant rather than sprinkling `8`
/// everywhere so a 32-bit target would need to change exactly one line.
pub const POINTER_SIZE: u32 = 8;

/// How a `BcType` flows through Cranelift SSA values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repr {
    /// No value at all (a Cranelift call/block with zero results).
    Unit,
    /// A single scalar Cranelift value of the given type.
    Scalar(Type),
    /// A single pointer-sized value. For `Str`/payload-bearing `Enum`/
    /// `Struct`/`Result` this points at an arena-allocated memory block
    /// laid out per [`layout_of`]. For `Array`/`FixedArray`/`Map*` it is
    /// the opaque `osc_array*`/`osc_map*` runtime pointer. For `Handle` it
    /// is a bare integer carried in a pointer-sized register. For `FnPtr`
    /// it is a code address.
    Pointer,
}

impl Repr {
    pub fn of(ty: &BcType, program: &ir::Program) -> Repr {
        match ty {
            BcType::Unit => Repr::Unit,
            BcType::I32 => Repr::Scalar(types::I32),
            BcType::I64 => Repr::Scalar(types::I64),
            BcType::F64 => Repr::Scalar(types::F64),
            BcType::Bool => Repr::Scalar(types::I8),
            BcType::Enum(name) => {
                if enum_has_payload(name, program) {
                    Repr::Pointer
                } else {
                    Repr::Scalar(types::I32)
                }
            }
            _ => Repr::Pointer,
        }
    }

    /// The Cranelift type of the single value used to represent this repr,
    /// or `None` for `Unit` (which carries no value at all).
    pub fn cl_type(&self) -> Option<Type> {
        match self {
            Repr::Unit => None,
            Repr::Scalar(t) => Some(*t),
            Repr::Pointer => Some(cl_pointer_type()),
        }
    }
}

/// The Cranelift integer type used for every pointer-repr value. A plain
/// `I64` (rather than `isa.pointer_type()`) is correct because every
/// target this backend supports is a 64-bit target (see [`POINTER_SIZE`]).
pub fn cl_pointer_type() -> Type {
    types::I64
}

pub fn enum_has_payload(name: &str, program: &ir::Program) -> bool {
    program
        .enums
        .get(name)
        .map(|info| info.variants.iter().any(|(_, tys)| !tys.is_empty()))
        .unwrap_or(false)
}

/// Sequential C-style layout of an ordered list of fields.
fn sequential_layout<'a>(
    fields: impl Iterator<Item = &'a BcType>,
    program: &ir::Program,
) -> (Vec<u32>, Layout) {
    let mut layout = Layout { size: 0, align: 1 };
    let mut offsets = Vec::new();
    for field_ty in fields {
        let field_layout = layout_of(field_ty, program);
        offsets.push(layout.place(field_layout));
    }
    (offsets, layout.finish())
}

/// The in-memory size/alignment of a value of type `ty`.
pub fn layout_of(ty: &BcType, program: &ir::Program) -> Layout {
    match ty {
        BcType::Unit => Layout { size: 0, align: 1 },
        BcType::I32 => Layout::scalar(4),
        BcType::I64 => Layout::scalar(8),
        BcType::F64 => Layout::scalar(8),
        BcType::Bool => Layout::scalar(1),
        BcType::Handle => Layout::scalar(POINTER_SIZE),
        BcType::Str => {
            // `{ const char* data; int32_t len; }` — matches `osc_str` in
            // runtime/osc_runtime.h exactly (see module docs).
            let (_, layout) = sequential_layout([BcType::Handle, BcType::I32].iter(), program);
            layout
        }
        BcType::Map
        | BcType::MapStrI32
        | BcType::MapStrI64
        | BcType::MapStrF64
        | BcType::MapI32Str
        | BcType::MapI32I32
        | BcType::Array(_)
        | BcType::FixedArray(_, _)
        | BcType::FnPtr(_, _) => Layout::scalar(POINTER_SIZE),
        BcType::Struct(name) => struct_layout(name, program).1,
        BcType::Enum(name) => enum_layout(name, program).total,
        BcType::Result(ok, err) => result_layout(ok, err, program).total,
    }
}

/// Field offsets (in declaration order) and overall layout of a
/// user-defined struct.
pub fn struct_layout(name: &str, program: &ir::Program) -> (Vec<u32>, Layout) {
    match program.structs.get(name) {
        Some(info) => sequential_layout(info.fields.iter().map(|(_, t)| t), program),
        None => (Vec::new(), Layout { size: 0, align: 1 }),
    }
}

/// Offset of `field_name` within a struct, and its type. Panics if the
/// field does not exist — callers only reach this after semantic analysis
/// (and the IR verifier) have already accepted the program, so an unknown
/// field here would be an internal compiler error, not a user error.
pub fn struct_field_offset(name: &str, field_name: &str, program: &ir::Program) -> (u32, BcType) {
    let info = program
        .structs
        .get(name)
        .unwrap_or_else(|| panic!("internal error: unknown struct '{name}'"));
    let (offsets, _) = struct_layout(name, program);
    for (i, (fname, fty)) in info.fields.iter().enumerate() {
        if fname == field_name {
            return (offsets[i], fty.clone());
        }
    }
    panic!("internal error: struct '{name}' has no field '{field_name}'");
}

/// Layout of a user-defined enum: a 4-byte tag (matching the C backend's
/// `int tag`) followed — only when at least one variant carries a payload
/// — by a union big enough for the largest variant's payload tuple.
/// Payload-less enums (`enum_has_payload` false) still get a layout here
/// (`{size: 4, align: 4}`, matching C's `typedef int Name;`) even though
/// [`Repr::of`] represents them as a bare scalar rather than a pointer to
/// one of these blocks — `layout_of` always answers "how many bytes",
/// independent of how the value flows through Cranelift SSA.
#[allow(dead_code)]
pub struct EnumLayout {
    pub has_payload: bool,
    pub payload_offset: u32,
    pub total: Layout,
    /// Per-variant payload field offsets (relative to the start of the
    /// enum block, i.e. already including `payload_offset`), keyed by
    /// variant name.
    pub variant_field_offsets: HashMap<String, Vec<u32>>,
}

pub fn enum_layout(name: &str, program: &ir::Program) -> EnumLayout {
    let tag_layout = Layout::scalar(4);
    let Some(info) = program.enums.get(name) else {
        return EnumLayout {
            has_payload: false,
            payload_offset: 0,
            total: tag_layout,
            variant_field_offsets: HashMap::new(),
        };
    };
    let has_payload = info.variants.iter().any(|(_, tys)| !tys.is_empty());
    if !has_payload {
        return EnumLayout {
            has_payload: false,
            payload_offset: 0,
            total: tag_layout,
            variant_field_offsets: HashMap::new(),
        };
    }

    // First pass: compute each variant's own tuple layout (offsets relative
    // to the *start of the union*), and the union's overall size/align.
    let mut union_layout = Layout { size: 0, align: 1 };
    let mut per_variant_local_offsets = HashMap::new();
    for (vname, tys) in &info.variants {
        let (offsets, layout) = sequential_layout(tys.iter(), program);
        union_layout.size = union_layout.size.max(layout.size);
        union_layout.align = union_layout.align.max(layout.align.max(1));
        per_variant_local_offsets.insert(vname.clone(), offsets);
    }

    let mut total = Layout { size: 0, align: 1 };
    total.place(tag_layout);
    let payload_offset = total.place(union_layout);
    let total = total.finish();

    let variant_field_offsets = per_variant_local_offsets
        .into_iter()
        .map(|(vname, offsets)| {
            let shifted = offsets.into_iter().map(|o| o + payload_offset).collect();
            (vname, shifted)
        })
        .collect();

    EnumLayout {
        has_payload: true,
        payload_offset,
        total,
        variant_field_offsets,
    }
}

/// Layout of a `Result<ok, err>` value: `{ uint8_t is_ok; union { ok; err; } value; }`,
/// matching `OSC_RESULT_DECL` in runtime/osc_runtime.h exactly (both are
/// plain C structs with no unusual alignment pragmas, so the ordinary
/// natural-alignment algorithm agrees with whatever C compiler built the
/// runtime archive).
#[allow(dead_code)]
pub struct ResultLayout {
    pub payload_offset: u32,
    pub ok_offset: u32,
    pub err_offset: u32,
    pub total: Layout,
}

pub fn result_layout(ok: &BcType, err: &BcType, program: &ir::Program) -> ResultLayout {
    let tag_layout = Layout::scalar(1);
    let ok_layout = layout_of(ok, program);
    let err_layout = layout_of(err, program);
    let union_layout = Layout {
        size: ok_layout.size.max(err_layout.size),
        align: ok_layout.align.max(err_layout.align).max(1),
    };

    let mut total = Layout { size: 0, align: 1 };
    total.place(tag_layout);
    let payload_offset = total.place(union_layout);
    let total = total.finish();

    ResultLayout {
        payload_offset,
        ok_offset: payload_offset,
        err_offset: payload_offset,
        total,
    }
}
