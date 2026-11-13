//! Reading color schemes from other terminals: base16 (YAML or JSON),
//! iTerm2 `.itermcolors`, Windows Terminal schemes (one, a list, or a whole
//! `settings.json`), and goatty's own theme files. The format is detected
//! from the content.

use serde_json::Value;

use crate::theme::{Color, ThemeColors};
use crate::themes::ThemeFile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Goatty,
    Base16,
    ITerm,
    WindowsTerminal,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Goatty => "goatty theme",
            Format::Base16 => "base16 scheme",
            Format::ITerm => "iTerm2 color preset",
            Format::WindowsTerminal => "Windows Terminal scheme",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub name: String,
    pub colors: ThemeColors,
    pub format: Format,
}

/// Parse `text`. `fallback_name` (usually the file stem) names schemes that
/// don't carry a name. A file can hold several schemes (Windows Terminal).
pub fn import(text: &str, fallback_name: &str) -> Result<Vec<Imported>, String> {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with('<') {
        return iterm(t, fallback_name).map(|i| vec![i]);
    }
    if t.starts_with('{') || t.starts_with('[') {
        let v: Value = serde_json::from_str(t).map_err(|e| format!("not valid JSON: {e}"))?;
        return json(&v, fallback_name);
    }
    base16_yaml(t, fallback_name).map(|i| vec![i])
}

fn json(v: &Value, fallback: &str) -> Result<Vec<Imported>, String> {
    match v {
        Value::Array(items) => {
            let out: Vec<Imported> = items.iter().filter_map(|i| windows_terminal(i, fallback).ok()).collect();
            if out.is_empty() { Err("no Windows Terminal schemes in the list".into()) } else { Ok(out) }
        }
        Value::Object(o) if o.contains_key("colors") && o.contains_key("name") => {
            let f: ThemeFile = serde_json::from_value(v.clone()).map_err(|e| format!("not a goatty theme: {e}"))?;
            Ok(vec![Imported { name: f.name, colors: f.colors, format: Format::Goatty }])
        }
        Value::Object(o) if o.contains_key("schemes") => json(&o["schemes"], fallback),
        Value::Object(o) if o.contains_key("base00") || o.get("palette").is_some_and(|p| p.get("base00").is_some()) => {
            let palette = o.get("palette").unwrap_or(v);
            let get = |k: &str| palette.get(k).and_then(Value::as_str).and_then(Color::parse);
            let name = o.get("name").or_else(|| o.get("scheme")).and_then(Value::as_str).unwrap_or(fallback);
            base16(name, get).map(|i| vec![i])
        }
        Value::Object(_) => windows_terminal(v, fallback).map(|i| vec![i]),
        _ => Err("unrecognized JSON".into()),
    }
}

fn windows_terminal(v: &Value, fallback: &str) -> Result<Imported, String> {
    // `#RRGGBBAA` is allowed; the alpha is ignored.
    let get = |k: &str| v.get(k).and_then(Value::as_str).and_then(|s| Color::parse(s.get(..7).unwrap_or(s)));
    const NAMES: [&str; 16] = [
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "purple",
        "cyan",
        "white",
        "brightBlack",
        "brightRed",
        "brightGreen",
        "brightYellow",
        "brightBlue",
        "brightPurple",
        "brightCyan",
        "brightWhite",
    ];
    let mut ansi = [Color(0); 16];
    for (slot, n) in ansi.iter_mut().zip(NAMES) {
        *slot = get(n).ok_or_else(|| format!("Windows Terminal scheme without {n:?}"))?;
    }
    let foreground = get("foreground").ok_or("scheme without \"foreground\"")?;
    let background = get("background").ok_or("scheme without \"background\"")?;
    Ok(Imported {
        name: v.get("name").and_then(Value::as_str).unwrap_or(fallback).into(),
        colors: ThemeColors {
            ansi,
            foreground,
            background,
            cursor: get("cursorColor").unwrap_or(foreground),
            selection_foreground: foreground,
            selection_background: get("selectionBackground").unwrap_or(ansi[8]),
            accent: None,
        },
        format: Format::WindowsTerminal,
    })
}

/// base16 → terminal colors, the mapping base16-shell uses.
fn base16(name: &str, get: impl Fn(&str) -> Option<Color>) -> Result<Imported, String> {
    let b = |k: &str| get(k).ok_or_else(|| format!("base16 scheme without {k}"));
    let ansi = [
        b("base00")?,
        b("base08")?,
        b("base0B")?,
        b("base0A")?,
        b("base0D")?,
        b("base0E")?,
        b("base0C")?,
        b("base05")?,
        b("base03")?,
        b("base08")?,
        b("base0B")?,
        b("base0A")?,
        b("base0D")?,
        b("base0E")?,
        b("base0C")?,
        b("base07")?,
    ];
    Ok(Imported {
        name: name.into(),
        colors: ThemeColors {
            ansi,
            foreground: b("base05")?,
            background: b("base00")?,
            cursor: b("base05")?,
            selection_foreground: b("base05")?,
            selection_background: b("base02")?,
            accent: None,
        },
        format: Format::Base16,
    })
}

/// base16 YAML (classic `base00: "1d1f21"` or tinted-theming's nested
/// `palette:`): a line-based read is enough for these flat files.
fn base16_yaml(text: &str, fallback: &str) -> Result<Imported, String> {
    let mut name: Option<String> = None;
    let mut keys: Vec<(String, Color)> = Vec::new();
    for line in text.lines() {
        let line = line.split(" #").next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once(':') else { continue };
        let k = k.trim().trim_matches('"');
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
        if (k == "scheme" || k == "name") && name.is_none() && !v.is_empty() {
            name = Some(v.to_string());
        } else if k.len() == 6
            && k.starts_with("base0")
            && let Some(c) = Color::parse(v)
        {
            keys.push((k.to_ascii_lowercase(), c));
        }
    }
    if keys.is_empty() {
        return Err("not a color scheme goatty can read (base16 YAML/JSON, .itermcolors, Windows Terminal, goatty theme)".into());
    }
    let name = name.unwrap_or_else(|| fallback.to_string());
    base16(&name, |k| keys.iter().find(|(kk, _)| *kk == k.to_ascii_lowercase()).map(|(_, c)| *c))
}

/// iTerm2 `.itermcolors` (an XML property list of component dicts).
fn iterm(text: &str, fallback: &str) -> Result<Imported, String> {
    let mut colors: Vec<(String, Color)> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<key>") {
        let after = &rest[start + 5..];
        let Some(end) = after.find("</key>") else { break };
        let key = after[..end].trim().to_string();
        let tail = after[end + 6..].trim_start();
        if let Some(body) = tail.strip_prefix("<dict>") {
            let Some(close) = body.find("</dict>") else { break };
            if let Some(c) = iterm_color(&body[..close]) {
                colors.push((key, c));
            }
            rest = &body[close + 7..];
        } else {
            rest = tail;
        }
    }
    let get = |k: &str| colors.iter().find(|(n, _)| n == k).map(|(_, c)| *c);
    let mut ansi = [Color(0); 16];
    for (i, slot) in ansi.iter_mut().enumerate() {
        *slot = get(&format!("Ansi {i} Color")).ok_or_else(|| format!("iTerm2 preset without \"Ansi {i} Color\""))?;
    }
    let foreground = get("Foreground Color").ok_or("iTerm2 preset without a foreground color")?;
    let background = get("Background Color").ok_or("iTerm2 preset without a background color")?;
    Ok(Imported {
        name: fallback.into(),
        colors: ThemeColors {
            ansi,
            foreground,
            background,
            cursor: get("Cursor Color").unwrap_or(foreground),
            selection_foreground: get("Selected Text Color").unwrap_or(foreground),
            selection_background: get("Selection Color").unwrap_or(ansi[8]),
            accent: None,
        },
        format: Format::ITerm,
    })
}

fn iterm_color(dict: &str) -> Option<Color> {
    let component = |name: &str| -> Option<f64> {
        let k = dict.find(&format!("<key>{name}</key>"))?;
        let after = &dict[k..];
        let open = after.find("<real>").map(|i| (i, 6, "</real>")).or_else(|| after.find("<integer>").map(|i| (i, 9, "</integer>")))?;
        let body = &after[open.0 + open.1..];
        body[..body.find(open.2)?].trim().parse().ok()
    };
    let to = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    Some(Color((to(component("Red Component")?) << 16) | (to(component("Green Component")?) << 8) | to(component("Blue Component")?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE16_YAML: &str = r#"
scheme: "Tomorrow Night"
author: "Chris Kempson"
base00: "1d1f21"
base01: "282a2e"
base02: "373b41"
base03: "969896"
base04: "b4b7b4"
base05: "c5c8c6"
base06: "e0e0e0"
base07: "ffffff"
base08: "cc6666"
base09: "de935f"
base0A: "f0c674"
base0B: "b5bd68"
base0C: "8abeb7"
base0D: "81a2be"
base0E: "b294bb"
base0F: "a3685a"
"#;

    #[test]
    fn base16_yaml_and_json() {
        let t = &import(BASE16_YAML, "file").unwrap()[0];
        assert_eq!(t.name, "Tomorrow Night");
        assert_eq!(t.format, Format::Base16);
        assert_eq!(t.colors.background, Color(0x1d1f21));
        assert_eq!(t.colors.ansi[1], Color(0xcc6666));
        assert_eq!(t.colors.ansi[15], Color(0xffffff));
        assert_eq!(t.colors.selection_background, Color(0x373b41));

        // tinted-theming YAML with a nested palette, and the JSON form.
        let nested = BASE16_YAML.replace("scheme:", "name:").replace("base0", "  base0").replace("author:", "palette:\n  author:");
        assert_eq!(import(&nested, "f").unwrap()[0].colors, t.colors);
        let json = r##"{"scheme": "T", "base00": "#1d1f21", "base01": "282a2e", "base02": "373b41", "base03": "969896", "base04": "b4b7b4",
            "base05": "c5c8c6", "base06": "e0e0e0", "base07": "ffffff", "base08": "cc6666", "base09": "de935f", "base0A": "f0c674",
            "base0B": "b5bd68", "base0C": "8abeb7", "base0D": "81a2be", "base0E": "b294bb", "base0F": "a3685a"}"##;
        assert_eq!(import(json, "f").unwrap()[0].colors, t.colors);
    }

    fn wt_scheme(name: &str) -> String {
        format!(
            r##"{{"name": "{name}", "background": "#0C0C0C", "foreground": "#CCCCCC", "cursorColor": "#FFFFFF", "selectionBackground": "#FFFFFF",
            "black": "#0C0C0C", "red": "#C50F1F", "green": "#13A10E", "yellow": "#C19C00", "blue": "#0037DA", "purple": "#881798",
            "cyan": "#3A96DD", "white": "#CCCCCC", "brightBlack": "#767676", "brightRed": "#E74856", "brightGreen": "#16C60C",
            "brightYellow": "#F9F1A5", "brightBlue": "#3B78FF", "brightPurple": "#B4009E", "brightCyan": "#61D6D6", "brightWhite": "#F2F2F2"}}"##
        )
    }

    #[test]
    fn windows_terminal_single_list_and_settings() {
        let one = &import(&wt_scheme("Campbell"), "f").unwrap()[0];
        assert_eq!(one.name, "Campbell");
        assert_eq!(one.format, Format::WindowsTerminal);
        assert_eq!(one.colors.ansi[5], Color(0x881798));
        assert_eq!(one.colors.cursor, Color(0xffffff));
        let list = format!("[{}, {}]", wt_scheme("A"), wt_scheme("B"));
        assert_eq!(import(&list, "f").unwrap().len(), 2);
        let settings = format!(r#"{{"profiles": {{}}, "schemes": [{}]}}"#, wt_scheme("C"));
        assert_eq!(import(&settings, "f").unwrap()[0].name, "C");
    }

    #[test]
    fn iterm_plist() {
        let mut plist = String::from("<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n");
        let entry = |k: &str, r: f64, g: f64, b: f64| {
            format!(
                "\t<key>{k}</key>\n\t<dict>\n\t\t<key>Alpha Component</key>\n\t\t<real>1</real>\n\t\t<key>Blue Component</key>\n\t\t<real>{b}</real>\n\t\t<key>Color Space</key>\n\t\t<string>sRGB</string>\n\t\t<key>Green Component</key>\n\t\t<real>{g}</real>\n\t\t<key>Red Component</key>\n\t\t<real>{r}</real>\n\t</dict>\n"
            )
        };
        for i in 0..16 {
            plist += &entry(&format!("Ansi {i} Color"), i as f64 / 15.0, 0.0, 1.0);
        }
        plist += &entry("Background Color", 0.0, 0.0, 0.0);
        plist += &entry("Foreground Color", 1.0, 1.0, 1.0);
        plist += &entry("Selection Color", 0.2, 0.2, 0.2);
        plist += "</dict>\n</plist>\n";
        let t = &import(&plist, "Snazzy").unwrap()[0];
        assert_eq!(t.name, "Snazzy");
        assert_eq!(t.format, Format::ITerm);
        assert_eq!(t.colors.ansi[0], Color(0x0000ff));
        assert_eq!(t.colors.ansi[15], Color(0xff00ff));
        assert_eq!(t.colors.background, Color(0));
        assert_eq!(t.colors.foreground, Color(0xffffff));
        assert_eq!(t.colors.selection_background, Color(0x333333));
        assert_eq!(t.colors.cursor, Color(0xffffff), "no cursor color: the foreground");
    }

    #[test]
    fn goatty_round_trip_and_garbage() {
        let mut colors = crate::find_theme("nord").unwrap().colors;
        colors.accent = Some(Color(0xa3be8c));
        let f = ThemeFile { name: "Mine".into(), colors };
        let json = serde_json::to_string(&f).unwrap();
        let t = &import(&json, "x").unwrap()[0];
        assert_eq!((t.name.as_str(), t.format, t.colors), ("Mine", Format::Goatty, f.colors));
        let t_primary = &import(&json.replace("\"accent\"", "\"primary\""), "x").unwrap()[0];
        assert_eq!(t_primary.colors, f.colors);
        assert!(import("hello world", "x").is_err());
        assert!(import("{\"name\": \"x\"}", "x").is_err());
        assert!(import("<plist></plist>", "x").is_err());
    }
}
