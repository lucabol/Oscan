use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::token::Span;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DebugInfo {
    #[default]
    None,
    LineTables,
}

impl DebugInfo {
    pub fn is_enabled(self) -> bool {
        self != Self::None
    }
}

impl FromStr for DebugInfo {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "line-tables" => Ok(Self::LineTables),
            other => Err(format!(
                "unknown debug-info level '{other}' (supported: none, line-tables)"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceFileId(u32);

impl SourceFileId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceLine {
    pub file: SourceFileId,
    pub line: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceLocation {
    pub file: SourceFileId,
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    files: Vec<PathBuf>,
    lines: Vec<Option<SourceLine>>,
}

impl SourceMap {
    pub fn location(&self, span: Span) -> Option<SourceLocation> {
        let flat_line = usize::try_from(span.line).ok()?.checked_sub(1)?;
        let origin = self.lines.get(flat_line)?.as_ref()?;
        Some(SourceLocation {
            file: origin.file,
            line: origin.line,
            column: u32::try_from(span.column.max(1)).unwrap_or(u32::MAX),
        })
    }

    pub fn path(&self, file: SourceFileId) -> &Path {
        &self.files[file.index()]
    }

    pub fn files(&self) -> impl Iterator<Item = (SourceFileId, &Path)> {
        self.files
            .iter()
            .enumerate()
            .map(|(index, path)| (SourceFileId(index as u32), path.as_path()))
    }

    pub fn primary_path(&self) -> Option<&Path> {
        self.files.first().map(PathBuf::as_path)
    }
}

#[derive(Default)]
pub struct SourceMapBuilder {
    files: Vec<PathBuf>,
    file_ids: HashMap<PathBuf, SourceFileId>,
}

impl SourceMapBuilder {
    pub fn intern_file(&mut self, path: PathBuf) -> SourceFileId {
        let path = Self::debugger_path(path);
        if let Some(id) = self.file_ids.get(&path) {
            return *id;
        }
        let id = SourceFileId(
            u32::try_from(self.files.len()).expect("too many source files for debug information"),
        );
        self.files.push(path.clone());
        self.file_ids.insert(path, id);
        id
    }

    pub fn finish(self, lines: Vec<Option<SourceLine>>) -> SourceMap {
        SourceMap {
            files: self.files,
            lines,
        }
    }

    #[cfg(windows)]
    fn debugger_path(path: PathBuf) -> PathBuf {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{rest}"))
        } else if let Some(rest) = text.strip_prefix(r"\\?\") {
            PathBuf::from(rest)
        } else {
            path
        }
    }

    #[cfg(not(windows))]
    fn debugger_path(path: PathBuf) -> PathBuf {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_flattened_lines_to_original_files() {
        let mut builder = SourceMapBuilder::default();
        let root = builder.intern_file(PathBuf::from("root.osc"));
        let imported = builder.intern_file(PathBuf::from("lib.osc"));
        let map = builder.finish(vec![
            Some(SourceLine {
                file: imported,
                line: 7,
            }),
            None,
            Some(SourceLine {
                file: root,
                line: 2,
            }),
        ]);

        assert_eq!(
            map.location(Span::new(1, 4)),
            Some(SourceLocation {
                file: imported,
                line: 7,
                column: 4,
            })
        );
        assert_eq!(map.location(Span::new(2, 1)), None);
        assert_eq!(map.path(root), Path::new("root.osc"));
    }

    #[test]
    fn parses_supported_debug_levels() {
        assert_eq!("none".parse(), Ok(DebugInfo::None));
        assert_eq!("line-tables".parse(), Ok(DebugInfo::LineTables));
        assert!("full".parse::<DebugInfo>().is_err());
    }

    #[cfg(windows)]
    #[test]
    fn removes_windows_verbatim_prefix_from_debugger_paths() {
        assert_eq!(
            SourceMapBuilder::debugger_path(PathBuf::from(r"\\?\C:\src\main.osc")),
            PathBuf::from(r"C:\src\main.osc")
        );
        assert_eq!(
            SourceMapBuilder::debugger_path(PathBuf::from(r"\\?\UNC\server\share\main.osc")),
            PathBuf::from(r"\\server\share\main.osc")
        );
    }
}
