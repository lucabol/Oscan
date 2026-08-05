use std::collections::HashMap;

use cranelift_codegen::ir::SourceLoc;
use cranelift_codegen::Context;
use cranelift_module::FuncId;
use cranelift_object::ObjectProduct;
use gimli::write::{
    Address, AttributeValue, DwarfUnit, EndianVec, LineProgram, LineString, RelocateWriter,
    Relocation, RelocationTarget, Sections,
};
use gimli::{constants, Encoding, Format, LittleEndian};
use object::write::{Relocation as ObjectRelocation, SymbolId};
use object::{BinaryFormat, RelocationEncoding, RelocationFlags, RelocationKind, SectionKind};

use crate::debuginfo::{SourceLocation, SourceMap};

#[derive(Clone)]
struct DebugSection {
    writer: EndianVec<LittleEndian>,
    relocations: Vec<Relocation>,
}

impl DebugSection {
    fn new() -> Self {
        DebugSection {
            writer: EndianVec::new(LittleEndian),
            relocations: Vec::new(),
        }
    }
}

impl RelocateWriter for DebugSection {
    type Writer = EndianVec<LittleEndian>;

    fn writer(&self) -> &Self::Writer {
        &self.writer
    }

    fn writer_mut(&mut self) -> &mut Self::Writer {
        &mut self.writer
    }

    fn relocate(&mut self, relocation: Relocation) {
        self.relocations.push(relocation);
    }
}

#[derive(Clone)]
struct FunctionSource {
    source_name: String,
    linkage_name: String,
    declaration: SourceLocation,
}

struct LineRow {
    offset: u64,
    location: SourceLocation,
}

struct FunctionDebug {
    func_id: FuncId,
    source: FunctionSource,
    size: u64,
    rows: Vec<LineRow>,
}

pub(super) struct CraneliftDebug {
    source_map: SourceMap,
    locations: Vec<SourceLocation>,
    location_ids: HashMap<SourceLocation, u32>,
    function_sources: HashMap<FuncId, FunctionSource>,
    functions: Vec<FunctionDebug>,
}

impl CraneliftDebug {
    pub(super) fn new(source_map: &SourceMap) -> Self {
        CraneliftDebug {
            source_map: source_map.clone(),
            locations: Vec::new(),
            location_ids: HashMap::new(),
            function_sources: HashMap::new(),
            functions: Vec::new(),
        }
    }

    pub(super) fn source_loc(&mut self, location: Option<SourceLocation>) -> SourceLoc {
        let Some(location) = location else {
            return SourceLoc::default();
        };
        if let Some(id) = self.location_ids.get(&location) {
            return SourceLoc::new(*id);
        }
        let id = u32::try_from(self.locations.len())
            .expect("internal error: Cranelift source-location table exceeded u32::MAX entries");
        assert_ne!(
            id,
            u32::MAX,
            "internal error: Cranelift source-location table exhausted its non-sentinel IDs"
        );
        self.locations.push(location);
        self.location_ids.insert(location, id);
        SourceLoc::new(id)
    }

    pub(super) fn set_function_source(
        &mut self,
        func_id: FuncId,
        source_name: &str,
        linkage_name: &str,
        declaration: SourceLocation,
    ) {
        self.function_sources.insert(
            func_id,
            FunctionSource {
                source_name: source_name.to_string(),
                linkage_name: linkage_name.to_string(),
                declaration,
            },
        );
    }

    pub(super) fn capture_function(
        &mut self,
        func_id: FuncId,
        context: &Context,
    ) -> Result<(), String> {
        let Some(source) = self.function_sources.remove(&func_id) else {
            return Ok(());
        };
        let code = context
            .compiled_code()
            .ok_or_else(|| "Cranelift did not retain compiled function code".to_string())?;
        let size = u64::from(code.buffer.total_size());
        if size == 0 {
            return Ok(());
        }

        let mut rows = Vec::new();
        for mapping in code.buffer.get_srclocs_sorted() {
            if mapping.loc.is_default() {
                continue;
            }
            let Some(location) = self.locations.get(mapping.loc.bits() as usize).copied() else {
                return Err(format!(
                    "Cranelift returned unknown source location {}",
                    mapping.loc.bits()
                ));
            };
            let offset = u64::from(mapping.start);
            if offset >= size {
                continue;
            }
            if rows
                .last()
                .is_some_and(|row: &LineRow| row.location == location)
            {
                continue;
            }
            rows.push(LineRow { offset, location });
        }
        if rows.first().map_or(true, |row| row.offset != 0) {
            rows.insert(
                0,
                LineRow {
                    offset: 0,
                    location: source.declaration,
                },
            );
        }

        self.functions.push(FunctionDebug {
            func_id,
            source,
            size,
            rows,
        });
        Ok(())
    }

    pub(super) fn write(self, product: &mut ObjectProduct) -> Result<(), String> {
        if self.functions.is_empty() {
            return Ok(());
        }

        let encoding = Encoding {
            format: Format::Dwarf32,
            version: 4,
            address_size: 8,
        };
        let primary_path = debug_path(
            self.source_map
                .primary_path()
                .ok_or_else(|| "source map has no primary source file".to_string())?,
        );
        let mut line_program = LineProgram::new(
            encoding,
            gimli::LineEncoding::default(),
            LineString::String(b".".to_vec()),
            None,
            LineString::String(primary_path.clone()),
            None,
        );
        let mut files = HashMap::new();
        for (file, path) in self.source_map.files() {
            let file_id = line_program.add_file(
                LineString::String(debug_path(path)),
                line_program.default_directory(),
                None,
            );
            files.insert(file, file_id);
        }

        let function_symbols: Vec<SymbolId> = self
            .functions
            .iter()
            .map(|function| product.function_symbol(function.func_id))
            .collect();
        for (symbol, function) in self.functions.iter().enumerate() {
            line_program.begin_sequence(Some(Address::Symbol { symbol, addend: 0 }));
            for source_row in &function.rows {
                let row = line_program.row();
                row.address_offset = source_row.offset;
                row.file = files[&source_row.location.file];
                row.line = u64::from(source_row.location.line);
                row.column = u64::from(source_row.location.column);
                row.is_statement = true;
                line_program.generate_row();
            }
            line_program.end_sequence(function.size);
        }

        let mut dwarf = DwarfUnit::new(encoding);
        dwarf.unit.line_program = line_program;
        add_compile_unit_attributes(&mut dwarf, &primary_path);
        for (symbol, function) in self.functions.iter().enumerate() {
            let entry = dwarf
                .unit
                .add(dwarf.unit.root(), constants::DW_TAG_subprogram);
            let name = dwarf
                .strings
                .add(function.source.source_name.as_bytes().to_vec());
            let linkage_name = dwarf
                .strings
                .add(function.source.linkage_name.as_bytes().to_vec());
            let die = dwarf.unit.get_mut(entry);
            die.set(constants::DW_AT_name, AttributeValue::StringRef(name));
            die.set(
                constants::DW_AT_linkage_name,
                AttributeValue::StringRef(linkage_name),
            );
            die.set(
                constants::DW_AT_low_pc,
                AttributeValue::Address(Address::Symbol { symbol, addend: 0 }),
            );
            die.set(
                constants::DW_AT_high_pc,
                AttributeValue::Udata(function.size),
            );
            die.set(
                constants::DW_AT_decl_file,
                AttributeValue::FileIndex(Some(files[&function.source.declaration.file])),
            );
            die.set(
                constants::DW_AT_decl_line,
                AttributeValue::Udata(u64::from(function.source.declaration.line)),
            );
            die.set(
                constants::DW_AT_decl_column,
                AttributeValue::Udata(u64::from(function.source.declaration.column)),
            );
            die.set(constants::DW_AT_external, AttributeValue::Flag(true));
        }

        let mut sections = Sections::new(DebugSection::new());
        dwarf
            .write(&mut sections)
            .map_err(|error| format!("failed to encode Cranelift DWARF: {error}"))?;
        append_sections(product, &sections, &function_symbols)
    }
}

fn add_compile_unit_attributes(dwarf: &mut DwarfUnit, primary_path: &[u8]) {
    let producer = dwarf.strings.add(b"Oscan compiler".to_vec());
    let name = dwarf.strings.add(primary_path.to_vec());
    let comp_dir = dwarf.strings.add(b".".to_vec());
    let root = dwarf.unit.root();
    let entry = dwarf.unit.get_mut(root);
    entry.set(
        constants::DW_AT_producer,
        AttributeValue::StringRef(producer),
    );
    entry.set(
        constants::DW_AT_language,
        AttributeValue::Language(constants::DW_LANG_lo_user),
    );
    entry.set(constants::DW_AT_name, AttributeValue::StringRef(name));
    entry.set(
        constants::DW_AT_comp_dir,
        AttributeValue::StringRef(comp_dir),
    );
    entry.set(constants::DW_AT_stmt_list, AttributeValue::LineProgramRef);
}

fn append_sections(
    product: &mut ObjectProduct,
    sections: &Sections<DebugSection>,
    function_symbols: &[SymbolId],
) -> Result<(), String> {
    let mut object_sections = Vec::new();
    sections
        .for_each(|id, section| {
            if !section.writer.slice().is_empty() {
                let object_id = product.object.add_section(
                    Vec::new(),
                    id.name().as_bytes().to_vec(),
                    SectionKind::Debug,
                );
                product
                    .object
                    .set_section_data(object_id, section.writer.slice().to_vec(), 1);
                object_sections.push((id, object_id));
            }
            Ok::<(), String>(())
        })
        .map_err(|error| error.to_string())?;

    sections
        .for_each(|id, section| {
            let Some((_, object_id)) = object_sections
                .iter()
                .find(|(section_id, _)| *section_id == id)
                .copied()
            else {
                return Ok(());
            };
            for relocation in &section.relocations {
                let (symbol, kind) = match relocation.target {
                    RelocationTarget::Symbol(symbol) => {
                        let Some(symbol) = function_symbols.get(symbol).copied() else {
                            return Err(format!(
                                "DWARF relocation referenced unknown function symbol {symbol}"
                            ));
                        };
                        (symbol, RelocationKind::Absolute)
                    }
                    RelocationTarget::Section(target) => {
                        let Some((_, target_id)) = object_sections
                            .iter()
                            .find(|(section_id, _)| *section_id == target)
                            .copied()
                        else {
                            return Err(format!(
                                "DWARF relocation referenced missing section {}",
                                target.name()
                            ));
                        };
                        let symbol = product.object.section_symbol(target_id);
                        let kind = if product.object.format() == BinaryFormat::Coff {
                            RelocationKind::SectionOffset
                        } else {
                            RelocationKind::Absolute
                        };
                        (symbol, kind)
                    }
                };
                product
                    .object
                    .add_relocation(
                        object_id,
                        ObjectRelocation {
                            offset: relocation.offset as u64,
                            symbol,
                            addend: relocation.addend,
                            flags: RelocationFlags::Generic {
                                kind,
                                encoding: RelocationEncoding::Generic,
                                size: relocation.size * 8,
                            },
                        },
                    )
                    .map_err(|error| {
                        format!("failed to add relocation to {}: {error}", id.name())
                    })?;
            }
            Ok(())
        })
        .map_err(|error| error.to_string())
}

fn debug_path(path: &std::path::Path) -> Vec<u8> {
    let path = path.to_string_lossy();
    if cfg!(windows) {
        path.replace('\\', "/").into_bytes()
    } else {
        path.into_owned().into_bytes()
    }
}
