use std::path::{Path, PathBuf};
use std::process::Command;

// sha2 is not yet a [build-dependencies] crate as of this writing (Bishop
// adds it to Cargo.toml's [dependencies] and [build-dependencies] in a
// parallel change; see docs/design/native-link-embedding.md §5.2/§8). Until
// that lands, this file intentionally fails `cargo check`/`cargo build` on
// this `use` alone — that failure is expected and reconciled once both
// halves of the native-link-embedding change are merged together.
use sha2::{Digest, Sha256};

fn main() {
    stamp_git_version();
    generate_native_link_assets();
}

fn stamp_git_version() {
    // Priority: OSCAN_VERSION env var (set by CI) > git describe > "unknown"
    let version = std::env::var("OSCAN_VERSION")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            Command::new("git")
                .args(["describe", "--tags", "--always", "--dirty"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "unknown".to_string())
        });

    println!("cargo:rustc-env=GIT_VERSION={version}");
    println!("cargo:rerun-if-env-changed=OSCAN_VERSION");
    // Rebuild when git HEAD changes (new commits, tags, etc.)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");
}

// ---------------------------------------------------------------------------
// Embedded native-link assets (docs/design/native-link-embedding.md §5).
//
// Reads OSCAN_EMBED_ASSETS_DIR (a directory staged by
// `scripts/prepare-embed-assets.ps1|.sh` / `release_tools.py
// prepare-embed-assets`, containing native-link-assets.json + the assets at
// their install_subpaths) and OSCAN_REQUIRE_EMBEDDED_ASSETS (release builds
// only; fails the build instead of silently omitting assets). No network
// access: only the already-staged directory is read. Writes
// `${OUT_DIR}/native_link_assets_generated.rs`, `include!`d by
// `src/backend/native_assets.rs`.
// ---------------------------------------------------------------------------

fn generate_native_link_assets() {
    println!("cargo:rerun-if-env-changed=OSCAN_EMBED_ASSETS_DIR");
    println!("cargo:rerun-if-env-changed=OSCAN_REQUIRE_EMBEDDED_ASSETS");

    let require = matches!(
        std::env::var("OSCAN_REQUIRE_EMBEDDED_ASSETS").as_deref(),
        Ok("1") | Ok("true")
    );

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set by cargo");
    let dest = Path::new(&out_dir).join("native_link_assets_generated.rs");

    let assets_dir = std::env::var_os("OSCAN_EMBED_ASSETS_DIR").map(PathBuf::from);

    let generated = match assets_dir {
        None => {
            if require {
                panic!(
                    "OSCAN_REQUIRE_EMBEDDED_ASSETS is set but OSCAN_EMBED_ASSETS_DIR is unset; \
                     stage assets first with scripts/prepare-embed-assets.ps1|.sh and set \
                     OSCAN_EMBED_ASSETS_DIR to its output directory before building."
                );
            }
            empty_generated_source()
        }
        Some(dir) => {
            let manifest_path = dir.join("native-link-assets.json");
            // Only rerun-if-changed the manifest when the dir was actually
            // configured; an absent OSCAN_EMBED_ASSETS_DIR must not make an
            // ordinary dev `cargo build` depend on a path that doesn't exist.
            println!("cargo:rerun-if-changed={}", manifest_path.display());
            match load_and_verify_embedded_assets(&dir, &manifest_path) {
                Ok(source) => source,
                Err(reason) => {
                    if require {
                        panic!(
                            "embedded native-link assets in {} are incomplete or invalid: {reason}",
                            dir.display()
                        );
                    }
                    println!(
                        "cargo:warning=embedded native-link assets in {} are incomplete or \
                         invalid ({reason}); building without embedded assets",
                        dir.display()
                    );
                    empty_generated_source()
                }
            }
        }
    };

    std::fs::write(&dest, generated)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", dest.display()));
}

fn empty_generated_source() -> String {
    r#"
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedAsset {
    pub role: &'static str,
    pub name: &'static str,
    pub lib: Option<&'static str>,
    pub install_subpath: &'static str,
    pub sha256: &'static str,
    pub len: usize,
    pub bytes: &'static [u8],
}

pub const EMBEDDED_ASSETS_PRESENT: bool = false;
pub static EMBEDDED_ASSETS: &[EmbeddedAsset] = &[];
pub static EMBEDDED_ASSET_MANIFEST_JSON: &str = "";
"#
    .to_string()
}

struct ManifestAsset {
    role: String,
    name: String,
    lib: Option<String>,
    install_subpath: String,
    sha256: String,
}

fn load_and_verify_embedded_assets(dir: &Path, manifest_path: &Path) -> Result<String, String> {
    if !manifest_path.is_file() {
        return Err(format!("missing {}", manifest_path.display()));
    }
    let manifest_json = std::fs::read_to_string(manifest_path)
        .map_err(|err| format!("cannot read {}: {err}", manifest_path.display()))?;
    let value = json_mini::parse(&manifest_json)
        .map_err(|err| format!("cannot parse {}: {err}", manifest_path.display()))?;

    let mut manifest_assets: Vec<ManifestAsset> = Vec::new();

    let linker = value.get("linker").ok_or("manifest is missing 'linker'")?;
    manifest_assets.push(read_manifest_asset(linker)?);

    let assets = value
        .get("assets")
        .and_then(json_mini::Value::as_array)
        .ok_or("manifest is missing 'assets' array")?;
    for asset in assets {
        manifest_assets.push(read_manifest_asset(asset)?);
    }

    // Verify sha256 of every staged file against the manifest *at build
    // time* (build.rs contract §5.2): a corrupt or incomplete stage never
    // gets embedded.
    let mut rendered_consts = String::new();
    let mut rendered_entries = String::new();
    for (index, asset) in manifest_assets.iter().enumerate() {
        let staged_path = dir.join(&asset.install_subpath);
        if !staged_path.is_file() {
            return Err(format!("missing staged file {}", staged_path.display()));
        }
        let bytes = std::fs::read(&staged_path)
            .map_err(|err| format!("cannot read {}: {err}", staged_path.display()))?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual = hex_encode(&hasher.finalize());
        if !actual.eq_ignore_ascii_case(&asset.sha256) {
            return Err(format!(
                "sha256 mismatch for {}: manifest has {}, staged file is {actual}",
                staged_path.display(),
                asset.sha256
            ));
        }
        println!("cargo:rerun-if-changed={}", staged_path.display());

        let absolute_path = staged_path.canonicalize().unwrap_or(staged_path.clone());
        let const_name = format!("EMBEDDED_ASSET_BYTES_{index}");
        rendered_consts.push_str(&format!(
            "static {const_name}: &[u8] = include_bytes!({:?});\n",
            absolute_path.display().to_string()
        ));

        let lib_expr = match &asset.lib {
            Some(lib) => format!("Some({lib:?})"),
            None => "None".to_string(),
        };
        rendered_entries.push_str(&format!(
            "    EmbeddedAsset {{ role: {:?}, name: {:?}, lib: {lib_expr}, \
             install_subpath: {:?}, sha256: {:?}, len: {}, bytes: {const_name} }},\n",
            asset.role,
            asset.name,
            asset.install_subpath,
            asset.sha256,
            bytes.len(),
        ));
    }

    let manifest_literal = format!("{manifest_json:?}");

    Ok(format!(
        "#[derive(Debug, Clone, Copy)]\n\
         pub struct EmbeddedAsset {{\n\
         \x20   pub role: &'static str,\n\
         \x20   pub name: &'static str,\n\
         \x20   pub lib: Option<&'static str>,\n\
         \x20   pub install_subpath: &'static str,\n\
         \x20   pub sha256: &'static str,\n\
         \x20   pub len: usize,\n\
         \x20   pub bytes: &'static [u8],\n\
         }}\n\
         \n\
         {rendered_consts}\n\
         pub const EMBEDDED_ASSETS_PRESENT: bool = true;\n\
         pub static EMBEDDED_ASSETS: &[EmbeddedAsset] = &[\n{rendered_entries}];\n\
         pub static EMBEDDED_ASSET_MANIFEST_JSON: &str = {manifest_literal};\n"
    ))
}

fn read_manifest_asset(value: &json_mini::Value) -> Result<ManifestAsset, String> {
    Ok(ManifestAsset {
        role: expect_str(value, "role")?.to_string(),
        name: expect_str(value, "name")?.to_string(),
        lib: value
            .get("lib")
            .and_then(json_mini::Value::as_str)
            .map(str::to_string),
        install_subpath: expect_str(value, "install_subpath")?.to_string(),
        sha256: expect_str(value, "sha256")?.to_string(),
    })
}

fn expect_str<'a>(value: &'a json_mini::Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(json_mini::Value::as_str)
        .ok_or_else(|| format!("expected string field '{key}'"))
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

/// A deliberately minimal JSON parser (object/array/string/number/bool/null)
/// sufficient for reading `native-link-assets.json` without adding a new
/// `[build-dependencies]` crate. Not a general-purpose JSON library.
mod json_mini {
    use std::iter::Peekable;
    use std::str::CharIndices;

    #[derive(Debug, Clone)]
    pub enum Value {
        Object(Vec<(String, Value)>),
        Array(Vec<Value>),
        String(String),
        Number,
        Bool,
        Null,
    }

    impl Value {
        pub fn get(&self, key: &str) -> Option<&Value> {
            match self {
                Value::Object(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
                _ => None,
            }
        }

        pub fn as_str(&self) -> Option<&str> {
            match self {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            }
        }

        pub fn as_array(&self) -> Option<&[Value]> {
            match self {
                Value::Array(items) => Some(items.as_slice()),
                _ => None,
            }
        }
    }

    type Chars<'a> = Peekable<CharIndices<'a>>;

    pub fn parse(input: &str) -> Result<Value, String> {
        let mut chars = input.char_indices().peekable();
        let value = parse_value(input, &mut chars)?;
        Ok(value)
    }

    fn skip_ws(chars: &mut Chars) {
        while let Some(&(_, c)) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
    }

    fn parse_value(input: &str, chars: &mut Chars) -> Result<Value, String> {
        skip_ws(chars);
        match chars.peek().copied() {
            Some((_, '{')) => parse_object(input, chars),
            Some((_, '[')) => parse_array(input, chars),
            Some((_, '"')) => parse_string(chars).map(Value::String),
            Some((_, 't')) | Some((_, 'f')) => parse_bool(input, chars),
            Some((_, 'n')) => parse_null(input, chars),
            Some((_, c)) if c == '-' || c.is_ascii_digit() => parse_number(input, chars),
            other => Err(format!("unexpected token at {other:?}")),
        }
    }

    fn expect_char(chars: &mut Chars, expected: char) -> Result<(), String> {
        match chars.next() {
            Some((_, c)) if c == expected => Ok(()),
            other => Err(format!("expected '{expected}', got {other:?}")),
        }
    }

    fn parse_object(input: &str, chars: &mut Chars) -> Result<Value, String> {
        expect_char(chars, '{')?;
        let mut pairs = Vec::new();
        skip_ws(chars);
        if let Some(&(_, '}')) = chars.peek() {
            chars.next();
            return Ok(Value::Object(pairs));
        }
        loop {
            skip_ws(chars);
            let key = parse_string(chars)?;
            skip_ws(chars);
            expect_char(chars, ':')?;
            let value = parse_value(input, chars)?;
            pairs.push((key, value));
            skip_ws(chars);
            match chars.next() {
                Some((_, ',')) => continue,
                Some((_, '}')) => break,
                other => return Err(format!("expected ',' or '}}', got {other:?}")),
            }
        }
        Ok(Value::Object(pairs))
    }

    fn parse_array(input: &str, chars: &mut Chars) -> Result<Value, String> {
        expect_char(chars, '[')?;
        let mut items = Vec::new();
        skip_ws(chars);
        if let Some(&(_, ']')) = chars.peek() {
            chars.next();
            return Ok(Value::Array(items));
        }
        loop {
            let value = parse_value(input, chars)?;
            items.push(value);
            skip_ws(chars);
            match chars.next() {
                Some((_, ',')) => continue,
                Some((_, ']')) => break,
                other => return Err(format!("expected ',' or ']', got {other:?}")),
            }
        }
        Ok(Value::Array(items))
    }

    fn parse_string(chars: &mut Chars) -> Result<String, String> {
        skip_ws(chars);
        expect_char(chars, '"')?;
        let mut out = String::new();
        loop {
            match chars.next() {
                Some((_, '"')) => break,
                Some((_, '\\')) => match chars.next() {
                    Some((_, '"')) => out.push('"'),
                    Some((_, '\\')) => out.push('\\'),
                    Some((_, '/')) => out.push('/'),
                    Some((_, 'n')) => out.push('\n'),
                    Some((_, 't')) => out.push('\t'),
                    Some((_, 'r')) => out.push('\r'),
                    Some((_, 'u')) => {
                        let mut code = 0u32;
                        for _ in 0..4 {
                            let (_, c) = chars.next().ok_or("truncated unicode escape")?;
                            code =
                                code * 16 + c.to_digit(16).ok_or("invalid unicode escape digit")?;
                        }
                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                    other => return Err(format!("invalid escape: {other:?}")),
                },
                Some((_, c)) => out.push(c),
                None => return Err("unterminated string".to_string()),
            }
        }
        Ok(out)
    }

    fn literal_matches(input: &str, chars: &mut Chars, literal: &str) -> bool {
        match chars.peek() {
            Some(&(idx, _)) => input[idx..].starts_with(literal),
            None => false,
        }
    }

    fn consume(chars: &mut Chars, count: usize) {
        for _ in 0..count {
            chars.next();
        }
    }

    fn parse_bool(input: &str, chars: &mut Chars) -> Result<Value, String> {
        if literal_matches(input, chars, "true") {
            consume(chars, 4);
            Ok(Value::Bool)
        } else if literal_matches(input, chars, "false") {
            consume(chars, 5);
            Ok(Value::Bool)
        } else {
            Err("invalid literal (expected true/false)".to_string())
        }
    }

    fn parse_null(input: &str, chars: &mut Chars) -> Result<Value, String> {
        if literal_matches(input, chars, "null") {
            consume(chars, 4);
            Ok(Value::Null)
        } else {
            Err("invalid literal (expected null)".to_string())
        }
    }

    fn parse_number(input: &str, chars: &mut Chars) -> Result<Value, String> {
        let start = chars.peek().map(|&(i, _)| i).unwrap_or(0);
        if let Some(&(_, '-')) = chars.peek() {
            chars.next();
        }
        while let Some(&(_, c)) = chars.peek() {
            if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-' {
                chars.next();
            } else {
                break;
            }
        }
        let end = chars.peek().map(|&(i, _)| i).unwrap_or(input.len());
        input[start..end]
            .parse::<f64>()
            .map(|_| Value::Number)
            .map_err(|err| format!("invalid number: {err}"))
    }
}
