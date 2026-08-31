use ratatui::style::Color;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    collections::BTreeMap,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThemeToken {
    Background,
    Surface,
    Header,
    Border,
    Text,
    Muted,
    Accent,
    Success,
    Warning,
    Error,
}
impl ThemeToken {
    pub const ALL: [Self; 10] = [
        Self::Background,
        Self::Surface,
        Self::Header,
        Self::Border,
        Self::Text,
        Self::Muted,
        Self::Accent,
        Self::Success,
        Self::Warning,
        Self::Error,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Surface => "surface",
            Self::Header => "header",
            Self::Border => "border",
            Self::Text => "text",
            Self::Muted => "muted",
            Self::Accent => "accent",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}
impl FromStr for ThemeToken {
    type Err = ThemeError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|token| token.as_str() == value)
            .ok_or_else(|| ThemeError::new("invalid_theme_token", "unknown semantic color"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorSpec {
    Rgb(u8, u8, u8),
    Ansi(u8),
    Named(String),
    Default,
}
impl ColorSpec {
    pub fn parse(value: &str) -> Result<Self, ThemeError> {
        if value == "default" {
            return Ok(Self::Default);
        };
        if let Some(hex) = value.strip_prefix('#') {
            if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Ok(Self::Rgb(
                    u8::from_str_radix(&hex[0..2], 16).expect("hex"),
                    u8::from_str_radix(&hex[2..4], 16).expect("hex"),
                    u8::from_str_radix(&hex[4..6], 16).expect("hex"),
                ));
            }
            return Err(ThemeError::new("invalid_theme_color", "expected #RRGGBB"));
        };
        if let Some(index) = value.strip_prefix("ansi:") {
            return index
                .parse::<u8>()
                .map(Self::Ansi)
                .map_err(|_| ThemeError::new("invalid_theme_color", "expected ansi index"));
        };
        let name = value.to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "black"
                | "red"
                | "green"
                | "yellow"
                | "blue"
                | "magenta"
                | "cyan"
                | "gray"
                | "darkgray"
                | "lightred"
                | "lightgreen"
                | "lightyellow"
                | "lightblue"
                | "lightmagenta"
                | "lightcyan"
                | "white"
        ) {
            Ok(Self::Named(name))
        } else {
            Err(ThemeError::new(
                "invalid_theme_color",
                "unknown color value",
            ))
        }
    }
    pub fn to_ratatui(&self) -> Color {
        match self {
            Self::Rgb(r, g, b) => Color::Rgb(*r, *g, *b),
            Self::Ansi(i) => Color::Indexed(*i),
            Self::Default => Color::Reset,
            Self::Named(n) => match n.as_str() {
                "black" => Color::Black,
                "red" => Color::Red,
                "green" => Color::Green,
                "yellow" => Color::Yellow,
                "blue" => Color::Blue,
                "magenta" => Color::Magenta,
                "cyan" => Color::Cyan,
                "gray" => Color::Gray,
                "darkgray" => Color::DarkGray,
                "lightred" => Color::LightRed,
                "lightgreen" => Color::LightGreen,
                "lightyellow" => Color::LightYellow,
                "lightblue" => Color::LightBlue,
                "lightmagenta" => Color::LightMagenta,
                "lightcyan" => Color::LightCyan,
                "white" => Color::White,
                _ => Color::Reset,
            },
        }
    }

    pub fn as_persisted(&self) -> String {
        match self {
            Self::Rgb(red, green, blue) => format!("#{red:02X}{green:02X}{blue:02X}"),
            Self::Ansi(index) => format!("ansi:{index}"),
            Self::Named(name) => name.clone(),
            Self::Default => "default".to_owned(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeError {
    code: &'static str,
    message: String,
}
impl ThemeError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub const fn code(&self) -> &'static str {
        self.code
    }
}
impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.code, self.message)
    }
}
impl std::error::Error for ThemeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YsdaTheme {
    pub name: String,
    pub background: Color,
    pub surface: Color,
    pub header: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
}
impl YsdaTheme {
    fn from_values(name: &str, values: [&str; 10]) -> Self {
        let c = values.map(|v| ColorSpec::parse(v).expect("built-in theme").to_ratatui());
        Self {
            name: name.to_owned(),
            background: c[0],
            surface: c[1],
            header: c[2],
            border: c[3],
            text: c[4],
            muted: c[5],
            accent: c[6],
            success: c[7],
            warning: c[8],
            error: c[9],
        }
    }
    fn set(&mut self, t: ThemeToken, c: Color) {
        match t {
            ThemeToken::Background => self.background = c,
            ThemeToken::Surface => self.surface = c,
            ThemeToken::Header => self.header = c,
            ThemeToken::Border => self.border = c,
            ThemeToken::Text => self.text = c,
            ThemeToken::Muted => self.muted = c,
            ThemeToken::Accent => self.accent = c,
            ThemeToken::Success => self.success = c,
            ThemeToken::Warning => self.warning = c,
            ThemeToken::Error => self.error = c,
        }
    }
    fn without_color(mut self) -> Self {
        for token in ThemeToken::ALL {
            self.set(token, Color::Reset)
        }
        self
    }
}
#[derive(Debug, Clone)]
pub struct ThemeRegistry {
    presets: BTreeMap<String, YsdaTheme>,
}
impl Default for ThemeRegistry {
    fn default() -> Self {
        let presets = [
            (
                "deep-navy",
                [
                    "#071423", "#101D2E", "#202A38", "#224B79", "#C5CAD3", "#7E8999", "#4389E6",
                    "#66BAD9", "#E0B866", "#E06C75",
                ],
            ),
            (
                "terminal",
                [
                    "default", "default", "default", "darkgray", "white", "gray", "cyan", "green",
                    "yellow", "red",
                ],
            ),
            (
                "nord",
                [
                    "#2E3440", "#3B4252", "#434C5E", "#4C566A", "#ECEFF4", "#D8DEE9", "#88C0D0",
                    "#A3BE8C", "#EBCB8B", "#BF616A",
                ],
            ),
            (
                "gruvbox",
                [
                    "#282828", "#3C3836", "#504945", "#665C54", "#EBDBB2", "#A89984", "#83A598",
                    "#B8BB26", "#FABD2F", "#FB4934",
                ],
            ),
        ]
        .into_iter()
        .map(|(n, c)| (n.to_owned(), YsdaTheme::from_values(n, c)))
        .collect();
        Self { presets }
    }
}
impl ThemeRegistry {
    pub fn resolve(&self, n: &str) -> Result<YsdaTheme, ThemeError> {
        self.presets
            .get(n)
            .cloned()
            .ok_or_else(|| ThemeError::new("invalid_theme_preset", "unknown theme preset"))
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.presets.keys().map(String::as_str)
    }
    pub fn resolve_preferences(
        &self,
        p: &UiPreferences,
        no_color: bool,
    ) -> Result<YsdaTheme, ThemeError> {
        let base = if p.theme == "custom" {
            "deep-navy"
        } else {
            &p.theme
        };
        let mut theme = self.resolve(base)?;
        theme.name = p.theme.clone();
        for (raw_token, raw_color) in &p.colors {
            theme.set(
                raw_token.parse()?,
                ColorSpec::parse(raw_color)?.to_ratatui(),
            )
        }
        Ok(if no_color {
            theme.without_color()
        } else {
            theme
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiPreferences {
    pub theme: String,
    pub colors: BTreeMap<String, String>,
}
impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            theme: "deep-navy".to_owned(),
            colors: BTreeMap::new(),
        }
    }
}
#[derive(Debug, Clone)]
pub struct UiPreferenceStore {
    path: PathBuf,
}
impl UiPreferenceStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn load(&self) -> Result<UiPreferences, ThemeError> {
        if !self.path.exists() {
            return Ok(UiPreferences::default());
        }
        toml::from_str(
            &fs::read_to_string(&self.path)
                .map_err(|e| ThemeError::new("theme_preferences_io", e.to_string()))?,
        )
        .map_err(|e| ThemeError::new("invalid_theme_preferences", e.to_string()))
    }
    pub fn persist(&self, p: &UiPreferences) -> Result<(), ThemeError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|e| ThemeError::new("theme_preferences_io", e.to_string()))?;
        #[cfg(unix)]
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|e| ThemeError::new("theme_preferences_io", e.to_string()))?;
        let temporary = self.path.with_extension("toml.tmp");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|e| ThemeError::new("theme_preferences_io", e.to_string()))?;
        file.write_all(
            toml::to_string_pretty(p)
                .map_err(|e| ThemeError::new("invalid_theme_preferences", e.to_string()))?
                .as_bytes(),
        )
        .and_then(|_| file.sync_all())
        .map_err(|e| ThemeError::new("theme_preferences_io", e.to_string()))?;
        fs::rename(temporary, &self.path)
            .map_err(|e| ThemeError::new("theme_preferences_io", e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ratatui::style::Color;
    use tempfile::tempdir;

    use super::{ColorSpec, ThemeRegistry, UiPreferenceStore, UiPreferences};

    #[test]
    fn deep_navy_uses_the_locked_ysda_values() {
        let theme = ThemeRegistry::default()
            .resolve("deep-navy")
            .expect("preset");
        assert_eq!(theme.background, Color::Rgb(0x07, 0x14, 0x23));
        assert_eq!(theme.accent, Color::Rgb(0x43, 0x89, 0xE6));
        assert_eq!(theme.success, Color::Rgb(0x66, 0xBA, 0xD9));
    }

    #[test]
    fn accepted_color_forms_parse_and_invalid_input_has_a_stable_code() {
        assert_eq!(
            ColorSpec::parse("#4389E6").expect("RGB"),
            ColorSpec::Rgb(0x43, 0x89, 0xE6)
        );
        assert_eq!(
            ColorSpec::parse("ansi:255").expect("ANSI"),
            ColorSpec::Ansi(255)
        );
        assert_eq!(
            ColorSpec::parse("default").expect("default"),
            ColorSpec::Default
        );
        assert_eq!(
            ColorSpec::parse("#GG0000").expect_err("invalid").code(),
            "invalid_theme_color"
        );
    }

    #[test]
    fn preferences_round_trip_atomically_and_no_color_resets_decoration() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join(".ysda/ui.toml");
        let store = UiPreferenceStore::new(&path);
        let mut preferences = UiPreferences {
            theme: "custom".to_owned(),
            ..UiPreferences::default()
        };
        preferences
            .colors
            .insert("accent".to_owned(), "#4389E6".to_owned());
        store.persist(&preferences).expect("persist preferences");
        assert_eq!(store.load().expect("load preferences"), preferences);
        let colored = ThemeRegistry::default()
            .resolve_preferences(&preferences, false)
            .expect("custom theme");
        assert_eq!(colored.accent, Color::Rgb(0x43, 0x89, 0xE6));
        let no_color = ThemeRegistry::default()
            .resolve_preferences(&preferences, true)
            .expect("NO_COLOR theme");
        assert_eq!(no_color.background, Color::Reset);
        assert_eq!(no_color.error, Color::Reset);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
                0o600
            );
        }
        fs::write(&path, "theme = [").expect("write invalid preference fixture");
        assert_eq!(
            store.load().expect_err("invalid TOML").code(),
            "invalid_theme_preferences"
        );
    }
}
