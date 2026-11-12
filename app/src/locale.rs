//! Interface languages and embedded translation catalogs.
use eframe::egui::{Context, Id, Ui};
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Language(&'static str);

struct Catalog {
    code: &'static str,
    name: &'static str,
    strings: &'static str,
}

const CATALOGS: &[Catalog] = &[
    Catalog { code: "en", name: "English", strings: "{}" },
    Catalog { code: "pt-BR", name: "Português (Brasil)", strings: include_str!("../locales/pt-BR.json") },
];

impl Default for Language {
    fn default() -> Self {
        Self("en")
    }
}

impl Language {
    pub fn all() -> impl Iterator<Item = Self> {
        CATALOGS.iter().map(|catalog| Self(catalog.code))
    }

    pub fn from_code(code: &str) -> Self {
        Self::all().find(|language| language.0.eq_ignore_ascii_case(code)).unwrap_or_default()
    }

    pub fn code(self) -> &'static str {
        self.0
    }

    pub fn label(self) -> &'static str {
        CATALOGS.iter().find(|catalog| catalog.code == self.0).map_or("English", |catalog| catalog.name)
    }

    fn lookup(self, text: &str) -> &str {
        type Strings = HashMap<String, String>;
        static STRINGS: OnceLock<HashMap<&'static str, Strings>> = OnceLock::new();
        let catalogs = STRINGS.get_or_init(|| CATALOGS.iter().map(|catalog| {
            let entries = serde_json::from_str(catalog.strings).unwrap_or_default();
            (catalog.code, entries)
        }).collect());
        catalogs.get(self.0).and_then(|entries| entries.get(text))
            .filter(|translation| !translation.trim().is_empty()).map_or(text, String::as_str)
    }
}

fn locale_id() -> Id {
    Id::new("interface-language")
}

pub fn set(ctx: &Context, language: Language) {
    ctx.data_mut(|data| data.insert_temp(locale_id(), language));
}

pub fn text<'a>(ctx: &Context, source: &'a str) -> &'a str {
    ctx.data(|data| data.get_temp::<Language>(locale_id()))
        .unwrap_or_default().lookup(source)
}

pub fn tr<'a>(ui: &Ui, source: &'a str) -> &'a str {
    text(ui.ctx(), source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_parse_and_missing_translations_fall_back_to_english() {
        for catalog in CATALOGS {
            let strings: HashMap<String, String> = serde_json::from_str(catalog.strings)
                .unwrap_or_else(|error| panic!("{}: {error}", catalog.code));
            assert_eq!(Language::from_code(catalog.code).label(), catalog.name);
            assert!(strings.values().all(|text| !text.trim().is_empty()));
        }
        let ctx = Context::default();
        assert_eq!(text(&ctx, "Settings"), "Settings");
        set(&ctx, Language::from_code("pt-br"));
        assert_eq!(text(&ctx, "Settings"), "Configurações");
        assert_eq!(text(&ctx, "Untranslated"), "Untranslated");
        assert_eq!(Language::from_code("unknown").code(), "en");
    }
}
