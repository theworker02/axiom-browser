//! System font discovery, CSS family matching and fallback chains.
//!
//! Fonts are found by file name in the platform font directories (no font file is read
//! until a glyph needs it). Each CSS family name maps to one of a small set of families
//! with metric-compatible files on Windows, Linux and macOS; unknown names fall through
//! to the next name in the `font-family` list and finally to `sans-serif`. Characters no
//! family covers are looked up in a per-script fallback list (CJK, Arabic, Indic, Thai,
//! symbols, emoji outlines).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

pub(crate) struct LoadedFont {
    pub shaper: rustybuzz::Face<'static>,
    pub raster: fontdue::Font,
    pub units_per_em: f32,
    /// Ascent, descent (positive, below the baseline) and line gap, in em.
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    /// Height of a lowercase `x` and advance of `0`, in em (CSS `ex` and `ch`).
    pub x_height: f32,
    pub ch_width: f32,
}

struct Entry {
    path: PathBuf,
    font: OnceLock<Option<Arc<LoadedFont>>>,
}

#[derive(Default, Clone, Copy)]
struct FaceSet {
    regular: Option<usize>,
    bold: Option<usize>,
    italic: Option<usize>,
    bold_italic: Option<usize>,
}

/// One font of a resolved chain; `embolden` asks for synthetic bold because the family
/// has no bold face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ChainFont {
    pub entry: u16,
    pub embolden: bool,
}

type ChainKey = (String, bool, bool);

pub(crate) struct Registry {
    entries: Vec<Entry>,
    families: Vec<FaceSet>,
    names: HashMap<&'static str, usize>,
    sans: usize,
    fallbacks: Vec<usize>,
    chains: Mutex<HashMap<ChainKey, Arc<[ChainFont]>>>,
    web: RwLock<WebFonts>,
}

struct FamilyDef {
    names: &'static [&'static str],
    regular: &'static [&'static str],
    bold: &'static [&'static str],
    italic: &'static [&'static str],
    bold_italic: &'static [&'static str],
}

/// The first family is the default (`sans-serif`).
const FAMILIES: &[FamilyDef] = &[
    FamilyDef {
        names: &[
            "sans-serif",
            "system-ui",
            "-apple-system",
            "blinkmacsystemfont",
            "ui-sans-serif",
            "-webkit-system-font",
            "segoe ui",
            "segoe ui variable",
            "segoe ui variable text",
            "segoe ui variable display",
            "roboto",
            "inter",
            "noto sans",
            "open sans",
            "helvetica neue",
            "lato",
            "source sans pro",
            "source sans 3",
            "ubuntu",
            "cantarell",
            "fira sans",
            "oxygen",
            "oxygen-sans",
            "droid sans",
            "sf pro text",
            "sf pro display",
            "dejavu sans",
            "cursive",
            "fantasy",
            "ui-rounded",
            "emoji",
            "math",
        ],
        regular: &[
            "segoeui.ttf",
            "DejaVuSans.ttf",
            "NotoSans-Regular.ttf",
            "LiberationSans-Regular.ttf",
            "Arial.ttf",
            "Helvetica.ttc",
        ],
        bold: &[
            "segoeuib.ttf",
            "DejaVuSans-Bold.ttf",
            "NotoSans-Bold.ttf",
            "LiberationSans-Bold.ttf",
            "Arial Bold.ttf",
        ],
        italic: &[
            "segoeuii.ttf",
            "DejaVuSans-Oblique.ttf",
            "NotoSans-Italic.ttf",
            "LiberationSans-Italic.ttf",
            "Arial Italic.ttf",
        ],
        bold_italic: &[
            "segoeuiz.ttf",
            "DejaVuSans-BoldOblique.ttf",
            "NotoSans-BoldItalic.ttf",
            "LiberationSans-BoldItalic.ttf",
            "Arial Bold Italic.ttf",
        ],
    },
    FamilyDef {
        names: &[
            "arial",
            "helvetica",
            "arimo",
            "liberation sans",
            "nimbus sans",
            "nimbus sans l",
            "arial nova",
        ],
        regular: &[
            "arial.ttf",
            "LiberationSans-Regular.ttf",
            "Arimo-Regular.ttf",
            "Arial.ttf",
        ],
        bold: &[
            "arialbd.ttf",
            "LiberationSans-Bold.ttf",
            "Arimo-Bold.ttf",
            "Arial Bold.ttf",
        ],
        italic: &[
            "ariali.ttf",
            "LiberationSans-Italic.ttf",
            "Arimo-Italic.ttf",
            "Arial Italic.ttf",
        ],
        bold_italic: &[
            "arialbi.ttf",
            "LiberationSans-BoldItalic.ttf",
            "Arimo-BoldItalic.ttf",
            "Arial Bold Italic.ttf",
        ],
    },
    FamilyDef {
        names: &[
            "serif",
            "ui-serif",
            "times new roman",
            "times",
            "liberation serif",
            "tinos",
            "nimbus roman",
            "linux libertine",
            "dejavu serif",
            "noto serif",
        ],
        regular: &[
            "times.ttf",
            "LiberationSerif-Regular.ttf",
            "DejaVuSerif.ttf",
            "NotoSerif-Regular.ttf",
            "Times New Roman.ttf",
            "Times.ttc",
        ],
        bold: &[
            "timesbd.ttf",
            "LiberationSerif-Bold.ttf",
            "DejaVuSerif-Bold.ttf",
            "NotoSerif-Bold.ttf",
            "Times New Roman Bold.ttf",
        ],
        italic: &[
            "timesi.ttf",
            "LiberationSerif-Italic.ttf",
            "DejaVuSerif-Italic.ttf",
            "NotoSerif-Italic.ttf",
            "Times New Roman Italic.ttf",
        ],
        bold_italic: &[
            "timesbi.ttf",
            "LiberationSerif-BoldItalic.ttf",
            "DejaVuSerif-BoldItalic.ttf",
            "NotoSerif-BoldItalic.ttf",
        ],
    },
    FamilyDef {
        names: &["georgia", "gelasio"],
        regular: &["georgia.ttf", "Georgia.ttf", "Gelasio-Regular.ttf"],
        bold: &["georgiab.ttf", "Georgia Bold.ttf", "Gelasio-Bold.ttf"],
        italic: &["georgiai.ttf", "Georgia Italic.ttf", "Gelasio-Italic.ttf"],
        bold_italic: &["georgiaz.ttf", "Georgia Bold Italic.ttf"],
    },
    FamilyDef {
        names: &[
            "monospace",
            "ui-monospace",
            "consolas",
            "menlo",
            "monaco",
            "sf mono",
            "sfmono-regular",
            "cascadia code",
            "cascadia mono",
            "source code pro",
            "dejavu sans mono",
            "liberation mono",
            "roboto mono",
            "fira code",
            "fira mono",
            "jetbrains mono",
            "ubuntu mono",
            "noto sans mono",
            "lucida console",
            "andale mono",
            "droid sans mono",
        ],
        regular: &[
            "consola.ttf",
            "DejaVuSansMono.ttf",
            "LiberationMono-Regular.ttf",
            "NotoSansMono-Regular.ttf",
            "Menlo.ttc",
            "cour.ttf",
        ],
        bold: &[
            "consolab.ttf",
            "DejaVuSansMono-Bold.ttf",
            "LiberationMono-Bold.ttf",
            "NotoSansMono-Bold.ttf",
            "courbd.ttf",
        ],
        italic: &[
            "consolai.ttf",
            "DejaVuSansMono-Oblique.ttf",
            "LiberationMono-Italic.ttf",
            "couri.ttf",
        ],
        bold_italic: &[
            "consolaz.ttf",
            "DejaVuSansMono-BoldOblique.ttf",
            "LiberationMono-BoldItalic.ttf",
            "courbi.ttf",
        ],
    },
    FamilyDef {
        names: &["courier new", "courier", "cousine"],
        regular: &[
            "cour.ttf",
            "LiberationMono-Regular.ttf",
            "Cousine-Regular.ttf",
            "Courier New.ttf",
        ],
        bold: &[
            "courbd.ttf",
            "LiberationMono-Bold.ttf",
            "Courier New Bold.ttf",
        ],
        italic: &["couri.ttf", "LiberationMono-Italic.ttf"],
        bold_italic: &["courbi.ttf", "LiberationMono-BoldItalic.ttf"],
    },
    FamilyDef {
        names: &["verdana"],
        regular: &["verdana.ttf", "Verdana.ttf"],
        bold: &["verdanab.ttf", "Verdana Bold.ttf"],
        italic: &["verdanai.ttf", "Verdana Italic.ttf"],
        bold_italic: &["verdanaz.ttf"],
    },
    FamilyDef {
        names: &["tahoma"],
        regular: &["tahoma.ttf", "Tahoma.ttf"],
        bold: &["tahomabd.ttf", "Tahoma Bold.ttf"],
        italic: &[],
        bold_italic: &[],
    },
    FamilyDef {
        names: &["trebuchet ms"],
        regular: &["trebuc.ttf", "Trebuchet MS.ttf"],
        bold: &["trebucbd.ttf"],
        italic: &["trebucit.ttf"],
        bold_italic: &["trebucbi.ttf"],
    },
    FamilyDef {
        names: &["calibri", "carlito"],
        regular: &["calibri.ttf", "Carlito-Regular.ttf"],
        bold: &["calibrib.ttf", "Carlito-Bold.ttf"],
        italic: &["calibrii.ttf", "Carlito-Italic.ttf"],
        bold_italic: &["calibriz.ttf", "Carlito-BoldItalic.ttf"],
    },
    FamilyDef {
        names: &["cambria", "caladea"],
        regular: &["cambria.ttc", "Caladea-Regular.ttf"],
        bold: &["cambriab.ttf", "Caladea-Bold.ttf"],
        italic: &["cambriai.ttf", "Caladea-Italic.ttf"],
        bold_italic: &["cambriaz.ttf", "Caladea-BoldItalic.ttf"],
    },
];

/// Fonts tried, in order, for characters the requested families lack.
const FALLBACKS: &[&str] = &[
    // Windows
    "seguisym.ttf",
    "msyh.ttc",
    "YuGothM.ttc",
    "malgun.ttf",
    "Nirmala.ttc",
    "Nirmala.ttf",
    "LeelawUI.ttf",
    "ebrima.ttf",
    "gadugi.ttf",
    "mmrtext.ttf",
    "himalaya.ttf",
    "seguihis.ttf",
    "seguiemj.ttf",
    "micross.ttf",
    "arial.ttf",
    "simsun.ttc",
    "l_10646.ttf",
    // Linux
    "NotoSansCJK-Regular.ttc",
    "NotoSansCJKsc-Regular.otf",
    "NotoSansCJKjp-Regular.otf",
    "wqy-microhei.ttc",
    "wqy-zenhei.ttc",
    "NotoSansArabic-Regular.ttf",
    "NotoSansHebrew-Regular.ttf",
    "NotoSansDevanagari-Regular.ttf",
    "NotoSansThai-Regular.ttf",
    "NotoSansSymbols-Regular.ttf",
    "NotoSansSymbols2-Regular.ttf",
    "DejaVuSans.ttf",
    "FreeSans.ttf",
    // macOS
    "PingFang.ttc",
    "Hiragino Sans GB.ttc",
    "AppleSDGothicNeo.ttc",
    "GeezaPro.ttc",
    "Thonburi.ttc",
    "Apple Symbols.ttf",
    "Arial Unicode.ttf",
];

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(extra) = std::env::var("AXIOM_FONT_DIR") {
        dirs.extend(std::env::split_paths(&extra));
    }
    if cfg!(windows) {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        dirs.push(Path::new(&windir).join("Fonts"));
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(Path::new(&local).join("Microsoft\\Windows\\Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.push("/System/Library/Fonts".into());
        dirs.push("/System/Library/Fonts/Supplemental".into());
        dirs.push("/Library/Fonts".into());
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(Path::new(&home).join("Library/Fonts"));
        }
    } else {
        dirs.push("/usr/share/fonts".into());
        dirs.push("/usr/local/share/fonts".into());
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(Path::new(&home).join(".fonts"));
            dirs.push(Path::new(&home).join(".local/share/fonts"));
        }
    }
    dirs
}

/// Lowercase file name → path for every font file under the font directories.
fn index_font_files() -> HashMap<String, PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut HashMap<String, PathBuf>) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < 5 {
                    walk(&path, depth + 1, out);
                }
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let lower = name.to_ascii_lowercase();
            if [".ttf", ".otf", ".ttc", ".otc"]
                .iter()
                .any(|ext| lower.ends_with(ext))
            {
                out.entry(lower).or_insert(path);
            }
        }
    }
    let mut out = HashMap::new();
    for dir in font_dirs() {
        walk(&dir, 0, &mut out);
    }
    out
}

impl Registry {
    pub(crate) fn global() -> &'static Registry {
        static REGISTRY: OnceLock<Registry> = OnceLock::new();
        REGISTRY.get_or_init(Registry::discover)
    }

    fn discover() -> Registry {
        let files = index_font_files();
        let mut entries: Vec<Entry> = Vec::new();
        let mut by_path: HashMap<PathBuf, usize> = HashMap::new();
        let mut entry_for = |candidates: &[&str]| -> Option<usize> {
            let path = candidates
                .iter()
                .find_map(|c| files.get(&c.to_ascii_lowercase()))?;
            Some(*by_path.entry(path.clone()).or_insert_with(|| {
                entries.push(Entry {
                    path: path.clone(),
                    font: OnceLock::new(),
                });
                entries.len() - 1
            }))
        };
        let mut families = Vec::new();
        let mut names = HashMap::new();
        for (i, def) in FAMILIES.iter().enumerate() {
            families.push(FaceSet {
                regular: entry_for(def.regular),
                bold: entry_for(def.bold),
                italic: entry_for(def.italic),
                bold_italic: entry_for(def.bold_italic),
            });
            for &name in def.names {
                names.insert(name, i);
            }
        }
        let fallbacks: Vec<usize> = FALLBACKS
            .iter()
            .filter_map(|f| entry_for(std::slice::from_ref(f)))
            .collect();
        if families[0].regular.is_none() {
            log::warn!(
                target: "axiom_text",
                "no sans-serif system font found; text uses the first available fallback"
            );
        }
        Registry {
            entries,
            families,
            names,
            sans: 0,
            fallbacks,
            chains: Mutex::new(HashMap::new()),
            web: RwLock::new(WebFonts::default()),
        }
    }

    pub(crate) fn font(&self, entry: u16) -> Option<Arc<LoadedFont>> {
        if entry as usize >= WEB_BASE {
            return self
                .web
                .read()
                .unwrap()
                .fonts
                .get(entry as usize - WEB_BASE)
                .cloned();
        }
        let e = self.entries.get(entry as usize)?;
        e.font
            .get_or_init(|| {
                let font = load_font(&e.path);
                if font.is_none() {
                    log::warn!(target: "axiom_text", "could not load font {}", e.path.display());
                }
                font.map(Arc::new)
            })
            .clone()
    }

    fn pick(&self, family: usize, bold: bool, italic: bool) -> Option<ChainFont> {
        Self::pick_in(self.families[family], bold, italic)
    }

    fn pick_in(set: FaceSet, bold: bool, italic: bool) -> Option<ChainFont> {
        let exact = match (bold, italic) {
            (true, true) => set.bold_italic.or(set.bold),
            (true, false) => set.bold,
            (false, true) => set.italic,
            (false, false) => set.regular,
        };
        if let Some(e) = exact {
            return Some(ChainFont {
                entry: e as u16,
                embolden: false,
            });
        }
        let base = if italic {
            set.italic.or(set.regular)
        } else {
            set.regular
        };
        base.map(|e| ChainFont {
            entry: e as u16,
            embolden: bold,
        })
    }

    /// Fonts to try for `families` (a CSS `font-family` value), best first.
    pub(crate) fn chain(&self, families: &str, bold: bool, italic: bool) -> Arc<[ChainFont]> {
        let key = (families.to_string(), bold, italic);
        if let Some(c) = self.chains.lock().unwrap().get(&key) {
            return c.clone();
        }
        let mut chain: Vec<ChainFont> = Vec::new();
        let push = |f: Option<ChainFont>, chain: &mut Vec<ChainFont>| {
            if let Some(f) = f {
                if !chain.iter().any(|c| c.entry == f.entry) {
                    chain.push(f);
                }
            }
        };
        let web = self.web.read().unwrap();
        for name in parse_family_list(families) {
            if let Some(&set) = web.families.get(&name) {
                push(Self::pick_in(set, bold, italic), &mut chain);
            } else if let Some(&fam) = self.names.get(name.as_str()) {
                push(self.pick(fam, bold, italic), &mut chain);
            }
        }
        drop(web);
        push(self.pick(self.sans, bold, italic), &mut chain);
        for &e in &self.fallbacks {
            push(
                Some(ChainFont {
                    entry: e as u16,
                    embolden: bold,
                }),
                &mut chain,
            );
        }
        let chain: Arc<[ChainFont]> = chain.into();
        self.chains.lock().unwrap().insert(key, chain.clone());
        chain
    }
}

/// Lowercase family names of a CSS `font-family` value, quotes removed.
pub fn parse_family_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|f| {
            f.trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|f| !f.is_empty())
        .collect()
}

fn load_font(path: &Path) -> Option<LoadedFont> {
    let bytes = std::fs::read(path).ok()?;
    // Loaded fonts live for the rest of the process (the registry is global and
    // each file is read at most once).
    load_font_data(Box::leak(bytes.into_boxed_slice()))
}

fn load_font_data(data: &'static [u8]) -> Option<LoadedFont> {
    let shaper = rustybuzz::Face::from_slice(data, 0)?;
    let raster = fontdue::Font::from_bytes(
        data,
        fontdue::FontSettings {
            collection_index: 0,
            scale: 40.0,
            load_substitutions: false,
        },
    )
    .ok()?;
    let upem = shaper.units_per_em() as f32;
    let ascent = shaper.ascender() as f32 / upem;
    let descent = -(shaper.descender() as f32) / upem;
    let line_gap = shaper.line_gap() as f32 / upem;
    let x_height = shaper
        .x_height()
        .filter(|h| *h > 0)
        .map(|h| h as f32)
        .or_else(|| {
            let g = shaper.glyph_index('x')?;
            Some(shaper.glyph_bounding_box(g)?.y_max as f32)
        })
        .map_or(0.5, |h| h / upem);
    let ch_width = shaper
        .glyph_index('0')
        .and_then(|g| shaper.glyph_hor_advance(g))
        .map_or(0.5, |a| a as f32 / upem);
    Some(LoadedFont {
        shaper,
        raster,
        units_per_em: upem,
        ascent,
        descent,
        line_gap,
        x_height,
        ch_width,
    })
}

/// Largest web font accepted after decompression.
const MAX_WEB_FONT_BYTES: usize = 32 << 20;

/// sfnt bytes of a TrueType/OpenType font or a WOFF 1.0 / 2.0 wrapper around one.
fn sfnt_data(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    match bytes.get(..4) {
        Some(b"wOFF") => decode_woff(&bytes),
        Some(b"wOF2") => {
            let sfnt = wuff::decompress_woff2(&bytes).map_err(|_| "corrupt WOFF2 font")?;
            if sfnt.len() > MAX_WEB_FONT_BYTES {
                return Err("WOFF2 font too large".into());
            }
            Ok(sfnt)
        }
        Some([0, 1, 0, 0] | b"OTTO" | b"true") => Ok(bytes),
        _ => Err("not a TrueType, OpenType or WOFF font".into()),
    }
}

/// Unwraps a WOFF 1.0 file (W3C WOFF §3–5) into the sfnt it contains.
fn decode_woff(data: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let u16_at = |o: usize| -> Result<u16, String> {
        data.get(o..o + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .ok_or_else(|| "truncated WOFF header".to_string())
    };
    let u32_at = |o: usize| -> Result<u32, String> {
        data.get(o..o + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| "truncated WOFF header".to_string())
    };
    let flavor = u32_at(4)?;
    let num_tables = u16_at(12)? as usize;
    let total = u32_at(16)? as usize;
    if num_tables == 0 || total > MAX_WEB_FONT_BYTES {
        return Err("invalid WOFF table directory".into());
    }
    struct Table {
        tag: u32,
        checksum: u32,
        data: Vec<u8>,
    }
    let mut tables = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let d = 44 + i * 20;
        let (tag, offset, comp, orig, checksum) = (
            u32_at(d)?,
            u32_at(d + 4)? as usize,
            u32_at(d + 8)? as usize,
            u32_at(d + 12)? as usize,
            u32_at(d + 16)?,
        );
        let raw = data
            .get(offset..offset.saturating_add(comp))
            .ok_or("WOFF table outside the file")?;
        let bytes = if comp < orig {
            let mut out = Vec::with_capacity(orig);
            flate2::read::ZlibDecoder::new(raw)
                .take(orig as u64)
                .read_to_end(&mut out)
                .map_err(|_| "corrupt WOFF table")?;
            if out.len() != orig {
                return Err("WOFF table has the wrong length".into());
            }
            out
        } else {
            raw.to_vec()
        };
        tables.push(Table {
            tag,
            checksum,
            data: bytes,
        });
    }
    tables.sort_by_key(|t| t.tag);
    let n = tables.len() as u16;
    let entry_selector = 15 - n.leading_zeros() as u16;
    let search_range = (1u16 << entry_selector) * 16;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&(n * 16 - search_range).to_be_bytes());
    let mut offset = 12 + 16 * tables.len();
    for t in &tables {
        out.extend_from_slice(&t.tag.to_be_bytes());
        out.extend_from_slice(&t.checksum.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(t.data.len() as u32).to_be_bytes());
        offset += t.data.len().div_ceil(4) * 4;
    }
    for t in &tables {
        out.extend_from_slice(&t.data);
        out.resize(out.len().div_ceil(4) * 4, 0);
    }
    if out.len() > MAX_WEB_FONT_BYTES {
        return Err("WOFF font too large".into());
    }
    Ok(out)
}

/// Offset of web font entry ids (system font entries count up from 0).
const WEB_BASE: usize = 0x8000;

/// Fonts registered at run time from `@font-face` rules, by document-scoped family key.
#[derive(Default)]
struct WebFonts {
    fonts: Vec<Arc<LoadedFont>>,
    by_hash: HashMap<u64, usize>,
    families: HashMap<String, FaceSet>,
}

impl Registry {
    /// Registers `bytes` as a face of the family `key` (lowercase). Identical font
    /// files share one loaded font.
    pub(crate) fn register_web_font(
        &self,
        key: &str,
        bytes: Vec<u8>,
        bold: bool,
        italic: bool,
    ) -> Result<(), String> {
        use std::hash::{Hash, Hasher};
        let sfnt = sfnt_data(bytes)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        sfnt.hash(&mut hasher);
        let hash = hasher.finish();
        let mut web = self.web.write().unwrap();
        let index = match web.by_hash.get(&hash) {
            Some(&i) => i,
            None => {
                if WEB_BASE + web.fonts.len() >= u16::MAX as usize {
                    return Err("too many web fonts".into());
                }
                let font = load_font_data(Box::leak(sfnt.into_boxed_slice()))
                    .ok_or("the font could not be parsed")?;
                let index = web.fonts.len();
                web.fonts.push(Arc::new(font));
                web.by_hash.insert(hash, index);
                index
            }
        };
        let set = web.families.entry(key.to_string()).or_default();
        let slot = match (bold, italic) {
            (false, false) => &mut set.regular,
            (true, false) => &mut set.bold,
            (false, true) => &mut set.italic,
            (true, true) => &mut set.bold_italic,
        };
        *slot = Some(WEB_BASE + index);
        drop(web);
        self.chains.lock().unwrap().clear();
        Ok(())
    }
}
