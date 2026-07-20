/// Mangle identifiers that clash with C keywords or standard type names.
pub(crate) fn mangle_c_name(name: &str) -> String {
    const C_RESERVED: &[&str] = &[
        "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else",
        "enum", "extern", "float", "for", "goto", "if", "int", "long", "register", "return",
        "short", "signed", "sizeof", "static", "struct", "switch", "typedef", "union", "unsigned",
        "void", "volatile", "while", "inline", "restrict",
    ];
    if C_RESERVED.contains(&name) {
        format!("{}_", name)
    } else {
        name.to_string()
    }
}
